//! `registerAppClientKey` / `revokeAppClientKey` (VS-2) against a real, fully
//! migrated Postgres - through `invoke_with_db`, the entry point the HTTP route
//! uses, so each procedure's `@allow` is the real one, plus the generated
//! delegate under several roles to pin `ClientKey`'s own write policy.
//!
//! The denial tests are the point of this file's second half. `ClientKey`'s
//! `create`/`update` are `hasRole('system')` only and `read` admits owner,
//! admin and system; the repo's recurring failure is a `hasRole('system')`
//! clause that is missing or too wide and nothing says so. Each test below
//! was run against a schema with that clause loosened and failed (the PR
//! description carries the captured output), so a green run means the policy
//! is what these assertions say it is.
//!
//! ```bash
//! cargo test -p sms-api --test client_keys_live_postgres -- --ignored
//! ```

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, client_key, procedures::ProcedureRegistry,
    procedures::register_app_client_key, procedures::revoke_app_client_key,
};
use sms_api::{HashPepper, Procedures};

/// #102: one live test at a time per binary - see
/// `replay_webhook_attempt_live_postgres.rs`'s identical mutex.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn principal(role: &str, kind: PrincipalKind) -> CratestackContext {
    Principal {
        sub: format!("client-keys-test-{role}"),
        kind,
        role: role.to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn human(role: &str) -> CratestackContext {
    principal(role, PrincipalKind::User)
}

fn sys() -> CratestackContext {
    principal("system", PrincipalKind::App)
}

fn procedures() -> Procedures {
    Procedures::new(
        HashPepper::new("client-keys-live-postgres-test-pepper-well-over-the-minimum")
            .expect("test pepper meets HashPepper::new's minimum length"),
    )
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .subsec_nanos();
    format!("{:06x}", (u64::from(nanos).wrapping_add(n)) % 0x0100_0000)
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

/// A fresh App and one `AppClient` of it, written the way their sole writers
/// (`create-app`, `provisionAppClient`) do: App as owner, `AppClient` as system.
async fn seed_client(db: &Cratestack) -> schema::AppClient {
    seed_client_scoped(db, " sms:send ").await
}

async fn seed_client_scoped(db: &Cratestack, scopes: &str) -> schema::AppClient {
    let suffix = unique_suffix();
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "client keys live test app".to_owned(),
            slug: format!("client-keys-test-{suffix}"),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&human("owner"))
        .await
        .expect("seeding the app");
    db.app_client()
        .create(schema::CreateAppClientInput {
            appId: app.id,
            clientId: format!("appc_client-keys-test-{suffix}"),
            label: "client keys live test".to_owned(),
            scopes: scopes.to_owned(),
            lastUsedAt: None,
            retiredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the app client")
}

/// The public half of a fresh Ed25519 key pair, distinct per call and per run.
/// The seed is hashed from a counter, the process id and the clock; the test
/// only ever needs the public bytes.
fn fresh_public_key() -> [u8; 32] {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_nanos();
    let seed: [u8; 32] = Sha256::digest(format!(
        "client-keys-test-{n}-{}-{nanos}",
        std::process::id()
    ))
    .into();
    cratestack::cose::Ed25519Signer::from_seed(&seed)
        .verify_key()
        .ed25519_bytes()
        .expect("an Ed25519 signer's verify key is Ed25519")
}

async fn register(
    db: &Cratestack,
    ctx: &CratestackContext,
    app_client_id: &str,
    public_key: &str,
    label: Option<&str>,
) -> Result<schema::ClientKey, CratestackError> {
    let procedures = procedures();
    let args = register_app_client_key::Args {
        args: schema::RegisterClientKeyInput {
            appClientId: app_client_id.to_owned(),
            publicKey: public_key.to_owned(),
            label: label.map(str::to_owned),
        },
    };
    register_app_client_key::invoke_with_db(db, &args, ctx, |db, authorized| {
        let (p, c, a) = (procedures.clone(), ctx.clone(), args.clone());
        async move { p.register_app_client_key(&db, &c, a, authorized).await }
    })
    .await
}

async fn revoke(
    db: &Cratestack,
    ctx: &CratestackContext,
    key_id: &str,
) -> Result<schema::ClientKey, CratestackError> {
    let procedures = procedures();
    let args = revoke_app_client_key::Args {
        args: schema::RevokeClientKeyInput {
            keyId: key_id.to_owned(),
        },
    };
    revoke_app_client_key::invoke_with_db(db, &args, ctx, |db, authorized| {
        let (p, c, a) = (procedures.clone(), ctx.clone(), args.clone());
        async move { p.revoke_app_client_key(&db, &c, a, authorized).await }
    })
    .await
}

async fn rows_for_thumbprint(db: &Cratestack, thumbprint: &str) -> Vec<schema::ClientKey> {
    db.client_key()
        .find_many()
        .where_expr(FilterExpr::from(
            client_key::thumbprint().eq(thumbprint.to_owned()),
        ))
        .run(&sys())
        .await
        .expect("reading client keys back as system")
}

fn b64(public: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(public)
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn registering_stores_the_key_with_its_rfc_9679_thumbprint() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let client = seed_client(&db).await;
    let public = fresh_public_key();

    let key = register(
        &db,
        &human("owner"),
        &client.id,
        &b64(&public),
        Some("composer"),
    )
    .await
    .expect("an owner registers a key");

    assert_eq!(key.appClientId, client.id);
    assert_eq!(key.alg, schema::ClientKeyAlg::ed25519);
    assert_eq!(key.publicKey, public.to_vec());
    assert_eq!(key.label.as_deref(), Some("composer"));
    assert!(key.revokedAt.is_none());
    assert_eq!(
        key.thumbprint,
        hex::encode(cratestack::cose::thumbprint::okp_ed25519_thumbprint(
            &public
        )),
        "the stored thumbprint is the crate's RFC 9679 value"
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn registering_the_same_key_twice_leaves_one_row_and_returns_it_again() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let client = seed_client(&db).await;
    let public = b64(&fresh_public_key());

    let first = register(&db, &human("admin"), &client.id, &public, None)
        .await
        .expect("first registration");
    let second = register(&db, &human("owner"), &client.id, &public, Some("ignored"))
        .await
        .expect("a repeat is not an error");

    assert_eq!(first.id, second.id);
    assert_eq!(second.label, None, "an idempotent repeat changes nothing");
    assert_eq!(rows_for_thumbprint(&db, &first.thumbprint).await.len(), 1);
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn two_concurrent_registrations_of_one_key_both_succeed_with_one_row() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let client = seed_client(&db).await;
    let public = b64(&fresh_public_key());
    let owner = human("owner");

    let (a, b) = tokio::join!(
        register(&db, &owner, &client.id, &public, None),
        register(&db, &owner, &client.id, &public, None),
    );
    let (a, b) = (a.expect("first racer"), b.expect("second racer"));

    assert_eq!(a.id, b.id);
    assert_eq!(rows_for_thumbprint(&db, &a.thumbprint).await.len(), 1);
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_key_registered_to_one_client_is_refused_for_another() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let (one, other) = (seed_client(&db).await, seed_client(&db).await);
    let public = b64(&fresh_public_key());
    register(&db, &human("owner"), &one.id, &public, None)
        .await
        .expect("registering for the first client");

    let error = register(&db, &human("owner"), &other.id, &public, None)
        .await
        .expect_err("one key cannot speak for two clients");
    assert!(matches!(error, CratestackError::Conflict(_)), "{error:?}");
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_revoked_key_stays_revoked_and_cannot_be_registered_again() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let client = seed_client(&db).await;
    let public = b64(&fresh_public_key());
    let key = register(&db, &human("owner"), &client.id, &public, None)
        .await
        .expect("registering");

    let revoked = revoke(&db, &human("admin"), &key.id)
        .await
        .expect("revoking");
    let first_stamp = revoked.revokedAt.expect("revocation stamps revokedAt");
    let again = revoke(&db, &human("owner"), &key.id)
        .await
        .expect("revoking twice is not an error");
    assert_eq!(
        again.revokedAt,
        Some(first_stamp),
        "the first revocation time is kept"
    );

    let error = register(&db, &human("owner"), &client.id, &public, None)
        .await
        .expect_err("a revoked key is never reinstated");
    assert!(matches!(error, CratestackError::Conflict(_)), "{error:?}");
    assert_eq!(rows_for_thumbprint(&db, &key.thumbprint).await.len(), 1);
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn bad_input_is_a_clean_error_and_writes_nothing() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let client = seed_client(&db).await;
    let owner = human("owner");
    let public = b64(&fresh_public_key());

    for bad in ["", "%%%", &b64(&[1; 32])[..20], &format!("{public}=")] {
        let error = register(&db, &owner, &client.id, bad, None)
            .await
            .expect_err("a malformed key");
        assert!(
            matches!(error, CratestackError::Validation(_)),
            "{bad:?}: {error:?}"
        );
    }
    let error = register(&db, &owner, &client.id, &public, Some("  "))
        .await
        .expect_err("a blank label");
    assert!(matches!(error, CratestackError::Validation(_)), "{error:?}");

    let error = register(&db, &owner, "nosuchclient000000000000", &public, None)
        .await
        .expect_err("an unknown client");
    assert!(matches!(error, CratestackError::NotFound(_)), "{error:?}");

    let error = revoke(&db, &owner, "nosuchkey00000000000000")
        .await
        .expect_err("an unknown key");
    assert!(matches!(error, CratestackError::NotFound(_)), "{error:?}");

    let thumbprint = hex::encode(cratestack::cose::thumbprint::okp_ed25519_thumbprint(
        &URL_SAFE_NO_PAD
            .decode(&public)
            .expect("the test key is base64url")
            .try_into()
            .expect("32 bytes"),
    ));
    assert_eq!(rows_for_thumbprint(&db, &thumbprint).await.len(), 0);
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_retired_client_cannot_gain_a_key() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let client = seed_client(&db).await;
    db.app_client()
        .update(client.id.clone())
        .set(schema::UpdateAppClientInput {
            active: Some(false),
            ..Default::default()
        })
        .if_match(client.version)
        .run(&human("owner"))
        .await
        .expect("deactivating the client");

    let error = register(
        &db,
        &human("owner"),
        &client.id,
        &b64(&fresh_public_key()),
        None,
    )
    .await
    .expect_err("a deactivated client");
    assert!(matches!(error, CratestackError::Validation(_)), "{error:?}");
}

/// The procedures' own `@allow`: only owner and admin. `developer`, `operator`
/// and `support` hold other console permissions; `app` is a machine caller; and
/// `system` is the synthetic role no HTTP token may carry - a procedure that
/// admitted it would hand every internal writer's power to a request.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn only_owner_and_admin_may_call_the_procedures() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let client = seed_client(&db).await;
    let public = b64(&fresh_public_key());
    let key = register(
        &db,
        &human("owner"),
        &client.id,
        &b64(&fresh_public_key()),
        None,
    )
    .await
    .expect("an owner registers a key");

    for role in ["developer", "operator", "support", "auditor"] {
        let error = register(&db, &human(role), &client.id, &public, None)
            .await
            .expect_err("registering");
        assert!(
            matches!(error, CratestackError::Forbidden(_)),
            "{role}: {error:?}"
        );
        let error = revoke(&db, &human(role), &key.id)
            .await
            .expect_err("revoking");
        assert!(
            matches!(error, CratestackError::Forbidden(_)),
            "{role}: {error:?}"
        );
    }
    for denied in [principal("app", PrincipalKind::App), sys()] {
        let error = register(&db, &denied, &client.id, &public, None)
            .await
            .expect_err("registering");
        assert!(matches!(error, CratestackError::Forbidden(_)), "{error:?}");
        let error = revoke(&db, &denied, &key.id).await.expect_err("revoking");
        assert!(matches!(error, CratestackError::Forbidden(_)), "{error:?}");
    }
    let still = rows_for_thumbprint(&db, &key.thumbprint).await;
    assert_eq!(still.len(), 1);
    assert!(
        still[0].revokedAt.is_none(),
        "no denied call revoked the key"
    );
}

/// `ClientKey`'s own policy through the generated delegate - what a request to
/// `POST /client_keys` / `PATCH /client_keys/{id}` would reach. Writes are
/// `hasRole('system')` only: a human `owner` is refused, which is what keeps
/// the thumbprint computed by the procedure rather than by the caller.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn the_generated_routes_cannot_write_a_key_except_as_system() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let client = seed_client(&db).await;
    let planted = |n: u8| schema::CreateClientKeyInput {
        appClientId: client.id.clone(),
        alg: schema::ClientKeyAlg::ed25519,
        publicKey: vec![n; 32],
        thumbprint: format!("{:0>64}", format!("{n:02x}{}", unique_suffix())),
        label: None,
        revokedAt: None,
    };

    for role in ["owner", "admin", "developer", "operator", "support"] {
        let error = db
            .client_key()
            .create(planted(1))
            .run(&human(role))
            .await
            .expect_err("a human planting a key through the generated route");
        assert!(
            matches!(error, CratestackError::Forbidden(_)),
            "{role}: {error:?}"
        );
    }
    let error = db
        .client_key()
        .create(planted(2))
        .run(&principal("app", PrincipalKind::App))
        .await
        .expect_err("an app planting a key");
    assert!(matches!(error, CratestackError::Forbidden(_)), "{error:?}");

    let created = db
        .client_key()
        .create(planted(3))
        .run(&sys())
        .await
        .expect("system is the one writer the policy admits");

    for role in ["owner", "admin", "developer", "operator"] {
        let error = db
            .client_key()
            .update(created.id.clone())
            .set(schema::UpdateClientKeyInput {
                revokedAt: Some(Some(chrono::Utc::now())),
                ..Default::default()
            })
            .run(&human(role))
            .await
            .expect_err("a human stamping revokedAt through the generated route");
        assert!(
            matches!(
                error,
                CratestackError::Forbidden(_) | CratestackError::NotFound(_)
            ),
            "{role}: {error:?}"
        );
        let error = db
            .client_key()
            .delete(created.id.clone())
            .run(&human(role))
            .await
            .expect_err("deleting a key: there is no delete action");
        assert!(
            matches!(
                error,
                CratestackError::Forbidden(_) | CratestackError::NotFound(_)
            ),
            "{role}: {error:?}"
        );
    }
    let still = rows_for_thumbprint(&db, &created.thumbprint).await;
    assert_eq!(still.len(), 1, "no human role deleted the row");
    assert!(still[0].revokedAt.is_none(), "no human role revoked it");
}

/// Reads: owner and admin list a client's keys; no other human role and no
/// app sees a row (the denial is an empty list, not an error).
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn only_owner_admin_and_system_can_read_keys() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let client = seed_client(&db).await;
    let key = register(
        &db,
        &human("owner"),
        &client.id,
        &b64(&fresh_public_key()),
        None,
    )
    .await
    .expect("registering");

    let visible = |ctx: CratestackContext| {
        let (db, id) = (&db, key.id.clone());
        async move {
            db.client_key()
                .find_many()
                .where_expr(FilterExpr::from(client_key::id().eq(id)))
                .run(&ctx)
                .await
                .expect("a list route answers an empty list, not an error, when denied")
                .len()
        }
    };
    for role in ["owner", "admin"] {
        assert_eq!(visible(human(role)).await, 1, "{role} lists keys");
    }
    assert_eq!(
        visible(sys()).await,
        1,
        "system reads keys (VS-3's resolver)"
    );
    for role in ["developer", "operator", "support", "auditor"] {
        assert_eq!(visible(human(role)).await, 0, "{role} must not list keys");
    }
    assert_eq!(visible(principal("app", PrincipalKind::App)).await, 0);
}

/// A client holding `console:pair` can attach a console key to any linked
/// person, so a key for it is a key that can speak as that person. Over HTTP
/// even an owner cannot register one; the operator's command line can.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_client_holding_the_pair_scope_takes_keys_only_from_the_operator() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let client = seed_client_scoped(&db, " sms:send console:pair ").await;
    let public = b64(&fresh_public_key());

    for role in ["owner", "admin"] {
        let error = register(&db, &human(role), &client.id, &public, None)
            .await
            .expect_err("a signed call must not register a key for a pairing client");
        assert!(
            matches!(error, CratestackError::Forbidden(_)),
            "{role}: {error:?}"
        );
    }

    // The same client, registered by the operator's command.
    let args = register_app_client_key::Args {
        args: schema::RegisterClientKeyInput {
            appClientId: client.id.clone(),
            publicKey: public.clone(),
            label: None,
        },
    };
    let ctx = human("owner");
    let key = register_app_client_key::invoke_with_db(&db, &args, &ctx, |db, _authorized| {
        let args = args.clone();
        async move {
            sms_api::client_keys::register(
                &db,
                &sys(),
                args.args,
                sms_api::client_keys::Registrant::Operator,
            )
            .await
        }
    })
    .await
    .expect("the operator registers it");
    assert_eq!(key.appClientId, client.id);
}
