//! `provisionAppClient` (#23), as VS-4 reshaped it, against a real, fully
//! migrated Postgres: the caller sends the public half of an Ed25519 key it
//! generated, the procedure makes an `AppClient` and registers that key in
//! one transaction, and nothing secret comes back because vsms never held a
//! private key.
//!
//! What this suite proves that no unit test can:
//!
//! 1. The `AppClient` and `ClientKey` rows persist, linked
//!    (`ClientKey.appClientId -> AppClient.id`), with the caller's exact key
//!    bytes and the RFC 9679 thumbprint, and the result names both.
//! 2. The client is real end to end: a request **signed by the matching
//!    private key** is accepted by the envelope layer of a router built over
//!    the same database, and reaches the route its scope admits (`GET /jobs`
//!    with `job:read`). A look-alike row would not authenticate.
//! 3. The refusals leave nothing behind: a malformed key, a key already
//!    registered to another client, an unknown or inactive app, and the
//!    `console:pair` scope each fail without creating an `AppClient`.
//!
//! `sms_test_support` provisions Postgres and applies every migration
//! automatically (a shared, self-healing container - see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-api --test provision_app_client_live_postgres -- --ignored
//! ```

#[path = "support/sealed_fixtures.rs"]
mod sealed_fixtures;

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind, system_context};
use sms_api::schema::{
    self, Cratestack, procedures::ProcedureRegistry, procedures::provision_app_client,
};
use sms_api::{HashPepper, Procedures};
use sms_test_support::sealed::TestKey;

/// #102: on a genuinely fresh database two tests of one binary can race on
/// Postgres's own `pg_type` catalog the first time they prepare the same
/// query shape. See `claim_live_postgres.rs`'s `TEST_MUTEX` doc.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const PEPPER: &str = "provision-app-client-live-postgres-test-pepper-over-the-minimum";

fn owner() -> CratestackContext {
    Principal {
        sub: "provision-app-client-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> CratestackContext {
    system_context("provision-app-client-test")
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

async fn seed_app(db: &Cratestack, active: bool) -> schema::App {
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "provision app client test app".to_owned(),
            slug: format!("provision-test-{}", unique_suffix()),
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

fn args(
    app_id: &str,
    label: &str,
    scopes: &[&str],
    public_key: &str,
) -> provision_app_client::Args {
    provision_app_client::Args {
        args: schema::ProvisionClientInput {
            appId: app_id.to_owned(),
            label: label.to_owned(),
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            publicKey: public_key.to_owned(),
        },
    }
}

async fn provision(
    db: &Cratestack,
    args: provision_app_client::Args,
) -> Result<schema::ProvisionClientResult, CratestackError> {
    let procedures = Procedures::new(HashPepper::new(PEPPER).expect("pepper meets the minimum"));
    let ctx = owner();
    provision_app_client::invoke_with_db(db, &args, &ctx, |db, authorized| {
        let (p, c, a) = (procedures.clone(), ctx.clone(), args.clone());
        async move { p.provision_app_client(&db, &c, a, authorized).await }
    })
    .await
}

async fn clients_labelled(db: &Cratestack, label: &str) -> Vec<schema::AppClient> {
    db.app_client()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::app_client::label().eq(label.to_owned()),
        ))
        .run(&sys())
        .await
        .expect("reading AppClient rows back")
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn provisioning_persists_a_linked_client_and_key_and_returns_nothing_secret() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db, true).await;
    let key = TestKey::app();
    let label = format!("provisioned-{}", unique_suffix());

    let result = provision(
        &db,
        args(
            &app.id,
            &label,
            &["sms:send", "sms:read"],
            &key.public_key_b64u(),
        ),
    )
    .await
    .expect("provisioning a client");

    assert!(result.clientId.starts_with("appc_"), "{}", result.clientId);
    assert_eq!(result.thumbprint, key.thumbprint_hex());

    let clients = clients_labelled(&db, &label).await;
    assert_eq!(clients.len(), 1, "exactly one AppClient is made");
    let client = &clients[0];
    assert_eq!(client.clientId, result.clientId);
    assert_eq!(client.appId, app.id);
    assert_eq!(client.scopes, " sms:send sms:read ");
    assert!(client.active);

    let stored = db
        .client_key()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::client_key::thumbprint().eq(result.thumbprint.clone()),
        ))
        .run(&sys())
        .await
        .expect("reading the ClientKey back");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, result.keyId);
    assert_eq!(
        stored[0].appClientId, client.id,
        "the key hangs off the new client"
    );
    assert_eq!(stored[0].alg, schema::ClientKeyAlg::ed25519);
    assert!(stored[0].revokedAt.is_none());
    let parsed = sms_api::client_keys::parse_public_key(&key.public_key_b64u()).unwrap();
    assert_eq!(stored[0].publicKey, parsed.bytes.to_vec());
}

