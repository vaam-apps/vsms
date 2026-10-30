//! VSA-3a's acceptance: the `OpenID` Provider and its two operator commands, as
//! real, separate OS processes against a real Postgres.
//!
//! Two states of the same switch (`auth.methods` and the `op:` block):
//!
//! - **Off** (the absent default, which permits `machine_tokens` and writes no
//!   `op:`; and `methods: [keypair]`): `/token`, `/jwks.json` and the discovery
//!   document are not routes (404), and `provision-client` and
//!   `rotate-signing-key` refuse, `auth_method_disabled`, naming
//!   `register-client`, and write nothing.
//! - **On** (`op:` written): `serve` refuses to start with no active signing
//!   key; `rotate-signing-key` and `provision-client` then work; the real
//!   `/token` mints a token a verifier can check against the served
//!   `/jwks.json`, with no refresh token; a replayed assertion and the refresh
//!   grant are refused; a rotation reaches a running process; the per-`client_id`
//!   bucket throttles.
//!
//! What is **not** asserted, on purpose: that the API accepts the token. It does
//! not yet (VSA-3b), and `a_minted_token_is_not_yet_accepted_by_the_api` pins that
//! so the day it changes the test fails and says so.
//!
//! ```bash
//! cargo test -p sms-gateway --test machine_tokens_live_postgres -- --ignored --test-threads=1 --nocapture
//! ```

use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration as StdDuration;

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack, provider as provider_filter};
use sms_test_support::config::{self, TestConfig};

#[path = "../../../crates/sms-api/tests/support/sealed_fixtures.rs"]
mod sealed_fixtures;

/// Real processes sharing one database: one at a time (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const ORANGE_PROVIDER_KEY: &str = "orange_cm";

