#![doc = include_str!("purge_retention.md")]

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sms_api::schema::{
    Cratestack, DeliveryReceipt, Job, MessageState, UpdateMessageInput, delivery_receipt, message,
};
use tracing::warn;

use crate::jobs::JobHandler;

/// §7.5's own retention for both halves of this job — `Message` and
/// `DeliveryReceipt` each carry their own `@@retain(days: 90)` in
/// `schema.cstack`, and both happen to be the same number.
const RETENTION: Duration = Duration::days(90);

/// Written over `Message.msisdn` on purge — see the module doc for why the
/// column stays `NOT NULL` rather than becoming `Option<String>`. Exactly
/// 13 characters: inside `@length(min: 12, max: 15)`, so
/// `UpdateMessageInput::validate()` accepts it, and unambiguously not an
/// MSISDN (no digit-only run this long exists in `sms_msisdn`'s own
/// Cameroon prefix tables).
const PURGED_MSISDN_PLACEHOLDER: &str = "purged-msisdn";

/// `Message` rows are purged in batches this size per run — same reasoning
/// `expire_stale::BATCH`/`reap_outbox::DELETE_BATCH` already give: a
/// backlog beyond this is picked up by tomorrow's run, not this one
/// invocation trying to clear an unbounded backlog in a single tick.
const MESSAGE_BATCH: i64 = 500;

/// `DeliveryReceipt` rows are deleted in batches this size per run, same
/// reasoning as [`MESSAGE_BATCH`].
const RECEIPT_BATCH: i64 = 500;

/// §7.4's terminal `Message` states — the ones with no outgoing row in
/// `message_state_transitions`, i.e. nothing can move a message out of one
/// of these ever again. See the module doc for why only these are eligible.
const TERMINAL_STATES: [MessageState; 5] = [
    MessageState::delivered,
    MessageState::failed,
    MessageState::expired,
    MessageState::rejected,
    MessageState::cancelled,
];

/// The `purge_retention` [`JobHandler`] — see the module doc for what gets
/// purged, what survives, and why.
pub struct PurgeRetention;

impl PurgeRetention {
    /// The testable core, the same seam `ExpireStale::run_at` and
    /// `ReapOutbox::run_at` use and for the same reason: live tests need
    /// control over the retention boundary without waiting out 90 real
    /// days. Unlike those two, `Message.createdAt` genuinely *can* be
    /// backdated through a delegate (see the live test's own doc for why),
    /// so this job's own tests seed real old timestamps rather than
    /// shifting a virtual `now` — the boundary proof this issue's own
    /// acceptance criterion asks for.
    pub async fn run_at(
        &self,
        db: &Cratestack,
        sys: &CratestackContext,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let cutoff = now - RETENTION;

        let purged = purge_messages(db, sys, cutoff, now)
            .await
            .map_err(|error| format!("purging retained messages: {error}"))?;
        if purged > 0 {
            tracing::info!(purged, "purged messages past the 90-day retention window");
        }

        let deleted = purge_delivery_receipts(db, sys, cutoff)
            .await
            .map_err(|error| format!("purging retained delivery receipts: {error}"))?;
        if deleted > 0 {
            tracing::info!(
                deleted,
                "deleted delivery receipts past the 90-day retention window"
            );
        }

        Ok(())
    }
}

#[async_trait]
impl JobHandler for PurgeRetention {
    fn kind(&self) -> &'static str {
        "purge_retention"
    }

    async fn run(
        &self,
        db: &Cratestack,
        sys: &CratestackContext,
        _job: &Job,
    ) -> Result<(), String> {
        self.run_at(db, sys, Utc::now()).await
    }
}

