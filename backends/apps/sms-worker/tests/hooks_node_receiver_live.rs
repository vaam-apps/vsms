//! `#44`'s first gate assertion: "a sample Node receiver opens the delivery
//! with an independent implementation" — proved against the *live delivery
//! path*, not a fixture.
//!
//! `backends/crates/sms-webhook/tests/sealed_vector.rs` already proves the
//! sealer's bytes open in the SDKs (`sdks/rust/vsms-sdk-rust/tests/webhook.rs`,
//! `sdks/node/vsms-sdk-node/test/webhook.test.ts`), and
//! `sms-webhook/tests/sealed_delivery.rs` that a real cratestack
//! `EnvelopeLayer` accepts them. That is not sufficient for this gate: it
//! proves the seal, not that what the real `hooks` role actually sends is that
//! seal, over a real socket, to a receiver that never saw this crate's code.
//! The interesting failure this test can catch and a fixture test structurally
//! cannot: a mismatch between what the worker seals and what is on the wire
//! (a body re-serialised after sealing, a route the endpoint URL does not
//! yield, an audience the row does not carry, a signing key the worker loaded
//! differently from the one the receiver pinned).
//!
//! Both delivery modes (VS-8R) run through the same receiver code, which is
//! configured for one: `sealed` (the SDK's `WebhookOpener`) or `hmac` (the
//! original signature, `src/signature.ts`, written in a language that shares
//! nothing with `sms-webhook`'s Rust). The `hmac` tests are
//! `a_real_hmac_delivery_...`, `an_hmac_delivery_in_a_rotation_window_...` and
//! `an_hmac_delivery_signed_with_another_secret_...`.
//!
//! # What's real here, and what isn't
//!
//! Real: a genuinely separate `sms-worker --roles hooks` OS process
//! (`CARGO_BIN_EXE_sms-worker`) holding its signing key from a file, a
//! genuinely separate `node src/gate-receiver.ts` OS process pinning only the
//! *public* keys, a real loopback TCP connection between them, a real
//! Postgres-backed `WebhookAttempt` row claimed through the real CAS claim loop
//! (`backends/crates/sms-worker/src/claim.rs`), and the actual
//! `examples/node/webhook-receiver` code, opening the delivery with
//! `@vaam-apps/vsms-node`'s `WebhookOpener` (cratestack's envelope compiled to
//! WebAssembly). Not real: the `WebhookAttempt` row itself is seeded directly
//! (the same convention `hooks_live_postgres.rs` uses) rather than produced by
//! a `Message` transition; that path is covered by
//! `backends/crates/sms-api/tests/webhooks_live_postgres.rs`.
//!
//! # Prerequisites this test cannot check for you
//!
//! - Docker (`sms_test_support` provisions and migrates Postgres).
//! - `cargo build -p sms-worker-bin` — `CARGO_BIN_EXE_sms-worker` points at
//!   whatever the last build produced; a stale or missing binary is a
//!   confusing failure otherwise (same caveat `kill9_reclaim_live.rs`
//!   documents).
//! - The Node SDK built (`pnpm --filter @vaam-apps/vsms-node run build`, which
//!   needs `just wasm-tools`) and `pnpm install --ignore-workspace
//!   --frozen-lockfile` inside `examples/node/webhook-receiver`, which copies
//!   the built SDK in.
//! - Node ≥23.6 on `PATH` as `node` (this repo's own `.nvmrc` pins 26, which
//!   qualifies) — native TS support unflagged, per the receiver's own
//!   `package.json` `engines` field.
//!
//! ```bash
//! cargo build -p sms-worker-bin
//! pnpm --filter @vaam-apps/vsms-node run build
//! (cd examples/node/webhook-receiver && pnpm install --ignore-workspace --frozen-lockfile)
//! cargo test -p sms-worker-bin --test hooks_node_receiver_live -- --ignored --nocapture
//! ```
//!
//! # The deliberate-break evidence, kept as a test
//!
//! The second test seeds the endpoint with an audience the receiver was not
//! configured with. The receiver's independent `WebhookOpener` refuses the
//! genuine delivery (`401`, logged `rejected-unverified` on the Node side),
//! `hooks.rs` treats that as retryable and backs off rather than reaching
//! `succeeded`, and the test asserts both observations. It is the one inversion
//! available without touching production sealing or opening code: the two
//! implementations either agree about the audience or they don't, and this is
//! what "don't" looks like end to end — permanently, not a transcript.

