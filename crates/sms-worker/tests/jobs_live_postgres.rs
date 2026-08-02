//! Proves `Role::Jobs`/`Role::Scheduler`'s real bodies (#35) against a
//! real, fully migrated Postgres: the `Claimable for Job` lease/reclaim
//! discipline, the `jobs` role's claim-run-transition loop including
//! backoff and `dead` exhaustion, the `scheduler` role's cadence/dedupe
//! behaviour, and the one real job kind wired up, `expire_stale`.
//!
//! Ignored by default, same convention as this crate's other live suites.
//! Run explicitly:
//!
//! ```bash
//! docker run --rm -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:16
//! createdb vsms_check
//! DATABASE_URL=postgres://postgres:postgres@localhost/vsms_check ./ci/apply-migrations.sh
//! DATABASE_URL=postgres://postgres:postgres@localhost/vsms_check \
//!     cargo test -p sms-worker --test jobs_live_postgres -- --ignored
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, job, message, Cratestack, Encoding, Job, MessageClass, MessageState, OperatorCode,
    UpdateJobInput,
};
use sms_worker::jobs::expire_stale::ExpireStale;
use sms_worker::jobs::{self, JobHandler, Registry};
use sms_worker::scheduler::{self, RecurringJobSpec};
use sms_worker::WorkerContext;

