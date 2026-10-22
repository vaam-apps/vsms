//! A message routed to AWS End User Messaging SMS, from claim to a signed
//! receipt moving it to `delivered`, against `sms-fake-aws` and a real
//! Postgres. No AWS account: what passes here is consistency with our
//! reading of AWS's documentation, not AWS (see `sms_fake_aws`'s crate doc).
//!
//! The real `AwsSmsProvider` sends (Signature Version 4, checked by the
//! fake), the fake publishes SNS notifications signed with a key it made at
//! start-up, and an in-process copy of `sms-gateway`'s DLR handler
//! authenticates each one with the adapter's real receiver and ingests it
//! through `sms_api::dlr::ingest`. The receiver fetches the signing
//! certificate from the fake through `RedirectSnsHttp`, the one seam; every
//! other check is the production one. `sms-gateway` is a binary crate with
//! no `lib.rs`, so the handler is copied here, as `chaos_live_postgres.rs`
//! already does for Orange.
//!
//! "Sent once" is asserted against the fake's ledger, never the database.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use chrono::{Duration as ChronoDuration, Utc};
use cratestack::CratestackContext;
use cratestack::sqlx::postgres::PgPoolOptions;
use rust_decimal::Decimal;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, DeliveryOutcome, Encoding, Message, MessageClass, MessageState, OperatorCode,
};
use sms_fake_aws::{
    EventStep, FakeAws, FakeAwsConfig, FaultPolicy, Redelivery, RedirectSnsHttp, SubmitDecision,
};
use sms_fake_orange::{DlrStatus, DlrStep, FakeOrange, TokenPolicy};
use sms_provider::{CallbackDisposition, RawCallback, SmsProvider};
use sms_provider_aws_sms::{AwsSmsConfig, AwsSmsProvider, MessageType};
use sms_provider_orange_cm::{OrangeCmConfig, OrangeCmProvider};
use sms_worker::WorkerContext;
use sms_worker::dispatch::tick;

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const ORANGE_SENDER: &str = "+2370000";

fn sys() -> CratestackContext {
    Principal {
        sub: "sms-worker-aws-fake-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CratestackContext {
    Principal {
        sub: "sms-worker-aws-fake-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{n:x}", Utc::now().timestamp_subsec_nanos())
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

/// Leftover claimable messages and active providers from an earlier test
/// in this binary would compete for the claim and the route.
async fn isolate(db: &Cratestack) {
    let backlog = db
        .message()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(schema::message::state().in_(
            [
                MessageState::accepted,
                MessageState::queued,
                MessageState::routed,
            ],
        )))
        .run(&sys())
        .await
        .expect("listing the claimable backlog");
    for message in backlog {
        let _ = db
            .message()
            .update(message.id.clone())
            .set(schema::UpdateMessageInput {
                state: Some(MessageState::cancelled),
                ..Default::default()
            })
            .if_match(message.version)
            .run(&sys())
            .await;
    }
    let active = db
        .provider()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::provider::state().eq(schema::ProviderState::active),
        ))
        .run(&owner())
        .await
        .expect("listing active providers");
    for provider in active {
        db.provider()
            .update(provider.id)
            .set(schema::UpdateProviderInput {
                state: Some(schema::ProviderState::disabled),
                ..Default::default()
            })
            .if_match(provider.version)
            .run(&owner())
            .await
            .expect("disabling a leftover provider");
    }
}

async fn seed_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "aws fake test app".to_owned(),
            slug: format!("aws-fake-{}", unique_suffix()),
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

/// An active provider row keyed `key`, and a catch-all route at `priority`.
async fn seed_routed_provider(
    db: &Cratestack,
    key: &str,
    kind: schema::ProviderKind,
    priority: i64,
) -> String {
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: key.to_owned(),
            displayName: format!("{key} (test)"),
            kind,
            config: "{}".to_owned(),
            credentialRef: "vault://test".to_owned(),
            maxTps: 10.0,
            maxDailySubmissions: 1000,
            supportsDlr: true,
            supportsAlphaSender: true,
            supportsUcs2: true,
            supportsConcat: true,
            costPerSegmentXaf: "30".parse().unwrap(),
            healthCheckedAt: None,
            circuitOpenUntil: None,
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
        .if_match(provider.version)
        .run(&owner())
        .await
        .expect("activating the provider");
    db.route()
        .create(schema::CreateRouteInput {
            name: format!("{key}-route"),
            priority,
            weight: 1,
            enabled: true,
            matchOperator: None,
            matchClass: None,
            matchAppId: None,
            matchPrefix: None,
            providerId: provider.id.clone(),
            failoverRouteId: None,
        })
        .run(&owner())
        .await
        .expect("seeding a catch-all route");
    provider.id
}

