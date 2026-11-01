//! Reading a message needs `sms:read`, over real HTTP: who can and who cannot
//! list or read a message, a receipt and a part, by a paired console key and by
//! a person's token, and that an app key still reads its own and only its own.
//!
//! `Message`'s `list`/`detail` policy admits any `user` (no role test), and a
//! role's permissions cannot be written into a policy, so the answer is Layer 2:
//! `router::MESSAGE_READ_ROUTES`. Anchors:
//!
//! - **a role is decided by its permission, not its name**: four built-in roles
//!   carry `sms:read` (`owner`, `admin`, `operator`, `auditor`) and read; two do
//!   not since `0013_role_read_scope` (`developer`, `support`) and are refused
//!   like any custom role without it; a custom role without it is refused on all
//!   six routes with a 403 that names the permission; a custom role holding only
//!   `sms:read` reads; `pending` (holds nothing) reads no message and nothing but the three
//!   collections open to any person;
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
//!   (`/app_clients?include=app,app.messages`) read `Message` under its own
//!   policy, which asks for the `messageRead` claim, so a caller without
//!   `sms:read` gets no message from them, each with a positive control;
//! - **opt-outs and consent records**: a person reads them only with `optout:read`
//!   or `optout:manage` (`auditor` holds the first since `0014_optout_read_scope`,
//!   `owner`, `admin`, `operator` and `support` the second; `developer` and
//!   `pending` neither); an app key reads opt-outs only with `optout:read` (its
//!   `optout:manage` scope does not count) and consent records only its own app's.
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

/// The built-in roles that hold `sms:read`: the ones `0013_role_read_scope`
/// leaves it with.
const READING_ROLES: [&str; 4] = ["owner", "admin", "operator", "auditor"];

/// The built-in roles `0013_role_read_scope` took it from, and the one it added
/// to hold nothing at all.
const NON_READING_ROLES: [&str; 3] = ["developer", "support", "pending"];

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

async fn rig() -> Rig {
    let db = db().await;
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

/// A bearer `GET`: the person who signed in with a password or single sign-on.
async fn bearer_get(base: String, token: String, path: String) -> (u16, String) {
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

/// What `0013_role_read_scope` and `0014_optout_read_scope` leave in the `roles`
/// table of a database that has run every migration, read through the generated client. The tests below rely
/// on it rather than set it, so a migration that stopped doing it fails here and
/// in every one of them, and not by being papered over by a fixture.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_migrated_seed_is_the_owners_decision() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let permissions = |key: &'static str| {
        let db = &db;
        async move {
            let rows = db
                .role()
                .find_many()
                .where_expr(FilterExpr::from(role::key().eq(key.to_owned())))
                .run(&sys())
                .await
                .expect("reading the role");
            assert_eq!(rows.len(), 1, "exactly one role keyed {key}");
            let row = rows.into_iter().next().expect("one");
            assert!(row.builtin, "{key} is built in");
            row.permissions
        }
    };
    for key in READING_ROLES {
        assert!(
            permissions(key).await.contains(" sms:read "),
            "{key} keeps sms:read"
        );
    }
    assert_eq!(
        permissions("developer").await,
        " app:read webhook:manage sms:send "
    );
    assert_eq!(
        permissions("support").await,
        " optout:manage delivery:read "
    );
    assert_eq!(
        permissions("pending").await,
        " ",
        "pending holds no permission at all"
    );
    // `0014_optout_read_scope`: the auditor reads opt-outs by `optout:read`, the
    // four roles that manage them by `optout:manage`; the others hold neither.
    assert_eq!(
        permissions("auditor").await,
        " sms:read app:read provider:read route:read job:read worker:read dashboard:read delivery:read audit:read optout:read ",
        "the auditor gains optout:read and nothing else"
    );
    for key in ["owner", "admin", "operator", "support"] {
        let held = permissions(key).await;
        assert!(held.contains(" optout:manage "), "{key} manages opt-outs");
        assert!(
            !held.contains(" optout:read "),
            "{key} is not given optout:read as well"
        );
    }
    for key in ["developer", "pending"] {
        assert!(
            !permissions(key).await.contains("optout:"),
            "{key} holds no opt-out permission"
        );
    }
}

