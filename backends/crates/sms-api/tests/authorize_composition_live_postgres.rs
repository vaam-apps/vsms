//! `authorizeComposition` over a real HTTP round trip against a real Postgres:
//! the gateway's statement that the person behind a sealed request may send.
//!
//! The console's composer spends a standing service key for a browser only if
//! this call, sealed by the browser's own key, is answered with that person's
//! name. The failure this file exists to prevent is the one the first design
//! had: a sealed read answered `200` for *every* signer the envelope accepts,
//! so any app client of any app, and any paired person of any role, could
//! spend the key. Each refusal below is therefore asserted as a non-2xx the
//! layer sealed, and the control (an admin) as a `200` that names the admin.
//!
//! ```bash
//! cargo test -p sms-api --test authorize_composition_live_postgres -- --ignored
//! ```

#[path = "support/sealed_fixtures.rs"]
mod sealed_fixtures;

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sealed_fixtures::{
    api_router, client, owner, register_app_key, seed_user_with_console_key, serve,
};
use sms_api::auth::system_context;
use sms_api::schema::{self, Cratestack, role};
use sms_test_support::sealed::TestKey;

const PEPPER: &str = "authorize-composition-live-test-pepper-well-over-the-minimum";

const HASH: &str = "3a6eb0790f39ac87c94f3856b2dd2c5d110e6811602261a9a923d3bb23adc8b7";

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CratestackContext {
    system_context("authorize-composition-live-test")
}

fn suffix() -> String {
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

struct Rig {
    db: Cratestack,
    base: String,
}

async fn rig() -> Rig {
    let db = db().await;
    let base = serve(api_router(&db, PEPPER)).await;
    Rig { db, base }
}

fn body(operation: &str, hash: &str) -> serde_json::Value {
    serde_json::json!({"args": {"operation": operation, "inputSha256": hash}})
}

async fn call(
    rig: &Rig,
    key: &TestKey,
    operation: &str,
    hash: &str,
) -> sms_test_support::sealed::Reply {
    client(&rig.base, key)
        .request("POST", "/$procs/authorizeComposition")
        .json(&body(operation, hash))
        .send()
        .await
}

/// Rewrites the permission column of a built-in role, returning what was there.
async fn set_permissions(db: &Cratestack, role_key: &str, permissions: &str) -> String {
    let row = db
        .role()
        .find_many()
        .where_expr(FilterExpr::from(role::key().eq(role_key.to_owned())))
        .limit(1)
        .run(&sys())
        .await
        .expect("reading the role")
        .into_iter()
        .next()
        .expect("the role is seeded");
    db.role()
        .update(row.id.clone())
        .set(schema::UpdateRoleInput {
            permissions: Some(permissions.to_owned()),
            ..Default::default()
        })
        .if_match(row.version)
        .run(&owner())
        .await
        .expect("updating the role");
    row.permissions
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_paired_admin_is_named_with_the_operation_and_the_hash_it_asked_for() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let key = TestKey::console();
    let admin = seed_user_with_console_key(&rig.db, "admin", &key).await;

    let reply = call(&rig, &key, "send", HASH).await;

    assert!(reply.sealed, "{}", reply.text());
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        reply.json(),
        serde_json::json!({
            "userId": admin.id,
            "role": "admin",
            "operation": "send",
            "inputSha256": HASH,
        })
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_app_client_of_any_app_is_refused_even_holding_sms_send() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let tag = suffix();
    let app = rig
        .db
        .app()
        .create(schema::CreateAppInput {
            name: "authorize composition test app".to_owned(),
            slug: format!("authorize-composition-{tag}"),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the app");
    let client_row = rig
        .db
        .app_client()
        .create(schema::CreateAppClientInput {
            appId: app.id,
            clientId: format!("appc_authorize_{tag}"),
            label: "authorize composition test client".to_owned(),
            scopes: " sms:send sms:read ".to_owned(),
            lastUsedAt: None,
            retiredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the app client");
    let key = TestKey::app();
    register_app_key(&rig.db, &client_row.id, &key).await;

    let reply = call(&rig, &key, "send", HASH).await;

    assert!(reply.sealed, "{}", reply.text());
    assert_eq!(reply.status, 403, "{}", reply.text());
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_person_whose_role_may_not_send_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    for role_key in ["auditor", "support", "developer"] {
        let key = TestKey::console();
        seed_user_with_console_key(&rig.db, role_key, &key).await;

        let reply = call(&rig, &key, "send", HASH).await;

        assert_eq!(reply.status, 403, "{role_key}: {}", reply.text());
        assert!(reply.sealed, "{role_key}");
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_role_that_lost_the_sms_send_permission_is_refused_at_layer_two() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let key = TestKey::console();
    seed_user_with_console_key(&rig.db, "operator", &key).await;
    let before = set_permissions(&rig.db, "operator", " sms:read ").await;

    let reply = call(&rig, &key, "preview", HASH).await;
    let restored = set_permissions(&rig.db, "operator", &before).await;

    assert_eq!(reply.status, 403, "{}", reply.text());
    assert!(
        reply.text().contains("sms:send"),
        "Layer 2 names the permission it wants: {}",
        reply.text()
    );
    assert_eq!(restored, " sms:read ");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_unknown_operation_or_a_malformed_hash_is_refused_for_an_admin() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let key = TestKey::console();
    seed_user_with_console_key(&rig.db, "admin", &key).await;

    for (operation, hash) in [
        ("delete", HASH),
        ("send", "not-a-hash"),
        ("send", &HASH.to_uppercase()),
    ] {
        let reply = call(&rig, &key, operation, hash).await;
        assert_eq!(reply.status, 422, "{operation} {hash}: {}", reply.text());
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_proof_is_single_use() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let key = TestKey::console();
    seed_user_with_console_key(&rig.db, "admin", &key).await;
    let prepared = client(&rig.base, &key)
        .request("POST", "/$procs/authorizeComposition")
        .json(&body("send", HASH))
        .seal()
        .await;

    let first = prepared.resend().await;
    let replay = prepared.resend().await;

    // The first delivery of a fresh nonce is answered (resend never opens the
    // answer, so only the status is read); the second is refused before the
    // procedure runs, with the layer's unsigned 401.
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(replay.status, 401, "{}", replay.text());
    assert!(!replay.sealed);
}
