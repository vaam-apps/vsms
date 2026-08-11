//! The live gate for #194: a real human login, through the real HTTP
//! surface, against a genuinely spawned `sms-gateway serve` process — not
//! an in-process router the way most of this workspace's other live
//! suites build (`rbac_layer2_live_postgres.rs`, `oidc_flow_live.rs`), and
//! not a hand-built `CoolContext` the way a unit test would. `POST /login`
//! is a raw axum route this binary's own `main.rs` mounts (see
//! `src/login.rs`'s own module doc), so — matching
//! `m1_acceptance_gate_live_postgres.rs`'s own precedent and its own
//! reasoning for why — the real, compiled `sms-gateway` binary is spawned
//! as a genuine child process (`CARGO_BIN_EXE_sms-gateway`, only available
//! to an integration test inside *this* package) rather than the route
//! handler being called directly.
//!
//! Four things this file proves, end to end over real HTTP:
//!
//! 1. A correct email/password, correct PKCE `code_verifier`, and matching
//!    `state` complete the full `/login` -> `/token` -> a real protected
//!    route round trip, and the resulting access token authenticates
//!    through `sms_api::auth::GatewayAuth`'s human path (#194's own
//!    `authenticate_human`) — not just that a token comes back, but that
//!    it actually *works* against the generated router.
//! 2. **The required PKCE guard-failure proof**: the identical flow with
//!    a *wrong* `code_verifier` at the `/token` step is refused with
//!    `invalid_grant` — see
//!    [`a_wrong_pkce_code_verifier_is_refused_at_the_token_exchange`].
//! 3. A wrong password at `/login` is refused with `invalid_credentials`,
//!    never reaching `handle_authorize` at all.
//! 4. `state` round-trips byte-for-byte through `/login`'s own redirect —
//!    the half of "state verified on the callback" this process can prove
//!    (the *verification* half is `admin`'s own TypeScript callback route;
//!    see `admin/lib/oidc.test.ts` for that half's guard-failure proof).
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. Run explicitly:
//!
//! ```bash
//! cargo test -p sms-gateway --test login_flow_live_postgres -- --ignored --nocapture
//! ```

use std::net::TcpListener as StdTcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration as StdDuration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sha2::{Digest, Sha256};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, provider as provider_filter, ClientAuthMethod, Cratestack};

const ORANGE_PROVIDER_KEY: &str = "orange_cm";
const TEST_HASH_PEPPER: &str = "login-flow-live-postgres-test-pepper-over-the-minimum-length";
const CONSOLE_CLIENT_ID: &str = "sms-console";
const REDIRECT_URI: &str = "http://127.0.0.1:1/callback";
const TEST_PASSWORD: &str = "correct horse battery staple login flow test";

