//! `registerConsoleKey` / `revokeConsoleKey` (VS-5a) against a real, fully
//! migrated Postgres - through `invoke_with_db`, the entry point the HTTP
//! route uses, so each procedure's `@allow` and `@authorize` are the real
//! ones, plus the generated delegate under several roles to pin
//! `ConsoleKey`'s own policy.
//!
//! Every test that says "refused" also asserts that nothing was written: a
//! pairing that fails halfway must not leave a `User` or a `FederatedLink`
//! behind. Each guard below was run against a loosened copy of the thing it
//! guards and failed (the PR description and AGENTS.md carry the captured
//! output), so a green run means the guard is what these assertions say it is.
//!
//! ```bash
//! cargo test -p sms-api --test console_keys_live_postgres -- --ignored
//! ```

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, CratestackError, FilterExpr, Value};
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use sha2::{Digest, Sha256};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::console_keys::{FEDERATION_ISSUER, PAIR_SCOPE, possession_message};
use sms_api::schema::{
    self, Cratestack, console_key, federated_link, procedures::ProcedureRegistry,
    procedures::register_console_key, procedures::revoke_console_key, user,
};
use sms_api::{HashPepper, Procedures};

/// #102: one live test at a time per binary - see
/// `rotate_webhook_secret_live_postgres.rs`'s identical mutex.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn principal(role: &str, kind: PrincipalKind, sub: &str) -> CratestackContext {
    Principal {
        sub: sub.to_owned(),
        kind,
        role: role.to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn human(role: &str) -> CratestackContext {
    principal(
        role,
        PrincipalKind::User,
        &format!("console-keys-test-{role}"),
    )
}

fn sys() -> CratestackContext {
    principal("system", PrincipalKind::App, "console-keys-test-system")
}

/// An app principal as `GatewayAuth` builds one, with the token's `scope`
/// claim in the `scope` extension (`None`: no such claim at all).
fn app(scope: Option<&str>) -> CratestackContext {
    let mut ctx = principal("app", PrincipalKind::App, "appc_vaam");
    if let Some(scope) = scope {
        ctx.extensions
            .insert("scope".to_owned(), Value::String(scope.to_owned()));
    }
    ctx
}

fn pairing_app() -> CratestackContext {
    app(Some(&format!(" sms:send {PAIR_SCOPE} ")))
}

fn procedures() -> Procedures {
    Procedures::new(
        HashPepper::new("console-keys-live-postgres-test-pepper-well-over-the-minimum")
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

fn vaam_user_id() -> String {
    format!("usr_{}", unique_suffix())
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

/// A distinct P-256 key per call and per run. The scalar is hashed from a
/// counter, the process id and the clock; a hash that is not a valid scalar
/// (probability about 2^-32) is simply hashed again.
fn fresh_key() -> SigningKey {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_nanos();
    let mut seed: [u8; 32] = Sha256::digest(format!(
        "console-keys-test-{n}-{}-{nanos}",
        std::process::id()
    ))
    .into();
    loop {
        if let Ok(key) = SigningKey::from_slice(&seed) {
            return key;
        }
        seed = Sha256::digest(seed).into();
    }
}

fn public_key_text(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_sec1_point(false).as_bytes())
}

fn thumbprint_of(key: &SigningKey) -> String {
    sms_api::console_keys::parse_public_key(&public_key_text(key))
        .expect("a P-256 point")
        .thumbprint
}

/// A pairing request the way a console and vaam would build it: signed by
/// `key` over the documented message.
fn pairing(
    vaam_user_id: &str,
    key: &SigningKey,
    issued_at: DateTime<Utc>,
) -> schema::RegisterConsoleKeyInput {
    let public_key = public_key_text(key);
    // Whole seconds, as a QR carries them.
    let issued_at = DateTime::from_timestamp(issued_at.timestamp(), 0).expect("a valid time");
    let signature: Signature = key.sign(&possession_message(&public_key, issued_at.timestamp()));
    schema::RegisterConsoleKeyInput {
        vaamUserId: vaam_user_id.to_owned(),
        displayName: Some("Ada Admin".to_owned()),
        publicKey: public_key,
        possessionProof: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        issuedAt: issued_at,
    }
}

async fn register(
    db: &Cratestack,
    ctx: &CratestackContext,
    input: schema::RegisterConsoleKeyInput,
) -> Result<schema::ConsoleKey, CratestackError> {
    let procedures = procedures();
    let args = register_console_key::Args { args: input };
    register_console_key::invoke_with_db(db, &args, ctx, |db, authorized| {
        let (p, c, a) = (procedures.clone(), ctx.clone(), args.clone());
        async move { p.register_console_key(&db, &c, a, authorized).await }
    })
    .await
}

async fn revoke(
    db: &Cratestack,
    ctx: &CratestackContext,
    key_id: &str,
) -> Result<schema::ConsoleKey, CratestackError> {
    let procedures = procedures();
    let args = revoke_console_key::Args {
        args: schema::RevokeConsoleKeyInput {
            keyId: key_id.to_owned(),
        },
    };
    revoke_console_key::invoke_with_db(db, &args, ctx, |db, authorized| {
        let (p, c, a) = (procedures.clone(), ctx.clone(), args.clone());
        async move { p.revoke_console_key(&db, &c, a, authorized).await }
    })
    .await
}

async fn keys_for_thumbprint(db: &Cratestack, thumbprint: &str) -> Vec<schema::ConsoleKey> {
    db.console_key()
        .find_many()
        .where_expr(FilterExpr::from(
            console_key::thumbprint().eq(thumbprint.to_owned()),
        ))
        .run(&sys())
        .await
        .expect("reading console keys back as system")
}

async fn links_for(db: &Cratestack, vaam_user_id: &str) -> Vec<schema::FederatedLink> {
    db.federated_link()
        .find_many()
        .where_expr(
            FilterExpr::from(federated_link::issuer().eq(FEDERATION_ISSUER.to_owned()))
                .and(federated_link::subject().eq(vaam_user_id.to_owned())),
        )
        .run(&sys())
        .await
        .expect("reading links back as system")
}

async fn users_with_email(db: &Cratestack, email: &str) -> Vec<schema::User> {
    db.user()
        .find_many()
        .where_expr(FilterExpr::from(user::email().eq(email.to_owned())))
        .run(&sys())
        .await
        .expect("reading users back as system")
}

fn synthetic_email(vaam_user_id: &str) -> String {
    format!("{vaam_user_id}@vaam.invalid")
}

/// Nothing at all exists for this person or this key.
async fn assert_nothing_written(db: &Cratestack, vaam_user_id: &str, thumbprint: &str) {
    assert_eq!(links_for(db, vaam_user_id).await.len(), 0, "no link");
    assert_eq!(
        users_with_email(db, &synthetic_email(vaam_user_id))
            .await
            .len(),
        0,
        "no user"
    );
    assert_eq!(keys_for_thumbprint(db, thumbprint).await.len(), 0, "no key");
}

/// An existing active user and its vaam link, as an earlier pairing (or the
/// SSO broker) leaves them, in `role_key`.
async fn seed_linked_user(db: &Cratestack, vaam_user_id: &str, role_key: &str) -> schema::User {
    let created = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("pending-{}", unique_suffix()),
            email: synthetic_email(vaam_user_id),
            displayName: "Seeded Person".to_owned(),
            roleKey: role_key.to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&human("owner"))
        .await
        .expect("seeding the user");
    db.federated_link()
        .create(schema::CreateFederatedLinkInput {
            issuer: FEDERATION_ISSUER.to_owned(),
            subject: vaam_user_id.to_owned(),
            userId: created.id.clone(),
        })
        .run(&sys())
        .await
        .expect("seeding the link");
    created
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn pairing_creates_an_admin_a_vaam_link_and_a_thirty_day_key() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let vaam = vaam_user_id();
    let key = fresh_key();
    let before = Utc::now();

    let paired = register(&db, &pairing_app(), pairing(&vaam, &key, Utc::now()))
        .await
        .expect("vaam's service client pairs a console");

    assert_eq!(paired.alg, schema::ConsoleKeyAlg::esp256);
    assert_eq!(
        paired.publicKey,
        key.verifying_key().to_sec1_point(false).as_bytes().to_vec(),
        "the stored key is the SEC1 uncompressed point"
    );
    assert_eq!(paired.thumbprint, thumbprint_of(&key));
    assert_eq!(paired.label, None);
    assert!(paired.revokedAt.is_none());
    assert!(paired.pairedAt >= before && paired.pairedAt <= Utc::now());
    assert_eq!(
        paired.expiresAt - paired.pairedAt,
        Duration::days(30),
        "R3: a console key lasts 30 days"
    );

    let links = links_for(&db, &vaam).await;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].userId, paired.userId);
    let users = users_with_email(&db, &synthetic_email(&vaam)).await;
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].id, paired.userId);
    assert_eq!(users[0].subject, users[0].id, "subject is the row's own id");
    assert_eq!(users[0].displayName, "Ada Admin");
    assert!(users[0].active);
}

