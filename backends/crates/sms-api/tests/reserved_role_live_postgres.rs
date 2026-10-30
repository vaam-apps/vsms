//! The reserved-role guard, proven past the database's own check.
//!
//! `Role.key` values `system` and `app` are reserved: a `Role` keyed `system`
//! assigned to a human would let that human reach every `hasRole('system')`
//! policy. The guard is closed two independent ways - a `CHECK (key NOT IN
//! ('system', 'app'))` on `roles.key` (`roles_key_not_reserved_check`, §2.10)
//! and a point-of-use refusal in `sms_api::signers` - and this file proves the
//! second *with the first removed*, which is the only way to prove it at all.
//!
//! (This was `a_role_keyed_system_never_authenticates_even_past_the_database_check`
//! in `sms-gateway`'s `login_flow_live_postgres.rs`, which VS-4 deleted with the
//! login it tested; the guard it proves is the signer directory's and never
//! needed the login. It runs on the in-process router, signed.)
//!
//! **The one raw-SQL test file here besides the other R1 exceptions**
//! (`cargo xtask no-raw-sqlx`, `CONTRIBUTING.md`): the constraint is dropped and
//! restored with DDL; every data write still goes through a real delegate.
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. Run explicitly:
//!
//! ```bash
//! cargo test -p sms-api --test reserved_role_live_postgres -- --ignored --nocapture
//! ```

use cratestack::CratestackContext;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack};
use sms_test_support::sealed::TestKey;

#[path = "support/sealed_fixtures.rs"]
mod sealed_fixtures;

/// Same reasoning as every other live suite's own copy of this mutex —
/// #102. Load-bearing here specifically because the test temporarily drops a
/// table-wide `CHECK` constraint — a schema-shape change every other test in
/// this binary would implicitly depend on staying in place. Holding this mutex
/// for the test's entire drop/seed/restore window is what makes that safe.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const TEST_PEPPER: &str = "reserved-role-live-postgres-test-pepper-over-the-minimum-length";

