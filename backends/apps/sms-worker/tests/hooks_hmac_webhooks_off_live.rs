//! `auth.methods` without `hmac_webhooks` (VSA-1), through the real binary.
//!
//! `sms-worker`'s `hooks` role claims no `hmac` attempt when its
//! `WorkerContext::hmac_webhooks` says the method is off, and
//! `backends/crates/sms-worker/tests/hooks_live_postgres.rs` proves that for a
//! context built by hand. This proves the other half, which no in-process test
//! can: that the spawned `sms-worker` reads `auth.methods` from its YAML
//! configuration and hands the answer to the role. Real workers, one per
//! configuration, over the same shape of backlog: with the key `[keypair]` (and no
//! signing key, so the worker serves nothing, and says so at start) the `hmac`
//! attempt stays `pending`, uncounted, and the endpoint receives nothing; a worker
//! started afterwards on a configuration without the key delivers that very
//! attempt at its first claim.
//!
//! ```bash
//! cargo build -p sms-worker-bin
//! cargo test -p sms-worker-bin --test hooks_hmac_webhooks_off_live -- --ignored --nocapture
//! ```

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, AttemptState, Cratestack, CreateWebhookAttemptInput, CreateWebhookEndpointInput,
    webhook_attempt,
};
use sms_test_support::config::{self, TestConfig};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The per-binary mutex every live suite takes (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn principal(kind: PrincipalKind, role: &str) -> CratestackContext {
    Principal {
        sub: "hooks-hmac-off-test".to_owned(),
        kind,
        role: role.to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    cratestack::uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
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

/// An app, an `hmac` endpoint at `url`, and one pending attempt for it.
async fn seed(db: &Cratestack, url: &str) -> schema::WebhookAttempt {
    let owner = principal(PrincipalKind::User, "owner");
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "hooks hmac off test app".to_owned(),
            slug: format!("hooks-hmac-off-{}", unique_suffix()),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner)
        .await
        .expect("seeding the app");
    let endpoint = db
        .webhook_endpoint()
        .create(CreateWebhookEndpointInput {
            appId: app.id,
            url: url.to_owned(),
            eventTypes: " message.delivered ".to_owned(),
            deliveryMode: schema::WebhookDeliveryMode::hmac,
            audience: None,
            secret: Some("k".repeat(40)),
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: false,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner)
        .await
        .expect("seeding the hmac endpoint");
    db.webhook_attempt()
        .create(CreateWebhookAttemptInput {
            endpointId: endpoint.id,
            sourceEventId: cratestack::uuid::Uuid::new_v4(),
            aggregateId: format!("cmsg{}", unique_suffix()),
            eventType: "message.delivered".to_owned(),
            payload: "{}".to_owned(),
            leaseOwner: None,
            leaseUntil: None,
            nextAttemptAt: Some(Utc::now()),
            lastStatusCode: None,
            lastError: None,
            lastAttemptAt: None,
            deliveredAt: None,
        })
        .run(&principal(PrincipalKind::App, "system"))
        .await
        .expect("seeding the attempt")
}

async fn reread(db: &Cratestack, id: &str) -> schema::WebhookAttempt {
    db.webhook_attempt()
        .find_many()
        .where_expr(FilterExpr::from(webhook_attempt::id().eq(id.to_owned())))
        .limit(1)
        .run(&principal(PrincipalKind::App, "system"))
        .await
        .expect("re-reading the attempt")
        .into_iter()
        .next()
        .expect("the attempt exists")
}

/// How many `hmac` endpoints the database holds: what the worker must say it
/// counted. Not a constant, because the database outlives a test run.
async fn hmac_endpoints(db: &Cratestack) -> i64 {
    sms_api::hmac_gate::count_hmac_endpoints(db, &principal(PrincipalKind::App, "system"))
        .await
        .expect("counting the hmac endpoints")
}

/// Kills the child if the test panics; owns its configuration directory so that
/// is removed only after the child is reaped.
struct KillOnDrop(
    Child,
    #[expect(
        dead_code,
        reason = "held for its Drop, which removes the configuration directory"
    )]
    TestConfig,
);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl KillOnDrop {
    /// Kills the worker and returns what it logged (to stdout, where `tracing` writes).
    fn kill_and_read_log(mut self) -> String {
        self.0.kill().expect("killing the worker");
        let mut log = String::new();
        if let Some(mut stdout) = self.0.stdout.take() {
            std::io::Read::read_to_string(&mut stdout, &mut log).expect("reading the worker's log");
        }
        self.0.wait().expect("reaping the worker");
        log
    }
}

/// A real `sms-worker --roles hooks` with `auth` appended to its overlay.
/// `--metrics-listen 127.0.0.1:0`: see `hooks_two_workers_live.rs` for why.
fn spawn_hooks_worker(database_url: &str, tag: &str, auth: &str) -> KillOnDrop {
    let config = TestConfig::write(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        &format!("hooks-hmac-off-{tag}"),
        &format!("{}{auth}", config::no_providers_overlay()),
    );
    let child = Command::new(env!("CARGO_BIN_EXE_sms-worker"))
        .args(["--roles", "hooks"])
        .args(["--database-url", database_url])
        .args(config.args())
        .env_remove(config::PEPPER_VAR)
        .args(["--worker-id", &format!("hooks-hmac-off-{tag}")])
        .args(["--metrics-listen", "127.0.0.1:0"])
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawning the real sms-worker binary");
    KillOnDrop(child, config)
}

/// Waits until the attempt leaves `pending`/`delivering`.
async fn settled(db: &Cratestack, id: &str) -> schema::WebhookAttempt {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let attempt = reread(db, id).await;
        if !matches!(
            attempt.state,
            AttemptState::pending | AttemptState::delivering
        ) {
            return attempt;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the worker did not settle the attempt in 30s: {attempt:#?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// How long a worker polls before the held-back assertions are made: the poll is
/// one second, so this is several claims. The worker's first poll is immediate, and
/// the boot line it logs first is read after it is killed.
const POLLS: Duration = Duration::from_secs(4);

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres and the built sms-worker binary — see module \
            docs"]
async fn a_real_worker_holds_an_hmac_attempt_back_when_the_yaml_says_keypair_only() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let database_url = sms_test_support::database_url().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let attempt = seed(&db, &format!("{}/hook", server.uri())).await;
    let count = hmac_endpoints(&db).await;

    let worker = spawn_hooks_worker(&database_url, "off", "auth:\n  methods: [keypair]\n");
    tokio::time::sleep(POLLS).await;
    let held = reread(&db, &attempt.id).await;
    let log = worker.kill_and_read_log();

    assert_eq!(held.state, AttemptState::pending, "{held:#?}");
    assert_eq!(held.attempts, 0, "never claimed, so never counted");
    assert_eq!(held.version, attempt.version, "the row is not written");
    assert!(held.lastError.is_none() && held.leaseOwner.is_none());
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "nothing may be sent to an hmac endpoint"
    );
    // The boot lines: how many hmac endpoints exist and that they wait, and that
    // this worker (no signing key, no hmac) serves nothing at all.
    assert!(
        log.contains(&format!("hmac_endpoints={count}")) && log.contains("hmac_webhooks=false"),
        "the hooks worker did not log its hmac endpoint count at boot: {log}"
    );
    assert!(log.contains("stay pending, uncounted"), "{log}");
    assert!(log.contains("serves no webhook delivery"), "{log}");

    // The method is permitted again (the key deleted, a restart): the first claim
    // of the next worker delivers the attempt that waited, with no replay.
    let restarted = spawn_hooks_worker(&database_url, "back", "");
    let delivered = settled(&db, &attempt.id).await;
    restarted.kill_and_read_log();
    assert_eq!(delivered.state, AttemptState::succeeded, "{delivered:#?}");
    assert_eq!(delivered.attempts, 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// The control: the same shape of backlog, the key absent. Without it the test
/// above would pass for a worker that delivered nothing at all.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres and the built sms-worker binary — see module \
            docs"]
async fn a_real_worker_delivers_the_same_attempt_when_auth_methods_is_absent() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let database_url = sms_test_support::database_url().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let attempt = seed(&db, &format!("{}/hook", server.uri())).await;
    let count = hmac_endpoints(&db).await;

    let worker = spawn_hooks_worker(&database_url, "on", "");
    let delivered = settled(&db, &attempt.id).await;
    let log = worker.kill_and_read_log();

    assert_eq!(delivered.state, AttemptState::succeeded, "{delivered:#?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(
        log.contains(&format!("hmac_endpoints={count}")) && log.contains("hmac_webhooks=true"),
        "the hooks worker did not log its hmac endpoint count at boot: {log}"
    );
}
