//! The live gate for the console sign-in (#194, restored by VSA-4a): a real
//! human login through the real HTTP surface, against a genuinely spawned
//! `sms-gateway serve` process - not an in-process router, because `POST /login`
//! is a raw axum route `main.rs` mounts and the operator commands are separate
//! OS processes (the precedent is `m1_acceptance_gate_live_postgres.rs` and
//! `machine_tokens_live_postgres.rs`).
//!
//! What this proves, end to end over real HTTP:
//!
//! 1. A correct email/password, correct PKCE `code_verifier` and matching `state`
//!    complete the `/login` -> `/token` round trip. `state` and `nonce` survive it
//!    byte for byte, an `id_token` comes back, **and no refresh token does**. The
//!    access token **is a credential for the API** (VSA-4b), and the `id_token`
//!    beside it is not; a console key paired to the same person is one too.
//! 2. **The PKCE guard-failure proof**: a wrong `code_verifier` is `invalid_grant`.
//!    A code is single use. Only S256 is accepted.
//! 3. **One generic 401** for every refusal of the person: an unknown email, a
//!    wrong password, a deactivated account, an account with no password, and a
//!    `client_id` that is not the console's.
//! 4. **The limiter runs before the password is looked at**: past the burst even
//!    the correct password is a 429.
//! 5. **The grant gate**: `refresh_token` is `unsupported_grant_type` whatever is
//!    served; `client_credentials` is refused when only `human_login` is served
//!    and `authorization_code` when only `machine_tokens` is; discovery lists
//!    exactly the served grants and no `authorization_endpoint`.
//! 6. **`serve` refuses to start** on a console client row that is missing, that
//!    carries `refresh_token` or that does not match the configuration, naming
//!    `seed-console-client`; the command reconciles it and `serve` then boots.
//! 7. **The operator commands**: `provision-user --password` writes the password
//!    to a `0600` file and prints it nowhere, refuses an existing password without
//!    `--reset`, and the password it wrote signs in; `bootstrap
//!    --owner-password-out` seeds the console client and does the same. (That all
//!    three refuse before the database when `human_login` is not served is
//!    `config_cutover.rs`, which needs no Postgres.)
//!
//! Ignored by default, like this workspace's other live suites:
//!
//! ```bash
//! cargo test -p sms-gateway --test login_flow_live_postgres -- --ignored --test-threads=1 --nocapture
//! ```

use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration as StdDuration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sha2::{Digest, Sha256};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack, provider as provider_filter};
use sms_test_support::config::{self, TestConfig};
use sms_test_support::sealed::TestKey;

#[path = "../../../crates/sms-api/tests/support/sealed_fixtures.rs"]
mod sealed_fixtures;

/// Real processes sharing one database: one at a time (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const ORANGE_PROVIDER_KEY: &str = "orange_cm";
const REDIRECT_URI: &str = "http://127.0.0.1:1/callback";
/// Generated per run rather than a literal: `CodeQL`'s
/// `rust/hard-coded-cryptographic-value` flags a literal that reaches a password
/// sink, even in a test, and a random value is just as good here.
static TEST_PASSWORD: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| sms_core::password::generate_password(32));

/// A password drawn at run time, for a test that needs one more than
/// [`TEST_PASSWORD`] (a wrong one, or a second account's). Never a literal.
fn test_password() -> String {
    sms_core::password::generate_password(32)
}

/// A throwaway `state` or `nonce`: random, so no test carries a fixed one.
fn token() -> String {
    sms_core::password::generate_password(16)
}

fn owner() -> CratestackContext {
    Principal {
        sub: "login-flow-gate-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> CratestackContext {
    sms_api::system_context("login-flow-gate-test-system")
}

fn unique_suffix() -> String {
    cratestack::uuid::Uuid::new_v4().simple().to_string()[..10].to_owned()
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
    StdTcpListener::bind("127.0.0.1:0")
        .expect("binding an ephemeral port")
        .local_addr()
        .expect("reading the bound address")
        .port()
}

/// `serve` resolves the `orange_cm` `Provider` row at startup (the DLR route).
async fn ensure_orange_cm_provider(db: &Cratestack) {
    let existing = db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(
            provider_filter::key().eq(ORANGE_PROVIDER_KEY.to_owned()),
        ))
        .limit(1)
        .run(&owner())
        .await
        .expect("looking up the orange_cm Provider row");
    if let Some(row) = existing.into_iter().next() {
        if row.state != schema::ProviderState::active {
            db.provider()
                .update(row.id.clone())
                .set(schema::UpdateProviderInput {
                    state: Some(schema::ProviderState::active),
                    ..Default::default()
                })
                .if_match(row.version)
                .run(&owner())
                .await
                .expect("reactivating the orange_cm Provider row");
        }
        return;
    }
    let created = db
        .provider()
        .create(schema::CreateProviderInput {
            key: ORANGE_PROVIDER_KEY.to_owned(),
            displayName: "Orange Cameroon (login flow test)".to_owned(),
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
        .if_match(created.version)
        .run(&owner())
        .await
        .expect("activating the orange_cm Provider row");
}

/// What a test deployment serves.
#[derive(Clone, Copy)]
enum Serves {
    /// `human_login` beside the keypair: the sign-in and no tokens.
    People,
    /// `machine_tokens` beside the keypair, and no `human_login:` block.
    Machines,
    /// The absent `auth.methods` with both blocks: everything.
    Both,
    /// `[machine_tokens, human_login]`: tokens alone, the keypair off.
    TokensOnly,
}

/// The buckets a test deployment is written with. The defaults are wide enough
/// that a test not about limiting never meets one.
#[derive(Clone, Copy)]
struct Limits {
    /// `human_login.login_rate_limit.per_email.burst`.
    per_email: u32,
    /// `human_login.login_rate_limit.aggregate.burst`.
    aggregate: u32,
    /// `op.token_rate_limit.burst`: per `client_id`, and per code for the
    /// `authorization_code` grant.
    token: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            per_email: 100,
            aggregate: 1000,
            token: 1000,
        }
    }
}

/// One spawned-process deployment: its own console client id (so a test that
/// breaks the row breaks nobody else's), its own configuration and key files.
struct Deployment {
    config: TestConfig,
    db_url: String,
    port: u16,
    keys: (PathBuf, PathBuf),
    console_client_id: String,
}

impl Deployment {
    async fn new(serves: Serves, login_burst: u32) -> Self {
        Self::with(
            serves,
            Limits {
                per_email: login_burst,
                ..Limits::default()
            },
        )
        .await
    }

    async fn with(serves: Serves, limits: Limits) -> Self {
        let port = free_port();
        let console_client_id = format!("console-{}", unique_suffix());
        let issuer = format!("http://127.0.0.1:{port}");
        let auth = match serves {
            Serves::People => "auth:\n  methods: [keypair, human_login]\n",
            Serves::Machines => "auth:\n  methods: [keypair, machine_tokens]\n",
            Serves::Both => "",
            Serves::TokensOnly => "auth:\n  methods: [machine_tokens, human_login]\n",
        };
        let op = format!(
            "op:\n  issuer: {issuer}\n  token_rate_limit:\n    burst: {}\n    \
             refill_per_second: 0.001\n  signing_key:\n    refresh_interval_secs: 1\n    \
             rotation_overlap_secs: 1800\n",
            limits.token
        );
        let human = format!(
            "human_login:\n  console_client:\n    client_id: {console_client_id}\n    \
             redirect_uris:\n      - {REDIRECT_URI}\n  login_rate_limit:\n    per_email:\n      \
             burst: {}\n      refill_per_second: 0.001\n    aggregate:\n      burst: {}\n      \
             refill_per_second: 0.001\n",
            limits.per_email, limits.aggregate
        );
        let blocks = match serves {
            Serves::Machines => op,
            Serves::People | Serves::Both | Serves::TokensOnly => format!("{op}{human}"),
        };
        let config = TestConfig::write(
            Path::new(env!("CARGO_TARGET_TMPDIR")),
            &format!("login-flow-{port}"),
            &format!(
                "{}\n{auth}{blocks}",
                config::orange_overlay("http://127.0.0.1:9")
            ),
        );
        let keys = sealed_fixtures::write_server_key_files(
            &std::env::temp_dir().join(format!("sms-login-flow-keys-{port}")),
        );
        Self {
            config,
            db_url: sms_test_support::database_url().await,
            port,
            keys,
            console_client_id,
        }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn serve_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sms-gateway"));
        command
            .arg("serve")
            .args(self.config.args())
            .env(config::PEPPER_VAR, config::PEPPER)
            .arg("--listen")
            .arg(format!("127.0.0.1:{}", self.port))
            .arg("--metrics-listen")
            .arg(format!("127.0.0.1:{}", free_port()))
            .arg("--database-url")
            .arg(&self.db_url)
            .arg("--envelope-signing-key-file")
            .arg(&self.keys.0)
            .arg("--envelope-next-signing-key-file")
            .arg(&self.keys.1)
            .env("NO_COLOR", "1");
        command
    }

