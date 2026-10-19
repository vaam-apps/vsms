//! The live gate for SSO: a real federated sign-in through the real HTTP
//! surface of a genuinely spawned `sms-gateway serve`, against a scripted
//! fake upstream `IdP` (`sms-fake-oidc`) and a real Postgres - the same shape
//! as `login_flow_live_postgres.rs`, for the same reasons.
//!
//! The test plays the browser (following redirects by hand, since nothing
//! listens on the console's `redirect_uri`) and the console's server leg
//! (the `/token` exchange with the PKCE verifier). What it proves, end to
//! end, per scenario:
//!
//! - a first sign-in provisions a least-privilege `User` + link and the
//!   resulting access token authenticates a real API call; a second sign-in
//!   reuses the link; linking is by `(iss, sub)`, never email;
//! - every malformed token (wrong `nonce`/`aud`/`iss`, expired, unknown or
//!   missing `kid`, bad signature, `alg: none`, `alg: HS256`) is refused with
//!   the one generic `access_denied` redirect and creates nothing;
//! - the MFA rule (`amr`, `acr`, and `acr "1"` + `amr ["pwd"]` refused), the
//!   one-shot `prompt=login` retry, and the group gate;
//! - inactive and deleted users, a replayed or expired `state`, a console
//!   request with an unregistered `redirect_uri`;
//! - configuration that cannot enforce its rules refuses to start, an
//!   unreachable `IdP` does not take password sign-in down, and no token, code or
//!   secret reaches the gateway's logs.
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

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const ORANGE_PROVIDER_KEY: &str = "orange_cm";
const TEST_HASH_PEPPER: &str = "sso-flow-live-postgres-test-pepper-over-the-minimum-length";
const CONSOLE_CLIENT_ID: &str = "sms-console";
const CONSOLE_REDIRECT_URI: &str = "http://127.0.0.1:1/callback";
const SSO_CLIENT_ID: &str = "vsms-sso-test";
const SSO_CLIENT_SECRET: &str = "sso-client-secret-that-must-never-be-logged";
const REQUIRED_GROUP: &str = "vsms-staff";

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
    Principal {
        sub: "sso-flow-test-system".to_owned(),
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
                .update(row.id)
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
            displayName: "Orange Cameroon (SSO flow gate test)".to_owned(),
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
        .update(created.id)
        .set(schema::UpdateProviderInput {
            state: Some(schema::ProviderState::active),
            ..Default::default()
        })
        .if_match(created.version)
        .run(&owner())
        .await
        .expect("activating the orange_cm Provider row");
}

async fn ensure_console_client(db: &Cratestack) {
    let input = schema::CreateOauthClientInput {
        clientId: CONSOLE_CLIENT_ID.to_owned(),
        appClientId: None,
        tokenEndpointAuthMethod: ClientAuthMethod::none,
        jwks: None,
        grantTypes: " authorization_code refresh_token ".to_owned(),
        scopes: " openid profile ".to_owned(),
        redirectUris: format!(" {CONSOLE_REDIRECT_URI} "),
        requirePkce: true,
    };
    match db.oauth_client().create(input).run(&sys()).await {
        Ok(_) => {}
        Err(e) if e.db_sqlstate() == Some(sms_api::errors::UNIQUE_VIOLATION) => {}
        Err(e) => panic!("seeding the sms-console OauthClient: {e:?}"),
    }
}

/// Everything the gateway process is told about SSO. `None` fields are left
/// off the command line entirely.
#[derive(Clone)]
struct SsoFlags {
    issuer: Option<String>,
    accepted_amr: Vec<String>,
    accepted_acr: Vec<String>,
    jit_role: Option<String>,
    required_group: Option<String>,
}

