//! Proves `Role::Hooks`'s real body (#40) against a real, fully migrated
//! Postgres and a real HTTP server (`wiremock`): the `Claimable for
//! WebhookAttempt` claim/reclaim discipline (`backends/crates/sms-worker/src/
//! claim.rs`), and `hooks.rs`'s own delivery/backoff/circuit-breaker logic
//! — the `attempt_state_transitions` state machine this PR adds, driven for
//! real rather than only unit-tested in isolation.
//!
//! Both delivery modes (VS-8R): a `sealed` endpoint's deliveries are opened by
//! a real `EnvelopeLayer`, an `hmac` endpoint's are verified with
//! `sms_webhook::verify` against the exact bytes `wiremock` recorded, including
//! the rotation overlap and the unchanged four-field body. The status handling
//! (retry, dead, 410, breaker) is one path for both and is exercised on each.
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
    UpdateWebhookAttemptInput, UpdateWebhookEndpointInput, WebhookAttempt, WebhookDeliveryMode,
    webhook_attempt,
};
use sms_test_support::webhook_receiver::WebhookReceiver;
use sms_webhook::{CONTRACT_SHA, Sealer};
use sms_worker::WorkerContext;
use sms_worker::claim::{
    ServedModes, claim_batch, claim_webhook_attempts, count_attempts_waiting,
    filter_by_endpoint_health,
};
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
    // Rows `a_keyless_claim_never_reads_the_sealed_backlogs_index_entries` bulk
    // inserts: thousands of them, past the 500-row sweep below, and left behind
    // by a test that failed before it could delete them.
    psql("DELETE FROM webhook_attempts WHERE aggregate_id LIKE 'bulk-%'").await;
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
            deliveryMode: schema::WebhookDeliveryMode::sealed,
            audience: Some(format!("hooks-test-{suffix}")),
            secret: None,
            prevSecret: None,
            secretRotatedAt: None,
            eventTypes: " message.delivered ".to_owned(),
            maskRecipient: true,
            maxAttempts: max_attempts,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the webhook endpoint")
}

/// An `hmac` endpoint (VS-8R): a secret, no audience, and `prev_secret` while a
/// rotation's overlap window is open.
async fn seed_hmac_endpoint(
    db: &Cratestack,
    app_id: &str,
    url: &str,
    max_attempts: i64,
    secret: &str,
    prev_secret: Option<&str>,
) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: url.to_owned(),
            deliveryMode: schema::WebhookDeliveryMode::hmac,
            audience: None,
            secret: Some(secret.to_owned()),
            prevSecret: prev_secret.map(str::to_owned),
            secretRotatedAt: prev_secret.map(|_| Utc::now()),
            eventTypes: " message.delivered ".to_owned(),
            maskRecipient: true,
            maxAttempts: max_attempts,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the hmac webhook endpoint")
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
        hmac_webhooks: sms_api::HmacWebhooks::Permitted,
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
fn http_client() -> hooks::HttpClients {
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

    let receiver = receiver_for(endpoint.audience.as_deref().expect("a sealed endpoint"));
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
    let receiver = receiver_for(endpoint.audience.as_deref().expect("a sealed endpoint"));
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

    let receiver = receiver_for(endpoint.audience.as_deref().expect("a sealed endpoint"));
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

/// Writes `value` into one of an endpoint's secret columns behind the API's back,
/// with `psql`. The API refuses a secret under 32 characters, so a short or empty
/// one exists only from before that bound, or from a hand edit; the worker still
/// signs with it (R5), and this is how the test gets one.
async fn force_column(endpoint_id: &str, column: &str, value: &str) {
    assert!(matches!(column, "secret" | "prev_secret"), "{column}");
    psql(&format!(
        "UPDATE webhook_endpoints SET {column} = '{value}' WHERE id = '{endpoint_id}'"
    ))
    .await;
}

/// Runs one statement with `psql` against the suite's database, for what the API
/// cannot do (a bulk insert, a hand edit, a statistics read), and returns its
/// output with no headers or alignment.
async fn psql(sql: &str) -> String {
    psql_at(&sms_test_support::database_url().await, sql)
}

/// [`psql`] against `database_url`.
fn psql_at(database_url: &str, sql: &str) -> String {
    let output = std::process::Command::new("psql")
        .arg(database_url)
        .args(["-v", "ON_ERROR_STOP=1", "-X", "-q", "-A", "-t", "-c"])
        .arg(sql)
        .output()
        .expect("spawning `psql`");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// An `hmac` row whose `secret` and `prevSecret` are empty (a row from before the
/// API's 32-character bound, or a hand edit) is delivered exactly as it was at
/// `29f93c4`: signed with the empty key, the empty previous secret a second
/// `v1=` value. Owner's ruling R5: vsms serves apps beyond vaam, so the HMAC
/// delivery stays byte-identical for the receivers built against it. The
/// worker warns instead of withholding (`hooks.rs`'s unit test reads the line).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_hmac_endpoint_with_empty_secrets_is_delivered_signed_with_the_empty_key() {
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
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hook", server.uri()),
        1,
        "whsec_to_be_emptied_0123456789abcdef0123456789",
        Some("whsec_prev_to_be_emptied_0123456789abcdef01234"),
    )
    .await;
    force_column(&endpoint.id, "secret", "").await;
    force_column(&endpoint.id, "prev_secret", "").await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgE", "message.delivered", "{}").await;

    hooks::tick(
        &worker_context(db.clone()),
        &sys(),
        "test-worker",
        &http_client(),
    )
    .await
    .expect("tick succeeds");

    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, AttemptState::succeeded, "{reread:?}");
    assert_eq!(reread.attempts, 1);
    let requests = server.received_requests().await.expect("recording is on");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let header = |name: &str| {
        request
            .headers
            .get(name)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    };
    let timestamp: i64 = header("x-sms-timestamp").parse().unwrap();
    let empty_key = sms_webhook::sign_v1(
        "",
        timestamp,
        &attempt.sourceEventId.to_string(),
        &request.body,
    );
    assert_eq!(
        header("x-sms-signature"),
        format!("v1={empty_key},v1={empty_key}"),
        "the empty secret signs, and the empty previous secret is a second value"
    );
    assert_eq!(
        reread_endpoint(&db, &endpoint.id).await.consecutiveFailures,
        0
    );
}

/// A process with no signing key (VS-8R) still delivers an `hmac` endpoint,
/// which needs none: the signature is the endpoint's own secret.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_worker_without_a_signing_key_still_delivers_an_hmac_endpoint() {
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
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hook", server.uri()),
        8,
        "whsec_nokey_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgK", "message.delivered", "{}").await;

    let ctx = WorkerContext {
        webhook_sealer: None,
        hmac_webhooks: sms_api::HmacWebhooks::Permitted,
        ..worker_context(db.clone())
    };
    hooks::tick(&ctx, &sys(), "test-worker", &http_client())
        .await
        .expect("tick succeeds without a key");
    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, AttemptState::succeeded);
}

/// Enough due `sealed` attempts to fill more than the claim's over-fetch window
/// (`BUDGET` 20 times `CANDIDATE_OVERFETCH_FACTOR` 3 = 60), all due *earlier* than
/// the `hmac` attempt the tests then seed, so a worker that fetched first and
/// filtered after would find only sealed rows and starve the `hmac` one.
const SEALED_BACKLOG: usize = 70;

/// Seeds [`SEALED_BACKLOG`] due `sealed` attempts to `url`, each due an hour ago.
async fn seed_sealed_backlog(
    db: &Cratestack,
    suffix: &str,
    app_id: &str,
    url: &str,
) -> Vec<schema::WebhookAttempt> {
    let endpoint = seed_endpoint(db, suffix, app_id, url, 8).await;
    let mut attempts = Vec::new();
    for n in 0..SEALED_BACKLOG {
        let attempt = seed_attempt(
            db,
            &endpoint.id,
            &format!("cmsgS{n}"),
            "message.delivered",
            "{}",
        )
        .await;
        let due_earlier = db
            .webhook_attempt()
            .update(attempt.id.clone())
            .set(UpdateWebhookAttemptInput {
                nextAttemptAt: Some(Some(Utc::now() - Duration::hours(1))),
                ..Default::default()
            })
            .if_match(attempt.version)
            .run(&sys())
            .await
            .expect("making the sealed attempt due earlier");
        attempts.push(due_earlier);
    }
    attempts
}

fn keyless(db: &Cratestack) -> WorkerContext {
    WorkerContext {
        webhook_sealer: None,
        hmac_webhooks: sms_api::HmacWebhooks::Permitted,
        ..worker_context(db.clone())
    }
}

/// A worker with no envelope signing key (VS-8R) delivers every `hmac` endpoint,
/// which needs none, and does not claim a `sealed` attempt at all: not counted,
/// not failed, not rescheduled, not touched. The filter is in the claim's own
/// query, so a sealed backlog larger than the over-fetch window (here 70 rows
/// due earlier than the `hmac` one) cannot crowd the `hmac` attempt out of it, and
/// the one `hmac` attempt is delivered by a single tick.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyless_worker_delivers_hmac_past_a_sealed_backlog_and_leaves_the_sealed_rows_alone() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let sealed_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&sealed_server)
        .await;
    let sealed = seed_sealed_backlog(
        &db,
        &suffix,
        &app_id,
        &format!("{}/hook", sealed_server.uri()),
    )
    .await;

    let hmac_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&hmac_server)
        .await;
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hook", hmac_server.uri()),
        8,
        "whsec_nokey_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let hmac_attempt = seed_attempt(&db, &endpoint.id, "cmsgH", "message.delivered", "{}").await;

    hooks::tick(&keyless(&db), &sys(), "test-worker", &http_client())
        .await
        .expect("tick succeeds without a key");

    assert_eq!(
        reread_attempt(&db, &hmac_attempt.id).await.state,
        AttemptState::succeeded,
        "the hmac attempt is delivered within one tick, behind a sealed backlog"
    );
    for before in &sealed {
        let after = reread_attempt(&db, &before.id).await;
        assert_eq!(after.state, AttemptState::pending, "{after:?}");
        assert_eq!(
            after.attempts, 0,
            "a sealed row a keyless worker cannot serve is not counted"
        );
        assert_eq!(after.version, before.version, "and not written at all");
    }
    assert!(sealed_server.received_requests().await.unwrap().is_empty());
}

