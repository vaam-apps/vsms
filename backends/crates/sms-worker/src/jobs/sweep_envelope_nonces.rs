#![doc = include_str!("sweep_envelope_nonces.md")]

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cratestack::{CratestackContext, FilterExpr};
use sms_api::schema::{Cratestack, Job, envelope_nonce};
use tracing::{debug, info};

use crate::jobs::{JobError, JobHandler};

/// Rows are deleted in batches this size, and a run takes at most
/// [`MAX_BATCHES`] of them. Together that bounds one run to
/// `BATCH * MAX_BATCHES` deletes, so a backlog (a long outage of the worker,
/// or a burst of traffic) is cleared over several 15-minute runs instead of
/// one invocation holding a transaction-sized chunk of the table.
const BATCH: i64 = 500;

/// See [`BATCH`].
const MAX_BATCHES: usize = 40;

/// Context wording for [`JobError::Database`] - see
/// `expire_stale::CTX_SUBMITTED`'s own doc for why this is a
/// `pub(crate) const`, not an inline literal.
pub(crate) const CTX_SWEEP: &str = "sweeping expired envelope nonces";

/// The `sweep_envelope_nonces` [`JobHandler`] - see the module doc.
pub struct SweepEnvelopeNonces;

impl SweepEnvelopeNonces {
    /// The testable core, the same seam `ReapOutbox::run_at` uses: `now` is
    /// a parameter so a live test can sweep "as of" a virtual clock instead of
    /// waiting out the 11-minute freshness window.
    ///
    /// Returns how many rows were deleted.
    pub async fn run_at(
        &self,
        db: &Cratestack,
        sys: &CratestackContext,
        now: DateTime<Utc>,
    ) -> Result<usize, JobError> {
        let mut deleted = 0_usize;
        for _ in 0..MAX_BATCHES {
            let batch = db
                .envelope_nonce()
                .find_many()
                .where_expr(FilterExpr::from(envelope_nonce::expiresAt().lt(now)))
                .order_by(envelope_nonce::expiresAt().asc())
                .limit(BATCH)
                .run(sys)
                .await
                .map_err(|source| JobError::Database {
                    context: CTX_SWEEP,
                    source,
                })?;
            let full = i64::try_from(batch.len()).is_ok_and(|len| len == BATCH);
            for row in batch {
                match db.envelope_nonce().delete(row.id).run(sys).await {
                    Ok(_) => deleted += 1,
                    // Another sweeper (a second worker, an overlapping run)
                    // took it first: the goal is met either way.
                    Err(cratestack::CratestackError::NotFound(_)) => {}
                    Err(source) => {
                        return Err(JobError::Database {
                            context: CTX_SWEEP,
                            source,
                        });
                    }
                }
            }
            if !full {
                break;
            }
        }

        if deleted > 0 {
            info!(deleted, "swept expired envelope nonces");
        } else {
            debug!("no expired envelope nonces to sweep");
        }
        Ok(deleted)
    }
}

#[async_trait]
impl JobHandler for SweepEnvelopeNonces {
    fn kind(&self) -> &'static str {
        "sweep_envelope_nonces"
    }

    async fn run(
        &self,
        db: &Cratestack,
        sys: &CratestackContext,
        _job: &Job,
    ) -> Result<(), JobError> {
        self.run_at(db, sys, Utc::now()).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::{BATCH, MAX_BATCHES, SweepEnvelopeNonces};
    use crate::jobs::JobHandler;

    #[test]
    fn kind_matches_the_scheduler_entry() {
        assert_eq!(SweepEnvelopeNonces.kind(), "sweep_envelope_nonces");
        assert!(
            crate::scheduler::schedule()
                .iter()
                .any(|spec| spec.kind == SweepEnvelopeNonces.kind()),
            "a handler nothing schedules never runs"
        );
    }

    #[test]
    fn one_run_is_bounded() {
        assert_eq!(BATCH * i64::try_from(MAX_BATCHES).unwrap(), 20_000);
    }
}
