//! The automatable half of [#36](https://github.com/vymalo/vsms/issues/36):
//! `kill -9` a *real* `sms-worker` process mid-submit and prove the message
//! is neither lost nor stuck — a second, independent process reclaims and
//! resubmits it. `docs/runbooks/36-handset-gate.md` covers the other half
//! (a real Orange handset), which stays permanently manual — no amount of
//! mocking proves a real SMS arrived. But "crash mid-submit, reclaim
//! without loss" needs no real Orange account, and #36's own text is the
//! reason to automate it: "lease reclamation is the kind of code that is
//! easy to write and easy to never actually exercise." Before this file,
//! the only evidence it worked was one undocumented manual dry run
//! (mentioned in the runbook) — this makes it a permanent, rerunnable
//! regression test instead of institutional memory.
//!
//! Unlike every other `*_live_postgres.rs` suite in this workspace, this one
//! spawns the actual compiled `sms-worker` binary as a subprocess and sends
//! it real `SIGKILL` (`Child::kill()` on Unix), rather than simulating a
//! crash in-process (`claim_live_postgres.rs` forces `leaseUntil` into the
//! past by hand). Only a real OS-level kill proves what an in-process
//! simulation can't: that a process torn down mid-`await`, with no chance
//! to run any Rust destructor, still leaves a reclaimable row — no
//! `Drop` impl anywhere gets to run, matching a real `kill -9` or an OOM
//! killer exactly, and not matching a graceful `SIGTERM` shutdown at all.
//!
//! This lives in `app/sms-worker` rather than `crates/sms-worker` because
//! `CARGO_BIN_EXE_sms-worker` is only injected by Cargo for integration
//! tests in the same package that defines the `[[bin]]` — see
//! `app/sms-worker/Cargo.toml`.
//!
//! Ignored by default, same convention as this workspace's other live
//! suites, but doubly so here: it also needs the real `sms-worker` binary
//! built first. Run explicitly:
//!
//! ```bash
//! docker run --rm -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:16
//! createdb vsms_check
//! DATABASE_URL=postgres://postgres:postgres@localhost/vsms_check ./ci/apply-migrations.sh
//! cargo build -p sms-worker-bin
//! DATABASE_URL=postgres://postgres:postgres@localhost/vsms_check \
//!     cargo test -p sms-worker-bin --test kill9_reclaim_live -- --ignored --nocapture
//! ```

use chrono::{Duration as ChronoDuration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::CoolContext;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, Encoding, Message, MessageClass, MessageState, OperatorCode,
};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The window the mock submit endpoint holds its *first* response open for.
/// Wide enough to comfortably fit a real OS process spawn plus this test's
/// own DB-polling latency between "worker wrote `routed`" and "this test
/// observes it and sends `SIGKILL`" — a real subprocess has genuine
/// scheduling jitter an in-process `tokio::spawn` doesn't. Every later
/// response is instant (see `submit_responder`) so the second, successful
/// submit doesn't also pay this cost.
const FIRST_SUBMIT_DELAY: Duration = Duration::from_secs(5);
const SENDER_NUMBER: &str = "+2370000";