/// ... and a worker that holds the key then delivers the sealed rows the keyless
/// one left, each once and counted once (the claim takes `BUDGET` rows a tick).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyed_worker_then_delivers_the_sealed_rows_a_keyless_one_left() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(u64::try_from(SEALED_BACKLOG).unwrap())
        .mount(&server)
        .await;
    let sealed =
        seed_sealed_backlog(&db, &suffix, &app_id, &format!("{}/hook", server.uri())).await;

    hooks::tick(&keyless(&db), &sys(), "test-worker", &http_client())
        .await
        .expect("tick succeeds without a key");
    assert!(server.received_requests().await.unwrap().is_empty());

    let keyed = worker_context(db.clone());
    for _ in 0..SEALED_BACKLOG.div_ceil(20) {
        hooks::tick(&keyed, &sys(), "test-worker", &http_client())
            .await
            .expect("tick succeeds with the key");
    }
    for before in &sealed {
        let after = reread_attempt(&db, &before.id).await;
        assert_eq!(after.state, AttemptState::succeeded, "{after:?}");
        assert_eq!(after.attempts, 1);
    }
}

/// The crash-reclaim half of the claim has the same filter: a `delivering` sealed
/// row whose lease expired is not taken by a keyless worker (it keeps its dead
/// owner, its version and its count), while an expired `hmac` one is, and a keyed
/// worker takes the sealed one afterwards without counting a second attempt.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_expired_lease_on_a_sealed_row_is_not_reclaimed_by_a_keyless_worker() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(2)
        .mount(&server)
        .await;
    let url = format!("{}/hook", server.uri());
    let sealed_endpoint = seed_endpoint(&db, &suffix, &app_id, &url, 8).await;
    let hmac_endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &url,
        8,
        "whsec_reclaim_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let mut crashed = Vec::new();
    for (endpoint, aggregate) in [(&sealed_endpoint, "cmsgR1"), (&hmac_endpoint, "cmsgR2")] {
        let attempt = seed_attempt(&db, &endpoint.id, aggregate, "message.delivered", "{}").await;
        crashed.push(
            db.webhook_attempt()
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
                .expect("simulating a crashed worker's expired lease"),
        );
    }
    let (sealed_row, hmac_row) = (&crashed[0], &crashed[1]);

    hooks::tick(&keyless(&db), &sys(), "test-worker", &http_client())
        .await
        .expect("tick succeeds without a key");
    let untouched = reread_attempt(&db, &sealed_row.id).await;
    assert_eq!(untouched.state, AttemptState::delivering, "{untouched:?}");
    assert_eq!(untouched.leaseOwner.as_deref(), Some("crashed-worker"));
    assert_eq!(untouched.version, sealed_row.version);
    assert_eq!(
        reread_attempt(&db, &hmac_row.id).await.state,
        AttemptState::succeeded,
        "an expired hmac lease is reclaimed by a keyless worker"
    );

    hooks::tick(
        &worker_context(db.clone()),
        &sys(),
        "test-worker",
        &http_client(),
    )
    .await
    .expect("tick succeeds with the key");
    let reclaimed = reread_attempt(&db, &sealed_row.id).await;
    assert_eq!(reclaimed.state, AttemptState::succeeded, "{reclaimed:?}");
    assert_eq!(reclaimed.attempts, 1, "resumed, not counted twice");
}

/// A keyless worker must not leave sealed rows pending silently: `SealedBacklogWatch`
/// counts the due sealed attempts it cannot serve and `warn`s, naming
/// `--envelope-signing-key-file` and the count, at most every five minutes. A worker
/// that holds the key has nothing to say, and a count of zero says nothing.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyless_worker_says_how_many_sealed_attempts_it_cannot_serve_every_five_minutes() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let (logs, _guard_logs) = capture_logs();
    let watch = hooks::SealedBacklogWatch::new();
    let start = std::time::Instant::now();

    assert_eq!(
        watch.report(&keyless(&db), &sys(), start).await.unwrap(),
        Some(0),
        "nothing waiting: the query runs and says nothing"
    );
    assert!(!logs.text().contains("WARN"), "{}", logs.text());

    let sealed =
        seed_sealed_backlog(&db, &suffix, &app_id, "https://hooks.example.test/hook").await;
    // Not counted: an `hmac` attempt, which a keyless worker serves.
    let hmac_endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/hmac",
        8,
        "whsec_count_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    seed_attempt(&db, &hmac_endpoint.id, "cmsgC1", "message.delivered", "{}").await;
    // Counted: a sealed attempt a crashed worker left `delivering`, lease expired.
    db.webhook_attempt()
        .update(sealed[0].id.clone())
        .set(UpdateWebhookAttemptInput {
            state: Some(AttemptState::delivering),
            leaseOwner: Some(Some("crashed-worker".to_owned())),
            leaseUntil: Some(Some(Utc::now() - Duration::seconds(5))),
            ..Default::default()
        })
        .if_match(sealed[0].version)
        .run(&sys())
        .await
        .expect("abandoning one sealed attempt");

    let interval = hooks::SEALED_BACKLOG_WARNING_EVERY;
    assert_eq!(
        watch
            .report(&keyless(&db), &sys(), start + interval / 2)
            .await
            .unwrap(),
        None,
        "not due: no query, no line"
    );
    assert!(!logs.text().contains("WARN"), "{}", logs.text());

    let count = watch
        .report(&keyless(&db), &sys(), start + interval)
        .await
        .unwrap();
    assert_eq!(count, Some(SEALED_BACKLOG));
    let text = logs.text();
    assert_eq!(text.matches("WARN").count(), 1, "{text}");
    assert!(text.contains("--envelope-signing-key-file"), "{text}");
    assert!(text.contains(&format!("{SEALED_BACKLOG}")), "{text}");
    assert!(
        text.contains("hmac deliveries are unaffected"),
        "a keyless worker that permits hmac still sends them: {text}"
    );

    // The same worker on a deployment that does not permit `hmac_webhooks` sends
    // no hmac delivery either, and the line must not say it does.
    let neither = with_hmac(keyless(&db), sms_api::HmacWebhooks::Disabled);
    let before = logs.text().len();
    hooks::SealedBacklogWatch::new()
        .report(&neither, &sys(), start)
        .await
        .unwrap();
    let text = logs.text();
    let line = &text[before..];
    assert_eq!(line.matches("WARN").count(), 1, "{text}");
    assert!(line.contains("sends no hmac delivery either"), "{line}");
    assert!(!line.contains("unaffected"), "{line}");

    assert_eq!(
        hooks::SealedBacklogWatch::new()
            .report(&worker_context(db.clone()), &sys(), start + interval)
            .await
            .unwrap(),
        None,
        "a worker with the key serves them: nothing to report"
    );
}

/// What `tracing` is handed while the returned guard lives (thread-local: this
/// file's tests run on `#[tokio::test]`'s current-thread runtime).
#[derive(Clone, Default)]
struct Logs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl Logs {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Logs;

    fn make_writer(&'a self) -> Logs {
        self.clone()
    }
}

fn capture_logs() -> (Logs, tracing::subscriber::DefaultGuard) {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_ansi(false)
        .finish();
    (logs, tracing::subscriber::set_default(subscriber))
}

/// Happy path for an `hmac` endpoint (#41, unchanged): claim, sign, POST,
/// `-> succeeded`. The signature is checked with `sms_webhook::verify` — what a
/// receiver calls — over the exact bytes `wiremock` received, the body is the
/// original four-field envelope (no `eventId`, `appId` or `endpointId`), the
/// event id travels in the signed header, and nothing a sealed delivery carries
/// is present (no COSE content type, no `Cratestack-*` header).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_successful_hmac_delivery_is_signed_and_keeps_the_four_field_body() {
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
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hook", server.uri()),
        8,
        "whsec_hmac_delivery_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
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

    hooks::tick(
        &worker_context(db.clone()),
        &sys(),
        "test-worker",
        &http_client(),
    )
    .await
    .expect("tick succeeds");

    let reread = reread_attempt(&db, &attempt.id).await;
    assert_eq!(reread.state, AttemptState::succeeded);
    assert_eq!(reread.attempts, 1);
    assert_eq!(reread.lastStatusCode, Some(200));
    assert!(reread.deliveredAt.is_some());

    let requests = server.received_requests().await.expect("recording is on");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let header = |name: &str| {
        request
            .headers
            .get(name)
            .unwrap_or_else(|| panic!("header {name} present"))
            .to_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(header("x-sms-event"), "message.delivered");
    assert_eq!(header("x-sms-event-id"), attempt.sourceEventId.to_string());
    assert_eq!(header("content-type"), "application/json");
    let timestamp: i64 = header("x-sms-timestamp")
        .parse()
        .expect("a decimal integer");
    assert!(
        sms_webhook::verify(
            &["whsec_hmac_delivery_0123456789abcdef0123456789abcdef"],
            timestamp,
            &header("x-sms-event-id"),
            &request.body,
            &header("x-sms-signature"),
        ),
        "the signature over the exact bytes received must verify against the endpoint's secret"
    );
    assert!(
        !sms_webhook::verify(
            &["some-other-secret"],
            timestamp,
            &header("x-sms-event-id"),
            &request.body,
            &header("x-sms-signature"),
        ),
        "and against nothing else"
    );
    for sealed_only in ["cratestack-contract", "cratestack-payload-type"] {
        assert!(request.headers.get(sealed_only).is_none(), "{sealed_only}");
    }

    let envelope: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    let mut keys: Vec<&str> = envelope
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["data", "id", "occurredAt", "type"]);
    assert_eq!(envelope["id"], attempt.id);
    assert_eq!(envelope["type"], "message.delivered");
    assert_eq!(envelope["data"]["messageId"], "cmsg000000000000000000");

    let endpoint_after = reread_endpoint(&db, &endpoint.id).await;
    assert_eq!(endpoint_after.consecutiveFailures, 0);
    assert!(endpoint_after.circuitOpenUntil.is_none());
}

/// Rotation overlap (#41, §4.4): while `prevSecret` is set, the header carries a
/// `v1=` for the current secret first and one for the previous secret second,
/// so a receiver that has learnt only one of them verifies.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_hmac_delivery_in_a_rotation_window_verifies_under_either_secret() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hook", server.uri()),
        8,
        "whsec_new_generation_0123456789abcdef0123456789abcdef",
        Some("whsec_old_generation_0123456789abcdef0123456789abcdef"),
    )
    .await;
    seed_attempt(&db, &endpoint.id, "cmsgR", "message.delivered", "{}").await;

    hooks::tick(
        &worker_context(db.clone()),
        &sys(),
        "test-worker",
        &http_client(),
    )
    .await
    .expect("tick succeeds");

    let requests = server.received_requests().await.unwrap();
    let request = &requests[0];
    let signature = request.headers["x-sms-signature"].to_str().unwrap();
    let event_id = request.headers["x-sms-event-id"].to_str().unwrap();
    let timestamp: i64 = request.headers["x-sms-timestamp"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let values: Vec<&str> = signature.split(',').collect();
    assert_eq!(values.len(), 2, "{signature}");
    for (generation, secret) in [
        (
            "the new generation",
            "whsec_new_generation_0123456789abcdef0123456789abcdef",
        ),
        (
            "the old generation",
            "whsec_old_generation_0123456789abcdef0123456789abcdef",
        ),
    ] {
        assert!(
            sms_webhook::verify(&[secret], timestamp, event_id, &request.body, signature),
            "{generation}'s secret must verify the delivery"
        );
    }
    // Current first: the first value is the new secret's alone.
    assert!(sms_webhook::verify(
        &["whsec_new_generation_0123456789abcdef0123456789abcdef"],
        timestamp,
        event_id,
        &request.body,
        values[0],
    ));
    assert!(sms_webhook::verify(
        &["whsec_old_generation_0123456789abcdef0123456789abcdef"],
        timestamp,
        event_id,
        &request.body,
        values[1],
    ));
}

