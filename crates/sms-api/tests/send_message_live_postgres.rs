//! `sendMessage` (#32) against a real, fully migrated Postgres — the nine
//! pre-persistence steps, exercised end to end through the actual
//! `ProcedureRegistry` trait method, not the crate-private helpers behind
//! it.
//!
//! Needs the full schema, including `0002_bootstrap`'s seeded
//! `operator_prefix_rules` (this suite relies on the real seed data —
//! `67x` → `mtn` — rather than inserting its own, since that data already
//! exists from milestone 0 and re-seeding it here would just be testing
//! that INSERT works, not that classification does).
//!
//! ```bash
//! docker run --rm -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:16
//! createdb vsms_check
//! DATABASE_URL=postgres://postgres:postgres@localhost/vsms_check ./ci/apply-migrations.sh
//! DATABASE_URL=postgres://postgres:postgres@localhost/vsms_check \
//!     cargo test -p sms-api --test send_message_live_postgres -- --ignored
//! ```

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, procedures::send_message, procedures::ProcedureRegistry, Cratestack, Encoding,
    MessageState, OperatorCode,
};
use sms_api::Procedures;

/// #102, found live: on a genuinely fresh database, this binary's own
/// tests — run concurrently by Rust's default multi-threaded test
/// harness — can race on Postgres's own `pg_type` catalog the first time
/// two of them prepare the exact same not-yet-cached query shape at the
/// same instant. See `crates/sms-worker/tests/claim_live_postgres.rs`'s
/// own `TEST_MUTEX` doc for the full reasoning — same mechanism, same
/// fix.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn owner() -> CoolContext {
    Principal {
        sub: "send-message-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> CoolContext {
    Principal {
        sub: "send-message-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// The context a machine caller's validated token would eventually
/// produce (#20/#21) — `kind == "app"`, `sub == clientId`. `role` doesn't
/// affect `sendMessage` itself; only `kind` and `sub` do.
///
/// #24: `GatewayAuth::authenticate` now also projects the token's `scope`
/// claim into `extensions`, which `sendMessage`'s own `require_permission(ctx,
/// "sms:send")` gate (Layer 2) checks before anything else in the
/// procedure runs. A hand-built context — this function never goes through
/// `GatewayAuth` — has to carry the same claim by hand, or every test below
/// would fail on that gate rather than on whatever it actually means to
/// exercise.
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

/// A counter folded together with wall-clock nanoseconds, not either alone:
/// the counter guards against two calls landing in the same nanosecond
/// within one process, and the nanoseconds guard against the counter
/// itself repeating across separate `cargo test` invocations — this
/// database is never reset between runs, so a bare per-process counter
/// collides with the previous run's rows (`apps_slug_key`,
/// `providers_key_key`, ...) the moment it restarts from zero.
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .subsec_nanos();
    // Zero-padded so every caller gets a fixed, predictable length
    // regardless of how large the counter has grown — `SenderId.value`'s
    // `@length(min: 3, ...)` in particular needs more than "a digit or two"
    // once contributed.
    format!("{:06x}", (u64::from(nanos).wrapping_add(n)) % 0x0100_0000)
}

/// A fresh MSISDN under the seeded `67x` (mtn) prefix, distinct on every
/// call — and, unlike `unique_suffix()`, distinct across separate test
/// *runs* too: this database is never reset between `cargo test`
/// invocations, so a test that persists a row keyed on the MSISDN itself
/// (`opt_outs.msisdn_hash`, unique) needs a number that has never been used
/// before, not just one that's unique within a single process. Wall-clock
/// nanoseconds supply that cross-run entropy; the counter only guards
/// against two calls landing in the same nanosecond within one process. A
/// decimal value, not `unique_suffix()`'s hex, because an MSISDN can't
/// contain `a`-`f`.
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

async fn db() -> Cratestack {
    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must point at a fully migrated database — see module docs");
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

/// A fresh app + one active client, with an optional default sender.
/// Returns `(clientId, App)`.
async fn seed_app_and_client(
    db: &Cratestack,
    monthly_quota: i64,
    default_sender_id: Option<String>,
) -> (String, schema::App) {
    let suffix = unique_suffix();
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "send-message test app".to_owned(),
            slug: format!("send-test-{}", suffix.to_lowercase()),
            description: None,
            defaultSenderIdId: default_sender_id,
            monthlyQuota: monthly_quota,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the app");

    let client_id = format!("client-{suffix}");
    // system, not owner: AppClient has no create policy for any human
    // role — provisionAppClient (#23) is its one intended writer, the
    // same way SmsClientStore is the one thing that ever reads
    // OauthClient. See the schema.cstack comment on AppClient's
    // `@@allow("create", ...)` for how this was found.
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

/// An active `SenderId` with one `approved` registration against a
/// throwaway `Provider` row — `SenderIdRegistration` requires a real
/// `providerId`.
async fn seed_approved_sender(db: &Cratestack) -> String {
    let suffix = unique_suffix();
    let value = format!("T{}", &suffix[..suffix.len().min(9)]).to_uppercase();

    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            // `^[a-z][a-z0-9_]{2,31}$` — 32 chars max total, so the
            // suffix has to stay short, not just lowercase.
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

    // active defaults true on create, matching an operator having already
    // activated it as part of the same onboarding flow.
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

fn args(to: &str, body: &str, sender_id: Option<&str>) -> send_message::Args {
    send_message::Args {
        args: schema::SendMessageInput {
            to: to.to_owned(),
            body: body.to_owned(),
            senderId: sender_id.map(str::to_owned),
            class: None,
            clientRef: None,
            scheduledAt: None,
            validityMinutes: None,
        },
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_well_formed_send_is_accepted_and_classified() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new();
    let (client_id, _app) = seed_app_and_client(&db, 1000, None).await;
    let sender = seed_approved_sender(&db).await;

    let result = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args("+237677123456", "Votre code est 4821", Some(&sender)),
        )
        .await
        .expect("a well-formed send must be accepted");

    assert_eq!(result.state, MessageState::accepted);
    assert_eq!(result.encoding, Encoding::gsm7);
    assert_eq!(result.segments, 1);
    // 677 -> mtn, from 0002_bootstrap's seeded operator_prefix_rules —
    // proves classify_operator actually queries the real table now,
    // rather than the hardcoded `unknown` previewMessage still reports.
    assert_eq!(result.operator, OperatorCode::mtn);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_unknown_client_id_is_unauthorized() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new();

    let error = procedures
        .send_message(
            &db,
            &app_caller("no-such-client"),
            args("+237677123456", "hi", None),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, cratestack::CoolError::Unauthorized(_)));
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_human_caller_is_rejected_with_a_clear_reason_not_a_guess() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new();
    // #24: give this caller the `sms:send` scope it would need to clear
    // `require_permission` (Layer 2), so the error asserted below is the
    // one this test actually means to exercise — `caller_client_id`'s
    // `kind == "app"` gap (Layer 1 admits an `owner` role here; nothing
    // about "human" is what stops this call) — not an incidental Layer 2
    // denial that would happen to contain neither "machine" nor "human".
    let mut human = Principal {
        sub: "a-human-user-id".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    human
        .extensions
        .insert("scope".to_owned(), Value::String("sms:send".to_owned()));

    let error = procedures
        .send_message(&db, &human, args("+237677123456", "hi", None))
        .await
        .unwrap_err();

    let message = error.to_string();
    assert!(
        message.contains("machine") || message.contains("human"),
        "expected the documented human-caller gap, got: {message}"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_opted_out_recipient_is_refused_before_persistence() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new();
    let (client_id, _app) = seed_app_and_client(&db, 1000, None).await;
    let sender = seed_approved_sender(&db).await;

    let to = unique_mtn_msisdn();
    let to = to.as_str();

    db.opt_out()
        .create(schema::CreateOptOutInput {
            msisdnHash: sha_of(to),
            msisdn: to.to_owned(),
            source: schema::OptOutSource::inbound_stop,
            scope: "all".to_owned(),
            reason: None,
            optedOutAt: Utc::now(),
        })
        .run(&sys())
        .await
        .expect("seeding the opt-out");

    let error = procedures
        .send_message(&db, &app_caller(&client_id), args(to, "hi", Some(&sender)))
        .await
        .unwrap_err();

    assert!(matches!(error, cratestack::CoolError::Validation(_)));

    let count = db
        .message()
        .aggregate()
        .count()
        .where_expr(cratestack::FilterExpr::from(
            schema::message::msisdnHash().eq(sha_of(to)),
        ))
        .run(&sys())
        .await
        .unwrap();
    assert_eq!(count, 0, "an opted-out send must not persist a row at all");
}

fn sha_of(input: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    let digest = Sha256::digest(input.as_bytes());
    let mut hex = String::from("sha256:");
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_full_monthly_quota_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new();
    // Quota of 0: any send at all should already be "at" quota.
    let (client_id, _app) = seed_app_and_client(&db, 0, None).await;
    let sender = seed_approved_sender(&db).await;

    let error = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args("+237677123456", "hi", Some(&sender)),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, cratestack::CoolError::Validation(_)));
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn no_sender_id_and_no_default_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new();
    let (client_id, _app) = seed_app_and_client(&db, 1000, None).await;

    let error = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args("+237677123456", "hi", None),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, cratestack::CoolError::Validation(_)));
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_unregistered_sender_id_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new();
    let (client_id, _app) = seed_app_and_client(&db, 1000, None).await;

    let error = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args("+237677123456", "hi", Some("NOTREGISTERED")),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, cratestack::CoolError::Validation(_)));
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_app_default_sender_is_used_when_none_is_given() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new();
    let sender = seed_approved_sender(&db).await;
    let sender_row = db
        .sender_id()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::sender_id::value().eq(sender.clone()),
        ))
        .limit(1)
        .run(&owner())
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let (client_id, _app) = seed_app_and_client(&db, 1000, Some(sender_row.id)).await;

    let result = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args("+237677123456", "hi", None),
        )
        .await
        .expect("the app's default sender must be used");

    assert_eq!(result.state, MessageState::accepted);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_unrecognised_prefix_classifies_as_unknown_not_a_guess() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new();
    let (client_id, _app) = seed_app_and_client(&db, 1000, None).await;
    let sender = seed_approved_sender(&db).await;

    // 640 is a valid, assigned mobile prefix (sms_msisdn::plan) that
    // 0002_bootstrap's seed data deliberately leaves unseeded (§3.4) — a
    // gap in the routing hint, not a reason to reject the send.
    let result = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args("+237640123456", "hi", Some(&sender)),
        )
        .await
        .expect("an unrecognised prefix is still a valid send");

    assert_eq!(result.operator, OperatorCode::unknown);
}