fn sys() -> CoolContext {
    Principal {
        sub: "sms-worker-kill9-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "sms-worker-kill9-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .subsec_nanos();
    format!("{:06x}", (u64::from(nanos).wrapping_add(n)) % 0x0100_0000)
}

async fn db() -> Cratestack {
    let url = database_url();
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must point at a fully migrated database — see module docs")
}

async fn seed_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "kill9 test app".to_owned(),
            slug: format!("kill9-test-{}", unique_suffix()),
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

async fn seed_active_provider(db: &Cratestack) -> String {
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!("kill9_test_{}", unique_suffix())
                .chars()
                .take(32)
                .collect(),
            displayName: "kill9 test provider".to_owned(),
            kind: schema::ProviderKind::orange_cm_http,
            config: "{}".to_owned(),
            credentialRef: "vault://test".to_owned(),
            maxTps: 5.0,
            maxDailySubmissions: 1000,
            supportsDlr: true,
            supportsAlphaSender: true,
            supportsUcs2: true,
            supportsConcat: true,
            costPerSegmentXaf: "15".parse().unwrap(),
            healthCheckedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding a provider");

    db.provider()
        .update(provider.id.clone())
        .set(schema::UpdateProviderInput {
            state: Some(schema::ProviderState::active),
            ..Default::default()
        })
        .run(&owner())
        .await
        .expect("activating the provider");

    provider.id
}

async fn seed_message(db: &Cratestack, app_id: &str) -> Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("kill9-test-{}", unique_suffix())),
            msisdn: "+237677123456".to_owned(),
            msisdnHash: format!("sha256:kill9-test-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            // Max priority — this database is never reset between runs
            // (same reasoning as every other live suite's identical
            // comment), so a lower priority risks sorting behind whatever
            // earlier runs left claimable.
            priority: 1000,
            body: Some("kill9 reclaim test".to_owned()),
            bodyHash: "sha256:kill9-test".to_owned(),
            bodyLength: 19,
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
            expiresAt: Utc::now() + ChronoDuration::hours(1),
            submittedAt: None,
            finalizedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message")
}

async fn reload(db: &Cratestack, id: &str) -> Message {
    db.message()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::message::id().eq(id.to_owned()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("reloading the message")
        .into_iter()
        .next()
        .expect("the message still exists")
}

async fn wait_for_state(
    db: &Cratestack,
    id: &str,
    want: MessageState,
    timeout: Duration,
) -> Message {
    let deadline = Instant::now() + timeout;
    loop {
        let message = reload(db, id).await;
        if message.state == want {
            return message;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {timeout:?} waiting for state {want:?}; last seen state was \
             {:?}",
            message.state
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Mounts a submit responder that delays only its first response
/// (`FIRST_SUBMIT_DELAY`, the kill window) and stamps every response with a
/// distinct `resourceURL` — the second, post-reclaim submit needs a
/// visibly different `providerMessageRef` to prove it's a fresh outbound
/// call, not a resumed one (`claim.rs`'s own documented reclaim semantics).
async fn mount_orange(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/oauth/v3/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "kill9-test-token",
            "expires_in": 3600,
        })))
        .mount(server)
        .await;

    let calls = Arc::new(AtomicU64::new(0));
    Mock::given(method("POST"))
        .and(path(format!(
            "/smsmessaging/v1/outbound/tel:{SENDER_NUMBER}/requests"
        )))
        .respond_with(move |_req: &wiremock::Request| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            let delay = if n == 0 {
                FIRST_SUBMIT_DELAY
            } else {
                Duration::ZERO
            };
            ResponseTemplate::new(201)
                .set_delay(delay)
                .set_body_json(serde_json::json!({
                    "outboundSMSMessageRequest": {
                        "resourceReference": {"resourceURL": format!("https://x/res-kill9-{n}")}
                    }
                }))
        })
        .mount(server)
        .await;
}

/// Kills the wrapped child if the test panics before an explicit `kill()`
/// call — a failed assertion mid-test shouldn't also leave an orphaned
/// `sms-worker` process polling a scratch database forever.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_worker(database_url: &str, orange_base_url: &str, worker_id: &str) -> KillOnDrop {
    let bin = env!("CARGO_BIN_EXE_sms-worker");
    let child = Command::new(bin)
        .args(["--roles", "dispatch"])
        .args(["--database-url", database_url])
        // Never a real Orange account — the responder above never checks
        // these, only that dispatch's own startup requires all three set
        // together (see main.rs's `orange_provider`).
        .args(["--orange-client-id", "kill9-test-client"])
        .args(["--orange-client-secret", "kill9-test-secret"])
        .args(["--orange-sender-number", SENDER_NUMBER])
        .args(["--orange-base-url", orange_base_url])
        .args(["--worker-id", worker_id])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawning the real sms-worker binary");
    KillOnDrop(child)
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres and the built sms-worker binary — see module \
            docs"]
