//! Proves `#68`'s `anchor_audit` job
//! (`backends/crates/sms-worker/src/jobs/anchor_audit.rs`) against a real, fully
//! migrated Postgres.
//!
//! Three things, each load-bearing to the design that module's own doc
//! lays out:
//!
//! - A new anchor genuinely covers real `cratestack_audit` rows and chains
//!   correctly off whatever anchor came before it.
//! - **The house standard this task exists to satisfy: a guard that only
//!   ever asserts "an anchor was written" proves nothing about
//!   tamper-evidence.** [`anchoring_covers_new_rows_chains_correctly_and_detects_a_tampered_row`]
//!   and [`a_tampered_anchor_row_breaks_chain_linkage`] each tamper with a
//!   real row through a real, direct SQL statement — simulating exactly the
//!   attacker this job defends against, one with Postgres access but no
//!   delegate — then assert the corresponding verification actually flips
//!   from pass to fail, then restore the row and assert it flips back. Per
//!   the module's own "be honest about what this proves" section, this
//!   demonstrates the two things a same-database chain *can* catch
//!   (an edited row within an already-anchored period; a directly-edited
//!   anchor row that a later anchor still chains against) — not the one
//!   thing it structurally cannot (deleting the single most recent anchor
//!   before anything else references it).
//! - The `JobHandler` entry point (`kind`/`run`) actually reaches the same
//!   logic `run_at` does, against a live database, not just a dispatch
//!   table with a matching string.
//!
//! # Shared database, and the trap that shaped this file's own structure
//!
//! `AuditAnchor` is write-once by construction — no `@@allow("update", ...)`
//! or `@@allow("delete", ...)` clause exists at all (`schema.cstack`'s own
//! doc on the model) — so unlike most of this crate's other live suites,
//! nothing in this file can reset the table between test functions, and
//! `sms_test_support` gives this whole binary one database, not one per
//! test.
//!
//! **Only one test function in this file ever calls `run_at` with a `now`
//! shifted ahead of real wall-clock time, and that is deliberate, found the
//! hard way.** `cratestack_audit.occurred_at` has no delegate seam to
//! backdate (it's `chrono::Utc::now()`, stamped by the framework itself at
//! write time) — the only way to prove a freshly seeded row falls inside an
//! anchor's covered period is to push the anchor's own `now` argument
//! *forward*, past `ANCHOR_LAG`, instead. The first cut of this suite did
//! that in three separate test functions and immediately started failing
//! `rowCount >= 1` on whichever ran second or third — reproduced and
//! confirmed against `audit_anchors` directly with `psql` before fixing
//! (see the PR description for the exact output). The mechanism: an
//! anchor's `periodStart` is inherited from whatever the *previous* anchor's
//! `periodEnd` already is, and `periodEnd` only ever moves forward, never
//! back. Once *any* call pushes it minutes ahead of real time (which
//! covering a just-now row against a fixed 5-minute `ANCHOR_LAG`
//! necessarily requires), every *subsequent* call's own freshly-seeded row
//! — created at genuine, unshifted wall-clock time only milliseconds later
//! — is already older than that inherited floor, permanently, for the rest
//! of the process. There is no way to walk `periodStart` back down within a
//! fast-running test suite; real time would have to actually elapse. So
//! this file confines the forward shift to exactly one test function
//! ([`anchoring_covers_new_rows_chains_correctly_and_detects_a_tampered_row`]);
//! every other test's own `run_at` call uses genuine, unshifted
//! `Utc::now()` (which only ever anchors up to *5 minutes in the past*,
//! never poisoning the floor for whoever runs next) and does not depend on
//! a specific fresh row being covered — see each test's own doc for how it
//! stays valid regardless of what already exists in the table.
//!
//! Ignored by default, same convention as this crate's other live suites.
//! Run explicitly:
//!
//! ```bash
//! cargo test -p sms-worker --test anchor_audit_live_postgres -- --ignored
//! ```

use chrono::{Duration as ChronoDuration, Utc};
use cratestack::CratestackContext;
use cratestack::sqlx;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::audit_log::{verify_chain_linkage, verify_period_content};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, AuditAnchor, Cratestack, audit_anchor};
use sms_worker::jobs::JobHandler;
use sms_worker::jobs::anchor_audit::AnchorAudit;