impl SsoFlags {
    fn amr_otp(issuer: &str) -> Self {
        Self {
            issuer: Some(issuer.to_owned()),
            accepted_amr: vec!["otp".to_owned()],
            accepted_acr: vec![],
            jit_role: None,
            required_group: Some(REQUIRED_GROUP.to_owned()),
        }
    }
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

struct GatewayProcess {
    child: Child,
    issuer: String,
    logs: Logs,
}

impl GatewayProcess {
    fn command(db_url: &str, port: u16, sso: Option<&SsoFlags>) -> (Command, String) {
        let issuer = format!("http://127.0.0.1:{port}");
        let mut command = Command::new(env!("CARGO_BIN_EXE_sms-gateway"));
        command
            .arg("serve")
            .args(["--listen", &format!("127.0.0.1:{port}")])
            .args(["--metrics-listen", &format!("127.0.0.1:{}", free_port())])
            .args(["--database-url", db_url])
            .args(["--issuer", &issuer])
            .args(["--orange-client-id", "sso-flow-test-orange-client-id"])
            .args([
                "--orange-client-secret",
                "sso-flow-test-orange-client-secret",
            ])
            .args(["--orange-sender-number", "+237600000000"])
            .args(["--hash-pepper", TEST_HASH_PEPPER])
            .args(["--console-client-id", CONSOLE_CLIENT_ID])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(sso) = sso {
            if let Some(sso_issuer) = &sso.issuer {
                command.args(["--sso-issuer", sso_issuer]);
            }
            command.args(["--sso-client-id", SSO_CLIENT_ID]);
            // Through the environment, the way a real deployment passes it.
            command.env("SMS_SSO_CLIENT_SECRET", SSO_CLIENT_SECRET);
            command.args([
                "--sso-redirect-uri",
                &format!("{issuer}/login/sso/callback"),
            ]);
            if let Some(group) = &sso.required_group {
                command.args(["--sso-required-group", group]);
            }
            if !sso.accepted_amr.is_empty() {
                command.args(["--sso-accepted-amr", &sso.accepted_amr.join(",")]);
            }
            if !sso.accepted_acr.is_empty() {
                command.args(["--sso-accepted-acr", &sso.accepted_acr.join(",")]);
            }
            if let Some(role) = &sso.jit_role {
                command.args(["--sso-jit-role", role]);
            }
            // Zero: the rotation test must be able to refetch at once.
            command.args(["--sso-jwks-min-refresh-secs", "0"]);
        }
        (command, issuer)
    }

    async fn spawn(db_url: &str, port: u16, sso: Option<&SsoFlags>) -> Self {
        let (mut command, issuer) = Self::command(db_url, port, sso);
        let mut child = command.spawn().expect("spawning sms-gateway serve");
        let logs = Logs::default();
        drain(child.stdout.take().expect("piped stdout"), logs.clone());
        drain(child.stderr.take().expect("piped stderr"), logs.clone());
        wait_until_ready(&issuer, &mut child).await;
        Self {
            child,
            issuer,
            logs,
        }
    }
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn wait_until_ready(issuer: &str, child: &mut Child) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(20);
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
            "sms-gateway serve never became ready within 20s"
        );
        tokio::time::sleep(StdDuration::from_millis(200)).await;
    }
}

/// A running SSO deployment: database, fake upstream `IdP`, gateway.
struct Rig {
    db: Cratestack,
    fake: FakeOidc,
    gateway: GatewayProcess,
    browser: reqwest::Client,
    suffix: String,
}

impl Rig {
    async fn start() -> Self {
        Self::start_with(SsoFlags::amr_otp).await
    }

    async fn start_with(flags: impl FnOnce(&str) -> SsoFlags) -> Self {
        let db_url = sms_test_support::database_url().await;
        let db = db().await;
        sms_auth::op::rotate_signing_key(&db, &sys(), sms_auth::op::ROTATION_OVERLAP)
            .await
            .expect("rotating in a signing key");
        ensure_orange_cm_provider(&db).await;
        ensure_console_client(&db).await;
        let fake = FakeOidc::start(SSO_CLIENT_ID, SSO_CLIENT_SECRET).await;
        let flags = flags(fake.issuer());
        let gateway = GatewayProcess::spawn(&db_url, free_port(), Some(&flags)).await;
        Self {
            db,
            fake,
            gateway,
            browser: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("browser client"),
            suffix: unique_suffix(),
        }
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
    verifier: String,
    challenge: String,
    state: String,
    nonce: String,
}

impl Attempt {
    fn new() -> Self {
        let mut raw = [0u8; 32];
        rand::Rng::fill(&mut rand::thread_rng(), &mut raw);
        let verifier = URL_SAFE_NO_PAD.encode(raw);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let tag = unique_suffix();
        Self {
            verifier,
            challenge,
            state: format!("console-state-{tag}"),
            nonce: format!("console-nonce-{tag}"),
        }
    }

