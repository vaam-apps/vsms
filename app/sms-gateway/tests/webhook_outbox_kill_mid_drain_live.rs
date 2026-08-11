//! `#44`'s second gate assertion: "kill `sms-api` mid-drain; no event is
//! lost" — the outbox row is written inside the mutation's own transaction
//! (§8.1), so it must survive a hard kill of the process that wrote it.
//!
//! # Read `crates/sms-api/src/webhooks.rs`'s own module doc first — it
//! # changes what "no loss" means here
//!
//! Every `@@emit`-annotated mutation triggers an *automatic* post-commit
//! drain of its own process's runtime (`cratestack-sqlx`'s `create.rs`/
//! `update.rs`), and that drain reads **every currently-undelivered row in
//! `cratestack_event_outbox`, table-wide** — not just the row the
//! triggering write produced (confirmed by reading `drain_event_outbox`
//! directly in the vendored source: `SELECT ... WHERE delivered_at IS
//! NULL`, no scoping to one event). So "kill mid-drain" for a single event
//! is not a fixed, precisely-timed instant — it's "kill the process while
//! it is somewhere inside that `SELECT`'s own sequential `for row in rows`
//! loop, having called `emit` for at least one row and not yet finished."
//!
//! This test manufactures a real, comfortably wide window for that without
//! touching any production delivery code: it seeds `N` (`ENDPOINT_COUNT`)
//! real `WebhookEndpoint` rows, all subscribed to `message.delivered` for
//! the same app. A single `message.delivered` event's own subscriber
//! (`enqueue_message_webhook_attempts`) then has to perform `N` sequential
//! `WebhookAttempt` creates — each a real network round trip to Postgres —
//! inside **one** `emit` call for **one** outbox row. That single row's own
//! `delivered_at` UPDATE never runs until all `N` creates return, so
//! killing the process at any point during that sequence proves the
//! property this gate cares about: the row survives `delivered_at IS
//! NULL`, and a later drain (a second, independent `sms-gateway serve`
//! process, exactly like a real restart) resumes it — the dedupe index
//! (`webhook_attempts_dedupe`) is what makes a from-scratch re-run of the
//! same `N`-endpoint loop converge on exactly `N` rows rather than `2N`.
//!
//! Read via the delegate (`db.webhook_attempt().find_many()...run(&sys())`)
//! rather than a raw SQL read of `cratestack_event_outbox` itself — same
//! discipline `drain_live_postgres.rs`/`reap_outbox_live_postgres.rs`
//! already established for that table (no delegate exists for it; R1's
//! point is to avoid raw `sqlx` wherever a delegate read can stand in, and
//! here one can: `WebhookAttempt` counts are a faithful, indirect proxy for
//! outbox delivery state).
//!
//! # The kill window is timed via a real DB-visible signal, plus a settle
//! # delay found necessary empirically, not assumed
//!
//! `Message.state` becomes visible to a second connection the instant the
//! write's own transaction commits — strictly *before* the automatic
//! post-commit drain that follows it even begins. This test polls for that
//! (`wait_for_message_state`, the same technique `kill9_reclaim_live.rs`
//! already uses).
//!
//! Killing the *instant* that poll observes the commit was tried first, and
//! consistently landed at `0`/`N` on this machine, every run, regardless of
//! how large `N` was made (checked up to 2000) — this test's own polling
//! connection, having issued the same tiny query repeatedly, resumes from
//! its `await` faster than `sms-gateway`'s own task can be rescheduled and
//! dispatch even its first query, so the kill reliably wins the race
//! against the drain *starting* at all. `0`/`N` is still a genuine
//! "undelivered row survives a kill" instant (and every un-delayed run
//! still passed the recovery assertion), but it's a weaker demonstration
//! than a kill that lands inside the loop with visible partial progress. A
//! short, fixed settle delay after observing the commit — [`KILL_SETTLE_DELAY`]
//! — fixes this: with `N` = [`ENDPOINT_COUNT`] and this delay, repeated
//! local runs landed consistently between roughly 20 and 50 completed
//! creates out of 800 at kill time (never `0`, never close to `N`) — see
//! this PR's own Verification section for the actual transcript.
//!
//! # What this test does *not* claim
//!
//! It does not claim every possible kill instant is provably "mid-loop"
//! (an extremely unlucky kill landing after the very last create but before
//! the final `UPDATE ... delivered_at` would still pass the recovery
//! assertions below, just without the "genuinely interrupted" evidence the
//! pre-recovery assertion also checks for) — this is an inherent property
//! of racing a real OS-level kill against real, non-deterministic I/O
//! timing, the same caveat `kill9_reclaim_live.rs` accepts for its own
//! 500ms buffer. What it does prove, deterministically, every run: **zero
//! loss and zero duplication** — the row count after recovery is exactly
//! `N`, never fewer (a lost delivery) and never more (a duplicate defeating
//! the dedupe index) — regardless of exactly when the kill landed.
//!
//! # Prerequisites
//!
//! Docker (`sms_test_support` provisions and migrates Postgres);
//! `CARGO_BIN_EXE_sms-gateway` (built automatically by `cargo test`).
//!
//! ```bash
//! cargo test -p sms-gateway --test webhook_outbox_kill_mid_drain_live -- --ignored --nocapture
//! ```
//!
//! # The deliberate-break evidence this gate asks for
//!
//! Confirmed by hand (see this PR's own description for the full
//! transcript, captured before this file was committed in its current,
//! un-broken form): temporarily removing `webhooks.rs`'s own dedupe catch
//! —
//!
//! ```text
//! Err(error) if error.db_sqlstate() == Some(UNIQUE_VIOLATION) => {}
//! ```
//!
//! — turns a genuine mid-loop kill into a **permanently stuck** recovery,
//! not merely a slower one: the retried handler re-walks the same endpoint
//! list in the same order, hits the *first* already-created row's `23505`
//! immediately, now propagates it as a hard `Err` instead of skipping it,
//! and the outbox row is left undelivered with `last_error` recorded —
//! forever, since every subsequent drain repeats the identical failure at
//! the identical row. This test's own post-recovery assertion
//! (`attempt count == ENDPOINT_COUNT`) fails exactly as expected, capped
//! at whatever count existed at the moment of the kill, no matter how many
//! further drains run. Restoring the dedupe catch makes it pass again.
//! This is the real, load-bearing half of "no event is lost" that a kill
//! alone doesn't exercise: durability of the *row* is the framework's
//! guarantee (`tx.commit()` before `drain_event_outbox()` ever runs, per
//! §8.1); idempotent, complete *recovery* from a partial attempt is this
//! subscriber's own responsibility, and this is what proves it's real.