fn owner() -> CoolContext {
    Principal {
        sub: "login-flow-gate-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> CoolContext {
    Principal {
        sub: "login-flow-gate-test-system".to_owned(),
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
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port");
    listener
        .local_addr()
        .expect("reading the bound address")
        .port()
}

/// Idempotent — same reasoning and shape as
/// `m1_acceptance_gate_live_postgres.rs`'s own `ensure_orange_cm_provider`:
/// `sms-gateway serve` refuses to start without an active `orange_cm`
/// `Provider` row (`resolve_provider_row_id`), and this suite has no
/// reason to send anything through it.
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
        .expect("looking up an existing orange_cm Provider row");

    if let Some(row) = existing.into_iter().next() {
        if row.state != schema::ProviderState::active {
            db.provider()
                .update(row.id)
                .set(schema::UpdateProviderInput {
                    state: Some(schema::ProviderState::active),
                    ..Default::default()
                })
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
            displayName: "Orange Cameroon (login flow gate test)".to_owned(),
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
        .run(&owner())
        .await
        .expect("activating the orange_cm Provider row");
}

/// `sms-console`, registered exactly the way `seed-console-client` (#194's
/// own CLI provisioning command) would — public client (`NoAuth`; the
/// BFF's `redirect_uri` and PKCE are the protection, no shared secret
/// exists for this client), `authorization_code`/`refresh_token`, PKCE
/// mandatory. Idempotent by construction: `create` then a `23505`-caught
/// fallback to the existing row, the same dedupe shape
/// `create_or_find_provider` in `main.rs` already uses for `Provider`.
async fn ensure_console_client(db: &Cratestack) {
    let input = schema::CreateOauthClientInput {
        clientId: CONSOLE_CLIENT_ID.to_owned(),
        appClientId: None,
        tokenEndpointAuthMethod: ClientAuthMethod::none,
        jwks: None,
        grantTypes: " authorization_code refresh_token ".to_owned(),
        scopes: " openid profile ".to_owned(),
        redirectUris: format!(" {REDIRECT_URI} "),
        requirePkce: true,
    };
    match db.oauth_client().create(input).run(&sys()).await {
        Ok(_) => {}
        Err(e) if e.db_sqlstate() == Some(sms_api::errors::UNIQUE_VIOLATION) => {}
        Err(e) => panic!("seeding the sms-console OauthClient: {e:?}"),
    }
}

/// A `Role` + `User` + `UserCredential`, seeded the same shape
/// `sms-gateway provision-user` (#194) constructs for real —
/// [`TEST_PASSWORD`] is the plaintext this suite's own login attempts use,
/// hashed with the real `sms_auth::login::hash_password` before storage,
/// never stored in the clear.
async fn seed_login_account(db: &Cratestack, suffix: &str) -> String {
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

    db.user_credential()
        .create(schema::CreateUserCredentialInput {
            userId: user.id,
            passwordHash: sms_auth::login::hash_password(TEST_PASSWORD)
                .expect("hashing the test password"),
        })
        .run(&sys())
        .await
        .expect("seeding a UserCredential");

    email
}

/// A fresh, RFC 7636-shaped PKCE pair: 43 URL-safe-base64 characters of
/// entropy (the spec's own minimum-length verifier), and its S256
/// challenge — the identical computation `authkestra_op::handlers::token`'s
/// own `handle_authorization_code` runs server-side to check it.
fn pkce_pair() -> (String, String) {
    let mut raw = [0u8; 32];
    rand::Rng::fill(&mut rand::thread_rng(), &mut raw);
    let verifier = URL_SAFE_NO_PAD.encode(raw);
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
    (verifier, challenge)
}

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
            .arg("login-flow-gate-test-orange-client-id")
            .arg("--orange-client-secret")
            .arg("login-flow-gate-test-orange-client-secret")
            .arg("--orange-sender-number")
            .arg("+237600000000")
            .arg("--hash-pepper")
            .arg(TEST_HASH_PEPPER)
            .arg("--console-client-id")
            .arg(CONSOLE_CLIENT_ID)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = command.spawn().expect("spawning sms-gateway serve");
        println!(
            "login_flow_live_postgres: spawned sms-gateway serve, pid {}, issuer {issuer}",
            child.id()
        );
        wait_until_ready(&issuer, &mut child).await;
        Self { child, issuer }
    }

    async fn kill_and_wait(mut self) {
        tokio::task::spawn_blocking(move || {
            let _ = self.child.kill();
            let _ = self.child.wait();
        })
        .await
        .expect("joining the blocking kill/wait task");
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

/// `/login`'s own request body shape — see `src/login.rs`'s `LoginRequest`
/// for the wire field names this must match exactly.
fn login_body(
    email: &str,
    password: &str,
    state: &str,
    code_challenge: &str,
    nonce: &str,
) -> serde_json::Value {
    serde_json::json!({
        "email": email,
        "password": password,
        "clientId": CONSOLE_CLIENT_ID,
        "redirectUri": REDIRECT_URI,
        "responseType": "code",
        "scope": "openid profile",
        "state": state,
        "codeChallenge": code_challenge,
        "codeChallengeMethod": "S256",
        "nonce": nonce,
    })
}

/// `code`/`state` parsed out of `/login`'s own `{"redirect": "..."}`
/// response — the URL `handle_authorize` built, never followed as a real
/// HTTP redirect by this test (there is nothing listening on
/// [`REDIRECT_URI`]; `admin`'s own callback route is what would receive
/// this in production).
fn parse_code_and_state(redirect: &str) -> (String, String) {
    let query = redirect
        .split_once('?')
        .map_or("", |(_, query)| query)
        .to_owned();
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

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_correct_login_completes_the_full_authorization_code_pkce_round_trip() {
    let db_url = sms_test_support::database_url().await;
    let db = db().await;
    let suffix = unique_suffix();

    sms_auth::op::rotate_signing_key(&db, &sys(), sms_auth::op::ROTATION_OVERLAP)
        .await
        .expect("rotating in a signing key");
    ensure_orange_cm_provider(&db).await;
    ensure_console_client(&db).await;
    let email = seed_login_account(&db, &suffix).await;

    let port = free_port();
    let process = GatewayProcess::spawn(&db_url, port).await;
    let client = reqwest::Client::new();

    let (verifier, challenge) = pkce_pair();
    let state = format!("state-{suffix}");
    let nonce = format!("nonce-{suffix}");

    // --- /login: correct credentials, correct PKCE challenge. ---
    let login_response = client
        .post(format!("{}/login", process.issuer))
        .json(&login_body(
            &email,
            TEST_PASSWORD,
            &state,
            &challenge,
            &nonce,
        ))
        .send()
        .await
        .expect("POSTing to /login");
    assert_eq!(
        login_response.status(),
        reqwest::StatusCode::OK,
        "a correct email/password/PKCE-challenge login must succeed"
    );
    let login_body: serde_json::Value = login_response.json().await.expect("parsing /login's body");
    let redirect = login_body["redirect"]
        .as_str()
        .expect("a successful login returns a redirect");

    // --- State round-trips byte-for-byte through the redirect (point 4
    // of this file's own module doc — the half this process can prove). ---
    let (code, returned_state) = parse_code_and_state(redirect);
    assert_eq!(
        returned_state, state,
        "the state this test sent to /login must come back unchanged in the redirect"
    );

    // --- /token: the real authorization_code + PKCE exchange, correct
    // verifier. ---
    let token_response = client
        .post(format!("{}/token", process.issuer))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CONSOLE_CLIENT_ID),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .expect("POSTing to /token");
    let token_status = token_response.status();
    let token_body: serde_json::Value = token_response.json().await.expect("parsing /token's body");
    assert!(
        token_status.is_success(),
        "a correct code_verifier must exchange successfully ({token_status}): {token_body}"
    );
    let access_token = token_body["access_token"]
        .as_str()
        .expect("a successful exchange returns access_token");
    assert!(
        token_body["id_token"].as_str().is_some(),
        "requesting the openid scope must yield an id_token: {token_body}"
    );

    // --- The resulting access token authenticates for real, through
    // GatewayAuth's human path, against a real generated route. `GET
    // /roles` is `auth().kind == "user"`-admitted (schema.cstack) — any
    // authenticated human token reaches it, which is exactly the
    // end-to-end claim this test makes: not just that a token comes back,
    // but that sms_api::auth::authenticate_human's User/Role lookup
    // actually resolves it to a working principal. ---
    let roles_response = client
        .get(format!("{}/roles", process.issuer))
        .bearer_auth(access_token)
        .send()
        .await
        .expect("calling GET /roles with the freshly issued human access token");
    assert_eq!(
        roles_response.status(),
        reqwest::StatusCode::OK,
        "a freshly issued human access token must authenticate against a real generated route"
    );

    process.kill_and_wait().await;
}

/// **The required guard-failure proof for PKCE (#194's own house
/// standard).** The identical flow as the success case above, except the
/// `code_verifier` presented at `/token` does not match the
/// `code_challenge` presented at `/login` — `handle_authorization_code`
/// (`authkestra_op::handlers::token`, unmodified library code) must refuse
/// this with `invalid_grant`, never issuing a token.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_wrong_pkce_code_verifier_is_refused_at_the_token_exchange() {
    let db_url = sms_test_support::database_url().await;
    let db = db().await;
    let suffix = unique_suffix();

    sms_auth::op::rotate_signing_key(&db, &sys(), sms_auth::op::ROTATION_OVERLAP)
        .await
        .expect("rotating in a signing key");
    ensure_orange_cm_provider(&db).await;
    ensure_console_client(&db).await;
    let email = seed_login_account(&db, &suffix).await;

    let port = free_port();
    let process = GatewayProcess::spawn(&db_url, port).await;
    let client = reqwest::Client::new();

    let (_correct_verifier, challenge) = pkce_pair();
    let (wrong_verifier, _unused_challenge) = pkce_pair();
    let state = format!("state-{suffix}");
    let nonce = format!("nonce-{suffix}");

    let login_response = client
        .post(format!("{}/login", process.issuer))
        .json(&login_body(
            &email,
            TEST_PASSWORD,
            &state,
            &challenge,
            &nonce,
        ))
        .send()
        .await
        .expect("POSTing to /login");
    assert_eq!(login_response.status(), reqwest::StatusCode::OK);
    let login_body: serde_json::Value = login_response.json().await.expect("parsing /login's body");
    let redirect = login_body["redirect"].as_str().expect("a redirect string");
    let (code, _state) = parse_code_and_state(redirect);

    let token_response = client
        .post(format!("{}/token", process.issuer))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CONSOLE_CLIENT_ID),
            // The wrong verifier — does not hash to `challenge` above.
            ("code_verifier", wrong_verifier.as_str()),
        ])
        .send()
        .await
        .expect("POSTing to /token with a mismatched code_verifier");
    let status = token_response.status();
    let body: serde_json::Value = token_response.json().await.expect("parsing the error body");

    assert!(
        !status.is_success(),
        "a code_verifier that doesn't match the original code_challenge must be refused, got \
         {status}: {body}"
    );
    assert_eq!(
        body["error"].as_str(),
        Some("invalid_grant"),
        "PKCE mismatch must be reported as invalid_grant per RFC 7636: {body}"
    );

    process.kill_and_wait().await;
}