    fn start_url(&self, gateway: &str, redirect_uri: &str, client_id: &str) -> String {
        reqwest::Url::parse_with_params(
            &format!("{gateway}/login/sso/start"),
            [
                ("client_id", client_id),
                ("redirect_uri", redirect_uri),
                ("response_type", "code"),
                ("scope", "openid profile"),
                ("state", self.state.as_str()),
                ("nonce", self.nonce.as_str()),
                ("code_challenge", self.challenge.as_str()),
                ("code_challenge_method", "S256"),
            ],
        )
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
/// response). Bounded, so a redirect loop fails the test rather than
/// hanging it.
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

async fn sign_in(rig: &Rig, attempt: &Attempt) -> Outcome {
    walk(
        &rig.browser,
        attempt.start_url(&rig.gateway.issuer, CONSOLE_REDIRECT_URI, CONSOLE_CLIENT_ID),
    )
    .await
}

/// The console's server leg: exchange the code at the gateway's `/token`.
async fn exchange(rig: &Rig, attempt: &Attempt, code: &str) -> serde_json::Value {
    let response = reqwest::Client::new()
        .post(format!("{}/token", rig.gateway.issuer))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", CONSOLE_REDIRECT_URI),
            ("client_id", CONSOLE_CLIENT_ID),
            ("code_verifier", attempt.verifier.as_str()),
        ])
        .send()
        .await
        .expect("POSTing /token");
    let status = response.status();
    let body: serde_json::Value = response.json().await.expect("a /token body");
    assert!(
        status.is_success(),
        "the console's /token exchange failed: {status}: {body}"
    );
    body
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

// ---------------------------------------------------------------------------
// The happy paths.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_first_sign_in_provisions_a_least_privilege_user_and_reaches_an_authenticated_api_call() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    rig.fake.queue_login(rig.spec());

    let attempt = Attempt::new();
    let Outcome::Code { code, state } = sign_in(&rig, &attempt).await else {
        panic!("a valid first sign-in must end at the console with a code");
    };
    assert_eq!(state, attempt.state, "the console's state round-trips");

    // The console's /api/auth/callback step, unchanged.
    let tokens = exchange(&rig, &attempt, &code).await;
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
        "the JIT role is the configured default"
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

    // A JIT user has no password: the break-glass path cannot be guessed
    // into for an account that never had one.
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

    // And the token really authenticates, through GatewayAuth's human path.
    let roles = reqwest::Client::new()
        .get(format!("{}/roles", rig.gateway.issuer))
        .bearer_auth(access_token)
        .send()
        .await
        .expect("GET /roles");
    assert_eq!(roles.status(), reqwest::StatusCode::OK);

    // Nothing sensitive reached the logs.
    let logs = rig.gateway.logs.text();
    for secret in [SSO_CLIENT_SECRET, access_token, id_token, code.as_str()] {
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

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_second_sign_in_reuses_the_link_and_never_re_reads_the_email() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;

    rig.fake.queue_login(rig.spec());
    let first = Attempt::new();
    assert!(matches!(sign_in(&rig, &first).await, Outcome::Code { .. }));
    let original = rig.users_with_email(&rig.email()).await.remove(0);

    // Same upstream sub, but the IdP now says a different email and name.
    rig.fake.queue_login(LoginSpec {
        email: Some(format!("renamed-{}@example.test", rig.suffix)),
        name: Some("Somebody Else".to_owned()),
        ..rig.spec()
    });
    let second = Attempt::new();
    let Outcome::Code { code, .. } = sign_in(&rig, &second).await else {
        panic!("the second sign-in must succeed");
    };
    let tokens = exchange(&rig, &second, &code).await;
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
    let attempt = Attempt::new();
    let outcome = sign_in(&rig, &attempt).await;
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

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_jit_role_is_the_configured_one_and_never_taken_from_a_claim() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start_with(|issuer| SsoFlags {
        jit_role: Some("auditor".to_owned()),
        ..SsoFlags::amr_otp(issuer)
    })
    .await;
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

    let attempt = Attempt::new();
    assert!(matches!(
        sign_in(&rig, &attempt).await,
        Outcome::Code { .. }
    ));
    let user = rig.users_with_email(&rig.email()).await.remove(0);
    assert_eq!(
        user.roleKey, "auditor",
        "the configured role, whatever the token says"
    );
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
        sign_in(&rig, &Attempt::new()).await,
        Outcome::Code { .. }
    ));

