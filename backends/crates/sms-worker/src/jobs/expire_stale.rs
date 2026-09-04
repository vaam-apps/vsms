#![doc = include_str!("expire_stale.md")]

use async_trait::async_trait;
use chrono::{Duration, Utc};
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sms_api::schema::{Cratestack, Job, MessageState, UpdateMessageInput, message};
use sms_api::{is_illegal_transition, map_database_error};
use tracing::warn;

use crate::jobs::{JobError, JobHandler};

/// How long `uncertain` waits before expiring, per §7.4.
const UNCERTAIN_GRACE: Duration = Duration::hours(6);

/// Bounds one run's work — this job runs every minute (§7.5's own table),
/// so a backlog beyond this batch is picked up by the very next run rather
/// than this one invocation trying to drain an unbounded queue.
const BATCH: i64 = 500;

/// Context wording for [`JobError::Database`] — a `pub(crate) const`, not an
/// inline literal, so the one live test that pins these thirteen wordings
/// (`jobs::tests::every_context_literal_matches_the_documented_wording`)
/// shares the exact value the real call site below writes into
/// `Job.lastError`, rather than an independent copy that could silently
/// drift from it. See that test's own doc for why the independent-copy
/// shape was the actual bug this replaces.
pub(crate) const CTX_SUBMITTED: &str = "expiring stale submitted messages";
/// See [`CTX_SUBMITTED`].
pub(crate) const CTX_UNCERTAIN: &str = "expiring stale uncertain messages";
/// See [`CTX_SUBMITTED`].
pub(crate) const CTX_UNDELIVERED: &str = "expiring stale undelivered messages";

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
        sys: &CratestackContext,
        now: chrono::DateTime<Utc>,
    ) -> Result<(), JobError> {
        expire_matching(
            db,
            sys,
            FilterExpr::from(message::state().eq(MessageState::submitted))
                .and(message::expiresAt().lte(now)),
        )
        .await
        .map_err(|source| JobError::Database {
            context: CTX_SUBMITTED,
            source,
        })?;

        expire_matching(
            db,
            sys,
            FilterExpr::from(message::state().eq(MessageState::uncertain))
                .and(message::updatedAt().lte(now - UNCERTAIN_GRACE)),
        )
        .await
        .map_err(|source| JobError::Database {
            context: CTX_UNCERTAIN,
            source,
        })?;

        expire_matching(
            db,
            sys,
            FilterExpr::from(message::state().eq(MessageState::undelivered))
                .and(message::expiresAt().lte(now)),
        )
        .await
        .map_err(|source| JobError::Database {
            context: CTX_UNDELIVERED,
            source,
        })?;

        Ok(())
    }
}

#[async_trait]
impl JobHandler for ExpireStale {
    fn kind(&self) -> &'static str {
        "expire_stale"
    }

    async fn run(
        &self,
        db: &Cratestack,
        sys: &CratestackContext,
        _job: &Job,
    ) -> Result<(), JobError> {
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
    sys: &CratestackContext,
    filter: FilterExpr,
) -> Result<(), CratestackError> {
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
            // would turn a genuine SM001 into `CratestackError::Conflict`, which
            // this function's own `Conflict`/`PreconditionFailed` arm
            // below would otherwise swallow as if it were the harmless
            // "message moved on" race it exists to catch — exactly the
            // silent-SM001 failure mode #70 exists to close.
            if is_illegal_transition(&error) {
                return Err(map_database_error(error));
            }
            match error {
                CratestackError::Conflict(reason) | CratestackError::PreconditionFailed(reason) => {
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
