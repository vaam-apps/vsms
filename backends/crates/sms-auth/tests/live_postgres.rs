//! Exercises both stores against a real Postgres, not just the pure mapping
//! logic in `src/lib.rs`'s unit tests.
//!
//! Ignored by default — `just test` / `cargo test --workspace` has no
//! `DATABASE_URL` and must stay green without one (§4.2's own worked example
//! is why: "parses" is not "compiles" is not "applies", and the reverse is
//! also true — a pure unit test passing is not proof the delegate call
//! against a live database behaves the same way). Run explicitly:
//!
//! `sms_test_support` reads `VSMS_TEST_DATABASE_URL` (or starts its container,
//! see its own module doc), so running this needs a Postgres and:
//!
//! ```bash
//! cargo test -p sms-auth --test live_postgres -- --ignored --test-threads=1
//! ```
//!
//! **VSA-3a added the last block of tests**, which hold the *restored tables'*
//! access path: the OP reads and writes `oauth_clients`, `oauth_signing_keys` and
//! `client_assertions` through the `CrateStack` delegates under the internal
//! `system` context, although every one of those models is `@@internal("all")`
//! (no generated route). That the delegates exist at all, and that a non-system
//! context is refused, is asserted here against the real database.

use authkestra_engine::store::traits::ClientAssertionStore;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, ClientAuthMethod, Cratestack};
use sms_auth::provision::{ProvisionError, provision_token_client};
use sms_auth::{SmsClientAssertionStore, SmsClientStore};
use std::sync::Arc;