use std::net::TcpListener as StdTcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    message, provider as provider_filter, webhook_attempt, Cratestack, CreateAppInput,
    CreateMessageInput, CreateProviderInput, CreateWebhookEndpointInput, Encoding, MessageClass,
    MessageState, OperatorCode, ProviderKind, ProviderState, UpdateMessageInput,
    UpdateProviderInput,
};
/// How many `WebhookEndpoint`s subscribe to the one event this test fires
/// — see this module's own doc for why this is the mechanism that widens
/// the kill window. Tuned empirically, alongside [`KILL_SETTLE_DELAY`]
/// below, against `sms_test_support`'s dockerised Postgres on the machine
/// this test was developed on: large enough that the settle delay lands
/// consistently mid-loop rather than at the very end, without making the
/// test itself (which also has to seed `N` real rows, and fully re-drain
/// them from scratch during recovery) too slow to want to run.
const ENDPOINT_COUNT: usize = 800;

/// How long this test waits, after observing the triggering `Message`
/// write's own commit, before sending `SIGKILL` — see this module's own
/// "kill window" doc section for why this exists at all: killing the
/// instant the commit is observed reliably wins the race against
/// `sms-gateway`'s own task even *starting* its drain loop, landing at
/// `0`/[`ENDPOINT_COUNT`] every time rather than genuinely mid-loop.
const KILL_SETTLE_DELAY: Duration = Duration::from_millis(50);

const ORANGE_PROVIDER_KEY: &str = "orange_cm";
const TEST_HASH_PEPPER: &str = "webhook-outbox-kill-mid-drain-live-test-pepper-over-minimum";

