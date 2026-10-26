//! `sms-gateway register-client` (VS-2) as a real, separate OS process against
//! a real Postgres: the operator-facing half of `registerAppClientKey`.
//!
//! Two fast tests cover input the command refuses before it touches the
//! database; the live ones prove the claims that matter - running it twice
//! leaves one key and exits 0, the app's signing client is created once and
//! shared by every key, and a revoked key or an unknown app is refused with a
//! non-zero exit and nothing written.
//!
//! ```bash
//! cargo test -p sms-gateway --test register_client_cli_live_postgres -- --ignored --nocapture
//! ```

use std::process::{Command, Output};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack, app_client, client_key};

/// See `provision_client_cli_live_postgres.rs`'s identical mutex (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn context(role: &str) -> CratestackContext {
    Principal {
        sub: format!("register-client-cli-test-{role}"),
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

/// A distinct public key per call. The seed only has to differ; the private
/// half is dropped on the spot, as it would be on the machine that owns it.
fn fresh_public_key() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .as_nanos();
    let mut seed = [0_u8; 32];
    seed[..8].copy_from_slice(&n.to_le_bytes());
    seed[8..24].copy_from_slice(&nanos.to_le_bytes());
    seed[24..28].copy_from_slice(&std::process::id().to_le_bytes());
    let public = cratestack::cose::Ed25519Signer::from_seed(&seed)
        .verify_key()
        .ed25519_bytes()
        .expect("an Ed25519 signer's verify key is Ed25519");
    URL_SAFE_NO_PAD.encode(public)
}

async fn db() -> (Cratestack, String) {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    (Cratestack::builder(pool).build(), url)
}

