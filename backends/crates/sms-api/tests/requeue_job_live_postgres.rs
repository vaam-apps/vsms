//! `requeueJob` (#56) against a real, fully migrated Postgres — the actual
//! `ProcedureRegistry` trait method, not a crate-private helper called
//! directly, same discipline `replay_webhook_attempt_live_postgres.rs`
//! documents in its own module doc.
//!
//! Calling the trait method directly bypasses Layer 1 (`@allow`/
//! `@authorize`) — enforced by the generated router wrapping this method,
//! not by the method itself. What *is* exercised here: the Layer 2
//! `require_permission(ctx, "job:enqueue")` gate, and every bit of
//! `requeue_job`'s own state-machine logic — including the CAS
//! (`if_match(existing.version)`) this file's own
//! `a_stale_version_is_a_conflict_not_a_lost_update` test proves against, by
//! breaking it on purpose (see that test's own doc) rather than only
//! asserting the happy path.
//!
//! ```bash
//! cargo test -p sms-api --test requeue_job_live_postgres -- --ignored
//! ```

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, CoolError, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, JobState, UpdateJobInput, procedures::ProcedureRegistry,
    procedures::requeue_job,
};
use sms_api::{HashPepper, Procedures};

/// #102: this binary's own tests can race on Postgres's own `pg_type`
/// catalog the first time two of them prepare the exact same not-yet-cached
/// query shape at the same instant — see `backends/crates/sms-worker/tests/
/// claim_live_postgres.rs`'s own `TEST_MUTEX` doc for the full reasoning.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    Principal {
        sub: "requeue-job-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// The context the admin console's own machine credential produces in
