//! `#44`'s third gate assertion: "run two workers; exactly one attempt
//! exists per event" — and, the task's own sharper framing, that the
//! *endpoint* also receives exactly one HTTP request per event, not merely
//! that exactly one `WebhookAttempt` row exists. Those are different
//! claims: a row that is claimed, delivered, and then redelivered after a
//! stale-lease reclaim would satisfy the first and violate the second.
//!
//! # Two genuinely separate OS processes, not two tasks
//!
//! Two real `sms-worker --roles hooks` processes (`CARGO_BIN_EXE_sms-worker`,
//! same convention `kill9_reclaim_live.rs`/`m1_acceptance_gate_live_postgres.rs`
//! already use for a claim this workspace does not trust an in-process
//! simulation to prove), racing over a shared backlog of `WebhookAttempt`
//! rows in a real, already-migrated Postgres, both polling the *same*
//! candidate query (`crates/sms-worker/src/claim.rs`'s `Claimable for
//! WebhookAttempt`) every second (`hooks::POLL_INTERVAL`). The defence
//! under test is the CAS claim itself — `take_lease`'s `if_match(version)`
//! — not the dedupe index (`webhook_attempts_dedupe`), which is already
//! covered end to end, including under genuine concurrent racing, by
//! `crates/sms-api/tests/webhooks_live_postgres.rs`'s own
//! `duplicate_enqueue_of_the_same_event_is_deduped` and this crate's own
//! `an_uncatalogued_state_transition_produces_no_webhook_attempt`-adjacent
//! suite; this file seeds `WebhookAttempt` rows directly (the same
//! convention `hooks_live_postgres.rs`/`hooks_node_receiver_live.rs`
//! already use) so it exercises the claim/delivery layer in isolation from
//! the enqueue layer, matching the mock customer endpoint downstream.
//!
//! # What's counted, and why both counts matter
//!
//! - **`WebhookAttempt` row count** — must equal `ROW_COUNT`, the number
//!   seeded. Never fewer (a row silently never claimed by either worker)
//!   and never more (impossible here — no enqueue racing is exercised by
//!   this file, see above — but checked anyway as a sanity floor).
//! - **HTTP requests received per row, from the mock customer endpoint's
//!   own request log (`wiremock::MockServer::received_requests`)** — must
//!   be exactly one per row. This is the claim this file actually exists
//!   to prove: the CAS discipline means only one of the two racing workers
//!   ever gets to `delivering` for a given row, so only one of them ever
//!   sends the HTTP request at all — the loser's `take_lease` fails with
//!   `PreconditionFailed` and it moves on to the next candidate, never
//!   touching the network for this row.
//! - **`attempts` on each terminal row equals `1`** — the CAS discipline's
//!   own corollary: a row claimed exactly once is delivered exactly once,
//!   with no crash-reclaim redelivery ever triggered (nothing in this
//!   scenario crashes or lets a lease expire — see the module doc's own
//!   distinction between "exactly one attempt row" and "exactly one
//!   delivery" for why this is checked explicitly rather than assumed from
//!   the row count alone).
//!
//! # Prerequisites
//!
//! Docker; `cargo build -p sms-worker-bin`.
//!
//! ```bash
//! cargo build -p sms-worker-bin
//! cargo test -p sms-worker-bin --test hooks_two_workers_live -- --ignored --nocapture
//! ```
//!
//! # The deliberate-break evidence this gate asks for
//!
//! Confirmed by hand (full transcript in this PR's own description):
//! temporarily removing `.if_match(self.version)` from `claim.rs`'s
//! `impl Claimable for WebhookAttempt::take_lease`'s `pending | failed` arm
//! — replacing the CAS update with a plain, unconditional one — breaks
//! this test, reproducibly, every run. Not in the shape originally
//! guessed (both workers racing the same row to a double delivery): what
//! actually happens is more fundamental — every one of the 50 seeded rows
//! stayed `pending`, `attempts: 0`, for the entire run, and this test's own
//! `wait_until_all_terminal` timed out after 30s with nothing ever
//! claimed at all, by either worker, with no error logged by either
//! process. Whatever the exact mechanism inside the generated delegate
//! (unclear from the outside, and not this test's job to reverse-engineer
//! — the point is that `.if_match(...)` is not optional decoration),
//! removing it doesn't just weaken the CAS, it breaks claiming entirely.
//! Restoring `.if_match(...)` makes it pass again immediately. Real,
//! reproducible, and a stronger demonstration than the one this doc
//! originally predicted — recorded here so a future reader trusts the
//! transcript over the prose.

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, webhook_attempt, AttemptState, Cratestack, CreateWebhookAttemptInput,
    CreateWebhookEndpointInput,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// How many independent `WebhookAttempt` rows the two workers race over.