/// R2: a paired console user is `admin`, never `owner`. Two guards hold it
/// and this test breaks neither's presence silently: the role written, and
/// `User.create`'s own policy, which refuses an `owner` from the very context
/// the pairing writes the user under.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_paired_user_is_an_admin_and_the_schema_would_refuse_an_owner() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let vaam = vaam_user_id();

    let paired = register(
        &db,
        &pairing_app(),
        pairing(&vaam, &fresh_key(), Utc::now()),
    )
    .await
    .expect("pairing");
    let created = users_with_email(&db, &synthetic_email(&vaam)).await;
    assert_eq!(created[0].roleKey, "admin", "R2: admin");
    assert_ne!(created[0].roleKey, "owner");
    assert_eq!(created[0].id, paired.userId);

    let admin_creating_an_owner = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("pending-{}", unique_suffix()),
            email: synthetic_email(&vaam_user_id()),
            displayName: "Would-be owner".to_owned(),
            roleKey: "owner".to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&principal(
            "admin",
            PrincipalKind::User,
            "sms-api:console-pairing",
        ))
        .await
        .expect_err("User.create refuses an owner from an admin context");
    assert!(
        matches!(admin_creating_an_owner, CratestackError::Forbidden(_)),
        "{admin_creating_an_owner:?}"
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn pairing_the_same_key_twice_returns_the_same_row() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let vaam = vaam_user_id();
    let key = fresh_key();

    let first = register(&db, &pairing_app(), pairing(&vaam, &key, Utc::now()))
        .await
        .expect("first pairing");
    let second = register(&db, &pairing_app(), pairing(&vaam, &key, Utc::now()))
        .await
        .expect("a repeat is not an error");

    assert_eq!(first.id, second.id);
    assert_eq!(first.expiresAt, second.expiresAt, "30 days are not renewed");
    assert_eq!(keys_for_thumbprint(&db, &first.thumbprint).await.len(), 1);
    assert_eq!(links_for(&db, &vaam).await.len(), 1);
    assert_eq!(
        users_with_email(&db, &synthetic_email(&vaam)).await.len(),
        1
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_second_key_of_the_same_person_joins_the_same_user() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let vaam = vaam_user_id();

    let laptop = register(
        &db,
        &pairing_app(),
        pairing(&vaam, &fresh_key(), Utc::now()),
    )
    .await
    .expect("first browser");
    let desktop = register(
        &db,
        &pairing_app(),
        pairing(&vaam, &fresh_key(), Utc::now()),
    )
    .await
    .expect("second browser");

    assert_ne!(laptop.id, desktop.id);
    assert_eq!(laptop.userId, desktop.userId);
    assert_eq!(
        users_with_email(&db, &synthetic_email(&vaam)).await.len(),
        1
    );
    assert_eq!(links_for(&db, &vaam).await.len(), 1);
}

/// Pairing never touches a role, in either direction: an owner who is
/// already linked stays an owner, a support user stays support.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_linked_person_keeps_the_role_they_have() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    for role in ["support", "owner"] {
        let vaam = vaam_user_id();
        let seeded = seed_linked_user(&db, &vaam, role).await;

        let paired = register(
            &db,
            &pairing_app(),
            pairing(&vaam, &fresh_key(), Utc::now()),
        )
        .await
        .expect("pairing a linked person");

        assert_eq!(
            paired.userId, seeded.id,
            "{role}: the linked user, not a new one"
        );
        let after = users_with_email(&db, &synthetic_email(&vaam)).await;
        assert_eq!(after.len(), 1, "{role}");
        assert_eq!(after[0].roleKey, role, "{role}: the role is untouched");
        assert_eq!(
            after[0].displayName, "Seeded Person",
            "{role}: nor the name"
        );
    }
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_deactivated_person_cannot_pair_and_nothing_is_written() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let vaam = vaam_user_id();
    let seeded = seed_linked_user(&db, &vaam, "admin").await;
    db.user()
        .update(seeded.id.clone())
        .set(schema::UpdateUserInput {
            active: Some(false),
            ..Default::default()
        })
        .if_match(seeded.version)
        .run(&human("owner"))
        .await
        .expect("deactivating the user");
    let key = fresh_key();

    let error = register(&db, &pairing_app(), pairing(&vaam, &key, Utc::now()))
        .await
        .expect_err("a deactivated account");
    assert!(matches!(error, CratestackError::Forbidden(_)), "{error:?}");
    assert_eq!(
        keys_for_thumbprint(&db, &thumbprint_of(&key)).await.len(),
        0
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn concurrent_first_pairings_make_one_user_and_one_row_per_key() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = pairing_app();

    // Two different keys of one new person, at once.
    let vaam = vaam_user_id();
    let (one, two) = (fresh_key(), fresh_key());
    let (a, b) = tokio::join!(
        register(&db, &app, pairing(&vaam, &one, Utc::now())),
        register(&db, &app, pairing(&vaam, &two, Utc::now())),
    );
    let (a, b) = (a.expect("first racer"), b.expect("second racer"));
    assert_eq!(a.userId, b.userId, "one person, one user");
    assert_eq!(
        users_with_email(&db, &synthetic_email(&vaam)).await.len(),
        1
    );
    assert_eq!(links_for(&db, &vaam).await.len(), 1);

    // One key of one new person, twice at once.
    let vaam = vaam_user_id();
    let same = fresh_key();
    let (a, b) = tokio::join!(
        register(&db, &app, pairing(&vaam, &same, Utc::now())),
        register(&db, &app, pairing(&vaam, &same, Utc::now())),
    );
    let (a, b) = (a.expect("first racer"), b.expect("second racer"));
    assert_eq!(a.id, b.id);
    assert_eq!(keys_for_thumbprint(&db, &a.thumbprint).await.len(), 1);
    assert_eq!(
        users_with_email(&db, &synthetic_email(&vaam)).await.len(),
        1
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_key_cannot_move_to_another_person() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let (ada, grace) = (vaam_user_id(), vaam_user_id());
    let key = fresh_key();
    let paired = register(&db, &pairing_app(), pairing(&ada, &key, Utc::now()))
        .await
        .expect("pairing Ada");

    let error = register(&db, &pairing_app(), pairing(&grace, &key, Utc::now()))
        .await
        .expect_err("Grace presenting Ada's key");
    assert!(matches!(error, CratestackError::Conflict(_)), "{error:?}");
    assert_eq!(
        keys_for_thumbprint(&db, &paired.thumbprint).await[0].userId,
        paired.userId
    );
    assert_eq!(
        links_for(&db, &grace).await.len(),
        0,
        "Grace was not linked"
    );
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_revoked_or_expired_key_is_never_registered_again() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let vaam = vaam_user_id();
    let revoked_key = fresh_key();
    let paired = register(
        &db,
        &pairing_app(),
        pairing(&vaam, &revoked_key, Utc::now()),
    )
    .await
    .expect("pairing");
    revoke(&db, &human("owner"), &paired.id)
        .await
        .expect("revoking");
    let error = register(
        &db,
        &pairing_app(),
        pairing(&vaam, &revoked_key, Utc::now()),
    )
    .await
    .expect_err("a revoked key is never reinstated");
    assert!(matches!(error, CratestackError::Conflict(_)), "{error:?}");
    assert!(
        keys_for_thumbprint(&db, &paired.thumbprint).await[0]
            .revokedAt
            .is_some()
    );

    // An expired key: written the way only `system` can, 31 days ago.
    let expired_key = fresh_key();
    let old = Utc::now() - Duration::days(31);
    let planted = db
        .console_key()
        .create(schema::CreateConsoleKeyInput {
            userId: paired.userId.clone(),
            alg: schema::ConsoleKeyAlg::esp256,
            publicKey: key_bytes(&expired_key),
            thumbprint: thumbprint_of(&expired_key),
            label: None,
            pairedAt: old,
            expiresAt: old + Duration::days(30),
            revokedAt: None,
        })
        .run(&sys())
        .await
        .expect("planting an expired key");
    let error = register(
        &db,
        &pairing_app(),
        pairing(&vaam, &expired_key, Utc::now()),
    )
    .await
    .expect_err("an expired key is never extended");
    assert!(matches!(error, CratestackError::Conflict(_)), "{error:?}");
    let after = &keys_for_thumbprint(&db, &planted.thumbprint).await[0];
    assert_eq!(
        after.expiresAt, planted.expiresAt,
        "the lifetime was not extended"
    );
}

fn key_bytes(key: &SigningKey) -> Vec<u8> {
    key.verifying_key().to_sec1_point(false).as_bytes().to_vec()
}

type Mutate = fn(&mut schema::RegisterConsoleKeyInput, &SigningKey);

/// Each way a possession proof can be wrong, with the reason the procedure
/// gives, and the proof that none of them leaves anything behind.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_bad_proof_is_refused_and_writes_nothing() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let now = Utc::now();

    let cases: [(&str, &str, Mutate); 6] = [
        ("signed by another key", "does not verify", |args, _| {
            let impostor = SigningKey::from_slice(&[0x42; 32]).expect("a scalar");
            let signature: Signature = impostor.sign(&possession_message(
                &args.publicKey,
                args.issuedAt.timestamp(),
            ));
            args.possessionProof = URL_SAFE_NO_PAD.encode(signature.to_bytes());
        }),
        (
            "issuedAt moved after signing",
            "does not verify",
            |args, _| {
                args.issuedAt += Duration::seconds(1);
            },
        ),
        (
            "another domain's message",
            "does not verify",
            |args, key| {
                let other = format!(
                    "vaam.admin-pair.v1\n{}\n{}\nlabel",
                    args.publicKey,
                    args.issuedAt.timestamp()
                );
                let signature: Signature = key.sign(other.as_bytes());
                args.possessionProof = URL_SAFE_NO_PAD.encode(signature.to_bytes());
            },
        ),
        (
            "a stale request, correctly signed",
            "expired",
            |args, key| {
                args.issuedAt -= Duration::seconds(301);
                let signature: Signature = key.sign(&possession_message(
                    &args.publicKey,
                    args.issuedAt.timestamp(),
                ));
                args.possessionProof = URL_SAFE_NO_PAD.encode(signature.to_bytes());
            },
        ),
        ("a truncated signature", "64-byte", |args, _| {
            args.possessionProof = URL_SAFE_NO_PAD.encode([7_u8; 63]);
        }),
        ("an Ed25519-sized key", "bytes", |args, _| {
            args.publicKey = URL_SAFE_NO_PAD.encode([9_u8; 32]);
        }),
    ];
    for (name, reason, mutate) in cases {
        let vaam = vaam_user_id();
        let key = fresh_key();
        let mut args = pairing(&vaam, &key, now);
        mutate(&mut args, &key);
        let error = register(&db, &pairing_app(), args).await.expect_err(name);
        match &error {
            CratestackError::Validation(message) => {
                assert!(message.contains(reason), "{name}: {message}");
            }
            other => panic!("{name}: expected a validation refusal, got {other:?}"),
        }
        assert_nothing_written(&db, &vaam, &thumbprint_of(&key)).await;
    }
}

