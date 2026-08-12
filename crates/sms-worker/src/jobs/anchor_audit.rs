//! `anchor_audit` — #68. §7.5's own table: "Merkle root of the day's audit
//! rows → append-only store." The issue itself: "the audit log is only
//! worth having if it can be shown not to have been edited. Periodic hash
//! anchoring so tampering is detectable... `@@audit` produces the rows;
//! this is about making them defensible."
//!
//! # What `@@audit` actually writes — read from the vendored source, not
//! # assumed
//!
//! `cratestack-sqlx-0.7.10/src/audit.rs` + `src/audit/schema.rs`, read
//! directly before designing anything here:
//!
//! - The table is `cratestack_audit`: `event_id UUID PRIMARY KEY,
//!   schema_name TEXT, model TEXT, operation TEXT, primary_key JSONB,
//!   actor JSONB, tenant TEXT, before JSONB, after JSONB, request_id TEXT,
//!   occurred_at TIMESTAMPTZ NOT NULL, delivered_at TIMESTAMPTZ, attempts
//!   BIGINT DEFAULT 0, last_error TEXT`.
//! - **It is insert-only.** `create.rs`/`update.rs`/`delete.rs` in
//!   `cratestack-sqlx` all route through the same `enqueue_audit_event`,
//!   which is a bare `INSERT`. Grepping the whole crate for any `UPDATE`
//!   or `DELETE` against `cratestack_audit` finds none — the `delivered_at`/
//!   `attempts`/`last_error` columns exist in the DDL (apparently
//!   provisioned for a future `AuditSink` drain mechanism, mirroring
//!   `cratestack_event_outbox`'s own shape) but nothing in this framework
//!   version ever reads or writes them. Vestigial, same shape as
//!   `crypto-aws-lc-rs`'s empty `install_fips_crypto_provider()` AGENTS.md
//!   already records finding once before — not acted on here, just noted
//!   so the next person doesn't assume a drain mechanism exists.
//! - **No ordering guarantee beyond `occurred_at`.** The primary key is a
//!   random `UUID` (`uuid::Uuid::new_v4()` in `build_audit_event`), not a
//!   sequence — there is no column this job could use as a monotonic
//!   cursor. `occurred_at` (`chrono::Utc::now()`, stamped once per event at
//!   build time, before the row's own transaction necessarily commits) is
//!   the only ordering signal, and it is wall-clock, not commit-order. See
//!   "the race this design accepts" below for what that costs.
//!
//! # The design decision: a hash chain, not a bare Merkle snapshot, and
//! # not external anchoring
//!
//! The issue names no implementation; three real options exist, and the
//! difference is not cosmetic:
//!
//! - **A hash chain** (chosen): each anchor folds every audit row in its
//!   own period into one digest, and includes the *previous anchor's own
//!   digest* as an input to its own. Editing or deleting any row already
//!   covered by an anchor changes that anchor's digest on recomputation;
//!   deleting the anchor row itself breaks the link the *next* anchor
//!   holds to it. Detects tampering with interior rows and with earlier
//!   anchors, not just "does the current snapshot look different."
//! - **A bare periodic Merkle/digest snapshot** (rejected): "the set as of
//!   time T hashes to X" on its own only proves membership at the moment
//!   it ran. Without chaining to what came before, deleting an *entire*
//!   past period's worth of rows — and its own anchor — leaves nothing
//!   contradicting the remaining anchors; each one is still internally
//!   self-consistent. A chain turns that from "silently possible" into
//!   "leaves a dangling `prevChainHash` the next-newer anchor's own
//!   verification can find," at the cost of some ordering bookkeeping —
//!   cheap, given this table is written to by a handful of models, not a
//!   flood.
//! - **External anchoring** (rejected, and not just for this PR): the
//!   strongest of the three — publishing a digest somewhere an operator
//!   with database access alone cannot rewrite (a notary service, a
//!   separate WORM store, even a signed commit to a repo the deployment
//!   itself has no write access to). This repo has no such service
//!   anywhere in `deploy/`, and standing one up is real infrastructure, not
//!   a job-kind PR — matching #68's own scope, which is "make `@@audit`'s
//!   rows defensible," not "stand up a compliance notary."
//!
//! **A keyed (HMAC) chain under a secret pepper was also considered and
//! rejected, for a reason specific to this codebase.** `crates/sms-api/src/
//! pepper.rs` already has exactly the machinery — a `HashPepper` loaded
//! outside the database — and keying the chain under it would raise the
//! bar from "an attacker with only `psql` access can recompute a valid-
//! looking chain" to "...cannot, without the pepper too." That is a real
//! improvement in isolation. It was rejected here because `pepper.rs`'s
//! own module doc is explicit that a pepper is designed to *rotate*, and
//! rotation does not retroactively rehash anything — so a keyed chain
//! would need every historical pepper value kept around indefinitely, just
//! to keep re-verifying anchors written under it, or every rotation would
//! make every past anchor look like a verification failure indistinguishable
//! from real tampering. That is a second, separate secret-management
//! design problem this ticket does not ask for and should not absorb as a
//! drive-by. A plain, unkeyed SHA-256 chain has no such coupling — it
//! proves everything the design doc's own §4.7 scoped ("if you need
//! tamper-evidence... hashes the day's audit rows... converts 'trust my
//! database' into 'verify my chain'"), no more.
//!
//! # Be honest about what this proves, and what it does not
//!
//! **An anchor stored in the same database an attacker already controls
//! proves less than "tamper-evident" sounds like it promises.** Concretely,
//! for an attacker who can write to this Postgres instance with no other
//! constraint:
//!
//! - Editing or deleting an audit row *without* touching the anchor chain
//!   is caught: the next `anchor_audit` run's own re-verification of the
//!   most recent period (see below), or a manual re-verification against
//!   an older anchor, recomputes a different `rangeHash` than what is
//!   stored.
//! - Editing or deleting an *entire past anchor row* is caught too, as
//!   long as a *later* anchor still exists to hold the deleted one's
//!   `chainHash` as its own `prevChainHash` — [`verify_chain_linkage`]
//!   checks exactly that, every run, over the whole table.
//! - **Deleting the single most-recent anchor, before the next one is
//!   written, is not caught by anything in this database.** Nothing yet
//!   references its `chainHash` as a `prevChainHash`. An attacker who can
//!   also recompute a fresh, internally-consistent chain from that point
//!   forward — i.e. who has the same write access this job itself has —
//!   leaves no contradiction anywhere in the table.
//!
//! So this genuinely raises the bar from "trust the database" to "trust
//! the database, or independently verify the chain against a copy taken
//! before an intrusion" — real, and worth having — but it does **not**
//! defend against an attacker with sustained, undetected write access to
//! this same database indefinitely. Closing that gap needs one of the two
//! rejected options above: ship anchors to somewhere this deployment
//! cannot itself rewrite (offsite replication of just this table, an
//! external notary, WORM storage), or accept that this control's job is to
//! make a *smash-and-grab* edit detectable, not to defend against a
//! standing, patient adversary. Both are real, named follow-up work, not
//! silently implied by "tamper-evident" in the issue title.
//!
//! # The race this design accepts, and why
//!
//! Each anchor covers `(periodStart, periodEnd]` — exclusive lower bound,
//! inclusive upper. `periodStart` is `None` (meaning "everything up to
//! `periodEnd`") only for the very first anchor; every anchor after that
//! inherits the previous anchor's own `periodEnd` as its `periodStart`, so
//! the covered ranges are contiguous with no gap and no overlap by
//! construction — *if* every row that will ever exist with `occurred_at`
//! in a given window is actually visible by the time that window's anchor
//! runs.
//!
//! It might not be. `occurred_at` is stamped inside the mutation's
//! transaction, before that transaction necessarily commits — a slow
//! transaction can stamp an `occurred_at` earlier than a fast one that
//! commits first. If a transaction is still open when this job draws its
//! `periodEnd` boundary and only commits afterward, its audit row's
//! `occurred_at` can land inside a window this job has already anchored —
//! and because windows only ever move forward, that row would never be
//! covered by *any* future anchor either, since the row it belongs "in"
//! has already passed.
//!
//! [`ANCHOR_LAG`] is the accepted mitigation, not a fix: `periodEnd` is
//! drawn as `now - ANCHOR_LAG`, not `now`, so only a transaction that stays
//! open longer than the lag can still slip through. Five minutes is a
//! large multiple of every write path in this codebase — audit-carrying
//! mutations commit within the same request, typically well under a
//! second — so the realistic risk is close to zero, not eliminated. A
//! genuinely stuck transaction that outlives the lag would leave its own
//! audit row permanently unanchored (still fully present and readable in
//! `cratestack_audit` — nothing is lost — just never folded into any
//! anchor's hash), which would look identical, from a verifier's chair, to
//! a row someone deleted from an anchored period and is hoping goes
//! unnoticed. This job does not currently detect that specific case (a row
//! that exists but was never covered, as opposed to a row that was covered
//! and no longer matches) — a documented, accepted gap, not a silent one.
//!
//! # What this job actually does, every run
//!
//! 1. Reads the most recent anchor, if any ([`latest_anchor`]).
//! 2. **Re-verifies the whole anchor chain's own internal linkage**
//!    ([`verify_chain_linkage`]) — cheap, `O(number of anchors)` (at most
//!    one row per scheduled run, ever), and does not touch `cratestack_audit`
//!    at all: every anchor's `prevChainHash` must equal the actual previous
//!    anchor's `chainHash`, and every anchor's own `chainHash` must still
//!    equal what recomputing it from that anchor's own stored fields
//!    produces. A mismatch is logged loudly (`error!`), every run, forever,
//!    until fixed — matching `reap_outbox`'s own "make a broken row loud,
//!    never silently swallow it" convention.
//! 3. **Re-verifies the most recent anchor's own row content**
//!    ([`verify_period_content`]) — re-reads `cratestack_audit` for exactly
//!    that anchor's `(periodStart, periodEnd]` and recomputes `rangeHash`,
//!    comparing against what is stored. Bounded to one period's worth of
//!    rows, not the whole history — see "explicitly out of scope" below for
//!    why a full-history re-verification is not this job's own hot path.
//! 4. Computes the next period — `periodStart` = the latest anchor's
//!    `periodEnd` (or `None` on the very first run), `periodEnd` = `now -
//!    ANCHOR_LAG` — and, if there is anything new to cover, folds every
//!    covered `cratestack_audit` row into a `rangeHash`, chains it onto the
//!    previous anchor's `chainHash` (or the fixed genesis sentinel), and
//!    writes the new `AuditAnchor` row. An anchor is written even when
//!    `rowCount` is zero — an unbroken chain, including "nothing happened"
//!    periods, is itself the thing that makes a *gap* in the chain (a
//!    period silently never anchored) visible as a gap, rather than
//!    indistinguishable from "quiet day."
//!
//! # Explicitly out of scope, named rather than silently dropped
//!
//! - **A full re-verification of every historical audit row, every run.**
//!   Cost grows with total audit history, not with one period — the
//!   opposite of this job's own "roughly cheap, runs daily forever" shape.
//!   An operator who needs that assurance can run the same
//!   [`verify_period_content`]/[`verify_chain_linkage`] functions this job
//!   already exposes as `pub`, walking every anchor, as a one-off — the
//!   functions exist and are tested; wiring a CLI subcommand around them is
//!   real, separate follow-up work, not built here.
//! - **External anchoring** — see "be honest about what this proves" above.
//! - **The row that exists but was never covered by any window** — see
//!   "the race this design accepts" above.
//!
//! # R1 exception, the seventh
//!
//! Same reasoning as `drain.rs`'s fifth and `reap_outbox.rs`'s sixth:
//! `cratestack_audit` is the framework's own internal bookkeeping table
//! (created lazily by `ensure_audit_table`), not one of `schema.cstack`'s
//! models — no delegate exists to read it, so there is no row-level policy
//! to bypass, no audit trail to skip (a `SELECT` isn't a mutation, and this
//! *is* the audit table besides), no `@version`/soft-delete concern.
//! `ci/assert-no-raw-sqlx.sh` and `CONTRIBUTING.md`'s own exceptions table
//! both name this file — and, as an eighth, test-only entry,
//! `crates/sms-worker/tests/anchor_audit_live_postgres.rs`, which has to
//! tamper with a raw `cratestack_audit` row directly to prove the
//! tamper-detection guard here can actually fail, the same "no delegate
//! exists for this table" reasoning applying from the attacker's side of
//! it as from the defender's.
//!
//! `AuditAnchor` itself is a real, new schema model (§68's own DDL change)
//! with a real delegate — reading/writing *it* never needs raw SQL, only
//! the underlying `cratestack_audit` rows being anchored do.

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use cratestack::sqlx;
use cratestack::{CoolContext, CoolError};
use sha2::{Digest, Sha256};
use sms_api::schema::{audit_anchor, AuditAnchor, Cratestack, CreateAuditAnchorInput, Job};
use tracing::{debug, error, info, warn};

