#![doc = include_str!("dlr.md")]

use sms_provider::{DeliveryOutcome, DeliveryUpdate, ProviderError, RawCallback};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tracing::warn;

/// MADAPI's real `DeliveryNotificationRequest`
/// (`mtn-sms-v3-swagger.yaml`, `definitions.DeliveryNotificationRequest`)
/// — every field marked optional in the schema, no `required` list at
/// all. `id`/`senderAddress`/`receiverAddress`/`submittedDate` are part
/// of the real shape but have no counterpart on [`DeliveryUpdate`] this
/// crate could populate, so they're left undeserialized (serde ignores
/// unknown wire fields by default — nothing here needs `deny_unknown_fields`).
#[derive(Debug, Deserialize)]
struct DeliveryNotification {
    #[serde(rename = "clientCorrelatorId")]
    client_correlator_id: Option<String>,
    #[serde(rename = "deliveryStatus")]
    delivery_status: Option<String>,
    details: Option<String>,
    #[serde(rename = "completedDate")]
    completed_date: Option<String>,
    error: Option<String>,
}

/// Map MADAPI's real, documented eight-value `status` enum
/// (`mtn-sms-v3-swagger.yaml`, `definitions.status`) onto
/// [`DeliveryOutcome`]. Every known value gets its own arm — no wildcard
/// covers any of the eight — so a future edit that adds a ninth real
/// value can't silently fall through to the trailing `_ => Unknown` and
/// look identical to a genuinely unrecognised string.
///
/// Two of the eight are not the obvious mapping:
///
/// - `ACCEPTD`/`ENROUTE` are progress, not verdicts — MADAPI's own
///   `status` title calls this "Delivery status", but SMPP (which this
///   vocabulary is visibly truncated from: `ACCEPTD` is `ACCEPTED`
///   trimmed to seven characters, the SMPP `message_state` convention)
///   uses both to mean "still in the pipeline, nothing decided yet."
///   [`DeliveryOutcome::InFlight`]'s own doc is explicit about why
///   folding progress into [`DeliveryOutcome::Uncertain`] is the bug
///   this variant exists to prevent: `Uncertain` starts §7.4's
///   `uncertain` state and its 6h expiry timer, and `uncertain ->
///   undelivered` is not a legal transition — a routine in-flight report
///   followed by a genuine, retryable failure would land the message in
///   `failed` permanently, with no retry, instead of `undelivered` with
///   one.
/// - `DELETED` is SMPP's own "removed from the delivery queue before an
///   attempt completed" — a deliberate administrative removal, not a
///   delivery attempt that failed and could be retried, and not a
///   passive timeout either. Mapped to [`DeliveryOutcome::Rejected`]
///   (terminal, not retryable) rather than
///   [`DeliveryOutcome::Failed`] (terminal, but on a retry schedule):
///   whatever caused MADAPI or an upstream operator to pull the message
///   from the queue is not something retrying the identical submission
///   would fix.
// `REJECTED`/`DELETED` share a body (both `Rejected`), and so do
// `UNKNOWN`/the trailing wildcard (both `Unknown`) — clippy's own
// `match_same_arms` would rather these merge into `"REJECTED" |
// "DELETED" => ...`. Deliberately left as separate arms: this match's
// whole job (see the fn's own doc) is that each of MADAPI's eight real
// values gets its own named arm, so the list stays a literal transcript
// of `mtn-sms-v3-swagger.yaml`'s own `status` enum rather than a set of
// clauses a future reader has to reverse-engineer from a merged pattern
// — and `"UNKNOWN"` staying its own arm (rather than folding into the
// wildcard) is what makes it visible that this is a *documented* eighth
// value, not merely "anything else."
#[allow(clippy::match_same_arms)]
fn outcome_of(status: &str) -> DeliveryOutcome {
    match status {
        "DELIVERED" => DeliveryOutcome::Delivered,
        "EXPIRED" => DeliveryOutcome::Expired,
        "REJECTED" => DeliveryOutcome::Rejected,
        "UNDELIVERED" => DeliveryOutcome::Failed,
        "ACCEPTD" | "ENROUTE" => DeliveryOutcome::InFlight,
        "DELETED" => DeliveryOutcome::Rejected,
        "UNKNOWN" => DeliveryOutcome::Unknown,
        _ => DeliveryOutcome::Unknown,
    }
}

