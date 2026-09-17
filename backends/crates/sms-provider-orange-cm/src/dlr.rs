#![doc = include_str!("dlr.md")]

use sms_provider::{DeliveryOutcome, DeliveryUpdate, ProviderError, RawCallback};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct DeliveryInfoNotification {
    #[serde(rename = "deliveryInfoNotification")]
    notification: DeliveryInfo,
}

#[derive(Debug, Deserialize)]
struct DeliveryInfo {
    /// Orange's own `{{resource_id}}` (<https://developer.orange.com/apis/sms/getting-started>,
    /// §4 "About SMS Delivery Receipt") — the same id `submit()` (`lib.rs`)
    /// recovers from `resourceURL`'s trailing path segment and stores as
    /// `Message.providerMessageRef`. Orange's own words: "the unique ID of
    /// your previously well sent SMS that you will need to use to
    /// correlate the corresponding SMS." It is Orange's value, minted by
    /// Orange, never a caller-supplied token — the documented submit
    /// request has no field for one at all. The *only* correlation key
    /// this notification carries; nothing in `deliveryInfo` itself
    /// identifies which message it's about.
    #[serde(rename = "callbackData")]
    callback_data: Option<String>,
    /// §4's own sample body shows `deliveryInfo` as a single JSON OBJECT,
    /// sibling to `callbackData`, not an array — see
    /// [`DeliveryInfoField`]'s own doc for why this still accepts an array
    /// too.
    #[serde(rename = "deliveryInfo")]
    delivery_info: DeliveryInfoField,
}

/// §4's documented DR body carries `deliveryInfo` as one JSON object, not
/// an array — the shape this module used to assume (inherited from the
/// wider GSMA `OneAPI` family, never confirmed against Orange's own docs)
/// was wrong, and a real single-object notification would have failed
/// `serde_json::from_slice` outright, rejecting every real DLR as
/// `MALFORMED_DLR` (400) before this fix. Accepted leniently, both shapes:
/// the object form because that's what the docs actually show; the array
/// form kept too because it costs nothing to keep accepting and guards
/// against a batched variant Orange's docs don't rule out (a partner
/// receiving several delivery events in one callback is a plausible
/// future/undocumented behaviour, not a shape worth hard-failing on if it
/// shows up).
///
/// Declaration order matters for `#[serde(untagged)]` — serde tries each
/// variant in order and keeps the first that deserializes successfully.
/// `One` must come first: a JSON array can never deserialize into
/// `DeliveryInfoEntry` (a struct, i.e. a JSON object), so it falls through
/// to `Many` correctly; a JSON object could in principle also satisfy a
/// single-element sequence deserializer if the ordering were reversed and
/// serde's untagged machinery were less strict, so this order is asserted
/// by `both_shapes_of_delivery_info_parse_to_the_same_outcome` below rather
/// than left to be "probably fine."
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DeliveryInfoField {
    One(DeliveryInfoEntry),
    Many(Vec<DeliveryInfoEntry>),
}

impl DeliveryInfoField {
    fn into_entries(self) -> Vec<DeliveryInfoEntry> {
        match self {
            Self::One(entry) => vec![entry],
            Self::Many(entries) => entries,
        }
    }
}

#[derive(Debug, Deserialize)]
struct DeliveryInfoEntry {
    #[serde(rename = "deliveryStatus")]
    delivery_status: String,
}