use sms_test_support::config::{self, TestConfig};
use std::io::{BufRead, BufReader};
use std::net::TcpListener as StdTcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use chrono::Utc;
use cratestack::CratestackContext;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack, CreateWebhookAttemptInput, CreateWebhookEndpointInput};

fn sys() -> CratestackContext {
    Principal {
        sub: "hooks-node-receiver-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CratestackContext {
    Principal {
        sub: "hooks-node-receiver-test-owner".to_owned(),
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

/// Same reasoning as every other live suite's copy of this mutex: the spawned
/// worker's claim loop is deliberately global (it must see every endpoint's
/// rows), so two tests that each start a real worker would steal each other's
/// attempts, and both seed through the same trigger-heavy tables at once.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

async fn db() -> Cratestack {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

/// Where the receiver on `port` will answer. The endpoint row is seeded before
/// the receiver starts, because the receiver is configured with the row's id.
fn receiver_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/webhooks/vsms")
}

fn free_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port");
    listener
        .local_addr()
        .expect("reading the bound address")
        .port()
}

async fn seed_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "hooks node receiver gate test app".to_owned(),
            slug: format!("hooks-node-receiver-{}", unique_suffix()),
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

/// `audience` is a parameter, not derived — the deliberate-break test seeds
/// one the receiver was not configured with, which needs a call site that can
/// do exactly that.
async fn seed_endpoint(
    db: &Cratestack,
    app_id: &str,
    url: &str,
    audience: &str,
) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: url.to_owned(),
            deliveryMode: schema::WebhookDeliveryMode::sealed,
            audience: Some(audience.to_owned()),
            secret: None,
            prevSecret: None,
            secretRotatedAt: None,
            eventTypes: " message.delivered ".to_owned(),
            maskRecipient: true,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the webhook endpoint")
}

/// An `hmac` endpoint (VS-8R) pointed at the same receiver.
async fn seed_hmac_endpoint(
    db: &Cratestack,
    app_id: &str,
    url: &str,
    secret: &str,
    prev_secret: Option<&str>,
) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: url.to_owned(),
            deliveryMode: schema::WebhookDeliveryMode::hmac,
            audience: None,
            secret: Some(secret.to_owned()),
            prevSecret: prev_secret.map(str::to_owned),
            secretRotatedAt: prev_secret.map(|_| Utc::now()),
            eventTypes: " message.delivered ".to_owned(),
            maskRecipient: true,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the hmac webhook endpoint")
}

/// The exact shape #38's own subscriber produces (`webhooks.rs::message_payload`)
/// — a masked `to`, matching this endpoint's own `maskRecipient: true` above.
/// Seeded directly rather than produced via a real `Message` transition; see
/// this module's own doc for why.
fn seed_payload(message_id: &str) -> String {
    serde_json::json!({
        "messageId": message_id,
        "appId": "capp00000000000000000",
        "clientRef": "hooks-node-receiver-gate",
        "to": "+2376****89",
        "state": "delivered",
        "operator": "orange",
        "segments": 1,
        "costXaf": "22.00",
    })
    .to_string()
}

