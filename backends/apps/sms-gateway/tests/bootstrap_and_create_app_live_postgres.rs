//! Production-readiness audit N1/N2: `sms-gateway create-app` and
//! `sms-gateway bootstrap` — both real CLI subcommands (`main.rs`'s
//! `Command::CreateApp`/`Command::Bootstrap`), not `Procedures` called
//! in-process. Proves the claims `docs/runbooks/deployment.adoc`'s
//! step 3/step 5 make:
//!
//! - `bootstrap` on a fresh database creates an active `orange_cm`
//!   `Provider` with a catch-all `Route` and the first operator `User` — in
//!   one call. The `User` has no credential (VS-4: there is no password to
//!   make), and the output says how a console key gets attached to it.
//! - A second run against an already-bootstrapped database is a clean
//!   no-op: no duplicate `Provider`/`Route`/`User` row.
//! - `create-app` is idempotent the same way `seed-dispatch` already is:
//!   `create` + catch `23505` on `App.slug`, returning the same id both
//!   times.
//!
//! What this file used to prove about an `OpenID` Provider signing key, an
//! `sms-console` `OauthClient` and a `UserCredential` is gone with those
//! things (VS-4); `serve` needs none of them to start, and
//! `m1_acceptance_gate_live_postgres.rs` starts it without.
//!
//! ```bash
//! cargo test -p sms-gateway --test bootstrap_and_create_app_live_postgres -- --ignored --nocapture
//! ```

use std::process::{Command, Output};

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack, app as app_filter};

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
/// takes a `HashPepper` unconditionally. Only the value's length matters.
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

/// `owner_email`/`owner_display_name` are optional flags on the real CLI (R4
/// — see `Command::Bootstrap`'s own doc comment) — `None` here means the
/// flag is omitted entirely, not passed as an empty string, so callers can
/// exercise the real backend-only path.
fn run_bootstrap_cli(
    database_url: &str,
    owner_email: Option<&str>,
    owner_display_name: Option<&str>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sms-gateway"));
    command
        .arg("bootstrap")
        .arg("--database-url")
        .arg(database_url);
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

struct BootstrapFixture<'a> {
    owner_email: &'a str,
    owner_display_name: &'a str,
}

/// Reads the `orange_cm` `Provider` row, asserting it is active.
async fn orange_provider(db: &Cratestack) -> schema::Provider {
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
    provider
}

async fn user_count_for_email(db: &Cratestack, email: &str) -> usize {
    db.user()
        .find_many()
        .where_expr(FilterExpr::from(schema::user::email().eq(email.to_owned())))
        .run(&sys())
        .await
        .expect("reading back User rows for an email")
        .len()
}

/// The first half of the live test: run `bootstrap` once and assert
/// every artifact it's supposed to create actually landed. Pulled out
/// purely to keep the test function itself under
/// `clippy::too_many_lines`.
///
/// Does *not* assume the database starts empty: this binary's one database
/// is shared, sequentially, by every test in this file
/// (`sms_test_support`'s "one database per binary" rule), and Rust gives no
/// ordering guarantee between them, so a sibling test's own bootstrap call
/// may have already created the `Provider`. The order-independent claim — a
/// second run creates nothing — is what
/// [`assert_second_bootstrap_run_is_a_no_op`] checks.
async fn assert_first_bootstrap_run_creates_everything(
    db: &Cratestack,
    db_url: &str,
    fixture: &BootstrapFixture<'_>,
) -> schema::Provider {
    let first = run_bootstrap_cli(
        db_url,
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
        first_stdout.contains("provisioned user:"),
        "the first run must actually provision the owner user: {first_stdout}"
    );
    assert!(
        first_stdout.contains("no credential exists to hand over")
            && first_stdout.contains("pair-console-key"),
        "the output must say the user has no credential and how a console key is attached: \
         {first_stdout}"
    );
    assert!(
        !first_stdout.to_lowercase().contains("password"),
        "bootstrap must not print or mention a password any more: {first_stdout}"
    );

    let provider = orange_provider(db).await;

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
    assert_eq!(
        user.subject, user.id,
        "subject is the row's own id, not the pending placeholder"
    );

    provider
}

