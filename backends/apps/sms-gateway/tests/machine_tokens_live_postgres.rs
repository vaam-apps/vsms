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
//! And since VSA-3b the API accepts the token it mints, as a bearer beside the
//! signed envelope: `a_minted_token_calls_a_real_procedure` sends and reads with
//! one; `a_token_that_is_not_ours_is_refused_whatever_is_wrong_with_it` signs
//! tokens with the deployment's own key (read from its table) to break each claim
//! in turn; `the_standing_is_read_from_rows_not_from_the_token` retires and
//! switches off the client behind a good token; `an_unknown_kid_reloads_the_key_set_once`
//! rotates a key behind a running process that refreshes hourly;
//! `machine_tokens_alone_boots_and_refuses_a_signature_and_a_key_registration`
//! shuts the signed door.
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
    op_block_refreshing(issuer, burst, 1)
}

/// [`op_block`] with the key refresh every `refresh_secs`.
fn op_block_refreshing(issuer: &str, burst: u32, refresh_secs: u32) -> String {
    // The overlap has to outlast a refresh and a token's lifetime (the loader
    // checks), which 1800 does for the one-second refresh and not for a slow one.
    let overlap = if refresh_secs > 60 {
        refresh_secs + 1800
    } else {
        1800
    };
    format!(
        "op:\n  issuer: {issuer}\n  token_rate_limit:\n    burst: {burst}\n    \
         refill_per_second: 0.01\n  signing_key:\n    refresh_interval_secs: {refresh_secs}\n    \
         rotation_overlap_secs: {overlap}\n"
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
        for (command, wording, args) in [
            (
                "rotate-signing-key",
                "mounts no OpenID Provider",
                Vec::<&str>::new(),
            ),
            (
                "provision-client",
                "issues no tokens",
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
            assert!(text.contains(wording), "{case}: {command}: {text}");
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
    post_token_asking(base_url, client_id, grant, assertion, Some("sms:send")).await
}

/// [`post_token`] asking for `scope`, or for none: a token minted without a
/// `scope` claim, which `/token` allows and which carries no scope of its own.
async fn post_token_asking(
    base_url: &str,
    client_id: &str,
    grant: &str,
    assertion: &str,
    scope: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut form = vec![
        ("grant_type", grant),
        ("client_id", client_id),
        (
            "client_assertion_type",
            "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
        ),
        ("client_assertion", assertion),
        ("refresh_token", "not-a-real-refresh-token"),
    ];
    if let Some(scope) = scope {
        form.push(("scope", scope));
    }
    let response = reqwest::Client::new()
        .post(format!("{base_url}/token"))
        .form(&form)
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
    // Refused by the grant gate (VSA-4a) before the provider reads the client: the
    // deployment serves `client_credentials` and nothing else, so the answer is
    // `unsupported_grant_type`, not the handler's `unauthorized_client` it was
    // before the gate existed. Either way no token is made.
    assert_eq!(
        refresh_body["error"], "unsupported_grant_type",
        "the refresh grant is not served: {refresh_body}"
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

    // 8. The boot line says tokens are minted and accepted as a bearer.
    let log = gateway.kill();
    assert!(log.contains("machine_tokens is served"), "{log}");
    assert!(log.contains("accepts them as a bearer"), "{log}");
    assert!(!log.contains("not yet"), "{log}");
}

// --- the API accepts the token (VSA-3b) -------------------------------------

/// A provisioned client of a running deployment: its id, its key and its app.
struct Machine {
    client_id: String,
    pem: String,
    app_id: String,
}

/// `rotate-signing-key` and `provision-client` for a fresh `App`, with `scopes`.
async fn provision(
    db: &Cratestack,
    deployment: &Deployment,
    tag: &str,
    scopes: &[&str],
) -> Machine {
    let app = seed_app(db).await;
    let _ = deployment.cli("rotate-signing-key", &[]);
    let key_out =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("op-{tag}-{}.pem", deployment.port));
    let _ = std::fs::remove_file(&key_out);
    let mut args = vec![
        "--app-id",
        app.id.as_str(),
        "--label",
        "bearer probe",
        "--key-out",
        key_out.to_str().expect("utf-8 path"),
    ];
    for scope in scopes {
        args.push("--scope");
        args.push(scope);
    }
    let provisioned = deployment.cli("provision-client", &args);
    assert!(provisioned.status.success(), "{}", stderr(&provisioned));
    Machine {
        client_id: client_id_of(&provisioned),
        pem: std::fs::read_to_string(&key_out).expect("the key file"),
        app_id: app.id,
    }
}

/// A real access token from the running deployment's `/token`.
async fn mint(base_url: &str, machine: &Machine) -> String {
    let (status, token) = post_token(
        base_url,
        &machine.client_id,
        "client_credentials",
        &sign_client_assertion(&machine.client_id, base_url, &machine.pem),
    )
    .await;
    assert_eq!(status, 200, "{token}");
    token["access_token"]
        .as_str()
        .expect("an access token")
        .to_owned()
}

/// A real access token minted asking for no scope, so it carries no `scope`
/// claim and acts with whatever the rows allow.
async fn mint_unscoped(base_url: &str, machine: &Machine) -> String {
    let (status, token) = post_token_asking(
        base_url,
        &machine.client_id,
        "client_credentials",
        &sign_client_assertion(&machine.client_id, base_url, &machine.pem),
        None,
    )
    .await;
    assert_eq!(status, 200, "{token}");
    assert!(
        token.get("scope").is_none_or(serde_json::Value::is_null),
        "an unscoped request gets an unscoped token: {token}"
    );
    token["access_token"]
        .as_str()
        .expect("an access token")
        .to_owned()
}

/// The status, content type and body of an API call made with `token` as a bearer.
async fn api(
    base_url: &str,
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<&serde_json::Value>,
) -> (u16, Option<String>, String) {
    let mut request = reqwest::Client::new()
        .request(method, format!("{base_url}{path}"))
        .bearer_auth(token)
        .header("accept", "application/json");
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .body(serde_json::to_vec(body).expect("JSON"));
    }
    let response = request.send().await.expect("calling the API with a token");
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    (
        status,
        content_type,
        response.text().await.unwrap_or_default(),
    )
}

/// `previewMessage` with `token`: a procedure that needs no scope, so only the
/// credential decides.
async fn preview_with(base_url: &str, token: &str) -> (u16, Option<String>, String) {
    api(
        base_url,
        reqwest::Method::POST,
        "/$procs/previewMessage",
        token,
        Some(&serde_json::json!({"args": {"body": "hello"}})),
    )
    .await
}

/// What the layer says to anything it will not authenticate, byte for byte.
const COARSE_REFUSAL: &str =
    r#"{"code":"UNAUTHORIZED","message":"request could not be authenticated","details":null}"#;

/// The real thing: a token the running OP minted sends a message and reads it
/// back, as plain JSON, with the message in the client's own app.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_minted_token_calls_a_real_procedure() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let port = free_port();
    let issuer = format!("http://127.0.0.1:{port}");
    let deployment = Deployment::on_port(port, &op_block(&issuer, 100), "call").await;
    let machine = provision(&db, &deployment, "call", &["sms:send", "sms:read"]).await;
    let sender = sealed_fixtures::seed_sender(&db).await;
    let gateway = deployment.spawn().await;
    // The send is made with the token that asked for `sms:send` alone (`mint`),
    // as before; reading the message back needs `sms:read`, so that is done with
    // an unscoped token, which acts with all the client holds. The narrower one
    // is refused the read below.
    let token = mint_unscoped(&issuer, &machine).await;
    let send_only = mint(&issuer, &machine).await;

    let client_ref = format!("minted-{}", unique_suffix());
    let (status, content_type, body) = api(
        &issuer,
        reqwest::Method::POST,
        "/$procs/sendMessage",
        &send_only,
        Some(&serde_json::json!({"args": {
            "to": sealed_fixtures::unique_mtn_msisdn(),
            "body": "sent with a minted token",
            "senderId": sender,
            "clientRef": client_ref,
        }})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(
        content_type
            .as_deref()
            .unwrap_or_default()
            .starts_with("application/json"),
        "a token caller is not sealed: {content_type:?}"
    );
    let sent: serde_json::Value = serde_json::from_str(&body).expect("plain JSON");
    assert!(sent["messageId"].is_string(), "{body}");

    let (status, _, body) = api(&issuer, reqwest::Method::GET, "/messages", &token, None).await;
    assert_eq!(status, 200, "{body}");
    let listed: serde_json::Value = serde_json::from_str(&body).expect("plain JSON");
    let items = listed["items"].as_array().expect("a page");
    assert!(
        items.iter().any(
            |item| item["id"] == sent["messageId"] && item["appId"] == machine.app_id.as_str()
        ),
        "the message is the client's app's: {body}"
    );

    // Nothing it was not given.
    let (status, _, body) = api(&issuer, reqwest::Method::GET, "/jobs", &token, None).await;
    assert_eq!(status, 403, "job:read was never provisioned: {body}");

    // A token that asked for `sms:send` alone sends but cannot read the
    // message back, though the client holds `sms:read`: a scope narrows.
    let (status, _, body) = api(&issuer, reqwest::Method::GET, "/messages", &send_only, None).await;
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("sms:read"), "{body}");
    gateway.kill();
}

/// Signs tokens with a signing key read out of the deployment's table (the test
/// holds the database, as the gateway does): a token exactly as good as one the
/// OP minted, except for whatever the test breaks.
struct Forge {
    kid: String,
    pem: String,
}

impl Forge {
    fn from_row(row: schema::OauthSigningKey) -> Self {
        Self {
            kid: row.id,
            pem: row.privateKeyPem,
        }
    }

    async fn by_id(db: &Cratestack, kid: &str) -> Self {
        Self::from_row(
            db.oauth_signing_key()
                .find_unique(kid.to_owned())
                .run(&context("system"))
                .await
                .expect("reading the signing key")
                .expect("the signing key exists"),
        )
    }

    async fn active(db: &Cratestack) -> Self {
        Self::from_row(
            db.oauth_signing_key()
                .find_many()
                .where_expr(FilterExpr::from(
                    sms_api::schema::oauth_signing_key::active().is_true(),
                ))
                .order_by(sms_api::schema::oauth_signing_key::createdAt().desc())
                .limit(1)
                .run(&context("system"))
                .await
                .expect("reading the signing keys")
                .into_iter()
                .next()
                .expect("an active signing key"),
        )
    }

    fn sign(&self, kid: Option<&str>, claims: &serde_json::Value) -> String {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = kid.map(str::to_owned);
        jsonwebtoken::encode(
            &header,
            claims,
            &jsonwebtoken::EncodingKey::from_rsa_pem(self.pem.as_bytes()).expect("the key"),
        )
        .expect("signing a forged token")
    }

    /// The claims `authkestra` stamps, each overridable (`null` removes it).
    fn claims(issuer: &str, client_id: &str, patch: &serde_json::Value) -> serde_json::Value {
        let now = chrono::Utc::now().timestamp();
        let mut claims = serde_json::json!({
            "iss": issuer, "sub": client_id, "aud": client_id,
            "exp": now + 900, "iat": now, "scope": "sms:send",
        });
        for (name, value) in patch.as_object().expect("a patch is an object") {
            if value.is_null() {
                claims.as_object_mut().unwrap().remove(name);
            } else {
                claims[name] = value.clone();
            }
        }
        claims
    }
}

/// Every way a token can be wrong is the layer's own coarse 401, byte for byte,
/// and the control, the same forgery with nothing broken, is accepted: the
/// forging itself is not what is refused.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_token_that_is_not_ours_is_refused_whatever_is_wrong_with_it() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let port = free_port();
    let issuer = format!("http://127.0.0.1:{port}");
    let deployment = Deployment::on_port(port, &op_block(&issuer, 100), "forged").await;
    let machine = provision(&db, &deployment, "forged", &["sms:send"]).await;
    let gateway = deployment.spawn().await;
    let forge = Forge::active(&db).await;
    let now = chrono::Utc::now().timestamp();
    let id = machine.client_id.as_str();
    let patched = |patch: serde_json::Value| Forge::claims(&issuer, id, &patch);

    // A key the deployment has never heard of.
    let stranger = Forge {
        kid: "kid-the-deployment-never-had".to_owned(),
        pem: rsa::pkcs8::EncodePrivateKey::to_pkcs8_pem(
            &rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).expect("a key"),
            rsa::pkcs8::LineEnding::LF,
        )
        .expect("a PEM")
        .to_string(),
    };
    let person = serde_json::json!({
        "provider_id": "console", "external_id": "usr1",
        "email": null, "username": null, "attributes": {}
    });
    let good = patched(serde_json::json!({}));
    let kid = Some(forge.kid.as_str());

    let cases: Vec<(&str, String)> = vec![
        (
            "wrong issuer",
            forge.sign(
                kid,
                &Forge::claims("https://evil.example", id, &serde_json::json!({})),
            ),
        ),
        (
            "no issuer",
            forge.sign(kid, &patched(serde_json::json!({"iss": null}))),
        ),
        (
            "audience is another resource",
            forge.sign(kid, &patched(serde_json::json!({"aud": "vsms"}))),
        ),
        (
            "audience leaves out sub",
            forge.sign(kid, &patched(serde_json::json!({"aud": ["a", "b"]}))),
        ),
        (
            "no audience",
            forge.sign(kid, &patched(serde_json::json!({"aud": null}))),
        ),
        (
            "expired",
            forge.sign(kid, &patched(serde_json::json!({"exp": now - 3600}))),
        ),
        (
            "not valid yet",
            forge.sign(kid, &patched(serde_json::json!({"nbf": now + 3600}))),
        ),
        (
            "no expiry",
            forge.sign(kid, &patched(serde_json::json!({"exp": null}))),
        ),
        ("no kid", forge.sign(None, &good)),
        (
            "a kid nobody has",
            forge.sign(Some("kid-nobody-has"), &good),
        ),
        (
            "signed by a stranger's key under our kid",
            stranger.sign(kid, &good),
        ),
        (
            "signed by a stranger's key under its own kid",
            stranger.sign(Some(&stranger.kid), &good),
        ),
        (
            "a person's token",
            forge.sign(kid, &patched(serde_json::json!({"identity": person}))),
        ),
        ("not a JWT", "not-a-token".to_owned()),
    ];
    for (case, token) in &cases {
        let (status, _, body) = preview_with(&issuer, token).await;
        assert_eq!(status, 401, "{case}: {body}");
        assert_eq!(body, COARSE_REFUSAL, "{case}");
    }

    // The control: nothing broken.
    let (status, _, body) = preview_with(&issuer, &forge.sign(kid, &good)).await;
    assert_eq!(status, 200, "{body}");
    gateway.kill();
}

