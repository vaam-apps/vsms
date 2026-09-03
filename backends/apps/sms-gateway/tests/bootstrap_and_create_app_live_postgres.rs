//! Production-readiness audit N1/N2: `sms-gateway create-app` and
//! `sms-gateway bootstrap` — both real CLI subcommands (`main.rs`'s
//! `Command::CreateApp`/`Command::Bootstrap`), not `Procedures` called
//! in-process. Proves the two claims `docs/runbooks/deployment.adoc`'s
//! step 3/step 5 now make:
//!
//! - `bootstrap` on a fresh database creates an active OP signing key, an
//!   active `orange_cm` `Provider` with a catch-all `Route`, the
//!   `sms-console` `OauthClient`, and the first operator `User` +
//!   `UserCredential` — in one call.
//! - A second run against an already-bootstrapped database is a clean
//!   no-op: no second signing key (the one genuinely non-idempotent step
//!   `bootstrap` deliberately skips outright rather than re-running), no
//!   duplicate `Provider`/`Route`/`OauthClient`/`User` row.
//! - `create-app` is idempotent the same way `seed-dispatch` already is:
//!   `create` + catch `23505` on `App.slug`, returning the same id both
//!   times.
//!
//! ```bash
//! cargo test -p sms-gateway --test bootstrap_and_create_app_live_postgres -- --ignored --nocapture
//! ```

use std::process::{Command, Output};

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack, app as app_filter, oauth_client as oauth_client_filter};

/// #102: within one test binary, Rust's default multi-threaded harness
/// can race two tests preparing the same not-yet-cached query shape
/// against Postgres's own `pg_type` catalog at the same instant — see
/// `backends/crates/sms-worker/tests/claim_live_postgres.rs`'s own
/// `TEST_MUTEX` doc for the full mechanism. Taken regardless of how many
/// live tests this file ends up with, per AGENTS.md's "any new live
/// suite must take the per-binary mutex" rule.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// #134: `bootstrap`/`create-app` never hash anything themselves, but
/// `Procedures::new` (which `create-app` goes through for `App.create`)
/// takes a `HashPepper` unconditionally — see
/// `provision_client_cli_live_postgres.rs`'s own identical constant for
/// the same reasoning. Only the value's length matters.
const TEST_HASH_PEPPER: &str = "bootstrap-and-create-app-cli-live-postgres-test-pepper-over-min";

fn sys() -> CratestackContext {
    Principal {
        sub: "bootstrap-cli-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
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

fn run_bootstrap_cli(
    database_url: &str,
    console_client_id: &str,
    console_redirect_uri: &str,
    owner_email: &str,
    owner_display_name: &str,
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
        .arg("bootstrap")
        .arg("--database-url")
        .arg(database_url)
        .arg("--console-client-id")
        .arg(console_client_id)
        .arg("--console-redirect-uri")
        .arg(console_redirect_uri)
        .arg("--owner-email")
        .arg(owner_email)
        .arg("--owner-display-name")
        .arg(owner_display_name)
        .env("SMS_HASH_PEPPER", TEST_HASH_PEPPER)
        .output()
        .expect("running `sms-gateway bootstrap`")
}

fn run_create_app_cli(database_url: &str, slug: &str, name: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
        .arg("create-app")
        .arg("--database-url")
        .arg(database_url)
        .arg("--slug")
        .arg(slug)
        .arg("--name")
        .arg(name)
        .env("SMS_HASH_PEPPER", TEST_HASH_PEPPER)
        .output()
        .expect("running `sms-gateway create-app`")
}

/// `create-app`'s own documented stdout contract
/// (`main.rs::create_app_command`): a line reading exactly
/// `app id: <id>`.
fn parse_app_id(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("app id: "))
        .unwrap_or_else(|| panic!("expected an \"app id: <id>\" line in stdout, got: {stdout:?}"))
        .trim()
        .to_owned()
}

async fn active_signing_key_ids(db: &Cratestack) -> Vec<String> {
    db.oauth_signing_key()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::oauth_signing_key::active().is_true(),
        ))
        .run(&sys())
        .await
        .expect("reading back OauthSigningKey rows")
        .into_iter()
        .map(|row| row.id)
        .collect()
}

async fn signing_key_row_count(db: &Cratestack) -> usize {
    db.oauth_signing_key()
        .find_many()
        .run(&sys())
        .await
        .expect("reading back every OauthSigningKey row")
        .len()
}

struct BootstrapFixture<'a> {
    console_client_id: &'a str,
    redirect_uri: &'a str,
    owner_email: &'a str,
    owner_display_name: &'a str,
}