use crate::jobs::JobHandler;

/// Safety margin subtracted from "now" before drawing an anchor's upper
/// boundary — see the module doc's "the race this design accepts" section.
/// A large multiple of every write path's real commit latency in this
/// codebase, not a value tuned against any measured worst case.
const ANCHOR_LAG: Duration = Duration::minutes(5);

/// Fixed 32-byte value folded in as the "previous row" when there is
/// nothing earlier to fold — i.e. the very first row of the very first
/// anchor's own range fold. Distinct from [`genesis_hex`], which is the
/// *anchor chain's* own starting value (`prevChainHash` on the first
/// anchor) — two different folds, two different genesis points, not
/// reused between them on purpose: conflating "no earlier row" with "no
/// earlier anchor" would make a range fold and a chain fold
/// indistinguishable if either one ever needed exactly zero inputs.
const ROW_FOLD_GENESIS: [u8; 32] = [0u8; 32];

/// `AuditAnchor.prevChainHash` on the very first anchor this deployment
/// ever writes — see `schema.cstack`'s own field doc for why this is a
/// fixed sentinel rather than `NULL`.
fn genesis_hex() -> String {
    "0".repeat(64)
}

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
        if let Some(start) = period_start {
            if period_end <= start {
                debug!("nothing new to anchor yet this run");
                return Ok(());
            }
        }

        let rows = fetch_audit_rows(db, period_start, period_end)
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