    /// A long-lived `serve`, ready.
    async fn spawn(&self) -> Gateway {
        let mut child = self
            .serve_command()
            .stderr(Stdio::inherit())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawning sms-gateway serve");
        wait_until_ready(&self.base_url(), &mut child).await;
        Gateway {
            child,
            base_url: self.base_url(),
        }
    }

    /// A `serve` that must refuse to start: its stderr, once it has exited.
    fn serve_refusal(&self) -> String {
        let mut child = self
            .serve_command()
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawning sms-gateway serve");
        let deadline = std::time::Instant::now() + StdDuration::from_secs(60);
        loop {
            if let Some(status) = child.try_wait().expect("polling serve") {
                assert!(!status.success(), "serve must refuse, it exited {status:?}");
                let mut text = String::new();
                std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut text)
                    .expect("reading serve's stderr");
                return text;
            }
            if std::time::Instant::now() > deadline {
                child.kill().ok();
                child.wait().ok();
                panic!("serve neither refused nor exited within 60s: it started");
            }
            std::thread::sleep(StdDuration::from_millis(100));
        }
    }

    /// `sms-gateway <subcommand> --config … --profile … --database-url …` plus
    /// `args`, run to completion.
    fn cli(&self, subcommand: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
            .arg(subcommand)
            .args(self.config.args())
            .arg("--database-url")
            .arg(&self.db_url)
            .args(args)
            .env("NO_COLOR", "1")
            .output()
            .unwrap_or_else(|error| panic!("running `sms-gateway {subcommand}`: {error}"))
    }

    fn cli_ok(&self, subcommand: &str, args: &[&str]) -> String {
        let output = self.cli(subcommand, args);
        assert!(
            output.status.success(),
            "{subcommand} failed: {}",
            stderr(&output)
        );
        stdout(&output)
    }

    /// Everything a served sign-in needs in the database, the way an operator
    /// leaves it: a signing key and the console client.
    async fn prepare(&self) {
        let db = db().await;
        ensure_orange_cm_provider(&db).await;
        self.cli_ok("rotate-signing-key", &[]);
        self.cli_ok("seed-console-client", &[]);
    }
}

struct Gateway {
    child: Child,
    base_url: String,
}

impl Gateway {
    fn kill(mut self) {
        self.child.kill().expect("killing sms-gateway serve");
        self.child.wait().expect("reaping sms-gateway serve");
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn wait_until_ready(base_url: &str, child: &mut Child) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(30);
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
            "sms-gateway serve never became ready within 30s"
        );
        tokio::time::sleep(StdDuration::from_millis(200)).await;
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A `Role` + `User` + `UserCredential` (unless `password` is `None`), seeded the
/// way `provision-user --password` constructs them. Returns `(user id, email)`.
async fn seed_account(
    db: &Cratestack,
    suffix: &str,
    password: Option<&str>,
    active: bool,
) -> (String, String) {
    seed_account_in(db, suffix, None, password, active).await
}

/// [`seed_account`] in a built-in role (`admin`, `operator`, ...) instead of a
/// role made for the test, when `role_key` is `Some`.
async fn seed_account_in(
    db: &Cratestack,
    suffix: &str,
    role_key: Option<&str>,
    password: Option<&str>,
    active: bool,
) -> (String, String) {
    let role_key = if let Some(role_key) = role_key {
        role_key.to_owned()
    } else {
        let role_key = format!("loginflow{}", suffix.to_lowercase());
        db.role()
            .create(schema::CreateRoleInput {
                key: role_key.clone(),
                label: "login flow gate test role".to_owned(),
                description: None,
                permissions: " message:read ".to_owned(),
            })
            .run(&owner())
            .await
            .expect("seeding a Role");
        role_key
    };

    let email = format!("login-flow-{suffix}@example.test");
    let user = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("login-flow-subject-{suffix}"),
            email: email.clone(),
            displayName: "Login Flow Gate Test User".to_owned(),
            roleKey: role_key,
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding a User");
    if !active {
        db.user()
            .update(user.id.clone())
            .set(schema::UpdateUserInput {
                active: Some(false),
                ..Default::default()
            })
            .if_match(user.version)
            .run(&owner())
            .await
            .expect("deactivating the User");
    }
    if let Some(password) = password {
        db.user_credential()
            .create(schema::CreateUserCredentialInput {
                userId: user.id.clone(),
                passwordHash: sms_core::password::hash_password(password)
                    .expect("hashing the test password"),
            })
            .run(&sys())
            .await
            .expect("seeding a UserCredential");
    }
    (user.id, email)
}

/// A fresh, RFC 7636-shaped PKCE pair: 43 URL-safe-base64 characters of entropy
/// and its S256 challenge, the identical computation the provider runs to check it.
fn pkce_pair() -> (String, String) {
    let mut raw = [0u8; 32];
    rand::Rng::fill(&mut rand::thread_rng(), &mut raw);
    let verifier = URL_SAFE_NO_PAD.encode(raw);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// `/login`'s request body (`snake_case`: the OAuth parameter names).
fn login_body(
    deployment: &Deployment,
    email: &str,
    password: &str,
    state: &str,
    code_challenge: &str,
    nonce: &str,
) -> serde_json::Value {
    serde_json::json!({
        "email": email,
        "password": password,
        "client_id": deployment.console_client_id,
        "redirect_uri": REDIRECT_URI,
        "response_type": "code",
        "scope": "openid profile",
        "state": state,
        "code_challenge": code_challenge,
        "code_challenge_method": "S256",
        "nonce": nonce,
    })
}

struct Answer {
    status: reqwest::StatusCode,
    body: serde_json::Value,
    headers: reqwest::header::HeaderMap,
}

async fn post_login(base_url: &str, body: &serde_json::Value) -> Answer {
    let response = reqwest::Client::new()
        .post(format!("{base_url}/login"))
        .json(body)
        .send()
        .await
        .expect("POSTing to /login");
    let status = response.status();
    let headers = response.headers().clone();
    let text = response.text().await.unwrap_or_default();
    Answer {
        status,
        body: serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)),
        headers,
    }
}

async fn post_token(base_url: &str, form: &[(&str, &str)]) -> Answer {
    let response = reqwest::Client::new()
        .post(format!("{base_url}/token"))
        .form(form)
        .send()
        .await
        .expect("POSTing to /token");
    let status = response.status();
    let headers = response.headers().clone();
    let text = response.text().await.unwrap_or_default();
    Answer {
        status,
        body: serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)),
        headers,
    }
}

/// `code`/`state` parsed out of `/login`'s `{"redirect": "..."}`: the URL
/// `handle_authorize` built, never followed (nothing listens on [`REDIRECT_URI`]).
fn parse_code_and_state(redirect: &str) -> (String, String) {
    let query = redirect.split_once('?').map_or("", |(_, query)| query);
    let params: std::collections::HashMap<_, _> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    (
        params
            .get("code")
            .cloned()
            .unwrap_or_else(|| panic!("redirect {redirect} carries no code")),
        params
            .get("state")
            .cloned()
            .unwrap_or_else(|| panic!("redirect {redirect} carries no state")),
    )
}

/// Reads the `nonce` claim straight out of an `id_token`'s payload segment,
/// deliberately unverified: it proves the *value* survived `/login` -> `/token`.
fn id_token_nonce_claim(id_token: &str) -> Option<String> {
    let payload = URL_SAFE_NO_PAD.decode(id_token.split('.').nth(1)?).ok()?;
    let payload: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    payload["nonce"].as_str().map(str::to_owned)
}

/// A good login for `email`, returning `(code, verifier)`.
async fn sign_in(
    deployment: &Deployment,
    base_url: &str,
    email: &str,
    password: &str,
) -> (String, String) {
    let (verifier, challenge) = pkce_pair();
    let answer = post_login(
        base_url,
        &login_body(deployment, email, password, &token(), &challenge, &token()),
    )
    .await;
    assert_eq!(answer.status, 200, "a good sign-in: {}", answer.body);
    let (code, _) = parse_code_and_state(answer.body["redirect"].as_str().expect("a redirect"));
    (code, verifier)
}

fn exchange<'a>(
    deployment: &'a Deployment,
    code: &'a str,
    verifier: &'a str,
) -> [(&'a str, &'a str); 5] {
    [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", REDIRECT_URI),
        ("client_id", deployment.console_client_id.as_str()),
        ("code_verifier", verifier),
    ]
}

