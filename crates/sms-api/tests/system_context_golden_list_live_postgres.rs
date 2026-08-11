//! #155's golden guard against the eighth instance of this repo's single
//! most repeated bug shape: a model whose `@@allow` doesn't admit a
//! `system`-role context on read/list/detail.
//!
//! `AGENTS.md`'s own "Invariants that fail the build rather than
//! production" section names the mechanism plainly: `CrateStack` denies a
//! list-route policy by filtering to an **empty array**, not by erroring —
//! so an internal system-context read of a model missing the right clause
//! just quietly returns nothing, and the caller behaves as though the
//! table were empty. That has now been found live, never by review, seven
//! separate times: `App`, `AppClient`, `SenderIdRegistration`,
//! `OperatorPrefixRule` (#94), `Provider`, `Job` (#100), `Message`
//! list/detail (#96 — the reason #29's claim loop silently returned zero
//! rows for a whole milestone), and `DeliveryReceipt` (#121).
//!
//! # Why this needs a real database
//!
//! Same reason `#87` survived `#78` unnoticed: the failure is row-level
//! policy filtering evaluated at query time, inside a generated closure
//! with no distinguishable shape between "denied on purpose" and "denied
//! because a clause is missing". Neither `cratestack check` nor `cargo
//! build` can see it. The only way to catch it is to seed a real row and
//! attempt a real read under a real system context against a real
//! Postgres, then check the *outcome* — which is exactly what
//! `tests/policy_golden_list_live_postgres.rs` already does for
//! `create`/`update`/`delete`, and what that file's own doc says
//! `read`/`list`/`detail` still needs: "a seed-then-compare test per
//! model, a different technique this file doesn't also try to be." This
//! file is that technique.
//!
//! # Deriving the model list, and why a hand-maintained classification is
//! still needed on top of it
//!
//! [`model_names_from_schema`] parses `schema/schema.cstack` itself for
//! every `model Foo {` line, rather than hand-copying a list that would
//! drift exactly the way `AGENTS.md`'s own "duplicated hardcoded file
//! lists" warning describes. That solves "did a model get renamed or
//! removed without this file noticing" — but it cannot, by itself, solve
//! the actual bug this file exists to catch: whether a *newly added*
//! model needs a system-context reader is a fact about intent (does some
//! future procedure or worker role need to read it under `sys()`?), not a
//! fact the schema text alone determines. A purely-derived list would
//! trivially "pass" for a brand new model that's missing `hasRole('system')`
//! by mistake, the exact same way the schema's own `@@allow` list would —
//! both would just agree the model isn't system-readable, and the omission
//! would sail through undetected.
//!
//! So this file keeps a small, explicit, justified classification —
//! [`SYSTEM_READABLE_MODELS`] (seeded and read back live, below) and
//! [`NOT_REQUIRED_TO_BE_SYSTEM_READABLE`] (models with no internal system
//! reader anywhere in this codebase today, one reason each) — and
//! [`every_model_in_the_schema_is_classified`] fails loudly, by name, the
//! moment a model exists in the schema but in neither list. That is this
//! file's answer to the acceptance criterion's own fallback clause: "if
//! deriving it is genuinely impractical, make the hand-maintained list
//! fail loudly when a model is added." Adding a 20th model forces a human
//! to put it in one list or the other *and say why* — it cannot silently
//! fall through unclassified.
//!
//! # The eighth instance — and the guard doing its job twice
//!
//! `WebhookEndpoint`'s `@@allow("read", ...)` was `auth().kind == "user"`
//! only, the same shape as the seven instances before it. What makes this
//! one different from those seven is that it never shipped broken: it was
//! caught **twice, independently, on the same day, before merge**, by two
//! changes that each needed a system-context read of this model —
//! `rotateWebhookSecret` (#41, `crates/sms-api/src/procedures.rs`) and
//! #38's `Message.created`/`Message.updated` subscribers
//! (`crates/sms-api/src/webhooks.rs`).
//!
//! The seven prior instances were each found live, after shipping, by a
//! reader silently returning nothing. This one was found by
//! [`every_model_in_the_schema_is_classified`] refusing to let
//! `WebhookEndpoint` move out of [`NOT_REQUIRED_TO_BE_SYSTEM_READABLE`]
//! unclassified, and by
//! [`every_system_readable_model_actually_admits_a_system_read`] failing
//! for real when the clause was missing. That is what this file's own
//! earlier framing ("a golden test... would end it; until someone writes
//! it, expect an eighth") predicted it would do once written.
//!
//! Two independent discoveries of one gap in a day is also the strongest
//! argument yet for the structural fix, not just the guard — see #176.
//!
//! # The ninth instance — `WebhookAttempt`, #40
//!
//! `WebhookAttempt`'s own `list`/`detail` policy had `create`/`update`
//! admitting `hasRole('system')` from the start (#38/#39) but no such clause
//! on `list`/`detail` — flagged explicitly, in advance, by #38/#39's own PR
//! description as a gap #40's `hooks` claim loop would need closed the
//! moment it existed (`crates/sms-worker/src/claim.rs`'s `Claimable for
//! WebhookAttempt::candidates` reads this model under `sys()` to find due
//! attempts). Same non-broken-on-arrival shape as `WebhookEndpoint` above:
//! fixed in the same PR that added the reader, caught by this file's guard
//! before merge, not found live afterward.
//!
//! Cross-checked against every `db.<model>()...run(sys)` call in
//! `crates/sms-api/src`, `crates/sms-worker/src`, and `crates/sms-auth/src`
//! (the only places a `system`-role [`CoolContext`] is ever constructed):
//! all 15 models an internal system context read as of the ninth instance
//! already admitted one; of the 4 that didn't (`Route`, `MessagePart`,
//! `User`, `Role`), 3 still have no internal reader. See
//! [`NOT_REQUIRED_TO_BE_SYSTEM_READABLE`]'s own per-model reasoning.
//!
//! # The tenth instance — `Route`, #62
//!
//! `Route` moved here the moment the routing rules engine (#62, §6.3) gave
//! it a real reader: `crates/sms-worker/src/routing.rs`'s `decide`, called
//! from `claim.rs`'s `accepted` branch, reads every `Route` row (and the
//! `Provider` rows they reference) under `sys` to hand to
//! `sms_routing::select_route`. Flagged in advance by `NOT_REQUIRED_TO_BE_SYSTEM_READABLE`'s
//! own prior entry for this model ("real Route-rule routing isn't built
//! yet"), the same way `WebhookEndpoint` (#41) and `WebhookAttempt` (#40)
//! were each flagged before their own readers landed — not found broken
//! live. Caught by this file's own guard before merge: `every_model_in_the_schema_is_classified`
//! would have failed the moment `Route` moved lists without a
//! classification decision, and `every_system_readable_model_actually_admits_a_system_read`
//! failed for real, on purpose, when `hasRole('system')` was pulled from
//! `Route`'s `read` clause to prove the guard actually guards something —
//! see the PR description for that run's exact failure output.
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. Run explicitly:
//!
//! ```bash
//! cargo test -p sms-api --test system_context_golden_list_live_postgres -- --ignored
//! ```