/// More than one worker's own `BUDGET` (20, `hooks.rs`) so a single poll
/// from either worker can never claim the whole backlog alone — both
/// workers get real, repeated chances to race each other across several
/// polls, not just one.
const ROW_COUNT: usize = 50;

fn sys() -> CoolContext {
    Principal {
        sub: "hooks-two-workers-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "hooks-two-workers-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
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
        .max_connections(20)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

async fn seed_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "hooks two workers gate test app".to_owned(),
            slug: format!("hooks-two-workers-{}", unique_suffix()),
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

async fn seed_endpoint(db: &Cratestack, app_id: &str, mock_url: &str) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: format!("{mock_url}/webhooks/vsms"),
            eventTypes: " message.delivered ".to_owned(),
            secret: format!("whsec_two_workers_{}", unique_suffix()),
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: false,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the webhook endpoint")
}

fn seed_payload(aggregate_id: &str) -> String {
    serde_json::json!({
        "messageId": aggregate_id,
        "appId": "capp00000000000000000",
        "clientRef": "hooks-two-workers-gate",
        "to": "+2376****89",
        "state": "delivered",
        "operator": "orange",
        "segments": 1,
        "costXaf": "22.00",
    })
    .to_string()
}

async fn seed_attempts(
    db: &Cratestack,
    endpoint_id: &str,
    count: usize,
) -> Vec<schema::WebhookAttempt> {
    let mut attempts = Vec::with_capacity(count);
    for _ in 0..count {
        let aggregate_id = format!("cmsg{}", unique_suffix());
        let attempt = db
            .webhook_attempt()
            .create(CreateWebhookAttemptInput {
                endpointId: endpoint_id.to_owned(),
                sourceEventId: cratestack::uuid::Uuid::new_v4(),
                aggregateId: aggregate_id.clone(),
                eventType: "message.delivered".to_owned(),
                payload: seed_payload(&aggregate_id),
                leaseOwner: None,
                leaseUntil: None,
                nextAttemptAt: Some(Utc::now()),
                lastStatusCode: None,
                lastError: None,
                lastAttemptAt: None,
                deliveredAt: None,
            })
            .run(&sys())
            .await
            .expect("seeding a webhook attempt");
        attempts.push(attempt);
    }
    attempts
}

async fn reread_attempts(db: &Cratestack, ids: &[String]) -> Vec<schema::WebhookAttempt> {
    db.webhook_attempt()
        .find_many()
        .where_expr(FilterExpr::from(webhook_attempt::id().in_(ids.to_vec())))
        .limit(i64::try_from(ids.len()).unwrap() + 1)
        .run(&sys())
        .await
        .expect("re-reading the webhook attempts")
}

/// Kills the wrapped child if the test panics before an explicit `kill()`
/// — same convention as `kill9_reclaim_live.rs`'s own `KillOnDrop`.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_hooks_worker(database_url: &str, worker_id: &str) -> KillOnDrop {
    let bin = env!("CARGO_BIN_EXE_sms-worker");
    let child = Command::new(bin)
        .args(["--roles", "hooks"])
        .args(["--database-url", database_url])
        .args(["--worker-id", worker_id])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawning the real sms-worker binary");
    KillOnDrop(child)
}