// --- the round trip ---------------------------------------------------------

/// VSA-4b: the API reads the person's access token, end to end from `/login`
/// through `/token`, as that person with the role their row says; the same
/// sign-in's `id_token` is not a credential.
async fn assert_the_api_reads_the_access_token_and_not_the_id_token(
    base_url: &str,
    access_token: &str,
    id_token: &str,
) {
    for (token, expected, what) in [
        (
            access_token,
            200,
            "a person's access token is accepted (VSA-4b)",
        ),
        (id_token, 401, "an ID token is not a credential"),
    ] {
        let reply = reqwest::Client::new()
            .get(format!("{base_url}/roles"))
            .bearer_auth(token)
            .send()
            .await
            .expect("calling GET /roles with a person's token");
        assert_eq!(reply.status(), expected, "{what}");
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_correct_login_completes_the_full_authorization_code_pkce_round_trip() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let (user_id, email) = seed_account(&db, &suffix, Some(TEST_PASSWORD.as_str()), true).await;
    let gateway = deployment.spawn().await;

    let (verifier, challenge) = pkce_pair();
    let state = format!("state-{suffix}");
    let nonce = format!("nonce-{suffix}");

    let login = post_login(
        &gateway.base_url,
        &login_body(
            &deployment,
            &email,
            TEST_PASSWORD.as_str(),
            &state,
            &challenge,
            &nonce,
        ),
    )
    .await;
    assert_eq!(
        login.status, 200,
        "a correct login must succeed: {}",
        login.body
    );
    assert_eq!(
        login
            .headers
            .get("cache-control")
            .map(|v| v.to_str().unwrap()),
        Some("no-store")
    );
    let (code, returned_state) =
        parse_code_and_state(login.body["redirect"].as_str().expect("a redirect"));
    assert_eq!(returned_state, state, "state must come back unchanged");

    let token = post_token(&gateway.base_url, &exchange(&deployment, &code, &verifier)).await;
    assert!(token.status.is_success(), "{} {}", token.status, token.body);
    let access_token = token.body["access_token"]
        .as_str()
        .expect("an access token");
    let id_token = token.body["id_token"]
        .as_str()
        .expect("an id_token (scope openid)");
    assert_eq!(
        id_token_nonce_claim(id_token).as_deref(),
        Some(nonce.as_str()),
        "the nonce must come back on the id_token unchanged"
    );
    assert!(
        token
            .body
            .get("refresh_token")
            .is_none_or(serde_json::Value::is_null),
        "no refresh token, ever: {}",
        token.body
    );

    // `lastLoginAt` is stamped by the login that just happened.
    let user = db
        .user()
        .find_unique(user_id.clone())
        .run(&sys())
        .await
        .unwrap()
        .unwrap();
    assert!(
        user.lastLoginAt.is_some(),
        "a good login stamps lastLoginAt"
    );

    assert_the_api_reads_the_access_token_and_not_the_id_token(
        &gateway.base_url,
        access_token,
        id_token,
    )
    .await;
    // A console key paired to the same person is let in by its signature too.
    let key = TestKey::console();
    sealed_fixtures::pair_console_key(&db, &user_id, &key, chrono::Duration::days(30)).await;
    let roles = sealed_fixtures::client(&gateway.base_url, &key)
        .request("GET", "/roles")
        .send()
        .await;
    assert!(
        roles.sealed && roles.status == 200,
        "a paired console key must authenticate: {} {}",
        roles.status,
        roles.text()
    );
    gateway.kill();
}

/// **The required PKCE guard-failure proof.** The identical flow, except the
/// `code_verifier` presented at `/token` does not match the `code_challenge`
/// presented at `/login`: `invalid_grant`, never a token.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_wrong_pkce_code_verifier_is_refused_at_the_token_exchange() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let (_, email) = seed_account(&db, &unique_suffix(), Some(TEST_PASSWORD.as_str()), true).await;
    let gateway = deployment.spawn().await;

    let (code, _right) = sign_in(
        &deployment,
        &gateway.base_url,
        &email,
        TEST_PASSWORD.as_str(),
    )
    .await;
    let (wrong_verifier, _) = pkce_pair();
    let token = post_token(
        &gateway.base_url,
        &exchange(&deployment, &code, &wrong_verifier),
    )
    .await;
    assert!(
        !token.status.is_success(),
        "{} {}",
        token.status,
        token.body
    );
    assert_eq!(
        token.body["error"].as_str(),
        Some("invalid_grant"),
        "{}",
        token.body
    );
    gateway.kill();
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_code_is_single_use() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let (_, email) = seed_account(&db, &unique_suffix(), Some(TEST_PASSWORD.as_str()), true).await;
    let gateway = deployment.spawn().await;

    let (code, verifier) = sign_in(
        &deployment,
        &gateway.base_url,
        &email,
        TEST_PASSWORD.as_str(),
    )
    .await;
    let first = post_token(&gateway.base_url, &exchange(&deployment, &code, &verifier)).await;
    assert!(first.status.is_success(), "{}", first.body);
    let second = post_token(&gateway.base_url, &exchange(&deployment, &code, &verifier)).await;
    assert!(
        !second.status.is_success(),
        "a code redeemed twice: {}",
        second.body
    );
    assert_eq!(
        second.body["error"].as_str(),
        Some("invalid_grant"),
        "{}",
        second.body
    );
    gateway.kill();
}

/// S256 only: `plain`, no method and no challenge are all refused, and what comes
/// back is a 400 and not a `{redirect}` the console could follow.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn only_s256_is_accepted_and_a_refusal_is_never_a_redirect() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let (_, email) = seed_account(&db, &unique_suffix(), Some(TEST_PASSWORD.as_str()), true).await;
    let gateway = deployment.spawn().await;

    let (verifier, challenge) = pkce_pair();
    for method in ["plain", "S512", "s256", ""] {
        let mut body = login_body(
            &deployment,
            &email,
            TEST_PASSWORD.as_str(),
            &token(),
            &challenge,
            &token(),
        );
        body["code_challenge_method"] = method.into();
        // `plain` would let the challenge be the verifier itself.
        if method == "plain" {
            body["code_challenge"] = verifier.clone().into();
        }
        let answer = post_login(&gateway.base_url, &body).await;
        assert_eq!(answer.status, 400, "method {method:?}: {}", answer.body);
        assert_eq!(answer.body["error"], "invalid_request", "method {method:?}");
        assert!(
            answer.body.get("redirect").is_none(),
            "method {method:?}: {}",
            answer.body
        );
    }
    gateway.kill();
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_redirect_uri_the_console_is_not_registered_with_is_refused_and_never_redirected_to() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let (_, email) = seed_account(&db, &unique_suffix(), Some(TEST_PASSWORD.as_str()), true).await;
    let gateway = deployment.spawn().await;

    let (_, challenge) = pkce_pair();
    let mut body = login_body(
        &deployment,
        &email,
        TEST_PASSWORD.as_str(),
        &token(),
        &challenge,
        &token(),
    );
    body["redirect_uri"] = "https://attacker.example/callback".into();
    let answer = post_login(&gateway.base_url, &body).await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert!(
        !answer.body.to_string().contains("attacker.example"),
        "{}",
        answer.body
    );

    // A scope the console is not registered for, `offline_access` above all: no
    // refresh token can be asked for.
    let mut body = login_body(
        &deployment,
        &email,
        TEST_PASSWORD.as_str(),
        &token(),
        &challenge,
        &token(),
    );
    body["scope"] = "openid profile offline_access".into();
    let answer = post_login(&gateway.base_url, &body).await;
    assert_eq!(
        answer.status, 400,
        "offline_access must be refused: {}",
        answer.body
    );
    gateway.kill();
}

// --- refusals ---------------------------------------------------------------