/// Select up to [`MESSAGE_BATCH`] terminal, not-yet-purged messages past
/// `cutoff` and redact each — see the module doc for the exact column
/// list. Per-row `if_match`, same CAS discipline as every other writer of
/// `Message` (this model carries `@version`): a row that moved on since it
/// was selected — which, for a terminal row, can in practice only be a
/// concurrent purge run reaching it first, since nothing else ever writes
/// a terminal message — is logged and skipped, not a fault.
async fn purge_messages(
    db: &Cratestack,
    sys: &CratestackContext,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<usize, CratestackError> {
    let candidates = db
        .message()
        .find_many()
        .where_expr(
            FilterExpr::from(message::state().in_(TERMINAL_STATES))
                .and(message::createdAt().lte(cutoff))
                .and(message::purgedAt().is_null()),
        )
        .limit(MESSAGE_BATCH)
        .run(sys)
        .await?;

    let mut purged = 0usize;
    for candidate in candidates {
        let result = db
            .message()
            .update(candidate.id.clone())
            .set(UpdateMessageInput {
                msisdn: Some(PURGED_MSISDN_PLACEHOLDER.to_owned()),
                body: Some(None),
                clientRef: Some(None),
                idempotencyKey: Some(None),
                stateReason: Some(None),
                purgedAt: Some(Some(now)),
                ..Default::default()
            })
            .if_match(candidate.version)
            .run(sys)
            .await;

        match result {
            Ok(_) => purged += 1,
            Err(
                CratestackError::Conflict(reason) | CratestackError::PreconditionFailed(reason),
            ) => {
                warn!(
                    message_id = %candidate.id,
                    reason,
                    "message changed before purge_retention reached it; skipping this run"
                );
            }
            Err(other) => return Err(other),
        }
    }
    Ok(purged)
}

/// Select up to [`RECEIPT_BATCH`] receipts past `cutoff` (by their own
/// `receivedAt`, independent of their parent `Message`'s age — see the
/// module doc) and delete each. No CAS: `DeliveryReceipt` carries no
/// `@version`, matching its own append-only, never-updated design (nothing
/// in this codebase calls `.update()` on it — `dlr.rs` only ever
/// `.create()`s one).
async fn purge_delivery_receipts(
    db: &Cratestack,
    sys: &CratestackContext,
    cutoff: DateTime<Utc>,
) -> Result<usize, CratestackError> {
    let candidates: Vec<DeliveryReceipt> = db
        .delivery_receipt()
        .find_many()
        .where_expr(FilterExpr::from(delivery_receipt::receivedAt().lte(cutoff)))
        .limit(RECEIPT_BATCH)
        .run(sys)
        .await?;

    let mut deleted = 0usize;
    for candidate in candidates {
        db.delivery_receipt().delete(candidate.id).run(sys).await?;
        deleted += 1;
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::{PURGED_MSISDN_PLACEHOLDER, PurgeRetention, RETENTION};
    use crate::jobs::JobHandler;

    #[test]
    fn kind_matches_the_scheduler_and_design_docs_naming() {
        assert_eq!(PurgeRetention.kind(), "purge_retention");
    }

    #[test]
    fn retention_matches_the_design_doc_and_both_models_own_retain_clause() {
        // §7.5 / schema.cstack's `@@retain(days: 90)` on both `Message` and
        // `DeliveryReceipt`.
        assert_eq!(RETENTION, chrono::Duration::days(90));
    }

    #[test]
    fn the_msisdn_placeholder_satisfies_messages_own_length_validator() {
        // `Message.msisdn @length(min: 12, max: 15)` — `UpdateMessageInput`'s
        // generated `validate()` runs this check against `Some`-wrapped
        // values regardless of the column's own nullability (update inputs
        // treat every field as present-or-absent, per
        // cratestack-macros' `validate_impl_tokens(&fields, true)`). A
        // placeholder outside this range would make every purge attempt
        // fail its own write.
        let len = PURGED_MSISDN_PLACEHOLDER.len();
        assert!(
            (12..=15).contains(&len),
            "placeholder must satisfy Message.msisdn's own @length(min: 12, max: 15): got {len}"
        );
    }

    #[test]
    fn the_msisdn_placeholder_is_not_a_plausible_msisdn() {
        // Belt-and-braces: it must not parse as a Cameroon MSISDN, or a
        // purged row would look like it still has a usable number.
        assert!(sms_msisdn::Msisdn::parse(PURGED_MSISDN_PLACEHOLDER).is_err());
    }
}
