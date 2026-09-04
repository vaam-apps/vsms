//! #25's own acceptance gate, closing epic #18 (M1 — sms-auth): a service
//! account provisioned through the real `provisionAppClient` procedure
//! actually *persists* — surviving a genuine OS process boundary, not just
//! an in-memory handle — and a real `client_credentials` +
//! `private_key_jwt` token exchanged against the restarted process can
//! call a protected route. Separately, a `developer`-role-shaped token is
//! refused on a `provider:update`-gated route.
//!
//! #25's own text is explicit about why this matters: *"An in-memory-only
//! test passes with the `GrantType` bug present and is therefore worthless
//! here."* Every other live suite in this workspace
//! (`oidc_flow_live.rs`, `rbac_layer2_live_postgres.rs`,
//! `send_message_live_postgres.rs`, ...) builds its axum router
//! in-process and drives it with `reqwest` against a bound
//! `TcpListener` — real HTTP, real Postgres, but the same OS process the
//! whole time. This file is the one exception: it spawns
//! `CARGO_BIN_EXE_sms-gateway` (the actual `sms-gateway serve` binary,
//! only reachable from an integration test inside *this* package — see
//! `backends/apps/sms-gateway/src/main.rs`, a binary crate with no `lib.rs`) as two
//! genuinely separate `std::process::Child` processes: provision against
//! the first, `kill()` (SIGKILL, not a graceful shutdown) and `wait()` it
//! to a real exit, then spawn a second, fresh process that has never held
//! anything about the first in memory, and prove the token exchange and a
//! protected mutation both work against *that* one.
//!
//! # Where this deliberately diverges from #25's own wording
//!
//! The ticket's own text says to provision "`provisionAppClient` over
//! HTTP" against the first process. That's not reachable in this
//! deployment: `provisionAppClient`'s own `@allow` in `schema.cstack` is
//! `hasRole('owner') || hasRole('admin')`, and `GatewayAuth::authenticate`
//! (this deployment's only `AuthProvider`) unconditionally sets `role:
//! "app"` for every real `client_credentials` token it ever issues — no
//! human-login path exists to obtain an `owner`/`admin`-role token from
//! (`sms_auth::op`'s own module doc; `rbac_layer2_live_postgres.rs`'s own
//! module doc reaches the identical conclusion for `PATCH
//! /providers/{id}`). So provisioning here calls
//! `Procedures::provision_app_client` directly — the exact real procedure
//! body `sms-gateway`'s own generated `$procs/provisionAppClient` route
//! would call, writing through the same `CrateStack` delegates (R1) to the
//! same live Postgres database both spawned processes are pointed at —
//! rather than round-tripping it through an HTTP route that no real token
//! in this deployment can reach. This is the same choice
//! `provision_app_client_live_postgres.rs` already made for the same
//! reason (see its own module doc). What *is* still exercised over real
//! HTTP, against a genuinely different OS process than the one that
//! provisioned it: the `/token` exchange and the protected mutation call
//! that follows it — the actual persistence claim this gate exists to
//! prove.
//!
//! The second half — a `developer`-role token refused on `provider:update`
//! — has the same gap `rbac_layer2_live_postgres.rs`'s own module doc
//! already documents: no token this deployment's real `/token` endpoint
//! can mint ever carries a `perms` claim at all (`sms_auth::op` never sets
//! one), so there is no real OAuth flow to obtain a `developer`-shaped
//! token from. [`sign_developer_stand_in_token`] hand-signs one directly
//! with `jsonwebtoken`, using the *same active OP signing key*
//! [`GatewayAuth`] validates against (so it passes real RS256/JWKS
//! validation structurally) — a deliberate stand-in for a future
//! human-login path, **not a real issuance route**. It reuses the
//! provisioned client's own `clientId` as `sub`, since
//! `GatewayAuth::authenticate` looks up an active `AppClient` by `sub`
//! regardless of what else a token claims — a token for a `client_id`
//! nothing in `app_client` recognises is rejected before `perms` is ever
//! read.
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. `sms_test_support` provisions Postgres and applies both
//! migrations automatically (a shared, self-healing container — see its
//! own module doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-gateway --test m1_acceptance_gate_live_postgres -- --ignored --nocapture
//! ```

use std::net::TcpListener as StdTcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration as StdDuration;