/// Same reasoning as every other live suite's own copy of this mutex — see
/// `claim_live_postgres.rs`'s own `TEST_MUTEX` doc (#102). Load-bearing
/// here for a second reason beyond the usual `pg_type` catalog race: it is
/// also what guarantees the tamper tests below never race a concurrently
/// running test function's own audit-row-producing seed.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CratestackContext {
    Principal {
        sub: "sms-worker-anchor-audit-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CratestackContext {
    Principal {
        sub: "sms-worker-anchor-audit-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .subsec_nanos();
    format!("{:06x}", (u64::from(nanos).wrapping_add(n)) % 0x0100_0000)
}

async fn db() -> Cratestack {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

/// Creates one `App` (a real `@@audit`-tagged model) under `owner()` — the
/// cheapest way to guarantee at least one fresh `cratestack_audit` row with
/// a real, current `occurred_at`. Returns the created row's own id, used to
/// target exactly this row's audit entry when tampering, rather than
/// guessing by model name/time alone.
async fn seed_audited_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "anchor audit test app".to_owned(),
            slug: format!("anchor-audit-test-{}", unique_suffix()),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the app")
        .id
}

/// The current latest `AuditAnchor`, if any — a small, test-local
/// equivalent of `anchor_audit.rs`'s own private `latest_anchor`, since
/// that helper isn't `pub` (nothing outside the job itself needs "the
/// latest anchor" as opposed to the two `pub` verification functions this
/// suite actually exercises).
async fn latest_anchor(db: &Cratestack) -> Option<AuditAnchor> {
    db.audit_anchor()
        .find_many()
        .order_by(audit_anchor::periodEnd().desc())
        .limit(1)
        .run(&sys())
        .await
        .expect("reading the latest audit anchor")
        .into_iter()
        .next()
}

/// The one and only forward-shifted `now` this suite ever uses — see the
/// module doc's own section on why this must stay confined to a single
/// call site. Ten minutes clears `anchor_audit.rs`'s own 5-minute
/// `ANCHOR_LAG` with room to spare; deliberately not importing that
/// private constant, since this test does not need to track its exact
/// value.
fn forward_shifted_now() -> chrono::DateTime<Utc> {
    Utc::now() + ChronoDuration::minutes(10)
}

/// The full positive-path story, in one continuous timeline so the only
/// forward clock shift in this suite (see the module doc) cannot be
/// reordered relative to any other test's own real-time-only calls:
///
/// 1. Seed a real `App` (one fresh `cratestack_audit` row).
/// 2. Anchor it with [`forward_shifted_now`] and assert the new anchor
///    covers that row, chains correctly off whatever anchor existed before
///    (relative, not assumed to be the first ever — this suite's database
///    is shared across test functions), and independently re-verifies.
/// 3. **Tamper with the raw `cratestack_audit` row directly** (the house
///    standard: prove a guard can fail, not just that it can pass) and
///    assert `verify_period_content` flips to `false`.
/// 4. Restore the row and assert verification passes again.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn anchoring_covers_new_rows_chains_correctly_and_detects_a_tampered_row() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let before = latest_anchor(&db).await;
    let app_id = seed_audited_app(&db).await;

    AnchorAudit
        .run_at(&db, &sys(), forward_shifted_now())
        .await
        .expect("anchor_audit run_at succeeds");

    let anchor = latest_anchor(&db)
        .await
        .expect("an anchor must exist after a run that had something to cover");

    assert_eq!(
        anchor.periodStart,
        before.as_ref().map(|previous| previous.periodEnd),
        "the new anchor's periodStart must equal the previous anchor's periodEnd (or None, if \
         this really is the first anchor) — no gap, no overlap"
    );
    let expected_prev_chain_hash = before
        .as_ref()
        .map_or_else(|| "0".repeat(64), |previous| previous.chainHash.clone());
    assert_eq!(
        anchor.prevChainHash, expected_prev_chain_hash,
        "the new anchor must chain off the previous anchor's own chainHash (or the genesis \
         sentinel, if this really is the first anchor)"
    );
    assert!(
        anchor.rowCount >= 1,
        "the anchor must cover at least this test's own freshly seeded App-create audit row"
    );

    // Baseline: before any tampering, the anchor must verify. Asserting
    // this first is what makes the tamper assertion below meaningful — a
    // guard that is broken in the other direction (always reports a
    // mismatch) would "pass" a tamper test that skipped this step.
    assert!(
        verify_period_content(&db, &anchor)
            .await
            .expect("verify_period_content succeeds"),
        "baseline: a freshly written, untampered anchor must verify against the live \
         cratestack_audit rows it was computed from"
    );
    assert!(
        verify_chain_linkage(&db, &sys())
            .await
            .expect("verify_chain_linkage succeeds")
            .is_empty(),
        "a legitimately-written chain must have no linkage breaks"
    );

    // `primary_key_from_snapshot` (`cratestack-sqlx`'s own `audit/redact.rs`)
    // stores the bare scalar value, not `{"id": ...}` — confirmed by reading
    // a real row back with `psql` before writing this query (`primary_key`
    // is literally `"c3cea7..."`, a JSON string, not a JSON object), not
    // assumed from the column's own name. `primary_key = $1` against a
    // `serde_json::Value::String` is the correct comparison; `primary_key
    // ->> 'id'` (object-field extraction on a JSON string) silently returns
    // SQL `NULL` and matches nothing — this test's own first run caught
    // that the hard way, via `RowNotFound`.
    let target_primary_key = serde_json::Value::String(app_id.clone());

    // Capture the real, untampered `after` snapshot so it can be restored
    // exactly, not just overwritten with different-but-plausible JSON.
    let original_after: serde_json::Value = sqlx::query_scalar(
        "SELECT after FROM cratestack_audit WHERE primary_key = $1 AND model = 'App'",
    )
    .bind(&target_primary_key)
    .fetch_one(db.pool())
    .await
    .expect("reading the real audit row's own after-snapshot before tampering");

    let tampered = serde_json::json!({
        "tampered": "by anchoring_covers_new_rows_chains_correctly_and_detects_a_tampered_row"
    });
    sqlx::query("UPDATE cratestack_audit SET after = $1 WHERE primary_key = $2 AND model = 'App'")
        .bind(&tampered)
        .bind(&target_primary_key)
        .execute(db.pool())
        .await
        .expect(
            "tampering with the audit row directly, simulating an attacker with only \
             Postgres access and no delegate — see anchor_audit.rs's own R1 exception",
        );

    let verified_while_tampered = verify_period_content(&db, &anchor)
        .await
        .expect("verify_period_content succeeds even against a tampered row");
    assert!(
        !verified_while_tampered,
        "TAMPER TEST FAILED TO DETECT TAMPERING: verify_period_content still reported true \
         after the underlying cratestack_audit row's own after-snapshot was overwritten \
         directly — the guard proves nothing if it cannot fail here"
    );

    // Restore, and prove restoring actually un-breaks verification too —
    // closing the loop rather than leaving the database (shared by every
    // other test in this binary) permanently tampered.
    sqlx::query("UPDATE cratestack_audit SET after = $1 WHERE primary_key = $2 AND model = 'App'")
        .bind(&original_after)
        .bind(&target_primary_key)
        .execute(db.pool())
        .await
        .expect("restoring the audit row's original after-snapshot");

    assert!(
        verify_period_content(&db, &anchor)
            .await
            .expect("verify_period_content succeeds"),
        "restoring the tampered row's original content must restore a passing verification"
    );
}