fn sys() -> CoolContext {
    Principal {
        sub: "sms-worker-jobs-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "sms-worker-jobs-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .subsec_nanos();
    format!("{:06x}", (u64::from(nanos).wrapping_add(n)) % 0x0100_0000)
}

async fn db() -> Cratestack {
    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must point at a fully migrated database — see module docs");
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

async fn seed_job(db: &Cratestack, kind: &str, max_attempts: i64) -> Job {
    db.job()
        .create(schema::CreateJobInput {
            kind: kind.to_owned(),
            dedupeKey: None,
            payload: "{}".to_owned(),
            priority: 500,
            runAt: Utc::now(),
            leaseOwner: None,
            leaseUntil: None,
            maxAttempts: max_attempts,
            lastError: None,
            startedAt: None,
            finishedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the job")
}

async fn reload_job(db: &Cratestack, id: &str) -> Job {
    db.job()
        .find_many()
        .where_expr(FilterExpr::from(job::id().eq(id.to_owned())))
        .limit(1)
        .run(&owner())
        .await
        .expect("reloading the job")
        .into_iter()
        .next()
        .expect("the job still exists")
}

fn worker_context(db: &Cratestack) -> WorkerContext {
    WorkerContext {
        db: db.clone(),
        // jobs/scheduler never touch the provider — see jobs.rs's own
        // module doc for why this crate holds `Arc<dyn SmsProvider>` at
        // all (dispatch's need, not this role's).
        provider: Arc::new(NeverCalledProvider),
    }
}

struct NeverCalledProvider;

#[async_trait]
impl sms_provider::SmsProvider for NeverCalledProvider {
    fn key(&self) -> &str {
        unimplemented!("jobs/scheduler never call the provider")
    }
    fn capabilities(&self) -> sms_provider::Capabilities {
        unimplemented!("jobs/scheduler never call the provider")
    }
    async fn submit(
        &self,
        _req: &sms_provider::SubmitRequest,
    ) -> Result<sms_provider::SubmitAck, sms_provider::ProviderError> {
        unimplemented!("jobs/scheduler never call the provider")
    }
    fn parse_dlr(
        &self,
        _raw: &sms_provider::RawCallback,
    ) -> Result<Vec<sms_provider::DeliveryUpdate>, sms_provider::ProviderError> {
        unimplemented!("jobs/scheduler never call the provider")
    }
    async fn health(&self) -> sms_provider::Health {
        unimplemented!("jobs/scheduler never call the provider")
    }
}

/// A handler that counts its own calls and always produces the same
/// outcome — `kind` is caller-supplied so each test registers it under a
/// fresh, unique string (this database is never reset between runs).
struct ScriptedHandler {
    kind_owned: String,
    calls: Arc<AtomicUsize>,
    succeed: bool,
}

#[async_trait]
impl JobHandler for ScriptedHandler {
    fn kind(&self) -> &'static str {
        // Leaked deliberately: `JobHandler::kind` returns `&'static str`
        // (matching `SmsProvider::key`'s own shape), but this test needs a
        // fresh string per run. A handful of leaked short strings across a
        // whole test binary run is a non-issue — no production code path
        // ever constructs a `JobHandler` with a caller-chosen `kind`.
        Box::leak(self.kind_owned.clone().into_boxed_str())
    }

    async fn run(&self, _db: &Cratestack, _sys: &CoolContext, _job: &Job) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.succeed {
            Ok(())
        } else {
            Err("scripted failure".to_owned())
        }
    }
}

fn registry_with(handler: ScriptedHandler) -> Registry {
    Registry::new().register(handler)
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_pending_job_is_claimed_run_and_marked_succeeded() {
    let db = db().await;
    let kind = format!("test_succeed_{}", unique_suffix());
    let seeded = seed_job(&db, &kind, 3).await;

    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(ScriptedHandler {
        kind_owned: kind,
        calls: calls.clone(),
        succeed: true,
    });

    let ctx = worker_context(&db);
    jobs::tick(&ctx, &sys(), "worker-1", &registry)
        .await
        .expect("tick succeeds");

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let after = reload_job(&db, &seeded.id).await;
    assert_eq!(after.state, schema::JobState::succeeded);
    assert!(after.finishedAt.is_some(), "trigger must stamp finishedAt");
    assert_eq!(after.attempts, 1);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_handler_error_backs_off_the_job_to_pending_with_a_future_run_at() {
    let db = db().await;
    let kind = format!("test_fail_{}", unique_suffix());
    let seeded = seed_job(&db, &kind, 3).await;

    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(ScriptedHandler {
        kind_owned: kind,
        calls: calls.clone(),
        succeed: false,
    });

    let ctx = worker_context(&db);
    jobs::tick(&ctx, &sys(), "worker-1", &registry)
        .await
        .expect("tick succeeds");

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let after = reload_job(&db, &seeded.id).await;
    assert_eq!(after.state, schema::JobState::pending);
    assert_eq!(after.attempts, 1);
    assert_eq!(after.lastError, Some("scripted failure".to_owned()));
    assert!(
        after.runAt > Utc::now(),
        "a backed-off job must not be immediately due again"
    );
    assert!(
        after.leaseOwner.is_none(),
        "a backed-off job holds no lease"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn exhausting_max_attempts_moves_a_failed_job_to_dead() {
    let db = db().await;
    let kind = format!("test_fail_once_{}", unique_suffix());
    let seeded = seed_job(&db, &kind, 1).await;

    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(ScriptedHandler {
        kind_owned: kind,
        calls: calls.clone(),
        succeed: false,
    });

    let ctx = worker_context(&db);
    jobs::tick(&ctx, &sys(), "worker-1", &registry)
        .await
        .expect("tick succeeds");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "exactly one attempt, no retry"
    );
    let after = reload_job(&db, &seeded.id).await;
    assert_eq!(after.state, schema::JobState::dead);
    assert_eq!(after.attempts, 1);
    assert!(after.finishedAt.is_some());
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_crashed_jobs_lease_reclaims_to_pending_and_only_actually_runs_on_the_next_claim() {
    let db = db().await;
    let kind = format!("test_reclaim_{}", unique_suffix());
    let seeded = seed_job(&db, &kind, 3).await;

    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with(ScriptedHandler {
        kind_owned: kind,
        calls: calls.clone(),
        succeed: true,
    });
    let ctx = worker_context(&db);

    // Claim it for real (pending -> running) but crash before it ever
    // runs — force the lease into the past exactly like
    // dispatch_live_postgres.rs's own `exhausting_max_attempts` test does
    // for Message.leaseUntil, which (unlike updatedAt) carries no touch
    // trigger forcing it back to now.
    let claimed = sms_worker::claim::claim_batch::<Job>(&db, &sys(), "worker-crashed", 10)
        .await
        .expect("claim_batch succeeds")
        .into_iter()
        .find(|j| j.id == seeded.id)
        .expect("the seeded job was claimed");
    assert_eq!(claimed.state, schema::JobState::running);

    db.job()
        .update(claimed.id.clone())
        .set(UpdateJobInput {
            leaseUntil: Some(Some(Utc::now() - Duration::seconds(1))),
            ..Default::default()
        })
        .if_match(claimed.version)
        .run(&sys())
        .await
        .expect("forcing the lease into the past to simulate a crashed worker");

    // First tick after the crash: reclaims running -> pending. Per
    // Claimable for Job's own doc, this must NOT execute the handler —
    // only the *next* claim does.
    jobs::tick(&ctx, &sys(), "worker-2", &registry)
        .await
        .expect("tick succeeds");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a reclaim tick must not run the handler — it only requeues"
    );
    let after_reclaim = reload_job(&db, &seeded.id).await;
    assert_eq!(after_reclaim.state, schema::JobState::pending);

    // Second tick: claims it for real (pending -> running) and runs it.
    jobs::tick(&ctx, &sys(), "worker-2", &registry)
        .await
        .expect("tick succeeds");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let after_run = reload_job(&db, &seeded.id).await;
    assert_eq!(after_run.state, schema::JobState::succeeded);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn scheduler_tick_does_not_double_enqueue_within_the_cadence_window() {
    let db = db().await;
    let kind: &'static str =
        Box::leak(format!("test_cadence_{}", unique_suffix()).into_boxed_str());
    let specs = vec![RecurringJobSpec {
        kind,
        cadence: Duration::hours(1),
        priority: 500,
        max_attempts: 3,
    }];
    let mut last_enqueued = HashMap::new();

    scheduler::tick(&db, &sys(), &specs, &mut last_enqueued).await;
    scheduler::tick(&db, &sys(), &specs, &mut last_enqueued).await;

    let rows = db
        .job()
        .find_many()
        .where_expr(FilterExpr::from(job::kind().eq(kind.to_owned())))
        .run(&owner())
        .await
        .expect("listing jobs of this kind");
    assert_eq!(
        rows.len(),
        1,
        "two ticks inside the cadence window must enqueue exactly once"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn scheduler_tick_enqueues_again_once_the_cadence_elapses() {
    let db = db().await;
    let kind: &'static str =
        Box::leak(format!("test_cadence_elapsed_{}", unique_suffix()).into_boxed_str());
    let specs = vec![RecurringJobSpec {
        kind,
        cadence: Duration::seconds(1),
        priority: 500,
        max_attempts: 3,
    }];
    let mut last_enqueued = HashMap::new();

    scheduler::tick(&db, &sys(), &specs, &mut last_enqueued).await;
    let first = db
        .job()
        .find_many()
        .where_expr(FilterExpr::from(job::kind().eq(kind.to_owned())))
        .run(&owner())
        .await
        .expect("listing jobs of this kind");
    assert_eq!(first.len(), 1);

    // The first instance must be terminal before a second can be
    // enqueued without colliding with `jobs_dedupe_idx` (which excludes
    // pending/running/failed, not just one state) — the point of this
    // test is cadence re-firing, not the dedupe-collision path, so clear
    // that precondition explicitly rather than tangle the two.
    // `pending -> succeeded` isn't a legal edge (only `running ->
    // succeeded` is) — claim it for real first, same as every other
    // writer of `Job`.
    let running = sms_worker::claim::claim_batch::<Job>(&db, &sys(), "worker-1", 10)
        .await
        .expect("claim_batch succeeds")
        .into_iter()
        .find(|j| j.id == first[0].id)
        .expect("the first instance was claimed");
    db.job()
        .update(running.id.clone())
        .set(UpdateJobInput {
            state: Some(schema::JobState::succeeded),
            ..Default::default()
        })
        .if_match(running.version)
        .run(&sys())
        .await
        .expect("marking the first instance terminal");

    // Simulate the cadence having elapsed by backdating this role's own
    // in-memory bookkeeping — the real clock doesn't need to move for
    // this, only scheduler::tick's due-check does.
    last_enqueued.insert(kind, Some(Utc::now() - Duration::seconds(2)));
    scheduler::tick(&db, &sys(), &specs, &mut last_enqueued).await;

    let after = db
        .job()
        .find_many()
        .where_expr(FilterExpr::from(job::kind().eq(kind.to_owned())))
        .run(&owner())
        .await
        .expect("listing jobs of this kind");
    assert_eq!(
        after.len(),
        2,
        "once the cadence elapses and the prior instance is terminal, a new one enqueues"
    );
}

async fn seed_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "jobs test app".to_owned(),
            slug: format!("jobs-test-{}", unique_suffix()),
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

/// Walks a fresh message to `submitted`, same chain
/// `dlr_ingestion_live_postgres.rs` uses — `create` can only ever produce
/// `accepted`.
async fn seed_submitted_message(
    db: &Cratestack,
    app_id: &str,
    expires_at: chrono::DateTime<Utc>,
) -> schema::Message {
    let created = db
        .message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("jobs-test-{}", unique_suffix())),
            msisdn: "+237677123456".to_owned(),
            msisdnHash: format!("sha256:jobs-test-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 1000,
            body: Some("expire_stale test".to_owned()),
            bodyHash: "sha256:jobs-test".to_owned(),
            bodyLength: 18,
            encoding: Encoding::gsm7,
            segments: 1,
            stateReason: None,
            routeId: None,
            providerId: None,
            providerMessageRef: None,
            providerMessageRefAlt: None,
            maxAttempts: 3,
            leaseOwner: None,
            leaseUntil: None,
            scheduledAt: None,
            expiresAt: expires_at,
            submittedAt: None,
            finalizedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message");

    let queued = db
        .message()
        .update(created.id.clone())
        .set(schema::UpdateMessageInput {
            state: Some(MessageState::queued),
            ..Default::default()
        })
        .if_match(created.version)
        .run(&sys())
        .await
        .expect("accepted -> queued");
    let routed = db
        .message()
        .update(queued.id.clone())
        .set(schema::UpdateMessageInput {
            state: Some(MessageState::routed),
            ..Default::default()
        })
        .if_match(queued.version)
        .run(&sys())
        .await
        .expect("queued -> routed");
    db.message()
        .update(routed.id.clone())
        .set(schema::UpdateMessageInput {
            state: Some(MessageState::submitted),
            ..Default::default()
        })
        .if_match(routed.version)
        .run(&sys())
        .await
        .expect("routed -> submitted")
}

async fn reload_message(db: &Cratestack, id: &str) -> schema::Message {
    db.message()
        .find_many()
        .where_expr(FilterExpr::from(message::id().eq(id.to_owned())))
        .limit(1)
        .run(&sys())
        .await
        .expect("reloading the message")
        .into_iter()
        .next()
        .expect("the message still exists")
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn expire_stale_expires_a_submitted_message_past_its_validity_window() {
    let db = db().await;
    let app_id = seed_app(&db).await;
    let seeded = seed_submitted_message(&db, &app_id, Utc::now() - Duration::minutes(1)).await;
    assert_eq!(seeded.state, MessageState::submitted);

    ExpireStale
        .run_at(&db, &sys(), Utc::now())
        .await
        .expect("expire_stale succeeds");

    let after = reload_message(&db, &seeded.id).await;
    assert_eq!(after.state, MessageState::expired);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn expire_stale_expires_an_uncertain_message_past_its_six_hour_grace() {
    let db = db().await;
    let app_id = seed_app(&db).await;
    let submitted = seed_submitted_message(&db, &app_id, Utc::now() + Duration::hours(1)).await;

    let uncertain = db
        .message()
        .update(submitted.id.clone())
        .set(schema::UpdateMessageInput {
            state: Some(MessageState::uncertain),
            ..Default::default()
        })
        .if_match(submitted.version)
        .run(&sys())
        .await
        .expect("submitted -> uncertain");

    // touch_updated_at forces Message.updatedAt to clock_timestamp() on
    // every write — it can't be backdated through any CrateStack delegate.
    // Instead of waiting 6 real hours, evaluate expire_stale against a
    // `now` far enough past the message's real (just-now) updatedAt that
    // the 6-hour grace has "elapsed" relative to that virtual clock — see
    // ExpireStale::run_at's own doc for why this is the intended seam.
    ExpireStale
        .run_at(&db, &sys(), Utc::now() + Duration::hours(7))
        .await
        .expect("expire_stale succeeds");

    let after = reload_message(&db, &uncertain.id).await;
    assert_eq!(after.state, MessageState::expired);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn expire_stale_leaves_a_fresh_uncertain_message_alone() {
    let db = db().await;
    let app_id = seed_app(&db).await;
    let submitted = seed_submitted_message(&db, &app_id, Utc::now() + Duration::hours(1)).await;
    let uncertain = db
        .message()
        .update(submitted.id.clone())
        .set(schema::UpdateMessageInput {
            state: Some(MessageState::uncertain),
            ..Default::default()
        })
        .if_match(submitted.version)
        .run(&sys())
        .await
        .expect("submitted -> uncertain");

    // Real "now" — the message just turned uncertain, nowhere near its
    // 6-hour grace.
    ExpireStale
        .run_at(&db, &sys(), Utc::now())
        .await
        .expect("expire_stale succeeds");

    let after = reload_message(&db, &uncertain.id).await;
    assert_eq!(
        after.state,
        MessageState::uncertain,
        "a fresh uncertain message must not be touched before its 6h grace elapses"
    );
}
