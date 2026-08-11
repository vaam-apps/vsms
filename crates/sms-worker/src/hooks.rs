//! `Role::Hooks`'s real body — #40. Claims due `WebhookAttempt` rows via
//! [`crate::claim::claim_batch`], signs and POSTs them with `sms-webhook`
//! (#41), and drives `pending`/`failed -> delivering -> succeeded|failed|dead`
//! per §8.5. `claim.rs`'s own `impl Claimable for WebhookAttempt` owns the
//! claim half (endpoint-health filtering, the crash-reclaim same-state
//! write); this module owns everything after a row reaches `delivering`.
//!
//! # The missing transition table, and the position this PR takes on it
//!
//! `AttemptState` shipped with #38/#39, but no `attempt_state_transitions`
//! table or trigger existed — nothing had written `WebhookAttempt.state` yet,
//! so R2 ("proposed by Rust, decided by Postgres") had nothing to decide
//! against. #38/#39's own PR left this exactly as open as it found it,
//! flagging it as the next role's problem. This PR resolves it in favour of
//! the same discipline `messages`/`jobs` already get — `attempt_state_
//! transitions` + `attempts_guard_transition` (§2.10, `0002_bootstrap`) —
//! rather than arguing webhook attempts are somehow exempt from R2. They
//! aren't: an illegal edge here (a bug in this file, a future admin-console
//! replay feature, a stray `psql` session) deserves the same SQLSTATE `SM001`
//! → `409 Conflict` backstop every other state machine in this system gets,
//! not a silent write or an opaque `500`.
//!
//! # What `WebhookAttempt.payload` contains, and what actually gets sent
//!
//! `payload` (written by `crates/sms-api/src/webhooks.rs`'s subscribers) is
//! the §8.4 `data` object *only* — not the outer envelope. This module
//! builds the envelope at delivery time ([`build_envelope`]): `id` from
//! `WebhookAttempt.id` (the only value that has existed since the row was
//! created — see `webhooks.rs`'s own doc for why the envelope's `id` can't
//! be anything else), `type` from `eventType`, `data` from `payload` parsed
//! back into a JSON value (never re-wrapped as a string — nesting an
//! already-JSON string inside a JSON string is exactly the kind of "close
//! but not the contract" bug a receiver's own parser would silently choke
//! on), and `occurredAt` — see [`build_envelope`]'s own doc for why this is
//! a documented approximation, not the original event's timestamp.
//!
//! **What gets signed is exactly what gets sent — never a second, later
//! re-serialization of the same logical value.** [`build_envelope`] returns
//! one `String`; its bytes are both what [`sms_webhook::sign_header`] HMACs
//! and what `reqwest` puts on the wire as the request body. Signing
//! anything else — the parsed `serde_json::Value` re-serialized a second
//! time by a different call site, say — would be the exact silent bug this
//! module's own doc (and #41's) warns about: `serde_json::Value`'s map type
//! does not preserve key order or exact whitespace, so a second
//! serialization is not guaranteed to produce byte-identical output to the
//! first, even for logically identical JSON.
//!
//! # `maskRecipient` — enforced upstream, not re-derived here
//!
//! §4.4/§8.4: an endpoint configured for masked recipients must never see a
//! plaintext MSISDN. That masking happens once, at insert time, in
//! `crates/sms-api/src/webhooks.rs::message_payload` — `payload`'s `to`
//! field is already whatever the matched endpoint's `maskRecipient` called
//! for by the time this module ever reads the row. This module's own
//! correctness obligation is narrower but just as real: never reconstruct
//! or enrich that value from anything else this crate can reach (the
//! `Message` row, if this crate ever gained a reason to read one here) —
//! [`build_envelope`] parses `payload` into a `data` object and embeds it
//! verbatim, touching no field of it individually, so there is no code path
//! by which a plaintext MSISDN could re-enter the outbound body even by
//! accident. `hooks_live_postgres.rs`'s `mask_recipient_payload_is_forwarded_
//! verbatim_never_reconstructed` test proves this against a real HTTP
//! capture, not just by reading this paragraph.

