//! `sms_auth::login::authenticate_user` against a real Postgres (#194, restored by
//! VSA-4a).
//!
//! Covers the login-side house-standard proof: an inactive account must be
//! refused, indistinguishably from a wrong password or an unknown email - see
//! [`removing_the_active_filter_would_let_a_deactivated_account_log_in`] for the
//! guard-failure proof - and, new with this restoration, that a good login stamps
//! `lastLoginAt` (which only works because `User.update` admits `system` again),
//! that a missing account costs the same Argon2 work as a wrong password, and that
//! a role a person must never hold ends the sign-in.
//!
//! Ignored by default, same convention as every other live suite here. Run
//! explicitly:
//!
//! ```bash
//! cargo test -p sms-auth --test login_live_postgres -- --ignored --test-threads=1
//! ```

use std::time::{Duration, Instant};

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack};
use sms_auth::login::{LoginError, authenticate_user};
use sms_core::password::hash_password;

/// Same reasoning as every other live suite's own copy of this mutex - #102.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn owner() -> CratestackContext {
    Principal {
        sub: "sms-auth-login-live-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// The context the test seeds and reads back with. `authenticate_user` builds its
/// own (`sms-gateway:login`), so a caller can never hand it a weaker one.
fn sys() -> CratestackContext {
    sms_api::system_context("sms-auth-login-live-test-system")
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

/// A `Role` + `User` (+ `UserCredential` unless `password` is `None`) seeded end to
/// end, the same shape `sms-gateway provision-user --password` constructs for
/// real. `password` is the plaintext this test's own assertions log in against,
/// never stored.
struct SeededAccount {
    user_id: String,
    email: String,
    role_key: String,
}

async fn seed_account(
    db: &Cratestack,
    suffix: &str,
    password: Option<&str>,
    active: bool,
) -> SeededAccount {
    // Role.key is @regex("^[a-z][a-z0-9_]{2,31}$") - 32 chars max, lowercase
    // only. unique_suffix()'s ascii-alphanumeric filter over a Debug-formatted
    // ThreadId can include uppercase letters and run long, so lowercase and
    // truncate explicitly rather than assume it fits.
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
            // #59: User is @version'd - runtime-enforced.
            .if_match(user.version)
            .run(&owner())
            .await
            .expect("deactivating the User");
    }

    if let Some(password) = password {
        db.user_credential()
            .create(schema::CreateUserCredentialInput {
                userId: user.id.clone(),
                passwordHash: hash_password(password).expect("hashing a real password"),
            })
            .run(&sys())
            .await
            .expect("seeding a UserCredential");
    }

    SeededAccount {
        user_id: user.id,
        email,
        role_key,
    }
}

async fn last_login_at(db: &Cratestack, user_id: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    db.user()
        .find_unique(user_id.to_owned())
        .run(&sys())
        .await
        .expect("reading the User back")
        .expect("the User exists")
        .lastLoginAt
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_correct_password_against_an_active_account_authenticates() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, Some("correct horse battery staple"), true).await;

    let authenticated = authenticate_user(&db, &account.email, "correct horse battery staple")
        .await
        .expect("a correct password against an active account must succeed");

    assert_eq!(authenticated.email, account.email);
    assert_eq!(authenticated.subject, account.user_id);
    assert_eq!(authenticated.role_key, account.role_key);
    assert_eq!(authenticated.permissions, vec!["message:read".to_owned()]);
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn the_wrong_password_against_a_real_active_account_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, Some("correct horse battery staple"), true).await;

    let result = authenticate_user(&db, &account.email, "totally the wrong password").await;

    assert!(
        matches!(result, Err(LoginError::InvalidCredentials)),
        "{result:?}"
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn an_email_with_no_matching_user_is_refused_identically_to_a_wrong_password() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let result =
        authenticate_user(&db, "no-such-account@example.test", "any password at all").await;

    // Same variant, same message, as a wrong password: see login.md on why a
    // caller must not be able to tell "no such account" from "wrong password".
    let error = result.expect_err("an unknown account must be refused");
    assert!(matches!(error, LoginError::InvalidCredentials), "{error:?}");
    assert_eq!(error.to_string(), "invalid email or password");
}

/// **The required guard-failure proof (#194's own house standard):** an inactive
/// account must be refused even with the exactly-correct password. This is the one
/// property that would silently break if `find_active_user`'s own
/// `user::active().is_true()` filter were ever dropped - proven by breaking it on
/// purpose in [`removing_the_active_filter_would_let_a_deactivated_account_log_in`],
/// not merely asserted here.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_correct_password_against_a_deactivated_account_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, Some("correct horse battery staple"), false).await;

    let result = authenticate_user(&db, &account.email, "correct horse battery staple").await;

    assert!(
        matches!(result, Err(LoginError::InvalidCredentials)),
        "a deactivated account's correct password must still be refused - active=false must \
         gate login exactly as hard as a wrong password does: {result:?}"
    );
    assert_eq!(
        last_login_at(&db, &account.user_id).await,
        None,
        "a refused login stamps nothing"
    );
}