/// The status handling is one path for both modes: a `410 Gone` kills the
/// attempt and deactivates an `hmac` endpoint exactly as it does a sealed one.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_410_deactivates_an_hmac_endpoint_and_a_500_backs_it_off() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let gone = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(410))
        .mount(&gone)
        .await;
    let failing = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&failing)
        .await;
    let gone_endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hook", gone.uri()),
        8,
        "whsec_a_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let failing_endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hook", failing.uri()),
        8,
        "whsec_b_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let gone_attempt =
        seed_attempt(&db, &gone_endpoint.id, "cmsgG", "message.delivered", "{}").await;
    let failing_attempt = seed_attempt(
        &db,
        &failing_endpoint.id,
        "cmsgF",
        "message.delivered",
        "{}",
    )
    .await;

    hooks::tick(
        &worker_context(db.clone()),
        &sys(),
        "test-worker",
        &http_client(),
    )
    .await
    .expect("tick succeeds");

    assert_eq!(
        reread_attempt(&db, &gone_attempt.id).await.state,
        AttemptState::dead
    );
    assert!(!reread_endpoint(&db, &gone_endpoint.id).await.active);

    let failed = reread_attempt(&db, &failing_attempt.id).await;
    assert_eq!(failed.state, AttemptState::failed);
    assert_eq!(failed.lastStatusCode, Some(500));
    assert_eq!(
        reread_endpoint(&db, &failing_endpoint.id)
            .await
            .consecutiveFailures,
        1
    );
}

/// An `hmac` delivery follows redirects as it always did (R5: the scheme is
/// unchanged for the apps that rely on it, and a receiver behind a `308` keeps
/// working): the redirected request carries the same signed body and the same
/// signature headers, and the answer of the final host decides the attempt. A
/// `sealed` delivery follows none (`a_redirect_is_not_followed`).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_hmac_redirect_is_followed_and_resends_the_signed_body() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(308).insert_header("location", "/moved"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/moved"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let secret = "whsec_redirected_0123456789abcdef0123456789";
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hook", server.uri()),
        8,
        secret,
        None,
    )
    .await;
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
    assert_eq!(reread.state, AttemptState::succeeded);
    assert_eq!(reread.lastStatusCode, Some(200));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "the original and the redirected one");
    let (first, second) = (&requests[0], &requests[1]);
    assert_eq!(first.url.path(), "/hook");
    assert_eq!(second.url.path(), "/moved");
    assert_eq!(first.body, second.body, "a 308 re-sends the same body");
    let signature = second.headers["x-sms-signature"].to_str().unwrap();
    let event_id = second.headers["x-sms-event-id"].to_str().unwrap();
    let timestamp: i64 = second.headers["x-sms-timestamp"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        sms_webhook::verify(&[secret], timestamp, event_id, &second.body, signature),
        "the redirected request still verifies"
    );
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
    let receiver = receiver_for(endpoint.audience.as_deref().expect("a sealed endpoint"));
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

/// Due `sealed` attempts the next test bulk-inserts: the size of the backlog the
/// finding measured (200 000). Smaller tables let the planner prefer a sequential
/// scan to walking `webhook_due_idx`, and the old claim would then pass.
const BULK_BACKLOG: u32 = 200_000;

/// How many entries scans have read from `index` so far, per the server's
/// statistics. A backend hands its counters to the shared view when it goes idle
/// for a while or exits, so a reading is only worth anything once every backend
/// that scanned has exited: the caller closes its pool first, and this waits for
/// the exits to land.
async fn index_entries_read(index: &str) -> i64 {
    index_entries_read_at(&sms_test_support::database_url().await, index).await
}

/// [`index_entries_read`] in the database at `database_url`.
async fn index_entries_read_at(database_url: &str, index: &str) -> i64 {
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    psql_at(
        database_url,
        &format!(
            "SELECT coalesce(max(idx_tup_read), 0) FROM pg_stat_user_indexes \
             WHERE indexrelname = '{index}'"
        ),
    )
    .parse()
    .expect("a count")
}

/// A database of its own for a test that reads the server's index counters: a copy
/// of the harness's migrated template (`vsms_template`, `sms_test_support`'s), so no
/// other test's connections, deleted rows or late-flushed counters are in it. Those
/// were what the first version of these tests read: an idle backend hands its
/// counters to the shared view on a timer of up to ten seconds, and the planner
/// reads the two ends of an index, dead entries included, to estimate a range, so
/// in the one database a claim of 20 rows was seen to read 90 and 300 entries when
/// the test before it had scanned a few hundred.
struct Scratch {
    admin: String,
    url: String,
    name: String,
}

impl Scratch {
    async fn create(tag: &str) -> Self {
        let base = sms_test_support::database_url().await;
        let (head, _) = base.rsplit_once('/').expect("a postgres URL has a path");
        let name = format!("vsms_hooks_scratch_{tag}");
        let admin = format!("{head}/postgres");
        psql_at(
            &admin,
            &format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
        );
        psql_at(
            &admin,
            &format!("CREATE DATABASE {name} TEMPLATE vsms_template"),
        );
        Self {
            url: format!("{head}/{name}"),
            admin,
            name,
        }
    }

    async fn cratestack(&self) -> Cratestack {
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&self.url)
            .await
            .expect("connecting to the scratch database");
        Cratestack::builder(pool).build()
    }

    fn psql(&self, sql: &str) -> String {
        psql_at(&self.url, sql)
    }

    fn drop_database(self) {
        psql_at(
            &self.admin,
            &format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name),
        );
    }
}

/// How many scans have used `index` so far; same reading rules as
/// [`index_entries_read`].
async fn index_scans(index: &str) -> i64 {
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    psql(&format!(
        "SELECT coalesce(max(idx_scan), 0) FROM pg_stat_user_indexes \
         WHERE indexrelname = '{index}'"
    ))
    .await
    .parse()
    .expect("a count")
}

/// M1: a keyless worker's claim costs what the `hmac` work costs, not what the
/// sealed backlog does. Two hundred thousand due sealed attempts sit ahead of one `hmac`
/// attempt in time. The claim used to filter by a subquery on the endpoint's mode,
/// which Postgres cannot push into `webhook_due_idx`: it walked that index in time
/// order and looked every row's endpoint up until it had enough `hmac` rows, so it
/// read every sealed entry on each poll. It now reads the `hmac` endpoints first
/// and asks `webhook_endpoint_due_idx` for each one's due attempts, and so reads no
/// entry of `webhook_due_idx` at all. The counters are the server's own
/// (`pg_stat_user_indexes`), read with `psql` after the claim's connections have
/// closed.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyless_claim_never_reads_the_sealed_backlogs_index_entries() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let sealed = seed_endpoint(&db, &suffix, &app_id, "https://hooks.example.test/hook", 8).await;
    psql(&format!(
        "INSERT INTO webhook_attempts \
           (id, endpoint_id, source_event_id, aggregate_id, event_type, payload, next_attempt_at) \
         SELECT 'bulk-' || n, '{}', gen_random_uuid(), 'bulk-' || n, 'message.delivered', '{{}}', \
                now() - interval '1 hour' \
         FROM generate_series(1, {BULK_BACKLOG}) AS n",
        sealed.id
    ))
    .await;
    psql("ANALYZE webhook_attempts").await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let hmac = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hook", server.uri()),
        8,
        "whsec_bulk_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let attempt = seed_attempt(&db, &hmac.id, "cmsgB", "message.delivered", "{}").await;

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&sms_test_support::database_url().await)
        .await
        .expect("connecting to Postgres");
    let ctx = keyless(&Cratestack::builder(pool.clone()).build());
    let due_before = index_entries_read("webhook_due_idx").await;
    let endpoint_due_before = index_scans("webhook_endpoint_due_idx").await;

    hooks::tick(&ctx, &sys(), "test-worker", &http_client())
        .await
        .expect("tick succeeds without a key");
    pool.close().await;

    assert_eq!(
        reread_attempt(&db, &attempt.id).await.state,
        AttemptState::succeeded,
        "the hmac attempt is delivered behind a {BULK_BACKLOG}-row sealed backlog"
    );
    let due_read = index_entries_read("webhook_due_idx").await - due_before;
    assert!(
        due_read < i64::from(BULK_BACKLOG) / 10,
        "the claim read {due_read} entries of webhook_due_idx, the sealed backlog's index"
    );
    assert!(
        index_scans("webhook_endpoint_due_idx").await > endpoint_due_before,
        "and it used the per-endpoint due list instead"
    );
    psql("DELETE FROM webhook_attempts WHERE aggregate_id LIKE 'bulk-%'").await;
}

/// The keyless claim asks for each `hmac` endpoint's due attempts separately and
/// merges them, and the merge must keep what a single query gave: the oldest due
/// attempts first, whichever endpoint holds them. Two endpoints, three attempts
/// each, one endpoint's all older than the other's; a claim of the two oldest takes
/// both of the older endpoint's. Run with each endpoint older in turn, because the
/// order the endpoints are read in is the database's to choose, and a merge that
/// kept it would pass whenever the older endpoint happened to come first.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyless_claim_takes_the_oldest_due_attempts_across_hmac_endpoints() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    // One `sealed` endpoint, with no attempts, makes this the fan-out path whatever
    // the other tests of this binary left in the table: without any `sealed`
    // endpoint the claim is `both_modes_candidates`, which is ordered by the index.
    seed_endpoint(
        &db,
        &suffix,
        &app_id,
        "https://hooks.example.test/sealed",
        8,
    )
    .await;
    let mut endpoints = Vec::new();
    for n in 0..2 {
        endpoints.push(
            seed_hmac_endpoint(
                &db,
                &app_id,
                &format!("https://hooks.example.test/order-{n}"),
                8,
                "whsec_order_0123456789abcdef0123456789abcdef",
                None,
            )
            .await,
        );
    }

    for (round, (older, newer)) in [(0, 1), (1, 0)].into_iter().enumerate() {
        clear_claimable_backlog(&db).await;
        let mut oldest = Vec::new();
        for (endpoint, minutes_ago) in [(older, [60, 50, 40]), (newer, [30, 20, 10])] {
            for (n, minutes) in minutes_ago.into_iter().enumerate() {
                let attempt = seed_attempt(
                    &db,
                    &endpoints[endpoint].id,
                    &format!("cmsgO{round}{endpoint}{n}"),
                    "message.delivered",
                    "{}",
                )
                .await;
                psql(&format!(
                    "UPDATE webhook_attempts SET next_attempt_at = now() - interval '{minutes} minutes' \
                     WHERE id = '{}'",
                    attempt.id
                ))
                .await;
                if endpoint == older && n < 2 {
                    oldest.push(attempt.id);
                }
            }
        }

        let mut claimed: Vec<String> =
            claim_webhook_attempts(&db, &sys(), "keyless-worker", 2, ServedModes::HmacOnly)
                .await
                .expect("the keyless claim")
                .into_iter()
                .map(|attempt| attempt.id)
                .collect();
        claimed.sort();
        oldest.sort();
        assert_eq!(
            claimed, oldest,
            "round {round}: the two oldest due attempts, both from endpoint {older}"
        );
    }
}