/// Map Orange's own status vocabulary onto [`DeliveryOutcome`].
///
/// Grounded in §4 "About SMS Delivery Receipt"'s own table
/// (<https://developer.orange.com/apis/sms/getting-started>), which
/// documents exactly five values — no longer a guess against the wider
/// GSMA `OneAPI` family. `DeliveredToTerminal` and `DeliveryImpossible`
/// are the two terminal-shaped ones. The other three are all non-final,
/// but they are not non-final in the *same way*, and the difference is
/// load-bearing rather than a nicety:
///
/// - `DeliveryUncertain` is a genuine absence of knowledge — Orange's own
///   gloss is "Delivery status unknown: e.g. because it was handed off to
///   another network." Nobody can say what happened, which is exactly what
///   [`DeliveryOutcome::Uncertain`] and §7.4's `uncertain` state mean.
/// - `DeliveredToNetwork` ("Successful delivery to network") and
///   `MessageWaiting` ("still queued for delivery. This is a temporary
///   state, pending transition to one of the preceding states") are
///   *progress*. Orange is reporting that the happy path is proceeding
///   normally. They map to [`DeliveryOutcome::InFlight`], which records a
///   receipt and changes no state.
///
/// Collapsing those last two into `Uncertain` is what this crate did until
/// the `deliveryInfo` parse bug was fixed and real DLRs could reach this
/// function at all. It was never observed, because no real Orange DLR had
/// ever parsed — and it was not cosmetic: see
/// [`DeliveryOutcome::InFlight`]'s own doc for how it silently destroyed a
/// message's retry path.
/// Anything outside that five-value vocabulary — a status this crate
/// doesn't recognise at all — still falls to [`DeliveryOutcome::Unknown`]
/// rather than being guessed into `Delivered` or `Failed`: the vocabulary
/// is now documented, but "documented" isn't "closed forever," and
/// `Unknown` costs nothing to keep as the honest default for whatever
/// Orange adds next.
///
/// `DeliveryImpossible` maps to `Failed`, not a stronger terminal
/// "the SMS never got there." Orange's own text is explicit that it isn't
/// definitive: "behind a `DeliveryImpossible` status, your SMS can still
/// be well delivered (e.g. if a phone has not reached a network for more
/// than 24 hours or has no battery); or you may get this status if you
/// are sending an SMS to a landline number or to a number that is no
/// longer in use." `Failed` is still the right mapping — this crate has
/// no way to distinguish "genuinely undeliverable" from "temporarily
/// unreachable, might still arrive" from a single DLR — but the
/// consequence is deliberately left to §7.4's own transition table and
/// `claim.rs`'s `undelivered`/retry machinery downstream, not decided
/// here: a `Failed` outcome can still resolve onward (retry, then a real
/// terminal state) rather than this function silently treating
/// `DeliveryImpossible` as an unrecoverable truth.
fn outcome_of(status: &str) -> DeliveryOutcome {
    match status {
        "DeliveredToTerminal" => DeliveryOutcome::Delivered,
        "DeliveryImpossible" => DeliveryOutcome::Failed,
        "DeliveryUncertain" => DeliveryOutcome::Uncertain,
        "DeliveredToNetwork" | "MessageWaiting" => DeliveryOutcome::InFlight,
        _ => DeliveryOutcome::Unknown,
    }
}