/// The most recent `AuditAnchor` by `periodEnd`, or `None` on a fresh
/// deployment that has never anchored anything.
async fn latest_anchor(
    db: &Cratestack,
    sys: &CoolContext,
) -> Result<Option<AuditAnchor>, CoolError> {
    let mut rows = db
        .audit_anchor()
        .find_many()
        .order_by(audit_anchor::periodEnd().desc())
        .limit(1)
        .run(sys)
        .await?;
    Ok(rows.pop())
}

/// Walks every `AuditAnchor`, oldest first, and checks two things per
/// anchor: its `prevChainHash` equals the actual previous anchor's
/// `chainHash` (or the genesis sentinel, for the oldest anchor on record),
/// and its own `chainHash` still equals what recomputing it from that
/// anchor's own stored `periodStart`/`periodEnd`/`rowCount`/`rangeHash`
/// produces. Returns a human-readable description per break found, rather
/// than failing the job outright — a linkage break is exactly the finding
/// this job exists to surface loudly, not a fault that should stop it from
/// also anchoring today's rows.
///
/// `pub` for the same reason every other job's own internals are: the live
/// tamper-detection test asserts against this directly, not by scraping a
/// log line.
pub async fn verify_chain_linkage(
    db: &Cratestack,
    sys: &CoolContext,
) -> Result<Vec<String>, CoolError> {
    let anchors = db
        .audit_anchor()
        .find_many()
        .order_by(audit_anchor::periodEnd().asc())
        .run(sys)
        .await?;

    let mut breaks = Vec::new();
    let mut previous_chain_hash: Option<String> = None;

    for anchor in &anchors {
        let expected_prev = previous_chain_hash.clone().unwrap_or_else(genesis_hex);
        if anchor.prevChainHash != expected_prev {
            breaks.push(format!(
                "anchor {} prevChainHash ({}) does not equal the expected previous link ({})",
                anchor.id, anchor.prevChainHash, expected_prev
            ));
        }

        let recomputed = compute_chain_hash_hex(
            &anchor.prevChainHash,
            anchor.periodStart,
            anchor.periodEnd,
            anchor.rowCount,
            &anchor.rangeHash,
        );
        if recomputed != anchor.chainHash {
            breaks.push(format!(
                "anchor {} chainHash ({}) does not match its own stored fields (recomputed {})",
                anchor.id, anchor.chainHash, recomputed
            ));
        }

        previous_chain_hash = Some(anchor.chainHash.clone());
    }

    Ok(breaks)
}