/// `registerConsoleKey` is for one caller: an app that holds `console:pair`.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn only_an_app_with_the_pair_scope_may_register() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let denied = [
        ("an app with no scope claim", app(None)),
        ("an app holding only sms:send", app(Some(" sms:send "))),
        ("a look-alike scope", app(Some(" console:pair2 "))),
        ("a prefix of the scope", app(Some(" console:pai "))),
        ("the internal system context", sys()),
        ("a human owner", human("owner")),
        ("a human admin", human("admin")),
        ("a human operator", human("operator")),
    ];
    for (name, ctx) in denied {
        let vaam = vaam_user_id();
        let key = fresh_key();
        let error = register(&db, &ctx, pairing(&vaam, &key, Utc::now()))
            .await
            .expect_err(name);
        assert!(
            matches!(error, CratestackError::Forbidden(_)),
            "{name}: {error:?}"
        );
        assert_nothing_written(&db, &vaam, &thumbprint_of(&key)).await;
    }

    // A human's `perms` never stand in for the scope.
    let mut with_perms = human("admin");
    with_perms.extensions.insert(
        "perms".to_owned(),
        Value::List(vec![Value::String(PAIR_SCOPE.to_owned())]),
    );
    let error = register(
        &db,
        &with_perms,
        pairing(&vaam_user_id(), &fresh_key(), Utc::now()),
    )
    .await
    .expect_err("perms are not a scope");
    assert!(matches!(error, CratestackError::Forbidden(_)), "{error:?}");

    register(
        &db,
        &pairing_app(),
        pairing(&vaam_user_id(), &fresh_key(), Utc::now()),
    )
    .await
    .expect("the scope alone, among others, is enough");
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn owner_admin_and_the_keys_own_user_may_revoke_nobody_else() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let vaam = vaam_user_id();
    let paired = register(
        &db,
        &pairing_app(),
        pairing(&vaam, &fresh_key(), Utc::now()),
    )
    .await
    .expect("pairing");
    let other = register(
        &db,
        &pairing_app(),
        pairing(&vaam_user_id(), &fresh_key(), Utc::now()),
    )
    .await
    .expect("pairing someone else");

    // Not allowed: any other human, an app, the system context.
    let own_to_other = principal("support", PrincipalKind::User, &other.userId);
    for (name, ctx) in [
        ("another user's own session", own_to_other),
        ("a support user", human("support")),
        ("a developer", human("developer")),
        ("an app holding the pair scope", pairing_app()),
        ("the system context", sys()),
    ] {
        let error = revoke(&db, &ctx, &paired.id).await.expect_err(name);
        assert!(
            matches!(
                error,
                CratestackError::Forbidden(_) | CratestackError::NotFound(_)
            ),
            "{name}: {error:?}"
        );
    }
    assert!(
        keys_for_thumbprint(&db, &paired.thumbprint).await[0]
            .revokedAt
            .is_none(),
        "no refused call revoked the key"
    );

    // Allowed: the key's own user (a plain support user, by `sub`).
    let own = principal("support", PrincipalKind::User, &paired.userId);
    let revoked = revoke(&db, &own, &paired.id)
        .await
        .expect("a user revokes their own key");
    let first_stamp = revoked.revokedAt.expect("revocation stamps revokedAt");
    assert!(first_stamp <= Utc::now());

    // Allowed: owner and admin, any key; revoking twice keeps the first stamp.
    for role in ["owner", "admin"] {
        let again = revoke(&db, &human(role), &paired.id).await.expect(role);
        assert_eq!(
            again.revokedAt,
            Some(first_stamp),
            "{role}: first revocation time kept"
        );
        let other_key = register(
            &db,
            &pairing_app(),
            pairing(&vaam_user_id(), &fresh_key(), Utc::now()),
        )
        .await
        .expect("another pairing");
        revoke(&db, &human(role), &other_key.id).await.expect(role);
    }
    assert!(
        keys_for_thumbprint(&db, &other.thumbprint).await[0]
            .revokedAt
            .is_none(),
        "the other person's key was never touched"
    );

    let error = revoke(&db, &human("owner"), "nosuchkey00000000000000")
        .await
        .expect_err("an unknown key");
    assert!(
        matches!(
            error,
            CratestackError::Forbidden(_) | CratestackError::NotFound(_)
        ),
        "{error:?}"
    );
}