async fn seed_attempt(
    db: &Cratestack,
    endpoint_id: &str,
    aggregate_id: &str,
) -> schema::WebhookAttempt {
    db.webhook_attempt()
        .create(CreateWebhookAttemptInput {
            endpointId: endpoint_id.to_owned(),
            sourceEventId: cratestack::uuid::Uuid::new_v4(),
            aggregateId: aggregate_id.to_owned(),
            eventType: "message.delivered".to_owned(),
            payload: seed_payload(aggregate_id),
            leaseOwner: None,
            leaseUntil: None,
            nextAttemptAt: Some(Utc::now()),
            lastStatusCode: None,
            lastError: None,
            lastAttemptAt: None,
            deliveredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the webhook attempt")
}

/// vsms's two envelope signing seeds for this run: `CURRENT` signs deliveries,
/// `NEXT` is only pinned. The receiver is given their *public* halves.
const CURRENT_SEED: [u8; 32] = [0x71; 32];
const NEXT_SEED: [u8; 32] = [0x72; 32];

fn public_key_b64u(seed: &[u8; 32]) -> String {
    use base64::Engine as _;
    let key = cratestack::cose::Ed25519Signer::from_seed(seed)
        .verify_key()
        .ed25519_bytes()
        .expect("an Ed25519 key");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key)
}

/// The PEM file the worker loads (`--envelope-signing-key-file`).
fn write_signing_key() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("hooks-node-receiver-key-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("creating the key directory");
    let path = dir.join("envelope-current.pem");
    std::fs::write(&path, sms_api::envelope::pem_of_seed(&CURRENT_SEED)).expect("writing the key");
    path
}

/// A real, spawned `node src/gate-receiver.ts` process — the actual
/// `examples/node/webhook-receiver` code, not a stand-in.
struct NodeReceiver {
    child: Child,
    base_url: String,
    /// What the receiver was configured as, for a failure message.
    configured: String,
}

impl NodeReceiver {
    /// A `sealed` receiver. `app_id` and `endpoint_id` are the app and the
    /// endpoint the receiver believes it serves (`WEBHOOK_RECEIVER_APP_ID`,
    /// `WEBHOOK_RECEIVER_ENDPOINT_ID`): the receiver has no mode without them.
    async fn spawn(port: u16, audience: &str, app_id: &str, endpoint_id: &str) -> Self {
        let mut command = Self::command(port);
        command
            .env(
                "WEBHOOK_RECEIVER_SERVER_CURRENT",
                public_key_b64u(&CURRENT_SEED),
            )
            .env("WEBHOOK_RECEIVER_SERVER_NEXT", public_key_b64u(&NEXT_SEED))
            .env("WEBHOOK_RECEIVER_AUDIENCE", audience)
            .env("WEBHOOK_RECEIVER_APP_ID", app_id)
            .env("WEBHOOK_RECEIVER_ENDPOINT_ID", endpoint_id);
        Self::start(command, port, format!("sealed, audience {audience}")).await
    }

    /// An `hmac` receiver holding the endpoint's secret (and, inside a rotation
    /// window, its previous one).
    async fn spawn_hmac(port: u16, secret: &str, prev_secret: Option<&str>) -> Self {
        let mut command = Self::command(port);
        command
            .env("WEBHOOK_RECEIVER_MODE", "hmac")
            .env("WEBHOOK_RECEIVER_SECRET", secret);
        if let Some(prev) = prev_secret {
            command.env("WEBHOOK_RECEIVER_PREV_SECRET", prev);
        }
        Self::start(command, port, "hmac".to_owned()).await
    }

    fn command(port: u16) -> Command {
        let mut command = Command::new("node");
        command
            .current_dir(node_receiver_dir())
            .arg("src/gate-receiver.ts")
            .env("WEBHOOK_RECEIVER_PORT", port.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        command
    }

    async fn start(mut command: Command, port: u16, configured: String) -> Self {
        let package_dir = node_receiver_dir();

        let mut child = command.spawn().unwrap_or_else(|error| {
            panic!(
                "spawning `node src/gate-receiver.ts` in {} failed: {error} — is Node >=23.6 on \
                 PATH?",
                package_dir.display()
            )
        });

        // Echo the child's stdout to this test's own — `--nocapture` then
        // shows both processes interleaved, which is what makes a failing
        // run legible.
        if let Some(stdout) = child.stdout.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    println!("[node] {line}");
                }
            });
        }

        let base_url = format!("http://127.0.0.1:{port}");
        wait_until_ready(&base_url, &mut child).await;
        Self {
            child,
            base_url,
            configured,
        }
    }

    async fn results(&self) -> Vec<serde_json::Value> {
        let response = reqwest::get(format!("{}/__test__/results", self.base_url))
            .await
            .expect("GETting the gate-receiver's own /__test__/results");
        let body: serde_json::Value = response
            .json()
            .await
            .expect("parsing /__test__/results as JSON");
        body["results"].as_array().cloned().unwrap_or_default()
    }
}