async fn kill_9_mid_submit_reclaims_and_resubmits_without_losing_the_message() {
    let database_url = database_url();
    let db = db().await;
    let server = MockServer::start().await;
    mount_orange(&server).await;

    seed_active_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id).await;

    let mut worker1 = spawn_worker(&database_url, &server.uri(), "kill9-test-worker-1");

    // `routed` is written by `claim_batch` *before* `submit_one` makes the
    // HTTP call (dispatch.rs's `tick`) — the moment this test observes it,
    // the real submit is either about to start or already in flight inside
    // `FIRST_SUBMIT_DELAY`'s window.
    let routed = wait_for_state(
        &db,
        &seeded.id,
        MessageState::routed,
        Duration::from_secs(10),
    )
    .await;
    assert!(
        routed.providerMessageRef.is_none(),
        "the claim must land before the provider ever responds"
    );
    assert!(
        routed.leaseUntil.is_some_and(|until| until > Utc::now()),
        "a freshly routed row must carry a live lease"
    );

    // Give the HTTP request time to actually leave the process before
    // killing it — landing the kill on the exact instant the DB write
    // commits risks the request never being sent at all, which would prove
    // nothing about mid-submit crashes.
    tokio::time::sleep(Duration::from_millis(500)).await;
    worker1.0.kill().expect("SIGKILL-ing the worker mid-submit");
    worker1.0.wait().expect("reaping the killed process");

    // The crash must leave exactly the state the runbook documents:
    // `routed`, no provider ref yet, lease still (nominally) live.
    let crashed = reload(&db, &seeded.id).await;
    assert_eq!(crashed.state, MessageState::routed);
    assert!(crashed.providerMessageRef.is_none());
    assert_eq!(
        crashed.attempts, 1,
        "the crash happened mid first attempt, not a fresh one"
    );

    // Force the two-minute dispatch lease into the past — the same
    // technique every other live suite in this workspace uses to avoid a
    // real sleep, standing in for either waiting out the lease or a
    // reaper role noticing it (neither of which this test needs to prove).
    db.message()
        .update(crashed.id.clone())
        .set(schema::UpdateMessageInput {
            leaseUntil: Some(Some(Utc::now() - ChronoDuration::seconds(1))),
            ..Default::default()
        })
        .if_match(crashed.version)
        .run(&sys())
        .await
        .expect("forcing the lease into the past");

    // A second, independent process — a real restart or a standby node
    // taking over, not the same one resuming (the runbook's own worked
    // example: "Restart sms-worker (same command as before — a fresh
    // process...)").
    let mut worker2 = spawn_worker(&database_url, &server.uri(), "kill9-test-worker-2");

    let submitted = wait_for_state(
        &db,
        &seeded.id,
        MessageState::submitted,
        Duration::from_secs(15),
    )
    .await;
    worker2.0.kill().ok();
    let _ = worker2.0.wait();

    assert_eq!(
        submitted.attempts, 1,
        "reclaiming a routed row resumes the same logical attempt (claim.rs's own reclaim \
         branch), it does not start a new one"
    );
    assert!(submitted.providerMessageRef.is_some());

    let requests = server
        .received_requests()
        .await
        .expect("wiremock records requests by default");
    let submit_calls = requests
        .iter()
        .filter(|r| r.url.path().starts_with("/smsmessaging/v1/outbound/"))
        .count();
    // The gate's own point, not a bug this test should hide: a crash
    // between "Orange accepts the request" and "we write the outcome" can
    // and does produce two real outbound submissions — nothing today gives
    // Orange a dedup key, and `providerMessageRef` has no uniqueness
    // constraint at the DB level either (see docs/runbooks/36-handset-gate.md
    // and this repo's tracked follow-up for real provider-side or
    // application-level submit idempotency). If this assertion ever
    // becomes 1, that's this gap having been closed, not a regression.
    assert_eq!(
        submit_calls, 2,
        "a crash mid-submit is expected to cause exactly one duplicate outbound submission on \
         reclaim, matching the runbook's own documented, currently-unmitigated finding"
    );
}
