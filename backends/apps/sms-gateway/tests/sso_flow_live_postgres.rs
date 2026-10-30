//! The live gate for single sign-on (VSA-4c): a real federated sign-in through the
//! real HTTP surface of a genuinely spawned `sms-gateway serve`, configured the way
//! a deployment is (a YAML overlay with a `human_login.sso` block, the client secret
//! through the environment), against a scripted fake upstream `IdP`
//! (`sms-fake-oidc`) and a real Postgres - the same shape as
//! `login_flow_live_postgres.rs`, for the same reasons.
//!
//! The test plays the browser (following redirects by hand, since nothing listens
//! on the console's `redirect_uri`) and the console's server leg (the `/token`
//! exchange with the PKCE verifier). What it proves, end to end, per scenario:
//!
//! - a first sign-in provisions a least-privilege `User` + link, the access token
//!   is that person at the API (never as an `owner` or an `admin`, and never its
//!   ID token), and linking is by `(iss, sub)`, never email;
//! - every malformed token (wrong `nonce`/`aud`/`iss`, expired, unknown or missing
//!   `kid`, bad signature, `alg: none`, `alg: HS256`) is refused with the one
//!   generic `access_denied` redirect and creates nothing;
//! - the MFA rule, the one-shot `prompt=login` retry, and the group gate;
//! - inactive and deleted users, a replayed or expired `state`, and every request
//!   the console could make that the sign-in must not start;
//! - `lastLoginAt` only once a code is issued, the bucket in front of both routes,
//!   the reaper, and a role that does not exist stopping `serve` at boot;
//! - an unreachable `IdP` does not take password sign-in down, and no token, code
//!   or secret reaches the gateway's logs.
//!
//! Ignored by default like the other live suites:
//!
//! ```bash
//! cargo test -p sms-gateway --test sso_flow_live_postgres -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::TcpListener as StdTcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sha2::{Digest, Sha256};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, ClientAuthMethod, Cratestack, federated_link, provider as provider_filter,
    sso_transaction, user,
};
use sms_fake_oidc::{FakeOidc, KidMode, LoginSpec, NonceMode, SigAlg, SigningKey};
use sms_test_support::config::{self, TestConfig};

#[path = "../../../crates/sms-api/tests/support/sealed_fixtures.rs"]
mod sealed_fixtures;

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const ORANGE_PROVIDER_KEY: &str = "orange_cm";
const CONSOLE_REDIRECT_URI: &str = "http://127.0.0.1:1/callback";
const SSO_CLIENT_ID: &str = "vsms-sso-test";
const REQUIRED_GROUP: &str = "vsms-staff";

/// A value drawn at run time. `CodeQL`'s `rust/hard-coded-cryptographic-value`
/// flags a literal that reaches a secret or a password sink, even in a test, and a
/// random one is just as good here.
fn random_text(length: usize) -> String {
    sms_core::password::generate_password(length)
}

fn owner() -> CratestackContext {
    Principal {
        sub: "sso-flow-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> CratestackContext {
    sms_api::system_context("sso-flow-test-system")
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

// ---------------------------------------------------------------------------
// The deployment under test.
// ---------------------------------------------------------------------------

/// What the YAML says about single sign-on. The upstream `issuer` is the fake's.
#[derive(Clone)]
struct SsoYaml {
    issuer: String,
    accepted_amr: Vec<String>,
    accepted_acr: Vec<String>,
    jit_role: String,
    required_group: String,
    /// `human_login.sso.rate_limit.burst`: wide, unless a test is about the bucket.
    rate_burst: u32,
}

impl SsoYaml {
    fn amr_otp(issuer: &str) -> Self {
        Self {
            issuer: issuer.to_owned(),
            accepted_amr: vec!["otp".to_owned()],
            accepted_acr: vec![],
            jit_role: "support".to_owned(),
            required_group: REQUIRED_GROUP.to_owned(),
            rate_burst: 1000,
        }
    }

    fn block(&self) -> String {
        let list = |values: &[String]| format!("[{}]", values.join(", "));
        format!(
            "  sso:\n    issuer: {}\n    client_id: {SSO_CLIENT_ID}\n    \
             client_secret: ${{SMS_SSO_CLIENT_SECRET}}\n    scopes: [openid, email, profile]\n    \
             required_group: {}\n    groups_claim: groups\n    accepted_amr: {}\n    \
             accepted_acr: {}\n    jit_role: {}\n    jwks_min_refresh_secs: 1\n    \
             rate_limit:\n      burst: {}\n      refill_per_second: 0.001\n",
            self.issuer,
            self.required_group,
            list(&self.accepted_amr),
            list(&self.accepted_acr),
            self.jit_role,
            self.rate_burst,
        )
    }
}

/// Which parts of the sign-in a deployment is written with.
#[derive(Clone)]
enum Written {
    /// `human_login` with an `sso:` block inside it.
    Sso(SsoYaml),
    /// `human_login` and no `sso:`.
    PasswordOnly,
    /// Machine tokens alone: no `human_login:` block at all.
    MachinesOnly,
}

/// Lines the child process wrote, for the "nothing sensitive is logged"
/// assertions.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<String>>);

impl Logs {
    fn text(&self) -> String {
        self.0.lock().expect("log lock").clone()
    }
}

fn drain(stream: impl std::io::Read + Send + 'static, logs: Logs) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            let mut text = logs.0.lock().expect("log lock");
            text.push_str(&line);
            text.push('\n');
        }
    });
}

/// One spawned-process deployment: its own console client id (so a test that
/// breaks the row breaks nobody else's), its own configuration and key files.
struct Deployment {
    config: TestConfig,
    db_url: String,
    port: u16,
    keys: (std::path::PathBuf, std::path::PathBuf),
    console_client_id: String,
    /// The identity-provider client secret, drawn for this deployment and handed to
    /// the gateway through `SMS_SSO_CLIENT_SECRET` only.
    secret: String,
    written: Written,
}

