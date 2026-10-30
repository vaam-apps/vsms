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
//! # VS-4: no `OpenID` Provider
//!
//! The process is started with no signing key of an `OpenID` Provider and no
//! issuer: neither exists any more, and `serve` needs neither.
//! `the_deleted_openid_provider_routes_are_not_served` requests each route the
//! provider used to mount (`/token`, `/jwks.json`, discovery, `/login`,
//! `/login/sso/*`) from that same real process and asserts the router's
//! not-found.
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
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration as StdDuration;

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, SenderIdKind, SenderIdRegistrationStatus, provider as provider_filter,
};
use sms_test_support::config::{self, TestConfig};
use sms_test_support::sealed::TestKey;

#[path = "../../../crates/sms-api/tests/support/sealed_fixtures.rs"]
mod sealed_fixtures;

/// Both tests spawn real processes and share one database: one at a time,
/// per the "any new live suite must take the per-binary mutex" rule (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The `Provider.key` `sms-provider-orange-cm::OrangeCmProvider` reports —
/// `backends/apps/sms-gateway/src/main.rs`'s own `resolve_provider_row_id` looks up
/// exactly this row at startup and refuses to serve without it, since the
/// binary always mounts the DLR route (#34). Duplicated here rather than
/// imported: that constant is private to `sms-provider-orange-cm`'s own
/// `lib.rs` (`examples/send_test_message.rs` duplicates it the same way,
/// for the same reason).
const ORANGE_PROVIDER_KEY: &str = "orange_cm";

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
/// exactly why this file lives here rather than in `sms-api`
/// alongside this workspace's other live suites.
struct GatewayProcess {
    child: Child,
    base_url: String,
}

impl GatewayProcess {
    /// Spawns `sms-gateway serve` bound to `127.0.0.1:{port}` against
    /// `db_url`, with dummy-but-well-formed Orange config (never
    /// exercised — this suite never runs `dispatch`, so the adapter is
    /// constructed but never asked to reach the network) and waits for it
    /// to actually start serving before returning.
    async fn spawn(db_url: &str, port: u16) -> Self {
        Self::spawn_with(db_url, port, "", false).await
    }

