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
//! `sms_test_support` provisions Postgres and applies both migrations
//! automatically (a shared, self-healing container — see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-api --test send_message_live_postgres -- --ignored
//! ```

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, procedures::send_message, procedures::ProcedureRegistry, Cratestack, Encoding,
    MessageClass, MessageState, OperatorCode,
};
use sms_api::{HashPepper, Procedures};

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

/// #134: the pepper every `Procedures` in this suite is built with, unless
/// a test is specifically about pepper *divergence* (see
/// `the_stored_hash_is_keyed_by_pepper_not_just_the_msisdn`). Fixed and
/// well over `HashPepper`'s own minimum, so it never needs touching when
/// that minimum changes.
fn test_pepper() -> HashPepper {
    HashPepper::new("send-message-live-postgres-test-pepper-well-over-the-minimum-length")
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
            circuitOpenUntil: None,
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
        // #59: SenderId is now @version'd.
        .if_match(sender.version)
        .run(&owner())
        .await
        .expect("activating the sender id");

    value
}

fn args(to: &str, body: &str, sender_id: Option<&str>) -> send_message::Args {
    args_with_class(to, body, sender_id, None)
}

/// #72: the same builder, with an explicit `class` — every consent/quiet-hours
/// test below needs to control this, where every pre-#72 test was happy to
/// let it default to `transactional`.
fn args_with_class(
    to: &str,
    body: &str,
    sender_id: Option<&str>,
    class: Option<MessageClass>,
) -> send_message::Args {
    send_message::Args {
        args: schema::SendMessageInput {
            to: to.to_owned(),
            body: body.to_owned(),
            senderId: sender_id.map(str::to_owned),
            class,
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
    let procedures = Procedures::new(test_pepper());
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
    let procedures = Procedures::new(test_pepper());

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
    let procedures = Procedures::new(test_pepper());
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

/// #72: opt-out honouring is class-gated (`consent::requires_recipient_consent_controls`)
/// — `notification` is one of the two classes it still covers, same as
/// before #72 changed this from an unconditional check to a gated one. See
/// `an_opted_out_recipient_still_receives_an_otp_or_transactional_message`
/// below for the exemption half of the same change.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_opted_out_recipient_is_refused_before_persistence() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());
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
        .send_message(
            &db,
            &app_caller(&client_id),
            args_with_class(to, "hi", Some(&sender), Some(MessageClass::notification)),
        )
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

/// #134: this **must** match what `sendMessage` actually persists, or
/// `an_opted_out_recipient_is_refused_before_persistence` above would pass
/// for the wrong reason — a hand-rolled hash that never collides with the
/// real one would make the opt-out lookup always miss, and the test would
/// only be proving `sendMessage` accepts a well-formed send, not that
/// opt-out matching works. Delegates to `sms_api::hmac_sha256_hex` — the
/// same function `Procedures::keyed_hash_hex` calls internally — instead of
/// duplicating the algorithm, which is exactly how this file's own
/// pre-#134 `sha_of` silently stopped matching production the moment the
/// scheme changed underneath it.
fn sha_of(input: &str) -> String {
    sms_api::hmac_sha256_hex(&test_pepper(), input)
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_full_monthly_quota_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());
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
    let procedures = Procedures::new(test_pepper());
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
    let procedures = Procedures::new(test_pepper());
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
    let procedures = Procedures::new(test_pepper());
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
    let procedures = Procedures::new(test_pepper());
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

/// Reads back a single `Message`'s `msisdnHash`, under the system context
/// the way `sendMessage`'s own internal reads do.
async fn msisdn_hash_of(db: &Cratestack, message_id: &str) -> String {
    db.message()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::message::id().eq(message_id.to_owned()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("reading back the message")
        .into_iter()
        .next()
        .expect("the message just created must exist")
        .msisdnHash
}

/// #134's own acceptance test, end to end through the real send path
/// rather than calling `hmac_sha256_hex` directly: the same MSISDN sent
/// under two different `Procedures` (two different peppers) must persist
/// two different `msisdnHash` values, and sent again under the *first*
/// pepper must persist the same value as the first time — proving the
/// stored hash is keyed by the pepper, not just a function of the MSISDN
/// the way the pre-#134 unkeyed `sha256_hex` was.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_stored_hash_is_keyed_by_pepper_not_just_the_msisdn() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let sender = seed_approved_sender(&db).await;
    let to = unique_mtn_msisdn();
    let to = to.as_str();

    let other_pepper =
        HashPepper::new("a-completely-different-send-message-live-postgres-pepper-value")
            .expect("the other test pepper meets HashPepper::new's minimum length");
    assert_ne!(
        sms_api::hmac_sha256_hex(&test_pepper(), "sanity-check"),
        sms_api::hmac_sha256_hex(&other_pepper, "sanity-check"),
        "the two test peppers in this test must actually differ"
    );

    let (client_a, _app_a) = seed_app_and_client(&db, 1000, None).await;
    let sent_a = Procedures::new(test_pepper())
        .send_message(&db, &app_caller(&client_a), args(to, "hi", Some(&sender)))
        .await
        .expect("sending under pepper A");
    let hash_a = msisdn_hash_of(&db, &sent_a.messageId).await;

    let (client_b, _app_b) = seed_app_and_client(&db, 1000, None).await;
    let sent_b = Procedures::new(other_pepper)
        .send_message(&db, &app_caller(&client_b), args(to, "hi", Some(&sender)))
        .await
        .expect("sending under pepper B — a distinct App, so quota/dedupe don't interfere");
    let hash_b = msisdn_hash_of(&db, &sent_b.messageId).await;

    assert_ne!(
        hash_a, hash_b,
        "the same MSISDN under two different peppers must persist two different hashes"
    );

    let (client_c, _app_c) = seed_app_and_client(&db, 1000, None).await;
    let sent_c = Procedures::new(test_pepper())
        .send_message(&db, &app_caller(&client_c), args(to, "hi", Some(&sender)))
        .await
        .expect("sending under pepper A again, from an independent Procedures instance");
    let hash_c = msisdn_hash_of(&db, &sent_c.messageId).await;

    assert_eq!(
        hash_a, hash_c,
        "the same pepper must hash the same MSISDN identically, even from a fresh Procedures"
    );
}

// ---------------------------------------------------------------------
// #72: consent records, the classification exemption, and quiet hours.
// ---------------------------------------------------------------------

/// Seeds a `ConsentRecord` matching `to`/`scope` under the given app —
/// mirrors `sha_of`'s own reasoning: goes through the real
/// `hmac_sha256_hex` `sendMessage` itself uses, never a hand-rolled second
/// copy of the hash.
async fn seed_consent(
    db: &Cratestack,
    app_id: &str,
    to: &str,
    scope: MessageClass,
) -> schema::ConsentRecord {
    db.consent_record()
        .create(schema::CreateConsentRecordInput {
            appId: app_id.to_owned(),
            msisdnHash: sha_of(to),
            msisdn: to.to_owned(),
            scope,
            channel: schema::ConsentChannel::web_form,
            consentedAt: Utc::now(),
            evidenceRef: Some("send-message-live-postgres-fixture".to_owned()),
        })
        .run(&sys())
        .await
        .expect("seeding a ConsentRecord")
}

/// #72's own guard-failure demonstration, as literally asked for: a
/// `marketing` send to a recipient with no consent record on file must be
/// refused, and a `transactional` send to that *same* recipient must not
/// be — proving the classification exemption is real and load-bearing, not
/// just documented. `marketing`'s refusal reason is deliberately not
/// pinned down further here (it could be "no consent on file" or "outside
/// quiet hours" depending on wall-clock time when this runs — both are
/// correct per `consent::requires_recipient_consent_controls` /
/// `consent::subject_to_quiet_hours` — see
/// `a_notification_send_with_no_consent_record_is_refused` below for the
/// consent-specific reason isolated from quiet hours entirely).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_marketing_send_is_refused_but_a_transactional_send_to_the_same_recipient_is_not() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());
    let (client_id, _app) = seed_app_and_client(&db, 1000, None).await;
    let sender = seed_approved_sender(&db).await;
    let to = unique_mtn_msisdn();
    let to = to.as_str();

    let marketing_error = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args_with_class(
                to,
                "50% off today only!",
                Some(&sender),
                Some(MessageClass::marketing),
            ),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(marketing_error, cratestack::CoolError::Validation(_)),
        "a marketing send with no consent on file must be refused, got: {marketing_error:?}"
    );

    let transactional_result = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args_with_class(
                to,
                "Your order has shipped",
                Some(&sender),
                Some(MessageClass::transactional),
            ),
        )
        .await
        .expect(
            "a transactional send to the same recipient, with no consent record either, \
             must not be refused — otp/transactional are exempt",
        );
    assert_eq!(transactional_result.state, MessageState::accepted);
}