/// The point of the exercise: the row is a credential. A router built over
/// this database accepts a request signed by the private key that matches the
/// registered public one, as the new client, and applies its scope.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_provisioned_client_authenticates_with_its_private_key_and_its_scope_applies() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db, true).await;
    let base = sealed_fixtures::serve(sealed_fixtures::api_router(&db, PEPPER)).await;

    for (scopes, expected) in [(&["job:read"][..], 200), (&["sms:send"][..], 403)] {
        let key = TestKey::app();
        provision(
            &db,
            args(
                &app.id,
                &format!("auth-{}", unique_suffix()),
                scopes,
                &key.public_key_b64u(),
            ),
        )
        .await
        .expect("provisioning a client");

        let reply = sealed_fixtures::client(&base, &key)
            .request("GET", "/jobs")
            .send()
            .await;
        assert_eq!(
            reply.status,
            expected,
            "a client scoped {scopes:?} reading /jobs: {}",
            reply.text()
        );
        assert!(reply.sealed, "the answer is sealed: {}", reply.text());
    }

    // A key nobody registered is refused whole: the coarse unsigned 401.
    let stranger = TestKey::app();
    let reply = sealed_fixtures::client(&base, &stranger)
        .request("GET", "/jobs")
        .send()
        .await;
    assert_eq!(reply.status, 401, "{}", reply.text());
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn refusals_create_no_client() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db, true).await;

    // A key that is not an Ed25519 point encoding.
    let label = format!("bad-key-{}", unique_suffix());
    let refused = provision(&db, args(&app.id, &label, &["sms:send"], "AAAA")).await;
    assert!(
        matches!(refused, Err(CratestackError::Validation(_))),
        "{refused:?}"
    );
    assert!(clients_labelled(&db, &label).await.is_empty());

    // The console:pair scope is the operator's, never an HTTP caller's.
    let label = format!("pair-{}", unique_suffix());
    let refused = provision(
        &db,
        args(
            &app.id,
            &label,
            &["console:pair"],
            &TestKey::app().public_key_b64u(),
        ),
    )
    .await;
    assert!(
        matches!(refused, Err(CratestackError::Forbidden(_))),
        "{refused:?}"
    );
    assert!(clients_labelled(&db, &label).await.is_empty());

    // An app that does not exist, and one that is inactive.
    let label = format!("no-app-{}", unique_suffix());
    let refused = provision(
        &db,
        args(
            "zzzzzzzzzzzzzzzzzzzzzzzz",
            &label,
            &["sms:send"],
            &TestKey::app().public_key_b64u(),
        ),
    )
    .await;
    assert!(
        matches!(refused, Err(CratestackError::NotFound(_))),
        "{refused:?}"
    );
    let inactive = seed_app(&db, false).await;
    let label = format!("inactive-{}", unique_suffix());
    let refused = provision(
        &db,
        args(
            &inactive.id,
            &label,
            &["sms:send"],
            &TestKey::app().public_key_b64u(),
        ),
    )
    .await;
    assert!(
        matches!(refused, Err(CratestackError::Validation(_))),
        "{refused:?}"
    );
    assert!(clients_labelled(&db, &label).await.is_empty());
}

/// A public key is one identity: provisioning a second client with a key
/// that is already registered is a conflict, and the second client is not
/// left half-made.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_key_already_registered_cannot_back_a_second_client() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db, true).await;
    let key = TestKey::app();

    provision(
        &db,
        args(
            &app.id,
            &format!("first-{}", unique_suffix()),
            &["sms:send"],
            &key.public_key_b64u(),
        ),
    )
    .await
    .expect("the first provisioning succeeds");

    let label = format!("second-{}", unique_suffix());
    let refused = provision(
        &db,
        args(&app.id, &label, &["sms:send"], &key.public_key_b64u()),
    )
    .await;
    assert!(
        matches!(refused, Err(CratestackError::Conflict(_))),
        "{refused:?}"
    );
    assert!(
        clients_labelled(&db, &label).await.is_empty(),
        "the refused call left no AppClient behind"
    );
}