/// The second half: re-run `bootstrap` against the already-bootstrapped
/// database from [`assert_first_bootstrap_run_creates_everything`] and
/// assert it's a clean no-op — the actual idempotency claim N2 exists to
/// prove.
async fn assert_second_bootstrap_run_is_a_no_op(
    db: &Cratestack,
    db_url: &str,
    fixture: &BootstrapFixture<'_>,
    provider: &schema::Provider,
) {
    let second = run_bootstrap_cli(
        db_url,
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
        second_stdout.contains("already exists — skipping provision-user"),
        "the second run must explicitly skip re-provisioning the owner: {second_stdout}"
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

    assert_eq!(
        user_count_for_email(db, fixture.owner_email).await,
        1,
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
        owner_email: "bootstrap-test-owner@example.test",
        owner_display_name: "Bootstrap Test Owner",
    };

    let provider = assert_first_bootstrap_run_creates_everything(&db, &db_url, &fixture).await;
    assert_second_bootstrap_run_is_a_no_op(&db, &db_url, &fixture, &provider).await;
}

/// R4: a backend-only deployment (`CONTRIBUTING.md`'s "the admin console is
/// optional, the backend must run without it") must be able to run
/// `bootstrap` with no owner flags and still get a working `Provider` +
/// `Route` — without `bootstrap` attempting (or silently no-op'ing) the
/// owner-account step, which has no meaning with no console.
///
/// This test shares this binary's one database with its siblings
/// (`sms_test_support`'s own "one database per binary" rule), so it
/// deliberately never asserts a *global* zero row count for `User` — Rust
/// test order isn't guaranteed, and another test in this file may have
/// already created one. The `User` table's own row count is compared
/// before/after to prove this call created none.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn bootstrap_backend_only_skips_the_owner_user_step() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;

    let user_count_before = db
        .user()
        .find_many()
        .run(&sys())
        .await
        .expect("reading back every User row before the backend-only run")
        .len();

    let output = run_bootstrap_cli(&db_url, None, None);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "a backend-only bootstrap run (no owner flags at all) must still succeed (status \
         {:?}); stdout: {stdout}\nstderr: {stderr}",
        output.status
    );

    assert!(
        stdout.contains("skipped — no --owner-email given (backend-only deployment)"),
        "step 2 (owner user) must print the R4 skip message: {stdout}"
    );
    assert!(
        !stdout.contains("provisioned user:"),
        "a backend-only run must never provision a user: {stdout}"
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

    // Step 1 still ran — the actual R4 claim: a backend-only deployment
    // still gets everything `sms-gateway serve` needs to route a message.
    let orange_provider = orange_provider(&db).await;
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

/// `--skip-seed-dispatch` must skip step 1 entirely: no `Provider` row and
/// no `Route` row may be created by *this* run, and the output must say why.
///
/// The database is shared with the sibling tests in this binary (see the
/// R4 test above), and they all create the `orange_cm` `Provider` by running
/// `bootstrap` without the flag — so a global "no such row" assertion would
/// depend on test order. Instead compare the `Provider` and `Route` counts
/// before and after this one call: a skipped step leaves both unchanged
/// whether or not a sibling already seeded them.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn bootstrap_with_skip_seed_dispatch_creates_no_provider_or_route() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;

    let provider_count_before = db
        .provider()
        .find_many()
        .run(&sys())
        .await
        .expect("reading back Provider rows before the run")
        .len();
    let route_count_before = db
        .route()
        .find_many()
        .run(&sys())
        .await
        .expect("reading back Route rows before the run")
        .len();

    let output = Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
        .arg("bootstrap")
        .arg("--database-url")
        .arg(&db_url)
        .arg("--skip-seed-dispatch")
        .env("SMS_HASH_PEPPER", TEST_HASH_PEPPER)
        .output()
        .expect("running `sms-gateway bootstrap --skip-seed-dispatch`");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "bootstrap --skip-seed-dispatch must succeed (status {:?}); stdout: {stdout}\nstderr: \
         {stderr}",
        output.status
    );
    assert!(
        stdout.contains("skipped — run seed-dispatch for the provider you will send through"),
        "the skipped step must say what to do instead: {stdout}"
    );
    assert!(
        !stdout.contains("created Provider"),
        "a skipped seed-dispatch step must not report creating anything: {stdout}"
    );

    let provider_count_after = db
        .provider()
        .find_many()
        .run(&sys())
        .await
        .expect("reading back Provider rows after the run")
        .len();
    let route_count_after = db
        .route()
        .find_many()
        .run(&sys())
        .await
        .expect("reading back Route rows after the run")
        .len();
    assert_eq!(
        provider_count_after, provider_count_before,
        "--skip-seed-dispatch must not create a Provider row"
    );
    assert_eq!(
        route_count_after, route_count_before,
        "--skip-seed-dispatch must not create a Route row"
    );
}

/// `--owner-email` and `--owner-display-name` are required together, in
/// either direction: a named, refused misconfiguration, not a silent no-op.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn bootstrap_refuses_one_owner_flag_without_the_other() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;

    let email_only = run_bootstrap_cli(&db_url, Some("orphan-owner@example.test"), None);
    assert!(
        !email_only.status.success(),
        "--owner-email without --owner-display-name must be refused"
    );
    let stderr = String::from_utf8_lossy(&email_only.stderr);
    assert!(
        stderr.contains("--owner-email requires --owner-display-name"),
        "the refusal must name which two flags are required together: {stderr}"
    );

    let name_only = run_bootstrap_cli(&db_url, None, Some("Orphan Owner"));
    assert!(
        !name_only.status.success(),
        "--owner-display-name without --owner-email must be refused"
    );
    let stderr = String::from_utf8_lossy(&name_only.stderr);
    assert!(
        stderr.contains("--owner-display-name requires --owner-email"),
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
