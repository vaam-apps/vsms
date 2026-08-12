//! #24's `@@allow` half of *"a test asserting the full generated policy
//! set ... so a typo'd `@@allow` ... fails the build rather than silently
//! no-opping."* `tests/field_expansion.rs` is the `@@use` half.
//!
//! §2.0's trap, verbatim: *"A misspelled `@@allow` action is dropped, and
//! deny-by-default makes that operation unreachable."* Neither `cratestack
//! check` nor `cargo build` can see this — the policy is evaluated inside
//! a generated closure with no distinguishable shape between "this role is
//! denied on purpose" and "this role is denied because the action name got
//! typo'd out of existence". The only way to catch it is to attempt the
//! real action, under a real context, against a real database, and check
//! the *outcome* against a hand-written expectation — which is what this
//! file is: a golden list, one per `@@allow`d model action, diffed against
//! reality.
//!
//! # Scope
//!
//! Two models, `create`/`update`/`delete` only — not all 19 models, and
//! not `read`/`list`/`detail`. Two reasons, not one:
//!
//! - Same precedent `tests/create_inputs.rs` already set: six of nineteen
//!   models, chosen for relevance, not exhaustiveness. Here: `Provider`
//!   (this PR's own `provider:write` anchor, `router::PROVIDER_WRITE_ROUTES`)
//!   and `AppClient` (`schema.cstack`'s own comment on its `create` policy
//!   names the exact failure mode this file exists to catch — for months,
//!   this model had *no* `@@allow("create", ...)` at all, and deny-by-
//!   default made every role, `owner` included, unable to create one until
//!   #32 found it live).
//! - `create`/`update`/`delete` fail *loud* on denial — a real
//!   `CoolError::Forbidden` this file can assert against directly.
//!   `find_many`-backed actions (`read`/`list`/`detail`) fail *quiet*:
//!   confirmed live in `crates/sms-auth/tests/oidc_flow_live.rs` (the
//!   `OauthSigningKey` assertion), `CrateStack`'s policy enforcement there is
//!   row-level filtering to an empty result, not a request-level error.
//!   Asserting that shape needs a seed-then-compare test per model, a
//!   different technique this file doesn't also try to be.
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. Run explicitly:
//!
//! `sms_test_support` provisions Postgres and applies both migrations
//! automatically (a shared, self-healing container — see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-api --test policy_golden_list_live_postgres -- --ignored
//! ```

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, CoolError};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack};

/// Same reasoning as every other live suite's own copy of this mutex —
/// see `crates/sms-worker/tests/claim_live_postgres.rs`'s doc (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Every human role this schema's policies name. `hasRole('system')` is
/// tested separately via [`system_ctx`] — it is never a role a human
/// caller's token carries — and a bare machine caller via [`app_ctx`].
const HUMAN_ROLES: &[&str] = &[
    "owner",
    "admin",
    "operator",
    "auditor",
    "developer",
    "support",
];

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
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

