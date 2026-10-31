//! Reading a message needs `sms:read`, over real HTTP: who can and who cannot
//! list or read a message, a receipt and a part, by a paired console key and by
//! a person's token, and that an app key still reads its own and only its own.
//!
//! `Message`'s `list`/`detail` policy admits any `user` (no role test), and a
//! role's permissions cannot be written into a policy, so the answer is Layer 2:
//! `router::MESSAGE_READ_ROUTES`. Anchors:
//!
//! - **a role is decided by its permission, not its name**: the six built-in
//!   roles all carry `sms:read` (`docs/architecture.md` section 5.2) and read; a
//!   custom role without it is refused on all six routes with a 403 that names
//!   the permission; a custom role holding only `sms:read` reads;
//! - **the other doors to a message are shut too**: `GET /delivery_receipts` and
//!   `GET /message_parts` with `?include=message` return the whole `Message`
//!   row (body and number) under `Message`'s own policy, so a role refused
//!   `GET /messages` must be refused those, and the message text never appears in
//!   a refusal;
//! - **a person's token is the same person**: a token and a console key for the
//!   same non-device-only role get the same answer;
//! - **an app key**: with `sms:read` it reads its own messages and none of
//!   another app's; without it (a send-only key, or none) it is refused;
//! - **HEAD is GET**: a `HEAD` answers with the `GET` handler and the full
//!   `Content-Length`, so it is gated as the `GET` (and so are the `Job`,
//!   `Provider` and `Route` read gates, which a `HEAD` used to walk past);
//! - **the doors through other models**: `GET /apps?include=messages`, a relation
//!   filter (`?messages.some.msisdn=`) and a nested include
//!   (`/consent_records?include=app,app.messages`) read `Message` under its own
//!   policy, which asks for the `messageRead` claim, so a caller without
//!   `sms:read` gets no message from them, each with a positive control;
//! - **opt-outs**: an app key reads them only with `optout:read`.
//!
//! Ignored by default, like this workspace's other live suites:
//!
//! ```bash
//! cargo test -p sms-api --test message_read_live_postgres -- --ignored --test-threads=1
//! ```

#[path = "support/sealed_fixtures.rs"]
mod sealed_fixtures;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use cratestack::CratestackContext;
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sealed_fixtures::{
    api_router_doors, client, owner, register_app_key, seed_user_with_console_key, serve,
};
use sms_api::auth::system_context;
use sms_api::schema::{self, Cratestack, Encoding, MessageClass, OperatorCode, role};
use sms_api::{ApiDoors, BearerRefusal, BearerVerifier, HmacWebhooks, VerifiedBearer};
use sms_test_support::sealed::{Reply, SealedClient, TestKey};

const PEPPER: &str = "message-read-live-test-pepper-well-over-the-minimum-length";

/// The six roles `0002_bootstrap` seeds for a person.
const BUILT_IN_ROLES: [&str; 6] = [
    "owner",
    "admin",
    "operator",
    "developer",
    "auditor",
    "support",
];

/// Races on Postgres's `pg_type` catalog on a fresh database; one test at a time.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CratestackContext {
    system_context("message-read-live-test")
}