use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use cratestack::{CoolContext, CoolError, FilterExpr};
use reqwest::StatusCode;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::map_database_error;
use sms_api::schema::{
    webhook_endpoint, AttemptState, UpdateWebhookAttemptInput, UpdateWebhookEndpointInput,
    WebhookAttempt, WebhookEndpoint,
};
use tracing::{error, warn};

use crate::claim::claim_batch;
use crate::WorkerContext;

/// How often this loop polls for claimable attempts. Same order of
/// magnitude as `dispatch`/`jobs`'s own poll intervals — `hooks` is
/// scale-to-N (§7.1), so a short interval per instance is cheap, not a
/// throughput ceiling the way `dispatch`'s TPS-derived budget is.
const POLL_INTERVAL: StdDuration = StdDuration::from_secs(1);

/// How many attempts one poll claims at once, before endpoint-health
/// filtering (`claim.rs`'s own `CANDIDATE_OVERFETCH_FACTOR` fetches beyond
/// this). Fixed, matching `jobs::BUDGET`'s own reasoning: no external
/// throughput ceiling constrains `hooks` the way Orange's contract
/// constrains `dispatch`.
const BUDGET: i64 = 20;

/// §8.5, verbatim: "Timeout 10s per attempt." Set on the client at
/// construction (`build_http_client`), not per-request — same pattern
/// `sms-provider-orange-cm::OrangeCmProvider::new` already uses for the
/// identical reason (`reqwest::Client::new()` sets no timeout at all, and a
/// connection that hangs forever is worse than one that fails fast).
const REQUEST_TIMEOUT: StdDuration = StdDuration::from_secs(10);

/// §8.5: "20 consecutive failures sets `circuitOpenUntil`."
const CIRCUIT_FAILURE_THRESHOLD: i64 = 20;

/// §8.5: "`circuitOpenUntil = now + 15min`."
const CIRCUIT_OPEN_DURATION: Duration = Duration::minutes(15);

/// §8.5, verbatim: "1s, 5s, 25s, 2m, 10m, 1h, 6h, 24h — eight attempts, then
/// `dead`." The schedule is fixed and shared; how many entries into it a
/// given endpoint actually gets before `dead` is `WebhookEndpoint.maxAttempts`
/// — a per-endpoint column, not this array's length (see [`write_outcome`]).
const BACKOFF_SCHEDULE: [Duration; 8] = [
    Duration::seconds(1),
    Duration::seconds(5),
    Duration::seconds(25),
    Duration::minutes(2),
    Duration::minutes(10),
    Duration::hours(1),
    Duration::hours(6),
    Duration::hours(24),
];

/// `attempts` is 1-based (incremented at claim time, before this ever runs)
/// — `backoff_for(1)` is the delay after the *first* failure. Capped at the
/// schedule's last entry past its length, same as `jobs::backoff_for` and
/// for the same reason: a schedule this short with a `maxAttempts` this
/// repo's own seed data sets to 8 never actually indexes past the end in
/// practice, but an endpoint with a larger `maxAttempts` must not panic.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn backoff_for(attempts: i64) -> Duration {
    let index = (attempts - 1).max(0) as usize;
    BACKOFF_SCHEDULE[index.min(BACKOFF_SCHEDULE.len() - 1)]
}

