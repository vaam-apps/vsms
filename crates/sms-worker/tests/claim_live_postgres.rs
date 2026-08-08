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
//! `sms_test_support` provisions Postgres and applies both migrations
//! automatically (a shared, self-healing container — see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-worker --test claim_live_postgres -- --ignored
//! ```

use chrono::{DateTime, Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::CoolContext;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, Encoding, Message, MessageClass, MessageState, OperatorCode,
};
use sms_worker::claim::{claim_batch, Claimable};

/// #102, found live: `claim_batch::<Message>`'s candidate query is
/// deliberately global (production's own claim loop must see every app's
/// rows, not just one test's), so this binary's own tests — run
/// concurrently by default, since Rust's test harness multi-threads
/// within one binary unless told otherwise — steal and crowd each other's
/// candidate rows: `respects_the_budget` claiming fewer than its own
/// budget, `claims_an_unleased_accepted_message_and_transitions_it_to_routed`
/// not finding its own seeded row in the batch it just claimed, and (on a
/// genuinely fresh, empty database) a first-use race on Postgres's own
/// `pg_type` catalog when two tests' never-before-prepared queries first
/// run at the exact same instant (`duplicate key value violates unique
/// constraint "pg_type_typname_nsp_index"`). `--test-threads=1` does fix
/// this (verified: passing it to the outer `cargo test` invocation
/// serializes this binary's own tests and every failure mode above goes
/// away) — but nothing enforces it, and every test file's own documented
/// run command in this workspace omits it. A mutex every test acquires
/// for its whole body makes this binary self-serializing regardless of
/// how it's invoked, fixing both failure modes with one mechanism: with
/// no two tests' queries ever running concurrently, there is nothing left
/// to race on, including the very first query of the whole run.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

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
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