/// Four built-in roles carry `sms:read` (section 5.2), so each reads, by a paired
/// key, on every route; `0013_role_read_scope` changed nobody's answer here.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_four_reading_roles_read_messages_with_their_console_key() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    for role in READING_ROLES {
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

/// `developer` and `support` hold no `sms:read` since `0013_role_read_scope`
/// (the owner's decision after #469): refused on every route a message can come
/// back by, exactly as a custom role without the permission is, and a refusal
/// never carries the message. `pending` holds nothing and is refused the same.
/// A developer reads its own app's messages through an app key with the scope
/// (`an_app_key_reads_its_own_messages_with_sms_read_and_not_without_it`).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn developer_and_support_and_pending_read_no_message_by_any_route() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    for role in NON_READING_ROLES {
        let api = rig.person_by_key(role).await;
        for probe in &PROBES {
            let reply = call(&api, probe, &rig.mine).await;
            assert_refused_naming_sms_read(&reply, &rig, role, probe);
            let head = call_as(&api, "HEAD", probe, &rig.mine).await;
            assert_eq!(
                head.status,
                403,
                "{role}: HEAD {} -> {}",
                describe(probe),
                head.text()
            );
        }
        // The webhook attempts' history is message data too.
        let attempts = api.request("GET", "/webhook_attempts").send().await;
        assert_eq!(attempts.status, 200, "{role}: {}", attempts.text());
        assert_eq!(
            attempts.json()["items"].as_array().map(Vec::len),
            Some(0),
            "{role} read a webhook attempt: {}",
            attempts.text()
        );
    }
}

/// `developer` passes `App`'s read policy (it names the role), so it reaches an
/// app's relation to `Message` by `?include=messages`, a relation filter and a
/// nested include, which read `Message` under its own policy and the claim.
/// Stripped of `sms:read` by the migration it gets the apps and no message, and
/// the relation filter neither matches nor counts one. The same requests as the
/// owner are the controls.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_developer_reaches_no_message_through_an_app() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let mine = rig.mine.app_id.clone();
    let body = rig.mine.body_text.clone();
    let developer = rig.person_by_key("developer").await;
    let owner = rig.person_by_key("owner").await;

    let include = "include=messages";
    let filter = "messages.some.msisdn=%2B237677123456";
    let with = owner.request("GET", "/apps").query(include).send().await;
    assert!(with.text().contains(&body), "control: {}", with.text());
    let shut = developer
        .request("GET", "/apps")
        .query(include)
        .send()
        .await;
    assert_eq!(shut.status, 200, "{}", shut.text());
    assert!(
        shut.text().contains(&mine),
        "the developer does read the apps themselves: {}",
        shut.text()
    );
    assert!(
        !shut.text().contains(&body) && !shut.text().contains("+237677123456"),
        "a developer's include=messages leaked a message: {}",
        shut.text()
    );
    let shut = developer.request("GET", "/apps").query(filter).send().await;
    assert!(
        !shut.text().contains(&mine) && shut.json()["totalCount"] == 0,
        "a developer's relation filter matched or counted a message: {}",
        shut.text()
    );
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