/// Every refusal of the person is the same answer: status, body and headers that
/// matter. Different answers would tell an attacker which half they got right.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn every_refusal_of_the_person_is_one_identical_401() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let good = seed_account(&db, &unique_suffix(), Some(TEST_PASSWORD.as_str()), true)
        .await
        .1;
    let inactive = seed_account(&db, &unique_suffix(), Some(TEST_PASSWORD.as_str()), false)
        .await
        .1;
    let no_password = seed_account(&db, &unique_suffix(), None, true).await.1;
    let gateway = deployment.spawn().await;
    let (_, challenge) = pkce_pair();

    let mut answers = Vec::new();
    let wrong_password = test_password();
    for (case, email, password, client_id) in [
        (
            "wrong password",
            good.as_str(),
            wrong_password.as_str(),
            None,
        ),
        (
            "unknown email",
            "nobody@example.test",
            TEST_PASSWORD.as_str(),
            None,
        ),
        (
            "deactivated",
            inactive.as_str(),
            TEST_PASSWORD.as_str(),
            None,
        ),
        (
            "no password set",
            no_password.as_str(),
            TEST_PASSWORD.as_str(),
            None,
        ),
        (
            "another client_id",
            good.as_str(),
            TEST_PASSWORD.as_str(),
            Some("another-client"),
        ),
        (
            "the machine client",
            good.as_str(),
            TEST_PASSWORD.as_str(),
            Some("appc_0123456789abcdef"),
        ),
    ] {
        let mut body = login_body(&deployment, email, password, &token(), &challenge, &token());
        if let Some(client_id) = client_id {
            body["client_id"] = client_id.into();
        }
        let answer = post_login(&gateway.base_url, &body).await;
        answers.push((case, answer));
    }
    for (case, answer) in &answers {
        assert_eq!(answer.status, 401, "{case}: {}", answer.body);
        assert_eq!(
            answer.body,
            serde_json::json!({ "error": "invalid_credentials" }),
            "{case}"
        );
        assert_eq!(
            answer
                .headers
                .get("cache-control")
                .map(|v| v.to_str().unwrap()),
            Some("no-store"),
            "{case}"
        );
    }
    gateway.kill();
}

/// **The limiter is in front of the password.** Past the burst, the *correct*
/// password is a 429: the request never reached the check, so no Argon2 work was
/// spent on it. (Source and email both draw on `login_rate_limit`; here one source
/// and one email, burst 3.)
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_limiter_answers_before_the_password_is_looked_at() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::People, 3).await;
    deployment.prepare().await;
    let (user_id, email) =
        seed_account(&db, &unique_suffix(), Some(TEST_PASSWORD.as_str()), true).await;
    let gateway = deployment.spawn().await;
    let (_, challenge) = pkce_pair();

    for attempt in 0..3 {
        let answer = post_login(
            &gateway.base_url,
            &login_body(
                &deployment,
                &email,
                &test_password(),
                &token(),
                &challenge,
                &token(),
            ),
        )
        .await;
        assert_eq!(answer.status, 401, "attempt {attempt} is within the burst");
    }
    let started = std::time::Instant::now();
    let throttled = post_login(
        &gateway.base_url,
        &login_body(
            &deployment,
            &email,
            TEST_PASSWORD.as_str(),
            &token(),
            &challenge,
            &token(),
        ),
    )
    .await;
    assert_eq!(
        throttled.status, 429,
        "past the burst even the correct password is throttled: {}",
        throttled.body
    );
    assert_eq!(throttled.body["error"], "rate_limited");
    // The proof that the password was never looked at: a *verified* password stamps
    // `lastLoginAt`, and the throttled request carried the right one.
    let user = db
        .user()
        .find_unique(user_id)
        .run(&sys())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        user.lastLoginAt, None,
        "a throttled request must not have been authenticated"
    );
    assert!(throttled.headers.contains_key("retry-after"));
    assert!(
        started.elapsed() < StdDuration::from_secs(2),
        "a throttled request is cheap"
    );
    // There is no source bucket: another email from this same machine (the same
    // TCP peer, which is everyone's behind the edge) is untouched by the
    // throttled one, and is judged on its own password.
    let other = format!("someone-else-{}@example.test", unique_suffix());
    let answer = post_login(
        &gateway.base_url,
        &login_body(
            &deployment,
            &other,
            &test_password(),
            &token(),
            &challenge,
            &token(),
        ),
    )
    .await;
    assert_eq!(
        answer.status, 401,
        "another address from the same peer is not throttled: {}",
        answer.body
    );
    gateway.kill();
}

/// **The lockout this release closed.** The sign-in used to be limited per TCP
/// peer, and behind Caddy (or the console's server) the peer is the same for
/// every person, so one bad `/login` every ten seconds from anywhere kept every
/// operator out. Here the "attacker" and the operator share a peer (loopback is
/// every client's address in this test) and the attacker spends everything one
/// email can spend; the operator, on another email, still signs in.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn one_address_hammering_the_sign_in_cannot_lock_other_operators_out() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::People, 3).await;
    deployment.prepare().await;
    let suffix = unique_suffix();
    let password = test_password();
    let (_, operator) = seed_account(&db, &suffix, Some(&password), true).await;
    let gateway = deployment.spawn().await;
    let (verifier, challenge) = pkce_pair();

    let attacked = format!("attacked-{suffix}@example.test");
    let mut statuses = Vec::new();
    for _ in 0..12 {
        let wrong = test_password();
        let answer = post_login(
            &gateway.base_url,
            &login_body(
                &deployment,
                &attacked,
                &wrong,
                &token(),
                &challenge,
                &token(),
            ),
        )
        .await;
        statuses.push(answer.status.as_u16());
    }
    assert_eq!(
        statuses,
        [401, 401, 401, 429, 429, 429, 429, 429, 429, 429, 429, 429],
        "the attacked address runs out of its own three tries"
    );

    let signed_in = post_login(
        &gateway.base_url,
        &login_body(
            &deployment,
            &operator,
            &password,
            &token(),
            &challenge,
            &token(),
        ),
    )
    .await;
    assert_eq!(
        signed_in.status, 200,
        "an operator on another address, from the same peer, still signs in: {}",
        signed_in.body
    );
    let (code, _) = parse_code_and_state(signed_in.body["redirect"].as_str().expect("redirect"));
    let redeemed = post_token(&gateway.base_url, &exchange(&deployment, &code, &verifier)).await;
    assert!(redeemed.status.is_success(), "{}", redeemed.body);
    gateway.kill();
}

/// The aggregate bucket: every sign-in draws on one budget, the Argon2 CPU
/// bound. Spent by distinct addresses, it refuses the next one with a `429` that
/// never reached the password check.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_aggregate_bucket_bounds_the_hashing_whoever_asks() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::with(
        Serves::People,
        Limits {
            aggregate: 3,
            ..Limits::default()
        },
    )
    .await;
    deployment.prepare().await;
    let password = test_password();
    let (user_id, email) = seed_account(&db, &unique_suffix(), Some(&password), true).await;
    let gateway = deployment.spawn().await;
    let (_, challenge) = pkce_pair();

    for index in 0..3 {
        let stranger = format!("stranger-{index}-{}@example.test", unique_suffix());
        let answer = post_login(
            &gateway.base_url,
            &login_body(
                &deployment,
                &stranger,
                &password,
                &token(),
                &challenge,
                &token(),
            ),
        )
        .await;
        assert_eq!(answer.status, 401, "{index}: {}", answer.body);
    }
    let throttled = post_login(
        &gateway.base_url,
        &login_body(
            &deployment,
            &email,
            &password,
            &token(),
            &challenge,
            &token(),
        ),
    )
    .await;
    assert_eq!(
        throttled.status, 429,
        "the shared budget is spent, so even the right password waits: {}",
        throttled.body
    );
    assert!(throttled.headers.contains_key("retry-after"));
    let user = db
        .user()
        .find_unique(user_id)
        .run(&sys())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        user.lastLoginAt, None,
        "a throttled request authenticated nobody"
    );
    gateway.kill();
}

/// **Bad codes cannot throttle a real redemption.** `/token` used to limit the
/// `authorization_code` grant per `client_id`, and the console's `client_id` is
/// public: anyone spamming `code=x` for it emptied the one bucket every
/// operator's real code (sixty seconds of life) needed. The grant is keyed on the
/// code now.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn spamming_bad_codes_cannot_throttle_a_real_redemption() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::with(
        Serves::People,
        Limits {
            token: 3,
            ..Limits::default()
        },
    )
    .await;
    deployment.prepare().await;
    let password = test_password();
    let (_, email) = seed_account(&db, &unique_suffix(), Some(&password), true).await;
    let gateway = deployment.spawn().await;
    let (code, verifier) = sign_in(&deployment, &gateway.base_url, &email, &password).await;

    let mut statuses = Vec::new();
    for _ in 0..15 {
        let bad = post_token(&gateway.base_url, &exchange(&deployment, "x", &verifier)).await;
        statuses.push(bad.status.as_u16());
    }
    assert_eq!(
        statuses.iter().filter(|status| **status == 429).count(),
        12,
        "the spammed code is throttled after its own three tries: {statuses:?}"
    );
    assert!(
        statuses.iter().take(3).all(|status| *status == 400),
        "and until then it is an ordinary invalid_grant: {statuses:?}"
    );

    let real = post_token(&gateway.base_url, &exchange(&deployment, &code, &verifier)).await;
    assert!(
        real.status.is_success(),
        "the real code has a bucket of its own: {} {}",
        real.status,
        real.body
    );
    gateway.kill();
}

