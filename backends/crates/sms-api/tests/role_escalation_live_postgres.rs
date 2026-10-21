//! Who may hand out `owner`.
//!
//! `owner` is the top of the console's role ladder: only an owner may delete a
//! `User` or change a `Role`'s permissions (`schema.cstack`, `model User` and
//! `model Role`). `User`'s `create`/`update` policy admits `admin` with no
//! condition on the *value* written, so an admin could, until this file's
//! fix, assign `owner` to anyone, itself included, or demote and deactivate an
//! owner. Production showed it on 2026-10-20: a user whose role was `admin`
//! updated its own record from `admin` to `owner` (cratestack_audit).
//!
//! Every case asserts the outcome, not the error type: a policy denial on an
//! update can surface as an error or as zero rows matched, and either is fine
//! as long as the row is unchanged when read back.
//!
//! Ignored by default like the other live suites:
//! cargo test -p sms-api --test role_escalation_live_postgres -- --ignored

use cratestack::CratestackContext;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack};

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

async fn db() -> Cratestack {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

fn principal(sub: &str, role: &str) -> CratestackContext {
    Principal {
        sub: sub.to_owned(),
        kind: PrincipalKind::User,
        role: role.to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CratestackContext {
    principal("role-escalation-test-owner", "owner")
}

fn suffix() -> String {
    cratestack::uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

/// A user with `role_key`, created by an owner (always allowed).
async fn seed_user(db: &Cratestack, role_key: &str) -> schema::User {
    let s = suffix();
    db.user()
        .create(schema::CreateUserInput {
            subject: format!("role-escalation-{s}"),
            email: format!("role-escalation-{s}@example.test"),
            displayName: format!("role escalation {s}"),
            roleKey: role_key.to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("an owner creates a user")
}

async fn role_of(db: &Cratestack, id: &str) -> (String, bool) {
    let user = db
        .user()
        .find_unique(id.to_owned())
        .run(&owner())
        .await
        .expect("reading the user back")
        .expect("the user exists");
    (user.roleKey, user.active)
}

async fn set(
    db: &Cratestack,
    ctx: &CratestackContext,
    user: &schema::User,
    input: schema::UpdateUserInput,
) {
    // The outcome is asserted by the caller; the result is deliberately ignored.
    let _ = db
        .user()
        .update(user.id.clone())
        .set(input)
        .if_match(user.version)
        .run(ctx)
        .await;
}

#[tokio::test]
#[ignore = "needs a live Postgres (sms-test-support)"]
async fn an_admin_cannot_promote_itself_to_owner() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let admin = seed_user(&db, "admin").await;
    let as_admin = principal(&admin.id, "admin");

    set(
        &db,
        &as_admin,
        &admin,
        schema::UpdateUserInput {
            roleKey: Some("owner".to_owned()),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(role_of(&db, &admin.id).await.0, "admin");
}

#[tokio::test]
#[ignore = "needs a live Postgres (sms-test-support)"]
async fn an_admin_cannot_promote_another_user_to_owner() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let admin = seed_user(&db, "admin").await;
    let target = seed_user(&db, "support").await;

    set(
        &db,
        &principal(&admin.id, "admin"),
        &target,
        schema::UpdateUserInput {
            roleKey: Some("owner".to_owned()),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(role_of(&db, &target.id).await.0, "support");
}

#[tokio::test]
#[ignore = "needs a live Postgres (sms-test-support)"]
async fn an_admin_cannot_create_an_owner() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let admin = seed_user(&db, "admin").await;
    let s = suffix();

    let result = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("role-escalation-{s}"),
            email: format!("role-escalation-{s}@example.test"),
            displayName: format!("role escalation {s}"),
            roleKey: "owner".to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&principal(&admin.id, "admin"))
        .await;

    assert!(result.is_err(), "an admin created an owner: {result:?}");
}

#[tokio::test]
#[ignore = "needs a live Postgres (sms-test-support)"]
async fn an_admin_cannot_demote_or_deactivate_an_owner() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let admin = seed_user(&db, "admin").await;
    let as_admin = principal(&admin.id, "admin");
    let target = seed_user(&db, "owner").await;

    set(
        &db,
        &as_admin,
        &target,
        schema::UpdateUserInput {
            roleKey: Some("support".to_owned()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(role_of(&db, &target.id).await, ("owner".to_owned(), true));

    set(
        &db,
        &as_admin,
        &target,
        schema::UpdateUserInput {
            active: Some(false),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(role_of(&db, &target.id).await, ("owner".to_owned(), true));
}

#[tokio::test]
#[ignore = "needs a live Postgres (sms-test-support)"]
async fn an_admin_still_manages_non_owner_users() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let admin = seed_user(&db, "admin").await;
    let target = seed_user(&db, "support").await;

    set(
        &db,
        &principal(&admin.id, "admin"),
        &target,
        schema::UpdateUserInput {
            roleKey: Some("operator".to_owned()),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(role_of(&db, &target.id).await.0, "operator");
}

#[tokio::test]
#[ignore = "needs a live Postgres (sms-test-support)"]
async fn an_owner_may_grant_owner() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let target = seed_user(&db, "admin").await;

    set(
        &db,
        &owner(),
        &target,
        schema::UpdateUserInput {
            roleKey: Some("owner".to_owned()),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(role_of(&db, &target.id).await.0, "owner");
}