/// `ConsoleKey`'s own policy through the generated delegate - what a request
/// to `POST /console_keys` / `PATCH /console_keys/{id}` would reach. Writes
/// are `hasRole('system')` only: a human `owner` is refused, which is what
/// keeps the proof, the point check and the thumbprint on the procedure.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn the_generated_routes_cannot_write_a_key_except_as_system() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let paired = register(
        &db,
        &pairing_app(),
        pairing(&vaam_user_id(), &fresh_key(), Utc::now()),
    )
    .await
    .expect("pairing");
    let planted = |n: u8| {
        let now = Utc::now();
        schema::CreateConsoleKeyInput {
            userId: paired.userId.clone(),
            alg: schema::ConsoleKeyAlg::esp256,
            publicKey: [vec![4_u8], vec![n; 64]].concat(),
            thumbprint: format!("{:0>64}", format!("{n:02x}{}", unique_suffix())),
            label: None,
            pairedAt: now,
            expiresAt: now + Duration::days(30),
            revokedAt: None,
        }
    };

    for (name, ctx) in [
        ("owner", human("owner")),
        ("admin", human("admin")),
        ("developer", human("developer")),
        (
            "the key's own user",
            principal("support", PrincipalKind::User, &paired.userId),
        ),
        ("an app", pairing_app()),
    ] {
        let error = db
            .console_key()
            .create(planted(1))
            .run(&ctx)
            .await
            .expect_err("planting a key through the generated route");
        assert!(
            matches!(error, CratestackError::Forbidden(_)),
            "{name}: {error:?}"
        );
    }
    let created = db
        .console_key()
        .create(planted(3))
        .run(&sys())
        .await
        .expect("system is the one writer the policy admits");

    for (name, ctx) in [
        ("owner", human("owner")),
        ("admin", human("admin")),
        (
            "the key's own user",
            principal("support", PrincipalKind::User, &paired.userId),
        ),
    ] {
        let error = db
            .console_key()
            .update(created.id.clone())
            .set(schema::UpdateConsoleKeyInput {
                revokedAt: Some(Some(Utc::now())),
                expiresAt: Some(Utc::now() + Duration::days(3650)),
                ..Default::default()
            })
            .run(&ctx)
            .await
            .expect_err("stamping or extending through the generated route");
        assert!(
            matches!(
                error,
                CratestackError::Forbidden(_) | CratestackError::NotFound(_)
            ),
            "{name}: {error:?}"
        );
        let error = db
            .console_key()
            .delete(created.id.clone())
            .run(&ctx)
            .await
            .expect_err("there is no delete action");
        assert!(
            matches!(
                error,
                CratestackError::Forbidden(_) | CratestackError::NotFound(_)
            ),
            "{name}: {error:?}"
        );
    }
    let still = keys_for_thumbprint(&db, &created.thumbprint).await;
    assert_eq!(still.len(), 1, "no human role deleted the row");
    assert!(still[0].revokedAt.is_none(), "no human role revoked it");
    assert_eq!(
        still[0].expiresAt, created.expiresAt,
        "no human role extended it"
    );
}

