//! `map_database_error` / `is_illegal_transition` against a **real**
//! database — the coverage whose absence let vymalo/vsms#87 ship unnoticed
//! through PR #78.
//!
//! `crates/sms-api/src/errors.rs`'s own `#[cfg(test)] mod tests` construct
//! `CoolError::DatabaseTyped { .. }` by hand. That is correct unit coverage
//! of the *mapping function*, and it is completely blind to the step before
//! it: whether a driver error surviving the framework's sqlx→`CoolError`
//! conversion still carries its SQLSTATE at all. In `cratestack-sqlx`
//! `=0.5.0` it did not — every generated write mapped through
//! `CoolError::Database(error.to_string())`, discarding SQLSTATE and
//! constraint before any application code could see them, so
//! `db_sqlstate()` was `None` on every database-rejected write and an
//! illegal state transition surfaced as `500 DATABASE_ERROR` rather than
//! `409 Conflict`. Fixed upstream in `cratestack-sqlx` 0.6.0
//! (cratestack/cratestack#267), which routes all twelve write paths through
//! `cool_error_from_sqlx`.
//!
//! These tests go through a real delegate call so that conversion is in the
//! path. If the pin ever moves back to a version without the fix, or a
//! future release regresses it, these fail — `cargo build` and the
//! hand-constructed unit tests will not.
//!
//! `sms_test_support` provisions Postgres and applies both migrations
//! automatically (a shared, self-healing container — see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-api --test errors_live_postgres -- --ignored
//! ```

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::CoolContext;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::errors::{is_illegal_transition, map_database_error, SM001, UNIQUE_VIOLATION};
use sms_api::schema::{
    self, Cratestack, Encoding, MessageClass, MessageState, OperatorCode, UpdateMessageInput,
};

/// #102, found live: on a genuinely fresh database, this binary's own
/// tests — run concurrently by Rust's default multi-threaded test
/// harness — can race on Postgres's own `pg_type` catalog the first time
/// two of them prepare the exact same not-yet-cached query shape at the
/// same instant. See `crates/sms-worker/tests/claim_live_postgres.rs`'s
/// own `TEST_MUTEX` doc for the full reasoning — same mechanism, same
/// fix.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    Principal {
        sub: "errors-live-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "errors-live-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// Unique across runs as well as within one: this database is never reset
/// between `cargo test` invocations, so a bare counter collides with the
/// previous run's rows on any unique column.
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
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

async fn seed_app(db: &Cratestack) -> String {
    let suffix = unique_suffix();
    db.app()
        .create(schema::CreateAppInput {
            name: "errors live test app".to_owned(),
            slug: format!("errors-test-{}", suffix.to_lowercase()),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the app")
        .id
}

/// A message in `accepted` — the only state `create` can produce, since
/// `Message.state` carries `@default('accepted')`.
async fn seed_accepted_message(
    db: &Cratestack,
    app_id: &str,
    idempotency_key: Option<String>,
) -> schema::Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: idempotency_key,
            msisdn: "+237677123456".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:errors-test-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 1000,
            body: Some("errors live test".to_owned()),
            bodyHash: "hmac-sha256-v1:errors-test".to_owned(),
            bodyLength: 16,
            encoding: Encoding::gsm7,
            segments: 1,
            stateReason: None,
            routeId: None,
            providerId: None,
            providerMessageRef: None,
            providerMessageRefAlt: None,
            maxAttempts: 3,
            leaseOwner: None,
            leaseUntil: None,
            scheduledAt: None,
            expiresAt: Utc::now() + Duration::hours(1),
            submittedAt: None,
            finalizedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message")
}

/// The headline case: an illegal transition must reach the caller as 409,
/// not 500.
///
/// `accepted -> delivered` is absent from `message_state_transitions`, so
/// `messages_guard_transition` raises `SM001`. This is the exact assertion
/// #87 recorded as failing on `cratestack-sqlx =0.5.0`.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn an_illegal_transition_surfaces_as_409_not_500() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let message = seed_accepted_message(&db, &app_id, None).await;

    let error = db
        .message()
        .update(message.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::delivered),
            ..Default::default()
        })
        .if_match(message.version)
        .run(&sys())
        .await
        .expect_err("accepted -> delivered is not a legal edge; the trigger must reject it");

    // The step that was broken: the SQLSTATE has to survive the framework's
    // own sqlx -> CoolError conversion for anything downstream to work.
    assert_eq!(
        error.db_sqlstate(),
        Some(SM001),
        "SQLSTATE was discarded before application code — this is vymalo/vsms#87 \
         regressing (upstream cratestack/cratestack#267). Raw error: {error}"
    );

    assert!(
        is_illegal_transition(&error),
        "is_illegal_transition must recognise a real trigger rejection"
    );

    let mapped = map_database_error(error);
    assert_eq!(
        mapped.status_code(),
        409,
        "an illegal transition is a client error, not a gateway fault"
    );
    assert!(matches!(mapped, cratestack::CoolError::Conflict(_)));
}

