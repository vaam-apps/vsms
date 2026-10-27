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
    UpdateWebhookAttemptInput, UpdateWebhookEndpointInput, webhook_attempt,
};
use sms_test_support::webhook_receiver::WebhookReceiver;
use sms_webhook::{CONTRACT_SHA, Sealer};
use sms_worker::WorkerContext;
use sms_worker::claim::{ServedModes, claim_webhook_attempts, count_sealed_attempts_waiting};
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
    let database_url = sms_test_support::database_url().await;
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
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    psql(&format!(
        "SELECT coalesce(max(idx_tup_read), 0) FROM pg_stat_user_indexes \
         WHERE indexrelname = '{index}'"
    ))
    .await
    .parse()
    .expect("a count")
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
/// the key. A deactivated endpoint's wait for nothing: no worker claims them, with
/// the key or without, so counting them would warn about an unmounted key for as
/// long as the rows exist. A circuit-open endpoint's are counted: the circuit is a
/// pause that ends by itself (`circuitOpenUntil`), after which they are exactly
/// what a keyless worker cannot take, and a filter over a relation cannot compare
/// the optional timestamp in SQL anyway.
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
        count_sealed_attempts_waiting(&db, &sys(), Utc::now())
            .await
            .expect("counting")
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
