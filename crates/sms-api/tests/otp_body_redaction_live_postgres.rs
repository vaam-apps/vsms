//! #165 regression: `docs/architecture.md:584` documents, as an
//! implemented decision, that `class = otp` messages never retain their
//! plaintext `body` — "an OTP gateway that stores OTP plaintext for 90
//! days is a credential database." The send procedure
//! (`crates/sms-api/src/procedures.rs`) wrote `body: Some(body)`
//! unconditionally, with no `class` check anywhere near it, so every OTP
//! body sat in Postgres for the full `@@retain(days: 90)` window — and,
//! since `@sensitive` redacts audit snapshots only (verified, same file),
//! was returned in plaintext by `GET /messages/{id}` to any principal
//! that passes the `detail` policy.
//!
//! **The fix is not "null it at creation."** Traced live before writing
//! any fix: `sms-worker`'s `dispatch` role and this api process are
//! separate OS processes with no coordination channel except Postgres
//! (the stack table's own "no broker, no Redis"), so
//! `crates/sms-worker/src/dispatch.rs`'s `submit_one` learns a message's
//! body only by re-reading the same `messages.body` column
//! `crates/sms-worker/src/claim.rs`'s `candidates()` just re-selected —
//! not an in-memory value carried from the send path. Nulling `body` at
//! creation would fail every OTP message at dispatch with "body missing"
//! (see `dispatch.rs`'s own `let Some(body) = message.body.clone() else`
//! branch). A `submitted -> undelivered -> queued` retry (§7.4, #122)
//! needs the same body again on a later attempt, so redacting on first
//! submit breaks retry too.
//!
//! So the redaction lives in Postgres's own `messages_guard_transition()`
//! trigger (§2.10), the same one that already stamps `finalizedAt`/
//! `submittedAt` as a side effect of a transition: for `class = 'otp'`,
//! `body` is nulled the instant a row reaches a state nothing transitions
//! out of (`delivered`, `failed`, `expired`, `rejected`, `cancelled` —
//! "terminality is data, not code", per the transition table's own
//! comment). This suite proves, against a real, fully migrated Postgres,
//! not just the trigger's SQL:
//!
//! 1. an OTP message created through the real `sendMessage` procedure has
//!    a body, keeps it through every non-terminal hop dispatch would see
//!    it in, and loses it the moment it reaches `delivered`;
//! 2. an OTP message that reaches a *different* terminal state
//!    (`rejected`, taken directly from `accepted`) is redacted too — this
//!    is a state-machine invariant, not a `delivered`-only special case;
//! 3. a non-OTP message reaching the identical terminal transition keeps
//!    its body — the redaction is keyed by `class`, not a blanket
//!    "terminal states never keep a body" rule;
//! 4. `bodyHash`/`bodyLength`/`segments` all survive redaction — they
//!    carry no plaintext, and nothing about this fix should touch them.
//!
//! `sms_test_support` provisions Postgres and applies both migrations
//! automatically (a shared, self-healing container — see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-api --test otp_body_redaction_live_postgres -- --ignored
//! ```

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, message, procedures::send_message, procedures::ProcedureRegistry, Cratestack, Encoding,
    MessageClass, MessageState, OperatorCode, UpdateMessageInput,
};
use sms_api::{HashPepper, Procedures};