fn context(role: &str) -> CratestackContext {
    Principal {
        sub: format!("machine-tokens-live-test-{role}"),
        kind: if role == "system" {
            PrincipalKind::App
        } else {
            PrincipalKind::User
        },
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
        .run(&context("owner"))
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
                .run(&context("owner"))
                .await
                .expect("reactivating the orange_cm Provider row");
        }
        return;
    }
    let created = db
        .provider()
        .create(schema::CreateProviderInput {
            key: ORANGE_PROVIDER_KEY.to_owned(),
            displayName: "Orange Cameroon (machine tokens test)".to_owned(),
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
        .run(&context("owner"))
        .await
        .expect("seeding the orange_cm Provider row");
    db.provider()
        .update(created.id.clone())
        .set(schema::UpdateProviderInput {
            state: Some(schema::ProviderState::active),
            ..Default::default()
        })
        .if_match(created.version)
        .run(&context("owner"))
        .await
        .expect("activating the orange_cm Provider row");
}

async fn seed_app(db: &Cratestack) -> schema::App {
    db.app()
        .create(schema::CreateAppInput {
            name: "machine tokens live test app".to_owned(),
            slug: format!("machine-tokens-{}", unique_suffix()),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&context("owner"))
        .await
        .expect("seeding the app")
}

/// The overlay's `op:` block for `issuer`, with the given bucket and a one-second
/// key refresh so a rotation reaches a running process inside the test.
fn op_block(issuer: &str, burst: u32) -> String {
    format!(
        "op:\n  issuer: {issuer}\n  token_rate_limit:\n    burst: {burst}\n    \
         refill_per_second: 0.01\n  signing_key:\n    refresh_interval_secs: 1\n    \
         rotation_overlap_secs: 1800\n"
    )
}

/// Everything a spawned `serve` or CLI needs: the written configuration (kept
/// alive beside the child), the key files and the database.
struct Deployment {
    config: TestConfig,
    db_url: String,
    port: u16,
    keys: (PathBuf, PathBuf),
}

impl Deployment {
    /// `extra` is appended to the overlay: the `auth:` and `op:` blocks.
    async fn new(extra: &str, tag: &str) -> Self {
        let port = free_port();
        Self::on_port(port, extra, tag).await
    }

    async fn on_port(port: u16, extra: &str, tag: &str) -> Self {
        let config = TestConfig::write(
            Path::new(env!("CARGO_TARGET_TMPDIR")),
            &format!("machine-tokens-{tag}-{port}"),
            &format!("{}\n{extra}", config::orange_overlay("http://127.0.0.1:9")),
        );
        let keys = sealed_fixtures::write_server_key_files(
            &std::env::temp_dir().join(format!("sms-machine-tokens-keys-{port}")),
        );
        Self {
            config,
            db_url: sms_test_support::database_url().await,
            port,
            keys,
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
            .arg("--database-url")
            .arg(&self.db_url)
            .arg("--envelope-signing-key-file")
            .arg(&self.keys.0)
            .arg("--envelope-next-signing-key-file")
            .arg(&self.keys.1)
            .env("NO_COLOR", "1");
        command
    }

    /// A long-lived `serve`, ready, with its log piped for the boot assertions.
    async fn spawn(&self) -> Gateway {
        let mut command = self.serve_command();
        command.stderr(Stdio::inherit()).stdout(Stdio::piped());
        let mut child = command.spawn().expect("spawning sms-gateway serve");
        wait_until_ready(&self.base_url(), &mut child).await;
        Gateway {
            child,
            base_url: self.base_url(),
        }
    }

    /// `sms-gateway <subcommand> --config … --profile …` plus `args`, run to
    /// completion.
    fn cli(&self, subcommand: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
            .arg(subcommand)
            .args(self.config.args())
            .arg("--database-url")
            .arg(&self.db_url)
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("running `sms-gateway {subcommand}`: {error}"))
    }
}

struct Gateway {
    child: Child,
    base_url: String,
}

impl Gateway {
    fn kill(mut self) -> String {
        self.child.kill().expect("killing sms-gateway serve");
        let mut log = String::new();
        if let Some(mut stdout) = self.child.stdout.take() {
            std::io::Read::read_to_string(&mut stdout, &mut log).expect("reading the log");
        }
        self.child.wait().expect("reaping sms-gateway serve");
        log
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

async fn count_rows(db: &Cratestack) -> (usize, usize, usize) {
    let system = context("system");
    (
        db.oauth_client()
            .find_many()
            .run(&system)
            .await
            .unwrap()
            .len(),
        db.oauth_signing_key()
            .find_many()
            .run(&system)
            .await
            .unwrap()
            .len(),
        db.app_client()
            .find_many()
            .run(&system)
            .await
            .unwrap()
            .len(),
    )
}

/// Switches every signing key off, so the next `serve` meets a fresh deployment.
async fn deactivate_signing_keys(db: &Cratestack) {
    let system = context("system");
    for key in db
        .oauth_signing_key()
        .find_many()
        .run(&system)
        .await
        .unwrap()
    {
        if key.active {
            db.oauth_signing_key()
                .update(key.id)
                .set(schema::UpdateOauthSigningKeyInput {
                    active: Some(false),
                    ..Default::default()
                })
                .run(&system)
                .await
                .expect("deactivating a signing key");
        }
    }
}

// --- the method off ---------------------------------------------------------

/// The routes the provider mounts, with the method each answers.
const OP_ROUTES: &[(&str, &str)] = &[
    ("POST", "/token"),
    ("GET", "/jwks.json"),
    ("GET", "/.well-known/openid-configuration"),
];

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_the_method_off_nothing_is_mounted_and_the_commands_refuse() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let app = seed_app(&db).await;
    let http = reqwest::Client::new();

    // The absent default permits machine_tokens and writes no op: block, so it
    // serves none; `[keypair]` does not permit it at all.
    for (case, extra) in [
        ("absent default, no op:", ""),
        ("methods: [keypair]", "auth:\n  methods: [keypair]\n"),
    ] {
        let deployment = Deployment::new(extra, "off").await;
        let before = count_rows(&db).await;

        let gateway = deployment.spawn().await;
        for (method, path) in OP_ROUTES {
            let url = format!("{}{path}", gateway.base_url);
            let request = if *method == "POST" {
                http.post(&url)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body("grant_type=client_credentials&client_id=appc_off")
            } else {
                http.get(&url)
            };
            let response = request.send().await.expect("requesting an OP route");
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            assert_eq!(
                status, 404,
                "{case}: {method} {path} must not be a route: {body}"
            );
            assert!(
                !body.contains("access_token") && !body.contains("\"keys\""),
                "{case}: {method} {path} answered with token material: {body}"
            );
        }
        let discovery: serde_json::Value = http
            .get(format!("{}/.well-known/vsms-auth", gateway.base_url))
            .send()
            .await
            .expect("requesting the auth discovery")
            .json()
            .await
            .expect("a JSON document");
        assert!(
            !discovery["methods"]
                .as_array()
                .expect("a list")
                .iter()
                .any(|method| method == "machine_tokens"),
            "{case}: machine_tokens must not be advertised: {discovery}"
        );
        gateway.kill();

        let key_out = deployment.config.args()[1].clone() + ".never-written.pem";
        for (command, args) in [
            ("rotate-signing-key", Vec::<&str>::new()),
            (
                "provision-client",
                vec![
                    "--app-id",
                    app.id.as_str(),
                    "--label",
                    "off",
                    "--scope",
                    "sms:send",
                    "--key-out",
                    key_out.as_str(),
                ],
            ),
        ] {
            let output = deployment.cli(command, &args);
            let text = stderr(&output);
            assert!(!output.status.success(), "{case}: {command} must refuse");
            assert!(
                text.contains("auth_method_disabled"),
                "{case}: {command}: {text}"
            );
            assert!(
                text.contains("issues no tokens"),
                "{case}: {command}: {text}"
            );
            assert!(
                text.contains("register-client"),
                "{case}: {command}: {text}"
            );
        }
        assert!(
            !Path::new(&key_out).exists(),
            "{case}: no key file may appear"
        );
        assert_eq!(
            count_rows(&db).await,
            before,
            "{case}: a refusal writes nothing"
        );
    }
}

// --- the method on ----------------------------------------------------------

#[derive(serde::Serialize)]
struct AssertionClaims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    jti: String,
    exp: i64,
}

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

async fn post_token(
    base_url: &str,
    client_id: &str,
    grant: &str,
    assertion: &str,
) -> (u16, serde_json::Value) {
    let response = reqwest::Client::new()
        .post(format!("{base_url}/token"))
        .form(&[
            ("grant_type", grant),
            ("client_id", client_id),
            (
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
            ),
            ("client_assertion", assertion),
            ("scope", "sms:send"),
            ("refresh_token", "not-a-real-refresh-token"),
        ])
        .send()
        .await
        .expect("POSTing to /token");
    let status = response.status().as_u16();
    let body = response.json().await.unwrap_or(serde_json::Value::Null);
    (status, body)
}

/// A field of `provision-client`'s documented stdout.
fn client_id_of(output: &Output) -> String {
    stdout(output)
        .lines()
        .find_map(|line| line.strip_prefix("provisioned client: "))
        .unwrap_or_else(|| panic!("no \"provisioned client:\" line in {:?}", stdout(output)))
        .trim()
        .to_owned()
}

#[cfg(unix)]
fn unix_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

/// Step 3: discovery, the auth discovery and `/jwks.json`, unsigned. Returns the
/// key set.
async fn assert_public_documents(http: &reqwest::Client, issuer: &str) -> serde_json::Value {
    let discovery: serde_json::Value = http
        .get(format!("{issuer}/.well-known/openid-configuration"))
        .send()
        .await
        .expect("discovery")
        .json()
        .await
        .expect("a JSON document");
    assert_eq!(discovery["issuer"], issuer);
    assert_eq!(discovery["token_endpoint"], format!("{issuer}/token"));
    assert_eq!(
        discovery["grant_types_supported"],
        serde_json::json!(["client_credentials"]),
        "no refresh_token and no authorization_code are advertised"
    );
    assert!(
        discovery["token_endpoint_auth_methods_supported"]
            .as_array()
            .expect("a list")
            .iter()
            .any(|method| method == "private_key_jwt")
    );
    let served: serde_json::Value = http
        .get(format!("{issuer}/.well-known/vsms-auth"))
        .send()
        .await
        .expect("auth discovery")
        .json()
        .await
        .expect("a JSON document");
    assert!(
        served["methods"]
            .as_array()
            .expect("a list")
            .iter()
            .any(|method| method == "machine_tokens"),
        "machine_tokens is advertised when it is served: {served}"
    );
    let jwks: serde_json::Value = http
        .get(format!("{issuer}/jwks.json"))
        .send()
        .await
        .expect("jwks")
        .json()
        .await
        .expect("a JSON document");
    let keys = jwks["keys"].as_array().expect("a key set");
    assert!(!keys.is_empty() && keys.iter().all(|key| key["d"].is_null()));
    jwks
}

/// Step 4's verification: the token is signed by a published key, issued by this
/// deployment, to this client, for the provisioned scope. Returns the `kid`.
fn assert_token_verifies(
    access_token: &str,
    keys: &[serde_json::Value],
    issuer: &str,
    client_id: &str,
) -> String {
    let kid = jsonwebtoken::decode_header(access_token)
        .expect("a JWT header")
        .kid
        .expect("a kid");
    let jwk = keys
        .iter()
        .find(|key| key["kid"] == kid.as_str())
        .unwrap_or_else(|| panic!("kid {kid} is published in /jwks.json"));
    let decoding = jsonwebtoken::DecodingKey::from_rsa_components(
        jwk["n"].as_str().expect("n"),
        jwk["e"].as_str().expect("e"),
    )
    .expect("a DecodingKey from the published JWK");
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    validation.set_issuer(&[issuer]);
    validation.validate_aud = false;
    let claims = jsonwebtoken::decode::<serde_json::Value>(access_token, &decoding, &validation)
        .expect("the token verifies against the served JWKS")
        .claims;
    assert_eq!(claims["sub"], client_id);
    assert_eq!(claims["iss"], issuer);
    assert_eq!(claims["scope"], "sms:send");
    kid
}

/// Step 2: the operator path, rotate then provision. Returns the client id and
/// the private key the command wrote.
fn operator_path(deployment: &Deployment, app: &schema::App, port: u16) -> (String, String) {
    let rotated = deployment.cli("rotate-signing-key", &[]);
    assert!(rotated.status.success(), "{}", stderr(&rotated));
    assert!(stdout(&rotated).contains("rotated: new signing key"));

    let key_out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("op-client-{port}.pem"));
    let _ = std::fs::remove_file(&key_out);
    let provisioned = deployment.cli(
        "provision-client",
        &[
            "--app-id",
            &app.id,
            "--label",
            "otp sender",
            "--scope",
            "sms:send",
            "--key-out",
            key_out.to_str().expect("utf-8 path"),
        ],
    );
    assert!(provisioned.status.success(), "{}", stderr(&provisioned));
    let client_id = client_id_of(&provisioned);
    #[cfg(unix)]
    assert_eq!(unix_mode(&key_out), 0o600, "the private key is owner-only");
    let private_key_pem = std::fs::read_to_string(&key_out).expect("the key file");
    assert!(
        !stdout(&provisioned).contains("BEGIN") && !stderr(&provisioned).contains("BEGIN"),
        "the key never reaches the terminal"
    );
    (client_id, private_key_pem)
}

/// Step 7: a rotation reaches the running process within the refresh interval,
/// the previous key keeps publishing, and a new token is signed by the new key.
async fn assert_rotation_reaches_the_process(
    deployment: &Deployment,
    http: &reqwest::Client,
    issuer: &str,
    client_id: &str,
    private_key_pem: &str,
    kid: &str,
) {
    let again = deployment.cli("rotate-signing-key", &[]);
    assert!(again.status.success(), "{}", stderr(&again));
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(20);
    let new_kid = loop {
        let (status, token) = post_token(
            issuer,
            client_id,
            "client_credentials",
            &sign_client_assertion(client_id, issuer, private_key_pem),
        )
        .await;
        assert_eq!(status, 200, "{token}");
        let new_kid = jsonwebtoken::decode_header(token["access_token"].as_str().unwrap())
            .unwrap()
            .kid
            .unwrap();
        if new_kid != kid {
            break new_kid;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the running gateway never picked up the rotated key"
        );
        tokio::time::sleep(StdDuration::from_millis(500)).await;
    };
    let rotated_jwks: serde_json::Value = http
        .get(format!("{issuer}/jwks.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let published: Vec<&str> = rotated_jwks["keys"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|key| key["kid"].as_str())
        .collect();
    assert!(
        published.contains(&kid) && published.contains(&new_kid.as_str()),
        "both the old and the new key publish during the overlap: {published:?}"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_served_provider_mints_a_verifiable_token_with_no_refresh_and_follows_a_rotation() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let app = seed_app(&db).await;
    let port = free_port();
    let issuer = format!("http://127.0.0.1:{port}");
    let deployment = Deployment::on_port(port, &op_block(&issuer, 100), "on").await;

    // 1. A fresh deployment has no active signing key, and `serve` refuses to
    //    start rather than fail on the first /token.
    deactivate_signing_keys(&db).await;
    let refused = deployment
        .serve_command()
        .output()
        .expect("running sms-gateway serve");
    assert!(
        !refused.status.success(),
        "serve must not start without a key"
    );
    assert!(
        stderr(&refused).contains("rotate-signing-key"),
        "{}",
        stderr(&refused)
    );

    let (client_id, private_key_pem) = operator_path(&deployment, &app, port);

    // 3. Serve it, and read the two public documents.
    let gateway = deployment.spawn().await;
    let http = reqwest::Client::new();
    let jwks = assert_public_documents(&http, &issuer).await;
    let keys = jwks["keys"].as_array().expect("a key set");

    // 4. A real exchange.
    let assertion = sign_client_assertion(&client_id, &issuer, &private_key_pem);
    let (status, token) = post_token(&issuer, &client_id, "client_credentials", &assertion).await;
    assert_eq!(status, 200, "{token}");
    assert_eq!(
        token["token_type"]
            .as_str()
            .map(str::to_lowercase)
            .as_deref(),
        Some("bearer")
    );
    assert_eq!(token["expires_in"], 900);
    assert!(
        token
            .get("refresh_token")
            .is_none_or(serde_json::Value::is_null),
        "a client_credentials grant issues no refresh token: {token}"
    );
    let access_token = token["access_token"].as_str().expect("an access token");
    let kid = assert_token_verifies(access_token, keys, &issuer, &client_id);

    // 5. The same assertion again is a replay (ClientAssertion's unique jti).
    let (replayed, body) = post_token(&issuer, &client_id, "client_credentials", &assertion).await;
    assert!(
        (400..500).contains(&replayed) && body.get("access_token").is_none(),
        "a replayed assertion is refused: {replayed} {body}"
    );

    // 6. The refresh grant is not available to a machine client.
    let fresh = sign_client_assertion(&client_id, &issuer, &private_key_pem);
    let (refresh_status, refresh_body) =
        post_token(&issuer, &client_id, "refresh_token", &fresh).await;
    assert!(
        (400..500).contains(&refresh_status) && refresh_body.get("access_token").is_none(),
        "the refresh grant is refused: {refresh_status} {refresh_body}"
    );

    // 7. A rotation reaches the running process within the refresh interval, the
    //    previous key keeps publishing, and a new token is signed by the new key.
    assert_rotation_reaches_the_process(
        &deployment,
        &http,
        &issuer,
        &client_id,
        &private_key_pem,
        &kid,
    )
    .await;

    // 8. The boot line says tokens are minted and not yet accepted (VSA-3b).
    let log = gateway.kill();
    assert!(log.contains("machine_tokens is served"), "{log}");
    assert!(log.contains("VSA-3b"), "{log}");
}

/// Pins the scope of this ticket: a token this OP mints is **not** a credential
/// of the API yet. When VSA-3b lands, this test is the one to rewrite.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_minted_token_is_not_yet_accepted_by_the_api() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let app = seed_app(&db).await;
    let port = free_port();
    let issuer = format!("http://127.0.0.1:{port}");
    let deployment = Deployment::on_port(port, &op_block(&issuer, 100), "scope").await;
    let _ = deployment.cli("rotate-signing-key", &[]);
    let key_out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("op-scope-{port}.pem"));
    let _ = std::fs::remove_file(&key_out);
    let provisioned = deployment.cli(
        "provision-client",
        &[
            "--app-id",
            &app.id,
            "--label",
            "scope probe",
            "--scope",
            "sms:send",
            "--key-out",
            key_out.to_str().expect("utf-8 path"),
        ],
    );
    assert!(provisioned.status.success(), "{}", stderr(&provisioned));
    let client_id = client_id_of(&provisioned);
    let pem = std::fs::read_to_string(&key_out).expect("the key file");

    let gateway = deployment.spawn().await;
    let (status, token) = post_token(
        &issuer,
        &client_id,
        "client_credentials",
        &sign_client_assertion(&client_id, &issuer, &pem),
    )
    .await;
    assert_eq!(status, 200, "{token}");
    let bearer = token["access_token"].as_str().expect("an access token");

    let response = reqwest::Client::new()
        .get(format!("{issuer}/messages"))
        .bearer_auth(bearer)
        .send()
        .await
        .expect("calling the API with the token");
    assert_eq!(
        response.status().as_u16(),
        401,
        "the API answers a bearer with the envelope layer's unsigned 401 until VSA-3b"
    );
    gateway.kill();
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_token_bucket_is_per_client_id_and_throttles_a_flood() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let port = free_port();
    let issuer = format!("http://127.0.0.1:{port}");
    let deployment = Deployment::on_port(port, &op_block(&issuer, 3), "limit").await;
    let _ = deployment.cli("rotate-signing-key", &[]);
    let gateway = deployment.spawn().await;

    let mut statuses = Vec::new();
    for _ in 0..5 {
        let (status, _) = post_token(&issuer, "appc_flood", "client_credentials", "x.y.z").await;
        statuses.push(status);
    }
    assert!(
        statuses[..3].iter().all(|status| *status != 429),
        "the burst is spent on the OP's own refusal first: {statuses:?}"
    );
    assert_eq!(
        &statuses[3..],
        [429, 429],
        "then the bucket throttles: {statuses:?}"
    );

    // Another client_id has its own bucket.
    let (other, _) = post_token(&issuer, "appc_other", "client_credentials", "x.y.z").await;
    assert_ne!(other, 429, "a different client_id is unaffected");
    gateway.kill();
}