fn ctx_for_role(role: &str) -> CoolContext {
    Principal {
        sub: format!("golden-list-{role}"),
        kind: PrincipalKind::User,
        role: role.to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// The context every internal, procedure-driven write in this codebase
/// runs under — never a real caller's token (see `GatewayAuth`'s own
/// doc), but a real, distinct grant this file has to check on its own
/// terms rather than assume from the human-role results.
fn system_ctx() -> CoolContext {
    Principal {
        sub: "golden-list-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// A bare machine (`client_credentials`) caller — `role == "app"`, the one
/// role `GatewayAuth` ever hands a real token (see its own doc). Included
/// because "no policy names this role" and "this role is denied" are the
/// same outcome and worth confirming stay the same outcome.
fn app_ctx() -> CoolContext {
    Principal {
        sub: "golden-list-app".to_owned(),
        kind: PrincipalKind::App,
        role: "app".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn is_forbidden<T: std::fmt::Debug>(result: &Result<T, CoolError>) -> bool {
    matches!(result, Err(CoolError::Forbidden(_)))
}

fn fresh_provider_input() -> schema::CreateProviderInput {
    let suffix = unique_suffix();
    schema::CreateProviderInput {
        key: format!(
            "golden_{}",
            suffix.to_lowercase().chars().take(20).collect::<String>()
        ),
        displayName: "Golden List Test Provider".to_owned(),
        kind: schema::ProviderKind::aggregator_http,
        config: "{}".to_owned(),
        credentialRef: "vault://test".to_owned(),
        maxTps: 5.0,
        maxDailySubmissions: 1000,
        supportsDlr: true,
        supportsAlphaSender: true,
        supportsUcs2: true,
        supportsConcat: true,
        costPerSegmentXaf: "15".parse().unwrap(),
        healthCheckedAt: None,
        circuitOpenUntil: None,
    }
}

/// Every role this file exercises, each attempting `create` on a *fresh*
/// `Provider` row of its own — a role wrongly denied a legitimate create,
/// or wrongly granted an illegitimate one, is the exact failure shape a
/// typo'd `@@allow("create", ...)` produces.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn provider_create_policy_matches_the_schema_exactly() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // schema.cstack: @@allow("create", hasRole('owner') || hasRole('admin'))
    for role in ["owner", "admin"] {
        let result = db
            .provider()
            .create(fresh_provider_input())
            .run(&ctx_for_role(role))
            .await;
        assert!(
            result.is_ok(),
            "role {role:?} must be able to create a Provider per schema.cstack: {result:?}"
        );
    }

    for role in HUMAN_ROLES
        .iter()
        .filter(|role| !["owner", "admin"].contains(role))
    {
        let result = db
            .provider()
            .create(fresh_provider_input())
            .run(&ctx_for_role(role))
            .await;
        assert!(
            is_forbidden(&result),
            "role {role:?} must NOT be able to create a Provider per schema.cstack: {result:?}"
        );
    }

    let result = db
        .provider()
        .create(fresh_provider_input())
        .run(&system_ctx())
        .await;
    assert!(
        is_forbidden(&result),
        "hasRole('system') is absent from Provider's create policy: {result:?}"
    );

    let result = db
        .provider()
        .create(fresh_provider_input())
        .run(&app_ctx())
        .await;
    assert!(
        is_forbidden(&result),
        "a bare machine caller has no create grant on Provider: {result:?}"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn provider_update_policy_matches_the_schema_exactly() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // schema.cstack: @@allow("update", hasRole('owner') || hasRole('admin')
    // || hasRole('operator') || hasRole('system'))
    let allowed = ["owner", "admin", "operator"];
    for role in allowed {
        let seeded = db
            .provider()
            .create(fresh_provider_input())
            .run(&ctx_for_role("owner"))
            .await
            .expect("seeding a provider to update");

        let result = db
            .provider()
            .update(seeded.id)
            .set(schema::UpdateProviderInput {
                maxTps: Some(7.0),
                ..Default::default()
            })
            // #59: Provider is now @version'd.
            .if_match(seeded.version)
            .run(&ctx_for_role(role))
            .await;
        assert!(
            result.is_ok(),
            "role {role:?} must be able to update a Provider per schema.cstack: {result:?}"
        );
    }

    for role in HUMAN_ROLES.iter().filter(|role| !allowed.contains(role)) {
        let seeded = db
            .provider()
            .create(fresh_provider_input())
            .run(&ctx_for_role("owner"))
            .await
            .expect("seeding a provider to update");

        let result = db
            .provider()
            .update(seeded.id)
            .set(schema::UpdateProviderInput {
                maxTps: Some(7.0),
                ..Default::default()
            })
            // #59: Provider is now @version'd.
            .if_match(seeded.version)
            .run(&ctx_for_role(role))
            .await;
        assert!(
            is_forbidden(&result),
            "role {role:?} must NOT be able to update a Provider per schema.cstack: {result:?}"
        );
    }

    // #63: `hasRole('system')` was absent here until this PR, and the
    // absence was silently wrong — not the "closes the REST route to every
    // real token" story this test used to tell (that reasoning was about
    // human/app-role tokens never carrying `system`, which is still true
    // and still why this is safe to grant: `system` is synthetic, minted
    // only by this codebase's own internal `sys()` contexts, never by any
    // real `GatewayAuth::authenticate` token). The gap was found live by
    // `crates/sms-worker/tests/dispatch_live_postgres.rs`'s own
    // `an_open_circuit_routes_new_messages_to_the_alternative_instead_of_rejecting`:
    // `dispatch.rs`'s new circuit-breaker writes
    // (`record_provider_failure`/`reset_provider_failures`) run under
    // `sys()` and got `Forbidden("update policy denied this operation")`
    // on every attempt, silently absorbed by that function's own
    // best-effort "log and drop" handling — so the breaker never opened,
    // caught only because the live test asserted the *effect*
    // (`circuitOpenUntil` actually set), not just that the call didn't
    // panic.
    let seeded = db
        .provider()
        .create(fresh_provider_input())
        .run(&ctx_for_role("owner"))
        .await
        .expect("seeding a provider to update");
    let result = db
        .provider()
        .update(seeded.id)
        .set(schema::UpdateProviderInput {
            maxTps: Some(7.0),
            ..Default::default()
        })
        // #59: Provider is now @version'd.
        .if_match(seeded.version)
        .run(&system_ctx())
        .await;
    assert!(
        result.is_ok(),
        "hasRole('system') must admit Provider's update policy — dispatch.rs's circuit breaker \
         writes this model under a system context: {result:?}"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn provider_delete_policy_matches_the_schema_exactly() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // schema.cstack: @@allow("delete", hasRole('owner'))
    let seeded = db
        .provider()
        .create(fresh_provider_input())
        .run(&ctx_for_role("owner"))
        .await
        .expect("seeding a provider to delete");
    let result = db
        .provider()
        .delete(seeded.id)
        .run(&ctx_for_role("owner"))
        .await;
    assert!(
        result.is_ok(),
        "owner must be able to delete a Provider per schema.cstack: {result:?}"
    );

    for role in HUMAN_ROLES.iter().filter(|role| **role != "owner") {
        let seeded = db
            .provider()
            .create(fresh_provider_input())
            .run(&ctx_for_role("owner"))
            .await
            .expect("seeding a provider to delete");
        let result = db
            .provider()
            .delete(seeded.id)
            .run(&ctx_for_role(role))
            .await;
        assert!(
            is_forbidden(&result),
            "role {role:?} must NOT be able to delete a Provider per schema.cstack — in \
             particular, admin must not: only owner may (§5.2's own table: provider:delete is \
             owner-only, admin gets everything except role editing and owner-level deletes): \
             {result:?}"
        );
    }
}

/// A fresh `App` — `AppClient.appId`'s required parent.
async fn fresh_app(db: &Cratestack) -> schema::App {
    let suffix = unique_suffix();
    db.app()
        .create(schema::CreateAppInput {
            name: "golden list test app".to_owned(),
            slug: format!("golden-list-{suffix}"),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&ctx_for_role("owner"))
        .await
        .expect("seeding an app")
}

fn fresh_app_client_input(app_id: String) -> schema::CreateAppClientInput {
    schema::CreateAppClientInput {
        appId: app_id,
        clientId: format!("golden-list-{}", unique_suffix()),
        label: "golden list test client".to_owned(),
        scopes: " sms:send ".to_owned(),
        lastUsedAt: None,
        retiredAt: None,
    }
}

/// The regression test for the exact bug `schema.cstack`'s own comment on
/// `AppClient`'s `create` policy documents: this model had **no**
/// `@@allow("create", ...)` at all for a stretch of this project's
/// history, and deny-by-default made every role — `owner` included —
/// unable to create one, found live only when #32 tried to seed a fixture
/// for its own test suite. `hasRole('system')` is the *only* grant; not
/// even `owner`/`admin` may create one directly (`provisionAppClient`,
/// #23, is this model's intended sole writer).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn app_client_create_policy_matches_the_schema_exactly() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = fresh_app(&db).await;

    let result = db
        .app_client()
        .create(fresh_app_client_input(app.id.clone()))
        .run(&system_ctx())
        .await;
    assert!(
        result.is_ok(),
        "hasRole('system') must be able to create an AppClient per schema.cstack: {result:?}"
    );

    for role in HUMAN_ROLES {
        let result = db
            .app_client()
            .create(fresh_app_client_input(app.id.clone()))
            .run(&ctx_for_role(role))
            .await;
        assert!(
            is_forbidden(&result),
            "role {role:?} — owner included — must NOT be able to create an AppClient \
             directly; this is the exact shape of the bug schema.cstack's own comment on this \
             policy documents (no create policy at all, previously): {result:?}"
        );
    }

    let result = db
        .app_client()
        .create(fresh_app_client_input(app.id.clone()))
        .run(&app_ctx())
        .await;
    assert!(
        is_forbidden(&result),
        "a bare machine caller has no create grant on AppClient: {result:?}"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn app_client_update_policy_matches_the_schema_exactly() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = fresh_app(&db).await;

    // schema.cstack: @@allow("update", hasRole('owner') || hasRole('admin'))
    let allowed = ["owner", "admin"];
    for role in allowed {
        let seeded = db
            .app_client()
            .create(fresh_app_client_input(app.id.clone()))
            .run(&system_ctx())
            .await
            .expect("seeding an app client to update");

        let result = db
            .app_client()
            .update(seeded.id)
            .set(schema::UpdateAppClientInput {
                label: Some("relabelled".to_owned()),
                ..Default::default()
            })
            // #59: AppClient is now @version'd.
            .if_match(seeded.version)
            .run(&ctx_for_role(role))
            .await;
        assert!(
            result.is_ok(),
            "role {role:?} must be able to update an AppClient per schema.cstack: {result:?}"
        );
    }

    for role in HUMAN_ROLES.iter().filter(|role| !allowed.contains(role)) {
        let seeded = db
            .app_client()
            .create(fresh_app_client_input(app.id.clone()))
            .run(&system_ctx())
            .await
            .expect("seeding an app client to update");

        let result = db
            .app_client()
            .update(seeded.id)
            .set(schema::UpdateAppClientInput {
                label: Some("relabelled".to_owned()),
                ..Default::default()
            })
            // #59: AppClient is now @version'd.
            .if_match(seeded.version)
            .run(&ctx_for_role(role))
            .await;
        assert!(
            is_forbidden(&result),
            "role {role:?} must NOT be able to update an AppClient per schema.cstack: \
             {result:?}"
        );
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn app_client_delete_policy_matches_the_schema_exactly() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = fresh_app(&db).await;

    // schema.cstack: @@allow("delete", hasRole('owner') || hasRole('admin'))
    let allowed = ["owner", "admin"];
    for role in allowed {
        let seeded = db
            .app_client()
            .create(fresh_app_client_input(app.id.clone()))
            .run(&system_ctx())
            .await
            .expect("seeding an app client to delete");

        let result = db
            .app_client()
            .delete(seeded.id)
            .run(&ctx_for_role(role))
            .await;
        assert!(
            result.is_ok(),
            "role {role:?} must be able to delete an AppClient per schema.cstack: {result:?}"
        );
    }

    for role in HUMAN_ROLES.iter().filter(|role| !allowed.contains(role)) {
        let seeded = db
            .app_client()
            .create(fresh_app_client_input(app.id.clone()))
            .run(&system_ctx())
            .await
            .expect("seeding an app client to delete");

        let result = db
            .app_client()
            .delete(seeded.id)
            .run(&ctx_for_role(role))
            .await;
        assert!(
            is_forbidden(&result),
            "role {role:?} must NOT be able to delete an AppClient per schema.cstack: \
             {result:?}"
        );
    }
}