/// A person's token is the same person as their key: `operator` and `auditor`
/// read, `developer`, `support` and `pending` do not, a custom role without
/// `sms:read` does not, and `owner` and `admin` are not reachable by a token at
/// all (`DEVICE_REQUIRED`, VSA-4b), which this does not change.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_persons_token_gets_the_answer_their_role_gets() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let get = |token: String, path: String| bearer_get(rig.base.clone(), token, path);
    let detail = format!("/messages/{}", rig.mine.message_id);
    let read_but_for = custom_role(&rig.db, &["sms:send", "app:read"]).await;
    let read_only = custom_role(&rig.db, &["sms:read"]).await;

    for role in ["operator", "auditor"] {
        let token = rig.person_by_token(role).await;
        for path in ["/messages", detail.as_str(), "/delivery_receipts"] {
            let (status, body) = get(token.clone(), path.to_owned()).await;
            assert_eq!(status, 200, "{role}: {path} -> {body}");
        }
    }
    // Since 0013_role_read_scope `developer` and `support` (the role a first
    // sign-in was created in) and `pending` (the role one is created in now)
    // are the same as a custom role without the permission.
    for role in NON_READING_ROLES {
        let token = rig.person_by_token(role).await;
        for path in ["/messages", detail.as_str(), "/delivery_receipts"] {
            let (status, body) = get(token.clone(), path.to_owned()).await;
            assert_eq!(status, 403, "{role}: {path} -> {body}");
            assert!(body.contains("sms:read"), "{role}: {path} -> {body}");
            assert!(
                !body.contains(&rig.mine.body_text),
                "{role}: {path} -> {body}"
            );
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
    let reader = rig.person_by_key("operator").await;
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

    // `developer` passes `App`'s read policy (it names roles) and holds no
    // `sms:read` (0013_role_read_scope).
    let stripped = rig.person_by_key("developer").await;
    let granted = rig.person_by_key("owner").await;

    let include = "include=messages";
    let filter = "messages.some.msisdn=%2B237677123456";
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
    let shut = deliver(stripped.request("HEAD", "/apps").query(include), "HEAD").await;
    assert_eq!(shut.status, 200, "{}", shut.text());
}

/// A nested include (`/app_clients?include=app,app.messages`) reaches a message
/// through two relations. It runs as a caller that LOADS `app`: `App`'s read
/// policy names built-in roles, so a custom role would get `app: null` and
/// `app.messages` would never be reached whatever `Message`'s policy says. The
/// `developer` (no `sms:read` since 0013) does load it, and reads `AppClient` by
/// role, so the app object is asserted present and an empty `messages` under it
/// is the message policy's doing: this test fails when that policy is reverted.
/// (It was `/consent_records` until `0014_optout_read_scope`, which a developer,
/// holding no opt-out permission, no longer reads.)
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_nested_include_reads_the_message_under_its_policy() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let mine = rig.mine.app_id.clone();
    let mine = mine.as_str();
    let body = rig.mine.body_text.clone();
    let body = body.as_str();
    let stripped = rig.person_by_key("developer").await;
    let granted = rig.person_by_key("owner").await;
    // The app's own key row: an `AppClient` of `mine` to include through.
    rig.app_key(mine, " sms:send ").await;
    let nested = format!("include=app,app.messages&appId={mine}");

    // Control: a caller who may read sees the message through the nested include.
    let with = granted
        .request("GET", "/app_clients")
        .query(&nested)
        .send()
        .await;
    assert_eq!(with.status, 200, "{}", with.text());
    assert!(
        with.text().contains(body),
        "control, nested: {}",
        with.text()
    );

    let shut = stripped
        .request("GET", "/app_clients")
        .query(&nested)
        .send()
        .await;
    assert_eq!(shut.status, 200, "{}", shut.text());
    let record = shut.json()["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["appId"] == mine))
        .cloned()
        .expect("the seeded app client is listed");
    assert_eq!(
        record["app"]["id"], mine,
        "the include loaded the app, so the probe reaches app.messages: {record}"
    );
    assert!(
        !shut.text().contains(body) && !shut.text().contains(&rig.mine.message_id),
        "a nested include leaked a message: {}",
        shut.text()
    );
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
    let allowed = rig.person_by_key("operator").await;
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

/// One opt-out and one consent record (of `rig.mine`'s app) on the same number.
struct OptFixture {
    number: String,
    opt_out_id: String,
    opt_out_hash: String,
    consent_id: String,
    consent_hash: String,
}

async fn seed_opt_out_and_consent(rig: &Rig) -> OptFixture {
    let number = format!(
        "+23767{:07}",
        u128::from_le_bytes(*cratestack::uuid::Uuid::new_v4().as_bytes()) % 10_000_000
    );
    let opt_out_hash = format!("hmac-sha256-v1:optout-{}", suffix());
    let consent_hash = format!("hmac-sha256-v1:consent-{}", suffix());
    let opt_out = rig
        .db
        .opt_out()
        .create(schema::CreateOptOutInput {
            msisdnHash: opt_out_hash.clone(),
            msisdn: number.clone(),
            source: schema::OptOutSource::inbound_stop,
            scope: "all".to_owned(),
            reason: None,
            optedOutAt: Utc::now(),
        })
        .run(&sys())
        .await
        .expect("seeding the opt-out");
    let consent = rig
        .db
        .consent_record()
        .create(schema::CreateConsentRecordInput {
            appId: rig.mine.app_id.clone(),
            msisdnHash: consent_hash.clone(),
            msisdn: number.clone(),
            scope: MessageClass::otp,
            channel: schema::ConsentChannel::web_form,
            consentedAt: Utc::now(),
            evidenceRef: None,
        })
        .run(&sys())
        .await
        .expect("seeding the consent record");
    OptFixture {
        number,
        opt_out_id: opt_out.id,
        opt_out_hash,
        consent_id: consent.id,
        consent_hash,
    }
}

/// A caller by either door: a paired key (sealed) or a person's bearer token.
enum Caller<'a> {
    Key(&'a SealedClient),
    Token { base: &'a str, token: &'a str },
}

impl Caller<'_> {
    /// `GET` of `collection` (`/opt_outs`), one row when `id` is given, with an
    /// optional query. Returns the status and the answer's text.
    async fn get(&self, collection: &str, id: Option<&str>, query: Option<&str>) -> (u16, String) {
        match self {
            Self::Key(api) => {
                let one = format!("{collection}/{{id}}");
                let mut request = match id {
                    Some(id) => api.request("GET", &one).params(&[id]),
                    None => api.request("GET", collection),
                };
                if let Some(query) = query {
                    request = request.query(query);
                }
                let reply = request.send().await;
                (reply.status, reply.text())
            }
            Self::Token { base, token } => {
                let mut path = collection.to_owned();
                if let Some(id) = id {
                    path = format!("{path}/{id}");
                }
                if let Some(query) = query {
                    path = format!("{path}?{query}");
                }
                bearer_get((*base).to_owned(), (*token).to_owned(), path).await
            }
        }
    }

    /// The `HEAD` of one row: its status. Only a key can seal one.
    async fn head_status(&self, collection: &str, id: &str) -> Option<u16> {
        match self {
            Self::Key(api) => Some(
                deliver(
                    api.request("HEAD", &format!("{collection}/{{id}}"))
                        .params(&[id]),
                    "HEAD",
                )
                .await
                .status,
            ),
            Self::Token { .. } => None,
        }
    }
}

fn items_of(body: &str) -> (usize, serde_json::Value) {
    let json: serde_json::Value = serde_json::from_str(body).expect("a JSON list");
    (
        json["items"].as_array().map_or(usize::MAX, Vec::len),
        json["totalCount"].clone(),
    )
}

/// `who` reads `collection` (opt-outs or consent records): the list filtered to
/// the seeded row finds it, the row is a 200, a `HEAD` of it is a 200.
async fn assert_reads(
    who: &str,
    caller: &Caller<'_>,
    collection: &str,
    id: &str,
    hash: &str,
    number: &str,
) {
    let (status, body) = caller
        .get(collection, None, Some(&format!("msisdnHash={hash}")))
        .await;
    assert_eq!(status, 200, "{who}: GET {collection}?msisdnHash -> {body}");
    let (items, total) = items_of(&body);
    assert!(
        body.contains(number) && items == 1 && total.as_i64().is_some_and(|t| t >= 1),
        "{who} should read {collection}: {body}"
    );
    let (status, body) = caller.get(collection, Some(id), None).await;
    assert_eq!(status, 200, "{who}: GET {collection}/{{id}} -> {body}");
    assert!(body.contains(number), "{who}: {collection} detail: {body}");
    if let Some(head) = caller.head_status(collection, id).await {
        assert_eq!(head, 200, "{who}: HEAD {collection}/{{id}}");
    }
}

/// `who` reads nothing of `collection`: a 200 and an empty list (`totalCount`
/// 0) however it is asked, a 404 for the row, by `GET` and by `HEAD`, and the
/// number appears nowhere.
async fn assert_refused(
    who: &str,
    caller: &Caller<'_>,
    collection: &str,
    id: &str,
    hash: &str,
    number: &str,
) {
    for query in [None, Some(format!("msisdnHash={hash}"))] {
        let (status, body) = caller.get(collection, None, query.as_deref()).await;
        assert_eq!(status, 200, "{who}: GET {collection} {query:?} -> {body}");
        let (items, total) = items_of(&body);
        assert!(
            !body.contains(number) && items == 0 && total == 0,
            "{who} read {collection} {query:?}: {body}"
        );
    }
    let (status, body) = caller.get(collection, Some(id), None).await;
    assert_eq!(status, 404, "{who}: GET {collection}/{{id}} -> {body}");
    assert!(!body.contains(number), "{who}: {body}");
    if let Some(head) = caller.head_status(collection, id).await {
        assert_eq!(head, 404, "{who}: HEAD {collection}/{{id}}");
    }
}

impl OptFixture {
    async fn assert_reads_both(&self, who: &str, caller: &Caller<'_>) {
        assert_reads(
            who,
            caller,
            "/opt_outs",
            &self.opt_out_id,
            &self.opt_out_hash,
            &self.number,
        )
        .await;
        assert_reads(
            who,
            caller,
            "/consent_records",
            &self.consent_id,
            &self.consent_hash,
            &self.number,
        )
        .await;
    }

    async fn assert_refused_both(&self, who: &str, caller: &Caller<'_>) {
        assert_refused(
            who,
            caller,
            "/opt_outs",
            &self.opt_out_id,
            &self.opt_out_hash,
            &self.number,
        )
        .await;
        assert_refused(
            who,
            caller,
            "/consent_records",
            &self.consent_id,
            &self.consent_hash,
            &self.number,
        )
        .await;
    }
}

/// Every permission a built-in role holds except the two that read opt-outs: a
/// role with all of them is still refused, so the permission decides and not
/// how powerful the role is.
const EVERYTHING_BUT_OPT_OUT: [&str; 20] = [
    "sms:read",
    "sms:send",
    "message:cancel",
    "app:read",
    "app:write",
    "client:provision",
    "provider:read",
    "provider:update",
    "route:read",
    "route:write",
    "sender:manage",
    "webhook:manage",
    "job:read",
    "job:enqueue",
    "worker:read",
    "dashboard:read",
    "audit:read",
    "delivery:read",
    "user:manage",
    "role:manage",
];

/// A person reads `OptOut` and `ConsentRecord` only with `optout:read` or
/// `optout:manage` (the claim `optOutRead`): the roles that manage opt-outs
/// (`owner`, `admin`, `operator`, `support`) and the auditor, which
/// `0014_optout_read_scope` gave `optout:read`, read them; a custom role holding
/// either permission reads them; `developer`, `pending` and a custom role with
/// every other permission get an empty list and a 404, by key and by token.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_person_reads_opt_outs_and_consent_records_only_with_an_opt_out_permission() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let fx = seed_opt_out_and_consent(&rig).await;

    // `owner` and `admin` are device-only: a paired key, never a token.
    for role in ["owner", "admin"] {
        let api = rig.person_by_key(role).await;
        fx.assert_reads_both(&format!("{role} by key"), &Caller::Key(&api))
            .await;
    }
    for role in ["support", "operator", "auditor"] {
        let api = rig.person_by_key(role).await;
        fx.assert_reads_both(&format!("{role} by key"), &Caller::Key(&api))
            .await;
        let token = rig.person_by_token(role).await;
        fx.assert_reads_both(
            &format!("{role} by token"),
            &Caller::Token {
                base: &rig.base,
                token: &token,
            },
        )
        .await;
    }
    for (label, permissions) in [
        ("a custom role holding optout:read", &["optout:read"]),
        ("a custom role holding optout:manage", &["optout:manage"]),
    ] {
        let role = custom_role(&rig.db, permissions).await;
        let api = rig.person_by_key(&role).await;
        fx.assert_reads_both(&format!("{label} by key"), &Caller::Key(&api))
            .await;
        let token = rig.person_by_token(&role).await;
        fx.assert_reads_both(
            &format!("{label} by token"),
            &Caller::Token {
                base: &rig.base,
                token: &token,
            },
        )
        .await;
    }

    let everything_else = custom_role(&rig.db, &EVERYTHING_BUT_OPT_OUT).await;
    for role in ["developer", "pending", everything_else.as_str()] {
        let who = if role == everything_else {
            "a custom role with every other permission"
        } else {
            role
        };
        let api = rig.person_by_key(role).await;
        fx.assert_refused_both(&format!("{who} by key"), &Caller::Key(&api))
            .await;
        let token = rig.person_by_token(role).await;
        fx.assert_refused_both(
            &format!("{who} by token"),
            &Caller::Token {
                base: &rig.base,
                token: &token,
            },
        )
        .await;
    }
}