fn owner() -> CratestackContext {
    Principal {
        sub: "reserved-role-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
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

/// The rows [`seed_reserved_role_account`] created, plus the raw
/// pool needed to restore the constraint it dropped — kept alive for the
/// whole test (the `Role`/`User` rows have to stay in place, `roleKey`
/// still pointing at `"system"`, for the signed request itself to resolve
/// against), then torn down explicitly by [`Self::cleanup`] once the
/// test's own assertions are done.
///
/// `landing_role_key` is a second, harmless `Role` created alongside the
/// reserved one, for [`Self::cleanup`]'s own use — see that method's doc
/// for why deleting the reserved `Role` outright needs somewhere else for
/// `User.roleKey` to point first.
struct ReservedRoleFixture {
    raw_pool: cratestack::sqlx::PgPool,
    role_id: String,
    landing_role_key: String,
    user_id: String,
}

impl ReservedRoleFixture {
    /// Restores `roles_key_not_reserved_check` by first making the
    /// reserved-key `Role` row genuinely unreferenced, then deleting it.
    ///
    /// **Why this can't simply delete `User` first, the naive order:**
    /// `User` carries `@@soft_delete` — `db.user().delete()` sets
    /// `deletedAt`, it does not remove the row, so `roleKey` keeps
    /// pointing at the reserved `Role` and `users_role_key_fkey` (this
    /// schema's FK columns are auto-emitted as of `cratestack-migrate`
    /// 0.7.8+, per `AGENTS.md`) blocks deleting that `Role` regardless —
    /// confirmed live, not assumed: the first version of this method tried
    /// exactly that order and failed with `update or delete on table
    /// "roles" violates foreign key constraint "users_role_key_fkey"`.
    /// Reassigning `roleKey` to [`Self::landing_role_key`] first — a real,
    /// non-reserved `Role` this fixture also created — clears the
    /// reference without needing a hard delete `User`'s own policy
    /// structurally can't do.
    ///
    /// The reserved `Role` delete has to happen *before* the constraint
    /// restore: `ADD CONSTRAINT` validates every existing row, and that
    /// row is exactly the one that would fail it.
    async fn cleanup(self, db: &Cratestack) {
        // #59 (landed after this branch): User is @version'd now, and
        // cratestack refuses a versioned update with no If-Match at
        // runtime. The fixture doesn't carry a version — it would go
        // stale the moment anything else touched the row — so read the
        // current one here instead of threading a field through.
        let current = db
            .user()
            .find_unique(self.user_id.clone())
            .run(&owner())
            .await
            .expect("re-reading the test User for its current version")
            .expect("the test User still exists at cleanup time");
        db.user()
            .update(self.user_id)
            .set(schema::UpdateUserInput {
                roleKey: Some(self.landing_role_key.clone()),
                ..Default::default()
            })
            .if_match(current.version)
            .run(&owner())
            .await
            .expect("reassigning the test User off the reserved-key Role before deleting it");
        // cratestack 0.7.13 (cratestack#519): DELETE on an `@version` model
        // now enforces `If-Match`, the same reason the `User` reassignment
        // just above re-reads its current version rather than threading one
        // through this fixture. `Role` was never updated after creation, so
        // a fresh read here is always current.
        let current_role = db
            .role()
            .find_unique(self.role_id.clone())
            .run(&owner())
            .await
            .expect("re-reading the test Role for its current version")
            .expect("the test Role still exists at cleanup time");
        db.role()
            .delete(self.role_id)
            .if_match(current_role.version)
            .run(&owner())
            .await
            .expect("deleting the reserved-role test Role");

        cratestack::sqlx::query(
            "ALTER TABLE roles ADD CONSTRAINT roles_key_not_reserved_check \
             CHECK (key NOT IN ('system', 'app'))",
        )
        .execute(&self.raw_pool)
        .await
        .expect("restoring roles_key_not_reserved_check");
    }
}

/// Runs `body` as a task of its own, then [`ReservedRoleFixture::cleanup`], and
/// only then lets a failed assertion in `body` surface.
///
/// The fixture drops a table-wide `CHECK` and keeps it dropped until `cleanup`;
/// an assertion that panicked before reaching `cleanup` left the constraint down
/// in the per-binary database, and every other test in the file then failed on
/// the missing constraint instead of on its own assertion. Found when a
/// sabotage of the person-token path did exactly that.
async fn cleanup_after<F>(fixture: ReservedRoleFixture, db: &Cratestack, body: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let outcome = tokio::spawn(body).await;
    fixture.cleanup(db).await;
    if let Err(error) = outcome {
        std::panic::resume_unwind(error.into_panic());
    }
}

/// Seeds a `User` pointed at a `Role` keyed `"system"` —
/// a row `roles_key_not_reserved_check` (§2.10) makes unreachable through
/// any normal path, so this function briefly drops that constraint and
/// creates the row through the real `db.role().create()` delegate (R1:
/// only the constraint removal/restoration is raw SQL — every actual data
/// write goes through a real `CrateStack` delegate, audited and
/// policy-checked exactly as any other). This is the R1 exception named in
/// `cargo xtask no-raw-sqlx` and `CONTRIBUTING.md` for this file: schema
/// DDL to construct an otherwise-unreachable test fixture, not a
/// production data-access bypass.
///
/// **The constraint stays dropped until [`ReservedRoleFixture::cleanup`]
/// runs** — deliberately, not a bug: the whole point of this fixture is a
/// `User.roleKey` that still resolves to `"system"` for the login attempt
/// the test drives against it, and `ADD CONSTRAINT` would refuse to come
/// back while that row exists. Callers must hold [`TEST_MUTEX`] from
/// before this call until after `cleanup` returns — see that static's own
/// doc for why a dropped table-wide constraint is a bigger blast radius
/// than an ordinary row fixture.
async fn seed_reserved_role_account(
    db_url: &str,
    db: &Cratestack,
    suffix: &str,
) -> ReservedRoleFixture {
    let raw_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(db_url)
        .await
        .expect("connecting a raw pool for the constraint drop/restore");

    cratestack::sqlx::query("ALTER TABLE roles DROP CONSTRAINT roles_key_not_reserved_check")
        .execute(&raw_pool)
        .await
        .expect("dropping roles_key_not_reserved_check to construct the fixture");

    let role = db
        .role()
        .create(schema::CreateRoleInput {
            key: "system".to_owned(),
            label: "reserved role bypass test — see seed_reserved_role_account".to_owned(),
            description: None,
            permissions: " ".to_owned(),
        })
        .run(&owner())
        .await
        .expect("seeding the reserved-key Role while the constraint is down");

    // Not reserved, created with the constraint already back — this is
    // ReservedRoleFixture::cleanup's own landing spot for User.roleKey,
    // never used by the signed request itself.
    let landing_role_key = format!("rrlanding{}", suffix.to_lowercase());
    db.role()
        .create(schema::CreateRoleInput {
            key: landing_role_key.clone(),
            label: "reserved role test cleanup landing spot".to_owned(),
            description: None,
            permissions: " ".to_owned(),
        })
        .run(&owner())
        .await
        .expect("seeding the cleanup landing Role");

    let email = format!("reserved-role-{suffix}@example.test");
    let user = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("reserved-role-subject-{suffix}"),
            email,
            displayName: "Reserved Role Test User".to_owned(),
            roleKey: role.key,
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding a User pointed at the reserved-key Role");

    ReservedRoleFixture {
        raw_pool,
        role_id: role.id,
        landing_role_key,
        user_id: user.id,
    }
}

/// **The required guard-failure proof for the reserved-role escalation
/// found in review (#194's own house standard).** A `Role` keyed
/// `"system"`, assigned to a human `User`, must never let that human reach
/// `hasRole('system')` — proven here past the point where the database
/// constraint alone would normally stop it (see
/// [`seed_reserved_role_account`]'s own doc), so this test actually
/// exercises `sms_api::signers`' independent, point-of-use reserved-key
/// check, not just the database's.
///
/// The signature is good (the key resolves to the person), so the refusal is
/// the principal mapper's: the moment a request signed by a key paired to this
/// account reaches any real route and the signer directory resolves the
/// person's role, it is the coarse unsigned 401.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_role_keyed_system_never_authenticates_even_past_the_database_check() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;
    let suffix = unique_suffix();

    let fixture = seed_reserved_role_account(&db_url, &db, &suffix).await;
    let user_id = fixture.user_id.clone();
    let body_db = db.clone();
    // Cleanup runs even when an assertion below fails: see `cleanup_after`.
    cleanup_after(fixture, &db, async move {
        let db = body_db;
        let base = sealed_fixtures::serve(sealed_fixtures::api_router(&db, TEST_PEPPER)).await;

        // A console key paired to this account must be refused on every real
        // route, because `sms_api::signers` rejects a role_key of "system" - the
        // guard this test exists to prove.
        let key = TestKey::console();
        sealed_fixtures::pair_console_key(&db, &user_id, &key, chrono::Duration::days(30)).await;
        let roles = sealed_fixtures::client(&base, &key)
            .request("GET", "/roles")
            .send()
            .await;
        assert_eq!(
            roles.status,
            401,
            "a console key whose person's role is the reserved key \"system\" must never \
             authenticate against any real route: {}",
            roles.text()
        );
        assert!(!roles.sealed, "the refusal is the coarse unsigned 401");
    })
    .await;
}

/// Answers every token with one fixed person of the console: the verifier is not
/// what is under test, the point-of-use guard behind it is.
struct OnePerson {
    user_id: String,
    client_id: String,
}

#[async_trait::async_trait]
impl sms_api::BearerVerifier for OnePerson {
    async fn verify(
        &self,
        _token: &str,
    ) -> Result<sms_api::VerifiedBearer, sms_api::BearerRefusal> {
        Ok(sms_api::VerifiedBearer::Person {
            user_id: self.user_id.clone(),
            client_id: self.client_id.clone(),
        })
    }
}

/// The same guard on the other way in (VSA-4b): a person's **token** for the
/// account whose role is the reserved key `"system"` is the coarse 401, not an
/// `app`-kind context with the synthetic role. The control is the same router
/// answering 200 for an ordinary person, so the 401 is the reserved-role guard
/// and not a missing console client or a verifier that refuses everything.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_persons_token_for_a_role_keyed_system_never_authenticates() {
    let _guard = TEST_MUTEX.lock().await;
    let db_url = sms_test_support::database_url().await;
    let db = db().await;
    let suffix = unique_suffix();

    let fixture = seed_reserved_role_account(&db_url, &db, &suffix).await;
    let console_id = format!("console_{suffix}");
    db.oauth_client()
        .create(schema::CreateOauthClientInput {
            clientId: console_id.clone(),
            appClientId: None,
            tokenEndpointAuthMethod: schema::ClientAuthMethod::none,
            jwks: None,
            grantTypes: " authorization_code ".to_owned(),
            scopes: " openid profile ".to_owned(),
            redirectUris: " https://console.example/cb ".to_owned(),
            requirePkce: true,
        })
        .run(&sms_api::system_context("reserved-role-test"))
        .await
        .expect("seeding the console client");
    // An ordinary person (the cleanup landing role has no permissions, which
    // `previewMessage` does not ask for).
    let ordinary = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("reserved-role-ordinary-{suffix}"),
            email: format!("reserved-role-ordinary-{suffix}@example.test"),
            displayName: "Ordinary".to_owned(),
            roleKey: fixture.landing_role_key.clone(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding an ordinary person");

    let reserved_user = fixture.user_id.clone();
    let ordinary_user = ordinary.id.clone();
    let body_db = db.clone();
    // Cleanup runs even when an assertion below fails: see `cleanup_after`.
    cleanup_after(fixture, &db, async move {
        let db = body_db;
        let preview = |base: String| async move {
            reqwest::Client::new()
                .post(format!("{base}/$procs/previewMessage"))
                .bearer_auth(format!("tok-{}", unique_suffix()))
                .header("accept", "application/json")
                .header("content-type", "application/json")
                .body(r#"{"args":{"body":"hello"}}"#)
                .send()
                .await
                .expect("sending")
                .status()
                .as_u16()
        };
        let router_for = |user_id: String| {
            sealed_fixtures::api_router_doors(
                &db,
                TEST_PEPPER,
                sms_api::HmacWebhooks::Permitted,
                sms_api::ApiDoors::new(
                    true,
                    Some(std::sync::Arc::new(OnePerson {
                        user_id,
                        client_id: console_id.clone(),
                    })),
                )
                .expect("a door is open"),
            )
        };

        let control = sealed_fixtures::serve(router_for(ordinary_user.clone())).await;
        assert_eq!(
            preview(control).await,
            200,
            "an ordinary person's token acts"
        );
        let reserved = sealed_fixtures::serve(router_for(reserved_user.clone())).await;
        assert_eq!(
            preview(reserved).await,
            401,
            "a token for a person whose role is the reserved key \"system\" must never authenticate"
        );
    })
    .await;
}

/// **The first of two independent guards against a `Role` keyed `"system"` —
/// the database itself.** `roles_key_not_reserved_check`
/// (`backends/migrations/postgres/0002_bootstrap`, generated from
/// `docs/architecture.md` §2.10) rejects `key IN ('system', 'app')` at
/// `INSERT`, through the *real* `db.role().create()` delegate — no policy
/// bypass, no raw `sqlx` (R1). Without this, an `owner` could create a `Role`
/// named `"system"` through ordinary generated CRUD (`Role.create`'s own
/// `@@allow` is `hasRole('owner')`), assign a human `User` to it, and that
/// human's next signed request would satisfy `hasRole('system')` everywhere.
///
/// The second guard — `sms_api::signers` refusing a `"system"`/`"app"`
/// `role_key` at the point of use — is the test below, which has to drop this
/// exact constraint to construct the otherwise-unreachable row it needs.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_role_keyed_system_is_rejected_by_the_database_check() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let result = db
        .role()
        .create(schema::CreateRoleInput {
            key: "system".to_owned(),
            label: "should never be creatable".to_owned(),
            description: None,
            permissions: " ".to_owned(),
        })
        .run(&owner())
        .await;

    let error = result.expect_err("a Role keyed \"system\" must be rejected, not created");
    assert_eq!(
        error.db_sqlstate(),
        Some("23514"),
        "expected a Postgres check_violation (23514) from roles_key_not_reserved_check, got: \
         {error:?}"
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_role_keyed_app_is_also_rejected_by_the_database_check() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let result = db
        .role()
        .create(schema::CreateRoleInput {
            key: "app".to_owned(),
            label: "should never be creatable either".to_owned(),
            description: None,
            permissions: " ".to_owned(),
        })
        .run(&owner())
        .await;

    let error = result.expect_err("a Role keyed \"app\" must be rejected, not created");
    assert_eq!(error.db_sqlstate(), Some("23514"));
}