/// What the request alone proves wrong is refused before the limiter and before
/// any hashing, as the `400` of a malformed request and never as the person's
/// `401`. With a per-email burst of **one**, a burst of such requests would
/// throttle the address if any of them spent a token: none does, and the good
/// request after them still signs in.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_request_that_can_never_succeed_spends_nothing_and_is_never_the_persons_401() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::People, 1).await;
    deployment.prepare().await;
    let password = test_password();
    let (user_id, email) = seed_account(&db, &unique_suffix(), Some(&password), true).await;
    let gateway = deployment.spawn().await;
    let (_, challenge) = pkce_pair();

    let mutations: [(&str, &str, serde_json::Value); 9] = [
        ("plain method", "code_challenge_method", "plain".into()),
        ("lower-case method", "code_challenge_method", "s256".into()),
        ("empty challenge", "code_challenge", "".into()),
        ("short challenge", "code_challenge", "a".repeat(42).into()),
        ("long challenge", "code_challenge", "a".repeat(129).into()),
        (
            "padded challenge",
            "code_challenge",
            format!("{challenge}=").into(),
        ),
        (
            "unregistered redirect",
            "redirect_uri",
            "https://evil.example/cb".into(),
        ),
        (
            "near-miss redirect",
            "redirect_uri",
            format!("{REDIRECT_URI}/").into(),
        ),
        ("empty redirect", "redirect_uri", "".into()),
    ];
    for (name, field, value) in mutations {
        for attempt in 0..3 {
            for (who, secret) in [("right", password.clone()), ("wrong", test_password())] {
                let mut body =
                    login_body(&deployment, &email, &secret, &token(), &challenge, &token());
                body[field] = value.clone();
                let answer = post_login(&gateway.base_url, &body).await;
                assert_eq!(
                    answer.status, 400,
                    "{name} ({who} password, attempt {attempt}): never the person's 401, \
                     never a 429: {}",
                    answer.body
                );
                assert_eq!(answer.body["error"], "invalid_request", "{name}");
            }
        }
    }
    let user = db
        .user()
        .find_unique(user_id)
        .run(&sys())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.lastLoginAt, None, "nothing signed in");

    let good = post_login(
        &gateway.base_url,
        &login_body(
            &deployment,
            &email,
            &password,
            &token(),
            &challenge,
            &token(),
        ),
    )
    .await;
    assert_eq!(
        good.status, 200,
        "the one token the address has was never spent: {}",
        good.body
    );
    gateway.kill();
}

/// `lastLoginAt` says a person signed in, so it is stamped when a code has been
/// issued and not when the password was merely right: a correct password with a
/// scope the console may not hold is refused by the library after the password
/// check, and stamps nothing; the same login asking for a legal scope does.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn last_login_is_stamped_only_once_a_code_is_issued() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let password = test_password();
    let (user_id, email) = seed_account(&db, &unique_suffix(), Some(&password), true).await;
    let gateway = deployment.spawn().await;
    let (_, challenge) = pkce_pair();
    let last_login = || async {
        db.user()
            .find_unique(user_id.clone())
            .run(&sys())
            .await
            .unwrap()
            .unwrap()
            .lastLoginAt
    };

    let mut refused = login_body(
        &deployment,
        &email,
        &password,
        &token(),
        &challenge,
        &token(),
    );
    refused["scope"] = "openid profile offline_access".into();
    let answer = post_login(&gateway.base_url, &refused).await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert_eq!(
        last_login().await,
        None,
        "the password was right and no code was issued: that is not a sign-in"
    );

    let good = post_login(
        &gateway.base_url,
        &login_body(
            &deployment,
            &email,
            &password,
            &token(),
            &challenge,
            &token(),
        ),
    )
    .await;
    assert_eq!(good.status, 200, "{}", good.body);
    assert!(last_login().await.is_some(), "a code was issued");
    gateway.kill();
}