async fn seed_message(db: &Cratestack, app_id: &str) -> Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("aws-fake-{}", unique_suffix())),
            msisdn: "+237677123456".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:aws-fake-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 1000,
            body: Some("Votre code: 123456".to_owned()),
            bodyHash: "hmac-sha256-v1:aws-fake".to_owned(),
            bodyLength: 18,
            encoding: Encoding::gsm7,
            segments: 1,
            stateReason: None,
            routeId: None,
            providerId: None,
            providerMessageRef: None,
            providerMessageRefAlt: None,
            excludedRouteIds: None,
            maxAttempts: 3,
            leaseOwner: None,
            leaseUntil: None,
            scheduledAt: None,
            expiresAt: Utc::now() + ChronoDuration::minutes(15),
            submittedAt: None,
            finalizedAt: None,
            purgedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message")
}

async fn reload(db: &Cratestack, id: &str) -> Message {
    db.message()
        .find_unique(id.to_owned())
        .run(&sys())
        .await
        .expect("reloading the message")
        .expect("the message still exists")
}

async fn receipts(db: &Cratestack, message_id: &str) -> Vec<schema::DeliveryReceipt> {
    db.delivery_receipt()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::delivery_receipt::messageId().eq(message_id.to_owned()),
        ))
        .run(&sys())
        .await
        .expect("listing delivery receipts")
}

/// One provider the in-process DLR route serves.
struct DlrEntry {
    provider: Arc<dyn SmsProvider>,
    provider_row_id: String,
}

#[derive(Clone)]
struct DlrState {
    db: Cratestack,
    providers: Arc<std::sync::OnceLock<HashMap<String, DlrEntry>>>,
}

/// `sms-gateway`'s `dlr_handler`, minus logging: authenticate, then ingest.
async fn dlr_handler(
    Path(provider_key): Path<String>,
    State(state): State<DlrState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(entry) = state.providers.get().and_then(|map| map.get(&provider_key)) else {
        return StatusCode::NOT_FOUND;
    };
    let raw = RawCallback {
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_owned()))
            .collect(),
        body: body.to_vec(),
    };
    match entry.provider.authenticate_callback(&raw).await {
        Ok(CallbackDisposition::Ingest) => {}
        Ok(CallbackDisposition::Acknowledged { .. }) => return StatusCode::OK,
        Ok(CallbackDisposition::Refused { .. }) => return StatusCode::FORBIDDEN,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE,
    }
    match sms_api::dlr::ingest(
        &state.db,
        &sys(),
        entry.provider.as_ref(),
        &entry.provider_row_id,
        &raw,
    )
    .await
    {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::BAD_REQUEST,
    }
}

struct Harness {
    db: Cratestack,
    app_id: String,
    ctx: WorkerContext,
    aws: FakeAws,
    aws_row_id: String,
    dlr_base: String,
    /// Only when the test also routes to Orange.
    orange: Option<(FakeOrange, String)>,
}

/// The real sending adapter, pointed at the fake.
fn aws_sender(aws: &FakeAws) -> AwsSmsProvider {
    AwsSmsProvider::sender(AwsSmsConfig {
        region: sms_fake_aws::DEFAULT_REGION.to_owned(),
        access_key_id: sms_fake_aws::DEFAULT_ACCESS_KEY_ID.to_owned(),
        secret_access_key: sms_fake_aws::DEFAULT_SECRET_ACCESS_KEY.to_owned(),
        configuration_set: sms_fake_aws::DEFAULT_CONFIGURATION_SET.to_owned(),
        origination_identity: None,
        message_type: MessageType::Transactional,
        tps_ceiling: 10.0,
        cost_per_segment_xaf: Decimal::new(30, 0),
        supports_alphanumeric_sender: true,
        endpoint_url: Some(aws.base_url().to_owned()),
        time_to_live: Some(Duration::from_mins(15)),
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(2),
    })
    .unwrap()
}