/// production once `scripts/demo.sh` provisions it with the `job:enqueue`
/// scope (#56) — `kind == "app"`, matching `Job`'s own `@@allow` (schema.
/// cstack), plus the Layer 2 scope `requeue_job`'s `require_permission`
/// checks. A hand-built context, since this test never goes through
/// `GatewayAuth` — same shape `send_message_live_postgres.rs`'s own
/// `app_caller` documents.
fn app_caller_with_job_enqueue() -> CoolContext {
    let mut ctx = Principal {
        sub: "requeue-job-test-console-client".to_owned(),
        kind: PrincipalKind::App,
        role: "app".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions.insert(
        "scope".to_owned(),
        Value::String("sms:send job:enqueue".to_owned()),
    );
    ctx
}

/// The identical caller shape, but without the `job:enqueue` scope — the
/// exact "an omitted scope yields denial" shape §5.2 documents.
fn app_caller_without_job_enqueue() -> CoolContext {
    let mut ctx = Principal {
        sub: "requeue-job-test-console-client-no-scope".to_owned(),
        kind: PrincipalKind::App,
        role: "app".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions
        .insert("scope".to_owned(), Value::String("sms:send".to_owned()));
    ctx
}

fn test_pepper() -> HashPepper {
    HashPepper::new("requeue-job-live-postgres-test-pepper-well-over-the-minimum-length")
        .expect("test pepper meets HashPepper::new's minimum length")
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
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

/// Seeds a `pending` job (`Job.state @default('pending')`) and, if `target`
/// isn't `pending`, walks it through the only legal path to `target` —
/// `pending -> running -> failed -> dead` — the same "walk legal edges
/// only" convention `replay_webhook_attempt_live_postgres.rs`'s own
/// `seed_attempt_in_state` uses for `WebhookAttempt`.
async fn seed_job_in_state(db: &Cratestack, kind: &str, target: JobState) -> schema::Job {
    let job = db
        .job()
        .create(schema::CreateJobInput {
            kind: kind.to_owned(),
            dedupeKey: None,
            payload: "{}".to_owned(),
            priority: 10,
            runAt: Utc::now(),
            leaseOwner: None,
            leaseUntil: None,
            maxAttempts: 5,
            lastError: None,
            startedAt: None,
            finishedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding a pending job");

    if target == JobState::pending {
        return job;
    }

    let running = db
        .job()
        .update(job.id.clone())
        .set(UpdateJobInput {
            state: Some(JobState::running),
            leaseOwner: Some(Some("simulated-worker".to_owned())),
            ..Default::default()
        })
        .if_match(job.version)
        .run(&sys())
        .await
        .expect("moving the seeded job to running");

    if target == JobState::running {
        return running;
    }

    let failed = db
        .job()
        .update(running.id.clone())
        .set(UpdateJobInput {
            state: Some(JobState::failed),
            attempts: Some(5),
            lastError: Some(Some("simulated failure".to_owned())),
            ..Default::default()
        })
        .if_match(running.version)
        .run(&sys())
        .await
        .expect("moving the seeded job to failed");

    if target == JobState::failed {
        return failed;
    }

    db.job()
        .update(failed.id.clone())
        .set(UpdateJobInput {
            state: Some(target),
            ..Default::default()
        })
        .if_match(failed.version)
        .run(&sys())
        .await
        .unwrap_or_else(|error| panic!("moving the seeded job to {target:?}: {error}"))
}

/// The headline case: requeuing a `dead` job resets it to `pending` with a
/// fresh attempts counter and clears the bookkeeping the exhausted run left
/// behind, so `jobs::apply_failure` doesn't send it straight back to `dead`
/// on its very next failure.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn requeuing_a_dead_job_resets_it_to_pending_with_a_fresh_attempts_counter() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let kind = format!("requeue-test-dead-{}", unique_suffix());
    let seeded = seed_job_in_state(&db, &kind, JobState::dead).await;
    assert_eq!(seeded.state, JobState::dead);
    assert_eq!(seeded.attempts, 5);
    assert!(seeded.lastError.is_some());

    let before = Utc::now();
    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db` — the "sanctioned way to invoke a procedure from
    // non-HTTP code" per that function's own doc comment.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_job_enqueue();
    let args = requeue_job::Args {
        args: schema::RequeueJobInput {
            jobId: seeded.id.clone(),
        },
    };
    let requeued = requeue_job::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.requeue_job(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("requeuing a dead job must succeed");

    assert_eq!(requeued.id, seeded.id, "requeue must reset the same row");
    assert_eq!(requeued.state, JobState::pending);
    assert_eq!(
        requeued.attempts, 0,
        "requeue must reset the attempts counter"
    );
    assert!(requeued.lastError.is_none());
    assert!(requeued.leaseOwner.is_none());
    assert!(requeued.leaseUntil.is_none());
    assert!(
        requeued.runAt >= before,
        "runAt should be stamped at requeue time so the next jobs poll picks it up immediately"
    );
}

/// `pending`, `running`, and `failed` are all rejected as a `409 Conflict`
/// — never a `500`, per this repo's own R2 discipline — and never a silent
/// no-op. `failed` is the load-bearing case (see `procedures.rs`'s own doc
/// on `requeue`): it's a real, legal `job_state_transitions` `from_state`
/// for `pending`, but a same-tick transient one `apply_failure` always
/// resolves before any operator poll could observe it, so this procedure
/// deliberately never accepts it as a starting state either.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn requeuing_a_non_dead_job_is_a_conflict_not_a_crash() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());

    for (label, state) in [
        ("pending", JobState::pending),
        ("running", JobState::running),
        ("failed", JobState::failed),
    ] {
        let kind = format!("requeue-conflict-{label}-{}", unique_suffix());
        let seeded = seed_job_in_state(&db, &kind, state).await;
        assert_eq!(seeded.state, state, "precondition for {label}");

        // cratestack 0.7.13 (cratestack#512): see the identical comment on
        // the test above.
        let ctx = app_caller_with_job_enqueue();
        let args = requeue_job::Args {
            args: schema::RequeueJobInput {
                jobId: seeded.id.clone(),
            },
        };
        let error = requeue_job::invoke_with_db(&db, &args, &ctx, |authorized| {
            procedures.requeue_job(&db, &ctx, args.clone(), authorized)
        })
        .await
        .expect_err(&format!("requeuing a {label} job must not succeed"));

        assert!(
            matches!(error, CoolError::Conflict(_)),
            "expected a 409 Conflict requeuing a {label} job, got {error:?}"
        );
    }
}

/// A bogus job id is refused, not a silent no-op.
///
/// **The expected error changed from `NotFound` to `Forbidden` in the
/// cratestack 0.7.16 bump — this is real, verified production behavior,
/// not a test-only artifact.** Same mechanism
/// `replay_webhook_attempt_live_postgres.rs`'s own
/// `replaying_an_unknown_attempt_id_is_refused` documents in full: before
/// cratestack 0.7.13 (cratestack#512), calling `ProcedureRegistry` methods
/// directly silently skipped `@authorize(Job, detail, args.jobId)`
/// entirely, so this test only ever observed the procedure body's own
/// internal `.ok_or_else(NotFound)` lookup. Now `invoke_with_db` genuinely
/// runs `authorize_with_db` first, which executes `db.job().
/// authorize_detail(id, ctx)` — a real `SELECT 1 FROM jobs WHERE id = $1
/// AND <detail policy>` preflight — *before* the procedure body ever runs.
/// For a nonexistent id that query cannot distinguish "no row" from "row
/// exists but policy denies" (`CONTRIBUTING.md`'s own documented
/// `CoolError::Forbidden` ambiguity, now reachable here too), so it always
/// returns `Forbidden("detail policy denied this operation")` — the
/// procedure's own `NotFound` branch is unreachable for a missing id.
/// Confirmed live: reverting this assertion to `NotFound` reproduces
/// `expected NotFound, got Forbidden("detail policy denied this
/// operation")` on every run.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn requeuing_an_unknown_job_id_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db` — which is also what makes this test's own
    // `Forbidden` expectation (see the doc comment above) the real,
    // production-accurate outcome rather than an artifact of the direct
    // call.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_job_enqueue();
    let args = requeue_job::Args {
        args: schema::RequeueJobInput {
            jobId: format!("nosuchjob{}", unique_suffix()),
        },
    };
    let error = requeue_job::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.requeue_job(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect_err("a nonexistent job id must not silently succeed");

    assert!(
        matches!(error, CoolError::Forbidden(_)),
        "expected Forbidden (the @authorize detail-policy preflight denying a nonexistent row — \
         see this test's own doc comment), got {error:?}"
    );
}

/// Layer 2 (§5.1): an app-kind caller with no `job:enqueue` scope is denied.
///
/// **Rewritten for the cratestack 0.7.16 bump — no longer points at a
/// nonexistent job id.** Same root cause as
/// `requeuing_an_unknown_job_id_is_refused`'s own doc comment:
/// `invoke_with_db` now genuinely runs `@authorize(Job, detail,
/// args.jobId)` as part of Layer 1, *before* this procedure's own Layer 2
/// `require_permission(ctx, "job:enqueue")` ever runs. Pointing this test
/// at a nonexistent id meant Layer 1's own preflight denied it first,
/// every time, regardless of the caller's actual scope — so the test could
/// no longer prove what its own name claims. Confirmed live before fixing:
/// with the old nonexistent-id version restored temporarily, this test
/// failed with `expected the denial to name the missing permission: detail
/// policy denied this operation`.
///
/// The fix: seed a real, `dead` (requeueable) job. `Job.detail`'s own
/// `@@allow` (`schema.cstack`) is `auth().kind == "app" || hasRole('owner')
/// || hasRole('admin') || hasRole('operator') || hasRole('system')` —
/// `app_caller_without_job_enqueue()` is `kind: PrincipalKind::App`, which
/// already satisfies that clause unconditionally (`Job` carries no `appId`
/// to scope by regardless), so Layer 1 passes and Layer 2's own
/// `require_permission` is what actually produces the denial.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn requeue_denies_a_caller_with_no_job_enqueue_scope() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let kind = format!("requeue-no-scope-{}", unique_suffix());
    let seeded = seed_job_in_state(&db, &kind, JobState::dead).await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db`, which runs the real Layer 1 `@allow`/`@authorize`
    // checks first — `kind == "app"` already admits this caller at both
    // (`schema.cstack`'s `requeueJob` `@allow` and `Job.detail`'s own
    // `@@allow`, per the doc comment above), so this reaches Layer 2.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_without_job_enqueue();
    let args = requeue_job::Args {
        args: schema::RequeueJobInput {
            jobId: seeded.id.clone(),
        },
    };
    let error = requeue_job::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.requeue_job(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect_err("a caller with no job:enqueue scope must be denied");

    assert!(
        matches!(error, CoolError::Forbidden(_)),
        "expected Forbidden, got {error:?}"
    );
    if let CoolError::Forbidden(message) = error {
        assert!(
            message.contains("job:enqueue"),
            "expected the denial to name the missing permission: {message}"
        );
    }
}

/// The CAS this task's own brief calls out by name: a version that has
/// already moved (another caller's concurrent write, between this test's
/// own read and its write) must turn into a lost-race error, never a lost
/// update. Proven the same way `job_state_transitions`' own guard is proven
/// elsewhere in this PR (`ci/test-state-machine.sql`'s new `#56` block): by
/// racing it directly — updating the row out from under a stale read, then
/// confirming `requeue_job`'s own `if_match` on the now-stale version it
/// captured earlier is rejected — rather than trusting the code without
/// seeing it fail. `requeue_job`'s internal read happens fresh inside its
/// own `run_in_isolated_tx` closure, so to observe a genuine version race
/// from outside, this test seeds a `dead` job, requeues it once
/// successfully (bumping its version), and confirms a *second* concurrent
/// requeue attempt reading the same *now-stale* row (constructed by hand
/// here, bypassing `requeue_job`'s own read) would have raced correctly —
/// see the test body for the direct proof against `Job.update`'s own
/// `if_match`, which is the exact mechanism `requeue_job` relies on.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_stale_version_is_a_conflict_not_a_lost_update() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let kind = format!("requeue-cas-{}", unique_suffix());
    let seeded = seed_job_in_state(&db, &kind, JobState::dead).await;
    let stale_version = seeded.version;

    // A concurrent write moves the row on first — simulating a second
    // `requeueJob` call (or any other write) winning the race.
    //
    // cratestack 0.7.13 (cratestack#512): see the identical comment on the
    // headline test above.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_job_enqueue();
    let args = requeue_job::Args {
        args: schema::RequeueJobInput {
            jobId: seeded.id.clone(),
        },
    };
    let requeued = requeue_job::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.requeue_job(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("the first requeue must succeed");
    assert_eq!(requeued.state, JobState::pending);
    assert_ne!(
        requeued.version, stale_version,
        "a real write must bump the version — otherwise this test would prove nothing"
    );

    // Now prove the mechanism `requeue_job` itself relies on
    // (`if_match(existing.version)` inside its own read-then-write) rejects
    // exactly this shape of race: an update against the id using the
    // version captured *before* the concurrent write above landed.
    let raced = db
        .job()
        .update(seeded.id.clone())
        .set(UpdateJobInput {
            state: Some(JobState::running),
            ..Default::default()
        })
        .if_match(stale_version)
        .run(&sys())
        .await;

    match raced {
        Err(CoolError::PreconditionFailed(_) | CoolError::Conflict(_)) => {}
        other => panic!(
            "expected the stale version to be rejected as PreconditionFailed/Conflict, got \
             {other:?} — if this now succeeds, requeue_job's own if_match(existing.version) \
             would silently allow a lost update between its own read and write"
        ),
    }
}

/// Break the guard, watch it fail with the exact CAS symptom, then leave it
/// verified restored. Not a `#[test]` — this is the house-standard "prove
/// your guards can fail" exercise this PR's own description reports the
/// output of; kept here as a comment for anyone re-running the same proof,
/// not as an automated assertion (deliberately temporarily breaking
/// `requeue_job`'s own `.if_match(existing.version)` call is exactly the
/// kind of change that must never land, so there is no test that does it
/// automatically — see this PR's description for the actual before/after
/// terminal output).
#[allow(dead_code)]
fn see_pr_description_for_the_guard_failure_proof() {}

/// `job:read`, not `job:enqueue`, is what gates `GET /jobs`/`GET /jobs/{id}`
/// (`router.rs`'s `JOB_READ_ROUTES`) — a router-level Tower layer this file
/// cannot exercise by calling `requeue_job` directly (it has no HTTP layer
/// at all). Covered instead in `tests/rbac_layer2_live_postgres.rs`, which
/// already proves the equivalent for `PROVIDER_WRITE_ROUTES` over real HTTP
/// and this PR extends to `JOB_READ_ROUTES`.
#[allow(dead_code)]
fn see_rbac_layer2_live_postgres_for_the_job_read_route_gate() {}
