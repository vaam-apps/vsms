//! Proves `Role::Hooks`'s real body (#40) against a real, fully migrated
//! Postgres and a real HTTP server (`wiremock`): the `Claimable for
//! WebhookAttempt` claim/reclaim discipline (`backends/crates/sms-worker/src/
//! claim.rs`), and `hooks.rs`'s own delivery/backoff/circuit-breaker logic
//! — the `attempt_state_transitions` state machine this PR adds, driven for
//! real rather than only unit-tested in isolation.
//!
//! Ignored by default, same convention as this crate's other live suites.
//! Run explicitly:
//!
//! ```bash
//! cargo test -p sms-worker --test hooks_live_postgres -- --ignored
//! ```

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, AttemptState, Cratestack, CreateWebhookAttemptInput, CreateWebhookEndpointInput,
    UpdateWebhookAttemptInput, UpdateWebhookEndpointInput, webhook_attempt,
};
use sms_test_support::webhook_receiver::WebhookReceiver;
use sms_webhook::{CONTRACT_SHA, Sealer};
use sms_worker::WorkerContext;
use sms_worker::hooks;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Same reasoning as every other live suite's own copy of this mutex (#102)
/// — see `claim_live_postgres.rs`'s own doc. `hooks::tick`'s own candidate
/// query is deliberately global (§7.3: a real claim loop must see every
/// endpoint's rows), so this binary's own tests, run concurrently by
/// default, would otherwise race on the same shared pool of claimable
/// attempts.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CratestackContext {
    Principal {
        sub: "sms-worker-hooks-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CratestackContext {
    Principal {
        sub: "sms-worker-hooks-test-owner".to_owned(),
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
        .expect("system clock is after the epoch")
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

/// Drains any non-terminal `WebhookAttempt` row left behind by a previous
/// test (in this run, or an earlier one against the same never-reset
/// database — `sms_test_support`'s own per-*binary* design, #118) to `dead`
/// before a new test seeds its own rows. `hooks::tick`'s claim query has no
/// way to know which test seeded which row, so a leftover claimable row is
/// exactly as eligible as the one a test is about to seed — the same
/// cross-test contamination `dispatch_live_postgres.rs`'s own
/// `clear_claimable_backlog` exists to prevent, applied to this model.
///
/// `pending`/`failed` have no direct edge to `dead` (only via `delivering`
/// — see `attempt_state_transitions`, §2.10), so this walks each row
/// through that one legal hop first.
async fn clear_claimable_backlog(db: &Cratestack) {
    let sys = sys();
    let backlog = db
        .webhook_attempt()
        .find_many()
        .where_expr(FilterExpr::from(webhook_attempt::state().in_([
            AttemptState::pending,
            AttemptState::failed,
            AttemptState::delivering,
        ])))
        .limit(500)
        .run(&sys)
        .await
        .expect("listing the webhook attempt backlog");

    for attempt in backlog {
        let delivering = if attempt.state == AttemptState::delivering {
            Some(attempt)
        } else {
            match db
                .webhook_attempt()
                .update(attempt.id.clone())
                .set(UpdateWebhookAttemptInput {
                    state: Some(AttemptState::delivering),
                    ..Default::default()
                })
                .if_match(attempt.version)
                .run(&sys)
                .await
            {
                Ok(row) => Some(row),
                Err(error) => {
                    tracing::warn!(
                        attempt_id = %attempt.id, %error,
                        "clearing the webhook attempt test backlog: could not move to delivering"
                    );
                    None
                }
            }
        };
        if let Some(row) = delivering {
            let result = db
                .webhook_attempt()
                .update(row.id.clone())
                .set(UpdateWebhookAttemptInput {
                    state: Some(AttemptState::dead),
                    ..Default::default()
                })
                .if_match(row.version)
                .run(&sys)
                .await;
            if let Err(error) = result {
                tracing::warn!(
                    attempt_id = %row.id, %error,
                    "clearing the webhook attempt test backlog: could not move to dead"
                );
            }
        }
    }
}

/// [`db`] plus [`clear_claimable_backlog`] — what every test in this file
/// should call instead of `db()` directly.
async fn isolated_db() -> Cratestack {
    let db = db().await;
    clear_claimable_backlog(&db).await;
    db
}

async fn seed_app(db: &Cratestack, suffix: &str) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "hooks test app".to_owned(),
            slug: format!("hooks-test-{suffix}"),
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

/// `max_attempts` and `mask_recipient` are per-test knobs — the two fields
/// this file's own tests actually vary.
async fn seed_endpoint(
    db: &Cratestack,
    suffix: &str,
    app_id: &str,
    url: &str,
    max_attempts: i64,
) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: url.to_owned(),
            audience: format!("hooks-test-{suffix}"),
            eventTypes: " message.delivered ".to_owned(),
            maskRecipient: true,
            maxAttempts: max_attempts,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the webhook endpoint")
}

/// Seeds one `pending` `WebhookAttempt` — the shape #38's own subscribers
/// produce, minus the database round trip through `Message`/`send`: this
/// file tests `hooks` in isolation, so it writes the row `hooks` would
/// otherwise be handed, directly.
async fn seed_attempt(
    db: &Cratestack,
    endpoint_id: &str,
    aggregate_id: &str,
    event_type: &str,
    payload: &str,
) -> schema::WebhookAttempt {
    db.webhook_attempt()
        .create(CreateWebhookAttemptInput {
            endpointId: endpoint_id.to_owned(),
            sourceEventId: cratestack::uuid::Uuid::new_v4(),
            aggregateId: aggregate_id.to_owned(),
            eventType: event_type.to_owned(),
            payload: payload.to_owned(),
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
        .expect("seeding the webhook attempt")
}

async fn reread_attempt(db: &Cratestack, id: &str) -> schema::WebhookAttempt {
    db.webhook_attempt()
        .find_many()
        .where_expr(FilterExpr::from(webhook_attempt::id().eq(id.to_owned())))
        .limit(1)
        .run(&sys())
        .await
        .expect("re-reading the webhook attempt")
        .into_iter()
        .next()
        .expect("the seeded attempt still exists")
}

async fn reread_endpoint(db: &Cratestack, id: &str) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .find_many()
        .where_expr(FilterExpr::from(
            sms_api::schema::webhook_endpoint::id().eq(id.to_owned()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("re-reading the webhook endpoint")
        .into_iter()
        .next()
        .expect("the seeded endpoint still exists")
}

fn worker_context(db: Cratestack) -> WorkerContext {
    // A lazy pool for the never-touched `provider` field — `hooks` never
    // constructs a `SmsProvider`, matching `dispatch_live_postgres.rs`'s own
    // reasoning for why a provider-less `WorkerContext` is fine outside
    // `dispatch`'s own tests.
    struct NeverCalled;
    #[async_trait::async_trait]
    impl sms_provider::SmsProvider for NeverCalled {
        fn key(&self) -> &str {
            unimplemented!("hooks never calls the provider")
        }
        fn capabilities(&self) -> sms_provider::Capabilities {
            unimplemented!("hooks never calls the provider")
        }
        async fn submit(
            &self,
            _req: &sms_provider::SubmitRequest,
        ) -> Result<sms_provider::SubmitAck, sms_provider::ProviderError> {
            unimplemented!("hooks never calls the provider")
        }
        fn parse_dlr(
            &self,
            _raw: &sms_provider::RawCallback,
        ) -> Result<Vec<sms_provider::DeliveryUpdate>, sms_provider::ProviderError> {
            unimplemented!("hooks never calls the provider")
        }
        async fn health(&self) -> sms_provider::Health {
            unimplemented!("hooks never calls the provider")
        }
    }
    WorkerContext {
        db,
        providers: std::sync::Arc::new(std::collections::HashMap::from([(
            "unused".to_owned(),
            std::sync::Arc::new(NeverCalled) as std::sync::Arc<dyn sms_provider::SmsProvider>,
        )])),
        webhook_sealer: Some(Sealer::new(vsms_signer()).expect("a sealer")),
    }
}

/// vsms's envelope signing key, as the worker would load it from its file.
fn vsms_signer() -> cratestack::cose::Ed25519Signer {
    cratestack::cose::Ed25519Signer::from_seed(&[0x42; 32])
}

/// What a receiver pins: the public half of [`vsms_signer`].
fn vsms_public_key() -> [u8; 32] {
    vsms_signer()
        .verify_key()
        .ed25519_bytes()
        .expect("an Ed25519 key")
}

/// The headers `wiremock` recorded, as the `(name, value)` pairs a receiver sees.
fn wire_headers(request: &wiremock::Request) -> Vec<(String, String)> {
    request
        .headers
        .iter()
        .map(|(name, value)| (name.as_str().to_owned(), value.to_str().unwrap().to_owned()))
        .collect()
}

/// A receiver the way vaam configures it: a real `EnvelopeLayer`, pinned to
/// vsms's key, answering for `audience` on `/hook`.
fn receiver_for(audience: &str) -> WebhookReceiver {
    WebhookReceiver::new(&[vsms_public_key()], audience, &["/hook"], CONTRACT_SHA)
}

/// The client the worker itself builds: a test that built its own would prove
/// nothing about the redirect policy production runs with.
fn http_client() -> reqwest::Client {
    hooks::build_http_client()
}

/// Happy path end to end: claim, seal, POST, `-> succeeded`. The bytes
/// `wiremock` recorded are handed to a real `EnvelopeLayer` (`Required`)
/// configured with vsms's public key and the endpoint's audience — what vaam
/// runs on `/webhooks/vsms` — and the envelope its handler sees is the one
/// §8.4 documents, with the event id inside the sealed body.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_successful_delivery_reaches_succeeded_and_a_real_envelope_layer_opens_it() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 8).await;
    let payload =
        r#"{"messageId":"cmsg000000000000000000","to":"+2376****89","state":"delivered"}"#;
    let attempt = seed_attempt(
        &db,
        &endpoint.id,
        "cmsg000000000000000000",
        "message.delivered",
        payload,
    )
    .await;

    let ctx = worker_context(db.clone());
    let http = http_client();
    hooks::tick(&ctx, &sys(), "test-worker", &http)
        .await
        .expect("tick succeeds");

    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, AttemptState::succeeded);
    assert_eq!(reread.attempts, 1);
    assert_eq!(reread.lastStatusCode, Some(200));
    assert!(
        reread.deliveredAt.is_some(),
        "delivered_at must be auto-stamped"
    );

    let requests = server
        .received_requests()
        .await
        .expect("request recording is on by default");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let headers = wire_headers(request);

    // No HMAC scheme is left on the wire: not the signature, not the timestamp,
    // and not the unauthenticated event headers the seal made redundant.
    for (name, _) in &headers {
        assert!(
            !name.starts_with("x-sms") && !name.contains("signature"),
            "unexpected header {name}"
        );
    }
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "content-type" && value.starts_with("application/cose")),
        "{headers:?}"
    );

    let receiver = receiver_for(&endpoint.audience);
    let verdict = receiver.deliver("/hook", &headers, &request.body).await;
    assert_eq!(
        verdict.status, 200,
        "a real EnvelopeLayer pinned to vsms's key and the endpoint's audience must accept \
         exactly the bytes the worker sent"
    );

    let bodies = receiver.bodies();
    assert_eq!(bodies.len(), 1);
    let envelope: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(envelope["id"], attempt.id);
    assert_eq!(envelope["eventId"], attempt.sourceEventId.to_string());
    assert_eq!(envelope["type"], "message.delivered");
    assert!(envelope["occurredAt"].is_string());
    // The two values a receiver compares with its own to know the delivery was
    // made for it, not merely addressed to its audience: signed with the rest.
    assert_eq!(envelope["appId"], endpoint.appId);
    assert_eq!(envelope["endpointId"], endpoint.id);
    assert_eq!(envelope["data"]["messageId"], "cmsg000000000000000000");

    // A clean delivery must not touch the endpoint's failure bookkeeping.
    let endpoint_after = reread_endpoint(&db, &endpoint.id).await;
    assert_eq!(endpoint_after.consecutiveFailures, 0);
    assert!(endpoint_after.circuitOpenUntil.is_none());
}

/// The worker's actual bytes, offered to receivers that are wrong in one way
/// each: another audience, a flipped bit, a second delivery of the same bytes.
/// Each is refused by the real layer, so none of the guards in `sms-webhook`'s
/// own suite is a property of a test fixture the worker does not produce.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn what_the_worker_sent_is_refused_by_a_wrong_audience_a_tampering_and_a_replay() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let endpoint = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 8).await;
    seed_attempt(&db, &endpoint.id, "cmsgA", "message.delivered", "{}").await;

    hooks::tick(
        &worker_context(db.clone()),
        &sys(),
        "test-worker",
        &http_client(),
    )
    .await
    .expect("tick succeeds");
    let requests = server.received_requests().await.expect("recording is on");
    assert_eq!(requests.len(), 1);
    let headers = wire_headers(&requests[0]);
    let body = requests[0].body.clone();

    // Another receiver's audience.
    let elsewhere = receiver_for("some-other-service");
    assert_eq!(
        elsewhere.deliver("/hook", &headers, &body).await.status,
        401
    );
    assert_eq!(elsewhere.bodies(), Vec::<String>::new());

    // One flipped bit.
    let mut tampered = body.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0x01;
    let receiver = receiver_for(&endpoint.audience);
    assert_eq!(
        receiver.deliver("/hook", &headers, &tampered).await.status,
        401
    );

    // The genuine bytes open once, and only once.
    assert_eq!(receiver.deliver("/hook", &headers, &body).await.status, 200);
    assert_eq!(receiver.deliver("/hook", &headers, &body).await.status, 401);
    assert_eq!(receiver.bodies().len(), 1);
}