// --- the grant gate ---------------------------------------------------------

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_refresh_grant_is_unsupported_whatever_is_served() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::Both, 100).await;
    deployment.prepare().await;
    let (_, email) = seed_account(&db, &unique_suffix(), Some(TEST_PASSWORD.as_str()), true).await;
    let gateway = deployment.spawn().await;

    // Even with a "refresh token" that was never issued, and a real client id.
    let answer = post_token(
        &gateway.base_url,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", "00000000-0000-0000-0000-000000000000"),
            ("client_id", deployment.console_client_id.as_str()),
        ],
    )
    .await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert_eq!(
        answer.body["error"], "unsupported_grant_type",
        "{}",
        answer.body
    );

    // And a full exchange never yields one to try.
    let (code, verifier) = sign_in(
        &deployment,
        &gateway.base_url,
        &email,
        TEST_PASSWORD.as_str(),
    )
    .await;
    let token = post_token(&gateway.base_url, &exchange(&deployment, &code, &verifier)).await;
    assert!(token.status.is_success(), "{}", token.body);
    assert!(
        token
            .body
            .get("refresh_token")
            .is_none_or(serde_json::Value::is_null)
    );
    gateway.kill();
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn client_credentials_is_refused_when_only_human_login_is_served() {
    let _guard = TEST_MUTEX.lock().await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let gateway = deployment.spawn().await;

    let answer = post_token(
        &gateway.base_url,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", "appc_0123456789abcdef"),
        ],
    )
    .await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert_eq!(
        answer.body["error"], "unsupported_grant_type",
        "{}",
        answer.body
    );

    // Discovery says the same, and points nowhere that is not a route.
    let discovery: serde_json::Value = reqwest::get(format!(
        "{}/.well-known/openid-configuration",
        gateway.base_url
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(
        discovery["grant_types_supported"],
        serde_json::json!(["authorization_code"])
    );
    assert_eq!(
        discovery["code_challenge_methods_supported"],
        serde_json::json!(["S256"])
    );
    assert!(
        discovery.get("authorization_endpoint").is_none(),
        "{discovery}"
    );
    let authorize = reqwest::get(format!("{}/authorize", gateway.base_url))
        .await
        .unwrap();
    assert!(
        matches!(authorize.status().as_u16(), 401 | 404),
        "GET /authorize is never mounted: {}",
        authorize.status()
    );
    let listed: serde_json::Value =
        reqwest::get(format!("{}/.well-known/vsms-auth", gateway.base_url))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    assert_eq!(
        listed["methods"],
        serde_json::json!(["keypair", "human_login"]),
        "{listed}"
    );
    gateway.kill();
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn authorization_code_is_refused_and_login_is_not_a_route_when_only_machine_tokens_is_served()
{
    let _guard = TEST_MUTEX.lock().await;
    let deployment = Deployment::new(Serves::Machines, 100).await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    deployment.cli_ok("rotate-signing-key", &[]);
    let gateway = deployment.spawn().await;

    let answer = post_token(
        &gateway.base_url,
        &[
            ("grant_type", "authorization_code"),
            ("code", "x"),
            ("client_id", "console-whatever"),
        ],
    )
    .await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert_eq!(
        answer.body["error"], "unsupported_grant_type",
        "{}",
        answer.body
    );

    // `POST /login` is not a route here: it is answered exactly as a path that
    // never existed is, whatever that answer is on this deployment's doors.
    let (_, challenge) = pkce_pair();
    let body = login_body(
        &deployment,
        "x@example.test",
        &test_password(),
        &token(),
        &challenge,
        &token(),
    );
    let login = post_login(&gateway.base_url, &body).await;
    let unknown = reqwest::Client::new()
        .post(format!("{}/no-such-route-at-all", gateway.base_url))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(login.status, unknown.status(), "{}", login.body);
    assert!(
        login
            .body
            .get("error")
            .is_none_or(|e| e != "invalid_credentials"),
        "the sign-in handler answered: {}",
        login.body
    );

    let discovery: serde_json::Value = reqwest::get(format!(
        "{}/.well-known/openid-configuration",
        gateway.base_url
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(
        discovery["grant_types_supported"],
        serde_json::json!(["client_credentials"])
    );
    assert!(
        discovery.get("code_challenge_methods_supported").is_none(),
        "{discovery}"
    );
    gateway.kill();
}

// --- serve's boot check -----------------------------------------------------

async fn console_row(db: &Cratestack, client_id: &str) -> schema::OauthClient {
    db.oauth_client()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::oauth_client::clientId().eq(client_id.to_owned()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("reading the console client")
        .into_iter()
        .next()
        .expect("the console client exists")
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn serve_refuses_a_console_client_that_is_missing_or_wrong_and_seeding_repairs_it() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.cli_ok("rotate-signing-key", &[]);

    // Missing.
    let refusal = deployment.serve_refusal();
    assert!(refusal.contains("is not registered"), "{refusal}");
    assert!(refusal.contains("seed-console-client"), "{refusal}");

    // Present, and carrying the refresh grant a pre-#446 row had.
    deployment.cli_ok("seed-console-client", &[]);
    let row = console_row(&db, &deployment.console_client_id).await;
    db.oauth_client()
        .update(row.id.clone())
        .set(schema::UpdateOauthClientInput {
            grantTypes: Some(" authorization_code refresh_token ".to_owned()),
            ..Default::default()
        })
        .run(&sys())
        .await
        .expect("giving the console client the refresh grant");
    let refusal = deployment.serve_refusal();
    assert!(refusal.contains("refresh_token"), "{refusal}");
    assert!(refusal.contains("seed-console-client"), "{refusal}");

    // Present, and registered for another redirect URI.
    db.oauth_client()
        .update(row.id.clone())
        .set(schema::UpdateOauthClientInput {
            grantTypes: Some(" authorization_code ".to_owned()),
            redirectUris: Some(" https://elsewhere.example/cb ".to_owned()),
            ..Default::default()
        })
        .run(&sys())
        .await
        .expect("moving the console client's redirect URI");
    let refusal = deployment.serve_refusal();
    assert!(refusal.contains("redirect URIs"), "{refusal}");

    // And the command makes it right again, saying what was wrong; serve boots.
    let seeded = deployment.cli_ok("seed-console-client", &[]);
    assert!(
        seeded.contains("reconciled") && seeded.contains("redirect URIs"),
        "{seeded}"
    );
    let again = deployment.cli_ok("seed-console-client", &[]);
    assert!(again.contains("nothing to do"), "{again}");
    let repaired = console_row(&db, &deployment.console_client_id).await;
    assert_eq!(repaired.redirectUris, format!(" {REDIRECT_URI} "));
    assert_eq!(repaired.grantTypes, " authorization_code ");
    deployment.spawn().await.kill();
}

// --- the operator commands --------------------------------------------------

#[cfg(unix)]
/// **A row that belongs to a machine is never taken over.** `seed-console-client`
/// repairs a stale console row; a row with the console's id that holds a public
/// key and authenticates with `private_key_jwt` is a service's, and rewriting it
/// would cut that service off. The command refuses, naming the conflict, and
/// leaves the row exactly as it was; `serve` refuses too, without offering the
/// command that would refuse again.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn seeding_refuses_to_take_over_a_machine_clients_row() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.cli_ok("rotate-signing-key", &[]);

    let machine_jwks = r#"{"keys":[{"kty":"RSA","n":"AQAB","e":"AQAB","kid":"machine"}]}"#;
    db.oauth_client()
        .create(schema::CreateOauthClientInput {
            clientId: deployment.console_client_id.clone(),
            appClientId: None,
            tokenEndpointAuthMethod: schema::ClientAuthMethod::private_key_jwt,
            jwks: Some(machine_jwks.to_owned()),
            grantTypes: " client_credentials ".to_owned(),
            scopes: " sms:send ".to_owned(),
            redirectUris: " ".to_owned(),
            requirePkce: false,
        })
        .run(&sys())
        .await
        .expect("registering a machine client under the console's id");
    let before = console_row(&db, &deployment.console_client_id).await;

    let seeded = deployment.cli("seed-console-client", &[]);
    assert!(
        !seeded.status.success(),
        "seeding must refuse a machine client's row: {}",
        stdout(&seeded)
    );
    let refusal = stderr(&seeded);
    assert!(refusal.contains("machine client"), "{refusal}");
    assert!(refusal.contains("public key"), "{refusal}");
    assert!(
        refusal.contains("private_key_jwt") || refusal.contains("auth method"),
        "{refusal}"
    );
    assert!(refusal.contains(&deployment.console_client_id), "{refusal}");

    let after = console_row(&db, &deployment.console_client_id).await;
    assert_eq!(
        after.tokenEndpointAuthMethod,
        before.tokenEndpointAuthMethod
    );
    assert_eq!(after.jwks, before.jwks);
    assert_eq!(after.grantTypes, before.grantTypes);
    assert_eq!(after.scopes, before.scopes);
    assert_eq!(after.updatedAt, before.updatedAt, "the row was not written");

    let boot = deployment.serve_refusal();
    assert!(boot.contains("machine client"), "{boot}");
    assert!(
        !boot.contains("run `sms-gateway seed-console-client`"),
        "serve must not offer the command that refuses this row: {boot}"
    );
}

fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn scratch_file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sms-login-flow-{}", unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn provision_user_writes_a_password_file_prints_nothing_secret_and_that_password_signs_in() {
    let _guard = TEST_MUTEX.lock().await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let suffix = unique_suffix();
    let email = format!("provisioned-{suffix}@example.test");
    let out = scratch_file("password");
    let out_str = out.display().to_string();

    let args = [
        "--email",
        email.as_str(),
        "--display-name",
        "Provisioned Person",
        "--role-key",
        "support",
        "--password",
        "--password-out",
        out_str.as_str(),
    ];
    let first = deployment.cli("provision-user", &args);
    assert!(first.status.success(), "{}", stderr(&first));
    let password = std::fs::read_to_string(&out).expect("the password file");
    assert_eq!(password.len(), 24, "a generated 24-character password");
    #[cfg(unix)]
    assert_eq!(mode_of(&out), 0o600);
    for (stream, text) in [("stdout", stdout(&first)), ("stderr", stderr(&first))] {
        assert!(
            !text.contains(&password),
            "{stream} printed the password: {text}"
        );
    }
    assert!(stdout(&first).contains(&out_str), "{}", stdout(&first));

    // The password it wrote signs in.
    let gateway = deployment.spawn().await;
    let (code, verifier) = sign_in(&deployment, &gateway.base_url, &email, &password).await;
    let token = post_token(&gateway.base_url, &exchange(&deployment, &code, &verifier)).await;
    assert!(token.status.is_success(), "{}", token.body);

    // A second run neither overwrites the file (O_EXCL) nor replaces the password.
    let again = deployment.cli("provision-user", &args);
    assert!(!again.status.success());
    assert!(
        stderr(&again).contains("already exists"),
        "{}",
        stderr(&again)
    );
    assert_eq!(std::fs::read_to_string(&out).unwrap(), password);

    gateway.kill();
}

/// `provision-user --password` for `email`, writing the password to `out`; with
/// `--reset` when `reset`.
fn provision_with_password<'a>(email: &'a str, out: &'a str, reset: bool) -> Vec<&'a str> {
    let mut args = vec![
        "--email",
        email,
        "--display-name",
        "Provisioned Person",
        "--role-key",
        "support",
        "--password",
        "--password-out",
        out,
    ];
    if reset {
        args.push("--reset");
    }
    args
}

/// An existing password is refused without `--reset`, and replaced with it: the new
/// password works and the old one stops.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_existing_password_is_refused_without_reset_and_replaced_with_it() {
    let _guard = TEST_MUTEX.lock().await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let email = format!("reset-{}@example.test", unique_suffix());
    let out = scratch_file("password");
    let out_str = out.display().to_string();
    deployment.cli_ok(
        "provision-user",
        &provision_with_password(&email, out_str.as_str(), false),
    );
    let password = std::fs::read_to_string(&out).expect("the password file");
    let gateway = deployment.spawn().await;

    // A fresh path without --reset: the existing password is refused, no file made.
    let other = scratch_file("second");
    let other_str = other.display().to_string();
    let refused = deployment.cli(
        "provision-user",
        &provision_with_password(&email, other_str.as_str(), false),
    );
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("--reset"), "{}", stderr(&refused));
    assert!(
        !other.exists(),
        "no file may be written for a refused password"
    );
    let (_, challenge) = pkce_pair();
    let still = post_login(
        &gateway.base_url,
        &login_body(
            &deployment,
            &email,
            &password,
            &token(),
            &challenge,
            &token(),
        ),
    )
    .await;
    assert_eq!(
        still.status, 200,
        "the refusal left the password working: {}",
        still.body
    );

    // --reset replaces it: the new password works and the old one stops.
    let reset = deployment.cli(
        "provision-user",
        &provision_with_password(&email, other_str.as_str(), true),
    );
    assert!(reset.status.success(), "{}", stderr(&reset));
    let new_password = std::fs::read_to_string(&other).unwrap();
    assert_ne!(new_password, password);
    // Neither the old password nor the one just written is on any stream of any of
    // the runs: only the path they were written to, and the account's id and email.
    for (run, output) in [("refused", &refused), ("reset", &reset)] {
        for (stream, text) in [("stdout", stdout(output)), ("stderr", stderr(output))] {
            for secret in [&password, &new_password] {
                assert!(
                    !text.contains(secret.as_str()),
                    "the {run} run's {stream} carries a password: {text}"
                );
            }
        }
    }
    assert!(stdout(&reset).contains(&other_str), "{}", stdout(&reset));
    let old = post_login(
        &gateway.base_url,
        &login_body(
            &deployment,
            &email,
            &password,
            &token(),
            &challenge,
            &token(),
        ),
    )
    .await;
    assert_eq!(old.status, 401, "the replaced password must stop working");
    let new = post_login(
        &gateway.base_url,
        &login_body(
            &deployment,
            &email,
            &new_password,
            &token(),
            &challenge,
            &token(),
        ),
    )
    .await;
    assert_eq!(new.status, 200, "{}", new.body);
    gateway.kill();
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_account_made_without_a_password_gets_one_later() {
    let _guard = TEST_MUTEX.lock().await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let email = format!("later-{}@example.test", unique_suffix());
    let made = deployment.cli_ok(
        "provision-user",
        &[
            "--email",
            email.as_str(),
            "--display-name",
            "Later",
            "--role-key",
            "support",
        ],
    );
    assert!(made.contains("no credential exists"), "{made}");

    let out = scratch_file("later");
    let out_str = out.display().to_string();
    let gave = deployment.cli_ok(
        "provision-user",
        &[
            "--email",
            email.as_str(),
            "--display-name",
            "Later",
            "--role-key",
            "support",
            "--password",
            "--password-out",
            out_str.as_str(),
        ],
    );
    assert!(gave.contains("password set"), "{gave}");
    assert_eq!(std::fs::read_to_string(&out).unwrap().len(), 24);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn bootstrap_with_a_password_file_seeds_the_console_client_and_the_owner_signs_in() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.cli_ok("rotate-signing-key", &[]);
    let email = format!("owner-{}@example.test", unique_suffix());
    let out = scratch_file("owner-password");
    let out_str = out.display().to_string();

    let ran = deployment.cli_ok(
        "bootstrap",
        &[
            "--skip-seed-dispatch",
            "--owner-email",
            email.as_str(),
            "--owner-display-name",
            "First Owner",
            "--owner-password-out",
            out_str.as_str(),
        ],
    );
    assert!(ran.contains("registered the console client"), "{ran}");
    let password = std::fs::read_to_string(&out).expect("the owner's password file");
    assert!(!ran.contains(&password), "bootstrap printed the password");
    #[cfg(unix)]
    assert_eq!(mode_of(&out), 0o600);

    let gateway = deployment.spawn().await;
    let (code, verifier) = sign_in(&deployment, &gateway.base_url, &email, &password).await;
    let token = post_token(&gateway.base_url, &exchange(&deployment, &code, &verifier)).await;
    assert!(token.status.is_success(), "{}", token.body);
    gateway.kill();
}

// --- both doors open --------------------------------------------------------

#[derive(serde::Serialize)]
struct AssertionClaims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    jti: String,
    exp: i64,
}

/// A `private_key_jwt` client assertion for `client_id`, signed with the PEM
/// `provision-client` wrote.
fn sign_client_assertion(client_id: &str, issuer: &str, private_key_pem: &str) -> String {
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
        .expect("an EncodingKey from the PEM the CLI wrote");
    let claims = AssertionClaims {
        iss: client_id,
        sub: client_id,
        aud: issuer,
        jti: unique_suffix(),
        exp: chrono::Utc::now().timestamp() + 60,
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        &key,
    )
    .expect("signing the client assertion")
}

/// `previewMessage` (a procedure that needs no scope, so only the credential
/// decides) with `token` as a bearer: the status and the body.
async fn preview_with(base_url: &str, token: &str) -> (u16, String) {
    let response = reqwest::Client::new()
        .post(format!("{base_url}/$procs/previewMessage"))
        .bearer_auth(token)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .body(r#"{"args":{"body":"hello"}}"#)
        .send()
        .await
        .expect("calling the API with a bearer token");
    (
        response.status().as_u16(),
        response.text().await.unwrap_or_default(),
    )
}

/// A machine client of a fresh app, provisioned the way an operator does it:
/// `(client id, private key PEM)`.
async fn provision_machine(db: &Cratestack, deployment: &Deployment) -> (String, String) {
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "login flow both doors app".to_owned(),
            slug: format!("login-flow-both-{}", unique_suffix()),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the app");
    let key_out = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("login-flow-both-{}.pem", deployment.port));
    let _ = std::fs::remove_file(&key_out);
    let provisioned = deployment.cli(
        "provision-client",
        &[
            "--app-id",
            app.id.as_str(),
            "--label",
            "both doors probe",
            "--key-out",
            key_out.to_str().expect("utf-8 path"),
            "--scope",
            "sms:send",
        ],
    );
    assert!(provisioned.status.success(), "{}", stderr(&provisioned));
    let client_id = stdout(&provisioned)
        .lines()
        .find_map(|line| line.strip_prefix("provisioned client: "))
        .expect("provision-client names the client")
        .trim()
        .to_owned();
    (
        client_id,
        std::fs::read_to_string(&key_out).expect("the key file"),
    )
}

/// A real access token for the machine client, from the running provider.
async fn mint_machine_token(base_url: &str, client_id: &str, pem: &str) -> String {
    let minted = reqwest::Client::new()
        .post(format!("{base_url}/token"))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            (
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
            ),
            (
                "client_assertion",
                sign_client_assertion(client_id, base_url, pem).as_str(),
            ),
            ("scope", "sms:send"),
        ])
        .send()
        .await
        .expect("POSTing to /token");
    assert!(minted.status().is_success(), "{}", minted.status());
    let minted: serde_json::Value = minted.json().await.expect("a token response");
    minted["access_token"]
        .as_str()
        .expect("an access token")
        .to_owned()
}