/// `hmac` endpoints the next tests backlog; with `ALL_HMAC_DUE_EACH` due attempts
/// apiece, interleaved in time so that every endpoint holds some of the oldest.
const ALL_HMAC_ENDPOINTS: u32 = 40;
const ALL_HMAC_DUE_EACH: u32 = 1_000;

/// Due `sealed` attempts, older than every `hmac` one, in the fan-out's backlog.
const SEALED_BEHIND_THE_HMAC: u32 = 40_000;

/// The claim's `budget` in the next tests.
const CLAIM_BUDGET: i64 = 20;

/// The most entries of `webhook_due_idx` a single time-ordered read of
/// [`CLAIM_BUDGET`] rows may be seen to read: ten times the rows it asks for, against
/// the 800 to 31 000 the per-endpoint reads of 40 backlogged endpoints take. Not
/// tighter, because the server's counter (`idx_tup_read`) also counts the entries the
/// *planner* reads at the two ends of the index to estimate the range
/// (`get_actual_variable_range`), and the first claim on a connection plans before it
/// executes.
const SINGLE_QUERY_ENTRIES: i64 = 10 * CLAIM_BUDGET;

/// What a claim of [`CLAIM_BUDGET`] took, and what it cost in index entries.
struct BackloggedClaim {
    /// The `aggregateId`s it claimed, sorted.
    claimed: Vec<String>,
    /// Entries it read of `webhook_due_idx`.
    due_idx: i64,
    /// Entries it read of `webhook_endpoint_due_idx`.
    endpoint_due_idx: i64,
}

/// The `sealed` side of the table a backlogged claim runs over.
#[derive(Clone, Copy)]
enum SealedEndpoint {
    /// None: every endpoint is `hmac`.
    Absent,
    /// One, active and holding nothing.
    Empty,
    /// One, holding [`SEALED_BEHIND_THE_HMAC`] due attempts older than every `hmac` one.
    WithBacklog,
}

/// Backlogs [`ALL_HMAC_ENDPOINTS`] `hmac` endpoints with [`ALL_HMAC_DUE_EACH`] due
/// attempts each, and runs one claim by a worker serving `served` over them.
async fn claim_behind_an_hmac_backlog(
    served: ServedModes,
    sealed_endpoint: SealedEndpoint,
) -> BackloggedClaim {
    let scratch = Scratch::create("backlog").await;
    let db = scratch.cratestack().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let sealed = match sealed_endpoint {
        SealedEndpoint::Absent => None,
        SealedEndpoint::Empty | SealedEndpoint::WithBacklog => Some(
            seed_endpoint(
                &db,
                &suffix,
                &app_id,
                "https://hooks.example.test/sealed",
                8,
            )
            .await,
        ),
    };
    for n in 0..ALL_HMAC_ENDPOINTS {
        seed_hmac_endpoint(
            &db,
            &app_id,
            &format!("https://hooks.example.test/all-hmac-{n}"),
            8,
            "whsec_allhmac_0123456789abcdef0123456789abcdef",
            None,
        )
        .await;
    }
    scratch
        .psql(&format!(
        "INSERT INTO webhook_attempts \
           (id, endpoint_id, source_event_id, aggregate_id, event_type, payload, next_attempt_at) \
         SELECT 'bulk-' || e.rn || '-' || k, e.id, gen_random_uuid(), 'bulk-' || e.rn || '-' || k, \
                'message.delivered', '{{}}', \
                now() - interval '1 hour' + (k * {ALL_HMAC_ENDPOINTS} + e.rn) * interval '1 millisecond' \
         FROM (SELECT id, row_number() OVER (ORDER BY id) AS rn \
                 FROM webhook_endpoints WHERE delivery_mode = 'hmac') e, \
              generate_series(1, {ALL_HMAC_DUE_EACH}) AS k \
         ORDER BY k, e.rn"
    ));
    if let (SealedEndpoint::WithBacklog, Some(sealed)) = (sealed_endpoint, sealed) {
        // A due sealed backlog older than every `hmac` attempt, as a deployment that
        // needs the fan-out has: it is what makes the planner prefer the endpoint's
        // own index to walking `webhook_due_idx` in time order and filtering. Without
        // it the 40 endpoints are each 2.5% of the table, and the planner, which
        // estimates a walk of about 800 entries for 20 rows, may take that walk.
        scratch
        .psql(&format!(
            "INSERT INTO webhook_attempts \
               (id, endpoint_id, source_event_id, aggregate_id, event_type, payload, next_attempt_at) \
             SELECT 'bulk-sealed-' || k, '{}', gen_random_uuid(), 'bulk-sealed-' || k, \
                    'message.delivered', '{{}}', \
                    now() - interval '2 hours' + k * interval '1 millisecond' \
             FROM generate_series(1, {SEALED_BEHIND_THE_HMAC}) AS k",
            sealed.id
        ));
    }
    scratch.psql("VACUUM ANALYZE webhook_attempts");

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&scratch.url)
        .await
        .expect("connecting to the scratch database");
    let claim_db = Cratestack::builder(pool.clone()).build();
    let due_before = index_entries_read_at(&scratch.url, "webhook_due_idx").await;
    let endpoint_due_before = index_entries_read_at(&scratch.url, "webhook_endpoint_due_idx").await;

    let claimed = claim_webhook_attempts(&claim_db, &sys(), "worker", CLAIM_BUDGET, served)
        .await
        .expect("the claim");
    pool.close().await;

    let mut ids: Vec<String> = claimed
        .into_iter()
        .map(|attempt| attempt.aggregateId)
        .collect();
    ids.sort();
    let result = BackloggedClaim {
        claimed: ids,
        due_idx: index_entries_read_at(&scratch.url, "webhook_due_idx").await - due_before,
        endpoint_due_idx: index_entries_read_at(&scratch.url, "webhook_endpoint_due_idx").await
            - endpoint_due_before,
    };
    drop(db);
    scratch.drop_database();
    result
}

/// Times rise with `k` first and the endpoint's rank second, so the oldest
/// [`CLAIM_BUDGET`] attempts are the first round of the first endpoints.
fn the_oldest_claimable() -> Vec<String> {
    let mut oldest: Vec<String> = (1..=CLAIM_BUDGET)
        .map(|rn| format!("bulk-{rn}-1"))
        .collect();
    oldest.sort();
    oldest
}

/// Review round 6, M-1, and round 7: when every endpoint is one the worker can claim
/// from, a keyless claim is the keyed claim's own time-ordered query, whatever the
/// number of backlogged `hmac` endpoints. The per-endpoint fan-out is what a
/// deployment that must leave an endpoint out needs; applied to a healthy all-`hmac`
/// one it read up to `endpoints x budget` full rows a poll for the answer one read
/// gives. Here 40 endpoints hold 1 000 due attempts each, and the claim of 20 must
/// read about 20 entries of `webhook_due_idx` and none of
/// `webhook_endpoint_due_idx` (the fan-out reads 40 x 20 = 800 of it), and still take
/// the 20 oldest, whichever endpoint holds them. The counters are the server's own
/// (`pg_stat_user_indexes`), so a claim that took the other path cannot pass.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyless_claim_over_healthy_hmac_endpoints_reads_about_budget_entries_of_the_time_ordered_index_only()
 {
    let _guard = TEST_MUTEX.lock().await;
    let claim = claim_behind_an_hmac_backlog(ServedModes::HmacOnly, SealedEndpoint::Absent).await;
    assert_eq!(
        claim.claimed,
        the_oldest_claimable(),
        "the 20 oldest due attempts of any endpoint"
    );
    assert_eq!(
        claim.endpoint_due_idx, 0,
        "a healthy all-hmac table is not the per-endpoint fan-out"
    );
    assert!(
        claim.due_idx > 0 && claim.due_idx <= SINGLE_QUERY_ENTRIES,
        "the claim read {} entries of webhook_due_idx for {ALL_HMAC_ENDPOINTS} backlogged hmac \
         endpoints; one read of the oldest {CLAIM_BUDGET} is all a healthy deployment needs",
        claim.due_idx
    );
}

/// ... and so does a worker that holds the key, over healthy endpoints of both modes
/// (round 7, case (e)): a `sealed` endpoint that is active and closed is no reason to
/// leave the time-ordered query, which is also what it was before the claim learned
/// to read the endpoints. The `sealed` endpoint here holds nothing, so the 20 oldest
/// are the `hmac` ones.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyed_claim_over_healthy_endpoints_of_both_modes_reads_about_budget_entries_of_the_time_ordered_index_only()
 {
    let _guard = TEST_MUTEX.lock().await;
    let claim = claim_behind_an_hmac_backlog(ServedModes::Both, SealedEndpoint::Empty).await;
    assert_eq!(claim.claimed, the_oldest_claimable());
    assert_eq!(claim.endpoint_due_idx, 0);
    assert!(
        claim.due_idx > 0 && claim.due_idx <= SINGLE_QUERY_ENTRIES,
        "{} entries of webhook_due_idx",
        claim.due_idx
    );
}