/// The `system` context this role does all its work under.
fn sys(worker: &str) -> CoolContext {
    Principal {
        sub: format!("sms-worker:hooks:{worker}"),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// A client with §8.5's own 10s timeout — `reqwest::Client::new()` sets
/// none, so building one explicitly is what actually enforces the "10s per
/// attempt" contract rather than merely documenting it.
///
/// # Panics
///
/// Never in practice: the only way `ClientBuilder::build` fails is a TLS
/// backend misconfiguration, and this builder sets only a timeout — same
/// reasoning `OrangeCmProvider::new`'s identical `.expect(...)` already
/// relies on.
fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("reqwest client builder with only a timeout set never fails")
}

/// Never returns on its own, matching [`crate::run`]'s contract for every
/// other role.
pub async fn run(ctx: WorkerContext, worker: &str) {
    let sys = sys(worker);
    let http = build_http_client();
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    loop {
        interval.tick().await;
        if let Err(error) = tick(&ctx, &sys, worker, &http).await {
            error!(%error, "hooks tick failed; retrying next poll");
        }
    }
}

/// One poll iteration: claim, then deliver every claimed row. `pub` for the
/// same reason `dispatch::tick`/`jobs::tick` are — live tests drive exactly
/// one iteration deterministically instead of racing [`run`]'s own timer.
///
/// Unlike `dispatch`/`jobs`, every successful claim result already carries
/// the state this loop wants to act on (`delivering`) — `claim.rs`'s own
/// `take_lease` targets it from both branches (`pending`/`failed` and the
/// crash-reclaim), so there is no "was this actually a fresh claim, or just
/// a routing hop" filter to apply before delivering, the way `dispatch`
/// filters out `queued` and `jobs` filters out `pending`.
pub async fn tick(
    ctx: &WorkerContext,
    sys: &CoolContext,
    worker: &str,
    http: &reqwest::Client,
) -> Result<(), CoolError> {
    let claimed = claim_batch::<WebhookAttempt>(&ctx.db, sys, worker, BUDGET).await?;
    for attempt in claimed {
        deliver_one(ctx, sys, http, attempt).await;
    }
    Ok(())
}

/// The four shapes a delivery attempt resolves to, before each is
/// translated into a `WebhookAttempt`/`WebhookEndpoint` write by
/// [`write_outcome`]. Kept distinct from [`AttemptState`] itself because
/// `Retryable` still branches on `maxAttempts` before it knows whether the
/// resulting state is `failed` or `dead`.
enum Outcome {
    /// 2xx. `status` is the exact code, recorded for operator visibility.
    Success { status: u16 },
    /// 410 Gone (§8.5): stop retrying *and* deactivate the endpoint,
    /// unconditionally — distinct from an ordinary exhausted-attempts
    /// `dead`, which touches only the attempt.
    Gone,
    /// A non-2xx, non-410 status, or a transport error — genuinely a signal
    /// about the *endpoint*, so this is what feeds `maxAttempts`/backoff
    /// and the circuit breaker.
    Retryable {
        status: Option<u16>,
        message: String,
    },
    /// The stored `payload` could not even be parsed into a request body —
    /// no HTTP call was ever attempted. Deliberately **not** folded into
    /// `Retryable`: this is a bug in our own subscriber (#38 writes
    /// `payload`), not any signal about the endpoint, and a completely
    /// healthy endpoint must never have its circuit breaker tripped by a
    /// row it never even received a request for. See [`write_outcome`]'s
    /// own handling of this variant for why it goes straight to `dead`
    /// rather than retrying — a stored payload does not become parseable
    /// on a later attempt, so retrying only burns `maxAttempts` before
    /// reaching the same `dead` outcome anyway, more slowly and with the
    /// same endpoint-blaming bug on every attempt in between.
    MalformedPayload { message: String },
}

/// Deliver one already-`delivering` attempt and write back whichever
/// transition its outcome implies. A write failure here is logged, not
/// propagated — one attempt's DB write failing must not stall the rest of
/// this tick's batch, same reasoning as `dispatch::submit_one`/
/// `jobs::run_one`.
async fn deliver_one(
    ctx: &WorkerContext,
    sys: &CoolContext,
    http: &reqwest::Client,
    attempt: WebhookAttempt,
) {
    // The FK `wha_endpoint_fk ... ON DELETE CASCADE` (§2.10) means a
    // `WebhookAttempt` cannot outlive its `WebhookEndpoint` under normal
    // operation — `None` here would mean the row survived its own parent's
    // deletion, which should be structurally impossible. Treated as a
    // transient condition rather than a `dead` write: log and leave the row
    // `delivering`, so the crash-reclaim lease-expiry path (`claim.rs`)
    // retries it, the same way an actual database error below does.
    let endpoint = match fetch_endpoint(ctx, sys, &attempt.endpointId).await {
        Ok(Some(endpoint)) => endpoint,
        Ok(None) => {
            error!(
                attempt_id = %attempt.id, endpoint_id = %attempt.endpointId,
                "claimed attempt's endpoint no longer exists (should be impossible under \
                 ON DELETE CASCADE); leaving delivering for lease-expiry reclaim"
            );
            return;
        }
        Err(error) => {
            error!(
                attempt_id = %attempt.id, %error,
                "fetching the endpoint for a claimed attempt failed; leaving delivering for \
                 lease-expiry reclaim"
            );
            return;
        }
    };

    let now = Utc::now();
    let body = match build_envelope(&attempt, now) {
        Ok(body) => body,
        Err(error) => {
            write_outcome(
                ctx,
                sys,
                &attempt,
                &endpoint,
                Outcome::MalformedPayload {
                    message: format!("payload did not parse as JSON: {error}"),
                },
                now,
            )
            .await;
            return;
        }
    };

    let outcome = send(http, &endpoint, &attempt, &body, now).await;
    write_outcome(ctx, sys, &attempt, &endpoint, outcome, now).await;
}

/// The actual signed HTTP POST. Split from [`deliver_one`] so the
/// request-building/signing logic is directly unit-testable without a
/// database — [`build_envelope`] and the header values below are pure
/// functions of their inputs.
async fn send(
    http: &reqwest::Client,
    endpoint: &WebhookEndpoint,
    attempt: &WebhookAttempt,
    body: &str,
    now: DateTime<Utc>,
) -> Outcome {
    let timestamp = now.timestamp();
    let event_id = attempt.sourceEventId.to_string();
    // Current secret first, `prevSecret` second — "oldest last" per §4.4;
    // `sign_header` reuses the canonical string across both rather than
    // recomputing it. `verify`'s own contract doesn't care about this
    // order, but `sign_header`'s caller-facing convention does.
    let secrets: Vec<&str> = std::iter::once(endpoint.secret.as_str())
        .chain(endpoint.prevSecret.as_deref())
        .collect();
    let signature = sms_webhook::sign_header(&secrets, timestamp, &event_id, body.as_bytes());

    let result = http
        .post(&endpoint.url)
        .header(sms_webhook::HEADER_EVENT, &attempt.eventType)
        .header(sms_webhook::HEADER_EVENT_ID, &event_id)
        .header(sms_webhook::HEADER_TIMESTAMP, timestamp.to_string())
        .header(sms_webhook::HEADER_SIGNATURE, signature)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_owned())
        .send()
        .await;

    match result {
        Ok(response) if response.status().is_success() => Outcome::Success {
            status: response.status().as_u16(),
        },
        Ok(response) if response.status() == StatusCode::GONE => Outcome::Gone,
        Ok(response) => Outcome::Retryable {
            status: Some(response.status().as_u16()),
            message: format!("endpoint responded {}", response.status()),
        },
        Err(error) => Outcome::Retryable {
            status: None,
            message: error.to_string(),
        },
    }
}

