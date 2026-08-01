//! Translating database-level rejections into HTTP-shaped errors.
//!
//! R2 says state transitions are proposed by Rust and decided by Postgres. The
//! deciding half is a `BEFORE UPDATE` trigger that raises SQLSTATE `SM001` on
//! an illegal edge. Left untranslated that arrives as
//! [`CoolError::DatabaseTyped`], which the framework maps to
//! `500 DATABASE_ERROR` — and a 500 reads as "the gateway is broken" when the
//! truth is "you asked for a transition that does not exist". Callers retry
//! 500s and do not retry 409s, so the distinction changes their behaviour, not
//! just their logs.

use cratestack::CoolError;

/// SQLSTATE raised by `messages_guard_transition` and `jobs_guard_transition`
/// when the proposed edge is absent from the transition table.
pub const SM001: &str = "SM001";

/// SQLSTATE for `unique_violation`. Dedupe is `create` plus catching this,
/// because `upsert` does not exist when the `@id` carries a default.
pub const UNIQUE_VIOLATION: &str = "23505";

/// Map database rejections that are really client errors onto the right shape.
///
/// - `SM001` → [`CoolError::Conflict`] → HTTP 409.
/// - `23505` → [`CoolError::Conflict`] → HTTP 409, so a duplicate
///   `idempotencyKey` or a re-inserted webhook attempt reads as "already
///   exists" rather than as an internal fault.
///
/// Everything else is returned untouched. In particular a genuine database
/// fault stays a 500, because turning every database error into a 4xx would
/// hide real outages behind a client-error status.
#[must_use]
pub fn map_database_error(error: CoolError) -> CoolError {
    match error.db_sqlstate() {
        Some(SM001) => CoolError::Conflict(illegal_transition_message(&error)),
        Some(UNIQUE_VIOLATION) => CoolError::Conflict(error.db_constraint().map_or_else(
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
pub fn is_illegal_transition(error: &CoolError) -> bool {
    error.db_sqlstate() == Some(SM001)
}

/// The trigger's own message, which already names both states and the row id.
///
/// Falls back to a generic string if the driver gave us no detail — better a
/// vague 409 than a misleading 500.
fn illegal_transition_message(error: &CoolError) -> String {
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

    fn db_error(sqlstate: &str, detail: &str, constraint: Option<&str>) -> CoolError {
        CoolError::DatabaseTyped(DbErrorInfo {
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
        assert!(matches!(mapped, CoolError::Conflict(_)), "got {mapped:?}");
        assert_eq!(mapped.status_code(), 409);
    }

    #[test]
    fn the_conflict_names_both_states_so_the_caller_can_act_on_it() {
        let error = db_error(
            SM001,
            "illegal message transition delivered -> queued on abc123",
            None,
        );
        let message = match map_database_error(error) {
            CoolError::Conflict(m) => m,
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
            CoolError::Conflict(m) => m,
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
        let mapped = map_database_error(CoolError::NotFound("message".to_owned()));
        assert!(matches!(mapped, CoolError::NotFound(_)));
    }

    #[test]
    fn a_missing_sqlstate_is_not_guessed_at() {
        let error = CoolError::DatabaseTyped(DbErrorInfo {
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
        assert!(!is_illegal_transition(&CoolError::Conflict("x".to_owned())));
    }

    #[test]
    fn a_detail_free_sm001_still_conflicts() {
        let mapped = map_database_error(db_error(SM001, "", None));
        assert!(matches!(mapped, CoolError::Conflict(_)));
        assert_eq!(mapped.status_code(), 409);
    }
}