async fn seed_app(db: &Cratestack) -> String {
    let slug = format!("reg-client-{}", unique_suffix());
    db.app()
        .create(schema::CreateAppInput {
            name: "register-client cli test app".to_owned(),
            slug: slug.clone(),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&context("owner"))
        .await
        .expect("seeding the app");
    slug
}

fn run(database_url: &str, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
        .arg("register-client")
        .arg("--database-url")
        .arg(database_url)
        .args(extra)
        .output()
        .expect("running `sms-gateway register-client`")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A `key: value` line of the command's documented stdout.
fn field(stdout: &str, name: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}: ")))
        .unwrap_or_else(|| panic!("expected a \"{name}: ...\" line in stdout, got: {stdout:?}"))
        .trim()
        .to_owned()
}

async fn client_rows(db: &Cratestack, client_id: &str) -> Vec<schema::AppClient> {
    db.app_client()
        .find_many()
        .where_expr(FilterExpr::from(
            app_client::clientId().eq(client_id.to_owned()),
        ))
        .run(&context("system"))
        .await
        .expect("reading app clients back")
}

async fn key_rows(db: &Cratestack, app_client_id: &str) -> Vec<schema::ClientKey> {
    db.client_key()
        .find_many()
        .where_expr(FilterExpr::from(
            client_key::appClientId().eq(app_client_id.to_owned()),
        ))
        .run(&context("system"))
        .await
        .expect("reading client keys back")
}

#[test]
fn an_invalid_role_is_refused_before_the_database_is_touched() {
    let output = run(
        "postgres://never-contacted.invalid/vsms",
        &[
            "--app",
            "anything",
            "--public-key",
            &fresh_public_key(),
            "--role",
            "developer",
        ],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("--role must be"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_malformed_public_key_is_refused_before_the_database_is_touched() {
    for bad in ["", "not-a-key", "AAAA"] {
        let output = run(
            "postgres://never-contacted.invalid/vsms",
            &["--app", "anything", "--public-key", bad],
        );
        assert!(!output.status.success(), "{bad:?} must be refused");
        assert!(
            stderr(&output).contains("--public-key"),
            "{bad:?}: {}",
            stderr(&output)
        );
    }
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn running_it_twice_leaves_one_key_and_exits_zero_both_times() {
    let _guard = TEST_MUTEX.lock().await;
    let (db, url) = db().await;
    let slug = seed_app(&db).await;
    let key = fresh_public_key();
    let args = [
        "--app",
        &slug,
        "--public-key",
        &key,
        "--label",
        "composer",
        "--scope",
        "sms:send",
    ];

    let first = run(&url, &args);
    assert!(first.status.success(), "first run: {}", stderr(&first));
    let second = run(&url, &args);
    assert!(second.status.success(), "second run: {}", stderr(&second));

    let (first_out, second_out) = (stdout(&first), stdout(&second));
    assert_eq!(field(&first_out, "key id"), field(&second_out, "key id"));
    assert_eq!(
        field(&first_out, "thumbprint"),
        field(&second_out, "thumbprint")
    );
    assert_eq!(field(&first_out, "thumbprint").len(), 64);
    assert!(
        field(&first_out, "app client").ends_with("(created)"),
        "{first_out}"
    );
    assert!(
        field(&second_out, "app client").ends_with("(existing)"),
        "{second_out}"
    );

    let client_id = format!("appc_{slug}");
    let clients = client_rows(&db, &client_id).await;
    assert_eq!(clients.len(), 1, "one signing client per app");
    assert_eq!(clients[0].scopes, " sms:send ");
    let keys = key_rows(&db, &clients[0].id).await;
    assert_eq!(keys.len(), 1, "the same key twice is one row");
    assert_eq!(keys[0].id, field(&first_out, "key id"));
    assert_eq!(keys[0].label.as_deref(), Some("composer"));
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_second_key_for_the_app_joins_the_same_client() {
    let _guard = TEST_MUTEX.lock().await;
    let (db, url) = db().await;
    let slug = seed_app(&db).await;

    let one = run(
        &url,
        &[
            "--app",
            &slug,
            "--public-key",
            &fresh_public_key(),
            "--scope",
            "sms:send",
        ],
    );
    assert!(one.status.success(), "{}", stderr(&one));
    // No --scope this time: the client exists, so none is needed.
    let two = run(&url, &["--app", &slug, "--public-key", &fresh_public_key()]);
    assert!(two.status.success(), "{}", stderr(&two));

    let clients = client_rows(&db, &format!("appc_{slug}")).await;
    assert_eq!(clients.len(), 1);
    assert_eq!(key_rows(&db, &clients[0].id).await.len(), 2);
    assert_ne!(
        field(&stdout(&one), "thumbprint"),
        field(&stdout(&two), "thumbprint")
    );
}

/// A named client is a second principal of the same App: its own row, its own
/// scopes, found again by name (idempotent) and never confused with the App's
/// default client.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_named_client_is_a_second_principal_of_the_same_app() {
    let _guard = TEST_MUTEX.lock().await;
    let (db, url) = db().await;
    let slug = seed_app(&db).await;
    let key = fresh_public_key();

    let default = run(
        &url,
        &[
            "--app",
            &slug,
            "--public-key",
            &fresh_public_key(),
            "--scope",
            "sms:send",
        ],
    );
    assert!(default.status.success(), "{}", stderr(&default));
    let named = run(
        &url,
        &[
            "--app",
            &slug,
            "--client-name",
            "console",
            "--public-key",
            &key,
            "--scope",
            "sms:read",
        ],
    );
    assert!(named.status.success(), "{}", stderr(&named));
    // Again, with no scope: the named client exists now.
    let again = run(
        &url,
        &[
            "--app",
            &slug,
            "--client-name",
            "console",
            "--public-key",
            &key,
        ],
    );
    assert!(again.status.success(), "{}", stderr(&again));

    let default_rows = client_rows(&db, &format!("appc_{slug}")).await;
    let named_rows = client_rows(&db, &format!("appc_{slug}_console")).await;
    assert_eq!(default_rows.len(), 1);
    assert_eq!(named_rows.len(), 1);
    assert_ne!(default_rows[0].id, named_rows[0].id, "two principals");
    assert_eq!(default_rows[0].appId, named_rows[0].appId, "one App");
    assert!(named_rows[0].scopes.contains("sms:read"));
    assert!(!named_rows[0].scopes.contains("sms:send"));
    assert_eq!(
        key_rows(&db, &named_rows[0].id).await.len(),
        1,
        "the key joined once"
    );
    assert_eq!(key_rows(&db, &default_rows[0].id).await.len(), 1);
}

#[test]
fn a_malformed_client_name_is_refused_before_the_database_is_touched() {
    let output = run(
        "postgres://unused:unused@127.0.0.1:1/none",
        &[
            "--app",
            "an-app",
            "--client-name",
            "Not Valid",
            "--public-key",
            &fresh_public_key(),
            "--scope",
            "sms:send",
        ],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("--client-name"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn an_unknown_app_or_a_first_run_without_scopes_writes_nothing() {
    let _guard = TEST_MUTEX.lock().await;
    let (db, url) = db().await;
    let key = fresh_public_key();

    let unknown = run(
        &url,
        &[
            "--app",
            "no-such-app-slug",
            "--public-key",
            &key,
            "--scope",
            "sms:send",
        ],
    );
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains("no App with slug"),
        "{}",
        stderr(&unknown)
    );
    assert_eq!(client_rows(&db, "appc_no-such-app-slug").await.len(), 0);

    let slug = seed_app(&db).await;
    let unscoped = run(&url, &["--app", &slug, "--public-key", &key]);
    assert!(!unscoped.status.success());
    assert!(
        stderr(&unscoped).contains("--scope"),
        "{}",
        stderr(&unscoped)
    );
    assert!(
        client_rows(&db, &format!("appc_{slug}")).await.is_empty(),
        "a refused first run must not leave a client with no scopes behind"
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_revoked_key_is_refused_with_a_non_zero_exit() {
    let _guard = TEST_MUTEX.lock().await;
    let (db, url) = db().await;
    let slug = seed_app(&db).await;
    let key = fresh_public_key();
    let args = ["--app", &slug, "--public-key", &key, "--scope", "sms:send"];
    let first = run(&url, &args);
    assert!(first.status.success(), "{}", stderr(&first));

    // Revocation is the procedure's, not the CLI's; stamp it the way
    // `revokeAppClientKey` does (system-only column).
    let key_id = field(&stdout(&first), "key id");
    db.client_key()
        .update(key_id)
        .set(schema::UpdateClientKeyInput {
            revokedAt: Some(Some(chrono::Utc::now())),
            ..Default::default()
        })
        .run(&context("system"))
        .await
        .expect("revoking as system");

    let again = run(&url, &args);
    assert!(!again.status.success(), "a revoked key is never reinstated");
    assert!(stderr(&again).contains("revoked"), "{}", stderr(&again));
}
