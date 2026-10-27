//! Shared fixtures for the live suites that call the generated routes (VS-3).
//!
//! Since the envelope layer is `Required` for every generated route, a suite
//! that used to mint a bearer token signs instead. This file holds the pieces
//! every such suite needs and that cannot live in `sms-test-support` (it knows
//! nothing of the schema, and `sms-api` dev-depends on it): the router built
//! with a known server key, a listener, and rows that register a test key.
//!
//! It is included, not imported - `#[path = "..."] mod sealed_fixtures;` from
//! `sms-api`'s own suites and from `sms-gateway`'s - because a `tests/` helper
//! has no crate of its own. Every including crate has `sms_api`,
//! `sms_test_support` and `cratestack` as dependencies.
#![allow(dead_code)]

use chrono::{DateTime, Duration, Utc};
use cratestack::axum::Router;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::auth::system_context;
use sms_api::envelope::{ServerKeys, pem_of_seed};
use sms_api::schema::{self, Cratestack, app_client, user};
use sms_test_support::sealed::{SealedClient, TestKey};

/// The seed of the key the test servers seal with.
pub const SERVER_CURRENT_SEED: [u8; 32] = [0x51; 32];
/// The seed of the `next` key; clients pin it beside the current one.
pub const SERVER_NEXT_SEED: [u8; 32] = [0x52; 32];

pub fn server_keys() -> ServerKeys {
    ServerKeys::from_seeds(&SERVER_CURRENT_SEED, &SERVER_NEXT_SEED).expect("distinct seeds")
}

/// What a client pins: `[current, next]`.
pub fn server_public_keys() -> Vec<[u8; 32]> {
    let (current, next) = server_keys().public_keys();
    vec![current, next]
}

/// The two key files `sms-gateway serve` needs, written under `dir` with the
/// same seeds, so a spawned process seals with the key [`client`] pins.
pub fn write_server_key_files(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    std::fs::create_dir_all(dir).expect("creating the key directory");
    let current = dir.join("envelope-signing-key.pem");
    let next = dir.join("envelope-next-signing-key.pem");
    std::fs::write(&current, pem_of_seed(&SERVER_CURRENT_SEED)).expect("writing the key");
    std::fs::write(&next, pem_of_seed(&SERVER_NEXT_SEED)).expect("writing the next key");
    (current, next)
}

/// `sms_api::router` over `db`, sealing with the test server key and the
/// production budgets.
pub fn api_router(db: &Cratestack, pepper: &str) -> Router {
    sms_api::router(
        db.clone(),
        server_keys().into_signer(),
        sms_api::HashPepper::new(pepper).expect("test pepper meets the minimum length"),
        sms_api::DEFAULT_IDEMPOTENCY_TTL,
        sms_api::default_rate_limit_config(),
        sms_api::default_source_rate_limit_config(),
    )
    .expect("the envelope layer accepts its configuration")
}

/// Serves `router` on an ephemeral loopback port, with connect info (the
/// source limiter keys on it), and returns `http://127.0.0.1:port`.
pub async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding an ephemeral port");
    let addr = listener.local_addr().expect("reading the bound address");
    tokio::spawn(async move {
        cratestack::axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("serving the test app");
    });
    format!("http://{addr}")
}

/// A client of `base_url` signing with `key`, trusting the test server keys.
pub fn client(base_url: &str, key: &TestKey) -> SealedClient {
    SealedClient::new(
        base_url,
        key,
        &server_public_keys(),
        sms_api::schema::OP_CONTRACTS,
    )
}

fn sys() -> CratestackContext {
    system_context("sealed-fixtures")
}