/// Parse a raw Orange DLR callback body into canonical delivery updates.
///
/// # Errors
///
/// The body isn't valid JSON in the expected shape, or (per #95's original
/// fix, still true against the documented shape) it carries no
/// `callbackData` at all — without it there is nothing to set
/// `provider_ref` to, so this rejects the whole notification rather than
/// silently falling back to a per-entry field (`address`) that can never
/// correlate to a `Message`, which is the exact bug this replaced.
pub(crate) fn parse(raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError> {
    let parsed: DeliveryInfoNotification =
        serde_json::from_slice(&raw.body).map_err(|error| ProviderError::Rejected {
            code: "MALFORMED_DLR".to_owned(),
            message: format!("could not parse delivery notification: {error}"),
        })?;

    let Some(provider_ref) = parsed.notification.callback_data else {
        return Err(ProviderError::Rejected {
            code: "MISSING_CALLBACK_DATA".to_owned(),
            message: "delivery notification carried no callbackData (resource_id) to correlate \
                      against"
                .to_owned(),
        });
    };

    Ok(parsed
        .notification
        .delivery_info
        .into_entries()
        .into_iter()
        .map(|entry| {
            let outcome = outcome_of(&entry.delivery_status);
            DeliveryUpdate {
                provider_ref: provider_ref.clone(),
                outcome,
                // Orange's notification doesn't carry an event timestamp in
                // this shape — the caller stamps arrival time
                // (`DeliveryReceipt.receivedAt`) instead.
                occurred_at: None,
                raw_status: entry.delivery_status,
                error_code: None,
                // §4's documented body carries no delivering-network field
                // either.
                delivering_network: None,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::parse;
    use sms_provider::{DeliveryOutcome, RawCallback};

    fn callback(body: &str) -> RawCallback {
        RawCallback {
            headers: vec![],
            body: body.as_bytes().to_vec(),
        }
    }

    /// §4's own documented shape, verbatim: `deliveryInfo` is a single
    /// object, not an array.
    #[test]
    fn parses_the_documented_single_object_shape() {
        let updates = parse(&callback(
            r#"{"deliveryInfoNotification":{"callbackData":"res-1","deliveryInfo":
                {"address":"tel:+237677123456","deliveryStatus":"DeliveredToTerminal"}
            }}"#,
        ))
        .unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].outcome, DeliveryOutcome::Delivered);
        assert_eq!(updates[0].provider_ref, "res-1");
    }

    /// The array shape isn't documented by Orange, but costs nothing to
    /// keep accepting — see [`super::DeliveryInfoField`]'s own doc.
    #[test]
    fn also_accepts_the_undocumented_array_shape() {
        let updates = parse(&callback(
            r#"{"deliveryInfoNotification":{"callbackData":"res-1","deliveryInfo":[
                {"address":"a","deliveryStatus":"DeliveredToTerminal"},
                {"address":"b","deliveryStatus":"DeliveryImpossible"}
            ]}}"#,
        ))
        .unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].outcome, DeliveryOutcome::Delivered);
        assert_eq!(updates[1].outcome, DeliveryOutcome::Failed);
    }

    /// Proves `#[serde(untagged)]`'s variant ordering actually works both
    /// ways, rather than trusting the reasoning in
    /// [`super::DeliveryInfoField`]'s own doc comment. If a future serde
    /// upgrade or a reordering of the enum's variants ever changed which
    /// shape wins, this is what would catch it.
    #[test]
    fn both_shapes_of_delivery_info_parse_to_the_same_outcome() {
        let object_shape = parse(&callback(
            r#"{"deliveryInfoNotification":{"callbackData":"res-1","deliveryInfo":
                {"address":"a","deliveryStatus":"DeliveredToTerminal"}
            }}"#,
        ))
        .unwrap();
        let array_shape = parse(&callback(
            r#"{"deliveryInfoNotification":{"callbackData":"res-1","deliveryInfo":[
                {"address":"a","deliveryStatus":"DeliveredToTerminal"}
            ]}}"#,
        ))
        .unwrap();
        assert_eq!(object_shape.len(), 1);
        assert_eq!(array_shape.len(), 1);
        assert_eq!(object_shape[0].outcome, array_shape[0].outcome);
        assert_eq!(object_shape[0].provider_ref, array_shape[0].provider_ref);
    }

    /// Every one of §4's five documented `deliveryStatus` values maps as
    /// the table says.
    #[test]
    fn every_documented_status_value_maps_correctly() {
        let cases = [
            ("DeliveredToTerminal", DeliveryOutcome::Delivered),
            ("DeliveryImpossible", DeliveryOutcome::Failed),
            ("DeliveryUncertain", DeliveryOutcome::Uncertain),
            // Progress, not a verdict — these two must NOT be `Uncertain`.
            // See `outcome_of`'s own doc: mapping them to `Uncertain` puts a
            // healthy in-flight message into §7.4's `uncertain` state, from
            // which `undelivered` is unreachable, so a later retryable
            // failure becomes a permanent `failed` instead of a retry.
            ("MessageWaiting", DeliveryOutcome::InFlight),
            ("DeliveredToNetwork", DeliveryOutcome::InFlight),
        ];
        for (status, expected) in cases {
            let updates = parse(&callback(&format!(
                r#"{{"deliveryInfoNotification":{{"callbackData":"res-1","deliveryInfo":
                    {{"address":"a","deliveryStatus":"{status}"}}
                }}}}"#,
            )))
            .unwrap();
            assert_eq!(
                updates[0].outcome, expected,
                "status {status} expected to map to {expected:?}"
            );
        }
    }

    #[test]
    fn an_unrecognised_status_is_unknown_not_a_guess() {
        let updates = parse(&callback(
            r#"{"deliveryInfoNotification":{"callbackData":"res-1","deliveryInfo":
                {"address":"a","deliveryStatus":"SomeFutureStatusThisCrateDoesNotKnowAbout"}
            }}"#,
        ))
        .unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::Unknown);
    }

    /// `provider_ref` comes from `callbackData` (Orange's own
    /// `resource_id`), and two different submissions' notifications are
    /// actually distinguishable by it.
    #[test]
    fn provider_ref_is_the_callback_data_and_differs_per_submission() {
        let a = parse(&callback(
            r#"{"deliveryInfoNotification":{"callbackData":"res-a","deliveryInfo":
                {"address":"tel:+237677000000","deliveryStatus":"DeliveredToTerminal"}
            }}"#,
        ))
        .unwrap();
        let b = parse(&callback(
            r#"{"deliveryInfoNotification":{"callbackData":"res-b","deliveryInfo":
                {"address":"tel:+237677000000","deliveryStatus":"DeliveredToTerminal"}
            }}"#,
        ))
        .unwrap();
        // Same destination address on both — a correlation key derived
        // from `address` would have collided. `callbackData` doesn't.
        assert_eq!(a[0].provider_ref, "res-a");
        assert_eq!(b[0].provider_ref, "res-b");
        assert_ne!(a[0].provider_ref, b[0].provider_ref);
    }

    #[test]
    fn a_notification_with_no_callback_data_is_rejected_not_matched_by_address() {
        let error = parse(&callback(
            r#"{"deliveryInfoNotification":{"deliveryInfo":
                {"address":"tel:+237677123456","deliveryStatus":"DeliveredToTerminal"}
            }}"#,
        ))
        .unwrap_err();
        assert!(matches!(
            error,
            sms_provider::ProviderError::Rejected { .. }
        ));
    }

    #[test]
    fn malformed_json_is_rejected_not_a_panic() {
        let error = parse(&callback("not json at all")).unwrap_err();
        assert!(matches!(
            error,
            sms_provider::ProviderError::Rejected { .. }
        ));
    }
}