/// The §8.4 envelope — `{id, type, occurredAt, data}` — built from the
/// stored row at delivery time. This module's own doc covers `id`/`type`;
/// documented here is `occurredAt`, the one field that is a genuine,
/// tracked approximation rather than a faithful reconstruction.
///
/// `WebhookAttempt` carries no creation timestamp — no `@use(Timestamps)`
/// on the model — and the framework's own `ModelEvent::occurred_at` (the
/// original event's real timestamp) is read and discarded by #38's
/// subscriber before this row exists to store it on. So `occurredAt` here
/// is stamped with `now` — the time of *this delivery attempt*, not the
/// original event. Accurate to within a second or two for a first attempt
/// (subscriber delivery is synchronous with the mutation that caused the
/// event, §8.2), increasingly approximate under retries, and wrong by up to
/// the full backoff schedule's span (up to 24h) for an attempt that only
/// succeeds on its last try. See §8.5's own "Implementation, #40" note for
/// the two ways to close this properly — both are schema/subscriber changes
/// to a model #38 owns, not something this function can fix alone.
fn build_envelope(
    attempt: &WebhookAttempt,
    now: DateTime<Utc>,
) -> Result<String, serde_json::Error> {
    let data: serde_json::Value = serde_json::from_str(&attempt.payload)?;
    let envelope = serde_json::json!({
        "id": attempt.id,
        "type": attempt.eventType,
        "occurredAt": now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "data": data,
    });
    Ok(envelope.to_string())
}