/// A retry is a **new** sealed request: a fresh nonce and timestamp over the
/// same `id` and `eventId`. Both attempts open under one receiver (a re-sent
/// nonce would be refused as a replay), and the first one's bytes are not the
/// second's.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_retry_is_sealed_afresh_with_the_same_event_id() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let endpoint = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 8).await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgR", "message.delivered", "{}").await;

    let ctx = worker_context(db.clone());
    let http = http_client();
    hooks::tick(&ctx, &sys(), "test-worker", &http)
        .await
        .expect("first tick");
    assert_eq!(
        reread_attempt(&db, &attempt.id).await.state,
        AttemptState::failed
    );
    // The first backoff entry is 1 s.
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    hooks::tick(&ctx, &sys(), "test-worker", &http)
        .await
        .expect("second tick");
    assert_eq!(
        reread_attempt(&db, &attempt.id).await.state,
        AttemptState::succeeded
    );

    let requests = server.received_requests().await.expect("recording is on");
    assert_eq!(requests.len(), 2);
    assert_ne!(
        requests[0].body, requests[1].body,
        "a retry must not resend the first bytes"
    );

    let receiver = receiver_for(&endpoint.audience);
    for request in &requests {
        assert_eq!(
            receiver
                .deliver("/hook", &wire_headers(request), &request.body)
                .await
                .status,
            200
        );
    }
    let bodies = receiver.bodies();
    let ids: Vec<serde_json::Value> = bodies
        .iter()
        .map(|body| serde_json::from_str::<serde_json::Value>(body).unwrap())
        .collect();
    assert_eq!(ids[0]["id"], ids[1]["id"]);
    assert_eq!(ids[0]["eventId"], ids[1]["eventId"]);
}

