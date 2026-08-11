//! `expire_stale` — the one real `kind` this milestone wires up. §7.5's
//! own table: "`submitted`/`uncertain` past validity → `expired`", 1-minute
//! cadence. #122 added a third rule for `undelivered`, on the same
//! `expiresAt` clock as `submitted` — see below.
//!
//! Three separate rules, not one, because the states measure "past
//! validity" against different clocks:
//!
//! - `submitted -> expired: no DLR in window` (§7.4) uses `Message.expiresAt`
//!   directly — the same validity budget set at creation (15 min for `otp`,
//!   24h for `notification`), unconsumed by the time a DLR should have
//!   arrived.
//! - `uncertain -> expired: 6h timer` (§7.4) is a *fresh* clock, not tied to
//!   the original `expiresAt` — a message can turn `uncertain` well within
//!   its original window, and per §7.4 "never retried automatically", it
//!   gets its own 6-hour grace regardless of how much of the original
//!   window was left. The schema has no dedicated
//!   `enteredUncertainAt` field, so this uses `updatedAt` (bumped by the
//!   `Timestamps` mixin's own touch trigger on every write) as the proxy
//!   for "when it became `uncertain`" — correct as long as nothing else
//!   writes to an `uncertain` message before this job or a late DLR does,
//!   which holds: `uncertain` is not itself a target of any operator or
//!   retry action in §7.4's diagram, only DLRs and this job ever move it.
//! - `undelivered -> expired` (§7.4, #122) uses `Message.expiresAt` directly,
//!   the same clock as `submitted`, deliberately *not* a fresh grace period
//!   like `uncertain`'s: `undelivered` is a genuinely retryable failure —
//!   `crates/sms-worker/src/claim.rs`'s `Claimable for Message` now selects
//!   it and retries via `-> queued` — but a retry that would only deliver
//!   after the message's own original validity budget is gone is exactly
//!   the outcome `expiresAt` exists to prevent (§7.4's own backoff
//!   paragraph: "capped by `maxAttempts` and hard-stopped by `expiresAt`").
//!   `claim.rs`'s shared `expiresAt` filter already stops retrying such a
//!   row; this rule is what actually moves it off `undelivered` once that
//!   happens, since retry exhaustion alone (`maxAttempts`) already reaches
//!   `failed` on its own via `claim.rs`'s own `undelivered` branch and
//!   never needs this job at all — this rule exists for the row that's
//!   still under `maxAttempts` but has simply run out of time.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_api::schema::{message, Cratestack, Job, MessageState, UpdateMessageInput};
use sms_api::{is_illegal_transition, map_database_error};
use tracing::warn;

use crate::jobs::JobHandler;

/// How long `uncertain` waits before expiring, per §7.4.
const UNCERTAIN_GRACE: Duration = Duration::hours(6);

/// Bounds one run's work — this job runs every minute (§7.5's own table),
/// so a backlog beyond this batch is picked up by the very next run rather
/// than this one invocation trying to drain an unbounded queue.
const BATCH: i64 = 500;

/// The `expire_stale` [`JobHandler`] — see the module doc for its two
/// rules.
pub struct ExpireStale;

impl ExpireStale {
    /// The testable core. [`JobHandler::run`] calls this with `Utc::now()`;
    /// live tests call it directly with a controlled `now` — the only way
    /// to prove the `uncertain` 6-hour cutoff without either waiting 6 real
    /// hours or defeating `touch_updated_at`, which unconditionally sets
    /// `Message.updatedAt` to `clock_timestamp()` on every `UPDATE` and so
    /// makes backdating it through any `CrateStack` delegate write genuinely
    /// impossible (an R1-compliant test has no way around that trigger,
    /// short of a raw-SQL exception this one assertion doesn't justify).
    pub async fn run_at(
        &self,
        db: &Cratestack,
        sys: &CoolContext,
        now: chrono::DateTime<Utc>,
    ) -> Result<(), String> {
        expire_matching(
            db,
            sys,
            FilterExpr::from(message::state().eq(MessageState::submitted))
                .and(message::expiresAt().lte(now)),
        )
        .await
        .map_err(|error| format!("expiring stale submitted messages: {error}"))?;

        expire_matching(
            db,
            sys,
            FilterExpr::from(message::state().eq(MessageState::uncertain))
                .and(message::updatedAt().lte(now - UNCERTAIN_GRACE)),
        )
        .await
        .map_err(|error| format!("expiring stale uncertain messages: {error}"))?;

        expire_matching(
            db,
            sys,
            FilterExpr::from(message::state().eq(MessageState::undelivered))
                .and(message::expiresAt().lte(now)),
        )
        .await
        .map_err(|error| format!("expiring stale undelivered messages: {error}"))?;

        Ok(())
    }
}

#[async_trait]
impl JobHandler for ExpireStale {
    fn kind(&self) -> &'static str {
        "expire_stale"
    }

    async fn run(&self, db: &Cratestack, sys: &CoolContext, _job: &Job) -> Result<(), String> {
        self.run_at(db, sys, Utc::now()).await
    }
}

/// Select up to [`BATCH`] matching messages and move each to `expired`.
/// Per-row `if_match`, same CAS discipline as every other writer of
/// `Message` — a row that moved on since it was selected (a DLR racing
/// this job, per `sms_api::dlr`) is logged and skipped, not a fault: the
/// row is simply no longer stale by the time this job got to it.
async fn expire_matching(
    db: &Cratestack,
    sys: &CoolContext,
    filter: FilterExpr,
) -> Result<(), CoolError> {
    let candidates = db
        .message()
        .find_many()
        .where_expr(filter)
        .limit(BATCH)
        .run(sys)
        .await?;

    for candidate in candidates {
        let result = db
            .message()
            .update(candidate.id.clone())
            .set(UpdateMessageInput {
                state: Some(MessageState::expired),
                ..Default::default()
            })
            .if_match(candidate.version)
            .run(sys)
            .await;

        if let Err(error) = result {
            // #71: checked against the raw error before any mapping — see
            // `crate::jobs::swallow_stale_write`'s own doc for why this
            // order matters and is not merely stylistic: mapping first
            // would turn a genuine SM001 into `CoolError::Conflict`, which
            // this function's own `Conflict`/`PreconditionFailed` arm
            // below would otherwise swallow as if it were the harmless
            // "message moved on" race it exists to catch — exactly the
            // silent-SM001 failure mode #70 exists to close.
            if is_illegal_transition(&error) {
                return Err(map_database_error(error));
            }
            match error {
                CoolError::Conflict(reason) | CoolError::PreconditionFailed(reason) => {
                    warn!(
                        message_id = %candidate.id,
                        reason,
                        "message moved on before expire_stale reached it; skipping"
                    );
                }
                other => return Err(other),
            }
        }
    }
    Ok(())
}