/// The consent half of #72's guard, isolated from quiet hours entirely by
/// using `notification` (needs consent, is never time-restricted — see
/// `consent::subject_to_quiet_hours`'s own doc) rather than `marketing` —
/// deterministic no matter what time this runs.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_notification_send_with_no_consent_record_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());
    let (client_id, _app) = seed_app_and_client(&db, 1000, None).await;
    let sender = seed_approved_sender(&db).await;
    let to = unique_mtn_msisdn();

    let error = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args_with_class(&to, "hi", Some(&sender), Some(MessageClass::notification)),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, cratestack::CoolError::Validation(_)));

    let count = db
        .message()
        .aggregate()
        .count()
        .where_expr(cratestack::FilterExpr::from(
            schema::message::msisdnHash().eq(sha_of(&to)),
        ))
        .run(&sys())
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "a notification send refused for missing consent must not persist a row"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_notification_send_with_a_matching_consent_record_succeeds() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());
    let (client_id, app) = seed_app_and_client(&db, 1000, None).await;
    let sender = seed_approved_sender(&db).await;
    let to = unique_mtn_msisdn();

    seed_consent(&db, &app.id, &to, MessageClass::notification).await;

    let result = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args_with_class(&to, "hi", Some(&sender), Some(MessageClass::notification)),
        )
        .await
        .expect("a matching, on-file consent record must let this send through");

    assert_eq!(result.state, MessageState::accepted);
}

