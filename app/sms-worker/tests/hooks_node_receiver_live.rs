//! `#44`'s first gate assertion: "a sample Node receiver verifies the
//! signature with an independent implementation" — proved against the
//! *live delivery path*, not a fixture.
//!
//! `crates/sms-webhook/tests/cross_language_fixtures.rs` and
//! `examples/node/webhook-receiver/src/cross-language-vectors.test.ts`
//! already proved the *algorithm* matches, against signatures a third tool
//! (`openssl dgst -sha256 -hmac`) computed — see `crates/sms-webhook/src/
//! lib.rs`'s own module doc. That is not sufficient for this gate: it
//! proves the canonical string and the HMAC match, not that what the real
//! `hooks` role (#40) actually signs is what it actually sends, over a real
//! socket, with the real four headers, to a receiver that never saw this
//! crate's code. The interesting failure this test can catch and a fixture
//! test structurally cannot: a mismatch between what's signed and what's
//! on the wire (a header renamed, a body re-serialised between signing and
//! sending, a masking regression), or a header this repo's own docs and the
//! Node receiver's own `README.md` flagged as an *unconfirmed assumption*
//! (`X-Sms-Event-Id` really is `WebhookAttempt.sourceEventId`; `data.messageId`
//! really is what the receiver should key on) turning out to be wrong once a
//! real sender exists.
//!
//! # What's real here, and what isn't
//!
//! Real: a genuinely separate `sms-worker --roles hooks` OS process
//! (`CARGO_BIN_EXE_sms-worker`), a genuinely separate `node
//! src/gate-receiver.ts` OS process, a real loopback TCP connection between
//! them, a real Postgres-backed `WebhookAttempt` row claimed through the
//! real CAS claim loop (`crates/sms-worker/src/claim.rs`), and the actual
//! `examples/node/webhook-receiver` code — not a copy, not a reimplementation
//! — verifying it. Not real: the `WebhookAttempt` row itself is seeded
//! directly (`db.webhook_attempt().create(...)`, the same convention
//! `hooks_live_postgres.rs` already uses) rather than produced by a real
//! `Message` transition through #38's subscribers — that path is already
//! covered end to end by `crates/sms-api/tests/webhooks_live_postgres.rs`,
//! and re-deriving it here would only add a second, slower way to seed the
//! same row `hooks` actually claims, without exercising anything new about
//! *this* assertion (signing/sending/receiving).
//!
//! # Prerequisites this test cannot check for you
//!
//! - Docker (`sms_test_support` provisions and migrates Postgres).
//! - `cargo build -p sms-worker-bin` — `CARGO_BIN_EXE_sms-worker` points at
//!   whatever the last build produced; a stale or missing binary is a
//!   confusing failure otherwise (same caveat `kill9_reclaim_live.rs`
//!   documents).
//! - `pnpm install` inside `examples/node/webhook-receiver` (or from
//!   `examples/`, its pnpm workspace root) — Node's native TypeScript
//!   support needs no build step, but `express` still has to be resolvable
//!   from `node_modules`.
//! - Node ≥23.6 on `PATH` as `node` (this repo's own `.nvmrc` pins 24,
//!   which qualifies) — native TS support unflagged, per the receiver's own
//!   `package.json` `engines` field.
//!
//! ```bash
//! cargo build -p sms-worker-bin
//! (cd examples && pnpm install)
//! cargo test -p sms-worker-bin --test hooks_node_receiver_live -- --ignored --nocapture
//! ```
//!
//! # The deliberate-break evidence this gate asks for
//!
//! Confirmed by hand, not merely reasoned about (see this PR's own
//! description for the full transcript): seeding the `WebhookEndpoint`
//! with a secret that does **not** match the receiver's own
//! `WEBHOOK_RECEIVER_SECRET` reproduces exactly the failure this test
//! exists to catch — the receiver's independent `verifySignature`
//! genuinely rejects the request (`401`, logged `rejected-signature` on
//! the Node side), `hooks.rs` treats that as `Outcome::Retryable` and
//! backs off rather than reaching `succeeded`, and this test's own
//! `wait_for_receiver_result` deadline expires with a clear panic message
//! naming what never arrived. Restoring the matching secret makes it pass
//! again. That is the one inversion available without touching production
//! signing/verification code — the two implementations either agree or
//! they don't, and this is what "don't" looks like end to end.

use std::io::{BufRead, BufReader};
use std::net::TcpListener as StdTcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::CoolContext;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack, CreateWebhookAttemptInput, CreateWebhookEndpointInput};