/// A redirect is a refusal, not a hop: the sealed bytes (recipient and text
/// included) are never re-sent to a host the seal was not made for.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_redirect_is_not_followed() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/elsewhere"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/elsewhere"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let endpoint = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 8).await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgT", "message.delivered", "{}").await;

    hooks::tick(
        &worker_context(db.clone()),
        &sys(),
        "test-worker",
        &http_client(),
    )
    .await
    .expect("tick succeeds");
    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, AttemptState::failed);
    assert_eq!(reread.lastStatusCode, Some(307));
}

/// A URL the seal cannot bind (here a query string) never reaches the network;
/// it is an ordinary endpoint failure with its reason recorded.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_endpoint_url_that_cannot_be_sealed_is_never_sent_and_says_why() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let endpoint = seed_endpoint(
        &db,
        &suffix,
        &app_id,
        &format!("{}/hook?tenant=1", server.uri()),
        8,
    )
    .await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgU", "message.delivered", "{}").await;

    hooks::tick(
        &worker_context(db.clone()),
        &sys(),
        "test-worker",
        &http_client(),
    )
    .await
    .expect("tick succeeds");
    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, AttemptState::failed);
    assert!(
        reread
            .lastError
            .as_deref()
            .unwrap_or_default()
            .contains("not sealed"),
        "{:?}",
        reread.lastError
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// Fail closed: a process with no signing key claims nothing. A claim would
/// have counted an attempt against an endpoint for a delivery that cannot be
/// made.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_worker_without_a_signing_key_claims_nothing() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let endpoint = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 8).await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgK", "message.delivered", "{}").await;

    let ctx = WorkerContext {
        webhook_sealer: None,
        ..worker_context(db.clone())
    };
    let error = hooks::tick(&ctx, &sys(), "test-worker", &http_client())
        .await
        .expect_err("no key, no tick");
    assert!(error.to_string().contains("no webhook sealer"), "{error}");

    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, AttemptState::pending);
    assert_eq!(reread.attempts, 0);
}

