//! Exercises both stores against a real Postgres, not just the pure mapping
//! logic in `src/lib.rs`'s unit tests.
//!
//! Ignored by default — `just test` / `cargo test --workspace` has no
//! `DATABASE_URL` and must stay green without one (§4.2's own worked example
//! is why: "parses" is not "compiles" is not "applies", and the reverse is
//! also true — a pure unit test passing is not proof the delegate call
//! against a live database behaves the same way). Run explicitly:
//!
//! `sms_test_support` provisions Postgres and applies both migrations
//! automatically (a shared, self-healing container — see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-auth --test live_postgres -- --ignored
//! ```

use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, ClientAuthMethod, Cratestack};
use sms_auth::{SmsClientAssertionStore, SmsClientStore};
use std::sync::Arc;

/// #102, found live: on a genuinely fresh database, this binary's own
/// tests — run concurrently by Rust's default multi-threaded test
/// harness — can race on Postgres's own `pg_type` catalog the first time
/// two of them prepare the exact same not-yet-cached query shape at the
/// same instant. See `crates/sms-worker/tests/claim_live_postgres.rs`'s
/// own `TEST_MUTEX` doc for the full reasoning — same mechanism, same
/// fix.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// A `system`-role context — the only one `OauthClient` and `ClientAssertion`
/// admit.
fn sys() -> cratestack::CoolContext {
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

    let store = SmsClientStore::new(db.clone(), sys.clone());
    let found = authkestra_op::ClientStore::find_client(&store, &client_id)
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
    let store = SmsClientStore::new(db, sys());

    let found = authkestra_op::ClientStore::find_client(&store, "no-such-client")
        .await
        .expect("a missing client is Ok(None), not an error");

    assert!(found.is_none());
}

/// The replay check depends on `db_sqlstate()` surviving the framework's
/// sqlx→`CoolError` conversion. Through `cratestack-sqlx` `=0.5.2` it did
/// not — every generated write discarded SQLSTATE and constraint, so a
/// replay fell through to "opaque fault" instead of "already spent"
/// ([cratestack/cratestack#267](https://github.com/cratestack/cratestack/issues/267),
/// [vymalo/vsms#87](https://github.com/vymalo/vsms/issues/87)). Fixed in
/// `cratestack-sqlx` 0.6.0.
///
/// This test was written to assert the correct behaviour and deliberately
/// left failing while the bug was live, so it would go green the moment the
/// pin moved rather than the regression going unnoticed a second time. It
/// now passes. Keep it: a hand-constructed `CoolError::DatabaseTyped` never
/// exercises that conversion, so this is the only shape of test that can
/// see the regression come back.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn record_jti_is_true_once_and_false_on_replay() {
    let _guard = TEST_MUTEX.lock().await;
    let db = Arc::new(db().await);
    let sys = sys();
    let store = SmsClientAssertionStore::new(db, sys);

    let jti = format!("test-jti-{}", unique_suffix());
    let expires_at = chrono::Utc::now() + chrono::Duration::minutes(5);

    let first = authkestra_op::ClientAssertionStore::record_jti(&store, &jti, expires_at)
        .await
        .expect("first presentation succeeds");
    assert!(first, "first use of a jti must be accepted");

    let second = authkestra_op::ClientAssertionStore::record_jti(&store, &jti, expires_at)
        .await
        .expect("a replay is a false, not an error");
    assert!(!second, "a replayed jti must be refused");
}