async fn fetch_endpoint(
    ctx: &WorkerContext,
    sys: &CoolContext,
    endpoint_id: &str,
) -> Result<Option<WebhookEndpoint>, CoolError> {
    Ok(ctx
        .db
        .webhook_endpoint()
        .find_many()
        .where_expr(FilterExpr::from(
            webhook_endpoint::id().eq(endpoint_id.to_owned()),
        ))
        .limit(1)
        .run(sys)
        .await?
        .into_iter()
        .next())
}

/// Write whichever `WebhookAttempt`/`WebhookEndpoint` transition `outcome`
/// implies. Up to two writes — the attempt's own state and the endpoint's
/// failure bookkeeping are separate rows with no shared transaction here
/// (`WebhookEndpoint` has no `@version` to make a combined CAS meaningful,
/// and the two writes have independent failure modes worth logging
/// separately) — but which arms touch the endpoint at all is the load-
/// bearing decision in this function: `Success`/`Gone`/`Retryable` all
/// reflect something the endpoint actually did (or failed to do), so all
/// three update its bookkeeping; `MalformedPayload` reflects nothing about
/// the endpoint — no request was ever sent to it — and must not. Errors
/// from either write are logged, not propagated, matching every other
/// outcome-writer in this crate.
async fn write_outcome(
    ctx: &WorkerContext,
    sys: &CoolContext,
    attempt: &WebhookAttempt,
    endpoint: &WebhookEndpoint,
    outcome: Outcome,
    now: DateTime<Utc>,
) {
    match outcome {
        Outcome::Success { status } => {
            write_succeeded(ctx, sys, attempt, status, now).await;
            reset_endpoint_failures(ctx, sys, endpoint).await;
        }
        Outcome::Gone => {
            write_dead(
                ctx,
                sys,
                attempt,
                Some(410),
                "endpoint returned 410 Gone; deactivating".to_owned(),
                now,
            )
            .await;
            deactivate_endpoint(ctx, sys, endpoint).await;
        }
        Outcome::Retryable { status, message } => {
            // `attempt.attempts` was already incremented at claim time
            // (claim.rs's take_lease) — this delivery *is* that counted
            // attempt, so comparing it against maxAttempts here, rather
            // than after another increment, is what makes the Nth failure
            // against an N-maxAttempts endpoint the one that goes `dead`.
            if attempt.attempts >= endpoint.maxAttempts {
                write_dead(
                    ctx,
                    sys,
                    attempt,
                    status,
                    format!("max attempts ({}) reached: {message}", endpoint.maxAttempts),
                    now,
                )
                .await;
            } else {
                write_failed_retry(ctx, sys, attempt, status, message, now).await;
            }
            record_endpoint_failure(ctx, sys, endpoint, now).await;
        }
        Outcome::MalformedPayload { message } => {
            // Loud on purpose — this is our own bug (#38's subscriber wrote
            // an unparseable payload), not routine traffic, and #42's
            // reap_outbox precedent is exactly "make a broken row loud
            // rather than silently retry or hide it." Straight to `dead`:
            // the stored payload will not become parseable on a later
            // attempt, so retrying only delays an identical outcome while
            // burning `maxAttempts` — and, unlike `Retryable`, this
            // deliberately never calls `record_endpoint_failure`: no HTTP
            // request was ever sent, so the endpoint did nothing wrong and
            // its circuit breaker must not react to this attempt at all.
            error!(
                attempt_id = %attempt.id, endpoint_id = %endpoint.id, message,
                "webhook attempt payload is malformed — a bug in our own subscriber, not the \
                 endpoint; marking dead without touching endpoint circuit-breaker state"
            );
            write_dead(
                ctx,
                sys,
                attempt,
                None,
                format!("malformed payload, not retried: {message}"),
                now,
            )
            .await;
        }
    }
}