/// A retryable failure (500) lands in `failed` with a scheduled retry, and
/// counts toward the endpoint's circuit-breaker bookkeeping — §8.5's own
/// "1s, 5s, 25s..." schedule, first entry.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_transient_failure_is_retried_with_backoff_and_counted_against_the_endpoint() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 8).await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsg1", "message.delivered", "{}").await;

    let ctx = worker_context(db.clone());
    let http = http_client();
    hooks::tick(&ctx, &sys(), "test-worker", &http)
        .await
        .expect("tick succeeds");

    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, AttemptState::failed);
    assert_eq!(reread.attempts, 1);
    assert_eq!(reread.lastStatusCode, Some(500));
    let next = reread.nextAttemptAt.expect("a retry must be scheduled");
    assert!(
        next > Utc::now(),
        "next_attempt_at must be in the future (backoff)"
    );
    assert!(
        next <= Utc::now() + Duration::seconds(30),
        "the first backoff entry is 1s — this must not be scheduled minutes out"
    );

    let endpoint_after = reread_endpoint(&db, &endpoint.id).await;
    assert_eq!(endpoint_after.consecutiveFailures, 1);
    assert!(endpoint_after.circuitOpenUntil.is_none());
}

/// An endpoint with `maxAttempts = 1` goes straight to `dead` on its first
/// failure — no second retry is ever scheduled.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn exhausting_max_attempts_goes_dead_without_a_further_retry() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 1).await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsg2", "message.delivered", "{}").await;

    let ctx = worker_context(db.clone());
    let http = http_client();
    hooks::tick(&ctx, &sys(), "test-worker", &http)
        .await
        .expect("tick succeeds");

    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, AttemptState::dead);
    assert_eq!(reread.attempts, 1);
    assert!(
        reread.nextAttemptAt.is_none() || reread.nextAttemptAt.unwrap() <= Utc::now(),
        "a dead attempt must not carry a fresh future retry time"
    );
}