fn sys() -> CoolContext {
    Principal {
        sub: "webhook-outbox-kill-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "webhook-outbox-kill-test-owner".to_owned(),
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

fn free_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port");
    listener
        .local_addr()
        .expect("reading the bound address")
        .port()
}

/// Idempotent, same convention as `m1_acceptance_gate_live_postgres.rs`'s
/// own `ensure_orange_cm_provider` — this database is never reset between
/// runs.
async fn ensure_orange_cm_provider(db: &Cratestack) -> String {
    let existing = db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(
            provider_filter::key().eq(ORANGE_PROVIDER_KEY.to_owned()),
        ))
        .limit(1)
        .run(&owner())
        .await
        .expect("looking up an existing orange_cm Provider row");

    if let Some(row) = existing.into_iter().next() {
        if row.state != ProviderState::active {
            db.provider()
                .update(row.id.clone())
                .set(UpdateProviderInput {
                    state: Some(ProviderState::active),
                    ..Default::default()
                })
                .run(&owner())
                .await
                .expect("reactivating the orange_cm Provider row");
        }
        return row.id;
    }

    let created = db
        .provider()
        .create(CreateProviderInput {
            key: ORANGE_PROVIDER_KEY.to_owned(),
            displayName: "Orange Cameroon (webhook outbox kill-mid-drain test)".to_owned(),
            kind: ProviderKind::orange_cm_http,
            config: "{}".to_owned(),
            credentialRef: "env:ORANGE_CM_CLIENT_ID".to_owned(),
            maxTps: 5.0,
            maxDailySubmissions: 5000,
            supportsDlr: true,
            supportsAlphaSender: true,
            supportsUcs2: true,
            supportsConcat: true,
            costPerSegmentXaf: "19".parse().expect("static decimal literal parses"),
            healthCheckedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the orange_cm Provider row");

    db.provider()
        .update(created.id.clone())
        .set(UpdateProviderInput {
            state: Some(ProviderState::active),
            ..Default::default()
        })
        .run(&owner())
        .await
        .expect("activating the orange_cm Provider row");

    created.id
}

async fn seed_app(db: &Cratestack, label: &str) -> String {
    db.app()
        .create(CreateAppInput {
            name: format!("webhook outbox kill test app ({label})"),
            // App.slug is @regex("^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$") — 40
            // chars max — so `label` must stay short.
            slug: format!("whk-mid-drain-{label}-{}", unique_suffix()),
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

/// `N` real `WebhookEndpoint` rows, all subscribed to `message.delivered`
/// for `app_id` — see this module's own doc for why this is what widens
/// the drain window. Each URL is syntactically valid but never actually
/// dialled: `hooks` (real HTTP delivery) never runs in this test, only
/// `webhooks.rs`'s own enqueue path.
async fn seed_many_endpoints(db: &Cratestack, app_id: &str, count: usize) {
    for i in 0..count {
        db.webhook_endpoint()
            .create(CreateWebhookEndpointInput {
                appId: app_id.to_owned(),
                url: format!(
                    "https://example.test/kill-mid-drain/{}/{i}",
                    unique_suffix()
                ),
                eventTypes: " message.delivered ".to_owned(),
                secret: format!("whsec_kill_mid_drain_{}_{i}", unique_suffix()),
                prevSecret: None,
                secretRotatedAt: None,
                maskRecipient: false,
                maxAttempts: 8,
                circuitOpenUntil: None,
            })
            .run(&owner())
            .await
            .expect("seeding a webhook endpoint");
    }
}

/// Walks a fresh message to `submitted` directly (bypassing HTTP/auth
/// entirely — this test's writer connection never registers subscribers,
/// so these intermediate transitions' own events are silently marked
/// delivered having done nothing, exactly like
/// `webhooks_live_postgres.rs`'s own `a_writer_that_never_registered_
/// subscribers_silently_loses_the_event` documents — which is fine here:
/// none of those intermediate events are what this test is about). Mirrors
/// `dlr_ingestion_live_postgres.rs`'s own `seed_submitted_message`.
async fn seed_submitted_message(db: &Cratestack, app_id: &str, provider_id: &str) -> String {
    let created = db
        .message()
        .create(CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("kill-mid-drain-{}", unique_suffix())),
            msisdn: "+237677123456".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:kill-mid-drain-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 1000,
            body: Some("webhook outbox kill-mid-drain test".to_owned()),
            bodyHash: "hmac-sha256-v1:kill-mid-drain-test".to_owned(),
            bodyLength: 27,
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
            expiresAt: Utc::now() + ChronoDuration::hours(1),
            submittedAt: None,
            finalizedAt: None,
            purgedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message");

    let queued = db
        .message()
        .update(created.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::queued),
            providerId: Some(Some(provider_id.to_owned())),
            ..Default::default()
        })
        .if_match(created.version)
        .run(&sys())
        .await
        .expect("accepted -> queued");

    let routed = db
        .message()
        .update(queued.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::routed),
            ..Default::default()
        })
        .if_match(queued.version)
        .run(&sys())
        .await
        .expect("queued -> routed");

    let provider_ref = format!("kill-mid-drain-ref-{}", unique_suffix());
    db.message()
        .update(routed.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::submitted),
            providerMessageRef: Some(Some(provider_ref)),
            ..Default::default()
        })
        .if_match(routed.version)
        .run(&sys())
        .await
        .expect("routed -> submitted")
        .id
}