/// Re-reads `cratestack_audit` for exactly `anchor`'s own
/// `(periodStart, periodEnd]` and checks whether folding those rows still
/// produces `anchor.rangeHash`. `pub` for the same reason
/// [`verify_chain_linkage`] is.
pub async fn verify_period_content(
    db: &Cratestack,
    anchor: &AuditAnchor,
) -> Result<bool, sqlx::Error> {
    let rows = fetch_audit_rows(db, anchor.periodStart, anchor.periodEnd).await?;
    let recomputed = hex::encode(fold_rows(&rows));
    Ok(recomputed == anchor.rangeHash)
}

/// One `cratestack_audit` row, decoded — see the module doc for the exact
/// DDL this mirrors.
struct AuditRow {
    event_id: cratestack::uuid::Uuid,
    schema_name: String,
    model: String,
    operation: String,
    primary_key: serde_json::Value,
    actor: serde_json::Value,
    tenant: Option<String>,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
    request_id: Option<String>,
    occurred_at: DateTime<Utc>,
}

/// `true` if `error` is Postgres's `42P01 undefined_table` — same
/// treatment `reap_outbox.rs`'s own `is_undefined_table` gives
/// `cratestack_event_outbox`: a table that was never created has, by
/// construction, no rows to anchor.
fn is_undefined_table(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .as_deref()
        == Some("42P01")
}