/// An app key reads opt-outs only with the `optout:read` scope (each row is a
/// raw number and the model has no owning app to scope by), and an app's
/// `optout:manage` scope does not count: `recordOptOut` admits person roles only,
/// so counting it would make a scope that does nothing a read of every tenant's
/// numbers. Consent records are the app's own whatever its scope, and the scope
/// does not widen that to another tenant's.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_app_key_reads_opt_outs_only_with_optout_read_and_its_own_consent_records() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let fx = seed_opt_out_and_consent(&rig).await;

    // Another tenant, with every scope but the two opt-out ones.
    let neither = rig
        .app_key(&rig.other.app_id, " sms:send sms:read job:read ")
        .await;
    fx.assert_refused_both("another tenant without optout:read", &Caller::Key(&neither))
        .await;

    // The scope that would manage them reads nothing.
    let manager = rig.app_key(&rig.other.app_id, " optout:manage ").await;
    fx.assert_refused_both("another tenant with optout:manage", &Caller::Key(&manager))
        .await;
    // A look-alike is not the scope.
    let lookalike = rig.app_key(&rig.other.app_id, " optout:readall ").await;
    fx.assert_refused_both(
        "another tenant with optout:readall",
        &Caller::Key(&lookalike),
    )
    .await;

    // `optout:read` reads every opt-out, and none of another app's consent.
    let reader = rig.app_key(&rig.other.app_id, " optout:read ").await;
    assert_reads(
        "another tenant with optout:read",
        &Caller::Key(&reader),
        "/opt_outs",
        &fx.opt_out_id,
        &fx.opt_out_hash,
        &fx.number,
    )
    .await;
    assert_refused(
        "another tenant with optout:read",
        &Caller::Key(&reader),
        "/consent_records",
        &fx.consent_id,
        &fx.consent_hash,
        &fx.number,
    )
    .await;

    // The app that holds the consent record reads it with any scope, and the
    // opt-out only with the opt-out scope.
    let owner_app = rig.app_key(&rig.mine.app_id, " sms:send ").await;
    assert_reads(
        "the consenting app with sms:send",
        &Caller::Key(&owner_app),
        "/consent_records",
        &fx.consent_id,
        &fx.consent_hash,
        &fx.number,
    )
    .await;
    assert_refused(
        "the consenting app with sms:send",
        &Caller::Key(&owner_app),
        "/opt_outs",
        &fx.opt_out_id,
        &fx.opt_out_hash,
        &fx.number,
    )
    .await;
}