/// What a good token may do is read from rows on every request: switching off
/// the `OauthClient` ends it on the very next call, and a client whose
/// `AppClient` is retired has no standing however the token verifies.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_standing_is_read_from_rows_not_from_the_token() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let port = free_port();
    let issuer = format!("http://127.0.0.1:{port}");
    let deployment = Deployment::on_port(port, &op_block(&issuer, 100), "standing").await;
    let live = provision(&db, &deployment, "standing-live", &["sms:send"]).await;
    let retired = provision(&db, &deployment, "standing-retired", &["sms:send"]).await;
    let gateway = deployment.spawn().await;
    let live_token = mint(&issuer, &live).await;
    let retired_token = mint(&issuer, &retired).await;

    // OauthClient.active: read on every request.
    assert_eq!(preview_with(&issuer, &live_token).await.0, 200);
    let oauth = db
        .oauth_client()
        .find_many()
        .where_expr(FilterExpr::from(
            sms_api::schema::oauth_client::clientId().eq(live.client_id.clone()),
        ))
        .limit(1)
        .run(&context("system"))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the oauth client");
    for (active, expected) in [(false, 401), (true, 200)] {
        db.oauth_client()
            .update(oauth.id.clone())
            .set(schema::UpdateOauthClientInput {
                active: Some(active),
                ..Default::default()
            })
            .run(&context("system"))
            .await
            .expect("flipping OauthClient.active");
        let (status, _, body) = preview_with(&issuer, &live_token).await;
        assert_eq!(status, expected, "OauthClient.active = {active}: {body}");
        if !active {
            assert_eq!(body, COARSE_REFUSAL);
        }
    }

    // AppClient.retiredAt: a token minted while the client was live, used after.
    let app_client = db
        .app_client()
        .find_many()
        .where_expr(FilterExpr::from(
            sms_api::schema::app_client::clientId().eq(retired.client_id.clone()),
        ))
        .limit(1)
        .run(&context("system"))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the app client");
    db.app_client()
        .update(app_client.id)
        .set(schema::UpdateAppClientInput {
            retiredAt: Some(Some(chrono::Utc::now())),
            ..Default::default()
        })
        .if_match(app_client.version)
        .run(&context("owner"))
        .await
        .expect("retiring the app client");
    let (status, _, body) = preview_with(&issuer, &retired_token).await;
    assert_eq!(
        (status, body.as_str()),
        (401, COARSE_REFUSAL),
        "AppClient.retiredAt"
    );
    gateway.kill();
}

