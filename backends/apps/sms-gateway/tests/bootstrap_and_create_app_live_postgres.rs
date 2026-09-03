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

/// `User.create`'s own `@@allow` is `hasRole('owner') ||
/// hasRole('admin')` — used by
/// [`seed_orphaned_user_with_no_credential`], the fixture for review
/// round 1's item 12 test.
fn owner() -> CratestackContext {
    Principal {
        sub: "bootstrap-cli-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
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

/// `console_redirect_uri`/`owner_email`/`owner_display_name` are all
/// optional flags on the real CLI (R4 — see `Command::Bootstrap`'s own
/// doc comment) — `None` here means the flag is omitted entirely, not
/// passed as an empty string, so callers can exercise the real
/// backend-only path.
fn run_bootstrap_cli(
    database_url: &str,
    console_client_id: &str,
    console_redirect_uri: Option<&str>,
    owner_email: Option<&str>,
    owner_display_name: Option<&str>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sms-gateway"));
    command
        .arg("bootstrap")
        .arg("--database-url")
        .arg(database_url)
        .arg("--console-client-id")
        .arg(console_client_id);
    if let Some(redirect_uri) = console_redirect_uri {
        command.arg("--console-redirect-uri").arg(redirect_uri);
    }
    if let Some(email) = owner_email {
        command.arg("--owner-email").arg(email);
    }
    if let Some(display_name) = owner_display_name {
        command.arg("--owner-display-name").arg(display_name);
    }
    command
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

/// State captured right after the first `bootstrap` run, for the second
/// run's own idempotency assertions to compare against. Deliberately
/// *not* built on the assumption that this test's own database starts
/// empty — see [`assert_first_bootstrap_run_creates_everything`]'s own
/// doc for why: this binary's one database is shared, sequentially, by
/// every test in this file (`sms_test_support`'s "one database per
/// binary" rule), and Rust gives no ordering guarantee between them, so
/// a sibling test's own bootstrap call may have already run first.
struct FirstRunState {
    active_signing_key_ids: Vec<String>,
    signing_key_row_count: usize,
    provider: schema::Provider,
}

/// The first half of the live test: run `bootstrap` once and assert
/// every artifact it's supposed to create actually landed. Pulled out
/// purely to keep the test function itself under
/// `clippy::too_many_lines` — same reason `main.rs`'s own multi-step
/// command functions are split into per-step helpers.
///
/// Does *not* assert the database starts with zero `OauthSigningKey`
/// rows, or that this specific call is the one that rotates a key in —
/// found live while adding the sibling backend-only test (review round
/// 1): this file's own test-isolation convention shares one database
/// across every test in the binary, sequentially, with no ordering
/// guarantee, so a `bootstrap_backend_only_...` run that happens to
/// execute first already leaves an active key behind, and this test's
/// own first call correctly reports "already exists — skipping" rather
/// than "rotated". The real, order-independent claim — a second run
/// never rotates an already-active key — is what
/// [`assert_second_bootstrap_run_is_a_no_op`] checks, by comparing state
/// *after this call* against state after the next one, not against an
/// assumed-empty starting point.
async fn assert_first_bootstrap_run_creates_everything(
    db: &Cratestack,
    db_url: &str,
    fixture: &BootstrapFixture<'_>,
) -> FirstRunState {
    let first = run_bootstrap_cli(
        db_url,
        fixture.console_client_id,
        Some(fixture.redirect_uri),
        Some(fixture.owner_email),
        Some(fixture.owner_display_name),
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
        first_stdout.contains("rotated: new signing key")
            || first_stdout.contains("already exists — skipping rotate-signing-key"),
        "step 1 must either rotate a key in or correctly report one already exists: \
         {first_stdout}"
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
) -> FirstRunState {
    let active_after_first = active_signing_key_ids(db).await;
    assert_eq!(
        active_after_first.len(),
        1,
        "exactly one OauthSigningKey must be active after the first run — regardless of \
         whether this call or an earlier sibling test's call is the one that created it"
    );
    let signing_key_row_count_after_first = signing_key_row_count(db).await;

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

    FirstRunState {
        active_signing_key_ids: active_after_first,
        signing_key_row_count: signing_key_row_count_after_first,
        provider,
    }
}

/// The second half: re-run `bootstrap` against the already-bootstrapped
/// database from [`assert_first_bootstrap_run_creates_everything`] and
/// assert it's a clean no-op — the actual idempotency claim N2 exists to
/// prove.
async fn assert_second_bootstrap_run_is_a_no_op(
    db: &Cratestack,
    db_url: &str,
    fixture: &BootstrapFixture<'_>,
    first_run: &FirstRunState,
) {
    let provider = &first_run.provider;
    let second = run_bootstrap_cli(
        db_url,
        fixture.console_client_id,
        Some(fixture.redirect_uri),
        Some(fixture.owner_email),
        Some(fixture.owner_display_name),
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
        active_after_second, first_run.active_signing_key_ids,
        "the second run must not rotate — the active signing key's id must be unchanged"
    );
    assert_eq!(
        signing_key_row_count(db).await,
        first_run.signing_key_row_count,
        "the second run must not have inserted a second OauthSigningKey row at all — compared \
         against the row count right after the first run, not an assumed starting count, since \
         this database is shared with sibling tests in this binary"
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

    let first_run = assert_first_bootstrap_run_creates_everything(&db, &db_url, &fixture).await;
    assert_second_bootstrap_run_is_a_no_op(&db, &db_url, &fixture, &first_run).await;
}

/// R4 (production-readiness audit review round 1, blocker 1): a
/// backend-only deployment (`CONTRIBUTING.md`'s "the admin console is
/// optional, the backend must run without it") must be able to run
/// `bootstrap` with none of the console/owner flags and still get a
/// working signing key + `Provider` + `Route` — the two steps that
/// gate `sms-gateway serve` ever binding its listener at all — without
/// `bootstrap` attempting (or silently no-op'ing) the console-client or
/// owner-account steps, which have no meaning with no console.
///
/// This test shares this binary's one database with the two tests
/// above it (`sms_test_support`'s own "one database per binary" rule),
/// so it deliberately never asserts a *global* zero row count for
/// `User`/`OauthClient` — Rust test order isn't guaranteed, and another
/// test in this file may have already created rows of both kinds.
/// Instead: a `console_client_id` unique to this test proves *this run*
/// created zero matching `OauthClient` rows (a global count could never
/// be exactly zero once the sibling tests have run, but a row keyed to
/// an id only this test ever uses can), and the `User` table's own row
/// count is compared before/after to prove this call created none.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn bootstrap_backend_only_skips_the_console_client_and_owner_user_steps() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;

    let console_client_id = "bootstrap-backend-only-test-sms-console";
    let user_count_before = db
        .user()
        .find_many()
        .run(&sys())
        .await
        .expect("reading back every User row before the backend-only run")
        .len();

    let output = run_bootstrap_cli(&db_url, console_client_id, None, None, None);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "a backend-only bootstrap run (no console flags at all) must still succeed (status \
         {:?}); stdout: {stdout}\nstderr: {stderr}",
        output.status
    );

    // Steps 1/2 (signing key, Provider/Route) still ran — the actual R4
    // claim: a backend-only deployment still gets everything
    // `sms-gateway serve` needs to bind its listener.
    assert!(
        stdout.contains("rotated: new signing key")
            || stdout.contains("already exists — skipping rotate-signing-key"),
        "step 1 must still run (or be correctly skipped if an earlier test already \
         bootstrapped this shared database): {stdout}"
    );

    // Steps 3/4 must both report the R4 skip, never attempt anything.
    let skip_count = stdout
        .matches("skipped — no --console-redirect-uri given (backend-only deployment)")
        .count();
    assert_eq!(
        skip_count, 2,
        "both step 3 (console client) and step 4 (owner user) must print the R4 skip message \
         exactly once each: {stdout}"
    );
    assert!(
        !stdout.contains("provisioned user:"),
        "a backend-only run must never provision a user: {stdout}"
    );

    let oauth_client_count = db
        .oauth_client()
        .find_many()
        .where_expr(FilterExpr::from(
            oauth_client_filter::clientId().eq(console_client_id.to_owned()),
        ))
        .run(&sys())
        .await
        .expect("reading back OauthClient rows for this test's own client id")
        .len();
    assert_eq!(
        oauth_client_count, 0,
        "a backend-only bootstrap run must never register an sms-console OauthClient"
    );

    let user_count_after = db
        .user()
        .find_many()
        .run(&sys())
        .await
        .expect("reading back every User row after the backend-only run")
        .len();
    assert_eq!(
        user_count_after, user_count_before,
        "a backend-only bootstrap run must not create any User row"
    );

    let orange_provider = db
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
        .expect("a backend-only bootstrap run must still create the orange_cm Provider row");
    assert_eq!(orange_provider.state, schema::ProviderState::active);

    let route_count = db
        .route()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::route::providerId().eq(orange_provider.id.clone()),
        ))
        .run(&sys())
        .await
        .expect("reading back the catch-all Route")
        .len();
    assert!(
        route_count >= 1,
        "a backend-only bootstrap run must still create the catch-all Route"
    );
}

/// R4 (blocker 1): `--owner-email` with no `--console-redirect-uri` is
/// a named, refused misconfiguration, not a silent no-op — an operator
/// who wants an owner account almost certainly also wants the console
/// client that account can actually sign into.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn bootstrap_refuses_owner_email_without_console_redirect_uri() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;

    let output = run_bootstrap_cli(
        &db_url,
        "bootstrap-validation-test-sms-console",
        None,
        Some("orphan-owner@example.test"),
        Some("Orphan Owner"),
    );
    assert!(
        !output.status.success(),
        "--owner-email without --console-redirect-uri must be refused, not silently accepted"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--owner-email requires --console-redirect-uri"),
        "the refusal must name which two flags are required together: {stderr}"
    );
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

/// Review round 1, item 12's own fixture: writes a real `User` row with
/// no `UserCredential` at all — simulating an interrupted earlier
/// provisioning attempt (the exact race `create_console_user_if_absent`'s
/// transaction wrap now prevents for any *future* attempt, but which a
/// database written before this fix could already contain).
async fn seed_orphaned_user_with_no_credential(db: &Cratestack, email: &str, display_name: &str) {
    db.user()
        .create(schema::CreateUserInput {
            subject: format!("orphan-test-{email}"),
            email: email.to_owned(),
            displayName: display_name.to_owned(),
            roleKey: "owner".to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding an orphaned User row with no UserCredential");
}

/// Review round 1, item 12: `bootstrap`'s duplicate-email path must not
/// silently report "already exists" for a `User` row with no
/// `UserCredential` — that account can never log in, and reporting
/// success would hide a real, actionable problem rather than surface it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn bootstrap_refuses_a_duplicate_email_with_no_credential() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;

    let email = "orphaned-owner@example.test";
    seed_orphaned_user_with_no_credential(&db, email, "Orphaned Owner").await;

    let output = run_bootstrap_cli(
        &db_url,
        "bootstrap-orphan-test-sms-console",
        Some("https://console.example.test/api/auth/callback"),
        Some(email),
        Some("Orphaned Owner"),
    );
    assert!(
        !output.status.success(),
        "bootstrap must refuse to silently report success for an orphaned User row"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already exists") && stderr.contains("no UserCredential"),
        "the refusal must name the actual problem: {stderr}"
    );
    assert!(
        stderr.contains("cannot repair it automatically"),
        "the refusal must say what this command can't do, not just that something's wrong: \
         {stderr}"
    );
}