impl Deployment {
    async fn new(written: &Written, secret: &str) -> Self {
        let port = free_port();
        let console_client_id = format!("console-{}", unique_suffix());
        let issuer = format!("http://127.0.0.1:{port}");
        let op = format!(
            "op:\n  issuer: {issuer}\n  token_rate_limit:\n    burst: 1000\n    \
             refill_per_second: 0.001\n  signing_key:\n    refresh_interval_secs: 1\n    \
             rotation_overlap_secs: 1800\n"
        );
        let human = format!(
            "human_login:\n  console_client:\n    client_id: {console_client_id}\n    \
             redirect_uris:\n      - {CONSOLE_REDIRECT_URI}\n  login_rate_limit:\n    per_email:\n      \
             burst: 1000\n      refill_per_second: 0.001\n    aggregate:\n      burst: 1000\n      \
             refill_per_second: 0.001\n"
        );
        let blocks = match written {
            Written::Sso(sso) => format!("{op}{human}{}", sso.block()),
            Written::PasswordOnly => format!("{op}{human}"),
            Written::MachinesOnly => format!("auth:\n  methods: [keypair, machine_tokens]\n{op}"),
        };
        let config = TestConfig::write(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            &format!("sso-flow-{port}"),
            &format!("{}\n{blocks}", config::orange_overlay("http://127.0.0.1:9")),
        );
        let keys = sealed_fixtures::write_server_key_files(
            &std::env::temp_dir().join(format!("sms-sso-flow-keys-{port}")),
        );
        Self {
            config,
            db_url: sms_test_support::database_url().await,
            port,
            keys,
            console_client_id,
            secret: secret.to_owned(),
            written: written.clone(),
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
            // Through the environment, the way a deployment hands a secret over -
            // and only to a deployment whose YAML reads it: a gateway that is
            // given the variable and does not reference it is refused (retired).
            .env_remove("SMS_SSO_CLIENT_SECRET")
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
            .env("NO_COLOR", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if matches!(self.written, Written::Sso(_)) {
            command.env("SMS_SSO_CLIENT_SECRET", &self.secret);
        }
        command
    }

    /// `sms-gateway <subcommand> --config ... --profile ... --database-url ...`,
    /// run to completion. The one-shot commands load as `Binary::Operator`, which is
    /// never handed the identity-provider secret.
    fn cli_ok(&self, subcommand: &str) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
            .arg(subcommand)
            .args(self.config.args())
            .arg("--database-url")
            .arg(&self.db_url)
            .env("NO_COLOR", "1")
            .env_remove("SMS_SSO_CLIENT_SECRET")
            .output()
            .unwrap_or_else(|error| panic!("running `sms-gateway {subcommand}`: {error}"));
        assert!(
            output.status.success(),
            "{subcommand} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Everything a served sign-in needs in the database, the way an operator
    /// leaves it: the orange provider row, a signing key and the console client.
    async fn prepare(&self) {
        ensure_orange_cm_provider(&db().await).await;
        self.cli_ok("rotate-signing-key");
        if !matches!(self.written, Written::MachinesOnly) {
            self.cli_ok("seed-console-client");
        }
    }

    /// A long-lived `serve`, ready, with its output captured.
    async fn spawn(&self) -> (Child, Logs) {
        let mut child = self
            .serve_command()
            .spawn()
            .expect("spawning sms-gateway serve");
        let logs = Logs::default();
        drain(child.stdout.take().expect("piped stdout"), logs.clone());
        drain(child.stderr.take().expect("piped stderr"), logs.clone());
        wait_until_ready(&self.base_url(), &mut child).await;
        (child, logs)
    }

    /// A `serve` that must refuse to start: its combined output, once it has
    /// exited.
    fn refusal(&self) -> String {
        let mut child = self
            .serve_command()
            .spawn()
            .expect("spawning sms-gateway serve");
        let logs = Logs::default();
        drain(child.stdout.take().expect("piped stdout"), logs.clone());
        drain(child.stderr.take().expect("piped stderr"), logs.clone());
        let deadline = std::time::Instant::now() + StdDuration::from_secs(60);
        loop {
            if let Some(status) = child.try_wait().expect("polling serve") {
                assert!(!status.success(), "serve must refuse, it exited {status:?}");
                std::thread::sleep(StdDuration::from_millis(300)); // let the drains finish
                return logs.text();
            }
            if std::time::Instant::now() > deadline {
                child.kill().ok();
                child.wait().ok();
                panic!("serve neither refused nor exited within 60s: it started");
            }
            std::thread::sleep(StdDuration::from_millis(100));
        }
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

/// A running single-sign-on deployment: database, fake upstream `IdP`, gateway.
struct Rig {
    db: Cratestack,
    fake: FakeOidc,
    deployment: Deployment,
    gateway: Child,
    logs: Logs,
    browser: reqwest::Client,
    suffix: String,
}

impl Rig {
    async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    async fn start_with(adjust: impl FnOnce(&mut SsoYaml)) -> Self {
        let secret = random_text(32);
        let fake = FakeOidc::start(SSO_CLIENT_ID, &secret).await;
        let mut yaml = SsoYaml::amr_otp(fake.issuer());
        adjust(&mut yaml);
        let deployment = Deployment::new(&Written::Sso(yaml), &secret).await;
        deployment.prepare().await;
        let (gateway, logs) = deployment.spawn().await;
        Self {
            db: db().await,
            fake,
            deployment,
            gateway,
            logs,
            browser: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("browser client"),
            suffix: unique_suffix(),
        }
    }

    fn base_url(&self) -> String {
        self.deployment.base_url()
    }

    /// A `LoginSpec` for a user unique to this rig.
    fn spec(&self) -> LoginSpec {
        LoginSpec {
            sub: format!("upstream-sub-{}", self.suffix),
            email: Some(self.email()),
            name: Some(format!("SSO User {}", self.suffix)),
            ..LoginSpec::default()
        }
    }

    fn email(&self) -> String {
        format!("sso-{}@example.test", self.suffix)
    }

    async fn users_with_email(&self, email: &str) -> Vec<schema::User> {
        self.db
            .user()
            .find_many()
            .where_expr(FilterExpr::from(user::email().eq(email.to_owned())))
            .run(&sys())
            .await
            .expect("listing users by email")
    }

    async fn link_for(&self, subject: &str) -> Option<schema::FederatedLink> {
        self.db
            .federated_link()
            .find_many()
            .where_expr(
                FilterExpr::from(federated_link::issuer().eq(self.fake.issuer().to_owned()))
                    .and(federated_link::subject().eq(subject.to_owned())),
            )
            .limit(1)
            .run(&sys())
            .await
            .expect("looking up the link")
            .into_iter()
            .next()
    }

    async fn open_transactions(&self) -> usize {
        self.db
            .sso_transaction()
            .find_many()
            .where_expr(
                FilterExpr::from(sso_transaction::isRetry().is_false())
                    .or(FilterExpr::from(sso_transaction::isRetry().is_true())),
            )
            .run(&sys())
            .await
            .expect("listing transactions")
            .len()
    }

    fn attempt(&self) -> Attempt {
        Attempt::new(&self.deployment.console_client_id)
    }

    async fn sign_in(&self, attempt: &Attempt) -> Outcome {
        walk(&self.browser, attempt.start_url(&self.base_url(), &[])).await
    }

    /// The console's server leg: exchange the code at the gateway's `/token`.
    async fn exchange(&self, attempt: &Attempt, code: &str) -> serde_json::Value {
        let response = self.post_token(attempt, code, &attempt.verifier).await;
        assert!(
            response.0.is_success(),
            "the console's /token exchange failed: {}: {}",
            response.0,
            response.1
        );
        response.1
    }

    async fn post_token(
        &self,
        attempt: &Attempt,
        code: &str,
        verifier: &str,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        let response = reqwest::Client::new()
            .post(format!("{}/token", self.base_url()))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", CONSOLE_REDIRECT_URI),
                ("client_id", attempt.client_id.as_str()),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .expect("POSTing /token");
        let status = response.status();
        (status, response.json().await.unwrap_or_default())
    }

    /// Signs in and takes the tokens: `(access, id, user)`.
    async fn tokens(&self, spec: LoginSpec) -> (String, String, schema::User) {
        let email = spec.email.clone().expect("the spec has an email");
        self.fake.queue_login(spec);
        let attempt = self.attempt();
        let Outcome::Code { code, .. } = self.sign_in(&attempt).await else {
            panic!("a valid sign-in must end at the console with a code");
        };
        let tokens = self.exchange(&attempt, &code).await;
        let user = self.users_with_email(&email).await.remove(0);
        (
            tokens["access_token"].as_str().expect("access").to_owned(),
            tokens["id_token"].as_str().expect("id").to_owned(),
            user,
        )
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.gateway.kill();
        let _ = self.gateway.wait();
    }
}

/// What the console's browser tab ends up holding after a sign-in.
#[derive(Debug)]
enum Outcome {
    /// Redirected to the console's callback with a code.
    Code { code: String, state: String },
    /// Redirected to the console's callback with an OAuth error.
    Error {
        error: String,
        state: Option<String>,
    },
    /// A plain, non-redirect response (the gateway had nowhere to send us).
    Plain,
}

/// The console's authorize request: a fresh PKCE pair, `state`, `nonce`.
struct Attempt {
    client_id: String,
    verifier: String,
    challenge: String,
    state: String,
    nonce: String,
}

impl Attempt {
    fn new(client_id: &str) -> Self {
        let mut raw = [0u8; 32];
        rand::Rng::fill(&mut rand::thread_rng(), &mut raw);
        let verifier = URL_SAFE_NO_PAD.encode(raw);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            client_id: client_id.to_owned(),
            verifier,
            challenge,
            state: random_text(16),
            nonce: random_text(16),
        }
    }

    /// The start URL, with `overrides` replacing a default parameter by name (and
    /// an empty value removing it).
    fn start_url(&self, gateway: &str, overrides: &[(&str, &str)]) -> String {
        let mut params: Vec<(&str, String)> = vec![
            ("client_id", self.client_id.clone()),
            ("redirect_uri", CONSOLE_REDIRECT_URI.to_owned()),
            ("response_type", "code".to_owned()),
            ("scope", "openid profile".to_owned()),
            ("state", self.state.clone()),
            ("nonce", self.nonce.clone()),
            ("code_challenge", self.challenge.clone()),
            ("code_challenge_method", "S256".to_owned()),
        ];
        for (name, value) in overrides {
            params.retain(|(existing, _)| existing != name);
            if !value.is_empty() {
                params.push((name, (*value).to_owned()));
            }
        }
        reqwest::Url::parse_with_params(&format!("{gateway}/login/sso/start"), params)
            .expect("a valid start url")
            .to_string()
    }
}

fn outcome_from_console_redirect(location: &str) -> Outcome {
    let url = reqwest::Url::parse(location).expect("a console redirect");
    let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
    if let Some(error) = params.get("error") {
        Outcome::Error {
            error: error.clone(),
            state: params.get("state").cloned(),
        }
    } else {
        Outcome::Code {
            code: params.get("code").cloned().expect("a code"),
            state: params.get("state").cloned().expect("a state"),
        }
    }
}

/// Follows redirects hop by hop until the console's callback (or a plain
/// response). Bounded, so a redirect loop fails the test rather than hanging it.
async fn walk(browser: &reqwest::Client, first: String) -> Outcome {
    let mut url = first;
    for _ in 0..8 {
        let response = browser.get(&url).send().await.expect("a browser hop");
        if response.status() == reqwest::StatusCode::FOUND {
            let location = response.headers()["location"]
                .to_str()
                .expect("a location")
                .to_owned();
            if location.starts_with(CONSOLE_REDIRECT_URI) {
                return outcome_from_console_redirect(&location);
            }
            url = location;
            continue;
        }
        return Outcome::Plain;
    }
    panic!("more than 8 redirect hops - a redirect loop");
}

fn assert_denied(outcome: &Outcome, attempt: &Attempt, why: &str) {
    match outcome {
        Outcome::Error { error, state } => {
            assert_eq!(error, "access_denied", "{why}");
            assert_eq!(
                state.as_deref(),
                Some(attempt.state.as_str()),
                "{why}: the console's own state must come back on the error redirect"
            );
        }
        other => panic!("{why}: expected an access_denied redirect, got {other:?}"),
    }
}

fn claim(token: &str, name: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()?
        .get(name)
        .cloned()
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

// ---------------------------------------------------------------------------
// The happy paths.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_first_sign_in_provisions_a_least_privilege_user_and_reaches_an_authenticated_api_call() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    rig.fake.queue_login(rig.spec());

    let attempt = rig.attempt();
    let Outcome::Code { code, state } = rig.sign_in(&attempt).await else {
        panic!("a valid first sign-in must end at the console with a code");
    };
    assert_eq!(state, attempt.state, "the console's state round-trips");

    // The console's /api/auth/callback step, unchanged.
    let tokens = rig.exchange(&attempt, &code).await;
    let access_token = tokens["access_token"].as_str().expect("access_token");
    let id_token = tokens["id_token"].as_str().expect("id_token");
    assert_eq!(
        claim(id_token, "nonce").and_then(|v| v.as_str().map(str::to_owned)),
        Some(attempt.nonce.clone()),
        "the console's nonce reaches the OP's id_token"
    );

    // The user was created, with the one fixed least-privilege role.
    let users = rig.users_with_email(&rig.email()).await;
    assert_eq!(users.len(), 1, "exactly one User");
    let user = &users[0];
    assert_eq!(
        user.roleKey, "support",
        "the JIT role is the configured one"
    );
    assert!(user.active);
    assert_eq!(
        user.subject, user.id,
        "User.subject stays vsms's own OP subject"
    );
    assert!(user.lastLoginAt.is_some(), "lastLoginAt is stamped");
    assert_eq!(
        claim(id_token, "sub").and_then(|v| v.as_str().map(str::to_owned)),
        Some(user.id.clone()),
        "the OP token's sub is the vsms User.id, not the upstream sub"
    );

    // The link keys on (issuer, upstream sub) and points at that user.
    let link = rig
        .link_for(&format!("upstream-sub-{}", rig.suffix))
        .await
        .expect("a FederatedLink");
    assert_eq!(link.userId, user.id);

    // A JIT user has no password: the break-glass path cannot be guessed into for
    // an account that never had one.
    let credentials = rig
        .db
        .user_credential()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::user_credential::userId().eq(user.id.clone()),
        ))
        .run(&sys())
        .await
        .expect("listing credentials");
    assert_eq!(credentials.len(), 0);

    // The access token is that person at the API.
    let (status, body) = preview_with(&rig.base_url(), access_token).await;
    assert_eq!(status, 200, "the SSO access token is a door: {body}");

    // Nothing sensitive reached the logs.
    let logs = rig.logs.text();
    for secret in [
        rig.deployment.secret.as_str(),
        access_token,
        id_token,
        code.as_str(),
    ] {
        assert!(
            !logs.contains(secret),
            "a secret leaked into the gateway log"
        );
    }
    assert!(!logs.contains("eyJ"), "a JWT leaked into the gateway log");
    assert_eq!(
        rig.open_transactions().await,
        0,
        "the transaction was consumed"
    );
}

/// **The token an SSO sign-in earns is the person's, with the person's cap.**
///
/// `support` (the JIT role) is accepted as that person. A person promoted to
/// `admin` afterwards gets the same token refused with `403 DEVICE_REQUIRED`:
/// VSA-4b's cap is on the role the row says, not on how the person signed in, so
/// no sign-in, a federated one included, turns an owner or an admin into a
/// browser session. (Promoted *before* the person's first API call: the bearer
/// door caches a standing for its TTL, and this test is about the cap, not the
/// cache.)
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_sso_access_token_is_the_person_and_a_promoted_admin_gets_403_device_required() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;

    let (support_token, _, _) = rig.tokens(rig.spec()).await;
    let (status, body) = preview_with(&rig.base_url(), &support_token).await;
    assert_eq!(status, 200, "a support person's SSO token: {body}");

    let promoted = LoginSpec {
        sub: format!("upstream-promoted-{}", rig.suffix),
        email: Some(format!("promoted-{}@example.test", rig.suffix)),
        ..rig.spec()
    };
    let (admin_token, _, person) = rig.tokens(promoted).await;
    rig.db
        .user()
        .update(person.id.clone())
        .set(schema::UpdateUserInput {
            roleKey: Some("admin".to_owned()),
            ..Default::default()
        })
        .if_match(person.version)
        .run(&owner())
        .await
        .expect("promoting the person to admin");
    let (status, body) = preview_with(&rig.base_url(), &admin_token).await;
    assert_eq!(status, 403, "an admin's SSO token is capped: {body}");
    assert!(body.contains(sms_api::DEVICE_REQUIRED), "{body}");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_id_token_from_an_sso_sign_in_is_refused_at_the_api() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let (access, id_token, _) = rig.tokens(rig.spec()).await;
    let (status, body) = preview_with(&rig.base_url(), &access).await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = preview_with(&rig.base_url(), &id_token).await;
    assert_eq!(status, 401, "an ID token has no door: {body}");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_second_sign_in_reuses_the_link_and_never_re_reads_the_email() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;

    rig.fake.queue_login(rig.spec());
    assert!(matches!(
        rig.sign_in(&rig.attempt()).await,
        Outcome::Code { .. }
    ));
    let original = rig.users_with_email(&rig.email()).await.remove(0);

    // Same upstream sub, but the IdP now says a different email and name.
    rig.fake.queue_login(LoginSpec {
        email: Some(format!("renamed-{}@example.test", rig.suffix)),
        name: Some("Somebody Else".to_owned()),
        ..rig.spec()
    });
    let second = rig.attempt();
    let Outcome::Code { code, .. } = rig.sign_in(&second).await else {
        panic!("the second sign-in must succeed");
    };
    let tokens = rig.exchange(&second, &code).await;
    assert_eq!(
        claim(tokens["id_token"].as_str().unwrap(), "sub")
            .and_then(|v| v.as_str().map(str::to_owned)),
        Some(original.id.clone()),
        "the same vsms User signs in again"
    );

    assert_eq!(rig.users_with_email(&rig.email()).await.len(), 1);
    assert!(
        rig.users_with_email(&format!("renamed-{}@example.test", rig.suffix))
            .await
            .is_empty(),
        "no second User - the link is by (iss, sub), and email is display data"
    );
    let after = rig.users_with_email(&rig.email()).await.remove(0);
    assert_eq!(
        after.displayName, original.displayName,
        "claims are not re-synced"
    );
    assert!(after.lastLoginAt >= original.lastLoginAt);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_existing_local_account_is_never_linked_by_email() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;

    // A password account that already owns this email.
    let local = rig
        .db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("pending-{}", rig.suffix),
            email: rig.email(),
            displayName: "Local Owner".to_owned(),
            roleKey: "owner".to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding a local owner");

    rig.fake.queue_login(rig.spec());
    let attempt = rig.attempt();
    let outcome = rig.sign_in(&attempt).await;
    assert_denied(
        &outcome,
        &attempt,
        "an upstream identity claiming a local account's email",
    );
    assert!(
        rig.link_for(&format!("upstream-sub-{}", rig.suffix))
            .await
            .is_none(),
        "no link may be created onto the local account"
    );
    let still = rig.users_with_email(&rig.email()).await;
    assert_eq!(still.len(), 1);
    assert_eq!(still[0].id, local.id);
    assert_eq!(still[0].roleKey, "owner", "and its role is untouched");
}

/// An address the identity provider has not confirmed, or one at `.invalid` (the
/// domain console pairing gives its accounts), creates nothing.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_unverified_or_reserved_email_is_refused_and_creates_nothing() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let cases = [
        (
            "no email_verified claim",
            LoginSpec {
                email_verified: None,
                ..rig.spec()
            },
            rig.email(),
            "email_unverified",
        ),
        (
            "email_verified false",
            LoginSpec {
                email_verified: Some(false),
                ..rig.spec()
            },
            rig.email(),
            "email_unverified",
        ),
        (
            "an .invalid address",
            LoginSpec {
                email: Some(format!("c{}@vaam.invalid", rig.suffix)),
                ..rig.spec()
            },
            format!("c{}@vaam.invalid", rig.suffix),
            "email_reserved",
        ),
    ];
    for (why, spec, email, reason) in cases {
        rig.fake.queue_login(spec);
        let attempt = rig.attempt();
        assert_denied(&rig.sign_in(&attempt).await, &attempt, why);
        assert!(rig.users_with_email(&email).await.is_empty(), "{why}");
        assert!(
            rig.logs.text().contains(reason),
            "{why}: {reason} not logged"
        );
    }
    assert!(
        rig.link_for(&format!("upstream-sub-{}", rig.suffix))
            .await
            .is_none()
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_jit_role_is_the_configured_one_and_never_taken_from_a_claim() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start_with(|yaml| yaml.jit_role = "auditor".to_owned()).await;
    let mut spec = rig.spec();
    // Every claim an IdP could use to try to pick a role.
    for claim_name in ["role", "roles", "vsms_role", "realm_access"] {
        spec.extra
            .insert(claim_name.to_owned(), serde_json::json!(["owner", "admin"]));
    }
    spec.groups = Some(vec![
        REQUIRED_GROUP.to_owned(),
        "owner".to_owned(),
        "admin".to_owned(),
    ]);
    rig.fake.queue_login(spec);

    assert!(matches!(
        rig.sign_in(&rig.attempt()).await,
        Outcome::Code { .. }
    ));
    let user = rig.users_with_email(&rig.email()).await.remove(0);
    assert_eq!(
        user.roleKey, "auditor",
        "the configured role, whatever the token says"
    );
}