/// Reads: owner, admin and system list every key; a user lists their own and
/// nobody else's; every other principal sees an empty list, not an error.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn owner_admin_system_and_the_keys_own_user_can_read_it() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let paired = register(
        &db,
        &pairing_app(),
        pairing(&vaam_user_id(), &fresh_key(), Utc::now()),
    )
    .await
    .expect("pairing");

    let visible = |ctx: CratestackContext| {
        let (db, id) = (&db, paired.id.clone());
        async move {
            db.console_key()
                .find_many()
                .where_expr(FilterExpr::from(console_key::id().eq(id)))
                .run(&ctx)
                .await
                .expect("a list route answers an empty list, not an error, when denied")
                .len()
        }
    };
    for (name, ctx) in [
        ("owner", human("owner")),
        ("admin", human("admin")),
        ("system", sys()),
        (
            "the key's own user",
            principal("support", PrincipalKind::User, &paired.userId),
        ),
    ] {
        assert_eq!(visible(ctx).await, 1, "{name} reads the key");
    }
    for (name, ctx) in [
        (
            "another support user",
            principal("support", PrincipalKind::User, "someone-else"),
        ),
        ("a developer", human("developer")),
        ("an auditor", human("auditor")),
        ("an app", pairing_app()),
        // An app whose `sub` happens to equal the user id is still an app.
        (
            "an app with the user's id as its sub",
            principal("app", PrincipalKind::App, &paired.userId),
        ),
    ] {
        assert_eq!(visible(ctx).await, 0, "{name} must not read the key");
    }
}