/// A token acts with the `AppClient`'s scopes **that its `OauthClient` also
/// lists**. `/token` checks only a requested scope against the `OauthClient`, so
/// a token minted with no `scope` claim carries none, and an admin adding
/// `job:read` to the `AppClient` alone must not hand it to the client's tokens.
/// Listing it on the `OauthClient` too is what grants it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_app_client_scope_the_oauth_client_lacks_is_not_granted_to_a_minted_token() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let port = free_port();
    let issuer = format!("http://127.0.0.1:{port}");
    let deployment = Deployment::on_port(port, &op_block(&issuer, 100), "ceiling").await;
    let machine = provision(&db, &deployment, "ceiling", &["sms:send"]).await;
    let gateway = deployment.spawn().await;
    // Minted before the AppClient is touched, asking for no scope.
    let token = mint_unscoped(&issuer, &machine).await;

    let app_client = db
        .app_client()
        .find_many()
        .where_expr(FilterExpr::from(
            sms_api::schema::app_client::clientId().eq(machine.client_id.clone()),
        ))
        .limit(1)
        .run(&context("system"))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the app client");
    db.app_client()
        .update(app_client.id.clone())
        .set(schema::UpdateAppClientInput {
            scopes: Some(" sms:send job:read ".to_owned()),
            ..Default::default()
        })
        .if_match(app_client.version)
        .run(&context("owner"))
        .await
        .expect("adding job:read to the AppClient alone");

    let (status, _, body) = api(&issuer, reqwest::Method::GET, "/jobs", &token, None).await;
    assert_eq!(
        status, 403,
        "job:read is on the AppClient only, so the token does not carry it: {body}"
    );
    assert_eq!(
        preview_with(&issuer, &token).await.0,
        200,
        "still a live token"
    );

    // Listing it on the OauthClient row, which is read on every request, grants it.
    let oauth = db
        .oauth_client()
        .find_many()
        .where_expr(FilterExpr::from(
            sms_api::schema::oauth_client::clientId().eq(machine.client_id.clone()),
        ))
        .limit(1)
        .run(&context("system"))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the oauth client");
    db.oauth_client()
        .update(oauth.id)
        .set(schema::UpdateOauthClientInput {
            scopes: Some(" sms:send job:read ".to_owned()),
            ..Default::default()
        })
        .run(&context("system"))
        .await
        .expect("listing job:read on the OauthClient too");
    let (status, _, body) = api(&issuer, reqwest::Method::GET, "/jobs", &token, None).await;
    assert_eq!(status, 200, "both rows list job:read: {body}");
    gateway.kill();
}