/// Starts `deployment`'s `serve` with the bearer door's refusals at `debug` in a
/// log file, and returns it with the log's path.
async fn spawn_logged(deployment: &Deployment, name: &str) -> (Gateway, PathBuf) {
    let log_path = scratch_file(name);
    let mut command = deployment.serve_command();
    command
        .env(
            "RUST_LOG",
            "sms_gateway=info,sms_api=info,cratestack=info,sms_api::bearer=debug",
        )
        .stdout(std::fs::File::create(&log_path).expect("the log file"))
        .stderr(Stdio::inherit());
    let mut child = command.spawn().expect("spawning sms-gateway serve");
    wait_until_ready(&deployment.base_url(), &mut child).await;
    (
        Gateway {
            child,
            base_url: deployment.base_url(),
        },
        log_path,
    )
}

/// How many refusals the door logged, and how many of them were `reason`.
fn refusals_in(log_path: &Path, reason: &str) -> (usize, usize) {
    let log = std::fs::read_to_string(log_path).expect("reading the serve log");
    (
        log.matches("a bearer token was refused").count(),
        log.matches(reason).count(),
    )
}

const REASON_ID_TOKEN: &str = "the token is an ID token, not an access token";
const REASON_HUMAN: &str =
    "the token belongs to a person, and this deployment accepts only a machine's";
const REASON_MACHINE: &str =
    "the token belongs to a machine client, and this deployment accepts only a person's";

/// The OP's own `TokenManager` for `deployment`, read from the database the way
/// `serve` reads it: tokens a test mints by hand are signed by the real key.
async fn op_signer(
    db: &Cratestack,
    deployment: &Deployment,
) -> std::sync::Arc<authkestra_engine::TokenManager> {
    sms_auth::op::load_signing_keys(db, &sys(), &deployment.base_url())
        .await
        .expect("loading the OP signing keys")
        .0
}

fn identity_of(user_id: &str) -> authkestra_engine::Identity {
    authkestra_engine::Identity {
        provider_id: "sms-console".to_owned(),
        external_id: user_id.to_owned(),
        email: None,
        username: None,
        attributes: std::collections::HashMap::new(),
    }
}

