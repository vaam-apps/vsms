//! Who may hand out `owner` - over real HTTP, signed.
//!
//! This is the end-to-end half of the "who may hand out `owner`" fix
//! (2026-10-20, a production `admin` promoted itself to `owner`). `User`'s
//! `@@allow` can refuse an admin *creating* an owner or *editing a stored*
//! owner (`role_escalation_live_postgres.rs` pins that half), but an update
//! policy runs against the stored row and cannot see the new `roleKey`, so
//! promotion is refused by `sms_api::owner_grant::enforce_owner_grant_guard`,
//! a request middleware - which only a real HTTP request through the real
//! router can exercise. These tests drive `PATCH /users/{id}`, `POST /users`
//! and `POST /$procs/provisionUser` with the same JSON the console's
//! `updateUser` / `provisionUser` send (`frontends/packages/gateway/src/
//! users.ts`), each person signing with a console key paired to their
//! account. The encodings an attacker might smuggle `owner` in are the ones a
//! seal still permits - value spellings, an undecodable body - plus the
//! payload types the layer refuses before the guard runs.
//!
//! (This was the second half of `sms-gateway`'s `login_flow_live_postgres.rs`,
//! which VS-4 deleted with the login it tested; these cases never needed a
//! login and were kept, on the in-process router rather than a spawned
//! gateway: the guard is a layer of `sms_api::router`.)
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. Run explicitly:
//!
//! ```bash
//! cargo test -p sms-api --test owner_grant_live_postgres -- --ignored --nocapture
//! ```

use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack};
use sms_test_support::sealed::{Reply, SealedClient, TestKey};

#[path = "support/sealed_fixtures.rs"]
mod sealed_fixtures;

/// #102: see every other live suite's copy of this mutex.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const TEST_PEPPER: &str = "owner-grant-live-postgres-test-pepper-over-the-minimum-length";