/// Every `cratestack_audit` row with `occurred_at` in `(period_start,
/// period_end]` — `period_start = None` means no lower bound. Ordered
/// `(occurred_at, event_id)` so folding is deterministic across repeated
/// reads of the same, unmodified rows.
async fn fetch_audit_rows(
    db: &Cratestack,
    period_start: Option<DateTime<Utc>>,
    period_end: DateTime<Utc>,
) -> Result<Vec<AuditRow>, sqlx::Error> {
    #[allow(clippy::type_complexity)]
    type Row = (
        cratestack::uuid::Uuid,
        String,
        String,
        String,
        serde_json::Value,
        serde_json::Value,
        Option<String>,
        Option<serde_json::Value>,
        Option<serde_json::Value>,
        Option<String>,
        DateTime<Utc>,
    );

    let rows: Vec<Row> = match sqlx::query_as(
        "SELECT event_id, schema_name, model, operation, primary_key, actor, tenant, before, \
         after, request_id, occurred_at \
         FROM cratestack_audit \
         WHERE ($1::timestamptz IS NULL OR occurred_at > $1) AND occurred_at <= $2 \
         ORDER BY occurred_at ASC, event_id ASC",
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db.pool())
    .await
    {
        Ok(rows) => rows,
        Err(error) if is_undefined_table(&error) => Vec::new(),
        Err(error) => return Err(error),
    };

    Ok(rows
        .into_iter()
        .map(
            |(
                event_id,
                schema_name,
                model,
                operation,
                primary_key,
                actor,
                tenant,
                before,
                after,
                request_id,
                occurred_at,
            )| AuditRow {
                event_id,
                schema_name,
                model,
                operation,
                primary_key,
                actor,
                tenant,
                before,
                after,
                request_id,
                occurred_at,
            },
        )
        .collect())
}