/// The other half of `map_database_error`: `23505` must arrive typed, with
/// its constraint name intact, so dedupe can tell "already exists" from a
/// real fault. Dedupe is `create` + catch, because `upsert` does not exist
/// when the `@id` carries a default (§2.0).
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_duplicate_idempotency_key_surfaces_as_a_named_409() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let key = format!("errors-test-idem-{}", unique_suffix());

    seed_accepted_message(&db, &app_id, Some(key.clone())).await;
    let error = db
        .message()
        .create(schema::CreateMessageInput {
            appId: app_id.clone(),
            clientRef: None,
            idempotencyKey: Some(key),
            msisdn: "+237677123457".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:errors-test-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 1000,
            body: Some("errors live test dup".to_owned()),
            bodyHash: "hmac-sha256-v1:errors-test".to_owned(),
            bodyLength: 20,
            encoding: Encoding::gsm7,
            segments: 1,
            stateReason: None,
            routeId: None,
            providerId: None,
            providerMessageRef: None,
            providerMessageRefAlt: None,
            maxAttempts: 3,
            leaseOwner: None,
            leaseUntil: None,
            scheduledAt: None,
            expiresAt: Utc::now() + Duration::hours(1),
            submittedAt: None,
            finalizedAt: None,
        })
        .run(&sys())
        .await
        .expect_err("messages_app_idem_key is unique where idempotency_key is not null");

    assert_eq!(
        error.db_sqlstate(),
        Some(UNIQUE_VIOLATION),
        "SQLSTATE was discarded before application code — vymalo/vsms#87. Raw error: {error}"
    );
    assert!(
        !is_illegal_transition(&error),
        "a unique violation is not a state-machine rejection"
    );

    let constraint = error.db_constraint().map(ToOwned::to_owned);
    let mapped = map_database_error(error);
    assert_eq!(mapped.status_code(), 409);

    // The constraint name is what lets a caller tell *which* uniqueness it
    // tripped; map_database_error folds it into the message when present.
    if let Some(name) = constraint {
        match mapped {
            cratestack::CoolError::Conflict(message) => assert!(
                message.contains(&name),
                "the 409 should name the constraint it tripped; got {message:?}"
            ),
            other => panic!("expected Conflict, got {other:?}"),
        }
    }
}

/// A *legal* transition must not be touched by any of this — a guard
/// against a future "fix" that turns every write error into a 409.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_legal_transition_still_succeeds() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let message = seed_accepted_message(&db, &app_id, None).await;

    let queued = db
        .message()
        .update(message.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::queued),
            ..Default::default()
        })
        .if_match(message.version)
        .run(&sys())
        .await
        .expect("accepted -> queued is a legal edge");

    assert_eq!(queued.state, MessageState::queued);
}