/// ... and when an endpoint must be left out (here a `sealed` one, for a keyless
/// worker) the fan-out asks each `hmac` endpoint for `budget` rows: 40 x 20 = 800
/// entries of `webhook_endpoint_due_idx`, and none of `webhook_due_idx`, the
/// time-ordered index.
///
/// The 40 000 older sealed attempts are there on purpose, not for volume. Whether the
/// planner answers an `endpoint_id = $1` query from `webhook_endpoint_due_idx` or by
/// walking `webhook_due_idx` in time order and filtering is its choice by estimated
/// cost, and with 40 endpoints each holding 2.5% of the table and nothing else it
/// estimates a walk of about 800 entries for 20 rows and may take it (`claim.md`
/// has the measurement). The sealed backlog stands in front of every `hmac` attempt
/// in time, which makes that walk the dearer plan by every estimate, so the plan this
/// test counts entries of is the one the fan-out is for. The assertion stays as exact
/// as the plan allows: the entry count is only bounded when the endpoint index is the
/// one used, and `endpoint_due_idx > 0` says it was.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyless_claim_beside_a_sealed_endpoint_asks_each_hmac_endpoint_for_budget_rows() {
    let _guard = TEST_MUTEX.lock().await;
    let claim =
        claim_behind_an_hmac_backlog(ServedModes::HmacOnly, SealedEndpoint::WithBacklog).await;
    assert_eq!(
        claim.claimed,
        the_oldest_claimable(),
        "the batch is the 20 oldest, whichever endpoint holds them"
    );
    assert!(
        claim.endpoint_due_idx > 0 && claim.due_idx < CLAIM_BUDGET,
        "with a sealed endpoint the keyless claim is the per-endpoint fan-out: {} entries of \
         webhook_endpoint_due_idx, {} of webhook_due_idx",
        claim.endpoint_due_idx,
        claim.due_idx
    );
    // Half as much again as 40 x 20, for the planner's own probes of the index; the
    // fan-out that asked each endpoint for three times the budget read 2 400.
    let ceiling = i64::from(ALL_HMAC_ENDPOINTS) * CLAIM_BUDGET * 3 / 2;
    assert!(
        claim.endpoint_due_idx <= ceiling,
        "{} entries read for {ALL_HMAC_ENDPOINTS} endpoints asked for {CLAIM_BUDGET} rows each \
         (at most {ceiling}, where asking for three times that reads 2 400)",
        claim.endpoint_due_idx
    );
}

/// ... and "every endpoint is one this worker can claim from" is every endpoint in
/// **any state**. A deactivated one can hold many due attempts: the keyless worker
/// will never take them, but the time-ordered query would fetch them (they are the
/// oldest) and drop them only after, leaving the `hmac` attempt behind them out of
/// the window. Here 70 due attempts of a deactivated `sealed` endpoint are older than
/// the one `hmac` attempt, which is more than the 60 rows an over-fetching window
/// held, and more than twice a claim's `budget`.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_deactivated_sealed_endpoint_still_keeps_a_keyless_claim_off_the_keyed_query() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    psql("DELETE FROM webhook_endpoints").await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let sealed = seed_sealed_backlog(&db, &suffix, &app_id, "https://hooks.example.test/off").await;
    psql(&format!(
        "UPDATE webhook_endpoints SET active = false WHERE id = '{}'",
        sealed[0].endpointId
    ))
    .await;
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/hmac",
        8,
        "whsec_inactive_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgI", "message.delivered", "{}").await;

    let claimed = claim_webhook_attempts(
        &db,
        &sys(),
        "keyless-worker",
        CLAIM_BUDGET,
        ServedModes::HmacOnly,
    )
    .await
    .expect("the keyless claim");
    assert_eq!(
        claimed.iter().map(|a| a.id.clone()).collect::<Vec<_>>(),
        vec![attempt.id],
        "the hmac attempt, past {SEALED_BACKLOG} due attempts of a deactivated sealed endpoint"
    );
}

/// ... and the same with the deactivated endpoint `hmac`, the table all-`hmac`: the
/// shape that brought the starvation back for keyless workers, since it is the one
/// a keyless worker could take the keyed query for.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_deactivated_hmac_endpoint_still_keeps_a_keyless_claim_off_the_keyed_query() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    psql("DELETE FROM webhook_endpoints").await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let off = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/off",
        8,
        "whsec_off_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    bulk_due(&off.id, "off", u32::try_from(SEALED_BACKLOG).unwrap(), 60).await;
    psql(&format!(
        "UPDATE webhook_endpoints SET active = false WHERE id = '{}'",
        off.id
    ))
    .await;
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/hmac",
        8,
        "whsec_on_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgI2", "message.delivered", "{}").await;

    let claimed = claim_webhook_attempts(
        &db,
        &sys(),
        "keyless-worker",
        CLAIM_BUDGET,
        ServedModes::HmacOnly,
    )
    .await
    .expect("the keyless claim");
    assert_eq!(
        claimed.iter().map(|a| a.id.clone()).collect::<Vec<_>>(),
        vec![attempt.id],
        "the healthy attempt, past {SEALED_BACKLOG} due attempts of a deactivated hmac endpoint"
    );
    psql("DELETE FROM webhook_attempts WHERE aggregate_id LIKE 'bulk-%'").await;
}

/// Inserts `count` due `pending` attempts of `endpoint_id` with `psql`, the oldest
/// due `minutes_ago` minutes ago and a millisecond apart, their `aggregateId` (and
/// `id`) `bulk-{tag}-{k}`; [`clear_claimable_backlog`] sweeps `bulk-%`.
async fn bulk_due(endpoint_id: &str, tag: &str, count: u32, minutes_ago: u32) {
    psql(&format!(
        "INSERT INTO webhook_attempts \
           (id, endpoint_id, source_event_id, aggregate_id, event_type, payload, next_attempt_at) \
         SELECT 'bulk-{tag}-' || k, '{endpoint_id}', gen_random_uuid(), 'bulk-{tag}-' || k, \
                'message.delivered', '{{}}', \
                now() - interval '{minutes_ago} minutes' + k * interval '1 millisecond' \
         FROM generate_series(1, {count}) AS k"
    ))
    .await;
}

/// Due attempts an unhealthy endpoint holds in the next tests: more than a claim's
/// [`CLAIM_BUDGET`] twice over, and more than the 60 rows the over-fetching claim
/// asked for, all older than the healthy endpoints' attempts.
const UNHEALTHY_BACKLOG: u32 = 70;

/// How an endpoint stops being claimable without anything being deleted: an
/// operator's deactivation (a 410 does it too) or an open circuit.
#[derive(Clone, Copy, Debug)]
enum Unhealthy {
    Deactivated,
    CircuitOpen,
}

impl Unhealthy {
    fn assignment(self) -> &'static str {
        match self {
            Self::Deactivated => "active = false",
            Self::CircuitOpen => "circuit_open_until = now() + interval '1 hour'",
        }
    }
}

/// Review round 7, finding 1: an endpoint a claim must not take from, holding
/// [`UNHEALTHY_BACKLOG`] due attempts older than everyone else's, must not be the
/// whole of the claim's window. Nothing purges attempts, so the endpoint keeps them
/// for as long as it stays deactivated or open, and a claim that fetched the oldest
/// rows of any endpoint and dropped the unhealthy ones after returned `[]` on every
/// tick: delivery stopped for every app. Two healthy `hmac` endpoints hold 5 newer
/// due attempts each (and, when `healthy_sealed`, a healthy `sealed` endpoint 3), and
/// a claim by a worker serving `served` must take exactly those, in one call, and
/// leave the unhealthy endpoint's 70 untouched.
async fn claim_past_an_unhealthy_backlog(
    served: ServedModes,
    unhealthy_mode: WebhookDeliveryMode,
    how: Unhealthy,
    healthy_sealed: bool,
) {
    let db = isolated_db().await;
    psql("DELETE FROM webhook_endpoints").await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let secret = "whsec_unhealthy_0123456789abcdef0123456789abcdef";
    let bad = match unhealthy_mode {
        WebhookDeliveryMode::sealed => {
            seed_endpoint(&db, &suffix, &app_id, "https://hooks.example.test/bad", 8).await
        }
        WebhookDeliveryMode::hmac => {
            seed_hmac_endpoint(
                &db,
                &app_id,
                "https://hooks.example.test/bad",
                8,
                secret,
                None,
            )
            .await
        }
    };
    let h1 = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/h1",
        8,
        secret,
        None,
    )
    .await;
    let h2 = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/h2",
        8,
        secret,
        None,
    )
    .await;
    bulk_due(&bad.id, "bad", UNHEALTHY_BACKLOG, 120).await;
    bulk_due(&h1.id, "h1", 5, 30).await;
    bulk_due(&h2.id, "h2", 5, 20).await;
    let mut expected: Vec<String> = (1..=5)
        .flat_map(|k| [format!("bulk-h1-{k}"), format!("bulk-h2-{k}")])
        .collect();
    if healthy_sealed {
        let sealed = seed_endpoint(
            &db,
            &format!("{suffix}-ok"),
            &app_id,
            "https://hooks.example.test/ok",
            8,
        )
        .await;
        bulk_due(&sealed.id, "s", 3, 10).await;
        if served == ServedModes::Both {
            expected.extend((1..=3).map(|k| format!("bulk-s-{k}")));
        }
    }
    expected.sort();
    psql(&format!(
        "UPDATE webhook_endpoints SET {} WHERE id = '{}'",
        how.assignment(),
        bad.id
    ))
    .await;

    let claimed = match served {
        ServedModes::Both => {
            claim_batch::<WebhookAttempt>(&db, &sys(), "keyed-worker", CLAIM_BUDGET).await
        }
        ServedModes::HmacOnly | ServedModes::SealedOnly | ServedModes::Neither => {
            claim_webhook_attempts(&db, &sys(), "keyless-worker", CLAIM_BUDGET, served).await
        }
    }
    .expect("the claim");
    let mut got: Vec<String> = claimed.into_iter().map(|a| a.aggregateId).collect();
    got.sort();
    assert_eq!(
        got, expected,
        "{served:?}, a {unhealthy_mode:?} endpoint {how:?} holding {UNHEALTHY_BACKLOG} older attempts: \
         the healthy endpoints' attempts, within one claim"
    );
    assert_eq!(
        psql(
            "SELECT count(*) FROM webhook_attempts \
             WHERE aggregate_id LIKE 'bulk-bad-%' AND state = 'pending' AND attempts = 0"
        )
        .await,
        UNHEALTHY_BACKLOG.to_string(),
        "and the unhealthy endpoint's attempts are not touched"
    );
    psql("DELETE FROM webhook_endpoints").await;
}

/// Case (i): a keyless worker, a table of `hmac` endpoints only, one of them
/// deactivated after a 410. Which is the table the all-`hmac` fallback of round 5
/// sent to the time-ordered query, whose window was that endpoint's rows alone.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyless_claim_over_all_hmac_endpoints_is_not_starved_by_a_deactivated_one() {
    let _guard = TEST_MUTEX.lock().await;
    claim_past_an_unhealthy_backlog(
        ServedModes::HmacOnly,
        WebhookDeliveryMode::hmac,
        Unhealthy::Deactivated,
        false,
    )
    .await;
}

