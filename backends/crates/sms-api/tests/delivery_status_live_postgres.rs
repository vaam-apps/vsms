//! `lookupDeliveryStatus` against a real, fully migrated Postgres, over real HTTP:
//! sealed calls through `sms_api::router`, so Layer 1 (`@allow`), Layer 2
//! (`require_permission`) and the body's own scoping all run, and the roles are the
//! ones the migrations seeded, not fixtures.
//!
//! - **who may ask**: `support` and `auditor` hold `delivery:read` (and a custom role
//!   holding only that); `owner`, `admin` and `operator` do not (the owner's decision:
//!   delivery status is a support tool), nor do `developer`, `pending` or a custom role
//!   without it. `support` holds no `sms:read` since 0013, so `listMessageReceipts`
//!   refuses it and the lookup does not;
//! - **what it answers**: state, timestamps and the newest receipt, found by id, by
//!   an app's client reference or by the whole number, newest first, and never a body,
//!   a hash, a full number, a cost or a provider's own text;
//! - **whose messages**: an app key with the scope sees its own app and none other;
//!   a foreign id is an empty answer, not an error;
//! - **the number oracle**: equality on the whole number only, one audit row per
//!   answered call holding a hash and never the number, and the 61st number lookup
//!   in an hour refused;
//! - **fail closed**: a lookup whose audit row cannot be written is not answered.
//!
//! ```bash
//! cargo test -p sms-api --test delivery_status_live_postgres -- --ignored --test-threads=1
//! ```

#[path = "support/sealed_fixtures.rs"]
mod sealed_fixtures;

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sealed_fixtures::{
    api_router, client, owner, register_app_key, seed_user_with_console_key, serve,
    unique_mtn_msisdn,
};
use serde_json::{Value, json};
use sms_api::auth::{Principal, PrincipalKind, system_context};
use sms_api::schema::{
    self, Cratestack, Encoding, MessageClass, MessageState, OperatorCode, UpdateMessageInput,
    delivery_lookup, role,
};
use sms_api::{HashPepper, audit_log, delivery_status, hmac_sha256_hex};
use sms_test_support::sealed::{Reply, SealedClient, TestKey};

/// One test at a time: Postgres's `pg_type` catalog races on a fresh database.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Names a message must never show the person who looked it up, as JSON keys.
const FORBIDDEN_KEYS: [&str; 15] = [
    "body",
    "bodyHash",
    "bodyLength",
    "msisdn",
    "msisdnHash",
    "costXaf",
    "stateReason",
    "clientRef",
    "idempotencyKey",
    "providerMessageRef",
    "providerMessageRefAlt",
    "rawPayload",
    "routeId",
    "excludedRouteIds",
    "leaseOwner",
];

fn sys() -> CratestackContext {
    system_context("delivery-status-live-test")
}