/// A consent record scoped to `marketing` does not authorise a
/// `notification` send to the same recipient — `ConsentRecord.scope`
/// matches `Message.class` exactly, not "any consent this recipient ever
/// gave".
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_consent_record_scoped_to_a_different_class_does_not_authorise_this_one() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());
    let (client_id, app) = seed_app_and_client(&db, 1000, None).await;
    let sender = seed_approved_sender(&db).await;
    let to = unique_mtn_msisdn();

    seed_consent(&db, &app.id, &to, MessageClass::marketing).await;

    let error = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args_with_class(&to, "hi", Some(&sender), Some(MessageClass::notification)),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, cratestack::CoolError::Validation(_)));
}

/// The opt-out half of the same classification exemption
/// (`an_opted_out_recipient_is_refused_before_persistence` above is the
/// "still covered" half): an opted-out recipient must still be able to
/// receive an OTP or a transactional message, per `docs/architecture.md`
/// §10 — "OTP and transactional are exempt in most regimes." Before #72
/// this was blocked unconditionally, which was accidentally *more*
/// restrictive than specified, not a compliance gap, but not what was
/// asked for either — see `consent.rs`'s own module doc.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_opted_out_recipient_still_receives_an_otp_or_transactional_message() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());
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

    let otp_result = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args_with_class(
                to,
                "Votre code est 4821",
                Some(&sender),
                Some(MessageClass::otp),
            ),
        )
        .await
        .expect("an OTP send to an opted-out recipient must still succeed");
    assert_eq!(otp_result.state, MessageState::accepted);

    let transactional_result = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args_with_class(
                to,
                "Your order has shipped",
                Some(&sender),
                Some(MessageClass::transactional),
            ),
        )
        .await
        .expect("a transactional send to an opted-out recipient must still succeed");
    assert_eq!(transactional_result.state, MessageState::accepted);
}