/// The other half of the house standard, for [`verify_chain_linkage`]
/// rather than [`verify_period_content`]: tampers with an `AuditAnchor`
/// row *directly* — `AuditAnchor` has no `@@allow("update", ...)` at all,
/// so there is no delegate path to reach this row through, legitimate or
/// otherwise, which is exactly why this needs the same raw-SQL R1
/// exception the sibling test does, applied to the anchor table instead of
/// the audit table.
///
/// Deliberately does **not** seed its own fresh row or use
/// [`forward_shifted_now`] — see the module doc's own section on why only
/// one test in this file may do that. Instead it ensures *some* anchor
/// exists via a harmless, real-time `run_at(..., Utc::now())` call (a
/// no-op if one already does — `run_at`'s own "nothing new to anchor yet"
/// early return, or a fresh zero-row anchor if none exists at all) and
/// tampers whichever one [`latest_anchor`] returns. `verify_chain_linkage`
/// does not care whether that anchor's own content is fresh or inherited
/// from another test — only that its metadata is internally consistent.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_tampered_anchor_row_breaks_chain_linkage() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    AnchorAudit
        .run_at(&db, &sys(), Utc::now())
        .await
        .expect("anchor_audit run_at succeeds");
    let anchor = latest_anchor(&db)
        .await
        .expect("an anchor must exist: either just created, or left by another test");

    assert!(
        verify_chain_linkage(&db, &sys())
            .await
            .expect("verify_chain_linkage succeeds")
            .is_empty(),
        "baseline: the chain must have no breaks before any tampering"
    );

    // Tamper with the anchor's own metadata directly — rowCount, not
    // rangeHash/chainHash themselves, to prove the design decision that
    // chainHash commits to an anchor's own fields, not only to the audit
    // rows it covers (see anchor_audit.rs's own doc on
    // `compute_chain_hash_hex`).
    sqlx::query("UPDATE audit_anchors SET row_count = row_count + 1 WHERE id = $1")
        .bind(&anchor.id)
        .execute(db.pool())
        .await
        .expect("tampering with the anchor row's own rowCount directly");

    let breaks_while_tampered = verify_chain_linkage(&db, &sys())
        .await
        .expect("verify_chain_linkage succeeds even against a tampered anchor row");
    assert!(
        !breaks_while_tampered.is_empty(),
        "TAMPER TEST FAILED TO DETECT TAMPERING: verify_chain_linkage reported no breaks after \
         an AuditAnchor row's own rowCount was overwritten directly — the guard proves nothing \
         if it cannot fail here"
    );
    assert!(
        breaks_while_tampered
            .iter()
            .any(|detail| detail.contains(&anchor.id)),
        "the reported break must name the tampered anchor's own id, not just \"something's \
         wrong\": {breaks_while_tampered:?}"
    );

    sqlx::query("UPDATE audit_anchors SET row_count = row_count - 1 WHERE id = $1")
        .bind(&anchor.id)
        .execute(db.pool())
        .await
        .expect("restoring the anchor row's original rowCount");

    assert!(
        verify_chain_linkage(&db, &sys())
            .await
            .expect("verify_chain_linkage succeeds")
            .is_empty(),
        "restoring the tampered anchor's original rowCount must restore a clean chain"
    );
}

