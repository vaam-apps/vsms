//! #137's own acceptance gate: `sms-gateway provision-client` — the CLI
//! subcommand this PR adds — actually produces an HTTP-usable
//! `private_key_jwt` client, not just a well-typed `ProvisionClientResult`.
//!
//! This is the CLI-shaped complement to
//! `backends/apps/sms-gateway/tests/m1_acceptance_gate_live_postgres.rs`, which
//! already proves `Procedures::provision_app_client` itself persists
//! correctly across a process restart. What that file does *not* cover,
//! and what #137's own evidence names as the actual gap, is the tool: no
//! test anywhere invoked the CLI surface an operator would actually run.
//! `backends/crates/sms-api/examples/send_test_message.rs` is the cautionary
//! example this file exists to not repeat — it writes an `AppClient` row
//! directly without ever setting `OauthClient.jwks`, so that client can
//! never complete a real `private_key_jwt` exchange, and nothing before
//! this file ever ran the resulting binary and proved otherwise.
//!
//! Two fast, non-`#[ignore]`d tests cover the CLI's input validation
//! (invalid `--role`, refusing to overwrite an existing `--key-out`) —
//! both fail before the command ever touches Postgres, so they need no
//! live database and run under a plain `cargo test`. The live test proves
//! the actual claim: a client provisioned by running the real
//! `sms-gateway provision-client` binary, over real Postgres, hands back a
//! key file that a genuinely separate `sms-gateway serve` process accepts
//! for a real `/token` exchange, and the resulting access token makes a
//! real authenticated `sendMessage` call succeed.
//!
//! ```bash
//! cargo test -p sms-gateway --test provision_client_cli_live_postgres -- --ignored --nocapture
//! ```

use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration as StdDuration;

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, provider as provider_filter, Cratestack, SenderIdKind, SenderIdRegistrationStatus,
};

/// The `Provider.key` `sms-provider-orange-cm::OrangeCmProvider` reports —
/// see `m1_acceptance_gate_live_postgres.rs`'s own identical constant for
/// why it's duplicated rather than imported (`Provider` key is private to
/// that crate's own `lib.rs`).
const ORANGE_PROVIDER_KEY: &str = "orange_cm";

/// #134: both `provision-client` and `serve` now refuse to start without
/// `--hash-pepper`/`SMS_HASH_PEPPER`, even though neither this suite's CLI
/// invocation nor its `sendMessage` call cares which pepper is in effect —
/// see `m1_acceptance_gate_live_postgres.rs`'s own identical
/// `TEST_HASH_PEPPER`/`test_pepper` for the same reasoning. Only the value's
/// length matters (`HashPepper::new`'s own minimum).
const TEST_HASH_PEPPER: &str = "provision-client-cli-live-postgres-test-pepper-over-minimum";

/// #102, found live: within one test binary, Rust's default
/// multi-threaded harness can race two tests preparing the same
/// not-yet-cached query shape against Postgres's own `pg_type` catalog at
/// the same instant. See `backends/crates/sms-worker/tests/claim_live_postgres.rs`'s
/// own `TEST_MUTEX` doc for the full mechanism. Only the live test below
/// touches Postgres at all, but the mutex is taken regardless, per
/// `AGENTS.md`'s "any new live suite must take the per-binary mutex" rule.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn owner() -> CoolContext {
    Principal {
        sub: "provision-client-cli-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> CoolContext {
    Principal {
        sub: "provision-client-cli-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
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
        .expect("system clock is after the epoch")
        .subsec_nanos();
    format!("{:06x}", (u64::from(nanos).wrapping_add(n)) % 0x0100_0000)
}

/// A fresh MSISDN under the seeded `67x` (mtn) prefix — see
/// `backends/crates/sms-api/tests/send_message_live_postgres.rs`'s own copy of this
/// helper for why it needs cross-run, not just cross-call, uniqueness.
fn unique_mtn_msisdn() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .subsec_nanos();
    let unique = (u64::from(nanos) + n) % 1_000_000;
    format!("+237677{unique:06}")
}

/// A scratch path under the OS temp dir, uniquely suffixed per call so
/// concurrent test runs (and reruns against the same machine) never
/// collide on a leftover key file from an earlier run.
fn scratch_key_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("vsms-provision-client-cli-test-{label}.pem"))
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