/// The first half of the live test: run `bootstrap` once against a fresh
/// database and assert every artifact it's supposed to create actually
/// landed. Pulled out purely to keep the test function itself under
/// `clippy::too_many_lines` — same reason `main.rs`'s own multi-step
/// command functions are split into per-step helpers.
async fn assert_first_bootstrap_run_creates_everything(
    db: &Cratestack,
    db_url: &str,
    fixture: &BootstrapFixture<'_>,
) -> (Vec<String>, schema::Provider) {
    // A fresh, just-migrated per-binary database (sms_test_support's own
    // guarantee) starts with zero rows in every table this test cares
    // about — asserted, not assumed, since the whole point of the
    // idempotency half that follows is a *delta* of zero on the second
    // run.
    assert_eq!(
        signing_key_row_count(db).await,
        0,
        "a fresh database must start with no OauthSigningKey rows"
    );

    let first = run_bootstrap_cli(
        db_url,
        fixture.console_client_id,
        fixture.redirect_uri,
        fixture.owner_email,
        fixture.owner_display_name,
    );
    let first_stdout = String::from_utf8_lossy(&first.stdout).into_owned();
    let first_stderr = String::from_utf8_lossy(&first.stderr).into_owned();
    assert!(
        first.status.success(),
        "the first bootstrap run must succeed (status {:?}); stdout: {first_stdout}\nstderr: \
         {first_stderr}",
        first.status
    );
    assert!(
        first_stdout.contains("rotated: new signing key"),
        "the first run must actually rotate in a key: {first_stdout}"
    );
    assert!(
        first_stdout.contains("provisioned user:"),
        "the first run must actually provision the owner user: {first_stdout}"
    );

    read_and_assert_bootstrap_artifacts(db, fixture).await
}

/// Reads back every artifact the first `bootstrap` run is supposed to
/// have created and asserts it's actually there — split out of
/// [`assert_first_bootstrap_run_creates_everything`] purely to stay under
/// `clippy::too_many_lines`.
async fn read_and_assert_bootstrap_artifacts(
    db: &Cratestack,
    fixture: &BootstrapFixture<'_>,
) -> (Vec<String>, schema::Provider) {
    let active_after_first = active_signing_key_ids(db).await;
    assert_eq!(
        active_after_first.len(),
        1,
        "exactly one OauthSigningKey must be active after the first run"
    );
    assert_eq!(signing_key_row_count(db).await, 1);

    let provider = db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::provider::key().eq("orange_cm".to_owned()),
        ))
        .run(&sys())
        .await
        .expect("reading back the orange_cm Provider row")
        .into_iter()
        .next()
        .expect("bootstrap must have created the orange_cm Provider row");
    assert_eq!(provider.state, schema::ProviderState::active);

    let routes = db
        .route()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::route::providerId().eq(provider.id.clone()),
        ))
        .run(&sys())
        .await
        .expect("reading back the catch-all Route");
    assert_eq!(
        routes.len(),
        1,
        "bootstrap must have created exactly one Route for the orange_cm Provider"
    );

    let oauth_client = db
        .oauth_client()
        .find_many()
        .where_expr(FilterExpr::from(
            oauth_client_filter::clientId().eq(fixture.console_client_id.to_owned()),
        ))
        .run(&sys())
        .await
        .expect("reading back the sms-console OauthClient")
        .into_iter()
        .next()
        .expect("bootstrap must have registered the sms-console OauthClient");
    assert_eq!(
        oauth_client.tokenEndpointAuthMethod,
        schema::ClientAuthMethod::none
    );

    let user = db
        .user()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::user::email().eq(fixture.owner_email.to_owned()),
        ))
        .run(&sys())
        .await
        .expect("reading back the owner User")
        .into_iter()
        .next()
        .expect("bootstrap must have provisioned the owner User");
    assert_eq!(user.roleKey, "owner");

    let credential_count = db
        .user_credential()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::user_credential::userId().eq(user.id.clone()),
        ))
        .run(&sys())
        .await
        .expect("reading back the owner's UserCredential")
        .len();
    assert_eq!(
        credential_count, 1,
        "bootstrap must have created exactly one UserCredential for the owner"
    );

    (active_after_first, provider)
}