use std::path::Path;

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, app, app_client, client_assertion, delivery_receipt, job, message, oauth_client,
    oauth_signing_key, operator_prefix_rule, opt_out, provider, route, sender_id,
    sender_id_registration, webhook_attempt, webhook_endpoint, ClientAuthMethod, Cratestack,
    DeliveryOutcome, Encoding, MessageClass, OperatorCode, OptOutSource, ProviderKind,
};

/// Same reasoning as every other live suite's own copy of this mutex — see
/// `crates/sms-worker/tests/claim_live_postgres.rs`'s doc (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Every model an internal `system`-role context reads somewhere in this
/// codebase today, cross-checked against every `db.<model>()...run(sys)` /
/// `run(&system_context())` call site in `crates/sms-api`,
/// `crates/sms-worker`, and `crates/sms-auth`:
///
/// - `App`, `AppClient` — `Procedures::resolve_app` (`crates/sms-api/src/procedures.rs`)
///   and `GatewayAuth`'s own `client_id` lookup (`crates/sms-api/src/auth.rs`).
/// - `OauthClient` — `SmsClientStore` (`crates/sms-auth/src/lib.rs`).
/// - `OauthSigningKey` — `sms_auth::op` load/rotate (`crates/sms-auth/src/op.rs`).
/// - `ClientAssertion` — `SmsClientAssertionStore` (`crates/sms-auth/src/lib.rs`).
/// - `SenderId`, `SenderIdRegistration` — `Procedures::resolve_sender_id`.
/// - `Provider` — `crates/sms-worker/src/routing.rs`'s `decide` (since #62;
///   formerly `cheapest_active_provider`, which this replaced) and
///   `Procedures::estimate_cost`.
/// - `Route` — `crates/sms-worker/src/routing.rs`'s `decide` (#62), the
///   routing rules engine's own I/O boundary — see this file's own "the
///   tenth instance" section above.
/// - `OperatorPrefixRule` — `Procedures::operator_table`.
/// - `Message` — the claim loop's own `candidates()` (`crates/sms-worker/src/claim.rs`)
///   and `dlr::ingest_one`.
/// - `DeliveryReceipt` — `dlr::ingest_one` writes it under `sys`; its
///   `list`/`detail` policy is exercised by anything that ever needs to
///   read a receipt back under a system context (a future reconciliation
///   job — see #121's own reasoning in `schema.cstack`).
/// - `Job` — the claim loop's own two-hop reclaim.
/// - `OptOut` — `Procedures::ensure_not_opted_out`.
/// - `WebhookEndpoint` — two internal system-context readers, found
///   independently: `Procedures::rotate_secret` (`rotateWebhookSecret`,
///   #41) reads the endpoint and writes its fresh
///   `secret`/`prevSecret`/`secretRotatedAt` under `sys`; and #38's
///   subscribers (`crates/sms-api/src/webhooks.rs`,
///   `enqueue_message_webhook_attempts`) resolve which endpoints
///   subscribe to a derived event type, also under `sys`.
/// - `WebhookAttempt` — the `hooks` role's own claim loop (`Claimable for
///   WebhookAttempt::candidates`, `crates/sms-worker/src/claim.rs`, #40)
///   reads due attempts under `sys`; `hooks.rs` also re-reads a claimed
///   row's own endpoint but never the attempt a second time.
const SYSTEM_READABLE_MODELS: &[&str] = &[
    "App",
    "AppClient",
    "OauthClient",
    "OauthSigningKey",
    "ClientAssertion",
    "SenderId",
    "SenderIdRegistration",
    "Provider",
    "Route",
    "OperatorPrefixRule",
    "Message",
    "DeliveryReceipt",
    "Job",
    "OptOut",
    "WebhookEndpoint",
    "WebhookAttempt",
];