/// §8.5: "410 Gone deactivates the endpoint immediately." Both the attempt
/// (`-> dead`) and the endpoint (`active = false`) must reflect it — one
/// HTTP response, two writes.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_410_response_kills_the_attempt_and_deactivates_the_endpoint() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(410))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 8).await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsg3", "message.delivered", "{}").await;

    let ctx = worker_context(db.clone());
    let http = http_client();
    hooks::tick(&ctx, &sys(), "test-worker", &http)
        .await
        .expect("tick succeeds");

    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, AttemptState::dead);
    assert_eq!(reread.lastStatusCode, Some(410));

    let endpoint_after = reread_endpoint(&db, &endpoint.id).await;
    assert!(
        !endpoint_after.active,
        "410 Gone must deactivate the endpoint"
    );
}

/// §8.5's circuit breaker: once `consecutiveFailures` crosses the
/// threshold, the endpoint's own attempts stop being claimed at all — a
/// fresh `pending` row for the same, now-open-circuit endpoint must be
/// skipped by the very next tick, not merely "would fail again."
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_circuit_breaker_opens_and_the_endpoint_stops_being_claimed() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = seed_endpoint(
        &db,
        &suffix,
        &app_id,
        &format!("{}/hook", server.uri()),
        100,
    )
    .await;
    // Fast-forward the endpoint to one failure short of the threshold —
    // driving 20 real HTTP round trips just to reach this state would make
    // the test slow without proving anything the direct write doesn't.
    db.webhook_endpoint()
        .update(endpoint.id.clone())
        .set(UpdateWebhookEndpointInput {
            consecutiveFailures: Some(19),
            ..Default::default()
        })
        // #59: WebhookEndpoint is now @version'd.
        .if_match(endpoint.version)
        .run(&sys())
        .await
        .expect("fast-forwarding consecutiveFailures");

    let tripping_attempt =
        seed_attempt(&db, &endpoint.id, "cmsg4", "message.delivered", "{}").await;

    let ctx = worker_context(db.clone());
    let http = http_client();
    hooks::tick(&ctx, &sys(), "test-worker", &http)
        .await
        .expect("tick succeeds");

    let endpoint_after = reread_endpoint(&db, &endpoint.id).await;
    assert_eq!(
        endpoint_after.consecutiveFailures, 0,
        "opening the circuit resets the counter for the next window"
    );
    let open_until = endpoint_after
        .circuitOpenUntil
        .expect("the circuit must now be open");
    assert!(open_until > Utc::now());
    assert!(
        open_until <= Utc::now() + Duration::minutes(16),
        "§8.5's own duration is 15 minutes"
    );
    let tripping_after = reread_attempt(&db, &tripping_attempt.id).await;
    assert_eq!(tripping_after.state, AttemptState::failed);

    // A second, distinct attempt on the same (now circuit-open) endpoint
    // must not be claimed by the very next tick.
    let excluded_attempt =
        seed_attempt(&db, &endpoint.id, "cmsg5", "message.submitted", "{}").await;
    hooks::tick(&ctx, &sys(), "test-worker", &http)
        .await
        .expect("tick succeeds");
    let excluded_after = reread_attempt(&db, &excluded_attempt.id).await;
    assert_eq!(
        excluded_after.state,
        AttemptState::pending,
        "a circuit-open endpoint's attempts must not be claimed"
    );
}