    /// [`Self::spawn`] with `extra` YAML appended to the profile overlay (the
    /// `auth:` block a case wants to try), and, with `capture`, the process's
    /// stdout (where `tracing` writes) piped instead of inherited so [`Self::kill_and_read_log`] can read
    /// what it logged at boot.
    async fn spawn_with(db_url: &str, port: u16, extra: &str, capture: bool) -> Self {
        let base_url = format!("http://127.0.0.1:{port}");
        // The same key files for both processes: a restart that changed the
        // server key would make every pinned client fail to verify.
        let (current_key, next_key) = sealed_fixtures::write_server_key_files(
            &std::env::temp_dir().join(format!("sms-m1-gate-keys-{port}")),
        );
        // Never exercised: this suite never runs `dispatch`, so the adapter is
        // constructed but never asked to reach the network.
        let config = TestConfig::write(
            Path::new(env!("CARGO_TARGET_TMPDIR")),
            &format!("m1-gate-{port}"),
            &format!("{}\n{extra}", config::orange_overlay("http://127.0.0.1:9")),
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_sms-gateway"));
        command
            .arg("serve")
            .args(config.args())
            .env(config::PEPPER_VAR, config::PEPPER)
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--database-url")
            .arg(db_url)
            .arg("--envelope-signing-key-file")
            .arg(&current_key)
            .arg("--envelope-next-signing-key-file")
            .arg(&next_key)
            .env("NO_COLOR", "1")
            .stderr(Stdio::inherit())
            .stdout(if capture {
                Stdio::piped()
            } else {
                Stdio::inherit()
            });

        let mut child = command.spawn().expect("spawning sms-gateway serve");
        println!(
            "m1_acceptance_gate_live_postgres: spawned sms-gateway serve, pid {}, base_url {base_url}",
            child.id()
        );

        wait_until_ready(&base_url, &mut child).await;
        Self { child, base_url }
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

impl GatewayProcess {
    /// [`Self::kill_and_wait`], returning what the process logged to stdout. Only
    /// meaningful for a process spawned with `capture`.
    async fn kill_and_read_log(mut self) -> String {
        tokio::task::spawn_blocking(move || {
            self.child
                .kill()
                .expect("sending SIGKILL to sms-gateway serve");
            let mut log = String::new();
            if let Some(mut stdout) = self.child.stdout.take() {
                std::io::Read::read_to_string(&mut stdout, &mut log)
                    .expect("reading the gateway's log");
            }
            self.child.wait().expect("reaping sms-gateway serve");
            log
        })
        .await
        .expect("joining the blocking kill/read task")
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

/// Polls `/healthz` until it answers, or fails
/// fast if the child has already exited (a misconfiguration — e.g. the
/// `orange_cm` `Provider` row this suite seeds up front going missing —
/// would otherwise just look like a slow-starting server until this
/// function's own timeout, which is a much less useful failure to debug).
async fn wait_until_ready(base_url: &str, child: &mut Child) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(15);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("sms-gateway serve exited before becoming ready: {status:?}");
        }
        if let Ok(response) = client.get(format!("{base_url}/healthz")).send().await
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
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;

    // --- Setup: everything `sms-gateway serve` itself requires before it
    // will even start (a `Provider` row keyed `orange_cm`,
    // `resolve_provider_row_id`'s own requirement - and no signing key of an
    // `OpenID` Provider, which no longer exists), plus what `sendMessage` needs
    // downstream (an approved `SenderId`, an `App` to register a client
    // for). ---
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
    let reply = sealed_fixtures::client(&process.base_url, key)
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
    let reply = sealed_fixtures::client(&process.base_url, developer_key)
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

/// Every route the embedded `OpenID` Provider and the human login mounted
/// before VS-4, with the method it was mounted for. Each is a request a stale
/// client or a script might still make; each must now fall through to the
/// router's not-found rather than reach any handler.
const DELETED_ROUTES: &[(&str, &str)] = &[
    ("POST", "/token"),
    ("GET", "/jwks.json"),
    ("GET", "/.well-known/openid-configuration"),
    ("POST", "/login"),
    ("GET", "/login/sso/start"),
    ("GET", "/login/sso/callback"),
];

/// VS-4's acceptance: the deleted routes answer 404 from a real
/// `sms-gateway serve`, which itself started with no OP signing key in the
/// database (there is no such table) and no `--issuer`. A route that is still
/// served (`/healthz`) is requested first, so a 404 below cannot be a process
/// that is not listening or a wrong base URL.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_deleted_openid_provider_routes_are_not_served() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;

    let process = GatewayProcess::spawn(&db_url, free_port()).await;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("building an HTTP client");

    let control = http
        .get(format!("{}/healthz", process.base_url))
        .send()
        .await
        .expect("requesting /healthz");
    assert_eq!(
        control.status().as_u16(),
        200,
        "the control route must still be served"
    );

    for (method, path) in DELETED_ROUTES {
        let url = format!("{}{path}", process.base_url);
        let request = match *method {
            "POST" => http
                .post(&url)
                .header("content-type", "application/x-www-form-urlencoded")
                .body("grant_type=client_credentials&client_id=appc_gone"),
            _ => http.get(&url),
        };
        let response = request
            .send()
            .await
            .unwrap_or_else(|error| panic!("requesting {method} {path}: {error}"));
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        assert_eq!(
            status, 404,
            "{method} {path} was an `OpenID` Provider route and must be gone, got {status}: {body}"
        );
        assert!(
            !body.contains("access_token")
                && !body.contains("\"keys\"")
                && !body.contains("issuer"),
            "{method} {path} must not return a token, a key set or a discovery document: {body}"
        );
    }

    process.kill_and_wait().await;
}

/// VSA-1's discovery document, from a real `sms-gateway serve`: mounted, outside
/// the envelope (a plain unsigned `GET` answers it), and listing what `auth.methods`
/// says, for the key absent, `[keypair]` (vaam) and `[keypair, hmac_webhooks]`.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_real_serve_lists_the_auth_methods_it_serves_without_a_signature() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let http = reqwest::Client::new();
    let app_id = db
        .app()
        .create(schema::CreateAppInput {
            name: "auth discovery test app".to_owned(),
            slug: format!(
                "auth-discovery-{}",
                &cratestack::uuid::Uuid::new_v4().simple().to_string()[..12]
            ),
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
        .id;

    for (written, expected, boot, hmac_create_status) in [
        (
            "",
            serde_json::json!(["keypair", "hmac_webhooks"]),
            "permitted=keypair,machine_tokens,hmac_webhooks,human_login served=keypair,hmac_webhooks explicit=false",
            201,
        ),
        (
            "auth:\n  methods: [keypair]\n",
            serde_json::json!(["keypair"]),
            "permitted=keypair served=keypair explicit=true",
            422,
        ),
        (
            "auth:\n  methods: [keypair, hmac_webhooks]\n",
            serde_json::json!(["keypair", "hmac_webhooks"]),
            "permitted=keypair,hmac_webhooks served=keypair,hmac_webhooks explicit=true",
            201,
        ),
    ] {
        let process = GatewayProcess::spawn_with(&db_url, free_port(), written, true).await;
        let response = http
            .get(format!("{}/.well-known/vsms-auth", process.base_url))
            .send()
            .await
            .expect("requesting the discovery document");
        assert_eq!(response.status().as_u16(), 200, "auth: {written:?}");
        let body: serde_json::Value = response.json().await.expect("a JSON document");
        assert_eq!(
            body,
            serde_json::json!({ "methods": expected }),
            "auth: {written:?}"
        );

        // The same setting reaches the API: a paired developer (who holds
        // `webhook:manage`) creating an hmac endpoint through the generated
        // route is refused or accepted as the YAML says.
        let key = TestKey::console();
        sealed_fixtures::seed_user_with_console_key(&db, "developer", &key).await;
        let developer = sealed_fixtures::client(&process.base_url, &key);
        let reply = developer
            .request("POST", "/webhook_endpoints")
            .json(&serde_json::json!({
                "appId": app_id,
                "url": "https://example.test/hooks",
                "eventTypes": " message.delivered ",
                "deliveryMode": "hmac",
                "secret": "k".repeat(32),
                "maskRecipient": true,
                "maxAttempts": 8,
            }))
            .send()
            .await;
        assert_eq!(
            reply.status,
            hmac_create_status,
            "auth: {written:?}: creating an hmac endpoint: {}",
            reply.text()
        );
        if hmac_create_status == 422 {
            assert!(
                reply.json()["message"]
                    .as_str()
                    .is_some_and(|message| message.starts_with("auth_method_disabled: ")),
                "{}",
                reply.text()
            );
        }

        let log = process.kill_and_read_log().await;
        assert!(
            log.contains(boot),
            "auth: {written:?}; the boot line is missing: {log}"
        );
        let hmac_on = written.is_empty() || written.contains("hmac_webhooks");
        assert!(
            log.contains(&format!("hmac_webhooks={hmac_on}")) && log.contains("hmac_endpoints="),
            "auth: {written:?}; the hmac endpoint count is missing from the boot log: {log}"
        );
    }
}