/// Same property as the test above, proven the other way: querying `User` with no
/// `active` filter at all (what `find_active_user` would degrade to if that clause
/// were ever accidentally dropped) *does* find the deactivated row - confirming the
/// guard above is testing something real, not vacuously passing because the row
/// does not exist for some unrelated reason (a typo'd email, a failed seed).
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn removing_the_active_filter_would_let_a_deactivated_account_log_in() {
    use sms_api::schema::user;

    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, Some("correct horse battery staple"), false).await;

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
        "the deactivated row is genuinely still there and findable: the active() filter in \
         find_active_user is the only thing standing between a deactivated account and a \
         successful login"
    );
}

/// **The first of two independent guards against a `Role` keyed `"system"` - the
/// database itself.** `roles_key_not_reserved_check` rejects `key IN ('system',
/// 'app')` at `INSERT`, through the *real* `db.role().create()` delegate - no
/// policy bypass, no raw `sqlx` (R1). Without this, an `owner` could create a
/// `Role` named `"system"` through ordinary generated CRUD, assign a human `User`
/// to it, and that human's next login would satisfy `hasRole('system')` everywhere.
///
/// The second guard, `sms_api::signers` refusing the key at the point of use, and
/// the third, `authenticate_user` refusing it, are proven in
/// `sms-gateway`'s `login_flow_live_postgres.rs`, which drops this constraint to
/// build the otherwise unreachable row.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
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
#[ignore = "needs a live, migrated Postgres - see module docs"]
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

// --- New with VSA-4a -------------------------------------------------------

/// A good login stamps `lastLoginAt`. The write runs under the `system` context,
/// so it works only because `User.update` admits `hasRole('system')` again: take
/// the clause out of `schemas/vsms.cstack` and this is the test that fails (the
/// login still succeeds, which is why only a read-back can see it).
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_good_login_stamps_last_login_at_and_a_refused_one_does_not() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, Some("correct horse battery staple"), true).await;
    assert_eq!(last_login_at(&db, &account.user_id).await, None);

    authenticate_user(&db, &account.email, "not the password")
        .await
        .expect_err("refused");
    assert_eq!(
        last_login_at(&db, &account.user_id).await,
        None,
        "a wrong password stamps nothing"
    );

    let before = chrono::Utc::now() - chrono::Duration::seconds(5);
    authenticate_user(&db, &account.email, "correct horse battery staple")
        .await
        .expect("a correct password");
    let stamped = last_login_at(&db, &account.user_id)
        .await
        .expect("lastLoginAt must be stamped by a successful login");
    assert!(
        stamped > before && stamped <= chrono::Utc::now(),
        "{stamped} is not now"
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn an_account_with_no_password_is_refused_identically() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, None, true).await;

    let result = authenticate_user(&db, &account.email, "").await;
    let error = result.expect_err("an account with no credential cannot log in");
    assert!(matches!(error, LoginError::InvalidCredentials), "{error:?}");
    let result = authenticate_user(&db, &account.email, "anything").await;
    assert!(matches!(result, Err(LoginError::InvalidCredentials)));
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_soft_deleted_account_is_refused_with_the_right_password() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, Some("correct horse battery staple"), true).await;

    let version = db
        .user()
        .find_unique(account.user_id.clone())
        .run(&sys())
        .await
        .expect("reading the User")
        .expect("the User exists")
        .version;
    db.user()
        .delete(account.user_id.clone())
        // #59: User is @version'd - runtime-enforced.
        .if_match(version)
        .run(&owner())
        .await
        .expect("soft-deleting the User");

    let result = authenticate_user(&db, &account.email, "correct horse battery staple").await;
    assert!(
        matches!(result, Err(LoginError::InvalidCredentials)),
        "{result:?}"
    );
}

/// **A missing account costs the same Argon2 work as a wrong password.** Proven by
/// the clock, because the side channel it closes is the clock: the median of a few
/// unknown-email attempts is at least half the median of wrong-password attempts.
/// Without the dummy verification the unknown email returns after two reads
/// (single-digit milliseconds against tens), and this fails by an order of
/// magnitude, not by a margin a loaded machine could close.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn an_unknown_email_takes_as_long_as_a_wrong_password() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let account = seed_account(&db, &suffix, Some("correct horse battery staple"), true).await;

    async fn median(db: &Cratestack, email: &str, attempts: usize) -> Duration {
        let mut taken = Vec::new();
        for _ in 0..attempts {
            let started = Instant::now();
            authenticate_user(db, email, "a wrong password")
                .await
                .expect_err("refused");
            taken.push(started.elapsed());
        }
        taken.sort();
        taken[attempts / 2]
    }

    // One of each first, so the dummy hash and the pool are warm.
    authenticate_user(&db, "warm-up@example.test", "x")
        .await
        .expect_err("refused");
    let wrong = median(&db, &account.email, 5).await;
    let unknown = median(&db, "nobody-at-all@example.test", 5).await;
    assert!(
        wrong >= Duration::from_millis(5),
        "a real Argon2 verification takes longer than {wrong:?}; is the test measuring it?"
    );
    assert!(
        unknown * 2 >= wrong,
        "an unknown email ({unknown:?}) must cost about what a wrong password ({wrong:?}) does"
    );
}