fn suffix() -> String {
    cratestack::uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
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

/// Answers from a table: a token string is a key into it.
#[derive(Default)]
struct Table(Mutex<HashMap<String, VerifiedBearer>>);

#[async_trait::async_trait]
impl BearerVerifier for Table {
    async fn verify(&self, token: &str) -> Result<VerifiedBearer, BearerRefusal> {
        self.0
            .lock()
            .unwrap()
            .get(token)
            .cloned()
            .ok_or(BearerRefusal::Malformed)
    }
}

/// One app's message, with the receipt and the part that hang off it.
struct Seeded {
    app_id: String,
    message_id: String,
    receipt_id: String,
    part_id: String,
    /// The text of the message body: random per run, and the thing a refusal
    /// must never contain.
    body_text: String,
}

struct Rig {
    db: Cratestack,
    table: Arc<Table>,
    base: String,
    console_client: String,
    mine: Seeded,
    other: Seeded,
}

async fn seed_app(db: &Cratestack) -> String {
    let tag = suffix();
    db.app()
        .create(schema::CreateAppInput {
            name: "message read test app".to_owned(),
            slug: format!("message-read-{tag}"),
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

async fn seed_message_graph(db: &Cratestack, provider_id: &str) -> Seeded {
    let app_id = seed_app(db).await;
    let tag = suffix();
    let body_text = format!("code {tag}");
    let message = db
        .message()
        .create(schema::CreateMessageInput {
            appId: app_id.clone(),
            clientRef: None,
            idempotencyKey: Some(format!("message-read-{tag}")),
            msisdn: "+237677123456".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:message-read-{tag}"),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 1000,
            body: Some(body_text.clone()),
            bodyHash: "hmac-sha256-v1:message-read".to_owned(),
            bodyLength: 17,
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
            expiresAt: Utc::now() + Duration::hours(1),
            submittedAt: None,
            finalizedAt: None,
            purgedAt: None,
            excludedRouteIds: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message");
    let receipt = db
        .delivery_receipt()
        .create(schema::CreateDeliveryReceiptInput {
            messageId: message.id.clone(),
            providerId: provider_id.to_owned(),
            providerMessageRef: format!("message-read-ref-{tag}"),
            outcome: schema::DeliveryOutcome::failed,
            rawStatus: "TemporaryError".to_owned(),
            errorCode: None,
            networkCode: OperatorCode::mtn,
            occurredAt: None,
            rawPayload: "{\"raw\":\"TemporaryError\"}".to_owned(),
        })
        .run(&sys())
        .await
        .expect("seeding the receipt");
    let part = db
        .message_part()
        .create(schema::CreateMessagePartInput {
            messageId: message.id.clone(),
            partIndex: 0,
            udhRef: None,
            providerPartRef: None,
            submittedAt: None,
            deliveredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the part");
    Seeded {
        app_id,
        message_id: message.id,
        receipt_id: receipt.id,
        part_id: part.id,
        body_text,
    }
}

async fn seed_console_client(db: &Cratestack) -> String {
    db.oauth_client()
        .create(schema::CreateOauthClientInput {
            clientId: format!("console_{}", suffix()),
            appClientId: None,
            tokenEndpointAuthMethod: schema::ClientAuthMethod::none,
            jwks: None,
            grantTypes: " authorization_code ".to_owned(),
            scopes: " openid profile ".to_owned(),
            redirectUris: " https://console.example/cb ".to_owned(),
            requirePkce: true,
        })
        .run(&sys())
        .await
        .expect("seeding the console client")
        .clientId
}

/// What `0002_bootstrap` seeds for `developer`.
const DEVELOPER_PERMISSIONS: [&str; 4] = ["app:read", "webhook:manage", "sms:read", "sms:send"];

/// Sets a built-in role's permissions (the console's role editor, as an owner).
/// A fresh router has a fresh standing cache, so the edit is seen by the next
/// `rig()`'s requests and not by a stale one.
async fn set_role_permissions(db: &Cratestack, key: &str, permissions: &[&str]) {
    let current = db
        .role()
        .find_many()
        .where_expr(FilterExpr::from(role::key().eq(key.to_owned())))
        .run(&sys())
        .await
        .expect("reading the role")
        .into_iter()
        .next()
        .expect("a built-in role is seeded");
    db.role()
        .update(current.id)
        .set(schema::UpdateRoleInput {
            permissions: Some(sms_core::pack(permissions).expect("no space in a permission")),
            ..Default::default()
        })
        .if_match(current.version)
        .run(&owner())
        .await
        .expect("editing the role");
}

async fn rig() -> Rig {
    let db = db().await;
    // A test that strips `developer` restores it; this holds if one died first.
    set_role_permissions(&db, "developer", &DEVELOPER_PERMISSIONS).await;
    let table = Arc::new(Table::default());
    let doors = ApiDoors::new(true, Some(table.clone() as Arc<dyn BearerVerifier>))
        .expect("a door is open");
    let base = serve(api_router_doors(
        &db,
        PEPPER,
        HmacWebhooks::Permitted,
        doors,
    ))
    .await;
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!("mread_{}", suffix()),
            displayName: "message read test provider".to_owned(),
            kind: schema::ProviderKind::orange_cm_http,
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
    let mine = seed_message_graph(&db, &provider.id).await;
    let other = seed_message_graph(&db, &provider.id).await;
    let console_client = seed_console_client(&db).await;
    Rig {
        db,
        table,
        base,
        console_client,
        mine,
        other,
    }
}

/// A role holding exactly `permissions`; a fresh key each call, so the 60 s
/// standing cache can never carry one test's role into the next.
async fn custom_role(db: &Cratestack, permissions: &[&str]) -> String {
    let key = format!("mread_{}", suffix());
    db.role()
        .create(schema::CreateRoleInput {
            key: key.clone(),
            label: format!("message read test role {key}"),
            description: None,
            permissions: sms_core::pack(permissions).expect("no space in a permission literal"),
        })
        .run(&owner())
        .await
        .expect("seeding the custom role");
    key
}

impl Rig {
    /// A person in `role`, reachable by a paired console key.
    async fn person_by_key(&self, role: &str) -> SealedClient {
        let key = TestKey::console();
        seed_user_with_console_key(&self.db, role, &key).await;
        client(&self.base, &key)
    }

    /// A person in `role` who signed in with a password: a token, no key.
    async fn person_by_token(&self, role: &str) -> String {
        let tag = suffix();
        let user = self
            .db
            .user()
            .create(schema::CreateUserInput {
                subject: format!("pending-{tag}"),
                email: format!("{tag}@fixture.invalid"),
                displayName: "Message Read Person".to_owned(),
                roleKey: role.to_owned(),
                lastLoginAt: None,
                deletedAt: None,
            })
            .run(&owner())
            .await
            .expect("seeding the person");
        let token = format!("tok-{}", suffix());
        self.table.0.lock().unwrap().insert(
            token.clone(),
            VerifiedBearer::Person {
                user_id: user.id,
                client_id: self.console_client.clone(),
            },
        );
        token
    }

    /// An app key holding exactly `scopes`, and the `AppClient`'s app.
    async fn app_key(&self, app_id: &str, scopes: &str) -> SealedClient {
        let tag = suffix();
        let row = self
            .db
            .app_client()
            .create(schema::CreateAppClientInput {
                appId: app_id.to_owned(),
                clientId: format!("appc_mread_{tag}"),
                label: "message read test client".to_owned(),
                scopes: scopes.to_owned(),
                lastUsedAt: None,
                retiredAt: None,
            })
            .run(&sys())
            .await
            .expect("seeding the app client")
            .id;
        let key = TestKey::app();
        register_app_key(&self.db, &row, &key).await;
        client(&self.base, &key)
    }
}

/// What a caller sees on the six routes that can hand a message back.
struct Probe {
    method: &'static str,
    template: &'static str,
    query: Option<&'static str>,
    id: fn(&Seeded) -> &str,
}

fn mine_message(s: &Seeded) -> &str {
    &s.message_id
}
fn mine_receipt(s: &Seeded) -> &str {
    &s.receipt_id
}
fn mine_part(s: &Seeded) -> &str {
    &s.part_id
}

/// Every route a message can come back by: the two of `Message` itself, and the
/// two models that carry a `message` relation, each plain and with the whole row
/// asked for by `include`.
const PROBES: [Probe; 11] = [
    Probe {
        method: "GET",
        template: "/messages",
        query: None,
        id: mine_message,
    },
    Probe {
        method: "GET",
        template: "/messages/{id}",
        query: None,
        id: mine_message,
    },
    Probe {
        method: "GET",
        template: "/delivery_receipts",
        query: None,
        id: mine_receipt,
    },
    Probe {
        method: "GET",
        template: "/delivery_receipts",
        query: Some("include=message"),
        id: mine_receipt,
    },
    Probe {
        method: "GET",
        template: "/delivery_receipts/{id}",
        query: None,
        id: mine_receipt,
    },
    Probe {
        method: "GET",
        template: "/delivery_receipts/{id}",
        query: Some("include=message"),
        id: mine_receipt,
    },
    Probe {
        method: "GET",
        template: "/message_parts",
        query: None,
        id: mine_part,
    },
    Probe {
        method: "GET",
        template: "/message_parts",
        query: Some("include=message"),
        id: mine_part,
    },
    Probe {
        method: "GET",
        template: "/message_parts/{id}",
        query: None,
        id: mine_part,
    },
    Probe {
        method: "GET",
        template: "/message_parts/{id}",
        query: Some("include=message"),
        id: mine_part,
    },
    Probe {
        method: "GET",
        template: "/message_parts",
        query: Some("message.msisdn=%2B237677123456"),
        id: mine_part,
    },
];

async fn call(api: &SealedClient, probe: &Probe, seeded: &Seeded) -> Reply {
    call_as(api, probe.method, probe, seeded).await
}

/// [`call`] with the method replaced: a `HEAD` for the `GET` it is answered by.
async fn call_as(api: &SealedClient, method: &str, probe: &Probe, seeded: &Seeded) -> Reply {
    let mut request = api.request(method, probe.template);
    let id = (probe.id)(seeded).to_owned();
    if probe.template.contains("{id}") {
        request = request.params(&[id.as_str()]);
    }
    if let Some(query) = probe.query {
        request = request.query(query);
    }
    deliver(request, method).await
}

/// Sends `request`. A `HEAD` answer is sealed but carries no body to open, so
/// it is delivered raw: its status and headers are what is under test.
async fn deliver(request: sms_test_support::sealed::CallBuilder<'_>, method: &str) -> Reply {
    if method == "HEAD" {
        request.seal().await.resend().await
    } else {
        request.send().await
    }
}

fn describe(probe: &Probe) -> String {
    format!(
        "{} {}{}",
        probe.method,
        probe.template,
        probe.query.map_or_else(String::new, |q| format!("?{q}"))
    )
}

fn assert_refused_naming_sms_read(reply: &Reply, rig: &Rig, who: &str, probe: &Probe) {
    let text = reply.text();
    assert_eq!(reply.status, 403, "{who}: {} -> {text}", describe(probe));
    assert!(
        text.contains("sms:read"),
        "{who}: {} -> the refusal names the permission it wants: {text}",
        describe(probe)
    );
    for leaked in [&rig.mine.body_text, &rig.other.body_text, "+237677123456"] {
        assert!(
            !text.contains(leaked),
            "{who}: {} -> a refusal carried message content: {text}",
            describe(probe)
        );
    }
}

/// Every built-in role carries `sms:read` (section 5.2), so every one of them
/// reads, by a paired key, on every route; the gate changed nobody's answer here.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn every_built_in_role_reads_messages_with_its_console_key() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    for role in BUILT_IN_ROLES {
        let api = rig.person_by_key(role).await;
        for probe in &PROBES {
            let reply = call(&api, probe, &rig.mine).await;
            assert_eq!(
                reply.status,
                200,
                "{role}: {} -> {}",
                describe(probe),
                reply.text()
            );
        }
        // Global roles read every app's messages: both apps' rows are in the list.
        let list = api.request("GET", "/messages").send().await.text();
        assert!(
            list.contains(&rig.mine.message_id) && list.contains(&rig.other.message_id),
            "{role}: a person in a global role lists every app's messages"
        );
    }
}

/// A role without `sms:read` is refused on every route, whatever else it holds,
/// and a refusal never carries the message. This is the finding: before the gate
/// each of these answered 200 with the body.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_role_without_sms_read_reads_no_message_by_any_route() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    // Everything a console role holds except `sms:read` and the admin-only rights.
    let role = custom_role(
        &rig.db,
        &[
            "sms:send",
            "app:read",
            "optout:manage",
            "delivery:read",
            "dashboard:read",
            "job:read",
        ],
    )
    .await;
    let api = rig.person_by_key(&role).await;
    for probe in &PROBES {
        let reply = call(&api, probe, &rig.mine).await;
        assert_refused_naming_sms_read(&reply, &rig, "custom role without sms:read", probe);
    }
}

/// The permission decides, not the role's name: a custom role whose only
/// permission is `sms:read` reads on every route.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_custom_role_holding_only_sms_read_reads() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let role = custom_role(&rig.db, &["sms:read"]).await;
    let api = rig.person_by_key(&role).await;
    for probe in &PROBES {
        let reply = call(&api, probe, &rig.mine).await;
        assert_eq!(reply.status, 200, "{}: {}", describe(probe), reply.text());
    }
}

/// A person's token is the same person as their key: `support` (the role a
/// first single-sign-on sign-in is created in) reads, a custom role without
/// `sms:read` does not, and `owner` and `admin` are not reachable by a token at
/// all (`DEVICE_REQUIRED`, VSA-4b), which this does not change.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_persons_token_gets_the_answer_their_role_gets() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let get = |token: String, path: String| {
        let base = rig.base.clone();
        async move {
            let response = reqwest::Client::new()
                .get(format!("{base}{path}"))
                .bearer_auth(token)
                .header("accept", "application/json")
                .send()
                .await
                .expect("sending a bearer call");
            (
                response.status().as_u16(),
                response.text().await.expect("reading the answer"),
            )
        }
    };
    let detail = format!("/messages/{}", rig.mine.message_id);
    let read_but_for = custom_role(&rig.db, &["sms:send", "app:read"]).await;
    let read_only = custom_role(&rig.db, &["sms:read"]).await;

    for role in ["operator", "developer", "auditor", "support"] {
        let token = rig.person_by_token(role).await;
        for path in ["/messages", detail.as_str(), "/delivery_receipts"] {
            let (status, body) = get(token.clone(), path.to_owned()).await;
            assert_eq!(status, 200, "{role}: {path} -> {body}");
        }
    }
    let token = rig.person_by_token(&read_only).await;
    let (status, body) = get(token, "/messages".to_owned()).await;
    assert_eq!(status, 200, "a custom role with sms:read: {body}");

    let token = rig.person_by_token(&read_but_for).await;
    for path in [
        "/messages",
        detail.as_str(),
        "/delivery_receipts?include=message",
        "/message_parts?include=message",
    ] {
        let (status, body) = get(token.clone(), path.to_owned()).await;
        assert_eq!(status, 403, "a role without sms:read: {path} -> {body}");
        assert!(body.contains("sms:read"), "{path} -> {body}");
        assert!(!body.contains(&rig.mine.body_text), "{path} -> {body}");
    }

    for role in ["owner", "admin"] {
        let token = rig.person_by_token(role).await;
        let (status, body) = get(token, "/messages".to_owned()).await;
        assert_eq!(status, 403, "{role} behind a token: {body}");
        assert!(body.contains(sms_api::DEVICE_REQUIRED), "{role}: {body}");
    }
}

/// An app key keeps reading its own messages, and only its own, and needs the
/// `sms:read` scope to do it: a send-only key (the `otp-sender` of the
/// deployment runbook) and a key with no scope are refused.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_app_key_reads_its_own_messages_with_sms_read_and_not_without_it() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;

    let reader = rig.app_key(&rig.mine.app_id, " sms:send sms:read ").await;
    let list = reader.request("GET", "/messages").send().await;
    assert_eq!(list.status, 200, "{}", list.text());
    let text = list.text();
    assert!(
        text.contains(&rig.mine.message_id) && !text.contains(&rig.other.message_id),
        "an app lists its own messages and not another app's: {text}"
    );
    let own = reader
        .request("GET", "/messages/{id}")
        .params(&[rig.mine.message_id.as_str()])
        .send()
        .await;
    assert_eq!(own.status, 200, "{}", own.text());
    assert!(own.text().contains(&rig.mine.body_text));
    let theirs = reader
        .request("GET", "/messages/{id}")
        .params(&[rig.other.message_id.as_str()])
        .send()
        .await;
    assert_eq!(
        theirs.status,
        404,
        "another app's message is a 404 (Layer 1), not its body: {}",
        theirs.text()
    );
    // Receipts and parts stay person-only at Layer 1: an app that passes the
    // permission still sees no row of them.
    for path in ["/delivery_receipts", "/message_parts"] {
        let reply = reader.request("GET", path).send().await;
        assert_eq!(reply.status, 200, "{path}: {}", reply.text());
        assert_eq!(
            reply.json()["items"].as_array().map(Vec::len),
            Some(0),
            "{path}: an app reads none: {}",
            reply.text()
        );
    }

    for (label, scopes) in [("send-only", " sms:send "), ("no scope", " ")] {
        let api = rig.app_key(&rig.mine.app_id, scopes).await;
        for probe in &PROBES[..2] {
            let reply = call(&api, probe, &rig.mine).await;
            assert_refused_naming_sms_read(&reply, &rig, label, probe);
        }
    }
}

/// A `HEAD` is answered by the `GET` handler with the full `Content-Length`, so
/// before it was gated as the `GET` it was a way past every gate and an oracle on
/// the body (`HEAD /messages?body__startsWith=…`: the length differs by whether
/// anything matched). A role without `sms:read` is refused every probe as a
/// `HEAD`, and the refusal's length does not depend on the question asked.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_head_is_refused_where_the_get_is() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let role = custom_role(&rig.db, &["sms:send", "app:read"]).await;
    let api = rig.person_by_key(&role).await;
    for probe in &PROBES {
        let reply = call_as(&api, "HEAD", probe, &rig.mine).await;
        assert_eq!(
            reply.status,
            403,
            "HEAD {} -> {}",
            describe(probe),
            reply.text()
        );
    }

    let first = rig.mine.body_text.chars().take(6).collect::<String>();
    let length_of = |reply: &Reply| {
        reply
            .headers
            .get("content-length")
            .map(|value| value.to_str().unwrap_or_default().to_owned())
    };
    let hit = deliver(
        api.request("HEAD", "/messages")
            .query(&format!("body__startsWith={}", first.replace(' ', "%20"))),
        "HEAD",
    )
    .await;
    let miss = deliver(
        api.request("HEAD", "/messages")
            .query("body__startsWith=zzzzzz"),
        "HEAD",
    )
    .await;
    assert_eq!((hit.status, miss.status), (403, 403));
    assert_eq!(
        length_of(&hit),
        length_of(&miss),
        "a refused HEAD tells nothing about whether the body matched"
    );

    // The method works for a role that may read, so the 403s above are the gate's.
    let reader = rig.person_by_key("support").await;
    let ok = deliver(reader.request("HEAD", "/messages"), "HEAD").await;
    assert_eq!(ok.status, 200, "{}", ok.text());
}