use authkestra_engine::token::Claims;
use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, SenderIdKind, SenderIdRegistrationStatus, oauth_signing_key,
    procedures::ProcedureRegistry, procedures::provision_app_client, provider as provider_filter,
};
use sms_api::{HashPepper, Procedures};

/// The `Provider.key` `sms-provider-orange-cm::OrangeCmProvider` reports —
/// `backends/apps/sms-gateway/src/main.rs`'s own `resolve_provider_row_id` looks up
/// exactly this row at startup and refuses to serve without it, since the
/// binary always mounts the DLR route (#34). Duplicated here rather than
/// imported: that constant is private to `sms-provider-orange-cm`'s own
/// `lib.rs` (`examples/send_test_message.rs` duplicates it the same way,
/// for the same reason).
const ORANGE_PROVIDER_KEY: &str = "orange_cm";

/// #134: `sms-gateway serve` now refuses to start without `--hash-pepper`
/// (env `SMS_HASH_PEPPER`) — both spawned `GatewayProcess`es below pass
/// this. This suite never sends a message (it provisions and exchanges
/// tokens, not `sendMessage`), so — like `provision_app_client_live_postgres.rs`
/// — the exact value doesn't matter, only that it clears `HashPepper::new`'s
/// minimum length.
const TEST_HASH_PEPPER: &str = "m1-acceptance-gate-live-postgres-test-pepper-over-minimum";

fn test_pepper() -> HashPepper {
    HashPepper::new(TEST_HASH_PEPPER).expect("test pepper meets HashPepper::new's minimum length")
}