/// Writes `bytes.len()` as an 8-byte big-endian prefix, then `bytes`
/// itself — the length-prefixed framing every variable-length field below
/// needs. Without it, two adjacent variable-length fields (e.g. `model` +
/// `operation`) could shift boundaries and hash identically for two
/// genuinely different rows — the exact "sentinel separator" class of bug
/// `AGENTS.md`'s own §2.0 already warns about for `pack()`/`unpack()`,
/// applied here to a hash instead of a delimited string.
fn write_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

/// A one-byte presence flag, then [`write_len_prefixed`] if present.
fn write_optional_str(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(text) => {
            hasher.update([1u8]);
            write_len_prefixed(hasher, text.as_bytes());
        }
        None => hasher.update([0u8]),
    }
}

/// A JSON value serialized with every object's keys sorted, recursively —
/// deterministic regardless of `serde_json`'s own `Map` ordering (which
/// varies with the `preserve_order` feature, itself subject to Cargo
/// feature unification from any dependency anywhere in the graph, not
/// something this crate fully controls) and regardless of Postgres's own
/// `jsonb` internal key order on the way back out through `sqlx`. Sorting
/// explicitly, rather than relying on either of those being stable,
/// removes the dependency on an assumption instead of just hoping it holds.
fn canonical_json(value: &serde_json::Value) -> String {
    fn sorted(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut ordered = serde_json::Map::new();
                for key in keys {
                    ordered.insert(key.clone(), sorted(&map[key]));
                }
                serde_json::Value::Object(ordered)
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(sorted).collect())
            }
            other => other.clone(),
        }
    }
    serde_json::to_string(&sorted(value)).unwrap_or_default()
}

/// One row's own digest — every column that can vary between two
/// otherwise-identical-looking rows, in a fixed, framed order. Changing
/// any field of any row changes this.
fn hash_audit_row(row: &AuditRow) -> [u8; 32] {
    let mut hasher = Sha256::new();
    write_len_prefixed(&mut hasher, row.event_id.as_bytes());
    write_len_prefixed(&mut hasher, row.schema_name.as_bytes());
    write_len_prefixed(&mut hasher, row.model.as_bytes());
    write_len_prefixed(&mut hasher, row.operation.as_bytes());
    write_len_prefixed(&mut hasher, canonical_json(&row.primary_key).as_bytes());
    write_len_prefixed(&mut hasher, canonical_json(&row.actor).as_bytes());
    write_optional_str(&mut hasher, row.tenant.as_deref());
    let before_json = row.before.as_ref().map(canonical_json);
    write_optional_str(&mut hasher, before_json.as_deref());
    let after_json = row.after.as_ref().map(canonical_json);
    write_optional_str(&mut hasher, after_json.as_deref());
    write_optional_str(&mut hasher, row.request_id.as_deref());
    hasher.update(row.occurred_at.timestamp_micros().to_be_bytes());
    hasher.finalize().into()
}