async fn reload_message_state(db: &Cratestack, id: &str) -> MessageState {
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
        .state
}

async fn provider_message_ref_of(db: &Cratestack, id: &str) -> String {
    db.message()
        .find_many()
        .where_expr(FilterExpr::from(message::id().eq(id.to_owned())))
        .limit(1)
        .run(&sys())
        .await
        .expect("reading back the seeded message")
        .into_iter()
        .next()
        .expect("the seeded message exists")
        .providerMessageRef
        .expect("seed_submitted_message always stamps providerMessageRef")
}

async fn wait_for_message_state(db: &Cratestack, id: &str, want: MessageState, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let state = reload_message_state(db, id).await;
        if state == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {timeout:?} waiting for message {id} to reach {want:?}; last seen \
             {state:?}"
        );
        // Deliberately tight — the whole point is to observe the commit as
        // close to the instant it happens as this process can manage, so
        // the kill lands as early as possible inside the drain window.
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

async fn attempt_count_for(db: &Cratestack, message_id: &str) -> usize {
    db.webhook_attempt()
        .find_many()
        .where_expr(FilterExpr::from(
            webhook_attempt::aggregateId().eq(message_id.to_owned()),
        ))
        .limit(i64::try_from(ENDPOINT_COUNT).unwrap() + 10)
        .run(&sys())
        .await
        .expect("listing webhook attempts")
        .len()
}

/// A real, spawned `sms-gateway serve` OS process — see
/// `m1_acceptance_gate_live_postgres.rs`'s own `GatewayProcess` for the
/// precedent this is modelled on. This suite never needs the OP/token
/// machinery that struct also carries, so it's a leaner, local copy rather
/// than sharing code across two different test binaries (Cargo integration
/// tests can't share a module without a shared support crate, and this is
/// the only other file that needs it).
struct GatewayProcess {
    child: Child,
    base_url: String,
}

impl GatewayProcess {
    async fn spawn(db_url: &str, port: u16) -> Self {
        let issuer = format!("http://127.0.0.1:{port}");
        let mut command = Command::new(env!("CARGO_BIN_EXE_sms-gateway"));
        command
            .arg("serve")
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--database-url")
            .arg(db_url)
            .arg("--issuer")
            .arg(&issuer)
            .arg("--orange-client-id")
            .arg("kill-mid-drain-test-orange-client-id")
            .arg("--orange-client-secret")
            .arg("kill-mid-drain-test-orange-client-secret")
            .arg("--orange-sender-number")
            .arg("+237600000000")
            .arg("--hash-pepper")
            .arg(TEST_HASH_PEPPER)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = command.spawn().expect("spawning sms-gateway serve");
        println!(
            "webhook_outbox_kill_mid_drain_live: spawned sms-gateway serve, pid {}, base_url \
             {issuer}",
            child.id()
        );

        let base_url = issuer;
        wait_until_ready(&base_url, &mut child).await;
        Self { child, base_url }
    }

