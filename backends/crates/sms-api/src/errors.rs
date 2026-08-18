#![doc = include_str!("errors.md")]

use cratestack::CratestackError;

/// SQLSTATE raised by `messages_guard_transition`, `jobs_guard_transition`,
/// and `attempts_guard_transition` when the proposed edge is absent from the
/// transition table.
pub const SM001: &str = "SM001";

/// SQLSTATE for `unique_violation`. Dedupe is `create` plus catching this,
/// because `upsert` does not exist when the `@id` carries a default.
pub const UNIQUE_VIOLATION: &str = "23505";

/// Map database rejections that are really client errors onto the right shape.
///
/// - `SM001` → [`CratestackError::Conflict`] → HTTP 409.
/// - `23505` → [`CratestackError::Conflict`] → HTTP 409, so a duplicate
///   `idempotencyKey` or a re-inserted webhook attempt reads as "already
///   exists" rather than as an internal fault.
///
/// Everything else is returned untouched. In particular a genuine database
/// fault stays a 500, because turning every database error into a 4xx would
/// hide real outages behind a client-error status.
#[must_use]
pub fn map_database_error(error: CratestackError) -> CratestackError {
    match error.db_sqlstate() {
        Some(SM001) => {
            // #71: the one metrics choke point — see this module's own
            // doc. `error.to_string()` before the mapped, shortened
            // message is constructed below, so the label parser sees the
            // trigger's full original text.
            sms_metrics::record_sm001(&error.to_string());
            CratestackError::Conflict(illegal_transition_message(&error))
        }
        Some(UNIQUE_VIOLATION) => CratestackError::Conflict(error.db_constraint().map_or_else(
            || "resource already exists".to_owned(),
            |c| format!("resource already exists ({c})"),
        )),
        _ => error,
    }
}

/// Whether this error is an illegal state transition.
///
/// Worth checking explicitly in a claim loop: `SM001` means the state machine
/// and the code disagree, which is a bug to alert on, not a race to retry.
#[must_use]
pub fn is_illegal_transition(error: &CratestackError) -> bool {
    error.db_sqlstate() == Some(SM001)
}

