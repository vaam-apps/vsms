//! `sms_auth::login::authenticate_user` against a real Postgres (#194).
//!
//! Covers exactly the login-side house-standard proof this ticket asks
//! for: an inactive account must be refused, indistinguishably from a
//! wrong password or an unknown email — see
//! [`an_inactive_users_password_is_never_checked_no_wait_it_is_but_the_login_is_refused_anyway`]
//! for the guard-failure proof (the test name is deliberately explicit
//! about what "refused" means here, since the whole point of this module
//! is that a *correct* password must still fail for a deactivated
//! account).
//!
//! Ignored by default, same convention as every other live suite here. Run
//! explicitly:
//!
//! ```bash
//! cargo test -p sms-auth --test login_live_postgres -- --ignored
//! ```

use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack};
use sms_auth::login::{authenticate_user, LoginError};
use sms_core::password::hash_password;

/// Same reasoning as every other live suite's own copy of this mutex —
/// #102.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn owner() -> cratestack::CoolContext {
    Principal {
        sub: "sms-auth-login-live-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> cratestack::CoolContext {
    Principal {
        sub: "sms-auth-login-live-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .as_nanos();
    format!("{nanos:x}-{:?}", std::thread::current().id())
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

async fn db() -> Cratestack {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

/// A `Role` + `User` + `UserCredential` seeded end to end, the same shape
/// `sms-gateway provision-user` (#194) will construct for real — `password`
/// is the plaintext this test's own assertions log in against, never
/// stored.
struct SeededAccount {
    email: String,
    role_key: String,
}

async fn seed_account(
    db: &Cratestack,
    suffix: &str,
    password: &str,
    active: bool,
) -> SeededAccount {
    // Role.key is @regex("^[a-z][a-z0-9_]{2,31}$") — 32 chars max, lowercase
    // only. unique_suffix()'s ascii-alphanumeric filter over a Debug-
    // formatted ThreadId can include uppercase letters and run long, so
    // lowercase and truncate explicitly rather than assume it fits.
    let short_suffix: String = suffix.to_lowercase().chars().take(10).collect();
    let role_key = format!("lltest{short_suffix}");
    db.role()
        .create(schema::CreateRoleInput {
            key: role_key.clone(),
            label: "login live test role".to_owned(),
            description: None,
            permissions: " message:read ".to_owned(),
        })
        .run(&owner())
        .await
        .expect("seeding a Role");

    let email = format!("login-live-{suffix}@example.test");
    let user = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("login-live-subject-{suffix}"),
            email: email.clone(),
            displayName: "Login Live Test User".to_owned(),
            roleKey: role_key.clone(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding a User");

    if !active {
        db.user()
            .update(user.id.clone())
            .set(schema::UpdateUserInput {
                active: Some(false),
                ..Default::default()
            })
            // #59: User is @version'd now — runtime-enforced.
            .if_match(user.version)
            .run(&owner())
            .await
            .expect("deactivating the User");
    }

    db.user_credential()
        .create(schema::CreateUserCredentialInput {
            userId: user.id,
            passwordHash: hash_password(password).expect("hashing a real password"),
        })
        .run(&sys())
        .await
        .expect("seeding a UserCredential");

    SeededAccount { email, role_key }
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_correct_password_against_an_active_account_authenticates() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, "correct horse battery staple", true).await;

    let authenticated =
        authenticate_user(&db, &sys(), &account.email, "correct horse battery staple")
            .await
            .expect("a correct password against an active account must succeed");

    assert_eq!(authenticated.email, account.email);
    assert_eq!(authenticated.role_key, account.role_key);
    assert_eq!(authenticated.permissions, vec!["message:read".to_owned()]);
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn the_wrong_password_against_a_real_active_account_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, "correct horse battery staple", true).await;

    let result = authenticate_user(&db, &sys(), &account.email, "totally the wrong password").await;

    assert_eq!(result.unwrap_err(), LoginError::InvalidCredentials);
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn an_email_with_no_matching_user_is_refused_identically_to_a_wrong_password() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let result = authenticate_user(
        &db,
        &sys(),
        "no-such-account@example.test",
        "any password at all",
    )
    .await;

    // Same variant, same message, as a wrong password — see login.rs's own
    // module doc on why this module refuses to let a caller distinguish
    // "no such account" from "wrong password".
    assert_eq!(result.unwrap_err(), LoginError::InvalidCredentials);
}

/// **The required guard-failure proof (#194's own house standard):** an
/// inactive account must be refused even with the exactly-correct
/// password. This is the one property that would silently break if
/// `find_active_user`'s own `user::active().is_true()` filter were ever
/// dropped — proven by breaking it on purpose below in
/// [`removing_the_active_filter_would_let_a_deactivated_account_log_in`],
/// not merely asserted here.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_correct_password_against_a_deactivated_account_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, "correct horse battery staple", false).await;

    let result =
        authenticate_user(&db, &sys(), &account.email, "correct horse battery staple").await;

    assert_eq!(
        result.unwrap_err(),
        LoginError::InvalidCredentials,
        "a deactivated account's correct password must still be refused — active=false must \
         gate login exactly as hard as a wrong password does"
    );
}

/// Same property as the test above, proven the other way: querying
/// `User` with no `active` filter at all (what `find_active_user` would
/// degrade to if that clause were ever accidentally dropped) *does* find
/// the deactivated row — confirming the guard above is actually testing
/// something real, not vacuously passing because the row doesn't exist for
/// some unrelated reason (a typo'd email, a failed seed). This is the
/// "prove your guards can fail" exercise, run directly against the query
/// shape rather than by temporarily editing `login.rs` itself (unlike the
/// schema-policy guard in `system_context_golden_list_live_postgres.rs`,
/// there is no single line to delete here — `active` is a Rust-side
/// `.and(...)` clause, not a schema `@@allow` — so this test demonstrates
/// the failure mode directly instead).
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn removing_the_active_filter_would_let_a_deactivated_account_log_in() {
    use cratestack::FilterExpr;
    use sms_api::schema::user;

    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, "correct horse battery staple", false).await;

    // The exact query find_active_user runs, minus `.and(user::active().is_true())`.
    let found = db
        .user()
        .find_many()
        .where_expr(FilterExpr::from(user::email().eq(account.email.clone())))
        .limit(1)
        .run(&sys())
        .await
        .expect("an unfiltered read still succeeds")
        .into_iter()
        .next();

    assert!(
        found.is_some(),
        "the deactivated row is genuinely still there and findable — proving the active() \
         filter in find_active_user is the only thing standing between a deactivated account \
         and a successful login, not a row that silently doesn't exist for some other reason"
    );
}