/// A webhook endpoint of `app_id` (the full number in its payloads) and one
/// attempt at it; returns the attempt's id.
async fn seed_attempt(rig: &Rig, app_id: &str, number: &str) -> String {
    let endpoint = rig
        .db
        .webhook_endpoint()
        .create(schema::CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: format!("https://example.test/webhooks/{}", suffix()),
            deliveryMode: schema::WebhookDeliveryMode::hmac,
            audience: None,
            eventTypes: " message.delivered ".to_owned(),
            secret: Some(format!("whsec_{}{}{}", suffix(), suffix(), suffix())),
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: false,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the endpoint");
    let attempt = rig
        .db
        .webhook_attempt()
        .create(schema::CreateWebhookAttemptInput {
            endpointId: endpoint.id,
            sourceEventId: cratestack::uuid::Uuid::new_v4(),
            aggregateId: format!("message-read-{}", suffix()),
            eventType: "message.delivered".to_owned(),
            payload: format!("{{\"to\":\"{number}\",\"state\":\"delivered\"}}"),
            leaseOwner: None,
            leaseUntil: None,
            nextAttemptAt: None,
            lastStatusCode: None,
            lastError: None,
            lastAttemptAt: None,
            deliveredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the attempt");
    attempt.id
}

/// A webhook attempt's `payload` is message data (`messageId`, `appId`,
/// `clientRef`, `state`, `operator`, `to`, which is the full number when the
/// endpoint's `maskRecipient` is false), so a person reads attempts only with
/// `sms:read`, as for the message. An app keeps reading its own endpoints'
/// attempts and not another tenant's.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_webhook_attempt_is_message_data_and_asks_for_the_claim() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let number = "+237677123456";

    let attempts = [
        seed_attempt(&rig, &rig.mine.app_id, number).await,
        seed_attempt(&rig, &rig.other.app_id, number).await,
    ];
    let (mine, theirs) = (attempts[0].as_str(), attempts[1].as_str());

    // A person without `sms:read`: no attempt, no number.
    let role = custom_role(&rig.db, &["webhook:manage", "app:read"]).await;
    let nobody = rig.person_by_key(&role).await;
    let list = nobody.request("GET", "/webhook_attempts").send().await;
    assert_eq!(list.status, 200, "{}", list.text());
    assert!(
        !list.text().contains(mine) && !list.text().contains(number),
        "a person without sms:read read an attempt: {}",
        list.text()
    );
    let one = nobody
        .request("GET", "/webhook_attempts/{id}")
        .params(&[mine])
        .send()
        .await;
    assert_eq!(one.status, 404, "{}", one.text());

    // Control: a person with it reads both tenants' attempts.
    let reader = rig.person_by_key("operator").await;
    let list = reader.request("GET", "/webhook_attempts").send().await;
    assert!(
        list.text().contains(mine) && list.text().contains(theirs) && list.text().contains(number),
        "control: {}",
        list.text()
    );

    // An app reads its own endpoints' attempts, with no scope for it, and not
    // another tenant's.
    let app = rig.app_key(&rig.mine.app_id, " sms:send ").await;
    let list = app.request("GET", "/webhook_attempts").send().await;
    assert_eq!(list.status, 200, "{}", list.text());
    assert!(
        list.text().contains(mine) && !list.text().contains(theirs),
        "an app reads its own attempts and only those: {}",
        list.text()
    );
    let other = app
        .request("GET", "/webhook_attempts/{id}")
        .params(&[theirs])
        .send()
        .await;
    assert_eq!(other.status, 404, "{}", other.text());
    let own = app
        .request("GET", "/webhook_attempts/{id}")
        .params(&[mine])
        .send()
        .await;
    assert_eq!(own.status, 200, "{}", own.text());
}

/// What a `pending` person can still read, and why each is not message data
/// or a phone number. Every other collection a person can GET is a 403 or an
/// empty page for it. Each entry must really be readable (below), so closing one
/// fails the test until the entry is deleted: an allowance that matches nothing
/// is stale.
const PENDING_CAN_READ: [(&str, &str); 1] = [(
    "/roles",
    "the role table (keys, labels, permission sets) is readable by any person: not message data",
)];

/// `pending` holds no permission, so a first sign-in reads no message: every
/// collection a person can GET is a 403 or an empty page for it, except the role
/// table above (which any signed-in person reads, and which holds no phone
/// number) and the person's own paired console keys. Opt-outs and consent records
/// are in the list of empty pages since `0014_optout_read_scope`: they are raw
/// numbers, and `pending` holds neither opt-out permission. Run by a paired key and by a bearer token, which are the same
/// person, over every `GET` collection route the schema generated, with rows
/// seeded behind the ones that could hold something.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_pending_person_reads_nothing_else_by_any_collection_route() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let number = "+237677123456";
    rig.db
        .opt_out()
        .create(schema::CreateOptOutInput {
            msisdnHash: format!("hmac-sha256-v1:optout-{}", suffix()),
            msisdn: number.to_owned(),
            source: schema::OptOutSource::inbound_stop,
            scope: "all".to_owned(),
            reason: None,
            optedOutAt: Utc::now(),
        })
        .run(&sys())
        .await
        .expect("seeding the opt-out");
    rig.db
        .consent_record()
        .create(schema::CreateConsentRecordInput {
            appId: rig.mine.app_id.clone(),
            msisdnHash: format!("hmac-sha256-v1:consent-{}", suffix()),
            msisdn: number.to_owned(),
            scope: MessageClass::otp,
            channel: schema::ConsentChannel::web_form,
            consentedAt: Utc::now(),
            evidenceRef: None,
        })
        .run(&sys())
        .await
        .expect("seeding the consent record");

    let paths: Vec<&'static str> = sms_api::route_table()
        .into_iter()
        .filter(|(method, path)| *method == "GET" && !path.contains('{'))
        .map(|(_, path)| path)
        .collect();
    assert!(
        paths.len() >= 20 && paths.contains(&"/messages") && paths.contains(&"/opt_outs"),
        "the schema's GET collections: {paths:?}"
    );

    let key = rig.person_by_key("pending").await;
    let token = rig.person_by_token("pending").await;
    let mut seen_open: Vec<&str> = Vec::new();
    for path in &paths {
        let by_key = key.request("GET", path).send().await;
        let by_token = bearer_get(rig.base.clone(), token.clone(), (*path).to_owned()).await;
        for (how, status, body) in [
            ("key", by_key.status, by_key.text()),
            ("token", by_token.0, by_token.1),
        ] {
            for leaked in [
                rig.mine.body_text.as_str(),
                rig.mine.message_id.as_str(),
                rig.other.message_id.as_str(),
            ] {
                assert!(
                    !body.contains(leaked),
                    "pending by {how}: GET {path} carried message data: {body}"
                );
            }
            if status == 403 {
                continue;
            }
            assert_eq!(status, 200, "pending by {how}: GET {path} -> {body}");
            let json: serde_json::Value = serde_json::from_str(&body).expect("a JSON answer");
            let rows = json["items"]
                .as_array()
                .or_else(|| json.as_array())
                .map_or(0, Vec::len);
            if let Some((_, reason)) = PENDING_CAN_READ.iter().find(|(open, _)| open == path) {
                assert!(
                    rows > 0,
                    "{path} ({reason}) is no longer readable: delete its entry"
                );
                seen_open.push(path);
            } else if *path == "/console_keys" {
                // Only the person's own paired key, on the key run; a token has none.
                assert!(rows <= 1, "pending by {how}: other people's keys: {body}");
            } else {
                assert_eq!(
                    rows, 0,
                    "pending by {how} read {rows} row(s) of {path}: {body}"
                );
            }
        }
    }
    for (open, reason) in PENDING_CAN_READ {
        assert!(
            seen_open.contains(&open),
            "{open} ({reason}) was never read"
        );
    }
}

/// A developer has no `sms:read` since `0013_role_read_scope`, so it cannot see an
/// attempt to replay it either: `replayWebhookAttempt` reads the attempt as the
/// caller first (`@authorize`), and the attempt's policy asks for the claim. An
/// admin still replays.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_developer_cannot_replay_a_webhook_attempt_but_an_admin_can() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let mine = seed_attempt(&rig, &rig.mine.app_id, "+237677123456").await;
    let mine = mine.as_str();
    let developer = rig.person_by_key("developer").await;
    let replay = |attempt: &str| serde_json::json!({"args": {"attemptId": attempt}});
    let refused = developer
        .request("POST", "/$procs/replayWebhookAttempt")
        .json(&replay(mine))
        .send()
        .await;
    assert_eq!(refused.status, 403, "{}", refused.text());
    let admin = rig.person_by_key("admin").await;
    let allowed = admin
        .request("POST", "/$procs/replayWebhookAttempt")
        .json(&replay(mine))
        .send()
        .await;
    assert_ne!(
        allowed.status,
        403,
        "an admin passes the policy and Layer 2: {}",
        allowed.text()
    );
}