/// A person who signed in through the identity provider has no password, so the
/// break-glass `POST /login` is a `401` for them, the same one as for no account.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_sso_provisioned_user_has_no_password_and_login_is_401() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    rig.tokens(rig.spec()).await;
    let attempt = rig.attempt();
    let login = reqwest::Client::new()
        .post(format!("{}/login", rig.base_url()))
        .json(&serde_json::json!({
            "email": rig.email(),
            "password": random_text(24),
            "client_id": attempt.client_id,
            "redirect_uri": CONSOLE_REDIRECT_URI,
            "response_type": "code",
            "scope": "openid profile",
            "state": attempt.state,
            "code_challenge": attempt.challenge,
            "code_challenge_method": "S256",
            "nonce": attempt.nonce,
        }))
        .send()
        .await
        .expect("POST /login");
    assert_eq!(login.status(), reqwest::StatusCode::UNAUTHORIZED);
}

/// The code an SSO sign-in earns is the OP's own: redeemed once, at `/token`, by
/// whoever holds the PKCE verifier.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_code_from_sso_redeems_once_at_token_with_pkce() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;

    // A wrong verifier is refused.
    rig.fake.queue_login(rig.spec());
    let wrong = rig.attempt();
    let Outcome::Code { code, .. } = rig.sign_in(&wrong).await else {
        panic!("a code");
    };
    let (status, _) = rig.post_token(&wrong, &code, &random_text(43)).await;
    assert!(!status.is_success(), "a wrong PKCE verifier: {status}");

    // The right one redeems, once.
    rig.fake.queue_login(rig.spec());
    let right = rig.attempt();
    let Outcome::Code { code, .. } = rig.sign_in(&right).await else {
        panic!("a code");
    };
    let (status, body) = rig.post_token(&right, &code, &right.verifier).await;
    assert!(status.is_success(), "{status} {body}");
    let (status, _) = rig.post_token(&right, &code, &right.verifier).await;
    assert!(!status.is_success(), "a code is single use: {status}");
}