    let cases = malformed_token_cases(&rig);
    for (why, spec) in cases {
        let before = rig.fake.authorize_requests().len();
        rig.fake.queue_login(spec);
        let attempt = Attempt::new();
        let outcome = sign_in(&rig, &attempt).await;
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
    let logs = rig.gateway.logs.text();
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
        sign_in(&rig, &Attempt::new()).await,
        Outcome::Code { .. }
    ));

    rig.fake.publish_key(SigningKey::Rsa2);
    rig.fake.queue_login(LoginSpec {
        key: SigningKey::Rsa2,
        ..rig.spec()
    });
    assert!(matches!(
        sign_in(&rig, &Attempt::new()).await,
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

    let attempt = Attempt::new();
    let Outcome::Code { code, state } = sign_in(&rig, &attempt).await else {
        panic!("the retry's otp token must sign the user in");
    };
    assert_eq!(state, attempt.state, "the console never sees the retry");
    exchange(&rig, &attempt, &code).await;

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

    let attempt = Attempt::new();
    let outcome = sign_in(&rig, &attempt).await;
    assert_denied(&outcome, &attempt, "a retry that still lacks the MFA claim");
    assert_eq!(
        rig.fake.authorize_requests().len(),
        2,
        "at most one retry - no loop"
    );
    assert!(rig.users_with_email(&rig.email()).await.is_empty());
    assert!(rig.gateway.logs.text().contains("mfa_not_satisfied"));
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn any_failure_other_than_the_mfa_claim_is_refused_at_once_with_no_retry() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let cases: Vec<(&str, LoginSpec)> = vec![
        // The MFA claim is also missing, but the nonce is wrong: the MFA
        // rule is NOT the only reason, so there is no second chance.
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
        let attempt = Attempt::new();
        assert_denied(&sign_in(&rig, &attempt).await, &attempt, why);
        assert_eq!(
            rig.fake.authorize_requests().len(),
            before + 1,
            "{why}: no retry"
        );
    }
    assert!(rig.users_with_email(&rig.email()).await.is_empty());
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_accepted_acr_satisfies_the_rule_when_only_acr_is_configured() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start_with(|issuer| SsoFlags {
        accepted_amr: vec![],
        accepted_acr: vec!["urn:vsms:mfa".to_owned()],
        ..SsoFlags::amr_otp(issuer)
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
    let refused = Attempt::new();
    assert_denied(
        &sign_in(&rig, &refused).await,
        &refused,
        "acr 1 is not the accepted acr",
    );

    rig.fake.queue_login(LoginSpec {
        acr: Some("urn:vsms:mfa".into()),
        amr: None,
        ..rig.spec()
    });
    let accepted = Attempt::new();
    assert!(matches!(
        sign_in(&rig, &accepted).await,
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
        sign_in(&rig, &Attempt::new()).await,
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
    let attempt = Attempt::new();
    assert_denied(&sign_in(&rig, &attempt).await, &attempt, "an inactive user");
    assert!(rig.gateway.logs.text().contains("user_inactive"));

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
        sign_in(&rig, &Attempt::new()).await,
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
    let attempt = Attempt::new();
    assert_denied(
        &sign_in(&rig, &attempt).await,
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
    let attempt = Attempt::new();

    // Walk by hand to the gateway's callback URL, without taking it.
    let start = rig
        .browser
        .get(attempt.start_url(&rig.gateway.issuer, CONSOLE_REDIRECT_URI, CONSOLE_CLIENT_ID))
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
            rig.gateway.issuer
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
    rig.db
        .sso_transaction()
        .create(schema::CreateSsoTransactionInput {
            stateHash: sms_auth::sso::store::state_hash(&state),
            nonce: "n".to_owned(),
            codeVerifier: "v".to_owned(),
            consoleRequest: serde_json::json!({
                "client_id": CONSOLE_CLIENT_ID,
                "redirect_uri": CONSOLE_REDIRECT_URI,
                "response_type": "code",
            })
            .to_string(),
            isRetry: false,
            expiresAt: chrono::Utc::now() - chrono::Duration::minutes(1),
        })
        .run(&sys())
        .await
        .expect("seeding an expired transaction");
    let response = rig
        .browser
        .get(format!(
            "{}/login/sso/callback?code=anything&state={state}",
            rig.gateway.issuer
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

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_console_request_the_op_would_refuse_is_refused_at_start_before_anything_is_stored() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let attempt = Attempt::new();
    let before = rig.open_transactions().await;

    let unregistered = rig
        .browser
        .get(attempt.start_url(
            &rig.gateway.issuer,
            "https://evil.example/cb",
            CONSOLE_CLIENT_ID,
        ))
        .send()
        .await
        .expect("start with an unregistered redirect_uri");
    assert_eq!(unregistered.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(
        unregistered.headers().get("location").is_none(),
        "never redirect to an unvalidated URI"
    );

    let unknown_client = rig
        .browser
        .get(attempt.start_url(&rig.gateway.issuer, CONSOLE_REDIRECT_URI, "nobody"))
        .send()
        .await
        .expect("start with an unknown client");
    assert_eq!(unknown_client.status(), reqwest::StatusCode::BAD_REQUEST);

    // No PKCE: the library's own redirect-able error, passed through.
    let no_pkce = reqwest::Url::parse_with_params(
        &format!("{}/login/sso/start", rig.gateway.issuer),
        [
            ("client_id", CONSOLE_CLIENT_ID),
            ("redirect_uri", CONSOLE_REDIRECT_URI),
            ("response_type", "code"),
            ("state", "s"),
        ],
    )
    .unwrap();
    let response = rig
        .browser
        .get(no_pkce)
        .send()
        .await
        .expect("start without PKCE");
    assert_eq!(response.status(), reqwest::StatusCode::FOUND);
    let location = response.headers()["location"].to_str().unwrap();
    assert!(
        location.starts_with(CONSOLE_REDIRECT_URI) && location.contains("error=invalid_request")
    );

    assert!(
        rig.fake.authorize_requests().is_empty(),
        "the upstream IdP was never contacted"
    );
    assert_eq!(
        rig.open_transactions().await,
        before,
        "no transaction row was written"
    );
}

// ---------------------------------------------------------------------------
// Configuration, discovery, resilience.
// ---------------------------------------------------------------------------

/// Spawns the gateway expecting it to exit, and returns its combined output.
fn gateway_exit_output(db_url: &str, sso: &SsoFlags) -> (std::process::ExitStatus, String) {
    let (mut command, _issuer) = GatewayProcess::command(db_url, free_port(), Some(sso));
    let mut child = command.spawn().expect("spawning sms-gateway serve");
    let logs = Logs::default();
    drain(child.stdout.take().unwrap(), logs.clone());
    drain(child.stderr.take().unwrap(), logs.clone());
    let deadline = std::time::Instant::now() + StdDuration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().expect("polling the child") {
            std::thread::sleep(StdDuration::from_millis(200)); // let the drain threads finish
            return (status, logs.text());
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the gateway kept running with a configuration it must refuse");
        }
        std::thread::sleep(StdDuration::from_millis(100));
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_configuration_that_cannot_enforce_its_rules_refuses_to_start() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;
    sms_auth::op::rotate_signing_key(&db, &sys(), sms_auth::op::ROTATION_OVERLAP)
        .await
        .expect("rotating in a signing key");
    ensure_orange_cm_provider(&db).await;
    let base = SsoFlags::amr_otp("https://idp.example.test/realms/staff");

    let cases: Vec<(&str, SsoFlags, &str)> = vec![
        (
            "both MFA lists empty",
            SsoFlags {
                accepted_amr: vec![],
                accepted_acr: vec![],
                ..base.clone()
            },
            "multi-factor",
        ),
        (
            "no required group",
            SsoFlags {
                required_group: None,
                ..base.clone()
            },
            "--sso-required-group",
        ),
        (
            "no issuer but the rest set",
            SsoFlags {
                issuer: None,
                ..base.clone()
            },
            "--sso-issuer",
        ),
        (
            "a role that does not exist",
            SsoFlags {
                jit_role: Some("no_such_role".to_owned()),
                ..base.clone()
            },
            "no_such_role",
        ),
        (
            "a reserved role",
            SsoFlags {
                jit_role: Some("system".to_owned()),
                ..base.clone()
            },
            "reserved",
        ),
        (
            "a cleartext remote issuer",
            SsoFlags {
                issuer: Some("http://idp.example.test/realms/staff".to_owned()),
                ..base.clone()
            },
            "https",
        ),
    ];
    for (why, flags, expected) in cases {
        let (status, output) = gateway_exit_output(&db_url, &flags);
        assert!(!status.success(), "{why}: the gateway must refuse to start");
        assert!(
            output.contains(expected),
            "{why}: expected {expected:?} in:\n{output}"
        );
        assert!(
            !output.contains(SSO_CLIENT_SECRET),
            "{why}: the secret was printed"
        );
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn discovery_advertises_the_start_url_only_when_sso_is_configured() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::start().await;
    let document: serde_json::Value = reqwest::get(format!(
        "{}/.well-known/openid-configuration",
        rig.gateway.issuer
    ))
    .await
    .expect("discovery")
    .json()
    .await
    .expect("json");
    assert_eq!(
        document["vsms_sso_login_endpoint"],
        format!("{}/login/sso/start", rig.gateway.issuer)
    );
    assert!(
        document["token_endpoint_auth_methods_supported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "private_key_jwt"),
        "the existing document is intact"
    );

    let plain =
        GatewayProcess::spawn(&sms_test_support::database_url().await, free_port(), None).await;
    let document: serde_json::Value =
        reqwest::get(format!("{}/.well-known/openid-configuration", plain.issuer))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    assert!(
        document.get("vsms_sso_login_endpoint").is_none(),
        "no SSO config, no advertisement"
    );
    let start = reqwest::Client::new()
        .get(format!("{}/login/sso/start", plain.issuer))
        .send()
        .await
        .unwrap();
    assert_eq!(
        start.status(),
        reqwest::StatusCode::NOT_FOUND,
        "and no SSO routes"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_unreachable_idp_does_not_stop_the_gateway_or_password_sign_in() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;
    sms_auth::op::rotate_signing_key(&db, &sys(), sms_auth::op::ROTATION_OVERLAP)
        .await
        .expect("rotating in a signing key");
    ensure_orange_cm_provider(&db).await;
    ensure_console_client(&db).await;

    // Nothing listens on this port.
    let dead = format!("http://127.0.0.1:{}/realms/staff", free_port());
    let gateway =
        GatewayProcess::spawn(&db_url, free_port(), Some(&SsoFlags::amr_otp(&dead))).await;
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let attempt = Attempt::new();
    let outcome = walk(
        &browser,
        attempt.start_url(&gateway.issuer, CONSOLE_REDIRECT_URI, CONSOLE_CLIENT_ID),
    )
    .await;
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
    db.user_credential()
        .create(schema::CreateUserCredentialInput {
            userId: user.id,
            passwordHash: sms_core::password::hash_password("break glass password 12345")
                .expect("hash"),
        })
        .run(&sys())
        .await
        .expect("seeding a credential");
    let login = reqwest::Client::new()
        .post(format!("{}/login", gateway.issuer))
        .json(&serde_json::json!({
            "email": email,
            "password": "break glass password 12345",
            "clientId": CONSOLE_CLIENT_ID,
            "redirectUri": CONSOLE_REDIRECT_URI,
            "responseType": "code",
            "scope": "openid profile",
            "state": "s",
            "codeChallenge": attempt.challenge,
            "codeChallengeMethod": "S256",
            "nonce": "n",
        }))
        .send()
        .await
        .expect("POST /login");
    assert_eq!(
        login.status(),
        reqwest::StatusCode::OK,
        "password sign-in still works"
    );
}