/// Models with no internal `system`-role reader anywhere in this codebase
/// today — each reason is a statement about *current* production code, not
/// a permanent exemption. `AGENTS.md`'s own issue text (#155) names exactly
/// this risk: M3/M4/M5/M6 each add models a system context will eventually
/// need to read (webhook delivery, consent records, retention bookkeeping),
/// and the moment a real reader is wired up, the model has to move to
/// [`SYSTEM_READABLE_MODELS`] — and, before it does, `schema.cstack` needs
/// its own `hasRole('system')` clause, the same way every prior instance
/// did.
const NOT_REQUIRED_TO_BE_SYSTEM_READABLE: &[(&str, &str)] = &[
    (
        "MessagePart",
        "nothing in this codebase creates or reads a MessagePart row yet; \
         concatenated-SMS part tracking has no writer or reader, system or \
         otherwise",
    ),
    (
        "User",
        "human/admin-console account management only; no internal system-role \
         code reads this",
    ),
    (
        "Role",
        "human/admin-console RBAC management only; no internal system-role \
         code reads this",
    ),
];

/// Parses `schema/schema.cstack` for every top-level `model Foo {` line —
/// the schema itself, not a hand-copied list, is the source of truth for
/// *which models exist*. See this file's own module doc for why the
/// classification of *what each one needs* still can't be derived the same
/// way.
///
/// Line-based, matching §2.0's own documented grammar constraint ("the
/// parser is line-based"): a model declaration is always `model Name {` on
/// its own line — the same shape every model in this schema already
/// follows.
fn model_names_from_schema() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/schema.cstack");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));

    text.lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("model ")
                .map(|rest| rest.trim_end_matches('{').trim().to_owned())
        })
        .filter(|name| !name.is_empty())
        .collect()
}