async fn seed_app(db: &Cratestack) -> schema::App {
    let suffix = unique_suffix();
    db.app()
        .create(schema::CreateAppInput {
            name: "provision-client cli test app".to_owned(),
            slug: format!("provision-client-cli-{suffix}"),
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

/// Idempotent — reuses an existing `orange_cm` `Provider` row (reactivating
/// it if inactive) rather than creating a fresh one every run. Mirrors
/// `m1_acceptance_gate_live_postgres.rs`'s own `ensure_orange_cm_provider`.
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
            displayName: "Orange Cameroon (provision-client CLI test)".to_owned(),
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

/// Runs `sms-gateway provision-client` as a real, separate OS process —
/// exactly the binary an operator would run, not `Procedures` called
/// in-process. Returns the raw [`Output`] so callers can assert on exit
/// status, stdout, and stderr independently.
fn run_provision_client_cli(
    database_url: &str,
    app_id: &str,
    label: &str,
    scopes: &[&str],
    role: &str,
    key_out: &Path,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sms-gateway"));
    command
        .arg("provision-client")
        .arg("--database-url")
        .arg(database_url)
        .arg("--app-id")
        .arg(app_id)
        .arg("--label")
        .arg(label)
        .arg("--role")
        .arg(role)
        .arg("--key-out")
        .arg(key_out)
        .arg("--hash-pepper")
        .arg(TEST_HASH_PEPPER);
    for scope in scopes {
        command.arg("--scope").arg(scope);
    }
    command
        .output()
        .expect("running `sms-gateway provision-client`")
}

/// Extracts the client id from `provision-client`'s own documented stdout
/// contract (`main.rs`'s `provision_client_command`): a line reading
/// exactly `provisioned client: <id>`.
fn parse_client_id(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("provisioned client: "))
        .unwrap_or_else(|| {
            panic!("expected a \"provisioned client: <id>\" line in stdout, got: {stdout:?}")
        })
        .trim()
        .to_owned()
}

#[derive(serde::Serialize)]
struct AssertionClaims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    jti: String,
    exp: i64,
}

/// A `private_key_jwt` client assertion per RFC 7523 §3 — see
/// `m1_acceptance_gate_live_postgres.rs`'s own identical helper for why no
/// `kid` is set.
fn sign_client_assertion(client_id: &str, issuer: &str, private_key_pem: &str) -> String {
    let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
        .expect("building an EncodingKey from the private key PEM the CLI wrote");

    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    let claims = AssertionClaims {
        iss: client_id,
        sub: client_id,
        aud: issuer,
        jti: unique_suffix(),
        exp: (Utc::now() + Duration::minutes(1)).timestamp(),
    };

    jsonwebtoken::encode(&header, &claims, &encoding_key).expect("signing the client assertion")
}

async fn request_token(
    issuer: &str,
    client_id: &str,
    assertion: &str,
    scope: &str,
) -> serde_json::Value {
    let response = reqwest::Client::new()
        .post(format!("{issuer}/token"))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            (
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
            ),
            ("client_assertion", assertion),
            ("scope", scope),
        ])
        .send()
        .await
        .expect("POSTing to /token");
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .expect("parsing the token response as JSON");
    assert!(
        status.is_success(),
        "token request failed ({status}): {body}"
    );
    body
}

/// A real, spawned `sms-gateway serve` OS process. Trimmed down from
/// `m1_acceptance_gate_live_postgres.rs`'s own `GatewayProcess` — this
/// suite never needs a process restart, only one long enough to prove a
/// CLI-provisioned client works against it over real HTTP.
struct GatewayProcess {
    child: Child,
    issuer: String,
}

impl GatewayProcess {
    async fn spawn(db_url: &str, port: u16) -> Self {
        let issuer = format!("http://127.0.0.1:{port}");
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
            .arg("provision-client-cli-test-orange-client-id")
            .arg("--orange-client-secret")
            .arg("provision-client-cli-test-orange-client-secret")
            .arg("--orange-sender-number")
            .arg("+237600000000")
            .arg("--hash-pepper")
            .arg(TEST_HASH_PEPPER)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = command.spawn().expect("spawning sms-gateway serve");
        wait_until_ready(&issuer, &mut child).await;
        Self { child, issuer }
    }
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        // Best-effort: don't leave an orphaned `sms-gateway serve` holding
        // the port if a later assertion panics.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// An OS-assigned free loopback port, read then released — see
/// `m1_acceptance_gate_live_postgres.rs`'s own `free_port` for the
/// accepted TOCTOU window.
fn free_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port");
    listener
        .local_addr()
        .expect("reading the bound address")
        .port()
}

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
        {
            if response.status().is_success() {
                return;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "sms-gateway serve never became ready within 15s"
        );
        tokio::time::sleep(StdDuration::from_millis(200)).await;
    }
}

