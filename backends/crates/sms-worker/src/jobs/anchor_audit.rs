#![doc = include_str!("anchor_audit.md")]

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use cratestack::CoolContext;
use sms_api::audit_log::{
    compute_chain_hash_hex, fold_rows, genesis_hex, latest_anchor, rows_in_period,
    verify_chain_linkage, verify_period_content,
};
use sms_api::schema::{Cratestack, CreateAuditAnchorInput, Job};
use tracing::{debug, error, info, warn};

use crate::jobs::JobHandler;

/// Safety margin subtracted from "now" before drawing an anchor's upper
/// boundary — see the module doc's "the race this design accepts" section.
/// A large multiple of every write path's real commit latency in this
/// codebase, not a value tuned against any measured worst case.
const ANCHOR_LAG: Duration = Duration::minutes(5);

/// The `anchor_audit` [`JobHandler`] — see the module doc for the design
/// this implements and exactly what it proves.
pub struct AnchorAudit;

impl AnchorAudit {
    /// The testable core, the same seam every other job's own `run_at`
    /// uses. Unlike `ExpireStale`/`ReapOutbox`, the virtual `now` here
    /// only ever moves `periodEnd` forward — there is no delegate seam to
    /// backdate `cratestack_audit.occurred_at` through (it is stamped by
    /// the framework itself, `chrono::Utc::now()`, at build time), so live
    /// tests drive real audit rows through real timing rather than
    /// pretending a clock has moved.
    pub async fn run_at(
        &self,
        db: &Cratestack,
        sys: &CoolContext,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let latest = latest_anchor(db, sys)
            .await
            .map_err(|error| format!("loading the most recent audit anchor: {error}"))?;

        let breaks = verify_chain_linkage(db, sys)
            .await
            .map_err(|error| format!("verifying the audit anchor chain's own linkage: {error}"))?;
        for detail in &breaks {
            error!(
                detail,
                "audit anchor chain linkage broken — an anchor row no longer matches what an \
                 earlier or later anchor says it should; possible tampering"
            );
        }

        if let Some(anchor) = &latest {
            match verify_period_content(db, anchor).await {
                Ok(true) => debug!(
                    anchor_id = %anchor.id,
                    "most recent audit anchor's content re-verified against live cratestack_audit rows"
                ),
                Ok(false) => error!(
                    anchor_id = %anchor.id,
                    "audit anchor content hash mismatch on reverification — the cratestack_audit \
                     rows covering this anchor's period no longer fold to the stored rangeHash; \
                     possible tampering"
                ),
                Err(error) => warn!(
                    anchor_id = %anchor.id,
                    %error,
                    "could not reverify the most recent audit anchor's content this run"
                ),
            }
        }

        let period_start = latest.as_ref().map(|anchor| anchor.periodEnd);
        let period_end = now - ANCHOR_LAG;
        if let Some(start) = period_start
            && period_end <= start
        {
            debug!("nothing new to anchor yet this run");
            return Ok(());
        }

        let rows = rows_in_period(db, period_start, period_end)
            .await
            .map_err(|error| format!("reading cratestack_audit for the new period: {error}"))?;
        let row_count = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        let range_hash_hex = hex::encode(fold_rows(&rows));
        let prev_chain_hash_hex = latest
            .as_ref()
            .map_or_else(genesis_hex, |anchor| anchor.chainHash.clone());
        let chain_hash_hex = compute_chain_hash_hex(
            &prev_chain_hash_hex,
            period_start,
            period_end,
            row_count,
            &range_hash_hex,
        );

        db.audit_anchor()
            .create(CreateAuditAnchorInput {
                periodStart: period_start,
                periodEnd: period_end,
                rowCount: row_count,
                rangeHash: range_hash_hex,
                prevChainHash: prev_chain_hash_hex,
                chainHash: chain_hash_hex,
            })
            .run(sys)
            .await
            .map_err(|error| format!("writing the new audit anchor: {error}"))?;

        info!(row_count, "anchored audit rows");
        Ok(())
    }
}

#[async_trait]
impl JobHandler for AnchorAudit {
    fn kind(&self) -> &'static str {
        "anchor_audit"
    }

    async fn run(&self, db: &Cratestack, sys: &CoolContext, _job: &Job) -> Result<(), String> {
        self.run_at(db, sys, Utc::now()).await
    }
}

#[cfg(test)]
mod tests {
    use super::{ANCHOR_LAG, AnchorAudit};
    use crate::jobs::JobHandler;

    #[test]
    fn kind_matches_the_scheduler_and_design_docs_naming() {
        assert_eq!(AnchorAudit.kind(), "anchor_audit");
    }

    #[test]
    fn anchor_lag_is_generous_relative_to_a_real_write_paths_commit_latency() {
        // See the module doc's "the race this design accepts" — five
        // minutes against writes that commit within one request.
        assert_eq!(ANCHOR_LAG, chrono::Duration::minutes(5));
    }
}