/// Parse `completedDate` as RFC3339. The Swagger types this field
/// `format: datetime` with no further specification, so RFC3339 is an
/// assumption, not a confirmed contract — see this module's own doc. A
/// value present but unparseable is logged and treated as absent, never
/// propagated as an error: `occurred_at` is documented `Option` on
/// [`DeliveryUpdate`] precisely because a provider doesn't always say
/// when something happened, and a malformed timestamp is the same kind
/// of absence as no timestamp at all, not a reason to reject the whole
/// notification.
fn parse_completed_date(raw: Option<&str>) -> Option<DateTime<Utc>> {
    let raw = raw?;
    match DateTime::parse_from_rfc3339(raw) {
        Ok(parsed) => Some(parsed.with_timezone(&Utc)),
        Err(error) => {
            warn!(
                raw,
                %error,
                "MTN completedDate did not parse as RFC3339; leaving occurred_at unset"
            );
            None
        }
    }
}

/// Parse a raw MADAPI DLR callback body into a canonical delivery update.
///
/// # Errors
///
/// The body isn't valid JSON in the expected shape, or it carries no
/// `clientCorrelatorId` at all — without it there is nothing to correlate
/// against a `Message`, the same reasoning
/// `sms-provider-orange-cm::dlr::parse` uses for its own required
/// `callbackData` field.
pub(crate) fn parse(raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError> {
    let parsed: DeliveryNotification =
        serde_json::from_slice(&raw.body).map_err(|error| ProviderError::Rejected {
            code: "MALFORMED_DLR".to_owned(),
            message: format!("could not parse delivery notification: {error}"),
        })?;

    let Some(provider_ref) = parsed.client_correlator_id.filter(|id| !id.is_empty()) else {
        return Err(ProviderError::Rejected {
            code: "MISSING_CLIENT_CORRELATOR_ID".to_owned(),
            message: "delivery notification carried no clientCorrelatorId to correlate against"
                .to_owned(),
        });
    };

    // A missing `deliveryStatus` is not rejected (see this module's own
    // doc) — an empty raw_status maps through `outcome_of`'s trailing
    // wildcard to `Unknown`, the same honest answer an unrecognised
    // string gets.
    let raw_status = parsed.delivery_status.unwrap_or_default();
    let outcome = outcome_of(&raw_status);
    let occurred_at = parse_completed_date(parsed.completed_date.as_deref());
    // `error` first, `details` as a fallback — see this module's own doc
    // on why there is no principled way to prefer one over the other
    // from the Swagger alone, and why losing whichever isn't chosen is
    // an accepted, documented information loss rather than an oversight.
    let error_code = parsed.error.or(parsed.details);

    Ok(vec![DeliveryUpdate {
        provider_ref,
        outcome,
        occurred_at,
        raw_status,
        error_code,
        // MADAPI's `DeliveryNotificationRequest` has no field naming the
        // network that actually delivered — see this module's own doc.
        delivering_network: None,
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
            r#"{"clientCorrelatorId":"c-1","deliveryStatus":"DELIVERED"}"#,
        ))
        .unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].outcome, DeliveryOutcome::Delivered);
        assert_eq!(updates[0].provider_ref, "c-1");
        assert_eq!(
            updates[0].delivering_network, None,
            "DeliveryNotificationRequest has no field for this — see the module doc"
        );
    }

    #[test]
    fn parses_a_failed_notification_preferring_error_over_details() {
        let updates = parse(&callback(
            r#"{"clientCorrelatorId":"c-2","deliveryStatus":"UNDELIVERED","error":"E101","details":"handset unreachable"}"#,
        ))
        .unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::Failed);
        assert_eq!(
            updates[0].error_code.as_deref(),
            Some("E101"),
            "error must win over details when both are present"
        );
    }

    #[test]
    fn details_is_the_fallback_when_error_is_absent() {
        let updates = parse(&callback(
            r#"{"clientCorrelatorId":"c-2b","deliveryStatus":"UNDELIVERED","details":"handset unreachable"}"#,
        ))
        .unwrap();
        assert_eq!(
            updates[0].error_code.as_deref(),
            Some("handset unreachable")
        );
    }

    /// Guard-failure proof (a): `ACCEPTD`/`ENROUTE` must land on
    /// `InFlight`, never `Uncertain` — collapsing them would cost the
    /// message its retry path the moment a genuine failure follows (see
    /// `outcome_of`'s own doc). Deliberately checks both progress values
    /// against the exact variant, not merely "not Uncertain", so a
    /// regression that maps either to some *other* wrong terminal
    /// outcome is caught too.
    #[test]
    fn acceptd_and_enroute_are_in_flight_progress_not_uncertain_or_a_terminal_outcome() {
        for status in ["ACCEPTD", "ENROUTE"] {
            let body = format!(r#"{{"clientCorrelatorId":"c-3","deliveryStatus":"{status}"}}"#);
            let updates = parse(&callback(&body)).unwrap();
            assert_eq!(
                updates[0].outcome,
                DeliveryOutcome::InFlight,
                "status {status} must map to InFlight, not {:?}",
                updates[0].outcome
            );
        }
    }

    #[test]
    fn deleted_is_rejected_a_terminal_administrative_removal_not_a_retryable_failure() {
        let updates = parse(&callback(
            r#"{"clientCorrelatorId":"c-4","deliveryStatus":"DELETED"}"#,
        ))
        .unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::Rejected);
    }

    #[test]
    fn expired_and_rejected_map_directly() {
        for (status, expected) in [
            ("EXPIRED", DeliveryOutcome::Expired),
            ("REJECTED", DeliveryOutcome::Rejected),
            ("UNKNOWN", DeliveryOutcome::Unknown),
        ] {
            let body = format!(r#"{{"clientCorrelatorId":"c-5","deliveryStatus":"{status}"}}"#);
            let updates = parse(&callback(&body)).unwrap();
            assert_eq!(updates[0].outcome, expected, "status {status}");
        }
    }

    #[test]
    fn an_unrecognised_status_is_unknown_not_a_guess() {
        let updates = parse(&callback(
            r#"{"clientCorrelatorId":"c-6","deliveryStatus":"SomeFutureStatusThisCrateDoesNotKnowAbout"}"#,
        ))
        .unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::Unknown);
    }

    /// A missing `deliveryStatus` is not a rejection — see the module doc
    /// on why every field of `DeliveryNotificationRequest` except
    /// `clientCorrelatorId` degrades rather than fails the notification.
    #[test]
    fn a_missing_delivery_status_is_unknown_not_rejected() {
        let updates = parse(&callback(r#"{"clientCorrelatorId":"c-7"}"#)).unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::Unknown);
        assert_eq!(updates[0].raw_status, "");
    }

    #[test]
    fn a_notification_with_no_client_correlator_id_is_rejected() {
        let error = parse(&callback(r#"{"deliveryStatus":"DELIVERED"}"#)).unwrap_err();
        assert!(matches!(
            error,
            sms_provider::ProviderError::Rejected { .. }
        ));
    }

    #[test]
    fn a_notification_with_an_empty_client_correlator_id_is_rejected() {
        let error = parse(&callback(
            r#"{"clientCorrelatorId":"","deliveryStatus":"DELIVERED"}"#,
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

    #[test]
    fn completed_date_round_trips_as_rfc3339() {
        let updates = parse(&callback(
            r#"{"clientCorrelatorId":"c-8","deliveryStatus":"DELIVERED","completedDate":"2026-08-11T12:00:00Z"}"#,
        ))
        .unwrap();
        assert!(updates[0].occurred_at.is_some());
    }

    /// Guard-failure proof (matching `parse_completed_date`'s own doc):
    /// an unparseable `completedDate` must not fail the whole
    /// notification — the rest of the update is still produced, with
    /// `occurred_at` left `None`.
    #[test]
    fn an_unparseable_completed_date_is_dropped_not_a_rejection() {
        let updates = parse(&callback(
            r#"{"clientCorrelatorId":"c-9","deliveryStatus":"DELIVERED","completedDate":"not a real datetime"}"#,
        ))
        .unwrap();
        assert_eq!(updates[0].outcome, DeliveryOutcome::Delivered);
        assert!(updates[0].occurred_at.is_none());
    }
}