fn sys() -> CratestackContext {
    Principal {
        sub: "m1-acceptance-gate-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CratestackContext {
    Principal {
        sub: "m1-acceptance-gate-test-owner".to_owned(),
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

/// A fresh MSISDN under the seeded `67x` (mtn) prefix — see
/// `backends/crates/sms-api/tests/send_message_live_postgres.rs`'s own copy of this
/// helper for why it needs cross-run, not just cross-call, uniqueness.
fn unique_mtn_msisdn() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .subsec_nanos();
    let unique = (u64::from(nanos) + n) % 1_000_000;
    format!("+237677{unique:06}")
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

/// An OS-assigned free loopback port, read then released — the small
/// window before `sms-gateway serve` actually binds it is the same
/// TOCTOU every other test in this workspace that binds `127.0.0.1:0`
/// then reads `local_addr()` accepts; nothing else in this test binary
/// competes for ports.
fn free_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port");
    listener
        .local_addr()
        .expect("reading the bound address")
        .port()
}

/// Idempotent: reuses an existing `orange_cm` `Provider` row (reactivating
/// it if some earlier run left it inactive) rather than creating a fresh
/// one every run — this database is never reset between runs, and
/// `sms-gateway serve` only ever wants the single row matching its
/// configured adapter's key. Mirrors `examples/send_test_message.rs`'s
/// own `ensure_provider`.
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
        if row.state != schema::ProviderState::active {
            db.provider()
                .update(row.id.clone())
                .set(schema::UpdateProviderInput {
                    state: Some(schema::ProviderState::active),
                    ..Default::default()
                })
                // #59: Provider is now @version'd.
                .if_match(row.version)
                .run(&owner())
                .await
                .expect("reactivating the orange_cm Provider row");
        }
        return row.id;
    }

    let created = db
        .provider()
        .create(schema::CreateProviderInput {
            key: ORANGE_PROVIDER_KEY.to_owned(),
            displayName: "Orange Cameroon (M1 acceptance gate test)".to_owned(),
            kind: schema::ProviderKind::orange_cm_http,
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
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the orange_cm Provider row");

    db.provider()
        .update(created.id.clone())
        .set(schema::UpdateProviderInput {
            state: Some(schema::ProviderState::active),
            ..Default::default()
        })
        // #59: Provider is now @version'd.
        .if_match(created.version)
        .run(&owner())
        .await
        .expect("activating the orange_cm Provider row");

    created.id
}

async fn seed_app(db: &Cratestack) -> schema::App {
    let suffix = unique_suffix();
    db.app()
        .create(schema::CreateAppInput {
            name: "m1 acceptance gate test app".to_owned(),
            slug: format!("m1-acceptance-gate-{suffix}"),
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
}

/// An active `SenderId` with an `approved` registration against
/// `provider_id` — `sendMessage`'s own `resolve_sender_id` requires both.
async fn seed_approved_sender(db: &Cratestack, provider_id: &str) -> String {
    let suffix = unique_suffix();
    let value = format!("T{}", &suffix[..suffix.len().min(9)]).to_uppercase();

    let sender = db
        .sender_id()
        .create(schema::CreateSenderIdInput {
            value: value.clone(),
            kind: SenderIdKind::alphanumeric,
            notes: None,
        })
        .run(&owner())
        .await
        .expect("seeding a sender id");

    db.sender_id_registration()
        .create(schema::CreateSenderIdRegistrationInput {
            senderIdId: sender.id.clone(),
            providerId: provider_id.to_owned(),
            status: SenderIdRegistrationStatus::approved,
            submittedAt: Some(Utc::now()),
            approvedAt: Some(Utc::now()),
            reference: None,
            rejectionReason: None,
        })
        .run(&owner())
        .await
        .expect("seeding an approved registration");

    db.sender_id()
        .update(sender.id)
        .set(schema::UpdateSenderIdInput {
            active: Some(true),
            ..Default::default()
        })
        // #59: SenderId is now @version'd.
        .if_match(sender.version)
        .run(&owner())
        .await
        .expect("activating the sender id");

    value
}

/// Reads back the PEM `sms_auth::op::rotate_signing_key` just persisted —
/// `system`-role only, per `OauthSigningKey`'s own schema comment (`@sensitive`
/// redacts audit snapshots only; this read policy is the real control, see
/// the repo's own root docs).
async fn read_signing_key_pem(db: &Cratestack, signing_key_id: &str) -> String {
    db.oauth_signing_key()
        .find_many()
        .where_expr(FilterExpr::from(
            oauth_signing_key::id().eq(signing_key_id.to_owned()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("reading back the signing key just rotated in")
        .into_iter()
        .next()
        .expect("the signing key rotate_signing_key just created must exist")
        .privateKeyPem
}

fn provision_args(app_id: &str, label: &str, scopes: Vec<String>) -> provision_app_client::Args {
    provision_app_client::Args {
        args: schema::ProvisionClientInput {
            appId: app_id.to_owned(),
            label: label.to_owned(),
            scopes,
        },
    }
}

#[derive(serde::Serialize)]
struct AssertionClaims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    jti: String,
    exp: i64,
}

/// A `private_key_jwt` client assertion per RFC 7523 §3, signed with
/// exactly the PEM `provisionAppClient` returned. No `kid` in the header —
/// `authkestra_op::client_assertion::verify_client_assertion`'s own
/// `select_key` treats a missing `kid` as "the jwks holds exactly one key,
/// use it," which a freshly provisioned client's `jwks` always does (see
/// `provision_app_client_live_postgres.rs`'s own identical comment).
fn sign_client_assertion(client_id: &str, issuer: &str, private_key_pem: &str) -> String {
    let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
        .expect("building an EncodingKey from the returned private key PEM");

    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    let claims = AssertionClaims {
        iss: client_id,
        sub: client_id,
        aud: issuer,
        jti: unique_suffix(),
        exp: (Utc::now() + Duration::minutes(1)).timestamp(),
    };

    jsonwebtoken::encode(&header, &claims, &encoding_key).expect("signing the client assertion")
}

/// Hand-signs a test-only token carrying the `developer` role's real
/// `perms` (§5.2 of the design doc, verbatim — no `provider:*` permission
/// at all), signed with the OP's own currently-active signing key so
/// `GatewayAuth`'s real RS256/JWKS validation accepts it structurally.
///
/// **This is not a real issuance path.** No flow in this deployment ever
/// mints a `perms`-bearing token — `sms_auth::op` never sets one, and
/// there is no human-login flow to obtain a role-bearing token from at
/// all (`sms_auth::op`'s own module doc). This exists solely to prove the
/// Layer-2 permission check on `PATCH /providers/{id}` actually denies a
/// `developer`-shaped caller, the same way `rbac_layer2_live_postgres.rs`
/// proves the equivalent for a real (scope-shaped) service-account token.
/// See this file's own module doc for the full reasoning.
fn sign_developer_stand_in_token(
    issuer: &str,
    signing_key_id: &str,
    signing_key_pem: &str,
    sub: &str,
) -> String {
    let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(signing_key_pem.as_bytes())
        .expect("building an EncodingKey from the OP's own active signing key PEM");

    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(signing_key_id.to_owned());

    let now = usize::try_from(Utc::now().timestamp()).expect("current unix time fits in usize");
    let exp = usize::try_from((Utc::now() + Duration::minutes(5)).timestamp())
        .expect("a five-minute-out expiry fits in usize");

    let perms = vec![
        serde_json::Value::String("app:read".to_owned()),
        serde_json::Value::String("webhook:manage".to_owned()),
        serde_json::Value::String("message:read".to_owned()),
        serde_json::Value::String("message:send".to_owned()),
    ];

    // authkestra-engine 0.8.0 marked `Claims` `#[non_exhaustive]` (see
    // AGENTS.md's authkestra-0.8 section, item A7's own finding —
    // `AuthorizeRequest` in `login.rs` hit the identical shape), so the
    // struct literal this function used to build no longer compiles from
    // outside the crate (E0639: cannot construct a non-exhaustive struct).
    // `Claims` still derives `serde::Deserialize` — it's the exact type
    // `validate_jwt_generic::<Claims>` decodes every real token into — so
    // building the wire shape by hand and deserializing it is the
    // sanctioned construction path a `#[non_exhaustive]` DTO with no
    // public constructor leaves open, and it produces the real `Claims`
    // type `GatewayAuth` will decode this signed token back into, not a
    // local stand-in shape that could drift from it.
    //
    // Same silent-field-loss shape `login.rs::build_authorize_request`'s
    // own doc comment names, worth repeating here rather than assuming:
    // `Claims` carries no `#[serde(deny_unknown_fields)]`, `iss`/`aud`/
    // `nbf`/`jti`/`scope` are all `Option`, and `extra` is
    // `#[serde(flatten)]` — a future authkestra rename of any of those
    // named fields wouldn't fail this `from_value` call at all, it would
    // silently produce `None` for the renamed field and stash the
    // now-unrecognised original key inside `extra` instead, `Ok`. Lower
    // stakes here than in production code (`sign_developer_stand_in_token`
    // is test-only, building a fixture whose shape this test itself
    // controls completely, and every field this test's own assertions
    // actually depend on — `perms`, via `extra`'s flatten catch-all, which
    // ignores field renames entirely — is unaffected either way), so this
    // stays `.expect(...)` rather than a graceful `Result` the way
    // `login.rs`'s real production version is. Named here rather than
    // silently relied on.
    let mut claims_value = serde_json::json!({
        "iss": issuer,
        "sub": sub,
        "aud": null,
        "exp": exp,
        "iat": now,
        "nbf": null,
        "jti": null,
        "scope": null,
    });
    if let serde_json::Value::Object(fields) = &mut claims_value {
        fields.insert("perms".to_owned(), serde_json::Value::Array(perms));
    }
    let claims: Claims = serde_json::from_value(claims_value).expect(
        "hand-built JSON matches Claims's own field names exactly — see this function's own \
         doc comment for why deserialization replaces the struct literal this test used before \
         authkestra-engine 0.8.0",
    );

    jsonwebtoken::encode(&header, &claims, &encoding_key)
        .expect("signing the hand-crafted developer-perms stand-in token")
}

async fn request_token(
    issuer: &str,
    client_id: &str,
    assertion: &str,
    scope: &str,
) -> serde_json::Value {
    let response = reqwest::Client::new()
        .post(format!("{issuer}/token"))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            (
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
            ),
            ("client_assertion", assertion),
            ("scope", scope),
        ])
        .send()
        .await
        .expect("POSTing to /token");
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .expect("parsing the token response as JSON");
    assert!(
        status.is_success(),
        "token request failed ({status}): {body}"
    );
    body
}

fn access_token(token_response: &serde_json::Value) -> &str {
    token_response["access_token"]
        .as_str()
        .expect("token response carries access_token")
}

/// A real, spawned `sms-gateway serve` OS process — `env!("CARGO_BIN_EXE_sms-gateway")`
/// is only set for an integration test inside *this* package, which is
/// exactly why this file lives here rather than in `sms-api`/`sms-auth`
/// alongside this workspace's other live suites.
struct GatewayProcess {
    child: Child,
    issuer: String,
}

impl GatewayProcess {
    /// Spawns `sms-gateway serve` bound to `127.0.0.1:{port}` against
    /// `db_url`, with dummy-but-well-formed Orange config (never
    /// exercised — this suite never runs `dispatch`, so the adapter is
    /// constructed but never asked to reach the network) and waits for it
    /// to actually start serving before returning.
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
            .arg("m1-acceptance-gate-test-orange-client-id")
            .arg("--orange-client-secret")
            .arg("m1-acceptance-gate-test-orange-client-secret")
            .arg("--orange-sender-number")
            .arg("+237600000000")
            .arg("--hash-pepper")
            .arg(TEST_HASH_PEPPER)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = command.spawn().expect("spawning sms-gateway serve");
        println!(
            "m1_acceptance_gate_live_postgres: spawned sms-gateway serve, pid {}, issuer {issuer}",
            child.id()
        );

        wait_until_ready(&issuer, &mut child).await;
        Self { child, issuer }
    }

    /// SIGKILL, then reap — a hard kill, not a graceful shutdown, so
    /// nothing this process held in memory (and nothing SIGTERM's
    /// graceful-drain path might otherwise flush) survives it. Blocks
    /// until the OS has genuinely finished exiting the process, which is
    /// why the caller can safely reuse the same port immediately
    /// afterward. Run on a blocking thread: `Child::kill`/`Child::wait`
    /// are synchronous syscalls, and this is called from async test code.
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
                "m1_acceptance_gate_live_postgres: killed sms-gateway serve, pid {pid}, exit status {status:?}"
            );
        })
        .await
        .expect("joining the blocking kill/wait task");
    }
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        // Best-effort safety net: if a test assertion panics before
        // `kill_and_wait` runs, don't leave an orphaned `sms-gateway
        // serve` holding the port for the rest of this test binary's run.
        // A no-op (ignored) if the process was already reaped.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Polls `/.well-known/openid-configuration` until it answers, or fails
