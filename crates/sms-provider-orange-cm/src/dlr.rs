//! Parsing Orange's delivery notification callback.
//!
//! **Not verified against a live Orange sandbox** — this repo has no Orange
//! Developer credentials, and §6.2 documents the submit path in detail but
//! not the DLR callback's JSON shape. What's implemented here follows the
//! `deliveryInfoNotification` shape common to the GSMA `OneAPI` SMS family
//! Orange's own outbound API belongs to (the same lineage as the
//! `outboundSMSMessageRequest` shape §6.2 *does* specify for submission).
//! Treat this module as the best available design until it can be checked
//! against a real callback payload, and add a fixture from Orange's sandbox
//! the moment one exists — see `parses_a_delivered_notification` below for
//! where it would slot in.
//!
//! **Known-broken, not just unverified: [`DeliveryUpdate::provider_ref`]
//! cannot correlate against `Message.providerMessageRef` yet.** `submit()`
//! (`lib.rs`) stores the `resource_id` UUID from `resourceURL` as
//! `providerMessageRef`. The `OneAPI` `deliveryInfoNotification` shape's
//! per-entry `address` field — the only per-entry identifier this shape
//! carries — is the *destination MSISDN*, not that UUID (confirmed by this
//! module's own test fixture: `"tel:+237677123456"` is a phone number, not
//! a resource id). A UUID and a phone number will never match, so every DLR
//! parsed by this module today would silently fail to find its message.
//! Caught in review (#94) by two independent bots agreeing on the same root
//! cause. Harmless right now — nothing calls [`parse`] outside this
//! module's own tests, since `dispatch` (§7.1) is still a stub — but this
//! **must** be resolved, most likely by echoing a correlation id through
//! `OneAPI`'s subscription-level `callbackData` (set when registering the
//! webhook, not derivable from this callback body alone), before any DLR
//! receiver route is wired to this adapter. Tracked in
//! [#95](https://github.com/vymalo/vsms/issues/95).

use sms_provider::{DeliveryOutcome, DeliveryUpdate, ProviderError, RawCallback};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct DeliveryInfoNotification {
    #[serde(rename = "deliveryInfoNotification")]
    notification: DeliveryInfo,
}

#[derive(Debug, Deserialize)]
struct DeliveryInfo {
    #[serde(rename = "deliveryInfo")]
    delivery_info: Vec<DeliveryInfoEntry>,
}

#[derive(Debug, Deserialize)]
struct DeliveryInfoEntry {
    /// The destination MSISDN, e.g. `"tel:+237677123456"` — **not** a
    /// correlation key, despite that being the obvious guess for the one
    /// per-entry identifier this shape carries. See the module doc: using
    /// this as `provider_ref` is a known, tracked bug (#95), kept here only
    /// because the field is real and worth carrying even though it can't
    /// do correlation duty.
    address: String,
    #[serde(rename = "deliveryStatus")]
    delivery_status: String,
}

/// Map Orange's own status vocabulary onto [`DeliveryOutcome`].
///
/// Deliberately exhaustive-looking but not actually exhaustive — Orange's
/// status set is not something this crate can enumerate with confidence
/// without a live sandbox, so anything not explicitly recognised falls to
/// [`DeliveryOutcome::Unknown`] rather than being guessed into `Delivered`
/// or `Failed`. `Unknown` is a correct, honest answer here; a wrong guess
/// is not.
fn outcome_of(status: &str) -> DeliveryOutcome {
    match status {
        "DeliveredToTerminal" => DeliveryOutcome::Delivered,
        "DeliveryImpossible" => DeliveryOutcome::Failed,
        "DeliveryUncertain" | "MessageWaiting" | "DeliveredToNetwork" => DeliveryOutcome::Uncertain,
        _ => DeliveryOutcome::Unknown,
    }
}

/// Parse a raw Orange DLR callback body into canonical delivery updates.
///
/// `provider_ref` is set from `address` below purely so the field is
/// populated with *something* real from the payload — it is not, and
/// cannot yet be, the correlation key `Message.providerMessageRef` needs.
/// See the module doc and #95: nothing in this crate calls `parse` outside
/// its own tests today, so this is a documented gap to close before
/// wiring, not a silent one.
pub(crate) fn parse(raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError> {
    let parsed: DeliveryInfoNotification =
        serde_json::from_slice(&raw.body).map_err(|error| ProviderError::Rejected {
            code: "MALFORMED_DLR".to_owned(),
            message: format!("could not parse delivery notification: {error}"),
        })?;

    Ok(parsed
        .notification
        .delivery_info
        .into_iter()
        .map(|entry| {
            let outcome = outcome_of(&entry.delivery_status);
            DeliveryUpdate {
                provider_ref: entry.address,
                outcome,
                // Orange's notification doesn't carry an event timestamp in
                // this shape — the caller stamps arrival time
                // (`DeliveryReceipt.receivedAt`) instead.
                occurred_at: None,
                raw_status: entry.delivery_status,
                error_code: None,
                // The GSMA OneAPI shape this module follows carries no
                // delivering-network field — same "unverified against a
                // live sandbox" caveat as the rest of this module.
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

    #[test]
    fn parses_a_delivered_notification() {
        let updates = parse(&callback(
            r#"{"deliveryInfoNotification":{"deliveryInfo":[
                {"address":"tel:+237677123456","deliveryStatus":"DeliveredToTerminal"}
            ]}}"#,
        ))
        .unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].outcome, DeliveryOutcome::Delivered);
        assert_eq!(updates[0].provider_ref, "tel:+237677123456");
    }

    #[test]
    fn parses_multiple_entries_in_one_callback() {
        let updates = parse(&callback(
            r#"{"deliveryInfoNotification":{"deliveryInfo":[
                {"address":"a","deliveryStatus":"DeliveredToTerminal"},
                {"address":"b","deliveryStatus":"DeliveryImpossible"}
            ]}}"#,
        ))
        .unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].outcome, DeliveryOutcome::Delivered);
        assert_eq!(updates[1].outcome, DeliveryOutcome::Failed);
    }

    #[test]
    fn an_unrecognised_status_is_unknown_not_a_guess() {
        let updates = parse(&callback(
            r#"{"deliveryInfoNotification":{"deliveryInfo":[
                {"address":"a","deliveryStatus":"SomeFutureStatusThisCrateDoesNotKnowAbout"}
            ]}}"#,
        ))
        .unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::Unknown);
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