// ---------------------------------------------------------------------------
// Refusals.
// ---------------------------------------------------------------------------

/// Every way an `id_token` can be wrong that the broker must refuse.
fn malformed_token_cases(rig: &Rig) -> Vec<(&'static str, LoginSpec)> {
    vec![
        (
            "wrong nonce",
            LoginSpec {
                nonce: NonceMode::Fixed("not-mine".into()),
                ..rig.spec()
            },
        ),
        (
            "missing nonce",
            LoginSpec {
                nonce: NonceMode::Missing,
                ..rig.spec()
            },
        ),
        (
            "wrong audience",
            LoginSpec {
                aud: Some(vec!["someone-else".into()]),
                ..rig.spec()
            },
        ),
        (
            "wrong issuer",
            LoginSpec {
                iss: Some("https://evil.example/realms/staff".into()),
                ..rig.spec()
            },
        ),
        (
            "expired",
            LoginSpec {
                exp_in_secs: -3600,
                ..rig.spec()
            },
        ),
        (
            "unknown kid",
            LoginSpec {
                key: SigningKey::Rsa3,
                ..rig.spec()
            },
        ),
        (
            "missing kid",
            LoginSpec {
                kid: KidMode::Missing,
                ..rig.spec()
            },
        ),
        (
            "signed by another key under a known kid",
            LoginSpec {
                key: SigningKey::Rsa3,
                kid: KidMode::Override(SigningKey::Rsa1.kid().to_owned()),
                ..rig.spec()
            },
        ),
        (
            "alg none",
            LoginSpec {
                alg: SigAlg::None,
                ..rig.spec()
            },
        ),
        (
            "alg HS256",
            LoginSpec {
                alg: SigAlg::Hs256,
                ..rig.spec()
            },
        ),
    ]
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn every_malformed_id_token_is_refused_generically_and_creates_nothing() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    // Prime the key cache so "unknown kid" means unknown, not first fetch.
    rig.fake.queue_login(LoginSpec {
        sub: format!("primer-{}", rig.suffix),
        email: Some(format!("primer-{}@example.test", rig.suffix)),
        ..LoginSpec::default()
    });
    assert!(matches!(
        rig.sign_in(&rig.attempt()).await,
        Outcome::Code { .. }
    ));

    let cases = malformed_token_cases(&rig);
    for (why, spec) in cases {
        let before = rig.fake.authorize_requests().len();
        rig.fake.queue_login(spec);
        let attempt = rig.attempt();
        let outcome = rig.sign_in(&attempt).await;
        assert_denied(&outcome, &attempt, why);
        assert_eq!(
            rig.fake.authorize_requests().len(),
            before + 1,
            "{why}: refused immediately - no prompt=login retry for anything but the MFA claim"
        );
        assert!(
            rig.users_with_email(&rig.email()).await.is_empty(),
            "{why}: a User was created"
        );
        assert!(
            rig.link_for(&format!("upstream-sub-{}", rig.suffix))
                .await
                .is_none(),
            "{why}: a link was created"
        );
    }
    let logs = rig.logs.text();
    for reason in [
        "nonce_mismatch",
        "audience_mismatch",
        "issuer_mismatch",
        "expired",
        "unknown_key_id",
        "missing_key_id",
        "signature_invalid",
        "algorithm_not_allowed",
    ] {
        assert!(
            logs.contains(reason),
            "the precise reason {reason:?} must reach the log"
        );
    }
    assert!(!logs.contains("eyJ"), "a JWT leaked into the gateway log");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_rotated_signing_key_is_picked_up_after_one_refetch() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    rig.fake.queue_login(rig.spec());
    assert!(matches!(
        rig.sign_in(&rig.attempt()).await,
        Outcome::Code { .. }
    ));

    // The refetch floor is one second (`jwks_min_refresh_secs: 1`, the smallest
    // the configuration allows): wait it out.
    tokio::time::sleep(StdDuration::from_millis(1300)).await;
    rig.fake.publish_key(SigningKey::Rsa2);
    rig.fake.queue_login(LoginSpec {
        key: SigningKey::Rsa2,
        ..rig.spec()
    });
    assert!(matches!(
        rig.sign_in(&rig.attempt()).await,
        Outcome::Code { .. }
    ));
}