/// Case (ii): the same with the circuit open, which ends by itself after fifteen
/// minutes and so only starves for that long, every fifteen minutes an endpoint
/// keeps failing.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyless_claim_over_all_hmac_endpoints_is_not_starved_by_a_circuit_open_one() {
    let _guard = TEST_MUTEX.lock().await;
    claim_past_an_unhealthy_backlog(
        ServedModes::HmacOnly,
        WebhookDeliveryMode::hmac,
        Unhealthy::CircuitOpen,
        false,
    )
    .await;
}

/// Case (iii): the keyed worker had this from the start. Every claim by a worker
/// that holds the key took the oldest rows of any endpoint, so a deactivated one with
/// a backlog stopped delivery for every app. (Fails on `47b8f1a`'s claim.)
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyed_claim_is_not_starved_by_a_deactivated_endpoint() {
    let _guard = TEST_MUTEX.lock().await;
    claim_past_an_unhealthy_backlog(
        ServedModes::Both,
        WebhookDeliveryMode::sealed,
        Unhealthy::Deactivated,
        true,
    )
    .await;
}

/// ... and by an endpoint whose circuit is open.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyed_claim_is_not_starved_by_a_circuit_open_endpoint() {
    let _guard = TEST_MUTEX.lock().await;
    claim_past_an_unhealthy_backlog(
        ServedModes::Both,
        WebhookDeliveryMode::hmac,
        Unhealthy::CircuitOpen,
        true,
    )
    .await;
}

/// ... and a keyless worker beside a healthy `sealed` endpoint, which takes the
/// per-endpoint path anyway, leaves the deactivated `hmac` endpoint's backlog out of
/// it too: the endpoints it asks are the claimable `hmac` ones, not every `hmac` one.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_keyless_claim_beside_a_sealed_endpoint_is_not_starved_by_a_deactivated_hmac_one() {
    let _guard = TEST_MUTEX.lock().await;
    claim_past_an_unhealthy_backlog(
        ServedModes::HmacOnly,
        WebhookDeliveryMode::hmac,
        Unhealthy::Deactivated,
        true,
    )
    .await;
}

/// Round 7, finding 4: the re-read of the candidates' endpoints just before the
/// leases takes the worker's mode into account, so an `hmac` endpoint an operator
/// switched to `sealed` after a keyless worker chose its attempts is dropped there
/// and not left to the delivery step. Driven through the seam: the candidates are
/// chosen, the endpoint is switched, and the filter is applied.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_candidate_whose_endpoint_was_switched_to_sealed_is_dropped_for_a_keyless_worker_only() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/switch",
        8,
        "whsec_switch2_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgSw", "message.delivered", "{}").await;
    let candidates = vec![reread_attempt(&db, &attempt.id).await];

    let keyless_kept = filter_by_endpoint_health(
        &db,
        &sys(),
        candidates.clone(),
        Utc::now(),
        CLAIM_BUDGET,
        ServedModes::HmacOnly,
    )
    .await
    .expect("filtering");
    assert_eq!(
        keyless_kept.len(),
        1,
        "an hmac endpoint is the keyless worker's"
    );

    psql(&format!(
        "UPDATE webhook_endpoints SET delivery_mode = 'sealed', audience = 'aud-{suffix}', \
         secret = NULL, prev_secret = NULL, secret_rotated_at = NULL WHERE id = '{}'",
        endpoint.id
    ))
    .await;
    let keyless_kept = filter_by_endpoint_health(
        &db,
        &sys(),
        candidates.clone(),
        Utc::now(),
        CLAIM_BUDGET,
        ServedModes::HmacOnly,
    )
    .await
    .expect("filtering");
    assert!(
        keyless_kept.is_empty(),
        "switched to sealed, it is no longer the keyless worker's: {keyless_kept:?}"
    );
    let keyed_kept = filter_by_endpoint_health(
        &db,
        &sys(),
        candidates,
        Utc::now(),
        CLAIM_BUDGET,
        ServedModes::Both,
    )
    .await
    .expect("filtering");
    assert_eq!(keyed_kept.len(), 1, "and it is a keyed worker's");
}

/// L1: an operator switches an endpoint `hmac` to `sealed` while a keyless worker
/// holds one of its attempts. The worker can no longer send it, and must not
/// pretend it tried: the attempt was counted at the claim and `maxAttempts` is 1,
/// so a failure written here would go `dead` without a byte sent, and feed the
/// endpoint's breaker. It leaves the row `delivering`, as for an endpoint that
/// vanished; the lease expires, a keyless reclaim skips it (the endpoint is sealed
/// now) and a worker with the key takes it, resuming the attempt already counted.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_switch_to_sealed_after_a_keyless_claim_is_left_for_a_keyed_worker_without_spending_an_attempt()
 {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let (logs, _guard_logs) = capture_logs();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hook", server.uri()),
        1,
        "whsec_switch_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgL1", "message.delivered", "{}").await;

    let keyless = keyless(&db);
    let claimed = claim_webhook_attempts(&db, &sys(), "keyless-worker", 20, ServedModes::HmacOnly)
        .await
        .expect("the keyless claim");
    assert_eq!(claimed.len(), 1, "the hmac attempt is claimed: {claimed:?}");
    let held = &claimed[0];
    assert_eq!((held.state, held.attempts), (AttemptState::delivering, 1));

    psql(&format!(
        "UPDATE webhook_endpoints SET delivery_mode = 'sealed', audience = 'aud-{suffix}', \
         secret = NULL, prev_secret = NULL, secret_rotated_at = NULL WHERE id = '{}'",
        endpoint.id
    ))
    .await;
    hooks::deliver_claimed(&keyless, &sys(), &http_client(), claimed.clone()).await;

    let after = reread_attempt(&db, &attempt.id).await;
    assert_eq!(after.state, AttemptState::delivering, "{after:?}");
    assert_eq!(after.attempts, 1, "nothing was counted");
    assert_eq!(after.version, held.version, "and nothing was written");
    assert_eq!(after.lastError, None);
    let endpoint_after = reread_endpoint(&db, &endpoint.id).await;
    assert_eq!(
        endpoint_after.consecutiveFailures, 0,
        "the breaker is untouched"
    );
    assert_eq!(endpoint_after.circuitOpenUntil, None);
    assert!(server.received_requests().await.unwrap().is_empty());
    let text = logs.text();
    assert!(text.contains(&attempt.id), "{text}");
    assert!(text.contains("--envelope-signing-key-file"), "{text}");

    // The lease runs out. A keyless worker reclaims nothing: the endpoint is sealed.
    psql(&format!(
        "UPDATE webhook_attempts SET lease_until = now() - interval '5 seconds' WHERE id = '{}'",
        attempt.id
    ))
    .await;
    hooks::tick(&keyless, &sys(), "keyless-worker", &http_client())
        .await
        .expect("a keyless tick");
    assert_eq!(
        reread_attempt(&db, &attempt.id).await.version,
        after.version
    );

    // A worker with the key delivers it once, and the attempt was counted once.
    hooks::tick(
        &worker_context(db.clone()),
        &sys(),
        "keyed-worker",
        &http_client(),
    )
    .await
    .expect("a keyed tick");
    let delivered = reread_attempt(&db, &attempt.id).await;
    assert_eq!(delivered.state, AttemptState::succeeded, "{delivered:?}");
    assert_eq!(delivered.attempts, 1, "resumed, not counted twice");
}

/// N1: the warning counts the sealed attempts that wait for a worker that holds
/// the key. A deactivated endpoint's attempts wait for nothing: no worker claims
/// them, with the key or without, so counting them would warn about an unmounted
/// key for as long as the rows exist. A circuit-open endpoint's are counted: the
/// circuit is a pause that ends by itself (`circuitOpenUntil`), after which they
/// are exactly what a keyless worker cannot take, and a filter over a relation
/// cannot compare the optional timestamp in SQL anyway.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_deactivated_endpoints_attempts_do_not_count_as_waiting_for_the_key() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let endpoint = seed_endpoint(&db, &suffix, &app_id, "https://hooks.example.test/hook", 8).await;
    for n in 0..3 {
        seed_attempt(
            &db,
            &endpoint.id,
            &format!("cmsgN{n}"),
            "message.delivered",
            "{}",
        )
        .await;
    }
    let waiting = || async {
        count_attempts_waiting(&db, &sys(), Utc::now(), WebhookDeliveryMode::sealed)
            .await
            .expect("counting")
            .get()
    };
    let update = |input: UpdateWebhookEndpointInput| {
        let db = db.clone();
        let id = endpoint.id.clone();
        async move {
            let version = reread_endpoint(&db, &id).await.version;
            db.webhook_endpoint()
                .update(id)
                .set(input)
                .if_match(version)
                .run(&sys())
                .await
                .expect("updating the endpoint");
        }
    };

    assert_eq!(waiting().await, 3, "an active endpoint's due attempts wait");

    update(UpdateWebhookEndpointInput {
        active: Some(false),
        ..Default::default()
    })
    .await;
    assert_eq!(
        waiting().await,
        0,
        "a deactivated endpoint's are not waiting"
    );

    update(UpdateWebhookEndpointInput {
        active: Some(true),
        circuitOpenUntil: Some(Some(Utc::now() + Duration::minutes(10))),
        ..Default::default()
    })
    .await;
    assert_eq!(
        waiting().await,
        3,
        "an open circuit is a pause: the rows still wait for the key"
    );
}

// --- auth.methods without hmac_webhooks (VSA-1) -----------------------------
//
// With the method off an `hmac` endpoint is excluded from the claim, the way a
// `sealed` one is for a worker without the signing key: nothing is written for
// its attempts, so they stay `pending`, uncounted and off the breaker, and are
// delivered at the first claim after the method is permitted again.

/// `ctx` with `hmac_webhooks` as `auth.methods` says.
fn with_hmac(ctx: WorkerContext, hmac: sms_api::HmacWebhooks) -> WorkerContext {
    WorkerContext {
        hmac_webhooks: hmac,
        ..ctx
    }
}

/// The deployment's `auth.methods` without `hmac_webhooks`, on a worker that
/// holds the signing key: it serves `sealed` only ([`ServedModes::SealedOnly`]).
fn hmac_off(db: &Cratestack) -> WorkerContext {
    with_hmac(worker_context(db.clone()), sms_api::HmacWebhooks::Disabled)
}

/// The paths a receiver was asked for, in order.
async fn requested_paths(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("request recording is on by default")
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect()
}

