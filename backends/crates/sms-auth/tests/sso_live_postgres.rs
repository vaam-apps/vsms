//! `sms_auth::sso::store` against a real Postgres: the single-use
//! transaction, the reaper, and just-in-time provisioning - including the
//! races the callback can actually hit.
//!
//! The HTTP half is `backends/apps/sms-gateway/tests/sso_flow_live_postgres.rs`;
//! this file pins the persistence guarantees underneath it, where a test can
//! make two callers genuinely concurrent.
//!
//! ```bash
//! cargo test -p sms-auth --test sso_live_postgres -- --ignored
//! ```

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack};
use sms_auth::sso::store::{self, TRANSACTION_TTL};
use sms_auth::sso::{AuthnSecrets, IdClaims, RawSsoConfig, Refusal, SsoConfig, SsoError};

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn owner() -> cratestack::CratestackContext {
    Principal {
        sub: "sms-auth-sso-live-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> cratestack::CratestackContext {
    Principal {
        sub: "sms-auth-sso-live-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{nanos:x}{}", COUNTER.fetch_add(1, Ordering::Relaxed))
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

fn config(issuer: &str) -> SsoConfig {
    SsoConfig::from_raw(RawSsoConfig {
        issuer: Some(issuer.to_owned()),
        client_id: Some("vsms".to_owned()),
        client_secret: Some("secret".to_owned()),
        redirect_uri: Some("https://vsms.example.test/login/sso/callback".to_owned()),
        required_group: Some("vsms-staff".to_owned()),
        accepted_amr: vec!["otp".to_owned()],
        ..RawSsoConfig::default()
    })
    .expect("valid")
    .expect("configured")
}

fn claims(suffix: &str) -> IdClaims {
    IdClaims {
        subject: format!("sub-{suffix}"),
        email: Some(format!("jit-{suffix}@example.test")),
        name: Some("Jit User".to_owned()),
        amr: vec!["otp".to_owned()],
        groups: vec!["vsms-staff".to_owned()],
        ..IdClaims::default()
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_transaction_is_single_use_and_stored_hashed() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let secrets = AuthnSecrets::generate();
    store::begin_transaction(&db, &sys(), &secrets, r#"{"client_id":"c"}"#, false)
        .await
        .expect("begin");

    // The raw state is nowhere in the table; its hash is.
    let rows = db
        .sso_transaction()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::sso_transaction::stateHash().eq(store::state_hash(&secrets.state)),
        ))
        .run(&sys())
        .await
        .expect("read");
    assert_eq!(rows.len(), 1);
    assert_ne!(rows[0].stateHash, secrets.state);

    let first = store::consume_transaction(&db, &sys(), &secrets.state)
        .await
        .expect("consume")
        .expect("the transaction is live");
    assert_eq!(first.nonce, secrets.nonce);
    assert_eq!(first.code_verifier, secrets.code_verifier);
    assert!(!first.is_retry);
    let second = store::consume_transaction(&db, &sys(), &secrets.state)
        .await
        .expect("consume");
    assert!(second.is_none(), "a second consume finds nothing");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn two_concurrent_consumers_of_one_state_cannot_both_win() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    for _ in 0..10 {
        let secrets = AuthnSecrets::generate();
        store::begin_transaction(&db, &sys(), &secrets, "{}", false)
            .await
            .expect("begin");
        let sys_a = sys();
        let sys_b = sys();
        let (a, b) = tokio::join!(
            store::consume_transaction(&db, &sys_a, &secrets.state),
            store::consume_transaction(&db, &sys_b, &secrets.state),
        );
        let winners = [a.expect("a"), b.expect("b")].into_iter().flatten().count();
        assert_eq!(winners, 1, "exactly one callback may consume a state");
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_expired_transaction_is_refused_and_the_reaper_removes_it() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let expired = format!("expired-{suffix}");
    db.sso_transaction()
        .create(schema::CreateSsoTransactionInput {
            stateHash: store::state_hash(&expired),
            nonce: "n".to_owned(),
            codeVerifier: "v".to_owned(),
            consoleRequest: "{}".to_owned(),
            isRetry: false,
            expiresAt: Utc::now() - Duration::seconds(1),
        })
        .run(&sys())
        .await
        .expect("seed expired");
    assert!(
        store::consume_transaction(&db, &sys(), &expired)
            .await
            .expect("consume")
            .is_none()
    );

    // And the reaper: one expired, one live; only the expired goes.
    let old = format!("old-{suffix}");
    db.sso_transaction()
        .create(schema::CreateSsoTransactionInput {
            stateHash: store::state_hash(&old),
            nonce: "n".to_owned(),
            codeVerifier: "v".to_owned(),
            consoleRequest: "{}".to_owned(),
            isRetry: false,
            expiresAt: Utc::now() - Duration::minutes(10),
        })
        .run(&sys())
        .await
        .expect("seed stale");
    let live = AuthnSecrets::generate();
    store::begin_transaction(&db, &sys(), &live, "{}", false)
        .await
        .expect("begin");
    let reaped = store::reap_expired(&db, &sys(), Utc::now(), 1000)
        .await
        .expect("reap");
    assert!(reaped >= 1);
    assert!(
        store::consume_transaction(&db, &sys(), &live.state)
            .await
            .expect("consume")
            .is_some(),
        "the reaper must not touch a live transaction (ttl {TRANSACTION_TTL})"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn first_sign_in_provisions_then_reuses_and_two_concurrent_first_sign_ins_make_one_user() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let cfg = config("https://idp.example.test/realms/live");
    let claims = claims(&suffix);

    // Two first sign-ins of the very same identity at once.
    let (sys_a, sys_b) = (sys(), sys());
    let (a, b) = tokio::join!(
        store::resolve_user(&db, &sys_a, &cfg, &claims),
        store::resolve_user(&db, &sys_b, &cfg, &claims),
    );
    let (a, b) = (a.expect("a"), b.expect("b"));
    assert_eq!(a.user_id, b.user_id, "one identity, one account");
    assert_eq!(
        [a.provisioned, b.provisioned]
            .into_iter()
            .filter(|p| *p)
            .count(),
        1,
        "exactly one of them created it"
    );

    let users = db
        .user()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::user::email().eq(claims.email.clone().unwrap()),
        ))
        .run(&sys())
        .await
        .expect("users");
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].roleKey, "support");
    assert_eq!(users[0].subject, users[0].id);

    // A later sign-in with a different email claim reuses the link.
    let renamed = IdClaims {
        email: Some(format!("other-{suffix}@example.test")),
        ..claims.clone()
    };
    let again = store::resolve_user(&db, &sys(), &cfg, &renamed)
        .await
        .expect("again");
    assert_eq!(again.user_id, a.user_id);
    assert!(!again.provisioned);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_email_already_held_locally_is_refused_and_creates_no_link() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let cfg = config("https://idp.example.test/realms/live");
    let claims = claims(&suffix);
    db.user()
        .create(schema::CreateUserInput {
            subject: format!("pending-{suffix}"),
            email: claims.email.clone().unwrap(),
            displayName: "Local".to_owned(),
            roleKey: "owner".to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("local user");
    let result = store::resolve_user(&db, &sys(), &cfg, &claims).await;
    assert!(
        matches!(result, Err(SsoError::Refused(Refusal::EmailInUse))),
        "{result:?}"
    );
    let links = db
        .federated_link()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::federated_link::subject().eq(claims.subject.clone()),
        ))
        .run(&sys())
        .await
        .expect("links");
    assert_eq!(links.len(), 0);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_token_with_no_usable_email_cannot_create_an_account() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let cfg = config("https://idp.example.test/realms/live");
    for email in [
        None,
        Some("not-an-email".to_owned()),
        Some("a b@c.test".to_owned()),
    ] {
        let c = IdClaims {
            email,
            ..claims(&suffix)
        };
        let result = store::resolve_user(&db, &sys(), &cfg, &c).await;
        assert!(
            matches!(result, Err(SsoError::Refused(Refusal::EmailMissing))),
            "{result:?}"
        );
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_same_subject_under_another_issuer_is_a_different_identity() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let one = config("https://idp-one.example.test/realms/a");
    let two = config("https://idp-two.example.test/realms/a");
    let first = store::resolve_user(&db, &sys(), &one, &claims(&suffix))
        .await
        .expect("first");
    // Same sub, other issuer: a new identity, and its (necessarily
    // different) email so the local-email guard is not what refuses it.
    let other_email = IdClaims {
        email: Some(format!("two-{suffix}@example.test")),
        ..claims(&suffix)
    };
    let second = store::resolve_user(&db, &sys(), &two, &other_email)
        .await
        .expect("second");
    assert_ne!(first.user_id, second.user_id);
}