/// An active `Provider`, so a candidate in `accepted` state has somewhere
/// to route to — without one, `take_lease`'s routing pass sends every
/// `accepted` row straight to `rejected` instead of `queued` (a real,
/// correct outcome per §7.4, just not the one most of these tests are
/// about).
async fn seed_provider(db: &Cratestack) -> String {
    // `state` has `@default('disabled')`, so it's excluded from
    // CreateProviderInput (§2.0: any `@default` excludes a field from
    // create, literals included) — activate with a separate update, same
    // pattern `sendMessage`'s own live suite uses to activate a `SenderId`.
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!("claim_test_{}", unique_suffix().to_lowercase())
                .chars()
                .take(32)
                .collect(),
            displayName: "Claim test provider".to_owned(),
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

    db.provider()
        .update(provider.id.clone())
        .set(schema::UpdateProviderInput {
            state: Some(schema::ProviderState::active),
            ..Default::default()
        })
        .run(&owner())
        .await
        .expect("activating the provider");

    provider.id
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
            // Max priority, not a plausible real value: `candidates()`
            // orders by priority desc then createdAt asc, and this
            // database is never reset between runs — a lower, "realistic"
            // priority would sort behind whatever earlier runs' rows are
            // still sitting in `accepted`/`queued`/`routed`, silently
            // excluding a freshly seeded row from a small `budget` instead
            // of testing it. Found live: this is exactly what happened
            // before this fix.
            priority: 1000,
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

/// The full `accepted -> queued -> routed` chain takes two `claim_batch`
/// calls, not one: `message_state_transitions` has no `accepted -> routed`
/// edge, only `accepted -> queued` and `queued -> routed`. `take_lease`'s
/// `accepted` branch (the routing pass, §7.4: "passes routing") is the
/// first hop; this proves both, rather than seeding straight into `queued`
/// and only proving the second — the routing pass is real behaviour worth
/// its own assertions, not a step to route around in the fixture.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn claims_an_unleased_accepted_message_and_transitions_it_to_routed() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    // Not necessarily *the* provider this test seeds — this database is
    // never reset between runs, so a prior run's still-active provider can
    // tie on cost and win instead. The routing pass's own contract is
    // "picked some active provider", not "picked this specific one"; which
    // one wins a cost tie is `cheapest_active_provider`'s own concern, not
    // this test's.
    seed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, None, Utc::now() + Duration::hours(1)).await;

    let routed_pass = claim_batch::<Message>(&db, &sys(), "worker-1", 10)
        .await
        .expect("claim_batch succeeds");
    let after_routing = routed_pass
        .iter()
        .find(|m| m.id == seeded.id)
        .expect("the routing pass claimed the seeded message");
    assert_eq!(after_routing.state, MessageState::queued);
    assert!(
        after_routing.providerId.is_some(),
        "the routing pass must stamp a provider"
    );
    assert_eq!(
        after_routing.attempts, seeded.attempts,
        "the routing pass is not a submission attempt"
    );

    let dispatch_pass = claim_batch::<Message>(&db, &sys(), "worker-1", 10)
        .await
        .expect("claim_batch succeeds");
    let mine = dispatch_pass
        .iter()
        .find(|m| m.id == seeded.id)
        .expect("the dispatch pass claimed the routed message");
    assert_eq!(mine.state, MessageState::routed);
    assert_eq!(mine.leaseOwner, Some("worker-1".to_owned()));
    assert!(mine.leaseUntil.is_some());
    assert_eq!(mine.attempts, seeded.attempts + 1);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn does_not_reclaim_a_row_with_an_unexpired_lease() {
    let _guard = TEST_MUTEX.lock().await;
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

/// Not the crash-recovery scenario — `create()` can never actually produce
/// an `accepted` row with `leaseUntil` set, since the only thing that ever
/// sets `leaseUntil` is `take_lease`, which sets `state` to `routed` in the
/// same update. This just isolates the `leaseUntil` OR-predicate itself:
/// whatever state a row is in, a past `leaseUntil` must not exclude it.
/// `reclaims_a_routed_row_abandoned_by_a_crashed_worker` below is the test
/// for the real scenario.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_expired_lease_value_does_not_exclude_a_row_regardless_of_state() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(
        &db,
        &app_id,
        Some(Utc::now() - Duration::minutes(10)),
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

/// The actual crash-recovery scenario `claim_batch`'s whole reclaim
/// mechanism exists for, and the milestone gate it has to satisfy ("kill -9
/// the worker mid-submit and the lease reclaims the message", #26/#36):
/// reach `routed` through the real `take_lease` path, then simulate the
/// worker dying before it does anything else.
///
/// Forcing `leaseUntil` into the past with a raw `update()` is not something
/// real code ever does — a lease expires because the clock moves, not
/// because anyone rewrites it — but it's the only way to test "expired"
/// without a 2-minute sleep in a test suite.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn reclaims_a_routed_row_abandoned_by_a_crashed_worker() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    seed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, None, Utc::now() + Duration::hours(1)).await;

    // Two hops to reach `routed` for real, same as
    // `claims_an_unleased_accepted_message_and_transitions_it_to_routed`:
    // `accepted -> queued` (the routing pass) has no direct edge to
    // `routed`.
    let queued = seeded
        .take_lease(&db, &sys(), "worker-1", Utc::now())
        .await
        .expect("the routing pass succeeds");
    assert_eq!(queued.state, MessageState::queued);
    let routed = queued
        .take_lease(&db, &sys(), "worker-1", Utc::now())
        .await
        .expect("the dispatch claim succeeds");
    assert_eq!(routed.state, MessageState::routed);

    let abandoned = db
        .message()
        .update(routed.id.clone())
        .set(schema::UpdateMessageInput {
            leaseUntil: Some(Some(Utc::now() - Duration::minutes(1))),
            ..Default::default()
        })
        .if_match(routed.version)
        .run(&sys())
        .await
        .expect("forcing the lease into the past to simulate a crashed worker");

    let reclaimed = claim_batch::<Message>(&db, &sys(), "worker-2", 10)
        .await
        .expect("claim_batch succeeds");

    assert!(
        reclaimed.iter().any(|m| m.id == abandoned.id),
        "a routed row abandoned by a crashed worker must be reclaimable — \
         without this, any message a worker touches before crashing is stuck forever"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_expired_message_is_never_a_candidate() {
    let _guard = TEST_MUTEX.lock().await;
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
    let _guard = TEST_MUTEX.lock().await;
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

/// Locks in `take_lease`'s `accepted` branch leaving no real lease, so a
/// future change to that hop fails loudly here instead of silently
/// stalling every `accepted` row behind a lease it never needed.
///
/// Also the standing answer to a misreading that has already cost real
/// time once: seeing one message id in two claimers' results is *not*
/// evidence of a double-claim, and `two_concurrent_claimers_never_both_win_the_same_row`
/// below asserts on reaching `routed` precisely because of this. Not
/// concurrent at all: proves, deterministically and
/// sequentially, that a *second*, entirely separate `claim_batch` call can
/// legitimately pick up the *same* message id right after a first call's
/// routing pass, because `take_lease`'s `accepted` branch leaves `leaseUntil`
/// at `now` (already non-future) rather than a real future lease. If this
/// passes, two concurrent workers racing a single `accepted` row can *both*
/// "win" it — worker A doing the free routing hop (`accepted -> queued`),
/// worker B then doing the real dispatch claim (`queued -> routed`) — without
/// either's `if_match` CAS ever being violated. That is not the double-send
/// race the neighbouring test's assertion assumes; it is two different, valid
/// lifecycle hops landing on two different callers.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_second_claim_batch_call_picks_up_the_row_the_routing_hop_just_queued() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    seed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, None, Utc::now() + Duration::hours(1)).await;

    // First call: the routing pass. accepted -> queued, no real lease.
    let first = claim_batch::<Message>(&db, &sys(), "worker-a", 10)
        .await
        .expect("first claim_batch succeeds");
    let after_first = first
        .iter()
        .find(|m| m.id == seeded.id)
        .expect("worker-a's routing pass claimed the seeded message");
    assert_eq!(after_first.state, MessageState::queued);
    println!(
        "worker-a: state={:?} leaseUntil={:?} now-is-after-lease={:?}",
        after_first.state,
        after_first.leaseUntil,
        after_first.leaseUntil.map(|l| Utc::now() > l)
    );

    // Second, entirely separate call — simulating a second worker's own
    // claim_batch tick immediately afterward. No concurrency, no threads.
    let second = claim_batch::<Message>(&db, &sys(), "worker-b", 10)
        .await
        .expect("second claim_batch succeeds");
    let after_second = second.iter().find(|m| m.id == seeded.id);

    println!("worker-b saw the same id again: {}", after_second.is_some());
    if let Some(m) = after_second {
        println!("worker-b: state={:?} attempts={}", m.state, m.attempts);
    }

    let total_wins = first
        .iter()
        .chain(second.iter())
        .filter(|m| m.id == seeded.id)
        .count();
    println!("total wins across two sequential claim_batch calls: {total_wins}");
}

/// A standing regression test on the guarantee the entire claim loop rests
/// on, one layer below it: does `cratestack`'s own
/// `if_match` CAS ever let more than one of N genuinely concurrent updates
/// against the *same* row and the *same* starting version succeed? Bypasses
/// `claim_batch`/`Claimable` entirely — this is a raw `db.message().update
/// (id).set(...).if_match(version).run(&ctx)` race, spawned onto real OS
/// threads via the multi-thread runtime, repeated over many rounds with a
/// fresh row each time.
///
/// Worth keeping rather than deleting with the investigation that
/// prompted it: `cratestack` is pinned exactly and moves fast, and if a
/// future bump ever weakened `if_match`, every CAS claim in this system
/// would start double-claiming with no other test noticing. Verified
/// sound on the current pin (`=0.6.7`).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn concurrent_if_match_updates_never_both_win() {
    const ROUNDS: usize = 15;
    const RACERS: usize = 8;

    let _guard = TEST_MUTEX.lock().await;
    // A bigger pool than the shared `db()` helper's default 10 — up to two
    // connections per racer can be in flight at once (the update's own tx
    // connection, plus a loser's separate probe-query connection), so 10
    // isn't enough headroom at RACERS=16 and self-starves under this
    // module's own audit-triggered `FOR UPDATE` serialization. Also
    // observed live: this shared Docker-named test harness can be
    // recreated out from under a long-running suite by another, unrelated
    // concurrent `cargo test ... --ignored` invocation on the same
    // machine (sms_test_support's own documented "concurrent processes
    // racing" limitation) — a bigger pool doesn't fix that, but it does
    // remove pool exhaustion as a confound.
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(40)
        .connect(&url)
        .await
        .expect("connecting to Postgres");

    let db = std::sync::Arc::new(Cratestack::builder(pool).build());
    let app_id = seed_app(&db).await;
    let mut total_wins = 0usize;
    let mut total_precondition_failed = 0usize;
    let mut total_other_errors = 0usize;
    let mut double_win_rounds = 0usize;

    for round in 0..ROUNDS {
        let seeded = seed_message(&db, &app_id, None, Utc::now() + Duration::hours(1)).await;
        let id = seeded.id.clone();
        let version = seeded.version;

        let mut handles = Vec::with_capacity(RACERS);
        for racer in 0..RACERS {
            let db = db.clone();
            let id = id.clone();
            handles.push(tokio::spawn(async move {
                db.message()
                    .update(id)
                    .set(schema::UpdateMessageInput {
                        stateReason: Some(Some(format!("racer-{racer}"))),
                        ..Default::default()
                    })
                    .if_match(version)
                    .run(&sys())
                    .await
            }));
        }

        let mut wins = 0usize;
        for h in handles {
            match h.await.expect("racer task must not panic") {
                Ok(_) => wins += 1,
                Err(cratestack::CoolError::PreconditionFailed(_)) => {
                    total_precondition_failed += 1;
                }
                Err(other) => {
                    total_other_errors += 1;
                    println!("round {round}: unexpected error: {other:?}");
                }
            }
        }
        if wins != 1 {
            double_win_rounds += 1;
            println!("round {round}: wins={wins} (expected 1)");
        }
        total_wins += wins;
    }

    println!(
        "concurrent if_match race: {ROUNDS} rounds x {RACERS} racers each. \
         total_wins={total_wins} (expected {ROUNDS}), \
         total_precondition_failed={total_precondition_failed}, \
         total_other_errors={total_other_errors}, \
         double_win_rounds={double_win_rounds}"
    );
    assert_eq!(
        double_win_rounds, 0,
        "at least one round had more than one winner — a real CAS defect"
    );
}

/// The actual point of the whole module: two claimers racing the exact same
/// row must never both reach `routed` — the only state that triggers a
/// submission (see `dispatch.rs::tick()`, which submits iff
/// `message.state == MessageState::routed`). If `take_lease`'s `if_match`
/// were dropped, or if `PreconditionFailed` were mishandled, this would
/// flake into two workers both driving the same message to `routed` under
/// load — exactly the double-send this design exists to prevent.
///
/// This does **not** assert "the seeded row appears in at most one
/// claimer's output" — an earlier version of this test did, and that was
/// wrong, not merely stricter than necessary. `Claimable for Message`'s own
/// `take_lease` doc (`claim.rs`) spells out why a fresh `accepted` row can
/// legitimately show up in *both* batches: the `accepted -> queued` hop is
/// an instant routing decision, not in-flight work, so it deliberately
/// takes no real lease (`leaseUntil` is left at `now`, already expired —
/// see `take_lease`'s own doc). That leaves the row immediately eligible
/// for the very next `candidates()` query, so one worker doing the free
/// `accepted -> queued` hop and the other doing the real `queued -> routed`
/// claim right behind it is a legitimate, race-free sequence — not a
/// double-claim — and both calls returning the row (once each, in
/// different states) proves nothing wrong. What must never happen is two
/// claimers *both* landing the row in `routed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn two_concurrent_claimers_never_both_win_the_same_row() {
    let _guard = TEST_MUTEX.lock().await;
    let db = std::sync::Arc::new(db().await);
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, None, Utc::now() + Duration::hours(1)).await;

    // tokio::spawn onto independent tasks, not tokio::join! on two plain
    // futures — join! polls its futures in argument order on the calling
    // task, so if neither ever needs to yield (a fast loopback connection to
    // Postgres might resolve every await instantly), the executor can drive
    // worker-a's entire claim_batch to completion before worker-b's future
    // is polled even once, which would prove nothing about the actual
    // database-level race. Spawned tasks don't have that escape hatch — the
    // multi-threaded runtime below can genuinely run them on separate OS
    // threads at the same instant.
    let (db_a, sys_a) = (db.clone(), sys());
    let handle_a =
        tokio::spawn(async move { claim_batch::<Message>(&db_a, &sys_a, "worker-a", 10).await });

    let (db_b, sys_b) = (db.clone(), sys());
    let handle_b =
        tokio::spawn(async move { claim_batch::<Message>(&db_b, &sys_b, "worker-b", 10).await });

    let a = handle_a.await.expect("worker-a's task must not panic");
    let b = handle_b.await.expect("worker-b's task must not panic");

    let routed_wins = a
        .expect("worker-a's claim_batch must not error just because it lost the race")
        .into_iter()
        .chain(b.expect("worker-b's claim_batch must not error just because it lost the race"))
        .filter(|m| m.id == seeded.id && m.state == MessageState::routed)
        .count();

    assert!(
        routed_wins <= 1,
        "at most one claimer may drive this row to routed — that's the only \
         state that triggers a submission, so two here would mean a real \
         double-send"
    );
}