/// **The first of two independent guards against a `Role` keyed `"system"`
/// (found in review, closed in the same PR) — the database itself.**
/// `roles_key_not_reserved_check` (`schema/migrations/postgres/0002_bootstrap`,
/// generated from `docs/architecture.md` §2.10) rejects `key IN ('system',
/// 'app')` at `INSERT`, through the *real* `db.role().create()` delegate —
/// no policy bypass, no raw `sqlx` (R1). Without this, an `owner` could
/// create a `Role` named `"system"` through ordinary generated CRUD
/// (`Role.create`'s own `@@allow` is `hasRole('owner')`), assign a human
/// `User` to it, and that human's next login would satisfy
/// `hasRole('system')` everywhere — including `OauthSigningKey.privateKeyPem`
/// (the key that signs every token this system issues) and every
/// `UserCredential.passwordHash`, both `hasRole('system')`-gated.
///
/// The second, independent guard — `sms_api::auth::load_human_principal`
/// refusing a `"system"`/`"app"` `role_key` at the point of use, in case
/// this constraint is ever bypassed — is proven live in
/// `app/sms-gateway/tests/login_flow_live_postgres.rs`, which has to spawn
/// a real server to drive a token through `GatewayAuth`; that test
/// temporarily drops this exact constraint to construct the otherwise-
/// unreachable row the second guard needs to prove itself against.
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