/// The always-on, no-database half of this file's guard: every model the
/// schema actually declares must appear in exactly one of
/// [`SYSTEM_READABLE_MODELS`] / [`NOT_REQUIRED_TO_BE_SYSTEM_READABLE`], and
/// every entry in those two lists must still name a real model. Not
/// `#[ignore]`d — this needs no Postgres at all, so it runs under plain
/// `just test` / `cargo test --workspace` and catches an unclassified new
/// model at the first `cargo check`-adjacent step in CI, well before
/// anyone has to think to run the live suites below.
#[test]
fn every_model_in_the_schema_is_classified() {
    let models = model_names_from_schema();
    assert!(
        models.len() >= 19,
        "parsed only {} model(s) from schema.cstack — this schema had 19 models \
         the day this test was written; either the parser broke or a model was \
         removed. Parsed: {models:?}",
        models.len()
    );

    for model in &models {
        let required = SYSTEM_READABLE_MODELS.contains(&model.as_str());
        let exempt = NOT_REQUIRED_TO_BE_SYSTEM_READABLE
            .iter()
            .any(|(name, _)| *name == model.as_str());
        assert!(
            required || exempt,
            "model {model:?} exists in schema.cstack but is classified in neither \
             SYSTEM_READABLE_MODELS nor NOT_REQUIRED_TO_BE_SYSTEM_READABLE in \
             crates/sms-api/tests/system_context_golden_list_live_postgres.rs. \
             This is exactly the gap #155 exists to close: decide whether an \
             internal system context needs to read {model:?}, add \
             hasRole('system') to its @@allow if so, add it to \
             SYSTEM_READABLE_MODELS and seed it in this file's live test — or, if \
             not, add it to NOT_REQUIRED_TO_BE_SYSTEM_READABLE with a one-line \
             reason. Do not let a new model go unclassified."
        );
        assert!(
            !(required && exempt),
            "model {model:?} is listed in both SYSTEM_READABLE_MODELS and \
             NOT_REQUIRED_TO_BE_SYSTEM_READABLE — pick one"
        );
    }

    for name in SYSTEM_READABLE_MODELS {
        assert!(
            models.iter().any(|m| m == name),
            "SYSTEM_READABLE_MODELS names {name:?}, which no longer exists in \
             schema.cstack — stale entry, remove it"
        );
    }
    for (name, _) in NOT_REQUIRED_TO_BE_SYSTEM_READABLE {
        assert!(
            models.iter().any(|m| m == name),
            "NOT_REQUIRED_TO_BE_SYSTEM_READABLE names {name:?}, which no longer \
             exists in schema.cstack — stale entry, remove it"
        );
    }
}