/// `switchWebhookDeliveryMode` to `sealed`, as an operator holding `webhook:manage`
/// calls it on a deployment whose `auth.methods` does not permit `hmac_webhooks`:
/// the way out of `hmac`, and the one switch that method leaves open.
async fn switch_to_sealed(db: &Cratestack, endpoint_id: &str, audience: &str) {
    use sms_api::schema::procedures::{ProcedureRegistry, switch_webhook_delivery_mode};
    let mut ctx = owner();
    ctx.extensions.insert(
        "perms".to_owned(),
        cratestack::Value::List(vec![cratestack::Value::String("webhook:manage".to_owned())]),
    );
    let procedures = sms_api::Procedures::new(
        sms_api::HashPepper::new("hooks-live-postgres-test-pepper-well-over-the-minimum-length")
            .expect("a pepper over the minimum length"),
        sms_api::HmacWebhooks::Disabled,
    );
    let args = switch_webhook_delivery_mode::Args {
        args: schema::SwitchWebhookDeliveryModeInput {
            endpointId: endpoint_id.to_owned(),
            mode: WebhookDeliveryMode::sealed,
            audience: Some(audience.to_owned()),
        },
    };
    switch_webhook_delivery_mode::invoke_with_db(db, &args, &ctx, |db, authorized| {
        let (p, c, a) = (procedures.clone(), ctx.clone(), args.clone());
        async move { p.switch_webhook_delivery_mode(&db, &c, a, authorized).await }
    })
    .await
    .expect("switching the endpoint to sealed");
}

/// The whole contract, in one story, through `hooks::tick`: with the method off an
/// `hmac` attempt is never claimed (still `pending`, `attempts` 0, no `lastError`,
/// the row not written, the endpoint's breaker as it was) while a `sealed` one
/// beside it is delivered, poll after poll; and with the method permitted again the
/// next claim delivers it, with every try it had.
///
/// The `hmac` endpoint's `maxAttempts` is 1 on purpose: an attempt that was claimed
/// and counted would be one failure from `dead`, so `attempts` 0 and `pending` are
/// the proof nothing counted it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn with_hmac_off_an_hmac_attempt_is_never_claimed_and_a_sealed_one_beside_it_is_delivered() {
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
    Mock::given(method("POST"))
        .and(path("/hmac-hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let sealed = seed_endpoint(&db, &suffix, &app_id, &format!("{}/hook", server.uri()), 8).await;
    let hmac = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hmac-hook", server.uri()),
        1,
        "whsec_held_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let sealed_attempt = seed_attempt(&db, &sealed.id, "cmsgS", "message.delivered", "{}").await;
    let hmac_attempt = seed_attempt(&db, &hmac.id, "cmsgH", "message.delivered", "{}").await;

    let off = hmac_off(&db);
    for _ in 0..2 {
        hooks::tick(&off, &sys(), "test-worker", &http_client())
            .await
            .expect("tick succeeds");
    }

    assert_eq!(
        reread_attempt(&db, &sealed_attempt.id).await.state,
        AttemptState::succeeded,
        "the sealed attempt beside it is delivered as ever"
    );
    assert_eq!(
        requested_paths(&server).await,
        ["/hook"],
        "nothing may reach the hmac endpoint"
    );

    let held = reread_attempt(&db, &hmac_attempt.id).await;
    assert_eq!(held.state, AttemptState::pending, "{held:#?}");
    assert_eq!(
        held.attempts, 0,
        "never claimed, so never counted: maxAttempts is untouched"
    );
    assert_eq!(held.version, hmac_attempt.version, "the row is not written");
    assert!(held.lastError.is_none() && held.lastStatusCode.is_none());
    assert!(held.leaseOwner.is_none() && held.leaseUntil.is_none());
    let endpoint_after = reread_endpoint(&db, &hmac.id).await;
    assert_eq!(endpoint_after.consecutiveFailures, 0);
    assert!(endpoint_after.circuitOpenUntil.is_none());
    assert_eq!(
        endpoint_after.version, hmac.version,
        "the endpoint is not written"
    );

    // The method is permitted again (a restart): the very next claim delivers it,
    // with no hour's wait and no replay.
    let on = with_hmac(worker_context(db.clone()), sms_api::HmacWebhooks::Permitted);
    hooks::tick(&on, &sys(), "test-worker", &http_client())
        .await
        .expect("tick succeeds");
    let delivered = reread_attempt(&db, &hmac_attempt.id).await;
    assert_eq!(delivered.state, AttemptState::succeeded, "{delivered:#?}");
    assert_eq!(delivered.attempts, 1, "every try it had, one spent");
    assert_eq!(requested_paths(&server).await, ["/hook", "/hmac-hook"]);
}

/// A crashed worker's abandoned `hmac` attempt (`delivering`, lease expired) is no
/// more claimable than a pending one while the method is off: it keeps its lease
/// owner and its count, and a worker of a deployment that permits the method resumes
/// it without counting it twice.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn with_hmac_off_a_crash_abandoned_hmac_attempt_is_not_reclaimed_until_the_method_returns() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hmac-hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let hmac = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hmac-hook", server.uri()),
        8,
        "whsec_held_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let attempt = seed_attempt(&db, &hmac.id, "cmsgR", "message.delivered", "{}").await;
    let abandoned = db
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

    hooks::tick(&hmac_off(&db), &sys(), "test-worker", &http_client())
        .await
        .expect("tick succeeds");
    let still = reread_attempt(&db, &attempt.id).await;
    assert_eq!(still.version, abandoned.version, "{still:#?}");
    assert_eq!(still.state, AttemptState::delivering);
    assert_eq!(still.leaseOwner.as_deref(), Some("crashed-worker"));
    assert_eq!(still.attempts, 1);
    assert_eq!(requested_paths(&server).await, Vec::<String>::new());

    let on = with_hmac(worker_context(db.clone()), sms_api::HmacWebhooks::Permitted);
    hooks::tick(&on, &sys(), "test-worker", &http_client())
        .await
        .expect("tick succeeds");
    let resumed = reread_attempt(&db, &attempt.id).await;
    assert_eq!(resumed.state, AttemptState::succeeded, "{resumed:#?}");
    assert_eq!(resumed.attempts, 1, "resumed, not counted twice");
}

/// What a claim by a worker serving `served` takes, as sorted `aggregateId`s,
/// from the table as it stands.
async fn claimed_by(db: &Cratestack, served: ServedModes) -> Vec<String> {
    let mut got: Vec<String> =
        claim_webhook_attempts(db, &sys(), "held-worker", CLAIM_BUDGET, served)
            .await
            .expect("the claim")
            .into_iter()
            .map(|attempt| attempt.aggregateId)
            .collect();
    got.sort();
    got
}

/// The per-endpoint fan-out, when some endpoint is unclaimable: an `hmac` endpoint
/// holding [`UNHEALTHY_BACKLOG`] due attempts older than everyone's, and two
/// `sealed` ones holding 5 and 3 newer. A worker serving `sealed` only takes
/// exactly those 8, in one claim, past the 70 (a window of the oldest rows would be
/// the `hmac` endpoint's alone, and nothing would be delivered for anyone); the 70
/// are not touched. A worker serving nothing takes nothing from the same table.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_claim_with_hmac_off_leaves_a_deep_hmac_backlog_and_takes_the_sealed_endpoints_behind_it()
{
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    psql("DELETE FROM webhook_endpoints").await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let hmac = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/hmac",
        8,
        "whsec_fanout_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let s1 = seed_endpoint(
        &db,
        &format!("{suffix}-1"),
        &app_id,
        "https://hooks.example.test/s1",
        8,
    )
    .await;
    let s2 = seed_endpoint(
        &db,
        &format!("{suffix}-2"),
        &app_id,
        "https://hooks.example.test/s2",
        8,
    )
    .await;
    bulk_due(&hmac.id, "hm", UNHEALTHY_BACKLOG, 120).await;
    bulk_due(&s1.id, "s1", 5, 30).await;
    bulk_due(&s2.id, "s2", 3, 20).await;

    assert!(
        claimed_by(&db, ServedModes::Neither).await.is_empty(),
        "a worker serving nothing claims nothing, behind any backlog"
    );
    let mut sealed: Vec<String> = (1..=5)
        .map(|k| format!("bulk-s1-{k}"))
        .chain((1..=3).map(|k| format!("bulk-s2-{k}")))
        .collect();
    sealed.sort();
    assert_eq!(
        claimed_by(&db, ServedModes::SealedOnly).await,
        sealed,
        "the sealed endpoints' attempts, within one claim, past {UNHEALTHY_BACKLOG} older hmac ones"
    );
    assert_eq!(
        psql(
            "SELECT count(*) FROM webhook_attempts \
             WHERE aggregate_id LIKE 'bulk-hm-%' AND state = 'pending' AND attempts = 0 \
               AND lease_owner IS NULL"
        )
        .await,
        UNHEALTHY_BACKLOG.to_string(),
        "and the hmac endpoint's attempts are not touched"
    );
    psql("DELETE FROM webhook_endpoints").await;
}

/// The single time-ordered query, when every endpoint is one the worker serves: a
/// worker serving `sealed` only over a table of `sealed` endpoints takes the oldest
/// due attempts of any, and a worker serving nothing over the same table takes none
/// (an empty set of claimable endpoints is no query at all).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_claim_with_hmac_off_over_sealed_endpoints_only_is_the_time_ordered_query() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    psql("DELETE FROM webhook_endpoints").await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let s1 = seed_endpoint(
        &db,
        &format!("{suffix}-1"),
        &app_id,
        "https://hooks.example.test/s1",
        8,
    )
    .await;
    let s2 = seed_endpoint(
        &db,
        &format!("{suffix}-2"),
        &app_id,
        "https://hooks.example.test/s2",
        8,
    )
    .await;
    bulk_due(&s1.id, "s1", 5, 30).await;
    bulk_due(&s2.id, "s2", 3, 20).await;

    assert_eq!(
        claimed_by(&db, ServedModes::Neither).await,
        Vec::<String>::new()
    );
    assert_eq!(
        claimed_by(&db, ServedModes::HmacOnly).await,
        Vec::<String>::new()
    );
    let mut all: Vec<String> = (1..=5)
        .map(|k| format!("bulk-s1-{k}"))
        .chain((1..=3).map(|k| format!("bulk-s2-{k}")))
        .collect();
    all.sort();
    assert_eq!(claimed_by(&db, ServedModes::SealedOnly).await, all);
    psql("DELETE FROM webhook_endpoints").await;
}

/// The re-read just before the leases drops an `hmac` endpoint's attempt for a
/// worker serving `sealed` only, and keeps it for one that serves both: the second
/// line of the claim filter, for the milliseconds between the endpoints' read and
/// the leases (an endpoint switched to `hmac` by another deployment's workers, say).
/// Driven through the seam: the candidate is chosen, then filtered.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_reread_before_the_leases_drops_an_hmac_candidate_when_hmac_is_off() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/seam",
        8,
        "whsec_seam_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgSeam", "message.delivered", "{}").await;
    let candidates = vec![reread_attempt(&db, &attempt.id).await];

    for (served, kept) in [
        (ServedModes::Both, 1),
        (ServedModes::HmacOnly, 1),
        (ServedModes::SealedOnly, 0),
        (ServedModes::Neither, 0),
    ] {
        let got = filter_by_endpoint_health(
            &db,
            &sys(),
            candidates.clone(),
            Utc::now(),
            CLAIM_BUDGET,
            served,
        )
        .await
        .expect("filtering");
        assert_eq!(got.len(), kept, "{served:?}");
    }
}

