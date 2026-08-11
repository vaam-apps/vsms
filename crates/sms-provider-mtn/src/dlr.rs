//! Parsing the aggregator's delivery notification callback.
//!
//! **Not verified against a live aggregator sandbox** — see `lib.rs`'s
//! module doc for the full honesty ledger. The shape assumed here is a
//! single JSON object per callback (not batched — some providers batch,
//! per [`sms_provider::SmsProvider::parse_dlr`]'s own doc, but nothing in
//! `docs/architecture.md` suggests this route does, so the simpler shape
//! is the default until a real payload says otherwise):
//!
//! ```json
//! {
//!   "messageId": "mtn-res-42",
//!   "status": "DELIVERED",
//!   "errorCode": "...",
//!   "network": "mtn",
//!   "occurredAt": "2026-08-11T12:00:00Z"
//! }
//! ```
//!
//! `messageId` is the same value `submit()` (`lib.rs`) returns as
//! `SubmitAck::provider_ref` — unlike Orange, this assumed shape needs no
//! `callbackData`/`provider_ref_alt` workaround, because the invented
//! contract is that the aggregator echoes its own id back rather than a
//! caller-supplied correlation token. If a real aggregator's DLR turns out
//! to reference the submission a different way, this is precisely the kind
//! of correlation gap #95 already shows this codebase can ship silently —
//! revisit this module and `submit`'s `provider_ref_alt` together.

use sms_provider::{DeliveryOutcome, DeliveryUpdate, ProviderError, RawCallback};

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct DeliveryNotification {
    #[serde(rename = "messageId")]
    message_id: Option<String>,
    status: String,
    #[serde(rename = "errorCode")]
    error_code: Option<String>,
    network: Option<String>,
    #[serde(rename = "occurredAt")]
    occurred_at: Option<DateTime<Utc>>,
}

/// Map the aggregator's assumed status vocabulary onto [`DeliveryOutcome`].
///
/// Deliberately not exhaustive, matching `sms-provider-orange-cm::dlr`'s own
/// precedent: without a real aggregator's status list, guessing an
/// unrecognised value into `Delivered`/`Failed` risks acting on the wrong
/// outcome. Anything not explicitly recognised here falls to
/// [`DeliveryOutcome::Unknown`], a correct and honest answer.
fn outcome_of(status: &str) -> DeliveryOutcome {
    match status {
        "DELIVERED" => DeliveryOutcome::Delivered,
        "FAILED" => DeliveryOutcome::Failed,
        "EXPIRED" => DeliveryOutcome::Expired,
        "REJECTED" => DeliveryOutcome::Rejected,
        // A message still in flight through the aggregator/MTN handoff —
        // not resolved either way yet. Same "don't guess" reasoning as
        // Orange's own `DeliveryUncertain`/`MessageWaiting` handling.
        "PENDING" | "UNCERTAIN" => DeliveryOutcome::Uncertain,
        _ => DeliveryOutcome::Unknown,
    }
}

/// Parse a raw aggregator DLR callback body into a canonical delivery
/// update.
///
/// # Errors
///
/// The body isn't valid JSON in the expected shape, or it carries no
/// `messageId` at all — without it there is nothing to correlate against a
/// `Message`, the same reasoning `sms-provider-orange-cm::dlr::parse` uses
/// for its own required `callbackData` field.
pub(crate) fn parse(raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError> {
    let parsed: DeliveryNotification =
        serde_json::from_slice(&raw.body).map_err(|error| ProviderError::Rejected {
            code: "MALFORMED_DLR".to_owned(),
            message: format!("could not parse delivery notification: {error}"),
        })?;

    let Some(provider_ref) = parsed.message_id.filter(|id| !id.is_empty()) else {
        return Err(ProviderError::Rejected {
            code: "MISSING_MESSAGE_ID".to_owned(),
            message: "delivery notification carried no messageId to correlate against".to_owned(),
        });
    };

    let outcome = outcome_of(&parsed.status);

    Ok(vec![DeliveryUpdate {
        provider_ref,
        outcome,
        occurred_at: parsed.occurred_at,
        raw_status: parsed.status,
        error_code: parsed.error_code,
        delivering_network: parsed.network,
    }])
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
            r#"{"messageId":"mtn-res-1","status":"DELIVERED","network":"mtn"}"#,
        ))
        .unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].outcome, DeliveryOutcome::Delivered);
        assert_eq!(updates[0].provider_ref, "mtn-res-1");
        assert_eq!(updates[0].delivering_network.as_deref(), Some("mtn"));
    }

    #[test]
    fn parses_a_failed_notification_with_an_error_code() {
        let updates = parse(&callback(
            r#"{"messageId":"mtn-res-2","status":"FAILED","errorCode":"HANDSET_UNREACHABLE"}"#,
        ))
        .unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::Failed);
        assert_eq!(
            updates[0].error_code.as_deref(),
            Some("HANDSET_UNREACHABLE")
        );
    }

    #[test]
    fn pending_and_uncertain_both_map_to_uncertain_not_a_guess() {
        for status in ["PENDING", "UNCERTAIN"] {
            let body = format!(r#"{{"messageId":"mtn-res-3","status":"{status}"}}"#);
            let updates = parse(&callback(&body)).unwrap();
            assert_eq!(
                updates[0].outcome,
                DeliveryOutcome::Uncertain,
                "status {status} should map to Uncertain"
            );
        }
    }

    #[test]
    fn an_unrecognised_status_is_unknown_not_a_guess() {
        let updates = parse(&callback(
            r#"{"messageId":"mtn-res-4","status":"SomeFutureStatusThisCrateDoesNotKnowAbout"}"#,
        ))
        .unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::Unknown);
    }

    #[test]
    fn a_notification_with_no_message_id_is_rejected() {
        let error = parse(&callback(r#"{"status":"DELIVERED"}"#)).unwrap_err();
        assert!(matches!(
            error,
            sms_provider::ProviderError::Rejected { .. }
        ));
    }

    #[test]
    fn a_notification_with_an_empty_message_id_is_rejected() {
        let error = parse(&callback(r#"{"messageId":"","status":"DELIVERED"}"#)).unwrap_err();
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

    #[test]
    fn occurred_at_round_trips_when_present() {
        let updates = parse(&callback(
            r#"{"messageId":"mtn-res-5","status":"DELIVERED","occurredAt":"2026-08-11T12:00:00Z"}"#,
        ))
        .unwrap();
        assert!(updates[0].occurred_at.is_some());
    }
}