    /// Real SIGKILL, then reap — see `m1_acceptance_gate_live_postgres.rs`'s
    /// identical `kill_and_wait` for why this runs on a blocking thread and
    /// why it's a hard kill rather than a graceful shutdown: nothing this
    /// process held in flight (including whatever's mid-`for row in rows`
    /// inside its own automatic drain) gets a chance to finish or clean up.
    async fn kill_and_wait(mut self) {
        let pid = self.child.id();
        tokio::task::spawn_blocking(move || {
            self.child
                .kill()
                .expect("sending SIGKILL to sms-gateway serve");
            let status = self
                .child
                .wait()
                .expect("reaping sms-gateway serve after SIGKILL");
            println!(
                "webhook_outbox_kill_mid_drain_live: killed sms-gateway serve, pid {pid}, exit \
                 status {status:?}"
            );
        })
        .await
        .expect("joining the blocking kill/wait task");
    }

    /// POST `/dlr/orange_cm` and wait for the response — used both to
    /// trigger the kill target's own commit+drain (fired without waiting,
    /// see the test body) and, on the *second* process, to trigger — and
    /// wait all the way through — the recovery drain.
    async fn post_dlr(&self, body: serde_json::Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}/dlr/{ORANGE_PROVIDER_KEY}", self.base_url))
            .json(&body)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .expect("POSTing to /dlr/orange_cm")
    }
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn wait_until_ready(base_url: &str, child: &mut Child) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("sms-gateway serve exited before becoming ready: {status:?}");
        }
        if let Ok(response) = client
            .get(format!("{base_url}/.well-known/openid-configuration"))
            .send()
            .await
        {
            if response.status().is_success() {
                return;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "sms-gateway serve never became ready within 15s"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// A well-formed Orange `deliveryInfoNotification` body driving `provider_ref`
/// to `Delivered` — see `crates/sms-provider-orange-cm/src/dlr.rs`'s own
/// `outcome_of` for the exact status string this maps from.
fn delivered_dlr_body(provider_ref: &str) -> serde_json::Value {
    serde_json::json!({
        "deliveryInfoNotification": {
            "callbackData": provider_ref,
            "deliveryInfo": [
                {"address": "tel:+237677123456", "deliveryStatus": "DeliveredToTerminal"}
            ]
        }
    })
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn killing_sms_gateway_mid_drain_loses_no_event_and_creates_no_duplicate() {
    let database_url = sms_test_support::database_url().await;
    let db = db().await;

    // `sms-gateway serve` fails at process start (not lazily) without an
    // active OP signing key — see AGENTS.md's M1 section. Idempotent
    // rotation isn't needed here the way `ensure_orange_cm_provider` is for
    // the Provider row: `rotate_signing_key` always mints a fresh key and
    // this database is never reset, so a rerun just accumulates keys, which
    // is harmless and exactly what a real operator's own repeated rotations
    // would do too.
    sms_auth::op::rotate_signing_key(&db, &sys(), sms_auth::op::ROTATION_OVERLAP)
        .await
        .expect("rotating in an active OP signing key");

    let provider_id = ensure_orange_cm_provider(&db).await;

    // --- Every fixture this test will ever need is seeded up front,
    // before process_a even starts — both the kill target's own (one app,
    // N endpoints subscribed to message.delivered, one message already
    // `submitted`) *and* the recovery-trigger message used later. This
    // ordering is load-bearing, not tidy: every one of these seeding calls
    // runs through this test's own `db`, which never calls
    // `register_subscribers` — exactly the "unregistered writer" shape
    // `webhooks_live_postgres.rs`'s own `a_writer_that_never_registered_
    // subscribers_silently_loses_the_event` documents. Its own automatic
    // post-commit drain scans *every* currently-undelivered outbox row
    // table-wide (not just its own), so any such write issued *after* the
    // kill would silently mark the still-stuck victim row delivered,
    // having done nothing — self-inflicting the exact loss this test
    // exists to disprove, before process_b ever gets a chance to recover
    // it for real. Found live, writing this test: an earlier draft seeded
    // the recovery-trigger fixture *after* the kill and it reliably wiped
    // the stuck row via this exact mechanism. Seeding everything up front
    // means the only write that happens after the kill is process_b's own
    // real, registered DLR POST. ---
    let app_id = seed_app(&db, "victim").await;
    seed_many_endpoints(&db, &app_id, ENDPOINT_COUNT).await;
    let message_id = seed_submitted_message(&db, &app_id, &provider_id).await;
    let provider_ref = provider_message_ref_of(&db, &message_id).await;

    let recovery_app_id = seed_app(&db, "recovery-trigger").await;
    let recovery_message_id = seed_submitted_message(&db, &recovery_app_id, &provider_id).await;
    let recovery_provider_ref = provider_message_ref_of(&db, &recovery_message_id).await;

    let port_a = free_port();
    let process_a = GatewayProcess::spawn(&database_url, port_a).await;

    // Fire the DLR that will trigger the N-endpoint drain, but don't await
    // its response — the entire commit+drain sequence happens *inside*
    // process_a's own handling of this request, strictly before it ever
    // writes a response, so waiting for it here would mean waiting for the
    // very thing this test is trying to interrupt.
    let dlr_body = delivered_dlr_body(&provider_ref);
    let fire_and_forget = {
        let process_a_url = process_a.base_url.clone();
        tokio::spawn(async move {
            // A response here (or a connection error from the kill) is
            // fine either way — nothing in this task's own outcome is
            // asserted on; it exists only to originate the request.
            let _ = reqwest::Client::new()
                .post(format!("{process_a_url}/dlr/{ORANGE_PROVIDER_KEY}"))
                .json(&dlr_body)
                .timeout(Duration::from_secs(5))
                .send()
                .await;
        })
    };

    // The real, DB-visible signal: the moment this test's own connection
    // can see state == delivered, process_a's write has committed and its
    // automatic post-commit drain is either about to start or already
    // walking the N-endpoint loop.
    wait_for_message_state(
        &db,
        &message_id,
        MessageState::delivered,
        Duration::from_secs(10),
    )
    .await;

    // See KILL_SETTLE_DELAY's own doc for why this sleep exists at all —
    // it's load-bearing, not a tidiness sleep.
    tokio::time::sleep(KILL_SETTLE_DELAY).await;

    process_a.kill_and_wait().await;
    let _ = fire_and_forget.await;

    // --- Post-kill: the message write itself is untouched (it committed
    // in a separate, already-finished transaction before the drain this
    // test interrupted ever began) — sanity-check that directly. ---
    assert_eq!(
        reload_message_state(&db, &message_id).await,
        MessageState::delivered,
        "the DLR-driven Message write committed before the drain started; a kill during the \
         drain must never roll back or revert the write that triggered it"
    );

    let count_after_kill = attempt_count_for(&db, &message_id).await;
    println!(
        "webhook_outbox_kill_mid_drain_live: {count_after_kill}/{ENDPOINT_COUNT} WebhookAttempt \
         rows exist immediately after the kill"
    );
    assert!(
        count_after_kill < ENDPOINT_COUNT,
        "expected the kill to land genuinely mid-drain (fewer than all {ENDPOINT_COUNT} \
         WebhookAttempt rows created yet); got {count_after_kill} — either the kill landed too \
         late (the drain had already finished; consider raising ENDPOINT_COUNT) or too early \
         (nothing had run yet, which is still a valid but less interesting instant)"
    );

    // --- Recovery: a second, independent sms-gateway serve process (a real
    // restart, same database, new port — no shared memory with process_a)
    // triggers its own automatic drain via the *only* write this test
    // issues after the kill (see the seeding comment above for why nothing
    // else may run here first), which — per drain_event_outbox's own
    // table-wide SELECT — also catches up the still-undelivered row the
    // kill left behind. ---
    let port_b = free_port();
    let process_b = GatewayProcess::spawn(&database_url, port_b).await;

    // Waited for in full this time — the point of this second call is to
    // observe the *completed* recovery drain, not to interrupt it.
    let recovery_response = process_b
        .post_dlr(delivered_dlr_body(&recovery_provider_ref))
        .await;
    assert_eq!(
        recovery_response.status(),
        reqwest::StatusCode::ACCEPTED,
        "the recovery-trigger DLR must itself be accepted"
    );

    let count_after_recovery = attempt_count_for(&db, &message_id).await;
    assert_eq!(
        count_after_recovery, ENDPOINT_COUNT,
        "after a second registered process's own drain, the originally-interrupted event must \
         have produced exactly {ENDPOINT_COUNT} WebhookAttempt rows — not fewer (loss) and not \
         more (the dedupe index failing to hold under a from-scratch retry of a partially \
         completed handler); got {count_after_recovery}"
    );

    process_b.kill_and_wait().await;
}