/// A happy Orange, routed at priority 500, registered for dispatch and for
/// the DLR route. Returns the fake and its provider row id.
async fn add_orange(
    db: &Cratestack,
    dlr_base: &str,
    registry: &mut HashMap<String, Arc<dyn SmsProvider>>,
    dlr_providers: &mut HashMap<String, DlrEntry>,
) -> (FakeOrange, String) {
    let fake = FakeOrange::start(
        sms_fake_orange::FaultPolicy::always(sms_fake_orange::SubmitDecision::accepted_with_dlrs(
            vec![DlrStep::after(
                Duration::from_millis(200),
                DlrStatus::Delivered,
            )],
        )),
        TokenPolicy::Always,
        format!("{dlr_base}/dlr/orange_cm"),
        ORANGE_SENDER,
    )
    .await;
    let orange_key = format!("orange_t_{}", unique_suffix());
    let orange_row_id =
        seed_routed_provider(db, &orange_key, schema::ProviderKind::orange_cm_http, 500).await;
    let provider: Arc<dyn SmsProvider> = Arc::new(OrangeCmProvider::new(OrangeCmConfig {
        client_id: "test".to_owned(),
        client_secret: "test".to_owned(),
        sender_number: ORANGE_SENDER.to_owned(),
        base_url: fake.base_url(),
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(2),
    }));
    registry.insert(orange_key, provider.clone());
    dlr_providers.insert(
        "orange_cm".to_owned(),
        DlrEntry {
            provider,
            provider_row_id: orange_row_id.clone(),
        },
    );
    (fake, orange_row_id)
}

