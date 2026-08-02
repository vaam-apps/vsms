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
    /// The resource reference from submission — our DLR correlation key.
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