fn owner() -> CoolContext {
    Principal {
        sub: "system-golden-list-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// The context every internal, procedure-driven read this file is guarding
/// runs under — `kind: App`, `role: "system"`, exactly as
/// `Procedures::sys()` / `auth::system_context()` build it. Never a real
/// caller's token; see either of those functions' own doc for why.
fn sys() -> CoolContext {
    Principal {
        sub: "system-golden-list-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
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
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

/// Reads `model_name` back under [`sys()`], filtered by `id` — the
/// seed-then-compare technique `policy_golden_list_live_postgres.rs`'s own
/// doc says a read/list/detail assertion needs. A model whose policy
/// silently filters the system context down to an empty array (this
/// file's entire reason to exist) fails here with a message naming the
/// model and its current `@@allow` clause, not a generic "assertion
/// failed".
macro_rules! assert_system_can_read_back {
    ($db:expr, $model_method:ident, $module:ident, $seeded_id:expr, $model_name:literal, $clause:literal) => {{
        let rows = $db
            .$model_method()
            .find_many()
            .where_expr(FilterExpr::from($module::id().eq($seeded_id.clone())))
            .limit(1)
            .run(&sys())
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "a system context reading {} errored rather than empty-filtering: {error:?}",
                    $model_name
                )
            });
        assert!(
            !rows.is_empty(),
            "a system context could not read back the {} row it (or an owner, on its \
             behalf) just seeded. {}'s current schema.cstack read/list/detail policy \
             is: {}. CrateStack denies a list-route policy by filtering to an EMPTY \
             ARRAY, not by erroring (AGENTS.md's own documented failure mode) — this \
             is the eighth instance of the bug #155 exists to catch.",
            $model_name, $model_name, $clause
        );
    }};
}

async fn seed_and_verify_app(db: &Cratestack, suffix: &str) -> schema::App {
    let seeded = db
        .app()
        .create(schema::CreateAppInput {
            name: "system golden list app".to_owned(),
            slug: format!("sys-golden-{}", suffix.to_lowercase()),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding an App");
    assert_system_can_read_back!(
        db,
        app,
        app,
        seeded.id,
        "App",
        "@@allow(\"read\", hasRole('owner') || hasRole('admin') || hasRole('operator') \
         || hasRole('auditor') || hasRole('developer') || hasRole('system'))"
    );
    seeded
}

async fn seed_and_verify_app_client(
    db: &Cratestack,
    suffix: &str,
    app_id: &str,
) -> schema::AppClient {
    let seeded = db
        .app_client()
        .create(schema::CreateAppClientInput {
            appId: app_id.to_owned(),
            clientId: format!("sys-golden-client-{suffix}"),
            label: "system golden list client".to_owned(),
            scopes: " sms:send ".to_owned(),
            lastUsedAt: None,
            retiredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding an AppClient");
    assert_system_can_read_back!(
        db,
        app_client,
        app_client,
        seeded.id,
        "AppClient",
        "@@allow(\"read\", hasRole('owner') || hasRole('admin') || hasRole('developer') \
         || hasRole('system'))"
    );
    seeded
}

async fn seed_and_verify_oauth_client(
    db: &Cratestack,
    suffix: &str,
    app_client_id: &str,
) -> schema::OauthClient {
    let seeded = db
        .oauth_client()
        .create(schema::CreateOauthClientInput {
            clientId: format!("sys-golden-oauth-{suffix}"),
            appClientId: Some(app_client_id.to_owned()),
            tokenEndpointAuthMethod: ClientAuthMethod::private_key_jwt,
            jwks: Some(r#"{"keys":[{"kty":"RSA","kid":"k1","n":"...","e":"AQAB"}]}"#.to_owned()),
            grantTypes: " client_credentials ".to_owned(),
            scopes: " sms:send ".to_owned(),
            redirectUris: " ".to_owned(),
            requirePkce: false,
        })
        .run(&sys())
        .await
        .expect("seeding an OauthClient");
    assert_system_can_read_back!(
        db,
        oauth_client,
        oauth_client,
        seeded.id,
        "OauthClient",
        "@@allow(\"read\", hasRole('system'))"
    );
    seeded
}

async fn seed_and_verify_oauth_signing_key(db: &Cratestack) -> schema::OauthSigningKey {
    let seeded = db
        .oauth_signing_key()
        .create(schema::CreateOauthSigningKeyInput {
            privateKeyPem: "-----BEGIN PRIVATE KEY-----\nsystem-golden-list-fixture\n\
                             -----END PRIVATE KEY-----"
                .to_owned(),
            expiresAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding an OauthSigningKey");
    assert_system_can_read_back!(
        db,
        oauth_signing_key,
        oauth_signing_key,
        seeded.id,
        "OauthSigningKey",
        "@@allow(\"read\", hasRole('system'))"
    );
    seeded
}

async fn seed_and_verify_client_assertion(
    db: &Cratestack,
    suffix: &str,
    now: chrono::DateTime<Utc>,
) -> schema::ClientAssertion {
    let seeded = db
        .client_assertion()
        .create(schema::CreateClientAssertionInput {
            jti: format!("sys-golden-jti-{suffix}"),
            expiresAt: now + Duration::hours(1),
        })
        .run(&sys())
        .await
        .expect("seeding a ClientAssertion");
    assert_system_can_read_back!(
        db,
        client_assertion,
        client_assertion,
        seeded.id,
        "ClientAssertion",
        "@@allow(\"read\", hasRole('system'))"
    );
    seeded
}

/// Admitted via `auth().kind == "app"` — `sys()`'s `PrincipalKind::App`
/// matches, not a `hasRole('system')` clause. Still exactly the shape this
/// file exists to catch if that clause is ever narrowed.
async fn seed_and_verify_sender_id(db: &Cratestack, suffix: &str) -> schema::SenderId {
    let value = format!("T{}", &suffix[..suffix.len().min(9)]).to_uppercase();
    let seeded = db
        .sender_id()
        .create(schema::CreateSenderIdInput {
            value,
            kind: "alphanumeric".to_owned(),
            notes: None,
        })
        .run(&owner())
        .await
        .expect("seeding a SenderId");
    assert_system_can_read_back!(
        db,
        sender_id,
        sender_id,
        seeded.id,
        "SenderId",
        "@@allow(\"read\", auth().kind == \"user\" || auth().kind == \"app\")"
    );
    seeded
}

async fn seed_and_verify_provider(db: &Cratestack, suffix: &str) -> schema::Provider {
    let seeded = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!(
                "sysgold_{}",
                suffix.to_lowercase().chars().take(20).collect::<String>()
            ),
            displayName: "System Golden List Provider".to_owned(),
            kind: ProviderKind::aggregator_http,
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
        .expect("seeding a Provider");
    assert_system_can_read_back!(
        db,
        provider,
        provider,
        seeded.id,
        "Provider",
        "@@allow(\"read\", hasRole('owner') || hasRole('admin') || hasRole('operator') \
         || hasRole('auditor') || hasRole('system'))"
    );
    seeded
}

/// #62: `crates/sms-worker/src/routing.rs`'s `decide` reads `Route` under
/// `sys` — the first internal system-context reader this model has ever
/// had, and the reason its `read` `@@allow` clause gained `hasRole('system')`
/// in the same change that added the reader.
async fn seed_and_verify_route(db: &Cratestack, suffix: &str, provider_id: &str) -> schema::Route {
    let seeded = db
        .route()
        .create(schema::CreateRouteInput {
            name: format!("system golden list route {suffix}"),
            priority: 0,
            weight: 1,
            enabled: true,
            matchOperator: None,
            matchClass: None,
            matchAppId: None,
            matchPrefix: None,
            providerId: provider_id.to_owned(),
            failoverRouteId: None,
        })
        .run(&owner())
        .await
        .expect("seeding a Route");
    assert_system_can_read_back!(
        db,
        route,
        route,
        seeded.id,
        "Route",
        "@@allow(\"read\", hasRole('owner') || hasRole('admin') || hasRole('operator') \
         || hasRole('auditor') || hasRole('system'))"
    );
    seeded
}

async fn seed_and_verify_sender_id_registration(
    db: &Cratestack,
    sender_id_id: &str,
    provider_id: &str,
    now: chrono::DateTime<Utc>,
) -> schema::SenderIdRegistration {
    let seeded = db
        .sender_id_registration()
        .create(schema::CreateSenderIdRegistrationInput {
            senderIdId: sender_id_id.to_owned(),
            providerId: provider_id.to_owned(),
            status: "pending".to_owned(),
            submittedAt: Some(now),
            approvedAt: None,
            reference: None,
            rejectionReason: None,
        })
        .run(&owner())
        .await
        .expect("seeding a SenderIdRegistration");
    assert_system_can_read_back!(
        db,
        sender_id_registration,
        sender_id_registration,
        seeded.id,
        "SenderIdRegistration",
        "@@allow(\"read\", auth().kind == \"user\" || hasRole('system'))"
    );
    seeded
}

/// A fixed, out-of-band prefix: `0002_bootstrap` seeds 67x/650-654 (mtn),
/// 69x/655-659 (orange), 68x (contested), 62x (camtel) — "9876" collides
/// with none of them, and this file's own database is dropped and
/// recreated fresh per test-binary run (`sms_test_support`'s own doc), so
/// no cross-run uniqueness concern applies either.
async fn seed_and_verify_operator_prefix_rule(db: &Cratestack) -> schema::OperatorPrefixRule {
    let seeded = db
        .operator_prefix_rule()
        .create(schema::CreateOperatorPrefixRuleInput {
            prefix: "9876".to_owned(),
            operator: OperatorCode::unknown,
            lastObservedAt: None,
            notes: Some("system-context-golden-list fixture, #155".to_owned()),
        })
        .run(&owner())
        .await
        .expect("seeding an OperatorPrefixRule");
    assert_system_can_read_back!(
        db,
        operator_prefix_rule,
        operator_prefix_rule,
        seeded.id,
        "OperatorPrefixRule",
        "@@allow(\"read\", hasRole('owner') || hasRole('admin') || hasRole('operator') \
         || hasRole('auditor') || hasRole('system'))"
    );
    seeded
}

async fn seed_and_verify_message(
    db: &Cratestack,
    suffix: &str,
    app_id: &str,
    now: chrono::DateTime<Utc>,
) -> schema::Message {
    let seeded = db
        .message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: None,
            msisdn: "+237677900000".to_owned(),
            msisdnHash: format!("test-hash-{suffix}"),
            operator: OperatorCode::mtn,
            senderIdValue: "TESTSEND".to_owned(),
            class: MessageClass::otp,
            priority: 100,
            maxAttempts: 3,
            body: Some("system context golden list probe".to_owned()),
            bodyHash: format!("test-body-hash-{suffix}"),
            bodyLength: 33,
            encoding: Encoding::gsm7,
            segments: 1,
            stateReason: None,
            routeId: None,
            providerId: None,
            providerMessageRef: None,
            providerMessageRefAlt: None,
            leaseOwner: None,
            leaseUntil: None,
            scheduledAt: None,
            expiresAt: now + Duration::hours(1),
            submittedAt: None,
            finalizedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding a Message");
    assert_system_can_read_back!(
        db,
        message,
        message,
        seeded.id,
        "Message",
        "@@allow(\"list\"/\"detail\", auth().kind == \"user\" || appId == auth().appId \
         || hasRole('system'))"
    );
    seeded
}

async fn seed_and_verify_delivery_receipt(
    db: &Cratestack,
    suffix: &str,
    message_id: &str,
    provider_id: &str,
    now: chrono::DateTime<Utc>,
) -> schema::DeliveryReceipt {
    let seeded = db
        .delivery_receipt()
        .create(schema::CreateDeliveryReceiptInput {
            messageId: message_id.to_owned(),
            providerId: provider_id.to_owned(),
            providerMessageRef: format!("sys-golden-ref-{suffix}"),
            outcome: DeliveryOutcome::delivered,
            rawStatus: "DELIVRD".to_owned(),
            errorCode: None,
            networkCode: OperatorCode::mtn,
            occurredAt: Some(now),
            rawPayload: "{}".to_owned(),
        })
        .run(&sys())
        .await
        .expect("seeding a DeliveryReceipt");
    assert_system_can_read_back!(
        db,
        delivery_receipt,
        delivery_receipt,
        seeded.id,
        "DeliveryReceipt",
        "@@allow(\"list\"/\"detail\", auth().kind == \"user\" || hasRole('system'))"
    );
    seeded
}

async fn seed_and_verify_job(db: &Cratestack, now: chrono::DateTime<Utc>) -> schema::Job {
    let seeded = db
        .job()
        .create(schema::CreateJobInput {
            kind: "system_golden_list_probe".to_owned(),
            dedupeKey: None,
            payload: "{}".to_owned(),
            priority: 500,
            runAt: now,
            maxAttempts: 5,
            leaseOwner: None,
            leaseUntil: None,
            lastError: None,
            startedAt: None,
            finishedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding a Job");
    assert_system_can_read_back!(
        db,
        job,
        job,
        seeded.id,
        "Job",
        "@@allow(\"list\"/\"detail\", hasRole('owner') || hasRole('admin') || \
         hasRole('operator') || hasRole('system'))"
    );
    seeded
}

/// Admitted via `auth().kind == "app"`, same shape as [`seed_and_verify_sender_id`]
/// above — not a `hasRole('system')` clause, but exactly what
/// `Procedures::ensure_not_opted_out` (`sendMessage`'s own opt-out check)
/// relies on being true.
async fn seed_and_verify_opt_out(
    db: &Cratestack,
    suffix: &str,
    now: chrono::DateTime<Utc>,
) -> schema::OptOut {
    let seeded = db
        .opt_out()
        .create(schema::CreateOptOutInput {
            msisdnHash: format!("sys-golden-optout-hash-{suffix}"),
            msisdn: "+237677900001".to_owned(),
            source: OptOutSource::admin,
            scope: "all".to_owned(),
            reason: None,
            optedOutAt: now,
        })
        .run(&owner())
        .await
        .expect("seeding an OptOut");
    assert_system_can_read_back!(
        db,
        opt_out,
        opt_out,
        seeded.id,
        "OptOut",
        "@@allow(\"read\", auth().kind == \"user\" || auth().kind == \"app\")"
    );
    seeded
}

/// #41: `rotateWebhookSecret` reads and updates a `WebhookEndpoint` under
/// `sys` — the first internal system-context reader this model has ever
/// had, and the reason its `read`/`update` `@@allow` clauses gained
/// `hasRole('system')` in the same change that added this function.
async fn seed_and_verify_webhook_endpoint(
    db: &Cratestack,
    suffix: &str,
    app_id: &str,
) -> schema::WebhookEndpoint {
    let seeded = db
        .webhook_endpoint()
        .create(schema::CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: format!("https://example.test/webhooks/{suffix}"),
            eventTypes: " message.delivered ".to_owned(),
            secret: format!("sys-golden-secret-{suffix}"),
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: true,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding a WebhookEndpoint");
    assert_system_can_read_back!(
        db,
        webhook_endpoint,
        webhook_endpoint,
        seeded.id,
        "WebhookEndpoint",
        "@@allow(\"read\", auth().kind == \"user\" || hasRole('system'))"
    );
    seeded
}

/// #40: `Claimable for WebhookAttempt::candidates`
/// (`crates/sms-worker/src/claim.rs`) reads due `WebhookAttempt` rows under
/// `sys` — the first internal system-context reader this model has ever
/// had, and the reason its `list`/`detail` `@@allow` clauses gained
/// `hasRole('system')` in the same change that added the claim loop.
async fn seed_and_verify_webhook_attempt(
    db: &Cratestack,
    suffix: &str,
    endpoint_id: &str,
) -> schema::WebhookAttempt {
    let seeded = db
        .webhook_attempt()
        .create(schema::CreateWebhookAttemptInput {
            endpointId: endpoint_id.to_owned(),
            sourceEventId: cratestack::uuid::Uuid::new_v4(),
            aggregateId: format!("sys-golden-aggregate-{suffix}"),
            eventType: "message.delivered".to_owned(),
            payload: "{}".to_owned(),
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
        .expect("seeding a WebhookAttempt");
    assert_system_can_read_back!(
        db,
        webhook_attempt,
        webhook_attempt,
        seeded.id,
        "WebhookAttempt",
        "@@allow(\"list\"/\"detail\", auth().kind == \"user\" || endpoint.appId == \
         auth().appId || hasRole('system'))"
    );
    seeded
}

/// Seeds one row per model in [`SYSTEM_READABLE_MODELS`] and proves a
/// system context can read each one back. This is the live half of #155's
/// guard — [`every_model_in_the_schema_is_classified`] above only checks
/// that every model is *classified*, not that the classification is
/// actually *true* against a real database.
///
/// # Proving this test can fail
///
/// Per #155's own acceptance criterion, this was verified to actually
/// fail: with `hasRole('system')` temporarily removed from `Provider`'s
/// `@@allow("read", ...)` in `schema/schema.cstack`, this test failed on
/// the `Provider` assertion with exactly the message
/// [`assert_system_can_read_back`] produces, naming `Provider` and its
/// (now-broken) clause. Restoring the clause restored a pass. See the PR
/// description for the exact diff and failure output.
///
/// #41 repeated the exercise for `WebhookEndpoint`, the model that PR
/// adds: with `hasRole('system')` removed from its `read` clause, this
/// test failed the same way, naming `WebhookEndpoint` and its
/// then-current (broken) clause; restoring it restored a pass. See #41's
/// PR description for that run's exact output. #40 repeated it a third
/// time for `WebhookAttempt`, this PR's own addition.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn every_system_readable_model_actually_admits_a_system_read() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let now = Utc::now();

    let app = seed_and_verify_app(&db, &suffix).await;
    let app_client = seed_and_verify_app_client(&db, &suffix, &app.id).await;
    seed_and_verify_oauth_client(&db, &suffix, &app_client.id).await;
    seed_and_verify_oauth_signing_key(&db).await;
    seed_and_verify_client_assertion(&db, &suffix, now).await;
    let sender_id = seed_and_verify_sender_id(&db, &suffix).await;
    let provider = seed_and_verify_provider(&db, &suffix).await;
    seed_and_verify_route(&db, &suffix, &provider.id).await;
    seed_and_verify_sender_id_registration(&db, &sender_id.id, &provider.id, now).await;
    seed_and_verify_operator_prefix_rule(&db).await;
    let message = seed_and_verify_message(&db, &suffix, &app.id, now).await;
    seed_and_verify_delivery_receipt(&db, &suffix, &message.id, &provider.id, now).await;
    seed_and_verify_job(&db, now).await;
    seed_and_verify_opt_out(&db, &suffix, now).await;
    let endpoint = seed_and_verify_webhook_endpoint(&db, &suffix, &app.id).await;
    seed_and_verify_webhook_attempt(&db, &suffix, &endpoint.id).await;
}