fn owner() -> cratestack::CratestackContext {
    Principal {
        sub: "owner-grant-test-owner".to_owned(),
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

/// A seeded staff `User` on one of §5.2's built-in roles, with a console key
/// paired to their account: what the console holds after pairing (VS-5b).
struct StaffUser {
    id: String,
    key: TestKey,
}

/// An `owner`-context create (always allowed) of a `User` on `role_key`, plus
/// a paired console key. `label` keeps the rows of one test apart and readable
/// in a failure.
async fn seed_staff_user(db: &Cratestack, suffix: &str, label: &str, role_key: &str) -> StaffUser {
    let email = format!("owner-grant-{label}-{suffix}@example.test");
    let user = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("owner-grant-subject-{label}-{suffix}"),
            email: email.clone(),
            displayName: format!("Owner Grant {label}"),
            roleKey: role_key.to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding a staff User");
    let key = TestKey::console();
    sealed_fixtures::pair_console_key(db, &user.id, &key, chrono::Duration::days(30)).await;
    StaffUser { id: user.id, key }
}

/// A signed-in `admin`, a signed-in `owner`, and a third, lower-privileged
/// user for them to act on, all behind one real `sms_api::router` over HTTP.
struct OwnerGrantRig {
    db: Cratestack,
    suffix: String,
    admin: StaffUser,
    admin_api: SealedClient,
    owner_api: SealedClient,
}

async fn owner_grant_rig() -> OwnerGrantRig {
    let db = db().await;
    let suffix = unique_suffix();

    let admin = seed_staff_user(&db, &suffix, "admin", "admin").await;
    let owner_user = seed_staff_user(&db, &suffix, "owner", "owner").await;

    let base = sealed_fixtures::serve(sealed_fixtures::api_router(&db, TEST_PEPPER)).await;
    let admin_api = sealed_fixtures::client(&base, &admin.key);
    let owner_api = sealed_fixtures::client(&base, &owner_user.key);
    OwnerGrantRig {
        db,
        suffix,
        admin,
        admin_api,
        owner_api,
    }
}

impl OwnerGrantRig {
    async fn user(&self, id: &str) -> schema::User {
        self.db
            .user()
            .find_unique(id.to_owned())
            .run(&owner())
            .await
            .expect("reading the user back")
            .expect("the user exists")
    }

    async fn user_by_email(&self, email: &str) -> Option<schema::User> {
        self.db
            .user()
            .find_many()
            .where_expr(FilterExpr::from(schema::user::email().eq(email.to_owned())))
            .run(&owner())
            .await
            .expect("listing users by email")
            .into_iter()
            .next()
    }

    /// `PATCH /users/{id}` the way the console's `updateUser` sends it:
    /// JSON body, `If-Match` the row's current version.
    async fn patch_user(
        &self,
        api: &SealedClient,
        target: &schema::User,
        body: serde_json::Value,
    ) -> Reply {
        api.request("PATCH", "/users/{id}")
            .params(&[&target.id])
            .if_match(&format!("\"{}\"", target.version))
            .json(&body)
            .send()
            .await
    }
}

fn assert_forbidden(reply: &Reply, what: &str) {
    assert_eq!(
        reply.status,
        403,
        "{what} must be refused with 403, got {}: {}",
        reply.status,
        reply.text()
    );
    assert!(
        reply.sealed,
        "{what}: a refusal made past the seal is sealed"
    );
}

/// The incident, replayed: an `admin` sends `PATCH` for its OWN record to
/// `roleKey = owner`. Refused by the request middleware (an `UPDATE` policy
/// cannot see the new value), and the row is unchanged.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_admin_cannot_promote_itself_to_owner() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = owner_grant_rig().await;
    let before = rig.user(&rig.admin.id).await;

    let reply = rig
        .patch_user(
            &rig.admin_api,
            &before,
            serde_json::json!({ "roleKey": "owner" }),
        )
        .await;
    assert_forbidden(&reply, "an admin promoting itself to owner");

    let after = rig.user(&rig.admin.id).await;
    assert_eq!(after.roleKey, "admin");
    assert_eq!(
        after.version, before.version,
        "the refused write must not bump the row"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_admin_cannot_promote_another_user_to_owner() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = owner_grant_rig().await;
    let target = seed_staff_user(&rig.db, &rig.suffix, "target", "support").await;
    let before = rig.user(&target.id).await;

    let reply = rig
        .patch_user(
            &rig.admin_api,
            &before,
            serde_json::json!({ "roleKey": "owner", "displayName": "Promoted" }),
        )
        .await;
    assert_forbidden(&reply, "an admin promoting another user to owner");

    let after = rig.user(&target.id).await;
    assert_eq!(after.roleKey, "support");
    assert_eq!(after.displayName, before.displayName);
}

/// The same refusal with a body the console would never send but an attacker
/// might. A seal narrows what can be smuggled (the payload type is a bare
/// `type/subtype`, so there is no `charset` parameter to hide behind) and
/// moves part of the defence earlier: a CBOR payload is refused by the
/// envelope itself, unsigned, before any guard runs. What reaches the guard
/// is JSON, and the guard still decides on its value however it is spelled,
/// and fails closed on a body it cannot read.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_admin_cannot_smuggle_owner_past_the_guard_in_another_encoding() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = owner_grant_rig().await;
    let target = seed_staff_user(&rig.db, &rig.suffix, "smuggle", "support").await;
    let before = rig.user(&target.id).await;
    let if_match = format!("\"{}\"", before.version);

    // Odd spellings of the value, in JSON.
    for value in ["owner", "Owner", " owner "] {
        let reply = rig
            .admin_api
            .request("PATCH", "/users/{id}")
            .params(&[&target.id])
            .if_match(&if_match)
            .payload(
                format!("{{\"roleKey\":\"{value}\"}}").as_bytes(),
                "application/json",
            )
            .send()
            .await;
        assert_forbidden(&reply, &format!("roleKey {value:?} in sealed JSON"));
    }

    // CBOR {"roleKey": "owner"}: a payload type the router's codec cannot
    // read is refused by the envelope, unsigned, never forwarded.
    let mut cbor = vec![0xA1, 0x67];
    cbor.extend_from_slice(b"roleKey");
    cbor.push(0x65);
    cbor.extend_from_slice(b"owner");
    let reply = rig
        .admin_api
        .request("PATCH", "/users/{id}")
        .params(&[&target.id])
        .if_match(&if_match)
        .payload(&cbor, "application/cbor")
        .send()
        .await;
    assert_eq!(
        reply.status,
        415,
        "a CBOR payload the router cannot decode is refused, not forwarded: {}",
        reply.text()
    );
    assert!(!reply.sealed, "the layer's own refusal is unsigned");

    // An undecodable body from a non-owner fails closed.
    let reply = rig
        .admin_api
        .request("PATCH", "/users/{id}")
        .params(&[&target.id])
        .if_match(&if_match)
        .payload(b"{not json", "application/json")
        .send()
        .await;
    assert_eq!(reply.status, 400, "{}", reply.text());

    assert_eq!(rig.user(&target.id).await.roleKey, "support");
}

/// Creating an owner is refused two ways, on purpose: `POST /users` by the
/// request middleware, and `POST /$procs/provisionUser` (whose `User` create
/// runs under the caller's own context) by `User`'s `create` policy.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_admin_cannot_create_an_owner() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = owner_grant_rig().await;

    let post_email = format!("owner-grant-post-{}@example.test", rig.suffix);
    let reply = rig
        .admin_api
        .request("POST", "/users")
        .json(&serde_json::json!({
            "subject": format!("owner-grant-post-{}", rig.suffix),
            "email": post_email,
            "displayName": "Created Owner",
            "roleKey": "owner",
        }))
        .send()
        .await;
    assert_forbidden(&reply, "an admin creating an owner via POST /users");
    assert!(rig.user_by_email(&post_email).await.is_none());

    let prov_email = format!("owner-grant-prov-{}@example.test", rig.suffix);
    let reply = rig
        .admin_api
        .request("POST", "/$procs/provisionUser")
        .json(&serde_json::json!({ "args": {
            "email": prov_email,
            "displayName": "Provisioned Owner",
            "roleKey": "owner",
        }}))
        .send()
        .await;
    assert!(
        (400..500).contains(&reply.status),
        "an admin provisioning an owner must be refused, got {}: {}",
        reply.status,
        reply.text()
    );
    assert!(rig.user_by_email(&prov_email).await.is_none());
}