/// #102, found live: on a genuinely fresh database, this binary's own
/// tests — run concurrently by Rust's default multi-threaded test
/// harness — can race on Postgres's own `pg_type` catalog the first time
/// two of them prepare the exact same not-yet-cached query shape at the
/// same instant. See `backends/crates/sms-worker/tests/claim_live_postgres.rs`'s
/// own `TEST_MUTEX` doc for the full reasoning — same mechanism, same
/// fix.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// A `system`-role context — the only one `OauthClient` and `ClientAssertion`
/// admit.
fn sys() -> cratestack::CratestackContext {
    Principal {
        sub: "sms-auth-live-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// Enough entropy that two test runs against a shared database don't collide
/// on `clientId`/`jti` uniqueness — a monotonic clock reading plus the OS
/// thread id, not cryptographic randomness, which this has no need of.
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

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn find_client_reads_a_persisted_private_key_jwt_client() {
    let _guard = TEST_MUTEX.lock().await;
    let db = Arc::new(db().await);
    let sys = sys();

    let client_id = format!("test-otp-{}", unique_suffix());
    db.oauth_client()
        .create(schema::CreateOauthClientInput {
            clientId: client_id.clone(),
            appClientId: None,
            tokenEndpointAuthMethod: ClientAuthMethod::private_key_jwt,
            jwks: Some(r#"{"keys":[{"kty":"RSA","kid":"k1","n":"x","e":"AQAB"}]}"#.to_owned()),
            grantTypes: " client_credentials ".to_owned(),
            scopes: " sms:send ".to_owned(),
            redirectUris: " ".to_owned(),
            requirePkce: false,
        })
        .run(&sys)
        .await
        .expect("seeding a persisted client");

    // `&mut store`, not `&store` — `ClientStore::find_client` takes
    // `&mut self` as of authkestra-engine 0.8.0 (AGENTS.md's
    // authkestra-0.8 section, item A1); nothing in `SmsClientStore`'s own
    // body actually mutates anything, but the trait signature must be
    // matched exactly regardless.
    let mut store = SmsClientStore::new(db.clone(), sys.clone());
    let found = authkestra_op::ClientStore::find_client(&mut store, &client_id)
        .await
        .expect("delegate read succeeds")
        .expect("the persisted client is found");

    assert_eq!(found.client_id, client_id);
    assert_eq!(found.client_secret_hash, None);
    assert_eq!(
        found.token_endpoint_auth_method,
        Some(authkestra_op::TokenEndpointAuthMethod::PrivateKeyJwt)
    );
    assert!(found.jwks.is_some());
    assert_eq!(
        found.grant_types,
        vec![authkestra_op::GrantType::ClientCredentials]
    );

    // No cleanup: OauthClient declares no `@@allow("delete", ...)` (§4.2 —
    // client rows are deactivated, never deleted), and `unique_suffix()`
    // means a re-run doesn't collide with what this run left behind. Run
    // against the throwaway database the module docs describe and `dropdb`
    // it afterwards, same as `ci/apply-migrations.sh`'s own worked example.
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn find_client_returns_none_for_an_unknown_client() {
    let _guard = TEST_MUTEX.lock().await;
    let db = Arc::new(db().await);
    let mut store = SmsClientStore::new(db, sys());

    let found = authkestra_op::ClientStore::find_client(&mut store, "no-such-client")
        .await
        .expect("a missing client is Ok(None), not an error");

    assert!(found.is_none());
}

/// The replay check depends on `db_sqlstate()` surviving the framework's
/// sqlx→`CratestackError` conversion. Through `cratestack-sqlx` `=0.5.2` it did
/// not — every generated write discarded SQLSTATE and constraint, so a
/// replay fell through to "opaque fault" instead of "already spent"
/// ([cratestack/cratestack#267](https://github.com/cratestack/cratestack/issues/267),
/// [vymalo/vsms#87](https://github.com/vymalo/vsms/issues/87)). Fixed in
/// `cratestack-sqlx` 0.6.0.
///
/// This test was written to assert the correct behaviour and deliberately
/// left failing while the bug was live, so it would go green the moment the
/// pin moved rather than the regression going unnoticed a second time. It
/// now passes. Keep it: a hand-constructed `CratestackError::DatabaseTyped` never
/// exercises that conversion, so this is the only shape of test that can
/// see the regression come back.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn record_jti_is_true_once_and_false_on_replay() {
    let _guard = TEST_MUTEX.lock().await;
    let db = Arc::new(db().await);
    let sys = sys();
    // `mut` — `ClientAssertionStore::record_jti` takes `&mut self` as of
    // authkestra-engine 0.8.0 (AGENTS.md item A1). The trait itself moved
    // out of `authkestra_op` into `authkestra_engine::store::traits` in
    // the same release (item A3) — `authkestra_op` re-exports its two
    // in-memory implementations but not the trait, so this file imports
    // it from its real home now, unlike `ClientStore`/`GrantType`/
    // `TokenEndpointAuthMethod` below, which stay reachable through
    // `authkestra_op`'s own compatibility re-export.
    let mut store = SmsClientAssertionStore::new(db, sys);

    let jti = format!("test-jti-{}", unique_suffix());
    let expires_at = chrono::Utc::now() + chrono::Duration::minutes(5);

    let first = ClientAssertionStore::record_jti(&mut store, &jti, expires_at)
        .await
        .expect("first presentation succeeds");
    assert!(first, "first use of a jti must be accepted");

    let second = ClientAssertionStore::record_jti(&mut store, &jti, expires_at)
        .await
        .expect("a replay is a false, not an error");
    assert!(!second, "a replayed jti must be refused");
}

// --- VSA-3a: the restored tables through the delegates -----------------------

/// An `owner`: the strongest human role, and not `system`.
fn owner() -> cratestack::CratestackContext {
    Principal {
        sub: "sms-auth-live-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

async fn seed_app(db: &Cratestack, active: bool) -> schema::App {
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "sms-auth provisioning test app".to_owned(),
            slug: format!("sms-auth-{}", &unique_suffix()[..12]),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding an app");
    if active {
        return app;
    }
    db.app()
        .update(app.id.clone())
        .set(schema::UpdateAppInput {
            active: Some(false),
            ..Default::default()
        })
        .if_match(app.version)
        .run(&owner())
        .await
        .expect("deactivating the app")
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn the_restored_tables_answer_the_system_context_and_refuse_everyone_else() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // The OP's context reads each table. (An empty result is a pass: the table
    // is there and the policy admitted the read.)
    db.oauth_client()
        .find_many()
        .limit(1)
        .run(&sys())
        .await
        .expect("system reads oauth_clients");
    db.oauth_signing_key()
        .find_many()
        .limit(1)
        .run(&sys())
        .await
        .expect("system reads oauth_signing_keys");
    db.client_assertion()
        .find_many()
        .limit(1)
        .run(&sys())
        .await
        .expect("system reads client_assertions");

    // A human `owner` is not the system context: `@@allow` admits `system` only,
    // and a list that is filtered by policy is an empty list, never the rows.
    // Seed one so empty means refused, not "nothing there".
    provision_token_client(
        &db,
        &sys(),
        &seed_app(&db, true).await.id,
        "policy probe",
        &["sms:send".to_owned()],
    )
    .await
    .expect("provisioning a probe client");
    for (table, rows) in [
        (
            "oauth_clients",
            db.oauth_client()
                .find_many()
                .run(&owner())
                .await
                .map(|r| r.len()),
        ),
        (
            "oauth_signing_keys",
            db.oauth_signing_key()
                .find_many()
                .run(&owner())
                .await
                .map(|r| r.len()),
        ),
    ] {
        assert!(
            matches!(rows, Err(_) | Ok(0)),
            "{table} must show an owner nothing, got {rows:?}"
        );
    }
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn rotating_keeps_one_signing_key_and_overlaps_the_previous_one_in_the_jwks() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let sys = sys();
    let issuer = "https://sms.example.test";

    let first = sms_auth::op::rotate_signing_key(&db, &sys, chrono::Duration::minutes(30))
        .await
        .expect("rotating in a first key");
    let second = sms_auth::op::rotate_signing_key(&db, &sys, chrono::Duration::minutes(30))
        .await
        .expect("rotating in a second key");
    assert_ne!(first, second);

    let (signing, jwks) = sms_auth::op::load_signing_keys(&db, &sys, issuer)
        .await
        .expect("loading the keys");
    let kids: Vec<String> = jwks.iter().filter_map(|jwk| jwk.kid.clone()).collect();
    assert!(
        kids.contains(&first) && kids.contains(&second),
        "the rotated-out key keeps publishing for its overlap, got {kids:?}"
    );
    assert_eq!(
        signing.public_jwk().and_then(|jwk| jwk.kid).as_deref(),
        Some(second.as_str()),
        "the newest key signs"
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_provisioned_client_is_a_private_key_jwt_client_holding_only_the_public_half() {
    let _guard = TEST_MUTEX.lock().await;
    let db = Arc::new(db().await);
    let sys = sys();
    let app = seed_app(&db, true).await;

    let provisioned =
        provision_token_client(&db, &sys, &app.id, "otp sender", &["sms:send".to_owned()])
            .await
            .expect("provisioning");
    assert!(provisioned.client_id.starts_with("appc_"));
    assert!(
        provisioned.private_key_pem.contains("BEGIN PRIVATE KEY"),
        "the key comes back once, as PKCS#8"
    );

    // The OP's own store reads it back as a private_key_jwt, client_credentials
    // client, with a JWK set and no secret.
    let mut store = SmsClientStore::new(db.clone(), sys.clone());
    let found = authkestra_op::ClientStore::find_client(&mut store, &provisioned.client_id)
        .await
        .expect("delegate read succeeds")
        .expect("the provisioned client is found");
    assert_eq!(
        found.token_endpoint_auth_method,
        Some(authkestra_op::TokenEndpointAuthMethod::PrivateKeyJwt)
    );
    assert_eq!(found.client_secret_hash, None);
    assert_eq!(
        found.grant_types,
        vec![authkestra_op::GrantType::ClientCredentials],
        "no refresh_token and no authorization_code, ever, for a machine client"
    );
    assert_eq!(found.scopes, vec!["sms:send"]);

    // Only the public half is stored.
    let stored = db
        .oauth_client()
        .find_many()
        .run(&sys)
        .await
        .expect("reading the registry")
        .into_iter()
        .find(|row| row.clientId == provisioned.client_id)
        .expect("the row exists");
    let jwks = stored.jwks.expect("a JWK set is stored");
    assert!(jwks.contains("\"n\"") && jwks.contains("\"e\""));
    assert!(
        !jwks.contains("\"d\"") && !jwks.contains("PRIVATE"),
        "no private material in the registry"
    );
    // It is the principal the AppClient names.
    assert!(stored.appClientId.is_some());
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn provisioning_refuses_the_pair_scope_an_unknown_app_and_an_inactive_one() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let sys = sys();
    let app = seed_app(&db, true).await;

    assert!(matches!(
        provision_token_client(&db, &sys, &app.id, "x", &["console:pair".to_owned()]).await,
        Err(ProvisionError::PairScope(_))
    ));
    assert!(matches!(
        provision_token_client(
            &db,
            &sys,
            "capp0000000000000000000000",
            "x",
            &["sms:send".to_owned()]
        )
        .await,
        Err(ProvisionError::AppNotFound(_))
    ));
    let inactive = seed_app(&db, false).await;
    assert!(matches!(
        provision_token_client(&db, &sys, &inactive.id, "x", &["sms:send".to_owned()]).await,
        Err(ProvisionError::AppInactive(_))
    ));
}