/// `HEAD` walked past `JOB_READ_ROUTES` and `PROVIDER_ROUTE_READ_ROUTES` too,
/// for an app key (their real perimeter): refused as a `HEAD` now, and a key
/// holding the scope still passes.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_head_does_not_walk_past_the_job_and_provider_read_gates() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let bare = rig.app_key(&rig.mine.app_id, " sms:send ").await;
    for path in ["/jobs", "/providers", "/routes"] {
        let get = bare.request("GET", path).send().await;
        let head = deliver(bare.request("HEAD", path), "HEAD").await;
        assert_eq!(
            (get.status, head.status),
            (403, 403),
            "{path}: GET {} / HEAD {}",
            get.text(),
            head.text()
        );
    }
    let scoped = rig
        .app_key(&rig.mine.app_id, " sms:send job:read provider:read ")
        .await;
    for path in ["/jobs", "/providers"] {
        let head = deliver(scoped.request("HEAD", path), "HEAD").await;
        assert_eq!(head.status, 200, "{path}: {}", head.text());
    }
}

/// The doors that reach a message from another model are shut by the message
/// policy itself, not by a route gate: `?include=`, a relation filter and its
/// `totalCount`, and a nested include read `Message` under its own `list`
/// policy, which asks for the `messageRead` claim. Each is run for a caller
/// without `sms:read` and, as the positive control that proves the request is
/// well formed, for one with it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn another_models_include_and_filter_read_the_message_under_its_policy() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let mine = rig.mine.app_id.clone();
    let mine = mine.as_str();
    let body = rig.mine.body_text.clone();
    let body = body.as_str();

    // `developer` passes `App`'s read policy (it names roles) and is made to lack
    // `sms:read` the way an owner would in the role editor.
    set_role_permissions(
        &rig.db,
        "developer",
        &["app:read", "webhook:manage", "sms:send"],
    )
    .await;
    let stripped = rig.person_by_key("developer").await;
    let granted_role = custom_role(&rig.db, &["app:read", "sms:read"]).await;
    let granted = rig.person_by_key("owner").await;

    let include = "include=messages";
    let filter = "messages.some.msisdn=%2B237677123456";
    // The same questions, put to `/apps` (a role-named policy) and, for the
    // nested include, to `/consent_records` (any person).
    let consent = rig
        .db
        .consent_record()
        .create(schema::CreateConsentRecordInput {
            appId: mine.to_owned(),
            msisdnHash: format!("hmac-sha256-v1:consent-{}", suffix()),
            msisdn: "+237677123456".to_owned(),
            scope: MessageClass::otp,
            channel: schema::ConsentChannel::web_form,
            consentedAt: Utc::now(),
            evidenceRef: None,
        })
        .run(&sys())
        .await
        .expect("seeding a consent record");
    let _ = consent;
    let nested = "include=app,app.messages";
    let nobody = rig
        .person_by_key(&custom_role(&rig.db, &["app:read"]).await)
        .await;
    let _ = granted_role;

    // Controls: a caller who may read sees the message through every door.
    let with = granted.request("GET", "/apps").query(include).send().await;
    assert_eq!(with.status, 200, "{}", with.text());
    assert!(
        with.text().contains(body),
        "control, include: {}",
        with.text()
    );
    let with = granted.request("GET", "/apps").query(filter).send().await;
    assert!(
        with.text().contains(mine),
        "control, relation filter finds the app: {}",
        with.text()
    );
    let with = granted
        .request("GET", "/consent_records")
        .query(nested)
        .send()
        .await;
    assert_eq!(with.status, 200, "{}", with.text());
    assert!(
        with.text().contains(body),
        "control, nested: {}",
        with.text()
    );

    // The doors, for callers without `sms:read`.
    let shut = stripped.request("GET", "/apps").query(include).send().await;
    assert_eq!(shut.status, 200, "{}", shut.text());
    assert!(
        !shut.text().contains(body) && !shut.text().contains("+237677123456"),
        "include=messages leaked a message: {}",
        shut.text()
    );
    let shut = stripped.request("GET", "/apps").query(filter).send().await;
    assert_eq!(shut.status, 200, "{}", shut.text());
    assert!(
        !shut.text().contains(mine) && shut.json()["totalCount"] == 0,
        "a relation filter on a message column matched, or counted: {}",
        shut.text()
    );
    let shut = nobody
        .request("GET", "/consent_records")
        .query(nested)
        .send()
        .await;
    assert_eq!(shut.status, 200, "{}", shut.text());
    assert!(
        !shut.text().contains(body) && !shut.text().contains("+237677123456\",\"app\""),
        "a nested include leaked a message: {}",
        shut.text()
    );
    let shut = deliver(stripped.request("HEAD", "/apps").query(include), "HEAD").await;
    assert_eq!(shut.status, 200, "{}", shut.text());

    set_role_permissions(&rig.db, "developer", &DEVELOPER_PERMISSIONS).await;
}