/// A key another process rotated in, noticed before the next periodic refresh does: the
/// unknown `kid` forces one reload of the key set and the token verifies.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_unknown_kid_reloads_the_key_set_once() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let port = free_port();
    let issuer = format!("http://127.0.0.1:{port}");
    // Refreshing every twenty minutes: the periodic refresh cannot be what makes this pass.
    let deployment =
        Deployment::on_port(port, &op_block_refreshing(&issuer, 100, 1200), "reload").await;
    let machine = provision(&db, &deployment, "reload", &["sms:send"]).await;
    let gateway = deployment.spawn().await;
    let old = mint(&issuer, &machine).await;
    assert_eq!(preview_with(&issuer, &old).await.0, 200);

    // Another process rotates: a new key the running gateway has not loaded.
    let rotated = deployment.cli("rotate-signing-key", &[]);
    assert!(rotated.status.success(), "{}", stderr(&rotated));
    let new_kid = stdout(&rotated)
        .lines()
        .find_map(|line| line.strip_prefix("rotated: new signing key "))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("the new key id")
        .to_owned();
    let forge = Forge::by_id(&db, &new_kid).await;
    let token = forge.sign(
        Some(&new_kid),
        &Forge::claims(&issuer, &machine.client_id, &serde_json::json!({})),
    );
    let (status, _, body) = preview_with(&issuer, &token).await;
    assert_eq!(status, 200, "the forced reload found {new_kid}: {body}");

    // And the key it loaded is the published one.
    let jwks: serde_json::Value = reqwest::get(format!("{issuer}/jwks.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        jwks["keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key["kid"] == new_kid.as_str()),
        "{jwks}"
    );
    // The token signed by the key that was current still verifies in the overlap.
    assert_eq!(preview_with(&issuer, &old).await.0, 200);
    gateway.kill();
}