/// fast if the child has already exited (a misconfiguration — e.g. the
/// `orange_cm` `Provider` row this suite seeds up front going missing —
/// would otherwise just look like a slow-starting server until this
/// function's own timeout, which is a much less useful failure to debug).
async fn wait_until_ready(issuer: &str, child: &mut Child) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(15);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("sms-gateway serve exited before becoming ready: {status:?}");
        }
        if let Ok(response) = client
            .get(format!("{issuer}/.well-known/openid-configuration"))
            .send()
            .await
            && response.status().is_success()
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "sms-gateway serve never became ready within 15s"
        );
        tokio::time::sleep(StdDuration::from_millis(200)).await;
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_persisted_client_credentials_client_survives_a_process_restart_and_a_developer_token_is_refused()
 {
    let db_url = sms_test_support::database_url().await;
    let db = db().await;

    // --- Setup: everything `sms-gateway serve` itself requires before it
    // will even start (an active signing key; a `Provider` row keyed
    // `orange_cm`, `resolve_provider_row_id`'s own requirement), plus what
    // `sendMessage` needs downstream (an approved `SenderId`, an `App` to
    // provision a client for). ---
    let signing_key_id =
        sms_auth::op::rotate_signing_key(&db, &sys(), sms_auth::op::ROTATION_OVERLAP)
            .await
            .expect("rotating in a first signing key");
    let signing_key_pem = read_signing_key_pem(&db, &signing_key_id).await;

    let provider_id = ensure_orange_cm_provider(&db).await;
    let app = seed_app(&db).await;
    let sender_id_value = seed_approved_sender(&db, &provider_id).await;

    let port = free_port();

    // 1 & 2: process #1.
    let process_one = GatewayProcess::spawn(&db_url, port).await;

    // 3: provision a service account for real, through the actual
    // `provision_app_client` procedure — real Postgres persistence, real
    // RSA keygen, real transaction. See this file's own module doc for
    // why this is a direct procedure call rather than an HTTP request to
    // process #1.
    let procedures = Procedures::new(test_pepper());
    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db` — see `provision_app_client_live_postgres.rs`'s
    // identical comment.
    let args = provision_args(
        &app.id,
        "m1 acceptance gate client",
        vec!["sms:send".to_owned()],
    );
    // `&owner()` called twice in one statement would borrow a temporary that
    // doesn't outlive the closure's returned future (E0515) — bind it once
    // instead.
    let ctx = owner();
    let provisioned = provision_app_client::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.provision_app_client(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("provisioning a service account through the real procedure");
    assert!(
        provisioned.privateKeyPem.contains("PRIVATE KEY"),
        "provisioning must return a real PEM-encoded private key"
    );

    // 4: kill process #1 for real. Nothing beyond this point can be
    // served from memory process #1 held — it no longer exists.
    process_one.kill_and_wait().await;

    // 5: process #2 — a genuinely fresh OS process, same database, same
    // port, zero shared memory with process #1. If provisioning had only
    // updated in-memory state, or if the `GrantType` `#[serde(untagged)]`
    // bug (#6) were still present, this is where it would show: process
    // #2 has never observed `provisioned.clientId` except through
    // Postgres.
    let process_two = GatewayProcess::spawn(&db_url, port).await;

    // 6: exchange the persisted private key for a real token against
    // process #2's own /token, then call a real protected mutation with
    // it — proving the whole chain works end to end only because the row
    // is actually in Postgres.
    assert_persisted_client_can_send_after_restart(&process_two, &provisioned, &sender_id_value)
        .await;

    // 7: the negative RBAC case — a hand-signed, test-only token carrying
    // the `developer` role's real perms (no `provider:*` permission at
    // all) must be refused on `PATCH /providers/{id}`, naming the missing
    // `provider:update` permission. See `sign_developer_stand_in_token`'s
    // own doc for why this is hand-signed rather than obtained from
    // `/token`.
    assert_developer_stand_in_token_is_refused_on_provider_write(
        &process_two,
        &signing_key_id,
        &signing_key_pem,
        &provisioned.clientId,
        &provider_id,
    )
    .await;

    process_two.kill_and_wait().await;
}