/// `listMessagesPage` is a stub today (it answers with its milestone) and its
/// policy was `auth() != null`: the day it is implemented it would have been a
/// door. It asks for the claim now.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn list_messages_page_asks_for_the_claim() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let body = serde_json::json!({"args": {}});
    let denied_role = custom_role(&rig.db, &["app:read"]).await;
    let denied = rig.person_by_key(&denied_role).await;
    let reply = denied
        .request("POST", "/$procs/listMessagesPage")
        .json(&body)
        .send()
        .await;
    assert_eq!(reply.status, 403, "{}", reply.text());
    let allowed = rig.person_by_key("support").await;
    let reply = allowed
        .request("POST", "/$procs/listMessagesPage")
        .json(&body)
        .send()
        .await;
    assert_ne!(
        reply.status,
        403,
        "a caller with sms:read is not refused by the policy: {}",
        reply.text()
    );
}

/// A person reads opt-outs as any `user` (the owner's decision to take: no
/// built-in role holds `optout:read`); an app key reads them only with the
/// `optout:read` scope, because each row is a raw number and the model has no
/// owning app to scope by. Before, any app key of any tenant listed them all.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_app_key_reads_opt_outs_only_with_optout_read() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let number = format!(
        "+23767{:07}",
        u128::from_le_bytes(*cratestack::uuid::Uuid::new_v4().as_bytes()) % 10_000_000
    );
    let row = rig
        .db
        .opt_out()
        .create(schema::CreateOptOutInput {
            msisdnHash: format!("hmac-sha256-v1:optout-{}", suffix()),
            msisdn: number.clone(),
            source: schema::OptOutSource::inbound_stop,
            scope: "all".to_owned(),
            reason: None,
            optedOutAt: Utc::now(),
        })
        .run(&sys())
        .await
        .expect("seeding the opt-out");

    // An app of another tenant, with every scope but `optout:read`.
    let other_tenant = rig
        .app_key(&rig.other.app_id, " sms:send sms:read job:read ")
        .await;
    let list = other_tenant.request("GET", "/opt_outs").send().await;
    assert_eq!(list.status, 200, "{}", list.text());
    assert!(
        !list.text().contains(&number) && list.json()["items"].as_array().map(Vec::len) == Some(0),
        "an app without optout:read listed a number: {}",
        list.text()
    );
    let one = other_tenant
        .request("GET", "/opt_outs/{id}")
        .params(&[row.id.as_str()])
        .send()
        .await;
    assert_eq!(one.status, 404, "{}", one.text());

    let reader = rig.app_key(&rig.other.app_id, " optout:read ").await;
    let list = reader.request("GET", "/opt_outs").send().await;
    assert!(list.text().contains(&number), "control: {}", list.text());

    for role in ["support", "operator"] {
        let api = rig.person_by_key(role).await;
        let list = api.request("GET", "/opt_outs").send().await;
        assert!(
            list.text().contains(&number),
            "{role} still reads opt-outs: {}",
            list.text()
        );
    }
}
