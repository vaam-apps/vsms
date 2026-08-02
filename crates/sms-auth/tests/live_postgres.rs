//! Exercises both stores against a real Postgres, not just the pure mapping
//! logic in `src/lib.rs`'s unit tests.
//!
//! Ignored by default — `just test` / `cargo test --workspace` has no
//! `DATABASE_URL` and must stay green without one (§4.2's own worked example
//! is why: "parses" is not "compiles" is not "applies", and the reverse is
//! also true — a pure unit test passing is not proof the delegate call
//! against a live database behaves the same way). Run explicitly:
//!
//! ```bash
//! docker run --rm -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:16
//! createdb vsms_check
//! DATABASE_URL=postgres://postgres:postgres@localhost/vsms_check ./ci/apply-migrations.sh
//! DATABASE_URL=postgres://postgres:postgres@localhost/vsms_check \
//!     cargo test -p sms-auth --test live_postgres -- --ignored
//! ```

use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, ClientAuthMethod, Cratestack};
use sms_auth::{SmsClientAssertionStore, SmsClientStore};
use std::sync::Arc;

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
    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must point at a migrated database — see module docs");
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
    let db = Arc::new(db().await);
    let store = SmsClientStore::new(db, sys());

    let found = authkestra_op::ClientStore::find_client(&store, "no-such-client")
        .await
        .expect("a missing client is Ok(None), not an error");

    assert!(found.is_none());
}

/// Asserts the **correct** behaviour, which cratestack-sqlx 0.5.0 does not
/// currently deliver — see the crate root's `KNOWN ISSUE` doc for the full
/// evidence. Every write-path query in `cratestack-sqlx` (`create`, `update`,
/// `delete`, `upsert`, and their `_many`/`_exec` variants) maps the
/// underlying `sqlx::Error` with `CoolError::Database(error.to_string())`
/// rather than `cool_error_from_sqlx`, so `db_sqlstate()` is `None` on every
/// database-rejected write, framework-wide — not something
/// `SmsClientAssertionStore` can work around from the calling side, because
/// the typed SQLSTATE is discarded before a `CoolError` ever reaches this
/// crate.
///
/// This test is expected to **fail** until
/// [cratestack/cratestack#267](https://github.com/cratestack/cratestack/issues/267)
/// lands and the pin here moves past it — see
/// [vymalo/vsms#87](https://github.com/vymalo/vsms/issues/87) for the
/// tracking issue. Left failing rather than deleted so it goes green the
/// moment the pin is bumped, instead of the regression going unnoticed a
/// second time.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn record_jti_is_true_once_and_false_on_replay() {
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