/// `HmacBacklogWatch`, the sealed watch's twin: with the method off it counts the
/// due attempts of **active** `hmac` endpoints (a crashed worker's abandoned one
/// included; a deactivated endpoint's, a sealed endpoint's and nothing at all not)
/// and `warn`s with `held=N`, at most every five minutes. A worker of a deployment
/// that permits the method has nothing to say.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn with_hmac_off_a_worker_says_how_many_hmac_attempts_it_holds_back_every_five_minutes() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let (logs, _guard_logs) = capture_logs();
    let watch = hooks::HmacBacklogWatch::new();
    let start = std::time::Instant::now();

    assert_eq!(
        watch.report(&hmac_off(&db), &sys(), start).await.unwrap(),
        Some(0),
        "nothing held: the query runs and says nothing"
    );
    assert!(!logs.text().contains("WARN"), "{}", logs.text());

    let active = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/active",
        8,
        "whsec_watch_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let mut attempts = Vec::new();
    for n in 0..4 {
        attempts.push(
            seed_attempt(
                &db,
                &active.id,
                &format!("cmsgW{n}"),
                "message.delivered",
                "{}",
            )
            .await,
        );
    }
    // Counted: an attempt a crashed worker left `delivering`, lease expired.
    db.webhook_attempt()
        .update(attempts[0].id.clone())
        .set(UpdateWebhookAttemptInput {
            state: Some(AttemptState::delivering),
            leaseOwner: Some(Some("crashed-worker".to_owned())),
            leaseUntil: Some(Some(Utc::now() - Duration::seconds(5))),
            ..Default::default()
        })
        .if_match(attempts[0].version)
        .run(&sys())
        .await
        .expect("abandoning one hmac attempt");
    // Not counted: a deactivated hmac endpoint's attempts, and a sealed endpoint's.
    let inactive = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/inactive",
        8,
        "whsec_off_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    seed_attempt(&db, &inactive.id, "cmsgW9", "message.delivered", "{}").await;
    psql(&format!(
        "UPDATE webhook_endpoints SET active = false WHERE id = '{}'",
        inactive.id
    ))
    .await;
    let sealed = seed_endpoint(&db, &suffix, &app_id, "https://hooks.example.test/s", 8).await;
    seed_attempt(&db, &sealed.id, "cmsgW8", "message.delivered", "{}").await;

    let interval = hooks::HMAC_BACKLOG_WARNING_EVERY;
    assert_eq!(
        watch
            .report(&hmac_off(&db), &sys(), start + interval / 2)
            .await
            .unwrap(),
        None,
        "not due: no query, no line"
    );
    assert!(!logs.text().contains("WARN"), "{}", logs.text());

    let count = watch
        .report(&hmac_off(&db), &sys(), start + interval)
        .await
        .unwrap();
    assert_eq!(count, Some(4));
    let text = logs.text();
    assert_eq!(text.matches("WARN").count(), 1, "{text}");
    assert!(text.contains("held=4"), "{text}");
    assert!(text.contains("hmac_webhooks"), "{text}");

    assert_eq!(
        hooks::HmacBacklogWatch::new()
            .report(&worker_context(db.clone()), &sys(), start + interval)
            .await
            .unwrap(),
        None,
        "a deployment that permits the method serves them: nothing to report"
    );
}

/// Inserts `n` due `pending` attempts for `endpoint_id` in one statement, the way
/// a subscriber would have over a long outage: a table far too large to seed through
/// the API. `tag` keeps the dedupe key (endpoint, aggregate, event type) unique, and
/// the `bulk-` prefix is what `clear_claimable_backlog` deletes, as it does for the
/// other test that inserts thousands.
async fn seed_due_attempts(endpoint_id: &str, tag: &str, n: i64) {
    psql(&format!(
        "INSERT INTO webhook_attempts \
           (id, endpoint_id, source_event_id, aggregate_id, event_type, payload, state, \
            next_attempt_at, version) \
         SELECT 'cap-{tag}-' || g, '{endpoint_id}', gen_random_uuid(), 'bulk-{tag}-' || g, \
                'message.delivered', '{{}}', 'pending', now() - interval '1 minute', 1 \
           FROM generate_series(1, {n}) g"
    ))
    .await;
}

/// The backlog count is bounded: it reads ids per endpoint up to
/// `WAITING_COUNT_CAP + 1` in all, so what it costs does not grow with a backlog
/// that nothing purges. Exactly the cap is stated as a number; one more, across two
/// endpoints that are each under it, is "more than 10000", and the watch's line
/// says so rather than a figure it did not count.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_backlog_count_stops_at_the_cap_and_says_more_than_it() {
    use sms_worker::claim::{WAITING_COUNT_CAP, Waiting};

    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let first = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/first",
        8,
        "whsec_cap1_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let second = seed_hmac_endpoint(
        &db,
        &app_id,
        "https://hooks.example.test/second",
        8,
        "whsec_cap2_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let count = || async {
        count_attempts_waiting(&db, &sys(), Utc::now(), WebhookDeliveryMode::hmac)
            .await
            .expect("counting")
    };

    let cap = i64::try_from(WAITING_COUNT_CAP).unwrap();
    seed_due_attempts(&first.id, "a", cap).await;
    assert_eq!(
        count().await,
        Waiting::of(WAITING_COUNT_CAP),
        "exactly the cap is stated as a number, not as `more than`"
    );
    assert!(!count().await.is_more_than_cap());

    // Neither endpoint is over the cap on its own: the total is what is capped.
    seed_due_attempts(&second.id, "b", 5).await;
    let over = count().await;
    assert!(over.is_more_than_cap(), "{over:?}");
    assert_eq!(
        over.get(),
        WAITING_COUNT_CAP + 1,
        "it stops at the first row past the cap"
    );
    assert_eq!(over.to_string(), "more than 10000");

    // And an endpoint alone far past the cap is still read only to it.
    seed_due_attempts(&first.id, "c", 2_000).await;
    assert_eq!(
        count().await.get(),
        WAITING_COUNT_CAP + 1,
        "a deeper backlog costs no more"
    );

    let (logs, _guard_logs) = capture_logs();
    let start = std::time::Instant::now();
    let reported = hooks::HmacBacklogWatch::new()
        .report(&hmac_off(&db), &sys(), start)
        .await
        .unwrap();
    assert_eq!(reported, Some(WAITING_COUNT_CAP + 1));
    assert!(
        logs.text().contains("held=more than 10000"),
        "{}",
        logs.text()
    );
    psql("DELETE FROM webhook_attempts WHERE aggregate_id LIKE 'bulk-%'").await;
}

/// After `switchWebhookDeliveryMode` to `sealed`, the attempts that waited for the
/// method are delivered **sealed**: the switch rewrites the endpoint row only, and
/// the claim, `deliver_one` and `send` read the endpoint's mode afresh for every
/// attempt. A real `EnvelopeLayer` pinned to vsms's key and the endpoint's new
/// audience accepts exactly the bytes sent, and the body carries the sealed
/// envelope's fields.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_hmac_attempt_held_back_is_delivered_sealed_after_the_endpoint_is_switched() {
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
    let endpoint = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hook", server.uri()),
        8,
        "whsec_switch_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let attempt = seed_attempt(&db, &endpoint.id, "cmsgSw", "message.delivered", "{}").await;

    let off = hmac_off(&db);
    hooks::tick(&off, &sys(), "test-worker", &http_client())
        .await
        .expect("tick succeeds");
    assert!(requested_paths(&server).await.is_empty(), "held back");
    assert_eq!(
        reread_attempt(&db, &attempt.id).await.state,
        AttemptState::pending
    );

    let audience = format!("switched-{suffix}");
    switch_to_sealed(&db, &endpoint.id, &audience).await;
    hooks::tick(&off, &sys(), "test-worker", &http_client())
        .await
        .expect("tick succeeds");

    let delivered = reread_attempt(&db, &attempt.id).await;
    assert_eq!(delivered.state, AttemptState::succeeded, "{delivered:#?}");
    assert_eq!(delivered.attempts, 1);
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 1);
    let headers = wire_headers(&requests[0]);
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "content-type" && value.starts_with("application/cose")),
        "sealed, not signed: {headers:?}"
    );
    assert!(
        headers.iter().all(|(name, _)| !name.starts_with("x-sms")),
        "{headers:?}"
    );
    let receiver = receiver_for(&audience);
    let verdict = receiver.deliver("/hook", &headers, &requests[0].body).await;
    assert_eq!(verdict.status, 200);
    let envelope: serde_json::Value = serde_json::from_str(&receiver.bodies()[0]).unwrap();
    assert_eq!(envelope["id"], attempt.id);
    assert_eq!(envelope["endpointId"], endpoint.id);
}

/// The delivery step's own guard, for a row that got past the claim: an `hmac`
/// attempt claimed under a context that permits the method and delivered under one
/// that does not (`deliver_claimed` is the seam, the way an operator's change lands
/// between the two halves of a tick). Nothing is sent, and nothing is written but
/// what the claim already wrote: the attempt stays `delivering`, counted once, for
/// the lease to expire and a worker that serves `hmac` to resume it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_hmac_attempt_that_got_past_the_claim_is_not_sent_by_a_worker_with_hmac_off() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hmac-hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let hmac = seed_hmac_endpoint(
        &db,
        &app_id,
        &format!("{}/hmac-hook", server.uri()),
        1,
        "whsec_guard_0123456789abcdef0123456789abcdef",
        None,
    )
    .await;
    let attempt = seed_attempt(&db, &hmac.id, "cmsgG", "message.delivered", "{}").await;

    let claimed = claim_webhook_attempts(&db, &sys(), "test-worker", 20, ServedModes::Both)
        .await
        .expect("the claim");
    assert_eq!(claimed.len(), 1);
    hooks::deliver_claimed(&hmac_off(&db), &sys(), &http_client(), claimed).await;

    assert!(requested_paths(&server).await.is_empty(), "nothing is sent");
    let left = reread_attempt(&db, &attempt.id).await;
    assert_eq!(left.state, AttemptState::delivering, "{left:#?}");
    assert_eq!(left.attempts, 1, "counted once, at the claim, and no more");
    assert_eq!(
        reread_endpoint(&db, &hmac.id).await.consecutiveFailures,
        0,
        "and off the breaker"
    );
}