/// `hooks` must never reconstruct or enrich the recipient it delivers — the
/// masked value already baked into `payload` by #38's subscriber must
/// survive into the actual bytes sent over the wire, unchanged.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_masked_recipient_reaches_the_endpoint_unchanged_never_reconstructed() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 8).await;
    let masked_payload =
        r#"{"messageId":"cmsg000000000000000000","to":"+2376****89","state":"delivered"}"#;
    seed_attempt(
        &db,
        &endpoint.id,
        "cmsg6",
        "message.delivered",
        masked_payload,
    )
    .await;

    let ctx = worker_context(db.clone());
    let http = http_client();
    hooks::tick(&ctx, &sys(), "test-worker", &http)
        .await
        .expect("tick succeeds");

    let requests = server.received_requests().await.expect("recording is on");
    assert_eq!(requests.len(), 1);
    // The body on the wire is a sealed message, so the value is read where a
    // receiver reads it: from the envelope a real `EnvelopeLayer` hands its handler.
    let receiver = receiver_for(&endpoint.audience);
    let verdict = receiver
        .deliver("/hook", &wire_headers(&requests[0]), &requests[0].body)
        .await;
    assert_eq!(verdict.status, 200, "{verdict:?}");
    let opened = receiver.bodies().remove(0);
    let envelope: serde_json::Value = serde_json::from_str(&opened).unwrap();
    assert_eq!(
        envelope["data"]["to"], "+2376****89",
        "the masked value must reach the endpoint byte-for-byte, not a reconstructed real MSISDN"
    );
    // The envelope the receiver opened must contain no plausible unmasked E.164
    // Cameroon mobile number for this fixture's own MSISDN — a coarse but
    // real second check that nothing else in the pipeline re-derived one.
    assert!(
        !opened.contains("+237677900000"),
        "an unmasked MSISDN must never appear anywhere in the delivered body"
    );
}