/// Unix file permission bits for `path`, e.g. `0o600`. Panics if `path`
/// does not exist or its metadata can't be read.
#[cfg(unix)]
fn unix_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .expect("reading metadata for the key file the CLI wrote")
        .permissions()
        .mode()
        & 0o777
}

// --- Fast, non-live tests: input validation that fails before the CLI
// ever touches Postgres, so these need no Docker and no #[ignore]. ---

#[test]
fn an_invalid_role_is_rejected_before_touching_the_database() {
    let key_out = scratch_key_path(&format!("invalid-role-{}", unique_suffix()));
    let output = run_provision_client_cli(
        // Deliberately unreachable — if the CLI ever tried to connect
        // before validating --role, this test would hang or fail with a
        // connection error instead of the intended validation error,
        // which is itself a useful signal that the ordering regressed.
        "postgres://unreachable.invalid:1/nope",
        "some-app-id",
        "some label",
        &["sms:send"],
        "not-a-real-role",
        &key_out,
    );
    assert!(
        !output.status.success(),
        "an invalid --role must be rejected: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("owner") && stderr.contains("admin"),
        "the rejection must name the two roles provisionAppClient's own @allow admits: {stderr}"
    );
    assert!(
        !key_out.exists(),
        "a rejected request must never write a key file"
    );
}

#[test]
fn refuses_to_overwrite_an_existing_key_out_file() {
    let key_out = scratch_key_path(&format!("existing-file-{}", unique_suffix()));
    std::fs::write(&key_out, b"not a real key, just occupying the path")
        .expect("seeding a pre-existing file at key_out");

    let output = run_provision_client_cli(
        // Same reasoning as the invalid-role test above: unreachable on
        // purpose, to prove the existence check runs before any attempt
        // to connect.
        "postgres://unreachable.invalid:1/nope",
        "some-app-id",
        "some label",
        &["sms:send"],
        "owner",
        &key_out,
    );
    assert!(
        !output.status.success(),
        "an existing --key-out file must be refused: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already exists"),
        "the rejection must say the file already exists: {stderr}"
    );
    assert_eq!(
        std::fs::read(&key_out).expect("reading back key_out"),
        b"not a real key, just occupying the path",
        "a refused overwrite must leave the original file byte-for-byte untouched"
    );

    let _ = std::fs::remove_file(&key_out);
}

// --- The live test: the actual claim #137 exists to prove. ---

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_client_provisioned_via_the_cli_completes_a_real_token_exchange_and_authenticated_call() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;

    // `sms-gateway serve` refuses to start without an active OP signing
    // key (`op::load_signing_keys`'s own error) — the same one-time setup
    // `docs/runbooks/getting-started.adoc` documents an operator running by
    // hand before the first `serve`.
    sms_auth::op::rotate_signing_key(&db, &sys(), sms_auth::op::ROTATION_OVERLAP)
        .await
        .expect("rotating in a first signing key");

    let app = seed_app(&db).await;
    let provider_id = ensure_orange_cm_provider(&db).await;
    let sender_id_value = seed_approved_sender(&db, &provider_id).await;

    let key_out = scratch_key_path(&unique_suffix());
    assert!(
        !key_out.exists(),
        "the scratch key path must start out absent: {}",
        key_out.display()
    );

    // Steps 1–4: run the real CLI binary, exactly as documented in
    // docs/runbooks/getting-started.adoc — not `Procedures` called
    // in-process — and prove its output contract and the key file it
    // wrote.
    let (client_id, private_key_pem) = provision_via_cli_and_read_key(&db_url, &app.id, &key_out);

    // Step 5: the actual claim — a genuinely separate `sms-gateway serve`
    // process, over real HTTP, exchanges the CLI-provisioned key for a
    // token and uses it for a real authenticated call. This is the exact
    // thing `send_test_message.rs` gets wrong (no `jwks`, so this step
    // would fail there) and the exact thing #137 asks to be verified live
    // rather than asserted.
    assert_cli_provisioned_client_can_send(&db_url, &client_id, &private_key_pem, &sender_id_value)
        .await;

    let _ = std::fs::remove_file(&key_out);
}