// ---------------------------------------------------------------------------
// The MFA rule, the one-shot retry, the group gate.
// ---------------------------------------------------------------------------

fn pwd_only(spec: LoginSpec) -> LoginSpec {
    LoginSpec {
        acr: Some("1".to_owned()),
        amr: Some(vec!["pwd".to_owned()]),
        ..spec
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_first_token_without_mfa_is_retried_once_with_prompt_login_and_then_accepted() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    // (a) the first token: acr "1", amr [pwd] -> not refused, sent back upstream.
    rig.fake.queue_login(pwd_only(rig.spec()));
    // (b) the retry's token carries otp.
    rig.fake.queue_login(rig.spec());

    let attempt = rig.attempt();
    let Outcome::Code { code, state } = rig.sign_in(&attempt).await else {
        panic!("the retry's otp token must sign the user in");
    };
    assert_eq!(state, attempt.state, "the console never sees the retry");
    rig.exchange(&attempt, &code).await;

    let log = rig.fake.authorize_requests();
    assert_eq!(log.len(), 2, "exactly one retry");
    assert!(
        !log[0].contains_key("prompt"),
        "the first request carries no prompt"
    );
    assert_eq!(
        log[1]["prompt"], "login",
        "the retry forces a fresh authentication"
    );
    for field in ["state", "nonce", "code_challenge"] {
        assert_ne!(
            log[0][field], log[1][field],
            "the retry has a fresh upstream {field}"
        );
    }
    assert_eq!(rig.users_with_email(&rig.email()).await.len(), 1);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_retry_whose_token_still_lacks_mfa_is_refused_and_never_retried_again() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    rig.fake.queue_login(pwd_only(rig.spec()));
    rig.fake.queue_login(pwd_only(rig.spec()));

    let attempt = rig.attempt();
    let outcome = rig.sign_in(&attempt).await;
    assert_denied(&outcome, &attempt, "a retry that still lacks the MFA claim");
    assert_eq!(
        rig.fake.authorize_requests().len(),
        2,
        "at most one retry - no loop"
    );
    assert_eq!(rig.users_with_email(&rig.email()).await.len(), 0);
    assert!(rig.logs.text().contains("mfa_not_satisfied"));
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn any_failure_other_than_the_mfa_claim_is_refused_at_once_with_no_retry() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let cases: Vec<(&str, LoginSpec)> = vec![
        // The MFA claim is also missing, but the nonce is wrong: the MFA rule is
        // NOT the only reason, so there is no second chance.
        (
            "bad nonce and no mfa",
            pwd_only(LoginSpec {
                nonce: NonceMode::Fixed("x".into()),
                ..rig.spec()
            }),
        ),
        (
            "wrong aud and no mfa",
            pwd_only(LoginSpec {
                aud: Some(vec!["other".into()]),
                ..rig.spec()
            }),
        ),
        (
            "missing group, otp present",
            LoginSpec {
                groups: Some(vec!["other".into()]),
                ..rig.spec()
            },
        ),
        (
            "no groups claim at all",
            LoginSpec {
                groups: None,
                ..rig.spec()
            },
        ),
        (
            "missing group AND no mfa",
            pwd_only(LoginSpec {
                groups: Some(vec![]),
                ..rig.spec()
            }),
        ),
    ];
    for (why, spec) in cases {
        let before = rig.fake.authorize_requests().len();
        rig.fake.queue_login(spec);
        let attempt = rig.attempt();
        assert_denied(&rig.sign_in(&attempt).await, &attempt, why);
        assert_eq!(
            rig.fake.authorize_requests().len(),
            before + 1,
            "{why}: no retry"
        );
    }
    assert_eq!(rig.users_with_email(&rig.email()).await.len(), 0);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_accepted_acr_satisfies_the_rule_when_only_acr_is_configured() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start_with(|yaml| {
        yaml.accepted_amr = vec![];
        yaml.accepted_acr = vec!["urn:vsms:mfa".to_owned()];
    })
    .await;
    // An otp amr is irrelevant when only acr is the rule.
    rig.fake.queue_login(LoginSpec {
        acr: Some("1".into()),
        ..rig.spec()
    });
    rig.fake.queue_login(LoginSpec {
        acr: Some("1".into()),
        ..rig.spec()
    });
    let refused = rig.attempt();
    assert_denied(
        &rig.sign_in(&refused).await,
        &refused,
        "acr 1 is not the accepted acr",
    );

    rig.fake.queue_login(LoginSpec {
        acr: Some("urn:vsms:mfa".into()),
        amr: None,
        ..rig.spec()
    });
    assert!(matches!(
        rig.sign_in(&rig.attempt()).await,
        Outcome::Code { .. }
    ));
}

// ---------------------------------------------------------------------------
// Accounts and transactions.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_deactivated_or_deleted_linked_user_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    rig.fake.queue_login(rig.spec());
    assert!(matches!(
        rig.sign_in(&rig.attempt()).await,
        Outcome::Code { .. }
    ));
    let created = rig.users_with_email(&rig.email()).await.remove(0);

    // Deactivated.
    let updated = rig
        .db
        .user()
        .update(created.id.clone())
        .set(schema::UpdateUserInput {
            active: Some(false),
            ..Default::default()
        })
        .if_match(created.version)
        .run(&owner())
        .await
        .expect("deactivating the user");
    rig.fake.queue_login(rig.spec());
    let attempt = rig.attempt();
    assert_denied(&rig.sign_in(&attempt).await, &attempt, "an inactive user");
    assert!(rig.logs.text().contains("user_inactive"));

    // Reactivated, then soft-deleted.
    let reactivated = rig
        .db
        .user()
        .update(created.id.clone())
        .set(schema::UpdateUserInput {
            active: Some(true),
            ..Default::default()
        })
        .if_match(updated.version)
        .run(&owner())
        .await
        .expect("reactivating the user");
    rig.fake.queue_login(rig.spec());
    assert!(matches!(
        rig.sign_in(&rig.attempt()).await,
        Outcome::Code { .. }
    ));
    let current = rig.users_with_email(&rig.email()).await.remove(0);
    assert!(current.version >= reactivated.version);
    rig.db
        .user()
        .delete(created.id.clone())
        .if_match(current.version)
        .run(&owner())
        .await
        .expect("soft-deleting the user");
    rig.fake.queue_login(rig.spec());
    let attempt = rig.attempt();
    assert_denied(
        &rig.sign_in(&attempt).await,
        &attempt,
        "a soft-deleted user",
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_replayed_callback_and_an_unknown_state_are_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    rig.fake.queue_login(rig.spec());
    let attempt = rig.attempt();

    // Walk by hand to the gateway's callback URL, without taking it.
    let start = rig
        .browser
        .get(attempt.start_url(&rig.base_url(), &[]))
        .send()
        .await
        .expect("start");
    assert_eq!(start.status(), reqwest::StatusCode::FOUND);
    let upstream = start.headers()["location"].to_str().unwrap().to_owned();
    let authorize = rig
        .browser
        .get(&upstream)
        .send()
        .await
        .expect("upstream authorize");
    let callback = authorize.headers()["location"].to_str().unwrap().to_owned();

    let first = rig
        .browser
        .get(&callback)
        .send()
        .await
        .expect("callback #1");
    assert_eq!(first.status(), reqwest::StatusCode::FOUND);
    assert!(
        first.headers()["location"]
            .to_str()
            .unwrap()
            .starts_with(CONSOLE_REDIRECT_URI)
    );

    let replay = rig
        .browser
        .get(&callback)
        .send()
        .await
        .expect("callback #2");
    assert_eq!(
        replay.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "a replayed state finds no transaction and has nowhere safe to redirect"
    );
    assert!(replay.headers().get("location").is_none());

    let forged = rig
        .browser
        .get(format!(
            "{}/login/sso/callback?code=anything&state=never-issued",
            rig.base_url()
        ))
        .send()
        .await
        .expect("forged callback");
    assert_eq!(forged.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_expired_transaction_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let state = format!("expired-state-{}", rig.suffix);
    seed_transaction(&rig.db, &state, &rig.deployment.console_client_id, -1).await;
    let response = rig
        .browser
        .get(format!(
            "{}/login/sso/callback?code=anything&state={state}",
            rig.base_url()
        ))
        .send()
        .await
        .expect("callback");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        rig.open_transactions().await,
        0,
        "and the expired row is consumed"
    );
}

/// A stored transaction for `state` that expires `minutes` from now (negative:
/// already).
async fn seed_transaction(db: &Cratestack, state: &str, console_client_id: &str, minutes: i64) {
    seed_transaction_to(db, state, console_client_id, CONSOLE_REDIRECT_URI, minutes).await;
}

/// [`seed_transaction`], for a console request that names `redirect_uri`.
async fn seed_transaction_to(
    db: &Cratestack,
    state: &str,
    console_client_id: &str,
    redirect_uri: &str,
    minutes: i64,
) {
    db.sso_transaction()
        .create(schema::CreateSsoTransactionInput {
            stateHash: sms_auth::sso::store::state_hash(state),
            nonce: "n".to_owned(),
            codeVerifier: "v".to_owned(),
            consoleRequest: serde_json::json!({
                "client_id": console_client_id,
                "redirect_uri": redirect_uri,
                "response_type": "code",
                "scope": "openid profile",
                "state": "s",
                "code_challenge": "c".repeat(43),
                "code_challenge_method": "S256",
                "nonce": "n",
            })
            .to_string(),
            isRetry: false,
            expiresAt: chrono::Utc::now() + chrono::Duration::minutes(minutes),
        })
        .run(&sys())
        .await
        .expect("seeding a transaction");
}

// ---------------------------------------------------------------------------
// What `start` will not begin.
// ---------------------------------------------------------------------------

/// A start request, answered: its status and whether it carried a `Location`.
async fn start_answer(rig: &Rig, attempt: &Attempt, overrides: &[(&str, &str)]) -> (u16, bool) {
    let response = rig
        .browser
        .get(attempt.start_url(&rig.base_url(), overrides))
        .send()
        .await
        .expect("start");
    (
        response.status().as_u16(),
        response.headers().get("location").is_some(),
    )
}

/// Nothing was stored and the identity provider was never contacted.
async fn assert_nothing_began(rig: &Rig, transactions_before: usize, why: &str) {
    assert!(
        rig.fake.authorize_requests().is_empty(),
        "{why}: the upstream IdP was contacted"
    );
    assert_eq!(
        rig.open_transactions().await,
        transactions_before,
        "{why}: a transaction row was written"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn start_refuses_a_client_id_that_is_not_the_console_and_stores_nothing() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    // A second client the OP does know, public, with the code grant and the same
    // redirect: the library alone would let it through. Only the console is
    // served by this route.
    let other = format!("other-{}", rig.suffix);
    rig.db
        .oauth_client()
        .create(schema::CreateOauthClientInput {
            clientId: other.clone(),
            appClientId: None,
            tokenEndpointAuthMethod: ClientAuthMethod::none,
            jwks: None,
            grantTypes: " authorization_code ".to_owned(),
            scopes: " openid profile ".to_owned(),
            redirectUris: format!(" {CONSOLE_REDIRECT_URI} "),
            requirePkce: true,
        })
        .run(&sys())
        .await
        .expect("registering a second client");
    let before = rig.open_transactions().await;
    let attempt = rig.attempt();

    for (why, client) in [
        (
            "a registered client that is not the console",
            other.as_str(),
        ),
        ("an unknown client", "nobody-at-all"),
    ] {
        let (status, redirected) = start_answer(&rig, &attempt, &[("client_id", client)]).await;
        assert_eq!(status, 400, "{why}");
        assert!(!redirected, "{why}: never redirected");
        assert_nothing_began(&rig, before, why).await;
    }
}

/// `POST /login`'s cheap checks, made again here (`console_authorize.rs`), plus the
/// parameters a query string can leave out.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn start_runs_the_same_cheap_checks_as_login() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let before = rig.open_transactions().await;
    let attempt = rig.attempt();
    let oversized = "x".repeat(1025);
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "a challenge method other than S256",
            vec![("code_challenge_method", "plain")],
        ),
        ("a short challenge", vec![("code_challenge", "short")]),
        (
            "a challenge that is not base64url",
            vec![(
                "code_challenge",
                "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!",
            )],
        ),
        (
            "an unregistered redirect_uri",
            vec![("redirect_uri", "https://evil.example/cb")],
        ),
        ("no nonce", vec![("nonce", "")]),
        ("no state", vec![("state", "")]),
        ("no scope", vec![("scope", "")]),
        ("no code challenge", vec![("code_challenge", "")]),
        (
            "an oversized parameter",
            vec![("state", oversized.as_str())],
        ),
    ];
    for (why, overrides) in cases {
        let (status, redirected) = start_answer(&rig, &attempt, &overrides).await;
        assert_eq!(status, 400, "{why}");
        assert!(!redirected, "{why}: never redirected");
        assert_nothing_began(&rig, before, why).await;
    }
}