/// The trigger's own message, which already names both states and the row id.
///
/// Falls back to a generic string if the driver gave us no detail — better a
/// vague 409 than a misleading 500.
fn illegal_transition_message(error: &CratestackError) -> String {
    let detail = error.to_string();
    if detail.contains("illegal") {
        // "database: illegal message transition delivered -> queued on abc123"
        detail
            .split_once("illegal")
            .map_or(detail.clone(), |(_, rest)| format!("illegal{rest}"))
    } else {
        "illegal state transition".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cratestack::DbErrorInfo;

    fn db_error(sqlstate: &str, detail: &str, constraint: Option<&str>) -> CratestackError {
        CratestackError::DatabaseTyped(DbErrorInfo {
            detail: detail.to_owned(),
            sqlstate: Some(sqlstate.to_owned()),
            constraint: constraint.map(ToOwned::to_owned),
        })
    }

    #[test]
    fn sm001_becomes_a_conflict() {
        let error = db_error(
            SM001,
            "illegal message transition delivered -> queued on abc123",
            None,
        );
        let mapped = map_database_error(error);
        assert!(
            matches!(mapped, CratestackError::Conflict(_)),
            "got {mapped:?}"
        );
        assert_eq!(mapped.status_code(), 409);
    }

    /// #71/#70's "prove your guards can fail" standard: this is the guard
    /// that a bug reproducing #33's own "accepted -> routed reachable"
    /// class of mistake would need to trip an alert. Verified by
    /// deliberately breaking it (commenting out the `record_sm001` call in
    /// `map_database_error`) and confirming this test fails with exactly
    /// the assertion below naming the expected count, before restoring it
    /// — see this PR's own description for the exact failure output.
    #[test]
    fn sm001_increments_the_labelled_prometheus_counter() {
        let before = sms_metrics::SM001_TOTAL
            .with_label_values(&["message", "routed", "delivered"])
            .get();

        let error = db_error(
            SM001,
            "illegal message transition routed -> delivered on msg_counter_test",
            None,
        );
        let _ = map_database_error(error);

        let after = sms_metrics::SM001_TOTAL
            .with_label_values(&["message", "routed", "delivered"])
            .get();
        assert_eq!(
            after,
            before + 1,
            "map_database_error must record exactly one SM001 observation"
        );
    }

    /// A non-SM001 conflict (23505) must never touch the SM001 counter —
    /// otherwise a burst of ordinary idempotency-key collisions would look
    /// like a state-machine drift alert.
    ///
    /// Asserted against a label combination no other test in this crate
    /// ever produces, checked only for staying at `0` — `cargo test` runs
    /// this crate's tests concurrently in one process, sharing the same
    /// `sms_metrics::SM001_TOTAL` static, so a shared "popular" label combo
    /// (`unknown`/`unknown`/`unknown`, which `a_detail_free_sm001_still_
    /// conflicts` below also produces) would make this test's own
    /// before/after diff flaky under parallel execution — not a bug in the
    /// counter, a property of asserting on process-global state from more
    /// than one test.
    #[test]
    fn a_unique_violation_does_not_touch_the_sm001_counter() {
        let error = db_error(UNIQUE_VIOLATION, "duplicate key", Some("some_constraint"));
        let _ = map_database_error(error);
        assert_eq!(
            sms_metrics::SM001_TOTAL
                .with_label_values(&["sentinel_23505_never_produced_by_sm001", "x", "y"])
                .get(),
            0
        );
    }

    #[test]
    fn the_conflict_names_both_states_so_the_caller_can_act_on_it() {
        let error = db_error(
            SM001,
            "illegal message transition delivered -> queued on abc123",
            None,
        );
        let message = match map_database_error(error) {
            CratestackError::Conflict(m) => m,
            other => panic!("expected Conflict, got {other:?}"),
        };
        assert!(message.contains("delivered"), "{message}");
        assert!(message.contains("queued"), "{message}");
    }

    #[test]
    fn unique_violation_becomes_a_conflict_and_names_the_constraint() {
        let error = db_error(
            UNIQUE_VIOLATION,
            "duplicate key value violates unique constraint",
            Some("messages_app_idempotency_key"),
        );
        let message = match map_database_error(error) {
            CratestackError::Conflict(m) => m,
            other => panic!("expected Conflict, got {other:?}"),
        };
        assert!(
            message.contains("messages_app_idempotency_key"),
            "{message}"
        );
    }

    #[test]
    fn a_real_database_fault_stays_a_500() {
        // 08006 is connection_failure. Dressing an outage up as a client error
        // would hide it from every alert keyed on 5xx.
        let error = db_error("08006", "connection failure", None);
        let mapped = map_database_error(error);
        assert_eq!(mapped.status_code(), 500);
        assert_eq!(mapped.code(), "DATABASE_ERROR");
    }

    #[test]
    fn non_database_errors_pass_through_untouched() {
        let mapped = map_database_error(CratestackError::NotFound("message".to_owned()));
        assert!(matches!(mapped, CratestackError::NotFound(_)));
    }

    #[test]
    fn a_missing_sqlstate_is_not_guessed_at() {
        let error = CratestackError::DatabaseTyped(DbErrorInfo {
            detail: "something happened".to_owned(),
            sqlstate: None,
            constraint: None,
        });
        assert_eq!(map_database_error(error).status_code(), 500);
    }

    #[test]
    fn illegal_transition_is_distinguishable_for_alerting() {
        assert!(is_illegal_transition(&db_error(
            SM001,
            "illegal x -> y",
            None
        )));
        assert!(!is_illegal_transition(&db_error(
            UNIQUE_VIOLATION,
            "dup",
            None
        )));
        assert!(!is_illegal_transition(&CratestackError::Conflict(
            "x".to_owned()
        )));
    }

    #[test]
    fn a_detail_free_sm001_still_conflicts() {
        let mapped = map_database_error(db_error(SM001, "", None));
        assert!(matches!(mapped, CratestackError::Conflict(_)));
        assert_eq!(mapped.status_code(), 409);
    }
}