/// The role ladder's top: the only context that may create a user in any
/// role, and update an `AppClient`.
pub fn owner() -> CratestackContext {
    sms_api::auth::Principal {
        sub: "sealed-fixtures-owner".to_owned(),
        kind: sms_api::PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// Registers `key` for the `AppClient` whose row id is `app_client_row_id`,
/// the way `registerAppClientKey` would (the thumbprint is computed by the
/// same function), under the system context, the one writer the policy admits.
pub async fn register_app_key(
    db: &Cratestack,
    app_client_row_id: &str,
    key: &TestKey,
) -> schema::ClientKey {
    let parsed = sms_api::client_keys::parse_public_key(&key.public_key_b64u())
        .expect("a test key is a valid Ed25519 key");
    assert_eq!(parsed.thumbprint, key.thumbprint_hex());
    db.client_key()
        .create(schema::CreateClientKeyInput {
            appClientId: app_client_row_id.to_owned(),
            alg: schema::ClientKeyAlg::ed25519,
            publicKey: parsed.bytes.to_vec(),
            thumbprint: parsed.thumbprint,
            label: Some("sealed fixture".to_owned()),
            revokedAt: None,
        })
        .run(&sys())
        .await
        .expect("registering the app key")
}

/// Pairs `key` to `user_id`, the way the CLI break-glass does, expiring after
/// `lives`. A negative `lives` plants an already-expired key (the table's
/// CHECK wants `expires_at > paired_at`, so `paired_at` is pushed back by the
/// same distance).
pub async fn pair_console_key(
    db: &Cratestack,
    user_id: &str,
    key: &TestKey,
    lives: Duration,
) -> schema::ConsoleKey {
    let parsed = sms_api::console_keys::parse_public_key(&key.public_key_b64u())
        .expect("a test console key is a valid P-256 key");
    assert_eq!(parsed.thumbprint, key.thumbprint_hex());
    let now: DateTime<Utc> = Utc::now();
    let paired_at = if lives < Duration::zero() {
        now + lives - Duration::minutes(1)
    } else {
        now
    };
    db.console_key()
        .create(schema::CreateConsoleKeyInput {
            userId: user_id.to_owned(),
            alg: schema::ConsoleKeyAlg::esp256,
            publicKey: parsed.bytes.to_vec(),
            thumbprint: parsed.thumbprint,
            label: Some("sealed fixture".to_owned()),
            pairedAt: paired_at,
            expiresAt: now + lives,
            revokedAt: None,
        })
        .run(&sys())
        .await
        .expect("pairing the console key")
}

/// A fresh active user in `role_key`, created as an owner (the only context
/// that may create any role), and a console key paired to them.
pub async fn seed_user_with_console_key(
    db: &Cratestack,
    role_key: &str,
    key: &TestKey,
) -> schema::User {
    let suffix = cratestack::uuid::Uuid::new_v4().simple().to_string();
    let created = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("pending-{suffix}"),
            email: format!("{suffix}@fixture.invalid"),
            displayName: "Sealed Fixture".to_owned(),
            roleKey: role_key.to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the user");
    pair_console_key(db, &created.id, key, Duration::days(30)).await;
    created
}

/// Marks an app client inactive, as a retirement would.
pub async fn deactivate_app_client(db: &Cratestack, app_client_row_id: &str) {
    let row = db
        .app_client()
        .find_many()
        .where_expr(FilterExpr::from(
            app_client::id().eq(app_client_row_id.to_owned()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("reading the app client")
        .into_iter()
        .next()
        .expect("the app client exists");
    db.app_client()
        .update(row.id)
        .set(schema::UpdateAppClientInput {
            active: Some(false),
            ..Default::default()
        })
        .if_match(row.version)
        .run(&owner())
        .await
        .expect("deactivating the app client");
}

/// Marks a user inactive.
pub async fn deactivate_user(db: &Cratestack, user_id: &str) {
    let row = db
        .user()
        .find_many()
        .where_expr(FilterExpr::from(user::id().eq(user_id.to_owned())))
        .limit(1)
        .run(&sys())
        .await
        .expect("reading the user")
        .into_iter()
        .next()
        .expect("the user exists");
    db.user()
        .update(row.id)
        .set(schema::UpdateUserInput {
            active: Some(false),
            ..Default::default()
        })
        .if_match(row.version)
        .run(&owner())
        .await
        .expect("deactivating the user");
}