/// A wrong password never reaches `handle_authorize` at all — `/login`
/// itself refuses it with `invalid_credentials`, and no code is ever
/// issued for the attempt to have leaked into.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_wrong_password_is_refused_at_login_before_any_code_is_issued() {
    let db_url = sms_test_support::database_url().await;
    let db = db().await;
    let suffix = unique_suffix();

    sms_auth::op::rotate_signing_key(&db, &sys(), sms_auth::op::ROTATION_OVERLAP)
        .await
        .expect("rotating in a signing key");
    ensure_orange_cm_provider(&db).await;
    ensure_console_client(&db).await;
    let email = seed_login_account(&db, &suffix).await;

    let port = free_port();
    let process = GatewayProcess::spawn(&db_url, port).await;
    let client = reqwest::Client::new();

    let (_verifier, challenge) = pkce_pair();
    let state = format!("state-{suffix}");
    let nonce = format!("nonce-{suffix}");

    let login_response = client
        .post(format!("{}/login", process.issuer))
        .json(&login_body(
            &email,
            "definitely the wrong password",
            &state,
            &challenge,
            &nonce,
        ))
        .send()
        .await
        .expect("POSTing to /login with the wrong password");
    let status = login_response.status();
    let body: serde_json::Value = login_response.json().await.expect("parsing the error body");

    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "body: {body}");
    assert_eq!(body["error"].as_str(), Some("invalid_credentials"));

    process.kill_and_wait().await;
}