/// The database refuses a row written around the procedure: the CHECKs in
/// `0007_console_keys` are the backstop the emitter cannot write.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn the_database_refuses_a_malformed_key_row() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let paired = register(
        &db,
        &pairing_app(),
        pairing(&vaam_user_id(), &fresh_key(), Utc::now()),
    )
    .await
    .expect("pairing");
    let now = Utc::now();
    let valid = |n: u8| schema::CreateConsoleKeyInput {
        userId: paired.userId.clone(),
        alg: schema::ConsoleKeyAlg::esp256,
        publicKey: [vec![4_u8], vec![n; 64]].concat(),
        thumbprint: format!("{:0>64}", format!("{n:02x}{}", unique_suffix())),
        label: None,
        pairedAt: now,
        expiresAt: now + Duration::days(30),
        revokedAt: None,
    };
    db.console_key()
        .create(valid(5))
        .run(&sys())
        .await
        .expect("the control row is accepted");

    let mut compressed_tag = valid(6);
    compressed_tag.publicKey[0] = 0x02;
    let mut uppercase_thumbprint = valid(7);
    uppercase_thumbprint.thumbprint = uppercase_thumbprint.thumbprint.to_uppercase();
    let mut backwards = valid(8);
    backwards.expiresAt = backwards.pairedAt;
    let mut orphan = valid(9);
    orphan.userId = "nosuchuser0000000000000".to_owned();
    for (name, bad) in [
        (
            "a first byte that is not the uncompressed tag",
            compressed_tag,
        ),
        (
            "a thumbprint that is not lowercase hex",
            uppercase_thumbprint,
        ),
        ("a key that expires when it is paired", backwards),
        ("a user that does not exist", orphan),
    ] {
        db.console_key()
            .create(bad)
            .run(&sys())
            .await
            .expect_err(name);
    }
    let mut short = valid(10);
    short.publicKey.truncate(33);
    // 33 bytes is also refused by the generated input's `@length`, before it
    // reaches the database; either way the row is not written.
    db.console_key()
        .create(short)
        .run(&sys())
        .await
        .expect_err("a short key");
}