/// `auth.methods: [keypair]`: no provider, so no token to present; a bearer is
/// the layer's own 401.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_keypair_alone_a_bearer_is_a_401() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let deployment = Deployment::new("auth:\n  methods: [keypair]\n", "keypair-only").await;
    let gateway = deployment.spawn().await;
    let (status, _, body) = preview_with(&gateway.base_url, "any-token-at-all").await;
    assert_eq!((status, body.as_str()), (401, COARSE_REFUSAL));
    gateway.kill();
}

/// `auth.methods: [machine_tokens]`: boots (the list is legal since VSA-3b), a
/// token works, a signed body is the layer's 415, a key registration is a 422,
/// and the two key-registering commands refuse and write nothing; with `keypair`
/// permitted the same command runs.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn machine_tokens_alone_boots_and_refuses_a_signature_and_a_key_registration() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    ensure_orange_cm_provider(&db).await;
    let port = free_port();
    let issuer = format!("http://127.0.0.1:{port}");
    let tokens_only = Deployment::on_port(
        port,
        &format!(
            "auth:\n  methods: [machine_tokens]\n{}",
            op_block(&issuer, 100)
        ),
        "tokens-only",
    )
    .await;
    let machine = provision(&db, &tokens_only, "tokens-only", &["sms:send"]).await;
    let gateway = tokens_only.spawn().await;
    let token = mint(&issuer, &machine).await;

    let (status, _, body) = preview_with(&issuer, &token).await;
    assert_eq!(status, 200, "{body}");

    // A signed request is refused by the layer, whoever signed it.
    let key = sms_test_support::sealed::TestKey::app();
    let signed = sealed_fixtures::client(&issuer, &key)
        .request("POST", "/$procs/previewMessage")
        .json(&serde_json::json!({"args": {"body": "hello"}}))
        .send()
        .await;
    assert_eq!(signed.status, 415, "{}", signed.text());
    assert!(!signed.sealed);

    // A key registration is a 422 for the one caller who can reach the route.
    for (procedure, mention) in [
        ("registerAppClientKey", "provision-client"),
        ("registerConsoleKey", "VSA-5"),
    ] {
        let (status, _, body) = api(
            &issuer,
            reqwest::Method::POST,
            &format!("/$procs/{procedure}"),
            &token,
            Some(&serde_json::json!({"args": {}})),
        )
        .await;
        assert_eq!(status, 422, "{procedure}: {body}");
        assert!(
            body.contains("auth_method_disabled") && body.contains(mention),
            "{body}"
        );
    }
    gateway.kill();

    // The commands refuse with the deployment's configuration, before the database.
    let app = seed_app(&db).await;
    let before = count_rows(&db).await;
    let public_key = key.public_key_b64u();
    let console_key = sms_test_support::sealed::TestKey::console().public_key_b64u();
    let refused = tokens_only.cli(
        "register-client",
        &["--app", &app.slug, "--public-key", &public_key],
    );
    let text = stderr(&refused);
    assert!(!refused.status.success(), "register-client must refuse");
    assert!(
        text.contains("auth_method_disabled") && text.contains("provision-client"),
        "{text}"
    );
    let refused = tokens_only.cli(
        "pair-console-key",
        &[
            "--user",
            "nobody@example.invalid",
            "--public-key",
            &console_key,
        ],
    );
    let text = stderr(&refused);
    assert!(!refused.status.success(), "pair-console-key must refuse");
    assert!(
        text.contains("auth_method_disabled") && text.contains("VSA-5"),
        "{text}"
    );
    assert_eq!(count_rows(&db).await, before, "a refusal writes nothing");

    // The control: a configuration that permits keypair runs the same command.
    let both = Deployment::new(
        &format!(
            "auth:\n  methods: [keypair, machine_tokens]\n{}",
            op_block(&issuer, 100)
        ),
        "both",
    )
    .await;
    let registered = both.cli(
        "register-client",
        &[
            "--app",
            &app.slug,
            "--public-key",
            &public_key,
            "--scope",
            "sms:send",
        ],
    );
    assert!(registered.status.success(), "{}", stderr(&registered));
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