fn tag() -> String {
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

/// A pepper made for this run, long enough for `HashPepper::new`.
fn run_pepper() -> String {
    format!(
        "{}-{}",
        cratestack::uuid::Uuid::new_v4().simple(),
        cratestack::uuid::Uuid::new_v4().simple()
    )
}

/// One message to seed, with the private things a lookup must not show filled in
/// with values that are random per run, so a leak is a string match.
struct Spec {
    msisdn: String,
    client_ref: Option<String>,
    body: String,
    state_reason: Option<String>,
    purged: bool,
}

impl Spec {
    fn to(msisdn: &str) -> Self {
        Self {
            msisdn: msisdn.to_owned(),
            client_ref: None,
            body: format!("secret-body-{}", tag()),
            state_reason: Some(format!("provider-said-{}", tag())),
            purged: false,
        }
    }
}

struct Rig {
    db: Cratestack,
    base: String,
    pepper: String,
    provider_id: String,
    app_a: String,
    app_b: String,
}

impl Rig {
    async fn new() -> Self {
        let db = db().await;
        let pepper = run_pepper();
        let base = serve(api_router(&db, &pepper)).await;
        let provider_id = db
            .provider()
            .create(schema::CreateProviderInput {
                key: format!("dlv_{}", tag()),
                displayName: "delivery status test provider".to_owned(),
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
            .expect("seeding a provider")
            .id;
        let app_a = Self::app(&db).await;
        let app_b = Self::app(&db).await;
        Self {
            db,
            base,
            pepper,
            provider_id,
            app_a,
            app_b,
        }
    }

    async fn app(db: &Cratestack) -> String {
        db.app()
            .create(schema::CreateAppInput {
                name: "delivery status test app".to_owned(),
                slug: format!("dlv-{}", tag()),
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

    fn hash(&self, e164: &str) -> String {
        hmac_sha256_hex(&HashPepper::new(&self.pepper).expect("pepper"), e164)
    }

    async fn message(&self, app_id: &str, spec: &Spec) -> schema::Message {
        self.db
            .message()
            .create(schema::CreateMessageInput {
                appId: app_id.to_owned(),
                clientRef: spec.client_ref.clone(),
                idempotencyKey: spec.client_ref.clone(),
                msisdn: if spec.purged {
                    "+000000000000".to_owned()
                } else {
                    spec.msisdn.clone()
                },
                msisdnHash: self.hash(&spec.msisdn),
                operator: OperatorCode::mtn,
                senderIdValue: "VYMALO".to_owned(),
                class: MessageClass::otp,
                priority: 1000,
                body: Some(spec.body.clone()),
                bodyHash: format!("hmac-sha256-v1:body-hash-{}", tag()),
                bodyLength: 17,
                encoding: Encoding::gsm7,
                segments: 1,
                stateReason: spec.state_reason.clone(),
                routeId: None,
                providerId: None,
                providerMessageRef: Some(format!("provider-ref-{}", tag())),
                providerMessageRefAlt: None,
                maxAttempts: 3,
                leaseOwner: None,
                leaseUntil: None,
                scheduledAt: None,
                expiresAt: Utc::now() + Duration::hours(1),
                submittedAt: None,
                finalizedAt: None,
                purgedAt: spec.purged.then(Utc::now),
                excludedRouteIds: None,
            })
            .run(&sys())
            .await
            .expect("seeding the message")
    }

    /// accepted -> queued -> routed -> submitted -> delivered.
    async fn deliver(&self, message: schema::Message) -> schema::Message {
        let mut current = message;
        for state in [
            MessageState::queued,
            MessageState::routed,
            MessageState::submitted,
            MessageState::delivered,
        ] {
            current = self
                .db
                .message()
                .update(current.id.clone())
                .set(UpdateMessageInput {
                    state: Some(state),
                    providerId: Some(Some(self.provider_id.clone())),
                    ..Default::default()
                })
                .if_match(current.version)
                .run(&sys())
                .await
                .expect("walking the state machine");
        }
        current
    }

    async fn receipt(&self, message_id: &str, outcome: schema::DeliveryOutcome, raw: &str) {
        self.db
            .delivery_receipt()
            .create(schema::CreateDeliveryReceiptInput {
                messageId: message_id.to_owned(),
                providerId: self.provider_id.clone(),
                providerMessageRef: format!("receipt-ref-{}", tag()),
                outcome,
                rawStatus: raw.to_owned(),
                errorCode: None,
                networkCode: OperatorCode::mtn,
                occurredAt: None,
                rawPayload: format!("{{\"raw-payload-marker\":\"{raw}\"}}"),
            })
            .run(&sys())
            .await
            .expect("seeding a receipt");
    }

    /// A person in `role`, reaching the router with a paired console key.
    async fn person(&self, role: &str) -> SealedClient {
        let key = TestKey::console();
        seed_user_with_console_key(&self.db, role, &key).await;
        client(&self.base, &key)
    }

    async fn custom_role(&self, permissions: &[&str]) -> String {
        let key = format!("dlv_{}", tag());
        self.db
            .role()
            .create(schema::CreateRoleInput {
                key: key.clone(),
                label: format!("delivery status test role {key}"),
                description: None,
                permissions: sms_core::pack(permissions).expect("no space in a literal"),
            })
            .run(&owner())
            .await
            .expect("seeding the custom role");
        key
    }

    /// An app key of `app_id` holding exactly `scopes`.
    async fn app_key(&self, app_id: &str, scopes: &str) -> SealedClient {
        let row = self
            .db
            .app_client()
            .create(schema::CreateAppClientInput {
                appId: app_id.to_owned(),
                clientId: format!("appc_dlv_{}", tag()),
                label: "delivery status test client".to_owned(),
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

    /// The `DeliveryLookup` rows whose `keyValue` is exactly `key_value`.
    async fn lookups(&self, key_value: &str) -> Vec<schema::DeliveryLookup> {
        self.db
            .delivery_lookup()
            .find_many()
            .where_expr(FilterExpr::from(
                delivery_lookup::keyValue().eq(key_value.to_owned()),
            ))
            .run(&sys())
            .await
            .expect("reading lookups")
    }
}

async fn lookup(who: &SealedClient, args: Value) -> Reply {
    who.request("POST", "/$procs/lookupDeliveryStatus")
        .json(&json!({ "args": args }))
        .send()
        .await
}

/// Every key anywhere in `value`.
fn keys_of(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                out.push(key.clone());
                keys_of(inner, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| keys_of(item, out)),
        _ => {}
    }
}

/// A one-element answer.
fn one(id: &str) -> Vec<String> {
    vec![id.to_owned()]
}

fn statuses(reply: &Reply) -> Vec<Value> {
    assert_eq!(reply.status, 200, "{}", reply.text());
    reply.json()["statuses"]
        .as_array()
        .expect("statuses")
        .clone()
}

fn ids(reply: &Reply) -> Vec<String> {
    statuses(reply)
        .iter()
        .map(|status| status["messageId"].as_str().expect("messageId").to_owned())
        .collect()
}

fn assert_refused_naming_delivery_read(reply: &Reply, who: &str) {
    let text = reply.text();
    assert_eq!(reply.status, 403, "{who}: {text}");
    assert!(text.contains("delivery:read"), "{who}: {text}");
}

fn permissions_text(permissions: &str) -> Vec<&str> {
    permissions.split_whitespace().collect()
}

/// What the migrations left in the `roles` table: read, never set, so a migration
/// that stopped doing it fails here and in every test below.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_migrated_seed_grants_delivery_read_to_support_and_auditor_alone() {
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
            permissions_text(&rows[0].permissions)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<String>>()
        }
    };
    for key in ["support", "auditor"] {
        assert!(
            permissions(key).await.contains(&"delivery:read".to_owned()),
            "{key} holds delivery:read"
        );
    }
    for key in ["owner", "admin", "operator", "developer", "pending"] {
        assert!(
            !permissions(key).await.contains(&"delivery:read".to_owned()),
            "{key} does not hold delivery:read: delivery status is a support tool"
        );
    }
    // The shape the whole suite leans on: support lost sms:read, auditor kept it,
    // operator holds it without delivery:read.
    assert!(
        !permissions("support")
            .await
            .contains(&"sms:read".to_owned())
    );
    assert!(
        permissions("auditor")
            .await
            .contains(&"sms:read".to_owned())
    );
    assert!(
        permissions("operator")
            .await
            .contains(&"sms:read".to_owned())
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn support_cannot_read_receipts_and_can_look_up_the_status() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let number = unique_mtn_msisdn();
    let message = rig.message(&rig.app_a, &Spec::to(&number)).await;
    rig.receipt(&message.id, schema::DeliveryOutcome::delivered, "Delivered")
        .await;
    let support = rig.person("support").await;

    // The receipts procedure is gated on sms:read, which support no longer holds.
    let receipts = support
        .request("POST", "/$procs/listMessageReceipts")
        .json(&json!({ "args": { "messageId": message.id } }))
        .send()
        .await;
    assert_eq!(receipts.status, 403, "{}", receipts.text());

    let reply = lookup(&support, json!({ "messageId": message.id })).await;
    let all = statuses(&reply);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0]["messageId"], message.id);
    assert_eq!(all[0]["appId"], rig.app_a);
    assert_eq!(all[0]["lastReceipt"]["rawStatus"], "Delivered");
    assert_eq!(all[0]["lastReceipt"]["outcome"], "delivered");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_status_is_found_by_id_by_client_reference_and_by_number_newest_first() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let number = unique_mtn_msisdn();
    let reference = format!("order-{}", tag());

    let older = rig.message(&rig.app_a, &Spec::to(&number)).await;
    let older = rig.deliver(older).await;
    rig.receipt(&older.id, schema::DeliveryOutcome::in_flight, "Sent")
        .await;
    rig.receipt(&older.id, schema::DeliveryOutcome::delivered, "Delivered")
        .await;
    let newer = rig
        .message(
            &rig.app_a,
            &Spec {
                client_ref: Some(reference.clone()),
                ..Spec::to(&number)
            },
        )
        .await;
    // Same reference in another app: a client reference belongs to one app.
    let _foreign = rig
        .message(
            &rig.app_b,
            &Spec {
                client_ref: Some(reference.clone()),
                ..Spec::to(&number)
            },
        )
        .await;
    let support = rig.person("support").await;

    // By number: the whole number, both apps, newest first.
    let by_number = lookup(&support, json!({ "msisdn": number })).await;
    let found = ids(&by_number);
    assert_eq!(found.len(), 3, "{}", by_number.text());
    assert_eq!(found[1], newer.id, "newest first: {found:?}");
    assert_eq!(found[2], older.id);
    let all = statuses(&by_number);
    let last = all.iter().find(|s| s["messageId"] == older.id).unwrap();
    assert_eq!(last["state"], "delivered");
    assert_eq!(
        last["lastReceipt"]["rawStatus"], "Delivered",
        "the newest receipt, not the first"
    );
    assert_eq!(last["segments"], 1);
    assert_eq!(last["attempts"], 0);
    let expected = format!("+237*******{}", &number[number.len() - 2..]);
    assert_eq!(last["recipient"], expected.as_str());

    // By client reference: one app's, named by a person.
    let by_ref = lookup(
        &support,
        json!({ "clientRef": reference, "appId": rig.app_a }),
    )
    .await;
    assert_eq!(ids(&by_ref), one(&newer.id));
    // A person has to say which app a reference belongs to.
    let unscoped = lookup(&support, json!({ "clientRef": reference })).await;
    assert_eq!(unscoped.status, 422, "{}", unscoped.text());

    // By id, and the person's optional app filter.
    assert_eq!(
        ids(&lookup(&support, json!({ "messageId": older.id })).await),
        one(&older.id)
    );
    let filtered = lookup(&support, json!({ "msisdn": number, "appId": rig.app_b })).await;
    assert_eq!(ids(&filtered).len(), 1);

    // limit and truncated.
    let limited = lookup(&support, json!({ "msisdn": number, "limit": 2 })).await;
    assert_eq!(ids(&limited).len(), 2);
    assert_eq!(limited.json()["truncated"], true);
    let roomy = lookup(&support, json!({ "msisdn": number, "limit": 3 })).await;
    assert_eq!(roomy.json()["truncated"], false);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_answer_never_carries_what_a_message_keeps_private() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let number = unique_mtn_msisdn();
    let reference = format!("ref-{}", tag());
    let spec = Spec {
        client_ref: Some(reference.clone()),
        ..Spec::to(&number)
    };
    let message = rig.message(&rig.app_a, &spec).await;
    rig.receipt(
        &message.id,
        schema::DeliveryOutcome::failed,
        "TemporaryError",
    )
    .await;
    let support = rig.person("support").await;

    let secrets = [
        spec.body.clone(),
        spec.state_reason.clone().unwrap(),
        message.bodyHash.clone(),
        message.msisdnHash.clone(),
        message.providerMessageRef.clone().unwrap(),
        reference.clone(),
        number.clone(),
        number[number.len() - 9..].to_owned(),
        "raw-payload-marker".to_owned(),
    ];
    let asks = [
        json!({ "messageId": message.id }),
        json!({ "msisdn": number }),
        json!({ "clientRef": reference, "appId": rig.app_a }),
    ];
    for ask in asks {
        let reply = lookup(&support, ask.clone()).await;
        assert_eq!(reply.status, 200, "{ask}: {}", reply.text());
        let text = reply.text();
        for secret in &secrets {
            assert!(
                !text.contains(secret),
                "{ask}: the answer carries {secret}: {text}"
            );
        }
        let mut keys = Vec::new();
        keys_of(&reply.json(), &mut keys);
        for forbidden in FORBIDDEN_KEYS {
            assert!(
                !keys.iter().any(|key| key == forbidden),
                "{ask}: the answer has a `{forbidden}` key: {keys:?}"
            );
        }
        // The nested receipt is a summary, and nothing else about the message.
        assert!(keys.iter().any(|key| key == "lastReceipt"));
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_role_is_decided_by_delivery_read_and_nothing_else() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let message = rig
        .message(&rig.app_a, &Spec::to(&unique_mtn_msisdn()))
        .await;
    let ask = json!({ "messageId": message.id });

    // Refused, and told which permission is missing: operator holds sms:read, owner
    // and admin hold nearly everything, and none of them hold delivery:read.
    for built_in in ["owner", "admin", "operator", "developer", "pending"] {
        let who = rig.person(built_in).await;
        assert_refused_naming_delivery_read(&lookup(&who, ask.clone()).await, built_in);
    }
    let unrelated = rig.custom_role(&["app:read", "sms:read"]).await;
    let who = rig.person(&unrelated).await;
    assert_refused_naming_delivery_read(&lookup(&who, ask.clone()).await, "custom role");

    // Allowed: the two built-ins that hold it, and a custom role holding only it.
    for built_in in ["support", "auditor"] {
        let who = rig.person(built_in).await;
        assert_eq!(
            ids(&lookup(&who, ask.clone()).await),
            one(&message.id),
            "{built_in}"
        );
    }
    let only = rig.custom_role(&["delivery:read"]).await;
    let who = rig.person(&only).await;
    assert_eq!(ids(&lookup(&who, ask.clone()).await), one(&message.id));

    // A refusal leaves no account of a lookup behind and shows no message content.
    assert_eq!(
        rig.lookups(&message.id).await.len(),
        3,
        "support, auditor, custom"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_app_key_with_the_scope_sees_its_own_app_and_no_other() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let shared = unique_mtn_msisdn();
    let reference = format!("shared-ref-{}", tag());
    let mine = rig
        .message(
            &rig.app_a,
            &Spec {
                client_ref: Some(reference.clone()),
                ..Spec::to(&shared)
            },
        )
        .await;
    let theirs = rig
        .message(
            &rig.app_b,
            &Spec {
                client_ref: Some(reference.clone()),
                ..Spec::to(&shared)
            },
        )
        .await;
    let key_a = rig.app_key(&rig.app_a, " sms:send delivery:read ").await;

    assert_eq!(
        ids(&lookup(&key_a, json!({ "messageId": mine.id })).await),
        one(&mine.id)
    );
    // Another app's id: an empty answer, as if it did not exist.
    let foreign = lookup(&key_a, json!({ "messageId": theirs.id })).await;
    assert_eq!(foreign.status, 200, "{}", foreign.text());
    assert_eq!(ids(&foreign), Vec::<String>::new());
    // A number both apps messaged: only the key's own app's message.
    assert_eq!(
        ids(&lookup(&key_a, json!({ "msisdn": shared })).await),
        one(&mine.id)
    );
    // A reference: its own app is implied.
    assert_eq!(
        ids(&lookup(&key_a, json!({ "clientRef": reference })).await),
        one(&mine.id)
    );
    // Naming another app is refused, not quietly ignored.
    let named = lookup(
        &key_a,
        json!({ "clientRef": reference, "appId": rig.app_b }),
    )
    .await;
    assert_eq!(named.status, 422, "{}", named.text());
    assert!(!named.text().contains(&theirs.id));
    // Naming its own is fine.
    assert_eq!(
        ids(&lookup(&key_a, json!({ "msisdn": shared, "appId": rig.app_a })).await),
        one(&mine.id)
    );

    // Without the scope: refused, whatever else it holds.
    for scopes in [" sms:send ", " sms:send sms:read ", " delivery:read2 "] {
        let key = rig.app_key(&rig.app_a, scopes).await;
        let reply = lookup(&key, json!({ "messageId": mine.id })).await;
        assert_refused_naming_delivery_read(&reply, scopes);
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_number_is_matched_whole() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let x = unique_mtn_msisdn();
    // Y differs from X in the last digit only: a lookup that matched a prefix or a
    // suffix of the number, and not the whole of it, would return both.
    let last = x
        .chars()
        .last()
        .and_then(|c| c.to_digit(10))
        .expect("a digit");
    let y = format!("{}{}", &x[..x.len() - 1], (last + 1) % 10);
    let at_x = rig.message(&rig.app_a, &Spec::to(&x)).await;
    let at_y = rig.message(&rig.app_a, &Spec::to(&y)).await;
    let support = rig.person("support").await;

    assert_eq!(
        ids(&lookup(&support, json!({ "msisdn": x })).await),
        one(&at_x.id)
    );
    assert_eq!(
        ids(&lookup(&support, json!({ "msisdn": y })).await),
        one(&at_y.id)
    );
    // The national form is the same number.
    let national = &x["+237".len()..];
    assert_eq!(
        ids(&lookup(&support, json!({ "msisdn": national })).await),
        one(&at_x.id)
    );
    // A prefix, a suffix, a partial number: refused, never a search.
    for partial in [&x[..x.len() - 3], &x[x.len() - 6..], "+2376", ""] {
        let reply = lookup(&support, json!({ "msisdn": partial })).await;
        assert_eq!(reply.status, 422, "{partial}: {}", reply.text());
    }
    // A number nobody messaged: empty, not an error.
    let unknown = lookup(&support, json!({ "msisdn": unique_mtn_msisdn() })).await;
    assert_eq!(ids(&unknown), Vec::<String>::new());
    // Two keys, or none, is a malformed query.
    for bad in [json!({}), json!({ "messageId": at_x.id, "msisdn": x })] {
        assert_eq!(lookup(&support, bad).await.status, 422);
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn every_answered_call_leaves_one_account_of_it_holding_a_hash_and_never_the_number() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let number = unique_mtn_msisdn();
    let reference = format!("audited-{}", tag());
    let message = rig
        .message(
            &rig.app_a,
            &Spec {
                client_ref: Some(reference.clone()),
                ..Spec::to(&number)
            },
        )
        .await;
    let support = rig.person("support").await;
    let key_a = rig.app_key(&rig.app_a, " delivery:read ").await;

    // By id, by a person.
    assert_eq!(
        ids(&lookup(&support, json!({ "messageId": message.id })).await).len(),
        1
    );
    let rows = rig.lookups(&message.id).await;
    assert_eq!(rows.len(), 1, "exactly one row per answered call");
    let row = &rows[0];
    assert_eq!(row.keyKind, schema::DeliveryLookupKey::message_id);
    assert_eq!(row.actorKind, "user");
    assert_eq!(row.actorRole, "support");
    assert_eq!(row.actorAppId, None);
    assert_eq!(row.appIdFilter, None);
    assert_eq!(row.resultCount, 1);

    // By number, by a person: the row holds the keyed hash.
    assert_eq!(
        ids(&lookup(&support, json!({ "msisdn": number })).await).len(),
        1
    );
    let hashed = rig.hash(&number);
    let rows = rig.lookups(&hashed).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].keyKind, schema::DeliveryLookupKey::msisdn);
    assert!(rows[0].keyValue.starts_with("hmac-sha256-v1:"));

    // By client reference, by an app key: the row holds a hash of the reference, the
    // app is named, and the reference is nowhere in it.
    assert_eq!(
        ids(&lookup(&key_a, json!({ "clientRef": reference })).await).len(),
        1
    );
    let ref_hash = hmac_sha256_hex(
        &HashPepper::new(&rig.pepper).unwrap(),
        &format!("delivery-lookup:client_ref:{reference}"),
    );
    let rows = rig.lookups(&ref_hash).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].keyKind, schema::DeliveryLookupKey::client_ref);
    assert_eq!(rows[0].actorKind, "app");
    assert_eq!(rows[0].actorAppId.as_deref(), Some(rig.app_a.as_str()));
    assert_eq!(rows[0].appIdFilter.as_deref(), Some(rig.app_a.as_str()));

    // No plaintext of the number or the reference anywhere in the three rows, and the
    // matching `cratestack_audit` entries hold the same hashes and no plaintext.
    let all: Vec<schema::DeliveryLookup> = rig
        .lookups(&message.id)
        .await
        .into_iter()
        .chain(rig.lookups(&hashed).await)
        .chain(rig.lookups(&ref_hash).await)
        .collect();
    for row in &all {
        let text = format!("{row:?}");
        assert!(!text.contains(&number[4..]), "{text}");
        assert!(!text.contains(&reference), "{text}");
    }
    let page = audit_log::list_audit_entries(
        &rig.db,
        &audit_log::AuditLogFilter {
            model: Some("DeliveryLookup".to_owned()),
            operation: None,
            actor_id: None,
            since: Some(Utc::now() - Duration::minutes(10)),
            until: None,
        },
        200,
        0,
    )
    .await
    .expect("reading the audit trail");
    for row in &all {
        let entry = page
            .entries
            .iter()
            .find(|entry| entry.primary_key.contains(&row.id))
            .unwrap_or_else(|| panic!("no cratestack_audit entry for lookup {}", row.id));
        assert_eq!(entry.operation, "create");
        let after = entry.after.clone().unwrap_or_default();
        assert!(after.contains(&row.keyValue), "{after}");
        assert!(!after.contains(&number[4..]), "{after}");
        assert!(!after.contains(&reference), "{after}");
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn refused_and_malformed_calls_leave_no_account_behind() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let message = rig
        .message(&rig.app_a, &Spec::to(&unique_mtn_msisdn()))
        .await;
    let support = rig.person("support").await;
    assert_eq!(
        ids(&lookup(&support, json!({ "messageId": message.id })).await).len(),
        1
    );
    let before = rig.lookups(&message.id).await.len();
    let outsider = rig.person("operator").await;
    assert_eq!(
        lookup(&outsider, json!({ "messageId": message.id }))
            .await
            .status,
        403
    );
    assert_eq!(
        lookup(&support, json!({ "messageId": message.id, "limit": 0 }))
            .await
            .status,
        422
    );
    assert_eq!(rig.lookups(&message.id).await.len(), before);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_sixty_first_number_lookup_in_an_hour_is_refused_and_ids_are_not_counted() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let number = unique_mtn_msisdn();
    let message = rig.message(&rig.app_a, &Spec::to(&number)).await;
    let support = rig.person("support").await;
    let other = rig.person("support").await;

    // One real number lookup, to learn who this caller is to the audit trail.
    assert_eq!(
        ids(&lookup(&support, json!({ "msisdn": number })).await).len(),
        1
    );
    let first = rig.lookups(&rig.hash(&number)).await;
    assert_eq!(first.len(), 1);
    let actor = first[0].actorSub.clone();

    // Fill the hour up to 59 (the first, and 58 more), by id lookups too: those are
    // not number lookups and must not count.
    for i in 0..58 {
        rig.db
            .delivery_lookup()
            .create(schema::CreateDeliveryLookupInput {
                actorSub: actor.clone(),
                actorKind: "user".to_owned(),
                actorRole: "support".to_owned(),
                actorAppId: None,
                keyKind: schema::DeliveryLookupKey::msisdn,
                keyValue: format!("seeded-{i}"),
                appIdFilter: None,
                resultCount: 0,
            })
            .run(&sys())
            .await
            .expect("seeding a lookup");
    }
    for _ in 0..3 {
        assert_eq!(
            ids(&lookup(&support, json!({ "messageId": message.id })).await).len(),
            1
        );
    }

    // The 60th is allowed, the 61st is refused with a stable, specific message.
    assert_eq!(
        ids(&lookup(&support, json!({ "msisdn": number })).await).len(),
        1
    );
    let refused = lookup(&support, json!({ "msisdn": number })).await;
    assert_eq!(refused.status, 403, "{}", refused.text());
    assert!(
        refused
            .text()
            .contains("delivery lookup budget exhausted: 60 number lookups per hour"),
        "{}",
        refused.text()
    );
    // The refusal is not an answer: it left no row. Ids still work for the same caller.
    let counted = rig
        .db
        .delivery_lookup()
        .aggregate()
        .count()
        .where_expr(
            FilterExpr::from(delivery_lookup::actorSub().eq(actor.clone()))
                .and(delivery_lookup::keyKind().eq(schema::DeliveryLookupKey::msisdn)),
        )
        .run(&sys())
        .await
        .expect("counting");
    assert_eq!(counted, 60);
    assert_eq!(
        ids(&lookup(&support, json!({ "messageId": message.id })).await).len(),
        1
    );
    // The budget is per caller.
    assert_eq!(
        ids(&lookup(&other, json!({ "msisdn": number })).await).len(),
        1
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_purged_message_shows_no_recipient() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let number = unique_mtn_msisdn();
    let live = rig.message(&rig.app_a, &Spec::to(&number)).await;
    let purged = rig
        .message(
            &rig.app_a,
            &Spec {
                purged: true,
                state_reason: None,
                ..Spec::to(&number)
            },
        )
        .await;
    let support = rig.person("support").await;

    // A purge keeps msisdnHash, so the number still finds both; only the purged one
    // loses its recipient.
    let all = statuses(&lookup(&support, json!({ "msisdn": number })).await);
    assert_eq!(all.len(), 2);
    let gone = all.iter().find(|s| s["messageId"] == purged.id).unwrap();
    assert_eq!(gone["purged"], true);
    assert!(gone["recipient"].is_null(), "{gone}");
    let kept = all.iter().find(|s| s["messageId"] == live.id).unwrap();
    assert_eq!(kept["purged"], false);
    assert!(kept["recipient"].as_str().unwrap().starts_with("+237"));
}

fn person_ctx(sub: &str) -> CratestackContext {
    Principal {
        sub: sub.to_owned(),
        kind: PrincipalKind::User,
        role: "support".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_window_bounds_a_number_or_reference_and_not_an_id() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let number = unique_mtn_msisdn();
    let old = rig.message(&rig.app_a, &Spec::to(&number)).await;
    // Ten days old: past the default seven-day window of a number lookup.
    rig.db
        .message()
        .update(old.id.clone())
        .set(UpdateMessageInput {
            createdAt: Some(Utc::now() - Duration::days(10)),
            ..Default::default()
        })
        .if_match(old.version)
        .run(&sys())
        .await
        .expect("backdating the message");
    let pepper = HashPepper::new(&rig.pepper).unwrap();
    let ctx = person_ctx(&format!("window-{}", tag()));
    let query = |msisdn: Option<&str>, id: Option<&str>, since| schema::DeliveryStatusQuery {
        messageId: id.map(str::to_owned),
        appId: None,
        clientRef: None,
        msisdn: msisdn.map(str::to_owned),
        since,
        until: None,
        limit: None,
    };
    let run = |args| delivery_status::lookup(&pepper, &rig.db, &ctx, args, Utc::now());

    let default_window = run(query(Some(&number), None, None)).await.unwrap();
    assert!(
        default_window.statuses.is_empty(),
        "a ten-day-old message is outside seven days"
    );
    let widened = run(query(
        Some(&number),
        None,
        Some(Utc::now() - Duration::days(30)),
    ))
    .await
    .unwrap();
    assert_eq!(widened.statuses.len(), 1);
    // An id is exact: found whatever its age.
    let by_id = run(query(None, Some(&old.id), None)).await.unwrap();
    assert_eq!(by_id.statuses.len(), 1);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_lookup_that_cannot_be_recorded_is_not_answered() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let message = rig
        .message(&rig.app_a, &Spec::to(&unique_mtn_msisdn()))
        .await;
    let pepper = HashPepper::new(&rig.pepper).unwrap();
    let ask = || schema::DeliveryStatusQuery {
        messageId: Some(message.id.clone()),
        appId: None,
        clientRef: None,
        msisdn: None,
        since: None,
        until: None,
        limit: None,
    };

    // The same call with a caller the audit row can hold is answered...
    let fine = person_ctx(&format!("recordable-{}", tag()));
    let answered = delivery_status::lookup(&pepper, &rig.db, &fine, ask(), Utc::now())
        .await
        .expect("a recordable caller is answered");
    assert_eq!(answered.statuses.len(), 1);

    // ...and with one the audit row cannot hold (a subject longer than the column
    // takes) it is an error and answers nothing: the message was found, and was
    // not handed over.
    let unrecordable = person_ctx(&"s".repeat(300));
    let before = rig.lookups(&message.id).await.len();
    let refused = delivery_status::lookup(&pepper, &rig.db, &unrecordable, ask(), Utc::now()).await;
    assert!(
        matches!(
            refused,
            Err(CratestackError::Validation(_)
                | CratestackError::Database(_)
                | CratestackError::Internal(_))
        ),
        "a lookup that cannot be accounted for must fail, got {refused:?}"
    );
    assert_eq!(rig.lookups(&message.id).await.len(), before);
}

/// What an `Idempotency-Key` does to a lookup, measured and not assumed: the
/// layer sits above every route, so a repeated key replays the first answer
/// without running the procedure. The consequences are pinned here so a change in
/// either direction is seen: the replay is the same answer (no new information,
/// nothing to count), and the first call still left its row.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_idempotency_key_replays_the_first_answer_and_the_first_call_is_recorded() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = Rig::new().await;
    let number = unique_mtn_msisdn();
    let message = rig.message(&rig.app_a, &Spec::to(&number)).await;
    let support = rig.person("support").await;
    let key = format!("lookup-{}", tag());

    let args = json!({ "args": { "messageId": message.id } });
    let ask = || async {
        support
            .request("POST", "/$procs/lookupDeliveryStatus")
            .json(&args)
            .idempotency_key(&key)
            .send()
            .await
    };
    let first = ask().await;
    let second = ask().await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(second.status, 200, "{}", second.text());
    assert_eq!(first.json(), second.json(), "the same answer");
    // Measured: the layer answers the second call itself, so the procedure ran, and
    // was recorded, once.
    assert_eq!(rig.lookups(&message.id).await.len(), 1);
}