impl Drop for NodeReceiver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn node_receiver_dir() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is backends/apps/sms-worker; the Node package lives at the
    // repo root's examples/node/webhook-receiver.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/node/webhook-receiver")
        .canonicalize()
        .expect(
            "examples/node/webhook-receiver must exist relative to backends/apps/sms-worker's own \
             manifest dir",
        )
}

async fn wait_until_ready(base_url: &str, child: &mut Child) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            panic!(
                "node src/gate-receiver.ts exited before becoming ready: {status:?} — check \
                 that the Node SDK is built and `pnpm install --ignore-workspace \
                 --frozen-lockfile` has been run in examples/node/webhook-receiver"
            );
        }
        if let Ok(response) = reqwest::get(format!("{base_url}/healthz")).await
            && response.status().is_success()
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "gate-receiver never became ready within 15s"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Kills the wrapped child if the test panics before an explicit `kill()`
/// — same convention as `kill9_reclaim_live.rs`'s own `KillOnDrop`.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_hooks_worker(database_url: &str, worker_id: &str) -> KillOnDrop {
    let bin = env!("CARGO_BIN_EXE_sms-worker");
    let config = TestConfig::write(
        &format!("hooks-node-{worker_id}"),
        &config::no_providers_overlay(),
    );
    let child = Command::new(bin)
        .args(["--roles", "hooks"])
        .args(["--database-url", database_url])
        .args(config.args())
        .args(["--worker-id", worker_id])
        .arg("--envelope-signing-key-file")
        .arg(write_signing_key())
        // Ephemeral metrics port: the default is a fixed 127.0.0.1:9091 and
        // `cargo test` runs test binaries in parallel, so two suites that each
        // spawn a worker contend for it and the loser dies at startup before
        // claiming anything. That took vsms's own main red on 2026-09-20.
        .args(["--metrics-listen", "127.0.0.1:0"])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawning the real sms-worker binary");
    KillOnDrop(child)
}