/// The library decides what it would issue: a request that passes every cheap check
/// and that it would still refuse is refused here, before a row or a redirect.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_console_request_the_op_would_refuse_is_refused_at_start_before_anything_is_stored() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let before = rig.open_transactions().await;
    let attempt = rig.attempt();
    for (why, overrides) in [
        (
            "an implicit-flow response type",
            [("response_type", "token")],
        ),
        (
            "a scope the console may not hold",
            [("scope", "openid vsms.admin")],
        ),
    ] {
        let (status, redirected) = start_answer(&rig, &attempt, &overrides).await;
        assert_eq!(status, 400, "{why}");
        assert!(!redirected, "{why}");
        assert_nothing_began(&rig, before, why).await;
    }
}

/// The library answers a request it refuses with a redirect carrying `error=`. The
/// first SSO broker forwarded that to the browser; neither route does now, and
/// the body says nothing of why.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_error_redirect_from_handle_authorize_is_a_400_never_forwarded() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let attempt = rig.attempt();
    let response = rig
        .browser
        .get(attempt.start_url(&rig.base_url(), &[("scope", "openid not-a-scope")]))
        .send()
        .await
        .expect("start");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(response.headers().get("location").is_none());
    let body = response.text().await.unwrap_or_default();
    assert!(!body.contains("error="), "{body}");
    assert!(!body.contains("invalid_scope"), "{body}");
    assert_eq!(rig.fake.authorize_requests().len(), 0);
}