/// #102: this binary's own tests can race Postgres's `pg_type` catalog on
/// first use of a not-yet-cached query shape. Same fix as every other live
/// suite in this workspace — see `claim_live_postgres.rs`'s own doc.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn owner() -> CoolContext {
    Principal {
        sub: "otp-body-redaction-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> CoolContext {
    Principal {
        sub: "otp-body-redaction-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn app_caller(client_id: &str) -> CoolContext {
    let mut ctx = Principal {
        sub: client_id.to_owned(),
        kind: PrincipalKind::App,
        role: "developer".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions
        .insert("scope".to_owned(), Value::String("sms:send".to_owned()));
    ctx
}

/// Same construction as every other live suite's `unique_suffix` — a
/// counter folded with wall-clock nanoseconds, so it stays unique both
/// within one process and across `cargo test` runs against the same
/// never-reset database.
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

fn unique_mtn_msisdn() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .subsec_nanos();
    let unique = (u64::from(nanos) + n) % 1_000_000;
    format!("+237677{unique:06}")
}

fn test_pepper() -> HashPepper {
    HashPepper::new("otp-body-redaction-live-postgres-test-pepper-well-over-the-minimum")
        .expect("test pepper meets HashPepper::new's minimum length")
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

/// A fresh app + one active `sms:send`-scoped client. Mirrors
/// `send_message_live_postgres.rs`'s own helper of the same name.
async fn seed_app_and_client(db: &Cratestack) -> (String, schema::App) {
    let suffix = unique_suffix();
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "otp body redaction test app".to_owned(),
            slug: format!("otp-body-redaction-{}", suffix.to_lowercase()),
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

    let client_id = format!("client-{suffix}");
    db.app_client()
        .create(schema::CreateAppClientInput {
            appId: app.id.clone(),
            clientId: client_id.clone(),
            label: "test client".to_owned(),
            scopes: " sms:send ".to_owned(),
            lastUsedAt: None,
            retiredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the app client");

    (client_id, app)
}

/// An active `SenderId` with one `approved` registration, same shape as
/// `send_message_live_postgres.rs`'s helper.
async fn seed_approved_sender(db: &Cratestack) -> String {
    let suffix = unique_suffix();
    let value = format!("T{}", &suffix[..suffix.len().min(9)]).to_uppercase();

    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!(
                "test_{}",
                suffix.to_lowercase().chars().take(20).collect::<String>()
            ),
            displayName: "Test Provider".to_owned(),
            kind: schema::ProviderKind::aggregator_http,
            config: "{}".to_owned(),
            credentialRef: "vault://test".to_owned(),
            maxTps: 5.0,
            maxDailySubmissions: 1000,
            supportsDlr: true,
            supportsAlphaSender: true,
            supportsUcs2: true,
            supportsConcat: true,
            costPerSegmentXaf: "15".parse().unwrap(),
            healthCheckedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding a provider");

    let sender = db
        .sender_id()
        .create(schema::CreateSenderIdInput {
            value: value.clone(),
            kind: "alphanumeric".to_owned(),
            notes: None,
        })
        .run(&owner())
        .await
        .expect("seeding a sender id");

    db.sender_id_registration()
        .create(schema::CreateSenderIdRegistrationInput {
            senderIdId: sender.id.clone(),
            providerId: provider.id,
            status: "approved".to_owned(),
            submittedAt: Some(Utc::now()),
            approvedAt: Some(Utc::now()),
            reference: None,
            rejectionReason: None,
        })
        .run(&owner())
        .await
        .expect("seeding an approved registration");

    db.sender_id()
        .update(sender.id)
        .set(schema::UpdateSenderIdInput {
            active: Some(true),
            ..Default::default()
        })
        .run(&owner())
        .await
        .expect("activating the sender id");

    value
}

/// A same-shape `state`-only transition — covers every hop below except
/// `-> submitted` (needs `providerMessageRef`) and `-> rejected` (needs
/// `stateReason`). Panics with the attempted edge on failure, which is
/// all any caller here needs: every edge exercised is meant to be legal.
async fn transition(db: &Cratestack, msg: &schema::Message, next: MessageState) -> schema::Message {
    db.message()
        .update(msg.id.clone())
        .set(UpdateMessageInput {
            state: Some(next),
            ..Default::default()
        })
        .if_match(msg.version)
        .run(&sys())
        .await
        .unwrap_or_else(|e| panic!("{:?} -> {next:?} failed: {e}", msg.state))
}

/// `routed -> submitted`, stamping a `providerMessageRef` the same way
/// `dispatch::write_submitted` does — required by the `NOT NULL`-adjacent
/// expectation every DLR-correlation query makes, even though nothing
/// here reads it back.
async fn submit(db: &Cratestack, msg: &schema::Message) -> schema::Message {
    db.message()
        .update(msg.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::submitted),
            providerMessageRef: Some(Some(format!("otp-body-redaction-ref-{}", unique_suffix()))),
            ..Default::default()
        })
        .if_match(msg.version)
        .run(&sys())
        .await
        .expect("routed -> submitted")
}

async fn reload(db: &Cratestack, id: &str) -> schema::Message {
    db.message()
        .find_many()
        .where_expr(FilterExpr::from(message::id().eq(id.to_owned())))
        .limit(1)
        .run(&sys())
        .await
        .expect("reloading the message")
        .into_iter()
        .next()
        .expect("the message still exists")
}

/// Directly `create`s a message in `accepted` (real created-message shape
/// `sendMessage` itself would produce, minus the procedure's own
/// validation), for the two tests that need a `class` other than `otp`
/// or a terminal state other than `delivered` — neither of which the
/// real `sendMessage` + full dispatch path can reach without a live
/// provider, which is out of scope here (covered by
/// `crates/sms-worker/tests/dispatch_live_postgres.rs`'s wiremock-backed
/// suite instead).
async fn seed_message(db: &Cratestack, app_id: &str, class: MessageClass) -> schema::Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("otp-body-redaction-{}", unique_suffix())),
            msisdn: unique_mtn_msisdn(),
            msisdnHash: format!("hmac-sha256-v1:otp-body-redaction-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class,
            priority: 1000,
            body: Some("Votre code est 482013".to_owned()),
            bodyHash: "hmac-sha256-v1:otp-body-redaction-test".to_owned(),
            bodyLength: 22,
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

/// Test 1: the real `sendMessage` procedure, through to `delivered`.
///
/// This is the acceptance criterion's own scenario: send an OTP message
/// through the actual API path (not a direct `db.message().create()`
/// shortcut), confirm the row it produced still has a body (dispatch
/// needs it), then drive the *real* legal transition chain
/// (`accepted -> queued -> routed -> submitted -> delivered`, one
/// `if_match` hop at a time, exactly as `claim.rs`/`dispatch.rs`/`dlr.rs`
/// each would) and prove the row's `body` is `NULL` once it lands,
/// without the hash/length/segments columns being touched.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn sending_an_otp_message_keeps_its_body_for_dispatch_then_redacts_it_on_delivery() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());
    let (client_id, _app) = seed_app_and_client(&db).await;
    let sender = seed_approved_sender(&db).await;

    let result = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            send_message::Args {
                args: schema::SendMessageInput {
                    to: unique_mtn_msisdn(),
                    body: "Votre code est 482013".to_owned(),
                    senderId: Some(sender),
                    class: Some(MessageClass::otp),
                    clientRef: None,
                    scheduledAt: None,
                    validityMinutes: None,
                },
            },
        )
        .await
        .expect("a well-formed otp send must be accepted");

    assert_eq!(result.state, MessageState::accepted);

    // Right after send: body must be present, or dispatch could never
    // submit this message at all.
    let created = reload(&db, &result.messageId).await;
    assert_eq!(created.class, MessageClass::otp);
    assert_eq!(
        created.body.as_deref(),
        Some("Votre code est 482013"),
        "body must survive send() — dispatch reads it back from this same column"
    );
    let body_hash = created.bodyHash.clone();
    let body_length = created.bodyLength;
    let segments = created.segments;

    // accepted -> queued -> routed -> submitted: exactly the hops
    // `claim.rs`/`dispatch.rs` drive. Body must still be there at every
    // one of these non-terminal states, or dispatch/retry would break.
    let queued = transition(&db, &created, MessageState::queued).await;
    assert!(queued.body.is_some(), "body redacted before queued");
    let routed = transition(&db, &queued, MessageState::routed).await;
    assert!(
        routed.body.is_some(),
        "body redacted before routed — dispatch::submit_one would fail this message"
    );
    let submitted = submit(&db, &routed).await;
    assert!(
        submitted.body.is_some(),
        "body redacted before a terminal state — a retryable DLR failure would have nothing to resend"
    );

    // submitted -> delivered: the terminal transition `dlr::ingest_one`
    // performs on a successful DLR. This is the one that must redact.
    let delivered = transition(&db, &submitted, MessageState::delivered).await;

    assert_eq!(
        delivered.body, None,
        "otp body must be NULL once the message reaches a terminal state"
    );
    assert!(
        delivered.finalizedAt.is_some(),
        "finalizedAt must still be stamped — this fix must not disturb that existing invariant"
    );
    // bodyHash/bodyLength/segments carry no plaintext and must survive
    // redaction untouched — this is what keeps the row auditable.
    assert_eq!(delivered.bodyHash, body_hash);
    assert_eq!(delivered.bodyLength, body_length);
    assert_eq!(delivered.segments, segments);

    // Reload independently of the `UpdateMessageInput` return value,
    // proving the trigger's write actually persisted rather than only
    // ever being visible on the RETURNING row of the statement that set
    // it — the acceptance criterion's own "verified live, not asserted"
    // bar.
    let reloaded = reload(&db, &delivered.id).await;
    assert_eq!(reloaded.body, None);
    assert_eq!(reloaded.state, MessageState::delivered);
}