/// Polls the receiver's own `/__test__/results` until it reports at least
/// one entry, or fails with a message naming what it actually saw (empty,
/// or a rejection) — a bare timeout with no context is a bad debugging
/// experience for a test that spans two processes and a network hop.
async fn wait_for_receiver_result(receiver: &NodeReceiver, timeout: Duration) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let results = receiver.results().await;
        if let Some(result) = results.first() {
            return result.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the node gate-receiver never recorded any processed result within {timeout:?} \
             (receiver configured: {}); either the hooks role never reached it, or the \
             delivery was refused before reaching the point this test can observe",
            receiver.configured
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn reread_attempt(db: &Cratestack, id: &str) -> schema::WebhookAttempt {
    db.webhook_attempt()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::webhook_attempt::id().eq(id.to_owned()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("re-reading the attempt")
        .into_iter()
        .next()
        .expect("the seeded attempt still exists")
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres, the built sms-worker binary, Node >=23.6, the \
            built Node SDK and `pnpm install` in examples/node/webhook-receiver — see module docs"]
async fn a_real_hooks_delivery_is_independently_opened_by_the_node_receiver() {
    let _guard = TEST_MUTEX.lock().await;
    let database_url = sms_test_support::database_url().await;
    let db = db().await;

    let audience = format!("gate-receiver-{}", unique_suffix());
    let port = free_port();
    let app_id = seed_app(&db).await;
    let endpoint = seed_endpoint(&db, &app_id, &receiver_url(port), &audience).await;
    let receiver = NodeReceiver::spawn(port, &audience, &app_id, &endpoint.id).await;
    let aggregate_id = format!("cmsg{}", unique_suffix());
    let attempt = seed_attempt(&db, &endpoint.id, &aggregate_id).await;

    let worker_id = format!("hooks-node-receiver-test-{}", unique_suffix());
    let _worker = spawn_hooks_worker(&database_url, &worker_id);

    // The independent verification this gate exists to prove: not "the
    // attempt reached `succeeded` in our own database" (that's provable
    // without a receiver at all — see hooks_live_postgres.rs) but "a
    // second implementation of the open, running as its own OS process with
    // only vsms's public keys, accepted the request."
    let result = wait_for_receiver_result(&receiver, Duration::from_secs(20)).await;
    assert_eq!(
        result["status"].as_str(),
        Some("accepted-new"),
        "the node receiver's own verdict on the real hooks delivery: {result}"
    );
    assert_eq!(
        result["eventType"].as_str(),
        Some("message.delivered"),
        "the receiver's own reading of the opened envelope's type: {result}"
    );
    assert_eq!(
        result["aggregateId"].as_str(),
        Some(aggregate_id.as_str()),
        "the receiver's own data.messageId reading must match what this test seeded: {result}"
    );

    // Corroborate from this system's own side too — not the gate's main
    // claim, but confirms the two observations (Node's HTTP-level verdict,
    // this database's own state) agree.
    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, schema::AttemptState::succeeded);
    assert_eq!(reread.lastStatusCode, Some(202));
}

/// The deliberate break, kept: the endpoint says the receiver is called
/// something it is not configured as, so the genuine delivery is refused by the
/// receiver's independent opener and the worker treats it as a failed attempt.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres, the built sms-worker binary, Node >=23.6, the \
            built Node SDK and `pnpm install` in examples/node/webhook-receiver — see module docs"]
async fn a_delivery_addressed_to_another_audience_is_refused_by_the_node_receiver() {
    let _guard = TEST_MUTEX.lock().await;
    let database_url = sms_test_support::database_url().await;
    let db = db().await;

    let port = free_port();
    let app_id = seed_app(&db).await;
    let endpoint = seed_endpoint(
        &db,
        &app_id,
        &receiver_url(port),
        "an-audience-the-receiver-does-not-answer-to",
    )
    .await;
    let receiver = NodeReceiver::spawn(
        port,
        "the-audience-the-receiver-is-configured-with",
        &app_id,
        &endpoint.id,
    )
    .await;
    let attempt = seed_attempt(&db, &endpoint.id, &format!("cmsg{}", unique_suffix())).await;

    let worker_id = format!("hooks-node-receiver-test-{}", unique_suffix());
    let _worker = spawn_hooks_worker(&database_url, &worker_id);

    let result = wait_for_receiver_result(&receiver, Duration::from_secs(20)).await;
    assert_eq!(
        result["status"].as_str(),
        Some("rejected-unverified"),
        "the receiver must refuse a delivery addressed to someone else: {result}"
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let reread = reread_attempt(&db, &attempt.id).await;
        if reread.state == schema::AttemptState::failed {
            assert_eq!(reread.lastStatusCode, Some(401));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the refused attempt never reached failed: {:?}",
            reread.state
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The attack the signed `appId` closes, with a real worker and a real Node
/// receiver. One gateway key signs every app's deliveries, so an endpoint of
/// *another* app that names this receiver's audience (and points at it) yields a
/// seal that passes the signature, the audience, the route, the contract, the
/// clock and the nonce. The receiver is configured for one app and refuses it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres, the built sms-worker binary, Node >=23.6, the \
            built Node SDK and `pnpm install` in examples/node/webhook-receiver — see module docs"]
async fn a_delivery_made_for_another_app_is_refused_though_it_names_the_receivers_audience() {
    let _guard = TEST_MUTEX.lock().await;
    let database_url = sms_test_support::database_url().await;
    let db = db().await;

    let audience = format!("gate-receiver-{}", unique_suffix());
    let port = free_port();
    let receivers_app = seed_app(&db).await;
    // The receiver's own endpoint, still waiting for its audience to be set.
    let own = seed_endpoint(
        &db,
        &receivers_app,
        &receiver_url(port),
        &format!("not-yet-set-{}", unique_suffix()),
    )
    .await;
    let receiver = NodeReceiver::spawn(port, &audience, &receivers_app, &own.id).await;

    // The endpoint belongs to another app and names the receiver's audience.
    let attackers_app = seed_app(&db).await;
    let endpoint = seed_endpoint(&db, &attackers_app, &receiver_url(port), &audience).await;
    let attempt = seed_attempt(&db, &endpoint.id, &format!("cmsg{}", unique_suffix())).await;

    let worker_id = format!("hooks-node-receiver-test-{}", unique_suffix());
    let _worker = spawn_hooks_worker(&database_url, &worker_id);

    let result = wait_for_receiver_result(&receiver, Duration::from_secs(20)).await;
    assert_eq!(
        result["status"].as_str(),
        Some("rejected-unverified"),
        "the receiver must refuse a delivery made for another app: {result}"
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let reread = reread_attempt(&db, &attempt.id).await;
        if reread.state == schema::AttemptState::failed {
            assert_eq!(reread.lastStatusCode, Some(401));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the refused attempt never reached failed: {:?}",
            reread.state
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The attack the signed `endpointId` closes, with a real worker and a real Node
/// receiver. `WebhookEndpoint` create is allowed for a global role with a free
/// `appId`, so an endpoint can be registered under the receiver's own app and
/// named with the receiver's audience whenever that name is free (the receiver's
/// own endpoint does not carry it yet, or no longer). Its deliveries carry the
/// right app id and the right audience: the signature, the audience, the route,
/// the contract, the clock, the nonce and the app id all pass. Only the endpoint
/// id, the one signed value no one but the gateway chooses, tells it from the
/// receiver's own endpoint, and the receiver refuses it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres, the built sms-worker binary, Node >=23.6, the \
            built Node SDK and `pnpm install` in examples/node/webhook-receiver — see module docs"]
async fn a_delivery_from_a_second_endpoint_of_the_receivers_own_app_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let database_url = sms_test_support::database_url().await;
    let db = db().await;

    let audience = format!("gate-receiver-{}", unique_suffix());
    let port = free_port();
    let app_id = seed_app(&db).await;
    // The receiver's own endpoint, whose audience the operator has not set yet,
    // so the name the receiver answers to is free.
    let own = seed_endpoint(
        &db,
        &app_id,
        &receiver_url(port),
        &format!("not-yet-set-{}", unique_suffix()),
    )
    .await;
    let receiver = NodeReceiver::spawn(port, &audience, &app_id, &own.id).await;

    // A second endpoint of the same app takes the free name and points at the
    // receiver.
    let squatter = seed_endpoint(&db, &app_id, &receiver_url(port), &audience).await;
    assert_ne!(squatter.id, own.id);
    assert_eq!(squatter.appId, own.appId);
    let attempt = seed_attempt(&db, &squatter.id, &format!("cmsg{}", unique_suffix())).await;

    let worker_id = format!("hooks-node-receiver-test-{}", unique_suffix());
    let _worker = spawn_hooks_worker(&database_url, &worker_id);

    let result = wait_for_receiver_result(&receiver, Duration::from_secs(20)).await;
    assert_eq!(
        result["status"].as_str(),
        Some("rejected-unverified"),
        "the receiver must refuse another endpoint's delivery though the app and audience match: {result}"
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let reread = reread_attempt(&db, &attempt.id).await;
        if reread.state == schema::AttemptState::failed {
            assert_eq!(reread.lastStatusCode, Some(401));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the refused attempt never reached failed: {:?}",
            reread.state
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// An `hmac` endpoint, end to end: a real `hooks` worker signs with the
/// endpoint's secret, a real Node receiver (the original TypeScript verifier,
/// which shares no code with `sms-webhook`) verifies it, dedupes on the signed
/// `X-Sms-Event-Id` and reads the unchanged four-field body.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres, the built sms-worker binary, Node >=23.6, the \
            built Node SDK and `pnpm install` in examples/node/webhook-receiver — see module docs"]
async fn a_real_hmac_delivery_is_independently_verified_by_the_node_receiver() {
    let _guard = TEST_MUTEX.lock().await;
    let database_url = sms_test_support::database_url().await;
    let db = db().await;

    let secret = format!("whsec_gate_0123456789abcdef0123456789_{}", unique_suffix());
    let port = free_port();
    let app_id = seed_app(&db).await;
    let endpoint = seed_hmac_endpoint(&db, &app_id, &receiver_url(port), &secret, None).await;
    let receiver = NodeReceiver::spawn_hmac(port, &secret, None).await;
    let aggregate_id = format!("cmsg{}", unique_suffix());
    let attempt = seed_attempt(&db, &endpoint.id, &aggregate_id).await;

    let worker_id = format!("hooks-node-receiver-test-{}", unique_suffix());
    let _worker = spawn_hooks_worker(&database_url, &worker_id);

    let result = wait_for_receiver_result(&receiver, Duration::from_secs(20)).await;
    assert_eq!(
        result["status"].as_str(),
        Some("accepted-new"),
        "the node receiver's own verdict on the real hmac delivery: {result}"
    );
    assert_eq!(result["eventType"].as_str(), Some("message.delivered"));
    assert_eq!(result["aggregateId"].as_str(), Some(aggregate_id.as_str()));

    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, schema::AttemptState::succeeded);
    assert_eq!(reread.lastStatusCode, Some(202));
}

/// During a rotation's overlap window the worker signs with both secrets, so a
/// receiver that has learnt only the previous one still verifies: the property
/// that makes a rotation a non-event for a receiver.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres, the built sms-worker binary, Node >=23.6, the \
            built Node SDK and `pnpm install` in examples/node/webhook-receiver — see module docs"]
async fn an_hmac_delivery_in_a_rotation_window_verifies_at_a_receiver_holding_only_the_old_secret()
{
    let _guard = TEST_MUTEX.lock().await;
    let database_url = sms_test_support::database_url().await;
    let db = db().await;

    let old = format!("whsec_old_0123456789abcdef0123456789_{}", unique_suffix());
    let new = format!("whsec_new_0123456789abcdef0123456789_{}", unique_suffix());
    let port = free_port();
    let app_id = seed_app(&db).await;
    let endpoint = seed_hmac_endpoint(&db, &app_id, &receiver_url(port), &new, Some(&old)).await;
    let receiver = NodeReceiver::spawn_hmac(port, &old, None).await;
    let attempt = seed_attempt(&db, &endpoint.id, &format!("cmsg{}", unique_suffix())).await;

    let worker_id = format!("hooks-node-receiver-test-{}", unique_suffix());
    let _worker = spawn_hooks_worker(&database_url, &worker_id);

    let result = wait_for_receiver_result(&receiver, Duration::from_secs(20)).await;
    assert_eq!(result["status"].as_str(), Some("accepted-new"), "{result}");
    assert_eq!(
        reread_attempt(&db, &attempt.id).await.state,
        schema::AttemptState::succeeded
    );
}

/// The deliberate break for the `hmac` mode, kept: the receiver holds a secret
/// the endpoint does not, so the genuine delivery is refused by its independent
/// verifier and the worker treats it as a failed attempt.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres, the built sms-worker binary, Node >=23.6, the \
            built Node SDK and `pnpm install` in examples/node/webhook-receiver — see module docs"]
async fn an_hmac_delivery_signed_with_another_secret_is_refused_by_the_node_receiver() {
    let _guard = TEST_MUTEX.lock().await;
    let database_url = sms_test_support::database_url().await;
    let db = db().await;

    let port = free_port();
    let app_id = seed_app(&db).await;
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &receiver_url(port),
        &format!(
            "whsec_endpoint_0123456789abcdef0123456789_{}",
            unique_suffix()
        ),
        None,
    )
    .await;
    let receiver = NodeReceiver::spawn_hmac(
        port,
        "whsec_some_other_secret_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let attempt = seed_attempt(&db, &endpoint.id, &format!("cmsg{}", unique_suffix())).await;

    let worker_id = format!("hooks-node-receiver-test-{}", unique_suffix());
    let _worker = spawn_hooks_worker(&database_url, &worker_id);

    let result = wait_for_receiver_result(&receiver, Duration::from_secs(20)).await;
    assert_eq!(
        result["status"].as_str(),
        Some("rejected-unverified"),
        "{result}"
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let reread = reread_attempt(&db, &attempt.id).await;
        if reread.state == schema::AttemptState::failed {
            assert_eq!(reread.lastStatusCode, Some(401));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the refused attempt never reached failed: {:?}",
            reread.state
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