/// Steps 1–4 of the live test: run `sms-gateway provision-client` for
/// real, assert its stdout contract, assert the key file it wrote is
/// present with `0600` permissions and holds a real PEM, and assert a
/// second run against the same `--key-out` is refused rather than
/// clobbering the first key. Returns `(clientId, privateKeyPem)` for the
/// token-exchange step that follows.
fn provision_via_cli_and_read_key(db_url: &str, app_id: &str, key_out: &Path) -> (String, String) {
    let output = run_provision_client_cli(
        db_url,
        app_id,
        "provision-client cli acceptance test client",
        &["sms:send"],
        "owner",
        key_out,
    );
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "provision-client must succeed (status {:?}); stdout: {stdout}\nstderr: {stderr}",
        output.status
    );

    // The CLI's own documented contract — print the client id and where
    // the key landed, in a shape an operator can paste straight into the
    // console's environment — never the key material itself.
    let client_id = parse_client_id(&stdout);
    assert!(
        client_id.len() >= 8,
        "clientId must satisfy @length(min: 8, max: 64): {client_id}"
    );
    assert!(
        stdout.contains(&format!("SMS_CONSOLE_CLIENT_ID={client_id}")),
        "stdout must print a line an operator can paste straight in: {stdout}"
    );
    assert!(
        stdout.contains(&key_out.display().to_string()),
        "stdout must print the key file path: {stdout}"
    );
    assert!(
        !stdout.contains("PRIVATE KEY") && !stderr.contains("PRIVATE KEY"),
        "the private key must never be printed anywhere — only written to --key-out"
    );

    // The key file itself — present, restrictively permissioned, and a
    // real PEM.
    assert!(key_out.exists(), "the key file must exist after success");
    #[cfg(unix)]
    assert_eq!(
        unix_mode(key_out),
        0o600,
        "the key file must be created 0600, not the umask default"
    );
    let private_key_pem = std::fs::read_to_string(key_out).expect("reading the key file");
    assert!(
        private_key_pem.contains("PRIVATE KEY"),
        "the key file must hold a real PEM-encoded private key"
    );

    // Running the exact same command again must refuse, not clobber the
    // key just written.
    let repeat = run_provision_client_cli(
        db_url,
        app_id,
        "provision-client cli acceptance test client (second attempt)",
        &["sms:send"],
        "owner",
        key_out,
    );
    assert!(
        !repeat.status.success(),
        "a second run against the same --key-out must be refused"
    );
    assert_eq!(
        std::fs::read_to_string(key_out).expect("reading the key file after the refused rerun"),
        private_key_pem,
        "a refused overwrite must leave the first key untouched"
    );

    (client_id, private_key_pem)
}

/// Step 5 of the live test: spawn a genuinely separate `sms-gateway
/// serve` process, exchange the CLI-provisioned key for a real access
/// token over real HTTP, and use it for a real authenticated
/// `sendMessage` call.
async fn assert_cli_provisioned_client_can_send(
    db_url: &str,
    client_id: &str,
    private_key_pem: &str,
    sender_id_value: &str,
) {
    let port = free_port();
    let process = GatewayProcess::spawn(db_url, port).await;

    let assertion = sign_client_assertion(client_id, &process.issuer, private_key_pem);
    let token_response = request_token(&process.issuer, client_id, &assertion, "sms:send").await;
    assert_eq!(
        token_response["scope"].as_str(),
        Some("sms:send"),
        "the OP must echo back exactly the scope the CLI provisioned the client with: \
         {token_response}"
    );
    let access_token = token_response["access_token"]
        .as_str()
        .expect("token response carries access_token");

    let send_response = reqwest::Client::new()
        .post(format!("{}/$procs/sendMessage", process.issuer))
        .bearer_auth(access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({"args": {
            "to": unique_mtn_msisdn(),
            "body": "provision-client CLI acceptance test (#137): a client provisioned by \
                     running the real binary, exchanged and used over real HTTP",
            "senderId": sender_id_value,
        }}))
        .send()
        .await
        .expect("calling sendMessage with the CLI-provisioned client's own token");
    let send_status = send_response.status();
    let send_body: serde_json::Value = send_response
        .json()
        .await
        .expect("parsing the sendMessage response");
    assert!(
        send_status.is_success(),
        "a CLI-provisioned, freshly-token-exchanged client must be able to send \
         ({send_status}): {send_body}"
    );
    assert!(
        send_body["messageId"].as_str().is_some(),
        "a successful send returns the persisted message's id: {send_body}"
    );
}
