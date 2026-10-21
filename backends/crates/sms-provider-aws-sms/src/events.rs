#![doc = include_str!("events.md")]

use chrono::{DateTime, Utc};
use serde::Deserialize;
use sms_provider::{DeliveryOutcome, DeliveryUpdate, ProviderError};

/// The fields of an End User Messaging SMS event record this adapter reads.
/// Everything else in the record (prices, `mcc`/`mnc`, `context`) is kept
/// verbatim in `DeliveryReceipt.rawPayload` by the caller and not parsed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    event_type: String,
    #[serde(default)]
    event_timestamp: Option<i64>,
    #[serde(default)]
    is_final: Option<bool>,
    message_id: String,
    #[serde(default)]
    message_status: Option<String>,
}

/// Parse the `Message` of one SNS notification into delivery updates.
///
/// A notification carries exactly one event, so the result has one element
/// for a text event and none for anything else (a voice or MMS event on a
/// shared configuration set is not ours to ingest, and is not an error
/// either — answering 400 would make SNS retry it forever).
///
/// # Errors
///
/// [`ProviderError::Rejected`] when `message` is not an event record at all.
pub(crate) fn parse(message: &str) -> Result<Vec<DeliveryUpdate>, ProviderError> {
    let event: Record = serde_json::from_str(message).map_err(|error| ProviderError::Rejected {
        code: "MALFORMED_EVENT".to_owned(),
        message: format!("SNS Message is not an End User Messaging SMS event: {error}"),
    })?;

    if !event.event_type.starts_with("TEXT_") {
        tracing::debug!(event_type = event.event_type, "ignoring a non-SMS event");
        return Ok(Vec::new());
    }
    if event.message_id.trim().is_empty() {
        return Err(ProviderError::Rejected {
            code: "MALFORMED_EVENT".to_owned(),
            message: "event carries an empty messageId".to_owned(),
        });
    }

    let outcome = outcome_of(&event.event_type, event.is_final == Some(true));
    let error_code = match outcome {
        DeliveryOutcome::Delivered | DeliveryOutcome::InFlight => None,
        _ => event.message_status.clone(),
    };

    Ok(vec![DeliveryUpdate {
        provider_ref: event.message_id,
        outcome,
        occurred_at: event
            .event_timestamp
            .and_then(DateTime::<Utc>::from_timestamp_millis),
        raw_status: event.event_type,
        error_code,
        delivering_network: None,
    }])
}