/// End-to-end sanity on the `JobHandler` entry point itself — `run_at` is
/// what every test above exercises directly; this proves the
/// `JobHandler::run`/`kind` wiring `default_registry` depends on also does
/// something real against a live database, the same convention every
/// other job's own live suite closes with. Uses real, unshifted time
/// internally (`AnchorAudit::run` always calls `run_at(..., Utc::now())`),
/// so — like the sibling test above — it never depends on covering a
/// specific fresh row and stays valid regardless of what the other tests
/// in this binary already did to the shared table.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_job_handler_entry_point_runs_without_error_against_a_live_database() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    seed_audited_app(&db).await;
    let job = db
        .job()
        .create(schema::CreateJobInput {
            kind: "anchor_audit".to_owned(),
            dedupeKey: None,
            payload: "{}".to_owned(),
            priority: 100,
            runAt: Utc::now(),
            leaseOwner: None,
            leaseUntil: None,
            maxAttempts: 3,
            lastError: None,
            startedAt: None,
            finishedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the anchor_audit job");

    let outcome = AnchorAudit.run(&db, &sys(), &job).await;
    assert!(
        outcome.is_ok(),
        "anchor_audit's JobHandler::run must succeed: {outcome:?}"
    );
    assert_eq!(AnchorAudit.kind(), "anchor_audit");
}