/// The second half: re-run `bootstrap` against the already-bootstrapped
/// database from [`assert_first_bootstrap_run_creates_everything`] and
/// assert it's a clean no-op — the actual idempotency claim N2 exists to
/// prove.
async fn assert_second_bootstrap_run_is_a_no_op(
    db: &Cratestack,
    db_url: &str,
    fixture: &BootstrapFixture<'_>,
    active_after_first: &[String],
    provider: &schema::Provider,
) {
    let second = run_bootstrap_cli(
        db_url,
        fixture.console_client_id,
        fixture.redirect_uri,
        fixture.owner_email,
        fixture.owner_display_name,
    );
    let second_stdout = String::from_utf8_lossy(&second.stdout).into_owned();
    let second_stderr = String::from_utf8_lossy(&second.stderr).into_owned();
    assert!(
        second.status.success(),
        "a re-run against an already-bootstrapped database must still exit 0 (status {:?}); \
         stdout: {second_stdout}\nstderr: {second_stderr}",
        second.status
    );
    assert!(
        second_stdout.contains("already exists — skipping rotate-signing-key"),
        "the second run must explicitly skip rotation, not rotate silently: {second_stdout}"
    );
    assert!(
        second_stdout.contains("already exists — skipping provision-user"),
        "the second run must explicitly skip re-provisioning the owner: {second_stdout}"
    );

    // The actual idempotency claim: no second signing key minted (same
    // id, not just the same count — a broken "skip" check that always
    // rotates would still converge to exactly one *active* row, since
    // rotation deactivates the previous one in the same call, so the
    // row *count* is the property that actually catches that bug; see
    // this PR's own guard-failure proof).
    let active_after_second = active_signing_key_ids(db).await;
    assert_eq!(
        active_after_second, active_after_first,
        "the second run must not rotate — the active signing key's id must be unchanged"
    );
    assert_eq!(
        signing_key_row_count(db).await,
        1,
        "the second run must not have inserted a second OauthSigningKey row at all"
    );

    let provider_count_after_second = db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::provider::key().eq("orange_cm".to_owned()),
        ))
        .run(&sys())
        .await
        .expect("reading back Provider rows after the second run")
        .len();
    assert_eq!(
        provider_count_after_second, 1,
        "the second run must not create a duplicate orange_cm Provider row"
    );

    let route_count_after_second = db
        .route()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::route::providerId().eq(provider.id.clone()),
        ))
        .run(&sys())
        .await
        .expect("reading back Route rows after the second run")
        .len();
    assert_eq!(
        route_count_after_second, 1,
        "the second run must not create a duplicate catch-all Route"
    );

    let oauth_client_count_after_second = db
        .oauth_client()
        .find_many()
        .where_expr(FilterExpr::from(
            oauth_client_filter::clientId().eq(fixture.console_client_id.to_owned()),
        ))
        .run(&sys())
        .await
        .expect("reading back OauthClient rows after the second run")
        .len();
    assert_eq!(
        oauth_client_count_after_second, 1,
        "the second run must not create a duplicate sms-console OauthClient"
    );

    let user_count_after_second = db
        .user()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::user::email().eq(fixture.owner_email.to_owned()),
        ))
        .run(&sys())
        .await
        .expect("reading back User rows after the second run")
        .len();
    assert_eq!(
        user_count_after_second, 1,
        "the second run must not create a duplicate owner User"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn bootstrap_on_a_fresh_database_creates_every_artifact_and_a_second_run_is_a_clean_no_op() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;

    let fixture = BootstrapFixture {
        console_client_id: "bootstrap-test-sms-console",
        redirect_uri: "https://console.example.test/api/auth/callback",
        owner_email: "bootstrap-test-owner@example.test",
        owner_display_name: "Bootstrap Test Owner",
    };

    let (active_after_first, provider) =
        assert_first_bootstrap_run_creates_everything(&db, &db_url, &fixture).await;
    assert_second_bootstrap_run_is_a_no_op(&db, &db_url, &fixture, &active_after_first, &provider)
        .await;
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn create_app_is_idempotent_and_returns_the_same_id_on_a_second_run() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;

    let slug = "create-app-cli-idempotency-test";
    let name = "create-app CLI idempotency test";

    let first = run_create_app_cli(&db_url, slug, name);
    let first_stdout = String::from_utf8_lossy(&first.stdout).into_owned();
    assert!(
        first.status.success(),
        "the first create-app run must succeed; stdout: {first_stdout}\nstderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        first_stdout.contains("created App"),
        "the first run must actually create the row: {first_stdout}"
    );
    let first_id = parse_app_id(&first_stdout);

    let second = run_create_app_cli(&db_url, slug, name);
    let second_stdout = String::from_utf8_lossy(&second.stdout).into_owned();
    assert!(
        second.status.success(),
        "a re-run against an already-created slug must still exit 0; stdout: {second_stdout}\
         \nstderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(
        second_stdout.contains("already exists"),
        "the second run must report the row as already existing, not create a duplicate: \
         {second_stdout}"
    );
    let second_id = parse_app_id(&second_stdout);

    assert_eq!(
        first_id, second_id,
        "both runs must resolve to the same App id"
    );

    let rows = db
        .app()
        .find_many()
        .where_expr(FilterExpr::from(app_filter::slug().eq(slug.to_owned())))
        .run(&sys())
        .await
        .expect("reading back App rows for this slug");
    assert_eq!(
        rows.len(),
        1,
        "exactly one App row must exist for this slug after two create-app runs"
    );
}