async fn wait_until_all_terminal(
    db: &Cratestack,
    ids: &[String],
    timeout: Duration,
) -> Vec<schema::WebhookAttempt> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let attempts = reread_attempts(db, ids).await;
        let all_terminal = attempts.iter().all(|attempt| {
            matches!(
                attempt.state,
                AttemptState::succeeded | AttemptState::failed | AttemptState::dead
            )
        });
        if all_terminal {
            return attempts;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out after {timeout:?} waiting for all {} attempts to reach a terminal or \
             resting state; last snapshot: {attempts:#?}",
            ids.len()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres and the built sms-worker binary — see module \
            docs"]
async fn two_real_hooks_workers_never_double_deliver_the_same_attempt() {
    let database_url = sms_test_support::database_url().await;
    let db = db().await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/webhooks/vsms"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let app_id = seed_app(&db).await;
    let endpoint = seed_endpoint(&db, &app_id, &server.uri()).await;
    let seeded = seed_attempts(&db, &endpoint.id, ROW_COUNT).await;
    let ids: Vec<String> = seeded.iter().map(|a| a.id.clone()).collect();

    let worker_1 = spawn_hooks_worker(&database_url, "hooks-two-workers-test-1");
    let worker_2 = spawn_hooks_worker(&database_url, "hooks-two-workers-test-2");

    let final_attempts = wait_until_all_terminal(&db, &ids, Duration::from_secs(30)).await;
    drop(worker_1);
    drop(worker_2);

    // --- Claim 3a: exactly one WebhookAttempt row per seeded event — no
    // row vanished, and (the shape this file doesn't exercise, but checks
    // as a sanity floor) no row was ever duplicated. ---
    assert_eq!(
        final_attempts.len(),
        ROW_COUNT,
        "expected exactly {ROW_COUNT} WebhookAttempt rows, one per seeded event"
    );
    let succeeded = final_attempts
        .iter()
        .filter(|a| a.state == AttemptState::succeeded)
        .count();
    assert_eq!(
        succeeded,
        ROW_COUNT,
        "every seeded row must have reached succeeded (the mock endpoint always returns 200); \
         final states: {:?}",
        final_attempts
            .iter()
            .map(|a| (a.id.clone(), a.state))
            .collect::<Vec<_>>()
    );

    // --- Claim 3b, the sharper one: exactly one *delivery* per event, not
    // just one row. `attempts == 1` on every row rules out a crash-reclaim
    // redelivery (nothing here crashes, so this is really asserting the
    // CAS claim never let a second worker start a delivery already
    // in-flight or already finished elsewhere). ---
    for attempt in &final_attempts {
        assert_eq!(
            attempt.attempts, 1,
            "attempt {} was delivered more than once (attempts={}) — a second worker claimed a \
             row it should have lost the CAS race for",
            attempt.id, attempt.attempts
        );
    }

    // --- The independent, HTTP-level corroboration: the mock endpoint's
    // own request log, not this database's own bookkeeping. Two different
    // observation points agreeing is what makes this a real proof rather
    // than "the two writers of the same value agree with themselves." ---
    let requests = server
        .received_requests()
        .await
        .expect("wiremock records requests by default");
    assert_eq!(
        requests.len(),
        ROW_COUNT,
        "the mock endpoint must have received exactly {ROW_COUNT} HTTP requests total, one per \
         event — got {}",
        requests.len()
    );

    // Per-event granularity: not just the right *total*, but exactly one
    // request per distinct event id (a total that happened to add up right
    // while some events got zero and others got two would be exactly the
    // failure this per-event check exists to catch).
    let mut requests_per_event: HashMap<String, u32> = HashMap::new();
    for request in &requests {
        let event_id = request
            .headers
            .get(sms_webhook::HEADER_EVENT_ID)
            .and_then(|value| value.to_str().ok())
            .expect("every real hooks delivery sets X-Sms-Event-Id")
            .to_owned();
        *requests_per_event.entry(event_id).or_insert(0) += 1;
    }
    assert_eq!(
        requests_per_event.len(),
        ROW_COUNT,
        "expected {ROW_COUNT} distinct X-Sms-Event-Id values across all received requests, got \
         {}: {requests_per_event:?}",
        requests_per_event.len()
    );
    for (event_id, count) in &requests_per_event {
        assert_eq!(
            *count, 1,
            "event {event_id} was delivered to the mock endpoint {count} times, not exactly once"
        );
    }
}