/// The quiet-hours bypass caught in review of #213, pinned so it cannot
/// come back.
///
/// The first cut of the guard checked the *accept* time. That is trivially
/// sidestepped: send a `marketing` message at a moment inside the allowed
/// window, with `scheduledAt` set to one outside it. `claim.rs`'s own
/// `candidates()` filter (`scheduledAt IS NULL OR scheduledAt <= now`)
/// holds the row until then and dispatches it — delivering during exactly
/// the hours the policy exists to protect.
///
/// Deterministic regardless of when it runs, and deliberately so: it picks
/// a `scheduledAt` the *same function `send()` calls* reports as outside
/// the window, rather than assuming anything about the wall clock. If the
/// window constants ever change, this test follows them instead of going
/// quietly stale.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_marketing_send_scheduled_into_quiet_hours_is_refused_however_it_is_accepted() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());
    let (client_id, app) = seed_app_and_client(&db, 1000, None).await;
    let sender = seed_approved_sender(&db).await;
    let to = unique_mtn_msisdn();

    // Consent on file, so a refusal can only be about the schedule.
    seed_consent(&db, &app.id, &to, MessageClass::marketing).await;

    // The next future instant the real guard reports as *outside* the
    // window — asked of the same function, never assumed.
    let mut scheduled = Utc::now() + chrono::Duration::hours(1);
    for _ in 0..24 {
        if !sms_api::consent::is_within_marketing_quiet_hours(scheduled) {
            break;
        }
        scheduled += chrono::Duration::hours(1);
    }
    assert!(
        !sms_api::consent::is_within_marketing_quiet_hours(scheduled),
        "the search must land outside the window, or this test proves nothing"
    );

    let mut args = args_with_class(
        &to,
        "50% off tomorrow!",
        Some(&sender),
        Some(MessageClass::marketing),
    );
    args.args.scheduledAt = Some(scheduled);

    let error = procedures
        .send_message(&db, &app_caller(&client_id), args)
        .await
        .expect_err(
            "a marketing send scheduled for delivery outside the allowed window must be \
             refused at accept time — otherwise the worker delivers it during quiet hours",
        );
    assert!(
        matches!(error, cratestack::CoolError::Validation(_)),
        "expected the quiet-hours Validation refusal, got: {error:?}"
    );
}

/// Proves `sendMessage` actually wires `consent::is_within_marketing_quiet_hours`
/// into the real send path — not just that the pure function itself is
/// correct (`crates/sms-api/src/consent.rs`'s own unit tests already cover
/// every boundary deterministically). Live-clock-adaptive by design: it
/// asks the exact same function `send()` calls internally what the
/// expected outcome is *right now*, so this test is correct and
/// deterministic no matter what time of day it actually runs — it can
/// never be flaky, and it can never silently stop testing the real
/// boundary the way a fixed "assume it's daytime" assumption could.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_marketing_send_matches_the_real_clocks_current_quiet_hours_state() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());
    let (client_id, app) = seed_app_and_client(&db, 1000, None).await;
    let sender = seed_approved_sender(&db).await;
    let to = unique_mtn_msisdn();

    // Consent on file so it can never be the reason for a refusal below —
    // quiet hours is the only variable this test means to exercise.
    seed_consent(&db, &app.id, &to, MessageClass::marketing).await;

    let expect_accepted = sms_api::consent::is_within_marketing_quiet_hours(Utc::now());

    let outcome = procedures
        .send_message(
            &db,
            &app_caller(&client_id),
            args_with_class(
                &to,
                "50% off today only!",
                Some(&sender),
                Some(MessageClass::marketing),
            ),
        )
        .await;

    if expect_accepted {
        let result = outcome.expect(
            "is_within_marketing_quiet_hours(now) is true, so this marketing send \
             must be accepted",
        );
        assert_eq!(result.state, MessageState::accepted);
    } else {
        let error = outcome.unwrap_err();
        assert!(
            matches!(error, cratestack::CoolError::Validation(_)),
            "is_within_marketing_quiet_hours(now) is false, so this marketing send \
             must be refused, got: {error:?}"
        );
    }
}