async fn write_succeeded(
    ctx: &WorkerContext,
    sys: &CoolContext,
    attempt: &WebhookAttempt,
    status: u16,
    now: DateTime<Utc>,
) {
    if let Err(error) = ctx
        .db
        .webhook_attempt()
        .update(attempt.id.clone())
        .set(UpdateWebhookAttemptInput {
            state: Some(AttemptState::succeeded),
            lastStatusCode: Some(Some(i64::from(status))),
            lastError: Some(None),
            lastAttemptAt: Some(Some(now)),
            leaseOwner: Some(None),
            leaseUntil: Some(None),
            ..Default::default()
        })
        .if_match(attempt.version)
        .run(sys)
        .await
    {
        log_write_failure(
            &attempt.id,
            AttemptState::succeeded,
            &map_database_error(error),
        );
    }
}

/// `delivering -> failed`, with `nextAttemptAt` set per [`backoff_for`] —
/// the row's actual resting state until the next due-list poll picks it up
/// again (`claim.rs`'s `candidates`, via `webhook_due_idx`).
async fn write_failed_retry(
    ctx: &WorkerContext,
    sys: &CoolContext,
    attempt: &WebhookAttempt,
    status: Option<u16>,
    message: String,
    now: DateTime<Utc>,
) {
    let next_attempt_at = now + backoff_for(attempt.attempts);
    if let Err(error) = ctx
        .db
        .webhook_attempt()
        .update(attempt.id.clone())
        .set(UpdateWebhookAttemptInput {
            state: Some(AttemptState::failed),
            lastStatusCode: Some(status.map(i64::from)),
            lastError: Some(Some(message)),
            lastAttemptAt: Some(Some(now)),
            nextAttemptAt: Some(Some(next_attempt_at)),
            leaseOwner: Some(None),
            leaseUntil: Some(None),
            ..Default::default()
        })
        .if_match(attempt.version)
        .run(sys)
        .await
    {
        log_write_failure(
            &attempt.id,
            AttemptState::failed,
            &map_database_error(error),
        );
    }
}

async fn write_dead(
    ctx: &WorkerContext,
    sys: &CoolContext,
    attempt: &WebhookAttempt,
    status: Option<u16>,
    message: String,
    now: DateTime<Utc>,
) {
    if let Err(error) = ctx
        .db
        .webhook_attempt()
        .update(attempt.id.clone())
        .set(UpdateWebhookAttemptInput {
            state: Some(AttemptState::dead),
            lastStatusCode: Some(status.map(i64::from)),
            lastError: Some(Some(message)),
            lastAttemptAt: Some(Some(now)),
            leaseOwner: Some(None),
            leaseUntil: Some(None),
            ..Default::default()
        })
        .if_match(attempt.version)
        .run(sys)
        .await
    {
        log_write_failure(&attempt.id, AttemptState::dead, &map_database_error(error));
    }
}

