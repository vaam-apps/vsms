//! Proves `claim_batch<Message>` against a real, fully migrated Postgres —
//! in particular, that two concurrent claimers racing the same row never
//! both win, which is the entire reason this module exists instead of
//! `.for_update()`.
//!
//! Unlike `lease`'s live suite, this one needs the *real* schema — the
//! `Message` state-machine trigger, the `App`/`Message` tables, `@version`.
//! Ignored by default, same convention as the rest of this workspace's live
//! suites. Run explicitly:
//!
//! ```bash
//! docker run --rm -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:16
//! createdb vsms_check
//! DATABASE_URL=postgres://postgres:postgres@localhost/vsms_check ./ci/apply-migrations.sh
//! DATABASE_URL=postgres://postgres:postgres@localhost/vsms_check \
//!     cargo test -p sms-worker --test claim_live_postgres -- --ignored
//! ```

use chrono::{DateTime, Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::CoolContext;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, Encoding, Message, MessageClass, MessageState, OperatorCode,
};
use sms_worker::claim::claim_batch;

fn sys() -> CoolContext {
    Principal {
        sub: "sms-worker-claim-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "sms-worker-claim-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

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
        .expect("DATABASE_URL must point at a fully migrated database — see module docs");
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

/// A fresh `App` per test, so `messages_app_idem_key`'s per-app uniqueness
/// can't make two tests' fixtures collide with each other.
async fn seed_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "claim test app".to_owned(),
            slug: format!("claim-test-{}", unique_suffix().to_lowercase()),
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

/// A `Message` in `accepted` state (create can't set any other state —
/// that's `@default('accepted')`'s whole point), with `leaseUntil` and
/// `expiresAt` left to the caller so each test can construct exactly the
/// candidate shape it needs to prove.
async fn seed_message(
    db: &Cratestack,
    app_id: &str,
    lease_until: Option<DateTime<Utc>>,
    expires_at: DateTime<Utc>,
) -> Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("claim-test-{}", unique_suffix())),
            msisdn: "+237677123456".to_owned(),
            msisdnHash: "sha256:claim-test".to_owned(),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 100,
            body: Some("claim loop test".to_owned()),
            bodyHash: "sha256:claim-test".to_owned(),
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
            leaseUntil: lease_until,
            scheduledAt: None,
            expiresAt: expires_at,
            submittedAt: None,
            finalizedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message")
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn claims_an_unleased_accepted_message_and_transitions_it_to_routed() {
    let db = db().await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, None, Utc::now() + Duration::hours(1)).await;

    let claimed = claim_batch::<Message>(&db, &sys(), "worker-1", 10)
        .await
        .expect("claim_batch succeeds");

    let mine = claimed
        .iter()
        .find(|m| m.id == seeded.id)
        .expect("the seeded message was claimed");
    assert_eq!(mine.state, MessageState::routed);
    assert_eq!(mine.leaseOwner, Some("worker-1".to_owned()));
    assert!(mine.leaseUntil.is_some());
    assert_eq!(mine.attempts, seeded.attempts + 1);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn does_not_reclaim_a_row_with_an_unexpired_lease() {
    let db = db().await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(
        &db,
        &app_id,
        Some(Utc::now() + Duration::minutes(10)), // held by "someone", not expired
        Utc::now() + Duration::hours(1),
    )
    .await;

    let claimed = claim_batch::<Message>(&db, &sys(), "worker-1", 10)
        .await
        .expect("claim_batch succeeds");

    assert!(
        !claimed.iter().any(|m| m.id == seeded.id),
        "a row with an unexpired lease must not be reclaimed"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn reclaims_a_row_with_an_expired_lease() {
    let db = db().await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(
        &db,
        &app_id,
        Some(Utc::now() - Duration::minutes(10)), // abandoned by a crashed worker
        Utc::now() + Duration::hours(1),
    )
    .await;

    let claimed = claim_batch::<Message>(&db, &sys(), "worker-1", 10)
        .await
        .expect("claim_batch succeeds");

    assert!(
        claimed.iter().any(|m| m.id == seeded.id),
        "an expired lease must be reclaimable — this is the only reaper the happy path has"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_expired_message_is_never_a_candidate() {
    let db = db().await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, None, Utc::now() - Duration::minutes(1)).await;

    let claimed = claim_batch::<Message>(&db, &sys(), "worker-1", 10)
        .await
        .expect("claim_batch succeeds");

    assert!(
        !claimed.iter().any(|m| m.id == seeded.id),
        "expiresAt in the past must exclude a row regardless of lease state"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn respects_the_budget() {
    let db = db().await;
    let app_id = seed_app(&db).await;
    for _ in 0..3 {
        seed_message(&db, &app_id, None, Utc::now() + Duration::hours(1)).await;
    }

    let claimed = claim_batch::<Message>(&db, &sys(), "worker-1", 2)
        .await
        .expect("claim_batch succeeds");

    assert_eq!(claimed.len(), 2, "budget=2 must claim at most 2 rows");
}

/// The actual point of the whole module: two claimers racing the exact same
/// row must never both win. If `take_lease`'s `if_match` were dropped, or if
/// `PreconditionFailed` were mishandled, this would flake into claiming the
/// same message twice under load — exactly the double-send this design
/// exists to prevent.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn two_concurrent_claimers_never_both_win_the_same_row() {
    let db = db().await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, None, Utc::now() + Duration::hours(1)).await;

    let (sys_a, sys_b) = (sys(), sys());
    let (a, b) = tokio::join!(
        claim_batch::<Message>(&db, &sys_a, "worker-a", 10),
        claim_batch::<Message>(&db, &sys_b, "worker-b", 10),
    );

    let wins = a
        .expect("worker-a's claim_batch must not error just because it lost the race")
        .into_iter()
        .chain(b.expect("worker-b's claim_batch must not error just because it lost the race"))
        .filter(|m| m.id == seeded.id)
        .count();

    assert_eq!(
        wins, 1,
        "exactly one of the two concurrent claimers must win"
    );
}