/// AWS at priority 1000, and with `with_orange`, a happy Orange behind it
/// at 500 — the go-live arrangement where Orange takes what AWS cannot.
async fn harness(policy: FaultPolicy, with_orange: bool) -> Harness {
    // `database_url()` installs the rustls crypto provider first.
    let db = db().await;
    isolate(&db).await;
    let app_id = seed_app(&db).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dlr_base = format!("http://{}", listener.local_addr().unwrap());
    let state = DlrState {
        db: db.clone(),
        providers: Arc::new(std::sync::OnceLock::new()),
    };
    let app = Router::new()
        .route("/dlr/{providerKey}", post(dlr_handler))
        .with_state(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let mut aws_config = FakeAwsConfig::with_defaults(format!("{dlr_base}/dlr/aws_sms"));
    aws_config.redelivery = Redelivery {
        attempts: 3,
        interval: Duration::from_millis(100),
    };
    let aws = FakeAws::start(aws_config, policy).await;

    let aws_key = format!("aws_t_{}", unique_suffix());
    let aws_row_id =
        seed_routed_provider(&db, &aws_key, schema::ProviderKind::aggregator_http, 1000).await;
    let sender: Arc<dyn SmsProvider> = Arc::new(aws_sender(&aws));
    let receiver: Arc<dyn SmsProvider> = Arc::new(
        AwsSmsProvider::receiver_with(
            aws.topic_arn().to_owned(),
            Arc::new(RedirectSnsHttp::new(aws.base_url()).unwrap()),
        )
        .unwrap(),
    );

    let mut registry: HashMap<String, Arc<dyn SmsProvider>> = HashMap::from([(aws_key, sender)]);
    let mut dlr_providers = HashMap::from([(
        "aws_sms".to_owned(),
        DlrEntry {
            provider: receiver,
            provider_row_id: aws_row_id.clone(),
        },
    )]);

    let orange = if with_orange {
        Some(add_orange(&db, &dlr_base, &mut registry, &mut dlr_providers).await)
    } else {
        None
    };
    assert!(state.providers.set(dlr_providers).is_ok());

    Harness {
        db,
        app_id,
        ctx: WorkerContext {
            db: state.db.clone(),
            providers: Arc::new(registry),
        },
        aws,
        aws_row_id,
        dlr_base,
        orange,
    }
}

/// Runs dispatch until the message leaves the claimable states, then waits
/// (up to 10 s) for it to reach `want`.
async fn drive_until(h: &Harness, message_id: &str, want: MessageState) -> Message {
    for _ in 0..200 {
        let message = reload(&h.db, message_id).await;
        if message.state == want {
            return message;
        }
        if matches!(
            message.state,
            MessageState::accepted | MessageState::queued | MessageState::routed
        ) {
            tick(&h.ctx, &sys(), "aws-fake-worker").await.expect("tick");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let message = reload(&h.db, message_id).await;
    panic!(
        "message never reached {want:?}; it is {:?} ({:?})",
        message.state, message.stateReason
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres"]
async fn a_message_routed_to_aws_is_delivered_by_a_signed_receipt() {
    let _guard = TEST_MUTEX.lock().await;
    let h = harness(
        FaultPolicy::always(SubmitDecision::delivered_after(Duration::from_millis(300))),
        false,
    )
    .await;
    let message = seed_message(&h.db, &h.app_id).await;

    let delivered = drive_until(&h, &message.id, MessageState::delivered).await;

    let accepted = h.aws.ledger().accepted();
    assert_eq!(accepted.len(), 1, "sent to AWS exactly once");
    assert_eq!(
        delivered.providerMessageRef.as_deref(),
        accepted[0].message_id.as_deref()
    );
    assert_eq!(delivered.providerId.as_deref(), Some(h.aws_row_id.as_str()));
    assert_eq!(
        accepted[0].context.get("vsmsMessageId"),
        Some(&message.id),
        "every message carries our own id in Context"
    );
    assert_eq!(accepted[0].time_to_live, Some(900));

    let mut outcomes: Vec<_> = receipts(&h.db, &message.id)
        .await
        .into_iter()
        .map(|receipt| (receipt.rawStatus, receipt.outcome))
        .collect();
    outcomes.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        outcomes,
        vec![
            ("TEXT_DELIVERED".to_owned(), DeliveryOutcome::delivered),
            ("TEXT_PENDING".to_owned(), DeliveryOutcome::in_flight),
        ]
    );
    assert!(
        h.aws
            .ledger()
            .notifications()
            .iter()
            .all(|n| n.acknowledged && n.last_status == Some(200)),
        "{:?}",
        h.aws.ledger().notifications()
    );
}

/// The SMS sandbox refuses every unverified number; with Orange routed
/// behind AWS, the message fails over and is delivered there. AWS saw one
/// refused submit, Orange one accepted one.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres"]
async fn the_aws_sandbox_refusal_fails_over_to_orange() {
    let _guard = TEST_MUTEX.lock().await;
    let h = harness(
        FaultPolicy::always(SubmitDecision::sandbox_unverified()),
        true,
    )
    .await;
    let message = seed_message(&h.db, &h.app_id).await;

    let delivered = drive_until(&h, &message.id, MessageState::delivered).await;

    let (orange, orange_row_id) = h.orange.as_ref().unwrap();
    assert_eq!(
        delivered.providerId.as_deref(),
        Some(orange_row_id.as_str())
    );
    let aws_submits = h.aws.ledger().submits();
    assert_eq!(aws_submits.len(), 1);
    assert_eq!(
        aws_submits[0].error_type.as_deref(),
        Some("ConflictException")
    );
    assert_eq!(h.aws.ledger().accepted().len(), 0);
    assert_eq!(orange.ledger().submits().len(), 1);
}

/// AWS took the message but the answer had no `MessageId`: `uncertain`,
/// and never sent a second time, even after more dispatch ticks.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres"]
async fn an_aws_accept_without_a_message_id_is_uncertain_and_never_resent() {
    let _guard = TEST_MUTEX.lock().await;
    let h = harness(
        FaultPolicy::always(SubmitDecision::without_message_id(vec![
            EventStep::delivered(Duration::from_millis(100)),
        ])),
        false,
    )
    .await;
    let message = seed_message(&h.db, &h.app_id).await;

    drive_until(&h, &message.id, MessageState::uncertain).await;
    for _ in 0..3 {
        tick(&h.ctx, &sys(), "aws-fake-worker").await.expect("tick");
    }
    tokio::time::sleep(Duration::from_millis(400)).await;

    assert_eq!(h.aws.ledger().submits().len(), 1, "never resubmitted");
    let after = reload(&h.db, &message.id).await;
    assert_eq!(after.state, MessageState::uncertain);
    assert_eq!(
        receipts(&h.db, &message.id).await.len(),
        0,
        "the receipt names a MessageId we never learned, so nothing correlates"
    );
}

/// A receipt the fake genuinely signed, with its event edited, is refused
/// on the route and leaves nothing behind.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres"]
async fn a_tampered_aws_receipt_is_refused_on_the_route() {
    let _guard = TEST_MUTEX.lock().await;
    let h = harness(
        FaultPolicy::always(SubmitDecision::accepted_silently()),
        false,
    )
    .await;
    let message = seed_message(&h.db, &h.app_id).await;
    let submitted = drive_until(&h, &message.id, MessageState::submitted).await;
    let message_id = submitted.providerMessageRef.clone().unwrap();

    let genuine = h.aws.signed_notification(&format!(
        r#"{{"eventType":"TEXT_PENDING","isFinal":false,"messageId":"{message_id}"}}"#
    ));
    let tampered = genuine.replacen("TEXT_PENDING", "TEXT_DELIVERED", 1);
    let tampered = tampered.replacen("\\\"isFinal\\\":false", "\\\"isFinal\\\":true", 1);
    assert_ne!(genuine, tampered);

    let client = reqwest::Client::new();
    let url = format!("{}/dlr/aws_sms", h.dlr_base);
    let status = client
        .post(&url)
        .body(tampered)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 403);
    assert_eq!(receipts(&h.db, &message.id).await.len(), 0);
    assert_eq!(
        reload(&h.db, &message.id).await.state,
        MessageState::submitted
    );

    // The genuine one, on the same route, is ingested.
    let status = client
        .post(&url)
        .body(genuine)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 200);
    assert_eq!(receipts(&h.db, &message.id).await.len(), 1);
}