/// A failed delivery's endpoint-side bookkeeping — `consecutiveFailures`
/// incremented, and the circuit opened (with the counter reset to zero) the
/// moment it crosses [`CIRCUIT_FAILURE_THRESHOLD`]. By construction this is
/// never called for an endpoint whose circuit is already open —
/// `claim.rs`'s own `filter_by_endpoint_health` excludes such an endpoint's
/// attempts from ever being claimed, so `hooks` never attempts, and
/// therefore never records a failure against, a breaker that has already
/// tripped.
///
/// No `@version`/CAS on this write (`WebhookEndpoint` has none) — a
/// best-effort read-modify-write, same class of race `rotateWebhookSecret`
/// already accepts for this model (its own `@isolation("serializable")` tx
/// closes a *different* race, not this one). Under concurrent `hooks`
/// workers failing against the same endpoint at once, `consecutiveFailures`
/// can under-count by a small amount — acceptable for a heuristic that
/// exists to stop hammering a dead endpoint, not to bill anyone.
async fn record_endpoint_failure(
    ctx: &WorkerContext,
    sys: &CoolContext,
    endpoint: &WebhookEndpoint,
    now: DateTime<Utc>,
) {
    let consecutive = endpoint.consecutiveFailures + 1;
    let opening_circuit = consecutive >= CIRCUIT_FAILURE_THRESHOLD;
    let next_consecutive = if opening_circuit { 0 } else { consecutive };

    let mut set = UpdateWebhookEndpointInput {
        consecutiveFailures: Some(next_consecutive),
        ..Default::default()
    };
    if opening_circuit {
        set.circuitOpenUntil = Some(Some(now + CIRCUIT_OPEN_DURATION));
        warn!(
            endpoint_id = %endpoint.id,
            threshold = CIRCUIT_FAILURE_THRESHOLD,
            "circuit breaker opened after consecutive failures"
        );
    }

    if let Err(error) = ctx
        .db
        .webhook_endpoint()
        .update(endpoint.id.clone())
        .set(set)
        .run(sys)
        .await
    {
        warn!(endpoint_id = %endpoint.id, %error, "recording endpoint failure count failed");
    }
}

/// A successful delivery resets `consecutiveFailures` to zero and clears
/// `circuitOpenUntil` defensively — see §8.5's own "Implementation" note in
/// the design doc for why these are the two things that reset the counter.
/// Skips the write entirely when there is nothing to reset, so a healthy
/// endpoint's every single success doesn't cost a pointless `UPDATE`.
async fn reset_endpoint_failures(
    ctx: &WorkerContext,
    sys: &CoolContext,
    endpoint: &WebhookEndpoint,
) {
    if endpoint.consecutiveFailures == 0 && endpoint.circuitOpenUntil.is_none() {
        return;
    }
    if let Err(error) = ctx
        .db
        .webhook_endpoint()
        .update(endpoint.id.clone())
        .set(UpdateWebhookEndpointInput {
            consecutiveFailures: Some(0),
            circuitOpenUntil: Some(None),
            ..Default::default()
        })
        .run(sys)
        .await
    {
        warn!(endpoint_id = %endpoint.id, %error, "resetting endpoint failure count failed");
    }
}

/// §8.5: "410 Gone deactivates the endpoint immediately."
async fn deactivate_endpoint(ctx: &WorkerContext, sys: &CoolContext, endpoint: &WebhookEndpoint) {
    if let Err(error) = ctx
        .db
        .webhook_endpoint()
        .update(endpoint.id.clone())
        .set(UpdateWebhookEndpointInput {
            active: Some(false),
            ..Default::default()
        })
        .run(sys)
        .await
    {
        warn!(endpoint_id = %endpoint.id, %error, "deactivating endpoint after 410 Gone failed");
    }
}

/// The only realistic cause of this write failing is a concurrent lease
/// reclaim (another worker decided this lease expired and reclaimed it) or
/// an operator-triggered replay racing this outcome — a legitimate race,
/// not a bug, so this logs and lets the tick continue rather than treating
/// it as fatal. Same shape as `dispatch::log_write_failure`.
fn log_write_failure(attempt_id: &str, attempted_state: AttemptState, error: &CoolError) {
    warn!(
        attempt_id,
        attempted_state = ?attempted_state, %error,
        "writing hooks' own transition failed — likely a concurrent reclaim or replay"
    );
}

