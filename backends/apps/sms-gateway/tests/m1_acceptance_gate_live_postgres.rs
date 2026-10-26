//! #25's own acceptance gate, closing epic #18 (M1 - sms-auth), as VS-3
//! restated it for the signed transport: a service account registered through
//! the real operator path *actually persists* - surviving a genuine OS process
//! boundary, not just an in-memory handle - and a request **signed** by its key
//! against the restarted process can call a protected route. Separately, a
//! `developer`-role person (a console key paired to a real `User`) is refused
//! on a `provider:update`-gated route.
//!
//! #25's own text is explicit about why this matters: *"An in-memory-only
//! test passes with the `GrantType` bug present and is therefore worthless
//! here."* Every other live suite in this workspace builds its axum router
//! in-process and drives it with `reqwest` against a bound `TcpListener` -
//! real HTTP, real Postgres, but the same OS process the whole time. This
//! file is the one exception: it spawns `CARGO_BIN_EXE_sms-gateway` (the actual
//! `sms-gateway serve` binary, only reachable from an integration test inside
//! *this* package) as two genuinely separate `std::process::Child` processes:
//! register against the first, `kill()` (SIGKILL, not a graceful shutdown) and
//! `wait()` it to a real exit, then spawn a second, fresh process that has never
//! held anything about the first in memory, and prove a sealed call and a
//! refusal both work against *that* one.
//!
//! # What VS-3 changed in this gate
//!
//! - **Provisioning.** The service account is registered by running the real
//!   `sms-gateway register-client` as its own OS process (the operator path
//!   that replaced `provision-client`), which calls `registerAppClientKey`'s
//!   `invoke_with_db` so `@allow` is the real one. Nothing about the account
//!   is held in this test's memory but the *private* half of its key, which is
//!   the only part that never reaches the gateway.
//! - **Authentication.** There is no `/token` exchange: the second process
//!   verifies the request's signature against the `ClientKey` row the first
//!   process's era wrote. The server's own signing key is read from files, so a
//!   restart seals with the same key and the pinned public key keeps verifying -
//!   a property a restart could break and this gate now checks.
//! - **The developer case.** It used to need a hand-signed stand-in token,
//!   because no flow minted a role-bearing credential for a human. A paired
//!   console key is exactly that, so the `developer` here is a real `User` in
//!   the real `developer` role, signing for real, and the refusal is the real
//!   Layer 2 check.
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. `sms_test_support` provisions Postgres and applies every migration
//! automatically (a shared, self-healing container - see its own module doc),
//! so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-gateway --test m1_acceptance_gate_live_postgres -- --ignored --nocapture
//! ```

use std::net::TcpListener as StdTcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration as StdDuration;

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, SenderIdKind, SenderIdRegistrationStatus, provider as provider_filter,
};
use sms_test_support::sealed::TestKey;

#[path = "../../../crates/sms-api/tests/support/sealed_fixtures.rs"]
mod sealed_fixtures;

/// The `Provider.key` `sms-provider-orange-cm::OrangeCmProvider` reports —
/// `backends/apps/sms-gateway/src/main.rs`'s own `resolve_provider_row_id` looks up
/// exactly this row at startup and refuses to serve without it, since the
/// binary always mounts the DLR route (#34). Duplicated here rather than
/// imported: that constant is private to `sms-provider-orange-cm`'s own
/// `lib.rs` (`examples/send_test_message.rs` duplicates it the same way,
/// for the same reason).
const ORANGE_PROVIDER_KEY: &str = "orange_cm";

const TEST_HASH_PEPPER: &str = "m1-acceptance-gate-live-postgres-test-pepper-over-minimum";

