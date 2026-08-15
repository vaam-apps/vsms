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
    /// Echoed back verbatim from the `callbackData` `submit()` sets on its
    /// own `receiptRequest` (`lib.rs`) — `Message.id`, via
    /// `SubmitRequest::reference`. The *only* correlation key this
    /// notification carries; nothing in `deliveryInfo` itself identifies
    /// which message it's about. See the module doc for why this replaced
    /// the old (never-working) per-entry `address` approach.
    #[serde(rename = "callbackData")]
    callback_data: Option<String>,
    #[serde(rename = "deliveryInfo")]
    delivery_info: Vec<DeliveryInfoEntry>,
}

#[derive(Debug, Deserialize)]
struct DeliveryInfoEntry {
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
/// # Errors
///
/// The body isn't valid JSON in the expected shape, or (per #95's fix) it
/// carries no `callbackData` at all — without it there is nothing to set
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
            message: "delivery notification carried no callbackData to correlate against"
                .to_owned(),
        });
    };

    Ok(parsed
        .notification
        .delivery_info
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
            r#"{"deliveryInfoNotification":{"callbackData":"msg-1","deliveryInfo":[
                {"address":"tel:+237677123456","deliveryStatus":"DeliveredToTerminal"}
            ]}}"#,
        ))
        .unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].outcome, DeliveryOutcome::Delivered);
        assert_eq!(updates[0].provider_ref, "msg-1");
    }

    #[test]
    fn parses_multiple_entries_in_one_callback() {
        let updates = parse(&callback(
            r#"{"deliveryInfoNotification":{"callbackData":"msg-1","deliveryInfo":[
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
            r#"{"deliveryInfoNotification":{"callbackData":"msg-1","deliveryInfo":[
                {"address":"a","deliveryStatus":"SomeFutureStatusThisCrateDoesNotKnowAbout"}
            ]}}"#,
        ))
        .unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::Unknown);
    }

    /// The core of #95's fix — proving `provider_ref` now comes from
    /// `callbackData`, not the destination address, and that two
    /// different messages' notifications are actually distinguishable.
    #[test]
    fn provider_ref_is_the_callback_data_not_the_address_and_differs_per_message() {
        let a = parse(&callback(
            r#"{"deliveryInfoNotification":{"callbackData":"msg-a","deliveryInfo":[
                {"address":"tel:+237677000000","deliveryStatus":"DeliveredToTerminal"}
            ]}}"#,
        ))
        .unwrap();
        let b = parse(&callback(
            r#"{"deliveryInfoNotification":{"callbackData":"msg-b","deliveryInfo":[
                {"address":"tel:+237677000000","deliveryStatus":"DeliveredToTerminal"}
            ]}}"#,
        ))
        .unwrap();
        // Same destination address on both — the old, broken correlation
        // key would have collided. The new one doesn't.
        assert_eq!(a[0].provider_ref, "msg-a");
        assert_eq!(b[0].provider_ref, "msg-b");
        assert_ne!(a[0].provider_ref, b[0].provider_ref);
    }

    #[test]
    fn a_notification_with_no_callback_data_is_rejected_not_matched_by_address() {
        let error = parse(&callback(
            r#"{"deliveryInfoNotification":{"deliveryInfo":[
                {"address":"tel:+237677123456","deliveryStatus":"DeliveredToTerminal"}
            ]}}"#,
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