/// Sequential fold over every row in order:
/// `acc = SHA256(acc || hash_audit_row(row))`, starting from
/// [`ROW_FOLD_GENESIS`]. Equivalent tamper-evidence to a Merkle tree for
/// this use — every verification here always has full read access to the
/// underlying rows and recomputes from scratch, so a Merkle tree's real
/// advantage (proving membership without revealing the whole set) buys
/// nothing; a sequential fold gives the identical "any row changed or was
/// removed changes the result" property with less code.
fn fold_rows(rows: &[AuditRow]) -> [u8; 32] {
    let mut acc = ROW_FOLD_GENESIS;
    for row in rows {
        let row_hash = hash_audit_row(row);
        let mut hasher = Sha256::new();
        hasher.update(acc);
        hasher.update(row_hash);
        acc = hasher.finalize().into();
    }
    acc
}

/// `AuditAnchor.chainHash` — commits to `prevChainHash` *and* this
/// anchor's own metadata (`periodStart`/`periodEnd`/`rowCount`), not just
/// `rangeHash`, so tampering with an anchor row's own fields is exactly as
/// detectable as tampering with the audit rows it covers. Every input here
/// is fixed-width (a 64-character hex string, or a fixed-size integer), so
/// unlike [`hash_audit_row`] no length-prefixing is needed — there is no
/// adjacent-variable-length-field ambiguity to guard against.
fn compute_chain_hash_hex(
    prev_chain_hash_hex: &str,
    period_start: Option<DateTime<Utc>>,
    period_end: DateTime<Utc>,
    row_count: i64,
    range_hash_hex: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prev_chain_hash_hex.as_bytes());
    match period_start {
        Some(start) => {
            hasher.update([1u8]);
            hasher.update(start.timestamp_micros().to_be_bytes());
        }
        None => hasher.update([0u8]),
    }
    hasher.update(period_end.timestamp_micros().to_be_bytes());
    hasher.update(row_count.to_be_bytes());
    hasher.update(range_hash_hex.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_json, compute_chain_hash_hex, fold_rows, genesis_hex, hash_audit_row,
        AnchorAudit, AuditRow, ANCHOR_LAG,
    };
    use crate::jobs::JobHandler;
    use chrono::{TimeZone, Utc};

    #[test]
    fn kind_matches_the_scheduler_and_design_docs_naming() {
        assert_eq!(AnchorAudit.kind(), "anchor_audit");
    }

    #[test]
    fn genesis_hex_is_exactly_64_characters_matching_the_schemas_own_check() {
        let genesis = genesis_hex();
        assert_eq!(genesis.len(), 64);
        assert!(genesis.chars().all(|c| c == '0'));
    }

    #[test]
    fn anchor_lag_is_generous_relative_to_a_real_write_paths_commit_latency() {
        // See the module doc's "the race this design accepts" — five
        // minutes against writes that commit within one request.
        assert_eq!(ANCHOR_LAG, chrono::Duration::minutes(5));
    }

    fn sample_row(model: &str, occurred_at: chrono::DateTime<Utc>) -> AuditRow {
        AuditRow {
            event_id: cratestack::uuid::Uuid::from_u128(1),
            schema_name: String::new(),
            model: model.to_owned(),
            operation: "create".to_owned(),
            primary_key: serde_json::json!({"id": "abc123"}),
            actor: serde_json::json!({"id": "user1", "claims": null, "ip": null}),
            tenant: None,
            before: None,
            after: Some(serde_json::json!({"id": "abc123", "name": "test"})),
            request_id: Some("req-1".to_owned()),
            occurred_at,
        }
    }

    #[test]
    fn canonical_json_ignores_key_order() {
        let a = serde_json::json!({"z": 1, "a": 2, "m": {"y": 1, "b": 2}});
        let b = serde_json::json!({"a": 2, "m": {"b": 2, "y": 1}, "z": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
    }

    #[test]
    fn canonical_json_distinguishes_different_content() {
        let a = serde_json::json!({"a": 1});
        let b = serde_json::json!({"a": 2});
        assert_ne!(canonical_json(&a), canonical_json(&b));
    }

    #[test]
    fn hashing_the_same_row_twice_is_deterministic() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let row = sample_row("App", now);
        let row_again = sample_row("App", now);
        assert_eq!(hash_audit_row(&row), hash_audit_row(&row_again));
    }

    #[test]
    fn changing_any_field_changes_the_row_hash() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let base = sample_row("App", now);
        let base_hash = hash_audit_row(&base);

        let mut different_model = sample_row("App", now);
        different_model.model = "Provider".to_owned();
        assert_ne!(hash_audit_row(&different_model), base_hash);

        let mut different_after = sample_row("App", now);
        different_after.after = Some(serde_json::json!({"id": "abc123", "name": "tampered"}));
        assert_ne!(hash_audit_row(&different_after), base_hash);

        let mut different_time = sample_row("App", now + chrono::Duration::seconds(1));
        different_time.event_id = base.event_id;
        assert_ne!(hash_audit_row(&different_time), base_hash);
    }

    #[test]
    fn fold_rows_is_order_sensitive() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut first = sample_row("App", now);
        first.event_id = cratestack::uuid::Uuid::from_u128(1);
        let mut second = sample_row("Provider", now);
        second.event_id = cratestack::uuid::Uuid::from_u128(2);

        let forward = vec![clone_row(&first), clone_row(&second)];
        let reversed = vec![clone_row(&second), clone_row(&first)];

        // The same two rows, folded in the opposite order, must produce a
        // different digest — an interior swap must be detectable, the
        // same property a real deletion-and-reinsertion attack relies on
        // not holding.
        assert_ne!(fold_rows(&forward), fold_rows(&reversed));
    }

    /// Test-only: [`AuditRow`] deliberately derives no `Clone` in
    /// production code (nothing there ever needs to duplicate one), so
    /// this fixture helper exists purely so the test above can build two
    /// differently-ordered `Vec`s from the same two logical rows.
    fn clone_row(row: &AuditRow) -> AuditRow {
        AuditRow {
            event_id: row.event_id,
            schema_name: row.schema_name.clone(),
            model: row.model.clone(),
            operation: row.operation.clone(),
            primary_key: row.primary_key.clone(),
            actor: row.actor.clone(),
            tenant: row.tenant.clone(),
            before: row.before.clone(),
            after: row.after.clone(),
            request_id: row.request_id.clone(),
            occurred_at: row.occurred_at,
        }
    }

    #[test]
    fn fold_rows_of_zero_rows_is_the_fixed_genesis_fold() {
        // An empty period must still produce a stable, reproducible value
        // — a "nothing happened" period is anchored, not skipped (see the
        // module doc's own reasoning: an unbroken chain is what makes a
        // *gap* visible) — and that value is exactly the row-fold genesis,
        // unchanged, since the fold loop never executes.
        assert_eq!(fold_rows(&[]), super::ROW_FOLD_GENESIS);
    }

    #[test]
    fn compute_chain_hash_hex_is_64_hex_characters() {
        let hash = compute_chain_hash_hex(&genesis_hex(), None, Utc::now(), 0, &genesis_hex());
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn compute_chain_hash_hex_changes_with_any_input() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let base = compute_chain_hash_hex(&genesis_hex(), None, now, 3, &genesis_hex());

        assert_ne!(
            compute_chain_hash_hex(&"1".repeat(64), None, now, 3, &genesis_hex()),
            base,
            "prevChainHash must be covered"
        );
        assert_ne!(
            compute_chain_hash_hex(&genesis_hex(), Some(now), now, 3, &genesis_hex()),
            base,
            "periodStart must be covered"
        );
        assert_ne!(
            compute_chain_hash_hex(
                &genesis_hex(),
                None,
                now + chrono::Duration::seconds(1),
                3,
                &genesis_hex()
            ),
            base,
            "periodEnd must be covered"
        );
        assert_ne!(
            compute_chain_hash_hex(&genesis_hex(), None, now, 4, &genesis_hex()),
            base,
            "rowCount must be covered"
        );
        assert_ne!(
            compute_chain_hash_hex(&genesis_hex(), None, now, 3, &"1".repeat(64)),
            base,
            "rangeHash must be covered"
        );
    }
}