fn sys() -> CoolContext {
    Principal {
        sub: "hooks-node-receiver-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
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

async fn db() -> Cratestack {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
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

/// `secret` is a parameter, not derived — the deliberate-break evidence
/// this module doc describes is "seed a mismatched secret," which needs a
/// call site that can do exactly that.
async fn seed_endpoint(
    db: &Cratestack,
    app_id: &str,
    url: &str,
    secret: &str,
) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: url.to_owned(),
            eventTypes: " message.delivered ".to_owned(),
            secret: secret.to_owned(),
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: true,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the webhook endpoint")
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

/// A real, spawned `node src/gate-receiver.ts` process — the actual
/// `examples/node/webhook-receiver` code, not a stand-in.
struct NodeReceiver {
    child: Child,
    base_url: String,
    secret: String,
}

impl NodeReceiver {
    async fn spawn(port: u16, secret: &str) -> Self {
        let package_dir = node_receiver_dir();
        let mut command = Command::new("node");
        command
            .current_dir(&package_dir)
            .arg("src/gate-receiver.ts")
            .env("WEBHOOK_RECEIVER_PORT", port.to_string())
            .env("WEBHOOK_RECEIVER_SECRET", secret)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = command.spawn().unwrap_or_else(|error| {
            panic!(
                "spawning `node src/gate-receiver.ts` in {} failed: {error} — is Node >=23.6 on \
                 PATH?",
                package_dir.display()
            )
        });

        // Echo the child's stdout to this test's own — `--nocapture` then
        // shows both processes interleaved, which is what made the
        // deliberate-break run (this module doc's own evidence) legible.
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
            secret: secret.to_owned(),
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
    // CARGO_MANIFEST_DIR is app/sms-worker; the Node package lives at the
    // repo root's examples/node/webhook-receiver.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/node/webhook-receiver")
        .canonicalize()
        .expect(
            "examples/node/webhook-receiver must exist relative to app/sms-worker's own \
             manifest dir",
        )
}

async fn wait_until_ready(base_url: &str, child: &mut Child) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            panic!(
                "node src/gate-receiver.ts exited before becoming ready: {status:?} — check \
                 that `pnpm install` has been run in examples/node/webhook-receiver"
            );
        }
        if let Ok(response) = reqwest::get(format!("{base_url}/healthz")).await {
            if response.status().is_success() {
                return;
            }
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
    let child = Command::new(bin)
        .args(["--roles", "hooks"])
        .args(["--database-url", database_url])
        .args(["--worker-id", worker_id])
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
             (secret used: {}); either the hooks role never reached it, or the signature was \
             rejected before ever reaching the point this test can observe",
            receiver.secret
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres, the built sms-worker binary, Node >=23.6, and \
            `pnpm install` in examples/node/webhook-receiver — see module docs"]
async fn a_real_hooks_delivery_is_independently_verified_by_the_node_receiver() {
    let database_url = sms_test_support::database_url().await;
    let db = db().await;

    let secret = format!("whsec_gate_test_{}", unique_suffix());
    let port = free_port();
    let receiver = NodeReceiver::spawn(port, &secret).await;

    let app_id = seed_app(&db).await;
    let endpoint = seed_endpoint(
        &db,
        &app_id,
        &format!("{}/webhooks/vsms", receiver.base_url),
        &secret,
    )
    .await;
    let aggregate_id = format!("cmsg{}", unique_suffix());
    let attempt = seed_attempt(&db, &endpoint.id, &aggregate_id).await;

    let worker_id = format!("hooks-node-receiver-test-{}", unique_suffix());
    let _worker = spawn_hooks_worker(&database_url, &worker_id);

    // The independent verification this gate exists to prove: not "the
    // attempt reached `succeeded` in our own database" (that's provable
    // without a receiver at all — see hooks_live_postgres.rs) but "a
    // second, from-scratch implementation of §4.4, running as its own OS
    // process, computed the same signature and accepted the request."
    let result = wait_for_receiver_result(&receiver, Duration::from_secs(20)).await;
    assert_eq!(
        result["status"].as_str(),
        Some("accepted-new"),
        "the node receiver's own verdict on the real hooks delivery: {result}"
    );
    assert_eq!(
        result["eventType"].as_str(),
        Some("message.delivered"),
        "the receiver's own X-Sms-Event / envelope.type reading: {result}"
    );
    assert_eq!(
        result["aggregateId"].as_str(),
        Some(aggregate_id.as_str()),
        "the receiver's own data.messageId reading must match what this test seeded: {result}"
    );

    // Corroborate from this system's own side too — not the gate's main
    // claim, but confirms the two observations (Node's HTTP-level verdict,
    // this database's own state) agree.
    let reread = db
        .webhook_attempt()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::webhook_attempt::id().eq(attempt.id.clone()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("re-reading the attempt")
        .into_iter()
        .next()
        .expect("the seeded attempt still exists");
    assert_eq!(reread.state, schema::AttemptState::succeeded);
    assert_eq!(reread.lastStatusCode, Some(202));
}