/// Step 6 of the gate: a real `/token` exchange against `process`, then a
/// real `sendMessage` call with the resulting access token — proving the
/// whole chain works end to end only because `provisioned`'s row is
/// actually in Postgres, not because `process` shares any memory with
/// whatever provisioned it.
async fn assert_persisted_client_can_send_after_restart(
    process: &GatewayProcess,
    provisioned: &schema::ProvisionClientResult,
    sender_id_value: &str,
) {
    let assertion = sign_client_assertion(
        &provisioned.clientId,
        &process.issuer,
        &provisioned.privateKeyPem,
    );
    let token_response = request_token(
        &process.issuer,
        &provisioned.clientId,
        &assertion,
        "sms:send",
    )
    .await;
    assert_eq!(
        token_response["scope"].as_str(),
        Some("sms:send"),
        "the OP must echo back exactly the scope the persisted client was provisioned with: \
         {token_response}"
    );
    let token = access_token(&token_response);

    let send_response = reqwest::Client::new()
        .post(format!("{}/$procs/sendMessage", process.issuer))
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({"args": {
            "to": unique_mtn_msisdn(),
            "body": "M1 acceptance gate (#25): a persisted client_credentials client, \
                     exchanged and used against a genuinely restarted process",
            "senderId": sender_id_value,
        }}))
        .send()
        .await
        .expect("calling sendMessage against the restarted process");
    let send_status = send_response.status();
    let send_body: serde_json::Value = send_response
        .json()
        .await
        .expect("parsing the sendMessage response");
    assert!(
        send_status.is_success(),
        "a persisted, freshly-token-exchanged client must be able to send ({send_status}): \
         {send_body}"
    );
    assert!(
        send_body["messageId"].as_str().is_some(),
        "a successful send returns the persisted message's id: {send_body}"
    );
}

/// Step 7 of the gate: a hand-signed, test-only token carrying the
/// `developer` role's real perms must be refused on `PATCH
/// /providers/{id}`, naming the missing `provider:update` permission. See
/// `sign_developer_stand_in_token`'s own doc for why this token is
/// hand-signed rather than obtained from `/token`.
async fn assert_developer_stand_in_token_is_refused_on_provider_write(
    process: &GatewayProcess,
    signing_key_id: &str,
    signing_key_pem: &str,
    client_id: &str,
    provider_id: &str,
) {
    let developer_token =
        sign_developer_stand_in_token(&process.issuer, signing_key_id, signing_key_pem, client_id);

    let patch_response = reqwest::Client::new()
        .patch(format!("{}/providers/{}", process.issuer, provider_id))
        .bearer_auth(developer_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({"maxTps": 10.0}))
        .send()
        .await
        .expect("calling PATCH /providers/{id} with a hand-signed developer-perms token");
    assert_eq!(patch_response.status(), reqwest::StatusCode::FORBIDDEN);
    let patch_body: serde_json::Value =
        patch_response.json().await.expect("parsing the error body");
    assert!(
        patch_body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("provider:update"),
        "expected the denial to name the missing permission: {patch_body}"
    );
}