#[cfg(test)]
mod tests {
    use super::{backoff_for, build_envelope, BACKOFF_SCHEDULE};
    use chrono::{Duration, TimeZone, Utc};

    #[test]
    fn the_first_attempt_backs_off_by_the_schedules_first_entry() {
        assert_eq!(backoff_for(1), Duration::seconds(1));
    }

    #[test]
    fn each_attempt_walks_one_step_further_into_the_schedule() {
        let len = i64::try_from(BACKOFF_SCHEDULE.len()).unwrap();
        for (attempts, expected) in (1..=len).zip(BACKOFF_SCHEDULE) {
            assert_eq!(backoff_for(attempts), expected, "attempts={attempts}");
        }
    }

    #[test]
    fn attempts_past_the_schedules_length_stay_capped_at_the_last_entry() {
        let last = *BACKOFF_SCHEDULE.last().unwrap();
        let len = i64::try_from(BACKOFF_SCHEDULE.len()).unwrap();
        assert_eq!(backoff_for(len + 1), last);
        assert_eq!(backoff_for(1000), last);
    }

    fn fixture_attempt() -> sms_api::schema::WebhookAttempt {
        sms_api::schema::WebhookAttempt {
            id: "cattempt00000000000000".to_owned(),
            endpointId: "cendpoint0000000000000".to_owned(),
            sourceEventId: cratestack::uuid::Uuid::nil(),
            aggregateId: "cmsg000000000000000000".to_owned(),
            eventType: "message.delivered".to_owned(),
            payload: r#"{"messageId":"cmsg000000000000000000","to":"+2376****89"}"#.to_owned(),
            state: sms_api::schema::AttemptState::delivering,
            attempts: 1,
            leaseOwner: None,
            leaseUntil: None,
            nextAttemptAt: None,
            lastStatusCode: None,
            lastError: None,
            lastAttemptAt: None,
            deliveredAt: None,
            version: 0,
        }
    }

    /// The envelope's `data` is the *parsed* payload embedded as a nested
    /// JSON object, never a second string encoding of it — a receiver
    /// parsing `data` as an object would see a JSON string in its place if
    /// this regressed, and that failure mode is exactly what this test
    /// exists to catch before a live test would.
    #[test]
    fn the_envelope_wraps_the_payload_as_a_nested_object_not_a_re_encoded_string() {
        let attempt = fixture_attempt();
        let now = Utc.with_ymd_and_hms(2026, 7, 28, 14, 3, 11).unwrap();
        let body = build_envelope(&attempt, now).expect("valid JSON payload parses");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(parsed["id"], "cattempt00000000000000");
        assert_eq!(parsed["type"], "message.delivered");
        assert_eq!(parsed["occurredAt"], "2026-07-28T14:03:11Z");
        assert!(parsed["data"].is_object(), "data must be a nested object");
        assert_eq!(parsed["data"]["messageId"], "cmsg000000000000000000");
    }

    /// The masking correctness bar this module can actually own: whatever
    /// string sits in `payload`'s `to` field survives into the envelope
    /// byte-for-byte, never reconstructed from anything else. A masked
    /// value in, a masked value out.
    #[test]
    fn a_masked_to_field_in_the_payload_survives_into_the_envelope_unchanged() {
        let attempt = fixture_attempt();
        let now = Utc::now();
        let body = build_envelope(&attempt, now).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["data"]["to"], "+2376****89");
    }

    #[test]
    fn a_malformed_payload_is_a_typed_error_not_a_panic() {
        let mut attempt = fixture_attempt();
        attempt.payload = "not json".to_owned();
        assert!(build_envelope(&attempt, Utc::now()).is_err());
    }
}