/// Test 2: redaction is a property of *any* terminal state, not a
/// `delivered`-only special case. `accepted -> rejected` (no active
/// provider, §7.4) is the shortest legal path to a terminal state that
/// never passes through `submitted` at all.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_otp_message_rejected_before_ever_being_submitted_still_loses_its_body() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let (_client_id, app) = seed_app_and_client(&db).await;
    let created = seed_message(&db, &app.id, MessageClass::otp).await;
    assert!(created.body.is_some());

    let rejected = db
        .message()
        .update(created.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::rejected),
            stateReason: Some(Some("no active provider".to_owned())),
            ..Default::default()
        })
        .if_match(created.version)
        .run(&sys())
        .await
        .expect("accepted -> rejected");

    assert_eq!(
        rejected.body, None,
        "otp body must be redacted on every terminal state, not only delivered"
    );

    let reloaded = reload(&db, &rejected.id).await;
    assert_eq!(reloaded.body, None);
}

/// Test 3: the redaction is keyed by `class`, not a blanket "terminal
/// states never keep a body" rule. A `transactional` message must keep
/// its body through the identical transition chain the first test just
/// proved redacts an `otp` one.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_non_otp_message_keeps_its_body_through_the_same_terminal_transition() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let (_client_id, app) = seed_app_and_client(&db).await;
    let created = seed_message(&db, &app.id, MessageClass::transactional).await;

    let queued = transition(&db, &created, MessageState::queued).await;
    let routed = transition(&db, &queued, MessageState::routed).await;
    let submitted = submit(&db, &routed).await;
    let delivered = transition(&db, &submitted, MessageState::delivered).await;

    assert!(
        delivered.body.is_some(),
        "a non-otp message must not have its body redacted on reaching a terminal state"
    );

    let reloaded = reload(&db, &delivered.id).await;
    assert!(reloaded.body.is_some());
}
