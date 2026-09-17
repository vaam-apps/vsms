#![doc = include_str!("dlr.md")]

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
        // `PENDING` is progress, not a verdict: the message is still in
        // flight through the aggregator/MTN handoff and nothing about its
        // fate has been decided. It maps to `InFlight`, which records a
        // receipt and leaves the message in `submitted` — *not* to
        // `Uncertain`, which would drive it into §7.4's `uncertain` state
        // and make a later retryable failure permanently `failed` (see
        // `DeliveryOutcome::InFlight`'s own doc, and Orange's identical
        // `MessageWaiting`/`DeliveredToNetwork` handling).
        "PENDING" => DeliveryOutcome::InFlight,
        // `UNCERTAIN` is the genuine absence of knowledge, the one this
        // assumed vocabulary shares with Orange's `DeliveryUncertain`.
        "UNCERTAIN" => DeliveryOutcome::Uncertain,
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

    /// `PENDING` and `UNCERTAIN` are both non-final, but they are not the
    /// same kind of non-final, and this test exists to keep them apart.
    /// Neither may be guessed into a terminal outcome; equally, `PENDING`
    /// must not become `Uncertain`, which would push a healthy in-flight
    /// message into §7.4's `uncertain` state and cost it its retry path on
    /// a later failure (`DeliveryOutcome::InFlight`'s own doc has the full
    /// mechanism).
    #[test]
    fn pending_is_progress_while_uncertain_is_an_absence_of_knowledge() {
        for (status, expected) in [
            ("PENDING", DeliveryOutcome::InFlight),
            ("UNCERTAIN", DeliveryOutcome::Uncertain),
        ] {
            let body = format!(r#"{{"messageId":"mtn-res-3","status":"{status}"}}"#);
            let updates = parse(&callback(&body)).unwrap();
            assert_eq!(
                updates[0].outcome, expected,
                "status {status} should map to {expected:?}"
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