fn sys() -> CratestackContext {
    Principal {
        sub: "m1-acceptance-gate-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CratestackContext {
    Principal {
        sub: "m1-acceptance-gate-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    cratestack::uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

/// A fresh MSISDN under the seeded `67x` (mtn) prefix - see
/// `backends/crates/sms-api/tests/send_message_live_postgres.rs`'s own copy of this
/// helper for why it needs cross-run, not just cross-call, uniqueness.
fn unique_mtn_msisdn() -> String {
    let n = u128::from_le_bytes(*cratestack::uuid::Uuid::new_v4().as_bytes()) % 1_000_000;
    format!("+237677{n:06}")
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

/// An OS-assigned free loopback port, read then released - the small
/// window before `sms-gateway serve` actually binds it is the same
/// TOCTOU every other test in this workspace that binds `127.0.0.1:0`
/// then reads `local_addr()` accepts; nothing else in this test binary
/// competes for ports.
fn free_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port");
    listener
        .local_addr()
        .expect("reading the bound address")
        .port()
}

/// Idempotent: reuses an existing `orange_cm` `Provider` row (reactivating
/// it if some earlier run left it inactive) rather than creating a fresh
/// one every run — this database is never reset between runs, and
/// `sms-gateway serve` only ever wants the single row matching its
/// configured adapter's key. Mirrors `examples/send_test_message.rs`'s
/// own `ensure_provider`.
async fn ensure_orange_cm_provider(db: &Cratestack) -> String {
    let existing = db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(
            provider_filter::key().eq(ORANGE_PROVIDER_KEY.to_owned()),
        ))
        .limit(1)
        .run(&owner())
        .await
        .expect("looking up an existing orange_cm Provider row");

    if let Some(row) = existing.into_iter().next() {
        if row.state != schema::ProviderState::active {
            db.provider()
                .update(row.id.clone())
                .set(schema::UpdateProviderInput {
                    state: Some(schema::ProviderState::active),
                    ..Default::default()
                })
                // #59: Provider is now @version'd.
                .if_match(row.version)
                .run(&owner())
                .await
                .expect("reactivating the orange_cm Provider row");
        }
        return row.id;
    }

    let created = db
        .provider()
        .create(schema::CreateProviderInput {
            key: ORANGE_PROVIDER_KEY.to_owned(),
            displayName: "Orange Cameroon (M1 acceptance gate test)".to_owned(),
            kind: schema::ProviderKind::orange_cm_http,
            config: "{}".to_owned(),
            credentialRef: "env:ORANGE_CM_CLIENT_ID".to_owned(),
            maxTps: 5.0,
            maxDailySubmissions: 5000,
            supportsDlr: true,
            supportsAlphaSender: true,
            supportsUcs2: true,
            supportsConcat: true,
            costPerSegmentXaf: "19".parse().expect("static decimal literal parses"),
            healthCheckedAt: None,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the orange_cm Provider row");

    db.provider()
        .update(created.id.clone())
        .set(schema::UpdateProviderInput {
            state: Some(schema::ProviderState::active),
            ..Default::default()
        })
        // #59: Provider is now @version'd.
        .if_match(created.version)
        .run(&owner())
        .await
        .expect("activating the orange_cm Provider row");

    created.id
}

async fn seed_app(db: &Cratestack) -> schema::App {
    let suffix = unique_suffix();
    db.app()
        .create(schema::CreateAppInput {
            name: "m1 acceptance gate test app".to_owned(),
            slug: format!("m1-acceptance-gate-{suffix}"),
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
}

/// An active `SenderId` with an `approved` registration against
/// `provider_id` — `sendMessage`'s own `resolve_sender_id` requires both.
async fn seed_approved_sender(db: &Cratestack, provider_id: &str) -> String {
    let suffix = unique_suffix();
    let value = format!("T{}", &suffix[..suffix.len().min(9)]).to_uppercase();

    let sender = db
        .sender_id()
        .create(schema::CreateSenderIdInput {
            value: value.clone(),
            kind: SenderIdKind::alphanumeric,
            notes: None,
        })
        .run(&owner())
        .await
        .expect("seeding a sender id");

    db.sender_id_registration()
        .create(schema::CreateSenderIdRegistrationInput {
            senderIdId: sender.id.clone(),
            providerId: provider_id.to_owned(),
            status: SenderIdRegistrationStatus::approved,
            submittedAt: Some(Utc::now()),
            approvedAt: Some(Utc::now()),
            reference: None,
            rejectionReason: None,
        })
        .run(&owner())
        .await
        .expect("seeding an approved registration");

    db.sender_id()
        .update(sender.id)
        .set(schema::UpdateSenderIdInput {
            active: Some(true),
            ..Default::default()
        })
        // #59: SenderId is now @version'd.
        .if_match(sender.version)
        .run(&owner())
        .await
        .expect("activating the sender id");

    value
}

/// A real, spawned `sms-gateway serve` OS process — `env!("CARGO_BIN_EXE_sms-gateway")`
/// is only set for an integration test inside *this* package, which is
/// exactly why this file lives here rather than in `sms-api`/`sms-auth`
/// alongside this workspace's other live suites.
struct GatewayProcess {
    child: Child,
    issuer: String,
}

impl GatewayProcess {
    /// Spawns `sms-gateway serve` bound to `127.0.0.1:{port}` against
    /// `db_url`, with dummy-but-well-formed Orange config (never
    /// exercised — this suite never runs `dispatch`, so the adapter is
    /// constructed but never asked to reach the network) and waits for it
    /// to actually start serving before returning.
    async fn spawn(db_url: &str, port: u16) -> Self {
        let issuer = format!("http://127.0.0.1:{port}");
        // The same key files for both processes: a restart that changed the
        // server key would make every pinned client fail to verify.
        let (current_key, next_key) = sealed_fixtures::write_server_key_files(
            &std::env::temp_dir().join(format!("sms-m1-gate-keys-{port}")),
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_sms-gateway"));
        command
            .arg("serve")
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--database-url")
            .arg(db_url)
            .arg("--issuer")
            .arg(&issuer)
            .arg("--orange-client-id")
            .arg("m1-acceptance-gate-test-orange-client-id")
            .arg("--orange-client-secret")
            .arg("m1-acceptance-gate-test-orange-client-secret")
            .arg("--orange-sender-number")
            .arg("+237600000000")
            .arg("--hash-pepper")
            .arg(TEST_HASH_PEPPER)
            .arg("--envelope-signing-key-file")
            .arg(&current_key)
            .arg("--envelope-next-signing-key-file")
            .arg(&next_key)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = command.spawn().expect("spawning sms-gateway serve");
        println!(
            "m1_acceptance_gate_live_postgres: spawned sms-gateway serve, pid {}, issuer {issuer}",
            child.id()
        );

        wait_until_ready(&issuer, &mut child).await;
        Self { child, issuer }
    }

    /// SIGKILL, then reap — a hard kill, not a graceful shutdown, so
    /// nothing this process held in memory (and nothing SIGTERM's
    /// graceful-drain path might otherwise flush) survives it. Blocks
    /// until the OS has genuinely finished exiting the process, which is
    /// why the caller can safely reuse the same port immediately
    /// afterward. Run on a blocking thread: `Child::kill`/`Child::wait`
    /// are synchronous syscalls, and this is called from async test code.
    async fn kill_and_wait(mut self) {
        let pid = self.child.id();
        tokio::task::spawn_blocking(move || {
            self.child
                .kill()
                .expect("sending SIGKILL to sms-gateway serve");
            let status = self
                .child
                .wait()
                .expect("reaping sms-gateway serve after SIGKILL");
            println!(
                "m1_acceptance_gate_live_postgres: killed sms-gateway serve, pid {pid}, exit status {status:?}"
            );
        })
        .await
        .expect("joining the blocking kill/wait task");
    }
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        // Best-effort safety net: if a test assertion panics before
        // `kill_and_wait` runs, don't leave an orphaned `sms-gateway
        // serve` holding the port for the rest of this test binary's run.
        // A no-op (ignored) if the process was already reaped.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Polls `/.well-known/openid-configuration` until it answers, or fails
/// fast if the child has already exited (a misconfiguration — e.g. the
/// `orange_cm` `Provider` row this suite seeds up front going missing —
/// would otherwise just look like a slow-starting server until this
/// function's own timeout, which is a much less useful failure to debug).
async fn wait_until_ready(issuer: &str, child: &mut Child) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(15);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("sms-gateway serve exited before becoming ready: {status:?}");
        }
        if let Ok(response) = client
            .get(format!("{issuer}/.well-known/openid-configuration"))
            .send()
            .await
            && response.status().is_success()
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "sms-gateway serve never became ready within 15s"
        );
        tokio::time::sleep(StdDuration::from_millis(200)).await;
    }
}

/// Runs the real `sms-gateway register-client` as its own OS process: the
/// operator path that registers an app's signing key. Returns once it exits 0.
async fn register_client_cli(db_url: &str, app_slug: &str, key: &TestKey) {
    let (db_url, app_slug, public_key) = (
        db_url.to_owned(),
        app_slug.to_owned(),
        key.public_key_b64u(),
    );
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
            .arg("register-client")
            .arg("--database-url")
            .arg(db_url)
            .arg("--app")
            .arg(app_slug)
            .arg("--public-key")
            .arg(public_key)
            .arg("--scope")
            .arg("sms:send")
            .output()
            .expect("running sms-gateway register-client")
    })
    .await
    .expect("joining the blocking register-client task");
    assert!(
        output.status.success(),
        "register-client must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_persisted_signing_client_survives_a_process_restart_and_a_developer_is_refused() {
    let db_url = sms_test_support::database_url().await;
    let db = db().await;

    // --- Setup: everything `sms-gateway serve` itself requires before it
    // will even start (an active OP signing key, until VS-4 deletes the OP;
    // a `Provider` row keyed `orange_cm`, `resolve_provider_row_id`'s own
    // requirement), plus what `sendMessage` needs downstream (an approved
    // `SenderId`, an `App` to register a client for). ---
    sms_auth::op::rotate_signing_key(&db, &sys(), sms_auth::op::ROTATION_OVERLAP)
        .await
        .expect("rotating in a first signing key");

    let provider_id = ensure_orange_cm_provider(&db).await;
    let app = seed_app(&db).await;
    let sender_id_value = seed_approved_sender(&db, &provider_id).await;
    let developer_key = TestKey::console();
    sealed_fixtures::seed_user_with_console_key(&db, "developer", &developer_key).await;

    let port = free_port();

    // 1 & 2: process #1.
    let process_one = GatewayProcess::spawn(&db_url, port).await;

    // 3: register a service account for real, through the actual
    // `register-client` operator command - its own OS process, the real
    // `registerAppClientKey` entry point, real Postgres persistence.
    let service_key = TestKey::app();
    register_client_cli(&db_url, &app.slug, &service_key).await;

    // 4: kill process #1 for real. Nothing beyond this point can be
    // served from memory process #1 held - it no longer exists.
    process_one.kill_and_wait().await;

    // 5: process #2 - a genuinely fresh OS process, same database, same
    // port, same key files, zero shared memory with process #1. If
    // registration had only updated in-memory state, this is where it would
    // show: process #2 has never observed the key except through Postgres.
    let process_two = GatewayProcess::spawn(&db_url, port).await;

    // 6: a request signed by the persisted key, against process #2, calls a
    // real protected mutation - proving the whole chain works end to end
    // only because the row is actually in Postgres.
    assert_persisted_client_can_send_after_restart(&process_two, &service_key, &sender_id_value)
        .await;

    // 7: the negative RBAC case - a `developer` (real user, real role, paired
    // key) must be refused on `PATCH /providers/{id}`, naming the missing
    // `provider:update` permission.
    assert_developer_is_refused_on_provider_write(&process_two, &developer_key, &provider_id).await;

    process_two.kill_and_wait().await;
}

/// Step 6 of the gate: a real sealed `sendMessage` against `process`, proving
/// the persisted `ClientKey` is what authenticates it - `process` shares no
/// memory with whatever registered the key.
async fn assert_persisted_client_can_send_after_restart(
    process: &GatewayProcess,
    key: &TestKey,
    sender_id_value: &str,
) {
    let reply = sealed_fixtures::client(&process.issuer, key)
        .request("POST", "/$procs/sendMessage")
        .json(&serde_json::json!({"args": {
            "to": unique_mtn_msisdn(),
            "body": "M1 acceptance gate (#25): a persisted signing client, \
                     used against a genuinely restarted process",
            "senderId": sender_id_value,
        }}))
        .send()
        .await;
    assert!(
        reply.sealed && (200..300).contains(&reply.status),
        "a persisted, registered client must be able to send ({}): {}",
        reply.status,
        reply.text()
    );
    assert!(
        reply.json()["messageId"].as_str().is_some(),
        "a successful send returns the persisted message's id: {}",
        reply.text()
    );
}

/// Step 7 of the gate: a `developer` must be refused on `PATCH
/// /providers/{id}`, naming the missing `provider:update` permission.
async fn assert_developer_is_refused_on_provider_write(
    process: &GatewayProcess,
    developer_key: &TestKey,
    provider_id: &str,
) {
    let reply = sealed_fixtures::client(&process.issuer, developer_key)
        .request("PATCH", "/providers/{id}")
        .params(&[provider_id])
        .json(&serde_json::json!({"maxTps": 10.0}))
        .send()
        .await;
    assert_eq!(reply.status, 403, "{}", reply.text());
    assert!(
        reply.json()["message"]
            .as_str()
            .unwrap_or_default()
            .contains("provider:update"),
        "expected the denial to name the missing permission: {}",
        reply.text()
    );
}