/// Controls: the guard must not break what admins and owners legitimately do.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_admin_still_manages_non_owners_and_an_owner_may_grant_owner() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = owner_grant_rig().await;

    // admin -> operator on another user: allowed.
    let target = seed_staff_user(&rig.db, &rig.suffix, "control", "support").await;
    let before = rig.user(&target.id).await;
    let reply = rig
        .patch_user(
            &rig.admin_api,
            &before,
            serde_json::json!({ "roleKey": "operator" }),
        )
        .await;
    assert!(
        (200..300).contains(&reply.status),
        "admin -> operator was refused ({}): {}",
        reply.status,
        reply.text()
    );
    assert_eq!(rig.user(&target.id).await.roleKey, "operator");

    // admin creating a non-owner via the provisioning procedure: allowed.
    let prov_email = format!("owner-grant-ok-{}@example.test", rig.suffix);
    let reply = rig
        .admin_api
        .request("POST", "/$procs/provisionUser")
        .json(&serde_json::json!({ "args": {
            "email": prov_email,
            "displayName": "Provisioned Operator",
            "roleKey": "operator",
        }}))
        .send()
        .await;
    assert!(
        (200..300).contains(&reply.status),
        "admin provisioning an operator was refused ({}): {}",
        reply.status,
        reply.text()
    );

    // owner -> owner: allowed (the middleware does not inspect an owner).
    let promote = seed_staff_user(&rig.db, &rig.suffix, "promote", "admin").await;
    let before = rig.user(&promote.id).await;
    let reply = rig
        .patch_user(
            &rig.owner_api,
            &before,
            serde_json::json!({ "roleKey": "owner" }),
        )
        .await;
    assert!(
        (200..300).contains(&reply.status),
        "owner -> owner was refused ({}): {}",
        reply.status,
        reply.text()
    );
    assert_eq!(rig.user(&promote.id).await.roleKey, "owner");
}