/// Lightbridge's P2 on the original PR, confirmed real: a `WebhookAttempt`
/// whose stored `payload` doesn't even parse as JSON is *our* bug (#38's
/// subscriber wrote it), not the endpoint's fault — no HTTP request is ever
/// sent for it. Before the fix, `deliver_one` routed this into
/// `Outcome::Retryable`, whose handling unconditionally calls
/// `record_endpoint_failure`, so a single malformed row could trip the
/// circuit breaker on an endpoint that never received a request and was
/// never unhealthy. The assertions here are the actual point — a test that
/// only checked the attempt reached a terminal state would have passed
/// both before and after the fix, which is exactly the blind spot that let
/// this through.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_malformed_payload_goes_dead_without_ever_touching_the_endpoints_circuit_state() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    // No mock registered to respond successfully — a malformed payload
    // must never reach the point of making an HTTP call at all. `expect(0)`
    // is a second, independent check on top of the `received_requests()`
    // assertion below: if `hooks` ever regresses to calling out for this
    // row, wiremock itself panics at the server's own drop.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let endpoint = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 8).await;
    let attempt = seed_attempt(
        &db,
        &endpoint.id,
        "cmsg8",
        "message.delivered",
        "not valid json",
    )
    .await;

    let ctx = worker_context(db.clone());
    let http = http_client();
    hooks::tick(&ctx, &sys(), "test-worker", &http)
        .await
        .expect("tick succeeds");

    let requests = server.received_requests().await.expect("recording is on");
    assert!(
        requests.is_empty(),
        "a malformed payload must never produce an outbound HTTP request"
    );

    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(
        reread.state,
        AttemptState::dead,
        "a malformed payload will never become parseable — no point retrying it"
    );

    // The actual regression this test guards: a completely healthy
    // endpoint's circuit-breaker bookkeeping must be untouched by a row it
    // was never even asked to receive.
    let endpoint_after = reread_endpoint(&db, &endpoint.id).await;
    assert_eq!(
        endpoint_after.consecutiveFailures, 0,
        "our own bug must never count as an endpoint failure"
    );
    assert!(
        endpoint_after.circuitOpenUntil.is_none(),
        "our own bug must never trip the endpoint's circuit breaker"
    );
    assert!(
        endpoint_after.active,
        "a malformed payload must not deactivate the endpoint either"
    );
}

/// A `delivering` row whose lease already expired (a crashed worker,
/// simulated directly) is reclaimed by the very next tick and delivered —
/// without double-counting `attempts`, since the reclaim resumes the
/// already-counted attempt rather than starting a new one (`claim.rs`'s own
/// same-state reclaim, mirroring `Message`'s `routed` reclaim).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_stale_delivering_lease_is_reclaimed_without_double_counting_attempts() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 8).await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsg7", "message.delivered", "{}").await;

    // Simulate a crash mid-delivery: pending -> delivering, attempts=1,
    // lease already expired — exactly the shape claim.rs's take_lease
    // itself would have left behind, without a real worker ever crashing.
    let stale = db
        .webhook_attempt()
        .update(attempt.id.clone())
        .set(UpdateWebhookAttemptInput {
            state: Some(AttemptState::delivering),
            attempts: Some(1),
            leaseOwner: Some(Some("crashed-worker".to_owned())),
            leaseUntil: Some(Some(Utc::now() - Duration::seconds(5))),
            ..Default::default()
        })
        .if_match(attempt.version)
        .run(&sys())
        .await
        .expect("simulating a stale delivering lease");
    assert_eq!(stale.attempts, 1);

    let ctx = worker_context(db.clone());
    let http = http_client();
    hooks::tick(&ctx, &sys(), "test-worker", &http)
        .await
        .expect("tick succeeds");

    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, AttemptState::succeeded);
    assert_eq!(
        reread.attempts, 1,
        "the reclaim must resume the already-counted attempt, not count a second one"
    );
}