// ---------------------------------------------------------------------------
// lastLoginAt, the bucket, the reaper.
// ---------------------------------------------------------------------------

async fn console_row(rig: &Rig) -> schema::OauthClient {
    rig.db
        .oauth_client()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::oauth_client::clientId().eq(rig.deployment.console_client_id.clone()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("reading the console client")
        .into_iter()
        .next()
        .expect("the console client exists")
}

async fn set_console_scopes(rig: &Rig, scopes: &str) {
    rig.db
        .oauth_client()
        .update(console_row(rig).await.id)
        .set(schema::UpdateOauthClientInput {
            scopes: Some(scopes.to_owned()),
            ..Default::default()
        })
        .run(&sys())
        .await
        .expect("changing the console client's scopes");
}

/// **A verified token whose authorization request the library then refuses signed
/// nobody in**, so it leaves no `lastLoginAt` behind. The console client loses a
/// scope between `start` and the callback; the callback answers `400`, the account
/// (created by the verified sign-in) has no stamp, and the next sign-in stamps it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn last_login_is_stamped_only_once_the_code_is_issued() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    rig.fake.queue_login(rig.spec());
    let attempt = rig.attempt();

    let start = rig
        .browser
        .get(attempt.start_url(&rig.base_url(), &[]))
        .send()
        .await
        .expect("start");
    assert_eq!(start.status(), reqwest::StatusCode::FOUND);
    let upstream = start.headers()["location"].to_str().unwrap().to_owned();
    let authorize = rig.browser.get(&upstream).send().await.expect("authorize");
    let callback = authorize.headers()["location"].to_str().unwrap().to_owned();

    set_console_scopes(&rig, " openid ").await;
    let refused = rig.browser.get(&callback).send().await.expect("callback");
    assert_eq!(refused.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(refused.headers().get("location").is_none());

    let created = rig.users_with_email(&rig.email()).await;
    assert_eq!(created.len(), 1, "the verified sign-in created the account");
    assert!(
        created[0].lastLoginAt.is_none(),
        "but nobody signed in, so nothing is stamped"
    );

    set_console_scopes(&rig, " openid profile ").await;
    rig.fake.queue_login(rig.spec());
    assert!(matches!(
        rig.sign_in(&rig.attempt()).await,
        Outcome::Code { .. }
    ));
    let signed_in = rig.users_with_email(&rig.email()).await.remove(0);
    assert!(
        signed_in.lastLoginAt.is_some(),
        "stamped once a code issued"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_backstop_bucket_answers_429_with_retry_after() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start_with(|yaml| yaml.rate_burst = 2).await;
    let attempt = rig.attempt();
    for _ in 0..2 {
        let response = rig
            .browser
            .get(attempt.start_url(&rig.base_url(), &[]))
            .send()
            .await
            .expect("start");
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
    }
    let throttled = rig
        .browser
        .get(attempt.start_url(&rig.base_url(), &[]))
        .send()
        .await
        .expect("start");
    assert_eq!(throttled.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    let retry_after: u64 = throttled.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .expect("Retry-After is whole seconds");
    assert!(retry_after >= 1);

    // One bucket for each route: the callback is not the start's to empty.
    let callback = rig
        .browser
        .get(format!(
            "{}/login/sso/callback?code=c&state=never-issued",
            rig.base_url()
        ))
        .send()
        .await
        .expect("callback");
    assert_eq!(callback.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn serve_reaps_expired_transactions() {
    let _guard = TEST_MUTEX.lock().await;
    let secret = random_text(32);
    let fake = FakeOidc::start(SSO_CLIENT_ID, &secret).await;
    let deployment = Deployment::new(&Written::Sso(SsoYaml::amr_otp(fake.issuer())), &secret).await;
    deployment.prepare().await;
    let db = db().await;
    let suffix = unique_suffix();
    let stale = [format!("stale-a-{suffix}"), format!("stale-b-{suffix}")];
    for state in &stale {
        seed_transaction(&db, state, &deployment.console_client_id, -10).await;
    }
    let live = format!("live-{suffix}");
    seed_transaction(&db, &live, &deployment.console_client_id, 10).await;
    let count = |db: Cratestack, state: String| async move {
        db.sso_transaction()
            .find_many()
            .where_expr(FilterExpr::from(
                sso_transaction::stateHash().eq(sms_auth::sso::store::state_hash(&state)),
            ))
            .run(&sys())
            .await
            .expect("reading a transaction")
            .len()
    };

    // The reaper's first tick is at boot.
    let (mut gateway, _logs) = deployment.spawn().await;
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(15);
    loop {
        let remaining =
            count(db.clone(), stale[0].clone()).await + count(db.clone(), stale[1].clone()).await;
        if remaining == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the expired transactions were never reaped"
        );
        tokio::time::sleep(StdDuration::from_millis(250)).await;
    }
    assert_eq!(
        count(db.clone(), live).await,
        1,
        "a live one is not touched"
    );
    let _ = gateway.kill();
    let _ = gateway.wait();
}

// ---------------------------------------------------------------------------
// Configuration, discovery, resilience.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn serve_refuses_a_jit_role_with_no_row() {
    let _guard = TEST_MUTEX.lock().await;
    let secret = random_text(32);
    let fake = FakeOidc::start(SSO_CLIENT_ID, &secret).await;
    let mut yaml = SsoYaml::amr_otp(fake.issuer());
    yaml.jit_role = "no_such_role".to_owned();
    let deployment = Deployment::new(&Written::Sso(yaml), &secret).await;
    deployment.prepare().await;
    let output = deployment.refusal();
    assert!(output.contains("no_such_role"), "{output}");
    assert!(output.contains("names no Role row"), "{output}");
    assert!(!output.contains(&secret), "the secret was printed");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_configuration_that_cannot_enforce_its_rules_refuses_to_start() {
    let _guard = TEST_MUTEX.lock().await;
    let secret = random_text(32);
    let base = SsoYaml::amr_otp("https://idp.example.test/realms/staff");
    let cases: Vec<(&str, SsoYaml, &str)> = vec![
        (
            "both MFA lists empty",
            SsoYaml {
                accepted_amr: vec![],
                accepted_acr: vec![],
                ..base.clone()
            },
            "multi-factor rule is empty",
        ),
        (
            "an owner as the JIT role",
            SsoYaml {
                jit_role: "owner".to_owned(),
                ..base.clone()
            },
            "can never be a just-in-time role",
        ),
        (
            "a reserved role",
            SsoYaml {
                jit_role: "system".to_owned(),
                ..base.clone()
            },
            "can never be a just-in-time role",
        ),
        (
            "a cleartext remote issuer",
            SsoYaml {
                issuer: "http://idp.example.test/realms/staff".to_owned(),
                ..base.clone()
            },
            "https",
        ),
    ];
    for (why, yaml, expected) in cases {
        let deployment = Deployment::new(&Written::Sso(yaml), &secret).await;
        let output = deployment.refusal();
        assert!(
            output.contains(expected),
            "{why}: expected {expected:?} in:\n{output}"
        );
        assert!(!output.contains(&secret), "{why}: the secret was printed");
    }

    // A retired flag's variable, still set and not referenced, is refused naming the key.
    let deployment = Deployment::new(&Written::PasswordOnly, &secret).await;
    let mut command = deployment.serve_command();
    command.env("SMS_SSO_ISSUER", "https://idp.example.test/realms/staff");
    let output = command.output().expect("running serve");
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("human_login.sso.issuer"), "{text}");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn discovery_advertises_the_start_url_only_when_sso_is_served() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let document: serde_json::Value = reqwest::get(format!(
        "{}/.well-known/openid-configuration",
        rig.base_url()
    ))
    .await
    .expect("discovery")
    .json()
    .await
    .expect("json");
    assert_eq!(
        document["vsms_sso_login_endpoint"],
        format!("{}/login/sso/start", rig.base_url())
    );
    assert!(
        document["token_endpoint_auth_methods_supported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "private_key_jwt"),
        "the existing document is intact"
    );
}

/// `human_login` with no `sso:` block, and a deployment with no `human_login` at
/// all: neither mounts the routes (each is answered as a path that never existed),
/// and neither advertises them.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_human_login_off_or_no_sso_block_the_routes_are_404_and_discovery_has_no_member() {
    let _guard = TEST_MUTEX.lock().await;
    for (why, written) in [
        ("no sso block", Written::PasswordOnly),
        ("no human_login", Written::MachinesOnly),
    ] {
        let deployment = Deployment::new(&written, &random_text(32)).await;
        deployment.prepare().await;
        let (mut gateway, _logs) = deployment.spawn().await;
        let base = deployment.base_url();
        let document: serde_json::Value =
            reqwest::get(format!("{base}/.well-known/openid-configuration"))
                .await
                .expect("discovery")
                .json()
                .await
                .expect("json");
        assert!(
            document.get("vsms_sso_login_endpoint").is_none(),
            "{why}: no SSO, no advertisement"
        );
        // Not a route is answered exactly as a path that never existed - whatever
        // that is for this deployment's doors (a `404`, or the envelope's own `400`
        // where a signed request is required) - and never as the route would.
        let get = |path: String| {
            let client = reqwest::Client::new();
            async move {
                let answer = client.get(path).send().await.expect("GET");
                (answer.status(), answer.text().await.unwrap_or_default())
            }
        };
        let unknown = get(format!("{base}/login/sso/never-a-route")).await;
        for path in ["/login/sso/start", "/login/sso/callback"] {
            assert_eq!(get(format!("{base}{path}")).await, unknown, "{why} {path}");
        }
        let _ = gateway.kill();
        let _ = gateway.wait();
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_unreachable_idp_does_not_stop_the_gateway_or_password_sign_in() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    // Nothing listens on this port.
    let dead = format!("http://127.0.0.1:{}/realms/staff", free_port());
    let secret = random_text(32);
    let deployment = Deployment::new(&Written::Sso(SsoYaml::amr_otp(&dead)), &secret).await;
    deployment.prepare().await;
    let (mut gateway, _logs) = deployment.spawn().await;
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let attempt = Attempt::new(&deployment.console_client_id);
    let outcome = walk(&browser, attempt.start_url(&deployment.base_url(), &[])).await;
    match outcome {
        Outcome::Error { error, .. } => assert_eq!(error, "temporarily_unavailable"),
        other => panic!("expected a temporarily_unavailable redirect, got {other:?}"),
    }

    // Password sign-in is the break-glass path and must be untouched.
    let suffix = unique_suffix();
    let email = format!("breakglass-{suffix}@example.test");
    let user = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("pending-{suffix}"),
            email: email.clone(),
            displayName: "Break Glass".to_owned(),
            roleKey: "owner".to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding a password user");
    // Random per run, like the passwords `provision-user` generates: no literal
    // credential lives in the test source.
    let password = random_text(24);
    db.user_credential()
        .create(schema::CreateUserCredentialInput {
            userId: user.id,
            passwordHash: sms_core::password::hash_password(&password).expect("hash"),
        })
        .run(&sys())
        .await
        .expect("seeding a credential");
    let login = reqwest::Client::new()
        .post(format!("{}/login", deployment.base_url()))
        .json(&serde_json::json!({
            "email": email,
            "password": password,
            "client_id": deployment.console_client_id,
            "redirect_uri": CONSOLE_REDIRECT_URI,
            "response_type": "code",
            "scope": "openid profile",
            "state": attempt.state,
            "code_challenge": attempt.challenge,
            "code_challenge_method": "S256",
            "nonce": attempt.nonce,
        }))
        .send()
        .await
        .expect("POST /login");
    assert_eq!(
        login.status(),
        reqwest::StatusCode::OK,
        "password sign-in still works"
    );
    let _ = gateway.kill();
    let _ = gateway.wait();
}

/// An error redirect is still a redirect. A transaction written when a redirect URI
/// was registered must not send the browser there once the console's list no longer
/// has it; a URI that is still registered gets its `access_denied` as before.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_callback_redirects_only_to_a_uri_registered_now() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let client = &rig.deployment.console_client_id;

    let stale = format!("stale-redirect-{}", rig.suffix);
    seed_transaction_to(&rig.db, &stale, client, "https://old-console.example/cb", 5).await;
    let answer = rig
        .browser
        .get(format!(
            "{}/login/sso/callback?error=access_denied&state={stale}",
            rig.base_url()
        ))
        .send()
        .await
        .expect("callback");
    assert_eq!(answer.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(answer.headers().get("location").is_none());

    let current = format!("current-redirect-{}", rig.suffix);
    seed_transaction(&rig.db, &current, client, 5).await;
    let answer = rig
        .browser
        .get(format!(
            "{}/login/sso/callback?error=access_denied&state={current}",
            rig.base_url()
        ))
        .send()
        .await
        .expect("callback");
    assert_eq!(answer.status(), reqwest::StatusCode::FOUND);
    let location = answer.headers()["location"].to_str().unwrap();
    assert!(
        location.starts_with(CONSOLE_REDIRECT_URI) && location.contains("error=access_denied"),
        "{location}"
    );
}

/// The upstream `code` is the one parameter the identity provider makes, and some
/// providers' codes are long. 3000 characters reach the code exchange (the fake
/// rejects it: `upstream_denied`); 5000 are not a code (`no_code`).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_callback_takes_a_long_upstream_code_but_not_an_unbounded_one() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let client = &rig.deployment.console_client_id;
    for (length, reason) in [(3000, "upstream_denied"), (5000, "no_code")] {
        let state = format!("long-code-{length}-{}", rig.suffix);
        seed_transaction(&rig.db, &state, client, 5).await;
        let code = "c".repeat(length);
        let answer = rig
            .browser
            .get(format!(
                "{}/login/sso/callback?code={code}&state={state}",
                rig.base_url()
            ))
            .send()
            .await
            .expect("callback");
        assert_eq!(answer.status(), reqwest::StatusCode::FOUND, "{length}");
        tokio::time::sleep(StdDuration::from_millis(300)).await;
        let log = rig.logs.text();
        assert!(
            log.contains(reason),
            "a {length}-character code should end as {reason}"
        );
        if length == 3000 {
            assert!(!log.contains("no_code"), "a long code is not an absent one");
        }
    }
}

/// A value the identity provider sent and the database or the `User` validators
/// refuse is not repeated in the gateway's log: the error's own text names it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_address_the_user_model_refuses_is_refused_and_never_logged() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let local = format!("sneaky-{}", rig.suffix);
    // Plausible to the broker (a local part, a dot after the first `@`), refused by
    // the model's `@email` rule, which wants exactly one `@`.
    let address = format!("{local}@x@example.test");
    rig.fake.queue_login(LoginSpec {
        email: Some(address.clone()),
        ..rig.spec()
    });
    let attempt = rig.attempt();
    assert_denied(
        &rig.sign_in(&attempt).await,
        &attempt,
        "an address the model refuses",
    );
    tokio::time::sleep(StdDuration::from_millis(300)).await;
    let log = rig.logs.text();
    assert!(
        log.contains("did not validate as a User"),
        "the refusal reached the log, so the test is exercising the path:\n{log}"
    );
    assert!(!log.contains(&local), "the address was logged:\n{log}");
    assert_eq!(rig.users_with_email(&address).await.len(), 0);
}