/// **A person's access token is accepted beside an open machine door, and the ID
/// token of the same sign-in is not.**
///
/// Both halves are proven in the one running process. The reason is read where
/// it is written, the door's own debug line (the response is the same coarse 401
/// for every refusal by design); and a machine token minted by the same provider
/// beside them is accepted too.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_persons_access_token_is_accepted_and_its_id_token_is_not() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::Both, 100).await;
    deployment.prepare().await;
    let password = test_password();
    let (_, email) = seed_account(&db, &unique_suffix(), Some(&password), true).await;

    let (machine_id, machine_pem) = provision_machine(&db, &deployment).await;
    let (gateway, log_path) = spawn_logged(&deployment, "both-doors-serve.log").await;
    let base_url = gateway.base_url.clone();

    let machine_token = mint_machine_token(&base_url, &machine_id, &machine_pem).await;
    let (code, verifier) = sign_in(&deployment, &base_url, &email, &password).await;
    let token = post_token(&base_url, &exchange(&deployment, &code, &verifier)).await;
    assert!(token.status.is_success(), "{} {}", token.status, token.body);
    let access_token = token.body["access_token"].as_str().expect("access token");
    let id_token = token.body["id_token"].as_str().expect("id_token");

    let (status, body) = preview_with(&base_url, &machine_token).await;
    assert_eq!(status, 200, "the machine door is open: {body}");
    let (status, body) = preview_with(&base_url, access_token).await;
    assert_eq!(status, 200, "a person's access token is accepted: {body}");
    let (status, body) = preview_with(&base_url, id_token).await;
    assert_eq!(status, 401, "a person's ID token has no door: {body}");
    assert!(
        body.contains("request could not be authenticated"),
        "the coarse refusal, whatever the reason: {body}"
    );
    gateway.kill();

    assert_eq!(
        refusals_in(&log_path, REASON_ID_TOKEN),
        (1, 1),
        "exactly the ID token was refused, as an ID token"
    );
}

/// `[keypair, human_login]`: only people are served, so a person's token has a
/// door (it did not before VSA-4b, when the keypair alone made the envelope
/// `Required`) and a machine's has none.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_only_people_served_a_persons_token_works_and_a_machines_does_not() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.prepare().await;
    let password = test_password();
    let (_, email) = seed_account(&db, &unique_suffix(), Some(&password), true).await;
    let (gateway, log_path) = spawn_logged(&deployment, "people-serve.log").await;
    let base_url = gateway.base_url.clone();

    let (code, verifier) = sign_in(&deployment, &base_url, &email, &password).await;
    let token = post_token(&base_url, &exchange(&deployment, &code, &verifier)).await;
    assert!(token.status.is_success(), "{}", token.body);
    let (status, body) =
        preview_with(&base_url, token.body["access_token"].as_str().unwrap()).await;
    assert_eq!(status, 200, "a person's token is a door: {body}");

    // A machine's token, signed by the real key: only people are served.
    let signer = op_signer(&db, &deployment).await;
    let machine = signer
        .issue_client_token_with_extra(
            "appc_handminted",
            900,
            Some("sms:send".to_owned()),
            Some("appc_handminted".to_owned()),
            std::collections::HashMap::new(),
        )
        .expect("minting a machine token");
    let (status, body) = preview_with(&base_url, &machine).await;
    assert_eq!(status, 401, "{body}");

    // A person's token addressed to something other than the console.
    let other = signer
        .issue_user_token_with_extra(
            identity_of("usr_handminted"),
            900,
            Some("openid profile".to_owned()),
            Some("some_other_client".to_owned()),
            std::collections::HashMap::new(),
        )
        .expect("minting a person token");
    let (status, body) = preview_with(&base_url, &other).await;
    assert_eq!(status, 401, "{body}");
    gateway.kill();

    assert_eq!(
        refusals_in(&log_path, REASON_MACHINE),
        (2, 1),
        "one MachineToken"
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.matches("the token is not addressed to its own client")
            .count()
            == 1,
        "the wrong audience is an Audience refusal:\n{log}"
    );
}

/// `[keypair, machine_tokens]`: no person realm, so a person's token (signed by
/// the real key, with a real account behind it) is a `HumanToken` refusal.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_only_machines_served_a_persons_token_is_refused_as_a_persons() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::Machines, 100).await;
    // `seed-console-client` refuses where `human_login` is not served: only the
    // provider's key is needed here.
    ensure_orange_cm_provider(&db).await;
    deployment.cli_ok("rotate-signing-key", &[]);
    let (user_id, _) = seed_account(&db, &unique_suffix(), None, true).await;
    let (gateway, log_path) = spawn_logged(&deployment, "machines-serve.log").await;

    let person = op_signer(&db, &deployment)
        .await
        .issue_user_token_with_extra(
            identity_of(&user_id),
            900,
            Some("openid profile".to_owned()),
            Some(deployment.console_client_id.clone()),
            std::collections::HashMap::new(),
        )
        .expect("minting a person token");
    let (status, body) = preview_with(&gateway.base_url, &person).await;
    assert_eq!(status, 401, "{body}");
    gateway.kill();

    assert_eq!(refusals_in(&log_path, REASON_HUMAN), (1, 1));
}

/// `[machine_tokens, human_login]`: the keypair is off, the envelope is `Off`,
/// a person's token works and a signed body is the layer's 415.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_tokens_alone_a_persons_token_works_and_a_signed_request_is_a_415() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let deployment = Deployment::new(Serves::TokensOnly, 100).await;
    deployment.prepare().await;
    let password = test_password();
    let (user_id, email) = seed_account(&db, &unique_suffix(), Some(&password), true).await;
    let gateway = deployment.spawn().await;

    let (code, verifier) = sign_in(&deployment, &gateway.base_url, &email, &password).await;
    let token = post_token(&gateway.base_url, &exchange(&deployment, &code, &verifier)).await;
    assert!(token.status.is_success(), "{}", token.body);
    let (status, body) = preview_with(
        &gateway.base_url,
        token.body["access_token"].as_str().unwrap(),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    let key = TestKey::console();
    sealed_fixtures::pair_console_key(&db, &user_id, &key, chrono::Duration::days(30)).await;
    let signed = sealed_fixtures::client(&gateway.base_url, &key)
        .request("POST", "/$procs/previewMessage")
        .json(&serde_json::json!({"args": {"body": "hello"}}))
        .send()
        .await;
    assert_eq!(signed.status, 415, "{}", signed.text());
    gateway.kill();
}

/// **Owner and admin act only from a paired console key.** The account signs in
/// with its password, and its access token is a 403 with the device text; paired
/// to a console key, the same account is let in. The owner is the one
/// `bootstrap --owner-password-out` writes.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_owner_or_admin_signed_in_with_a_password_is_refused_until_a_key_is_paired() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let deployment = Deployment::new(Serves::People, 100).await;
    deployment.cli_ok("rotate-signing-key", &[]);

    // The owner, exactly as bootstrap leaves them.
    let owner_email = format!("owner-{}@example.test", unique_suffix());
    let out = scratch_file("owner-password-4b");
    let out_str = out.display().to_string();
    deployment.cli_ok(
        "bootstrap",
        &[
            "--skip-seed-dispatch",
            "--owner-email",
            owner_email.as_str(),
            "--owner-display-name",
            "First Owner",
            "--owner-password-out",
            out_str.as_str(),
        ],
    );
    let owner_password = std::fs::read_to_string(&out).expect("the owner's password file");
    // An admin, seeded the way provision-user leaves one.
    let admin_password = test_password();
    let (admin_id, admin_email) = seed_account_in(
        &db,
        &unique_suffix(),
        Some("admin"),
        Some(&admin_password),
        true,
    )
    .await;
    let gateway = deployment.spawn().await;

    let device_text = sms_api::DEVICE_REQUIRED;
    for (who, email, password) in [
        ("owner", owner_email.as_str(), owner_password.as_str()),
        ("admin", admin_email.as_str(), admin_password.as_str()),
    ] {
        let (code, verifier) = sign_in(&deployment, &gateway.base_url, email, password).await;
        let token = post_token(&gateway.base_url, &exchange(&deployment, &code, &verifier)).await;
        assert!(token.status.is_success(), "{who}: {}", token.body);
        let access = token.body["access_token"].as_str().expect("access token");
        for path in ["/users", "/roles"] {
            let reply = reqwest::Client::new()
                .get(format!("{}{path}", gateway.base_url))
                .bearer_auth(access)
                .send()
                .await
                .expect("calling the API");
            let status = reply.status().as_u16();
            let body = reply.text().await.unwrap_or_default();
            assert_eq!(status, 403, "{who} {path}: {body}");
            assert!(body.contains(device_text), "{who} {path}: {body}");
        }
    }

    // Paired to a console key (a device), the admin acts with the role.
    let key = TestKey::console();
    sealed_fixtures::pair_console_key(&db, &admin_id, &key, chrono::Duration::days(30)).await;
    let users = sealed_fixtures::client(&gateway.base_url, &key)
        .request("GET", "/users")
        .send()
        .await;
    assert!(
        users.sealed && users.status == 200,
        "a paired key carries the admin role: {} {}",
        users.status,
        users.text()
    );
    gateway.kill();
}