/// One event type onto one outcome. Only a final event can be a verdict
/// other than `TEXT_DELIVERED`; anything else is progress — see `events.md`
/// for why.
fn outcome_of(event_type: &str, is_final: bool) -> DeliveryOutcome {
    match event_type {
        "TEXT_DELIVERED" => DeliveryOutcome::Delivered,
        _ if !is_final => DeliveryOutcome::InFlight,
        "TEXT_SUCCESSFUL" => DeliveryOutcome::Delivered,
        "TEXT_QUEUED" | "TEXT_PENDING" | "TEXT_SENT" => DeliveryOutcome::InFlight,
        "TEXT_INVALID"
        | "TEXT_INVALID_MESSAGE"
        | "TEXT_BLOCKED"
        | "TEXT_CARRIER_BLOCKED"
        | "TEXT_SPAM"
        | "TEXT_PROTECT_BLOCKED" => DeliveryOutcome::Rejected,
        "TEXT_UNREACHABLE" | "TEXT_CARRIER_UNREACHABLE" => DeliveryOutcome::Failed,
        "TEXT_TTL_EXPIRED" => DeliveryOutcome::Expired,
        "TEXT_UNKNOWN" => DeliveryOutcome::Uncertain,
        _ => DeliveryOutcome::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::{outcome_of, parse};
    use chrono::{TimeZone, Utc};
    use sms_provider::{DeliveryOutcome, ProviderError};

    /// AWS's own example SMS event record, verbatim from
    /// <https://docs.aws.amazon.com/sms-voice/latest/userguide/configuration-sets-event-format.html>.
    const AWS_EXAMPLE: &str = r#"{
        "eventType": "TEXT_SUCCESSFUL",
        "eventVersion": "1.0",
        "eventTimestamp": 1686975103470,
        "isFinal": true,
        "originationPhoneNumber": "+12065550152",
        "destinationPhoneNumber": "+14255550156",
        "isInternationalSend": false,
        "mcc": "310",
        "mnc": "800",
        "messageId": "862a8790-60c0-4430-9b2b-658bdexample",
        "messageRequestTimestamp": 1686975103170,
        "messageEncoding": "GSM",
        "messageType": "PROMOTIONAL",
        "messageStatus": "SUCCESSFUL",
        "messageStatusDescription": "Message has been accepted by phone carrier",
        "context": {"account": "bar"},
        "totalMessageParts": 1,
        "totalMessagePrice": 0.09582,
        "totalCarrierFee": 0.0
    }"#;

    #[test]
    fn parses_aws_own_example_record() {
        let updates = parse(AWS_EXAMPLE).unwrap();
        assert_eq!(updates.len(), 1);
        let update = &updates[0];
        assert_eq!(update.provider_ref, "862a8790-60c0-4430-9b2b-658bdexample");
        assert_eq!(update.outcome, DeliveryOutcome::Delivered);
        assert_eq!(update.raw_status, "TEXT_SUCCESSFUL");
        assert_eq!(update.error_code, None);
        assert_eq!(
            update.occurred_at,
            Some(Utc.timestamp_millis_opt(1_686_975_103_470).unwrap())
        );
    }

    /// The one mapping that depends on more than the event type. A
    /// carrier acceptance that AWS says is the last word is the best
    /// confirmation this route will ever give; one that isn't is progress.
    #[test]
    fn carrier_acceptance_is_delivered_only_when_final() {
        assert_eq!(
            outcome_of("TEXT_SUCCESSFUL", true),
            DeliveryOutcome::Delivered
        );
        assert_eq!(
            outcome_of("TEXT_SUCCESSFUL", false),
            DeliveryOutcome::InFlight
        );
    }

    #[test]
    fn every_documented_text_event_type_has_a_deliberate_outcome() {
        // The EventType enum from botocore's pinpoint-sms-voice-v2 model,
        // minus TEXT_ALL (a subscription filter, never an event).
        let cases = [
            ("TEXT_SENT", DeliveryOutcome::InFlight),
            ("TEXT_PENDING", DeliveryOutcome::InFlight),
            ("TEXT_QUEUED", DeliveryOutcome::InFlight),
            ("TEXT_DELIVERED", DeliveryOutcome::Delivered),
            ("TEXT_INVALID", DeliveryOutcome::Rejected),
            ("TEXT_INVALID_MESSAGE", DeliveryOutcome::Rejected),
            ("TEXT_UNREACHABLE", DeliveryOutcome::Failed),
            ("TEXT_CARRIER_UNREACHABLE", DeliveryOutcome::Failed),
            ("TEXT_BLOCKED", DeliveryOutcome::Rejected),
            ("TEXT_CARRIER_BLOCKED", DeliveryOutcome::Rejected),
            ("TEXT_SPAM", DeliveryOutcome::Rejected),
            ("TEXT_UNKNOWN", DeliveryOutcome::Uncertain),
            ("TEXT_TTL_EXPIRED", DeliveryOutcome::Expired),
            ("TEXT_PROTECT_BLOCKED", DeliveryOutcome::Rejected),
        ];
        for (event_type, expected) in cases {
            assert_eq!(outcome_of(event_type, true), expected, "{event_type}");
        }
        assert_eq!(
            outcome_of("TEXT_SOMETHING_NEW", true),
            DeliveryOutcome::Unknown
        );
    }

    /// A failure that is not final may still be followed by a delivery, so
    /// treating it as a verdict could resubmit a message that then arrives:
    /// a duplicate SMS. `TEXT_DELIVERED` is the one event that needs no
    /// finality.
    #[test]
    fn a_non_final_failure_is_progress_not_a_verdict() {
        for event_type in [
            "TEXT_INVALID",
            "TEXT_INVALID_MESSAGE",
            "TEXT_UNREACHABLE",
            "TEXT_CARRIER_UNREACHABLE",
            "TEXT_BLOCKED",
            "TEXT_CARRIER_BLOCKED",
            "TEXT_SPAM",
            "TEXT_UNKNOWN",
            "TEXT_TTL_EXPIRED",
            "TEXT_PROTECT_BLOCKED",
            "TEXT_SOMETHING_NEW",
        ] {
            assert_eq!(
                outcome_of(event_type, false),
                DeliveryOutcome::InFlight,
                "{event_type}"
            );
        }
        assert_eq!(
            outcome_of("TEXT_DELIVERED", false),
            DeliveryOutcome::Delivered
        );
    }

    #[test]
    fn a_failure_with_no_is_final_field_is_treated_as_not_final() {
        let updates = parse(
            r#"{"eventType":"TEXT_CARRIER_UNREACHABLE","messageId":"m-1","messageStatus":"CARRIER_UNREACHABLE"}"#,
        )
        .unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::InFlight);
        assert_eq!(updates[0].error_code, None);
    }

    #[test]
    fn a_failure_carries_the_status_as_its_error_code() {
        let updates = parse(
            r#"{"eventType":"TEXT_CARRIER_UNREACHABLE","isFinal":true,"messageId":"m-1","messageStatus":"CARRIER_UNREACHABLE"}"#,
        )
        .unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::Failed);
        assert_eq!(
            updates[0].error_code.as_deref(),
            Some("CARRIER_UNREACHABLE")
        );
    }

    #[test]
    fn a_non_text_event_is_ignored_not_rejected() {
        let updates = parse(r#"{"eventType":"VOICE_COMPLETED","messageId":"v-1"}"#).unwrap();
        assert_eq!(updates.len(), 0);
    }

    #[test]
    fn a_body_that_is_not_an_event_is_rejected() {
        assert!(matches!(
            parse("not json"),
            Err(ProviderError::Rejected { code, .. }) if code == "MALFORMED_EVENT"
        ));
        assert!(matches!(
            parse(r#"{"eventType":"TEXT_DELIVERED","messageId":"  "}"#),
            Err(ProviderError::Rejected { .. })
        ));
    }
}
