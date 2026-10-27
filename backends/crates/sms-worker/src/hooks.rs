#![doc = include_str!("hooks.md")]

use std::sync::LazyLock;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Utc};
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use reqwest::StatusCode;
use sms_api::auth::system_context;
use sms_api::map_database_error;
use sms_api::schema::{
    AttemptState, UpdateWebhookAttemptInput, UpdateWebhookEndpointInput, WebhookAttempt,
    WebhookDeliveryMode, WebhookEndpoint, webhook_endpoint,
};
use sms_webhook::Sealer;
use tracing::{error, warn};

use crate::WorkerContext;
use crate::breaker::{self, BreakerPolicy};
use crate::claim::{ServedModes, claim_webhook_attempts, count_sealed_attempts_waiting};
use crate::throttle::Throttle;

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

/// §8.5, verbatim: "20 consecutive failures sets `circuitOpenUntil`" for
/// 15 minutes. See `breaker.md` for why this and `dispatch::PROVIDER_
/// BREAKER` each declare their own `BreakerPolicy` rather than sharing a
/// value defined in that module.
const ENDPOINT_BREAKER: BreakerPolicy = BreakerPolicy {
    failure_threshold: 20,
    open_duration: Duration::minutes(15),
};

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

/// The `system` context this role does all its work under — see
/// [`sms_api::auth::system_context`] for the shared constructor and the
/// invariant it documents.
fn sys(worker: &str) -> CratestackContext {
    system_context(format!("sms-worker:hooks:{worker}"))
}

/// The HTTP clients `hooks` delivers with: one per delivery mode, because the
/// two modes keep different promises about redirects.
///
/// - **`sealed` follows none (VS-8).** A delivery is sealed for one audience and
///   one route, and a `307`/`308` would re-send those bytes, recipient and
///   message body included, to a host the seal was never made for. A redirect is
///   a non-2xx answer like any other: it retries, and an operator fixes the URL.
/// - **`hmac` keeps reqwest's default policy**, as it had before sealed mode
///   existed: up to ten redirects, a `307`/`308` re-sending the body. Other apps'
///   receivers sit behind redirects today (a bare host to `www`, `http` to
///   `https`), and the owner's ruling (R5) is that the HMAC scheme survives
///   unchanged for them. The caveat is the usual one: a redirect is followed to
///   wherever the receiver's own server sends it, the signed body and
///   `X-Sms-Signature` with it, so whoever controls an `hmac` receiver's
///   redirects controls where its deliveries go. That is a trust the endpoint's
///   registrant already extends by choosing the URL.
///
/// Both carry §8.5's 10 s timeout: `reqwest::Client::new()` sets none, so
/// building one explicitly is what enforces "10s per attempt" rather than
/// merely documenting it. A redirect chain shares that one timeout.
pub struct HttpClients {
    sealed: reqwest::Client,
    hmac: reqwest::Client,
}

impl HttpClients {
    /// The client an endpoint in `mode` delivers with.
    fn for_mode(&self, mode: WebhookDeliveryMode) -> &reqwest::Client {
        match mode {
            WebhookDeliveryMode::sealed => &self.sealed,
            WebhookDeliveryMode::hmac => &self.hmac,
        }
    }
}

/// Builds the clients `hooks` delivers with ([`HttpClients`]).
///
/// # Panics
///
/// Never in practice: the only way `ClientBuilder::build` fails is a TLS
/// backend misconfiguration, and these builders set only a timeout and a
/// redirect policy — same
/// reasoning `OrangeCmProvider::new`'s identical `.expect(...)` already
/// relies on.
#[must_use]
pub fn build_http_client() -> HttpClients {
    let builder = || reqwest::Client::builder().timeout(REQUEST_TIMEOUT);
    HttpClients {
        sealed: builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client builder with a timeout and a redirect policy never fails"),
        hmac: builder()
            .build()
            .expect("reqwest client builder with a timeout never fails"),
    }
}

/// How often a worker without the envelope signing key repeats what it cannot
/// serve. A backlog that grows by the second is still one line per interval.
pub const SEALED_BACKLOG_WARNING_EVERY: StdDuration = StdDuration::from_mins(5);

/// A worker without the envelope signing key claims no `sealed` attempt
/// ([`ServedModes::HmacOnly`]), so those attempts stay `pending` and nothing in
/// the claim loop says so. This is what does: every
/// [`SEALED_BACKLOG_WARNING_EVERY`] it counts the sealed attempts that are due and
/// `warn`s with the number and the flag to mount. Without it, a deployment that
/// forgot the key would see sealed deliveries wait forever and no log line.
pub struct SealedBacklogWatch {
    every: Throttle,
}

impl SealedBacklogWatch {
    /// A watch that has not reported yet, so its first call asks.
    #[must_use]
    pub fn new() -> Self {
        Self {
            every: Throttle::new(SEALED_BACKLOG_WARNING_EVERY),
        }
    }

    /// Counts the sealed attempts `ctx` cannot serve and warns if there are any.
    /// `None` when there was nothing to ask: the worker holds the key (it serves
    /// them), or the last question was less than [`SEALED_BACKLOG_WARNING_EVERY`]
    /// ago. `Some(count)` when it asked, `0` meaning nothing is waiting and nothing
    /// was said.
    ///
    /// # Errors
    ///
    /// Whatever the count returns; the interval is spent either way, so a
    /// database that is down is asked again in five minutes, not every poll.
    pub async fn report(
        &self,
        ctx: &WorkerContext,
        sys: &CratestackContext,
        now: Instant,
    ) -> Result<Option<usize>, CratestackError> {
        if ctx.webhook_sealer.is_some() || !self.every.admit("sealed-backlog", now) {
            return Ok(None);
        }
        let waiting = count_sealed_attempts_waiting(&ctx.db, sys, Utc::now()).await?;
        let waiting = usize::try_from(waiting).unwrap_or(0);
        if waiting > 0 {
            warn!(
                waiting,
                "sealed webhook deliveries are waiting that this worker cannot send: it holds no \
                 envelope signing key. Start it with --envelope-signing-key-file \
                 (SMS_ENVELOPE_SIGNING_KEY_FILE, the gateway's key), or run a hooks worker that \
                 has it. hmac deliveries are unaffected, and nothing is counted against the sealed \
                 attempts meanwhile"
            );
        }
        Ok(Some(waiting))
    }
}

impl Default for SealedBacklogWatch {
    fn default() -> Self {
        Self::new()
    }
}

/// Never returns on its own, matching [`crate::run`]'s contract for every
/// other role.
pub async fn run(ctx: WorkerContext, worker: &str) {
    let sys = sys(worker);
    let http = build_http_client();
    let backlog = SealedBacklogWatch::new();
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    loop {
        interval.tick().await;
        if let Err(error) = tick(&ctx, &sys, worker, &http).await {
            error!(%error, "hooks tick failed; retrying next poll");
        }
        if let Err(error) = backlog.report(&ctx, &sys, Instant::now()).await {
            error!(%error, "counting the sealed attempts this worker cannot serve failed");
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
///
/// Each claimed row is delivered by its endpoint's own `deliveryMode`
/// ([`send`]): `hmac` needs nothing but the endpoint's secret, `sealed` needs
/// `ctx.webhook_sealer`. A process without the signing key therefore claims
/// `hmac` attempts only ([`ServedModes::HmacOnly`]); it never takes a `sealed`
/// one, so nothing about one is written until a worker that holds the key
/// does, and [`SealedBacklogWatch`] says that they are waiting.
/// `backends/apps/sms-worker` says so at start when `hooks` runs without a key.
///
/// # Errors
///
/// Whatever the claim returns.
pub async fn tick(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    worker: &str,
    http: &HttpClients,
) -> Result<(), CratestackError> {
    let served = if ctx.webhook_sealer.is_some() {
        ServedModes::Both
    } else {
        ServedModes::HmacOnly
    };
    let claimed = claim_webhook_attempts(&ctx.db, sys, worker, BUDGET, served).await?;
    for attempt in claimed {
        deliver_one(ctx, sys, http, ctx.webhook_sealer.as_ref(), attempt).await;
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
    /// A non-2xx, non-410 status, a transport error, or a request that could
    /// not be built (a signer that fails, a row with no secret): the delivery
    /// failed, so this is what feeds `maxAttempts`/backoff and the circuit
    /// breaker.
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
    sys: &CratestackContext,
    http: &HttpClients,
    sealer: Option<&Sealer>,
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
    let body = match build_envelope(&attempt, &endpoint, now) {
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

    let outcome = send(http, sealer, &endpoint, &attempt, &body, now).await;
    write_outcome(ctx, sys, &attempt, &endpoint, outcome, now).await;
}

/// The actual HTTP POST, in the endpoint's own delivery mode. Split from
/// [`deliver_one`] so the request-building logic is directly unit-testable
/// without a database — [`build_envelope`] is a pure function of its inputs.
///
/// A `sealed` endpoint's request is [`sealed_request`], an `hmac` endpoint's
/// [`hmac_request`]; the status handling after the send is one function for
/// both, so a `410` deactivates and a `5xx` retries whichever way the bytes were
/// authenticated. A `sealed` request follows no redirect and an `hmac` one
/// follows reqwest's default ([`HttpClients`]).
async fn send(
    http: &HttpClients,
    sealer: Option<&Sealer>,
    endpoint: &WebhookEndpoint,
    attempt: &WebhookAttempt,
    body: &str,
    now: DateTime<Utc>,
) -> Outcome {
    let client = http.for_mode(endpoint.deliveryMode);
    let request = match endpoint.deliveryMode {
        WebhookDeliveryMode::sealed => sealed_request(client, sealer, endpoint, body).await,
        WebhookDeliveryMode::hmac => {
            warn_empty_hmac_keys(endpoint, &EMPTY_KEY_WARNINGS, Instant::now());
            hmac_request(client, endpoint, attempt, body, now)
        }
    };
    let request = match request {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };

    match request.send().await {
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

/// A `sealed` delivery: the envelope JSON inside a `COSE_Sign1` under vsms's
/// envelope signing key, addressed to the endpoint's `audience`.
///
/// **Every call seals afresh.** A retry is a new request with a new nonce and a
/// new timestamp, never the first attempt's bytes sent again: a delivery whose
/// backoff outlasts the receiver's 300 s skew (the schedule runs to 24 h) would
/// otherwise be refused as stale, and a re-sent nonce is refused as a replay.
/// The `id` and `eventId` inside the body are what stay the same.
///
/// A delivery that cannot be sealed (no signing key in this process, no
/// audience on the row, an audience or URL the seal refuses, a signer that
/// fails) never reaches the network, and is an ordinary [`Outcome::Retryable`]:
/// counted, backed off, able to trip the breaker and able to go `dead`, like any
/// other failure. None of them is expected. A worker without the key never
/// claims a sealed attempt ([`crate::claim::ServedModes`]), and the database
/// refuses a sealed row whose audience or URL cannot be sealed, so what is left
/// is a signer that fails, which is as much the delivery's failure as a
/// connection that does.
async fn sealed_request(
    http: &reqwest::Client,
    sealer: Option<&Sealer>,
    endpoint: &WebhookEndpoint,
    body: &str,
) -> Result<reqwest::RequestBuilder, Outcome> {
    let not_sealed = |message: String| Outcome::Retryable {
        status: None,
        message,
    };
    let Some(sealer) = sealer else {
        return Err(not_sealed(
            "delivery not sealed: this worker holds no envelope signing key. Start it with \
             --envelope-signing-key-file (the gateway's key), or switch the endpoint to hmac"
                .to_owned(),
        ));
    };
    // The CHECK `webhook_endpoints_audience_required_for_sealed_check` makes a
    // sealed row without one impossible; an empty one is `NoAudience` below.
    let audience = endpoint.audience.as_deref().unwrap_or_default();
    let delivery = sealer
        .seal(audience, &endpoint.url, body.as_bytes())
        .await
        .map_err(|error| not_sealed(format!("delivery not sealed: {error}")))?;
    let mut request = http.post(&endpoint.url).body(delivery.body);
    for (name, value) in &delivery.headers {
        request = request.header(name, value);
    }
    Ok(request)
}

/// An `hmac` delivery, exactly as it was before sealed mode existed (#41): the
/// four-field envelope as JSON, the four `X-Sms-*` headers, and
/// `X-Sms-Signature` over `v1\n{timestamp}\n{eventId}\n{sha256(body)}` keyed by
/// the endpoint's `secret` and, while a rotation's overlap window is open, by its
/// `prevSecret` too (current first, "oldest last" per §4.4).
///
/// **Byte-identical to `29f93c4` for every row that has a `secret`**, including
/// an empty one (owner's ruling R5: vsms serves apps beyond vaam, and their
/// receivers were built against this). An empty `secret` is signed with the empty
/// key and an empty `prevSecret` is a second `v1=` value, as they always were;
/// [`warn_empty_hmac_keys`] says so loudly instead of this function changing the
/// bytes. The API now refuses a secret under 32 characters, but a row from before
/// that bound can hold one.
///
/// The only row this cannot sign is one with no `secret` at all, which the CHECK
/// `webhook_endpoints_delivery_mode_hmac_needs_secret_check` makes impossible. It
/// is an ordinary [`Outcome::Retryable`]: counted, backed off, and fed to the
/// breaker like any other failure, never a request signed with nothing.
fn hmac_request(
    http: &reqwest::Client,
    endpoint: &WebhookEndpoint,
    attempt: &WebhookAttempt,
    body: &str,
    now: DateTime<Utc>,
) -> Result<reqwest::RequestBuilder, Outcome> {
    let Some(secret) = endpoint.secret.as_deref() else {
        return Err(Outcome::Retryable {
            status: None,
            message: "delivery not signed: this hmac endpoint has no secret. Rotate it \
                      (rotateWebhookSecret) to mint one"
                .to_owned(),
        });
    };
    let timestamp = now.timestamp();
    let event_id = attempt.sourceEventId.to_string();
    let secrets: Vec<&str> = std::iter::once(secret)
        .chain(endpoint.prevSecret.as_deref())
        .collect();
    let signature = sms_webhook::sign_header(&secrets, timestamp, &event_id, body.as_bytes());

    Ok(http
        .post(&endpoint.url)
        .header(sms_webhook::HEADER_EVENT, &attempt.eventType)
        .header(sms_webhook::HEADER_EVENT_ID, &event_id)
        .header(sms_webhook::HEADER_TIMESTAMP, timestamp.to_string())
        .header(sms_webhook::HEADER_SIGNATURE, signature)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_owned()))
}

/// How often one `hmac` endpoint's empty-key warning repeats. A busy endpoint
/// would otherwise log it once per event.
const EMPTY_KEY_WARNING_EVERY: StdDuration = StdDuration::from_mins(10);

/// Which endpoints have said so recently ([`warn_empty_hmac_keys`]).
static EMPTY_KEY_WARNINGS: LazyLock<Throttle> =
    LazyLock::new(|| Throttle::new(EMPTY_KEY_WARNING_EVERY));

/// Says, at `warn`, that an `hmac` endpoint is signing with an empty key, at most
/// once per [`EMPTY_KEY_WARNING_EVERY`] per endpoint.
///
/// [`hmac_request`] signs such a row exactly as `29f93c4` did, because other apps'
/// receivers were built against those bytes (R5), so the delivery is not where
/// the problem is stopped. A signature under the empty key is one anyone can
/// compute, though, and the operator who can fix it should be told which
/// endpoint it is. The line names the endpoint and which of its two keys is
/// empty, and nothing of any key's value.
fn warn_empty_hmac_keys(endpoint: &WebhookEndpoint, throttle: &Throttle, now: Instant) {
    let secret_empty = endpoint.secret.as_deref().is_some_and(str::is_empty);
    let prev_secret_empty = endpoint.prevSecret.as_deref().is_some_and(str::is_empty);
    if !(secret_empty || prev_secret_empty) || !throttle.admit(&endpoint.id, now) {
        return;
    }
    warn!(
        endpoint_id = %endpoint.id, app_id = %endpoint.appId, secret_empty, prev_secret_empty,
        "hmac webhook endpoint signs with an empty key, so anyone can compute its \
         X-Sms-Signature; deliveries are unchanged for the receivers that rely on them. Rotate \
         the endpoint's secret (rotateWebhookSecret; twice when the secret itself is empty, \
         since the first rotation moves the empty one into prevSecret)"
    );
}

/// The §8.4 envelope, built from the stored rows at delivery time, in the shape
/// the endpoint's mode delivers: a `sealed` endpoint's is `{id, eventId, type,
/// occurredAt, appId, endpointId, data}` (contract v2), an `hmac` endpoint's is
/// the original `{id, type, occurredAt, data}`, byte for byte what its receiver
/// verifies today (`eventId` rides in `X-Sms-Event-Id`, which the HMAC signs, and
/// there is no audience for `appId` and `endpointId` to disambiguate). This
/// module's own doc covers `id`/`type`; a sealed `appId` and `endpointId` are the
/// endpoint's own, in the signed body so a receiver can tell a delivery made
/// *for it* from one merely addressed to its audience (`sms-webhook`'s
/// `lib.md`, "What a receiver configures"). Documented here is `occurredAt`,
/// the one field that is a genuine, tracked approximation rather than a
/// faithful reconstruction.
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
    endpoint: &WebhookEndpoint,
    now: DateTime<Utc>,
) -> Result<String, serde_json::Error> {
    let data: serde_json::Value = serde_json::from_str(&attempt.payload)?;
    let occurred_at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let envelope = match endpoint.deliveryMode {
        WebhookDeliveryMode::sealed => serde_json::json!({
            "id": attempt.id,
            "eventId": attempt.sourceEventId,
            "type": attempt.eventType,
            "occurredAt": occurred_at,
            "appId": endpoint.appId,
            "endpointId": endpoint.id,
            "data": data,
        }),
        WebhookDeliveryMode::hmac => serde_json::json!({
            "id": attempt.id,
            "type": attempt.eventType,
            "occurredAt": occurred_at,
            "data": data,
        }),
    };
    Ok(envelope.to_string())
}

async fn fetch_endpoint(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    endpoint_id: &str,
) -> Result<Option<WebhookEndpoint>, CratestackError> {
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
/// failure bookkeeping are separate rows with no shared transaction here.
/// **Update, #59:** `WebhookEndpoint` gained `@version`, so a combined CAS
/// across both rows in one transaction is now possible in principle, but
/// this function deliberately still doesn't do that — the two writes keep
/// independent failure modes worth logging separately (an attempt-state
/// write failure is a bug to alert on; an endpoint-bookkeeping write losing
/// a race is an accepted, silent, best-effort miss), and combining them
/// would make the attempt's own outcome hostage to a race on a value nothing
/// else in this function needs to be atomic with. But which arms touch the
/// endpoint at all is the load-bearing decision in this function:
/// `Success`/`Gone`/`Retryable` all
/// reflect something the endpoint actually did (or failed to do), or a delivery
/// that could not be made, so all three update its bookkeeping;
/// `MalformedPayload` reflects nothing about the endpoint — no request was ever
/// sent to it and the row is our own bug — and must not touch it. Errors
/// from either write are logged, not propagated, matching every other
/// outcome-writer in this crate.
async fn write_outcome(
    ctx: &WorkerContext,
    sys: &CratestackContext,
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
    sys: &CratestackContext,
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
    sys: &CratestackContext,
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
    sys: &CratestackContext,
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
/// moment it crosses [`ENDPOINT_BREAKER`]'s own threshold (the decision
/// itself lives in [`breaker::on_failure`], shared with `dispatch.rs`'s
/// identically-shaped provider breaker — see `breaker.md`). By construction
/// this is never called for an endpoint whose circuit is already open —
/// `claim.rs`'s own `filter_by_endpoint_health` excludes such an endpoint's
/// attempts from ever being claimed, so `hooks` never attempts, and
/// therefore never records a failure against, a breaker that has already
/// tripped.
///
/// **Update, #59:** `WebhookEndpoint` gained `@version`, so this write now
/// carries `if_match(endpoint.version)`. Still deliberately best-effort,
/// not a CAS-retry loop: a losing race falls into the same `Err(error) =>
/// warn!(...)` arm below as any other write failure always has, so a lost
/// race is now a detected-and-dropped `PreconditionFailed` instead of a
/// silent overwrite — a strictly more honest failure, not a behaviour
/// change. Under concurrent `hooks` workers failing against the same
/// endpoint at once, `consecutiveFailures` can still under-count by a small
/// amount (the loser's write is dropped, not retried) — acceptable for a
/// heuristic that exists to stop hammering a dead endpoint, not to bill
/// anyone.
async fn record_endpoint_failure(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    endpoint: &WebhookEndpoint,
    now: DateTime<Utc>,
) {
    let decision = breaker::on_failure(&ENDPOINT_BREAKER, endpoint.consecutiveFailures, now);

    let mut set = UpdateWebhookEndpointInput {
        consecutiveFailures: Some(decision.consecutive_failures),
        ..Default::default()
    };
    if decision.opened_circuit() {
        set.circuitOpenUntil = Some(decision.circuit_open_until);
        warn!(
            endpoint_id = %endpoint.id,
            threshold = ENDPOINT_BREAKER.failure_threshold,
            "circuit breaker opened after consecutive failures"
        );
    }

    if let Err(error) = ctx
        .db
        .webhook_endpoint()
        .update(endpoint.id.clone())
        .set(set)
        .if_match(endpoint.version)
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
///
/// #59: carries `if_match(endpoint.version)`, same best-effort-not-retried
/// reasoning as `record_endpoint_failure` above.
async fn reset_endpoint_failures(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    endpoint: &WebhookEndpoint,
) {
    if !breaker::needs_reset(endpoint.consecutiveFailures, endpoint.circuitOpenUntil) {
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
        .if_match(endpoint.version)
        .run(sys)
        .await
    {
        warn!(endpoint_id = %endpoint.id, %error, "resetting endpoint failure count failed");
    }
}

/// §8.5: "410 Gone deactivates the endpoint immediately."
///
/// #59: carries `if_match(endpoint.version)`, same best-effort-not-retried
/// reasoning as `record_endpoint_failure` above.
async fn deactivate_endpoint(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    endpoint: &WebhookEndpoint,
) {
    if let Err(error) = ctx
        .db
        .webhook_endpoint()
        .update(endpoint.id.clone())
        .set(UpdateWebhookEndpointInput {
            active: Some(false),
            ..Default::default()
        })
        .if_match(endpoint.version)
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
fn log_write_failure(attempt_id: &str, attempted_state: AttemptState, error: &CratestackError) {
    warn!(
        attempt_id,
        attempted_state = ?attempted_state, %error,
        "writing hooks' own transition failed — likely a concurrent reclaim or replay"
    );
}

#[cfg(test)]
mod tests {
    use super::{
        BACKOFF_SCHEDULE, Sealer, backoff_for, build_envelope, build_http_client, hmac_request,
    };
    use chrono::{Duration, TimeZone, Utc};
    use sms_api::schema::{WebhookDeliveryMode, WebhookEndpoint};

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

    fn fixture_endpoint() -> WebhookEndpoint {
        WebhookEndpoint {
            createdAt: Utc::now(),
            updatedAt: Utc::now(),
            id: "cendpoint0000000000000".to_owned(),
            appId: "capp00000000000000000".to_owned(),
            url: "https://api.vaam.store/webhooks/vsms".to_owned(),
            deliveryMode: WebhookDeliveryMode::sealed,
            audience: Some("vaam".to_owned()),
            eventTypes: " message.delivered ".to_owned(),
            secret: None,
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: true,
            active: true,
            maxAttempts: 8,
            circuitOpenUntil: None,
            consecutiveFailures: 0,
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
        let body =
            build_envelope(&attempt, &fixture_endpoint(), now).expect("valid JSON payload parses");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(parsed["id"], "cattempt00000000000000");
        assert_eq!(parsed["type"], "message.delivered");
        assert_eq!(parsed["occurredAt"], "2026-07-28T14:03:11Z");
        // The two values a receiver checks to know the delivery is for it.
        assert_eq!(parsed["appId"], "capp00000000000000000");
        assert_eq!(parsed["endpointId"], "cendpoint0000000000000");
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
        let body = build_envelope(&attempt, &fixture_endpoint(), now).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["data"]["to"], "+2376****89");
    }

    #[test]
    fn a_malformed_payload_is_a_typed_error_not_a_panic() {
        let mut attempt = fixture_attempt();
        attempt.payload = "not json".to_owned();
        assert!(build_envelope(&attempt, &fixture_endpoint(), Utc::now()).is_err());
    }

    fn hmac_endpoint() -> WebhookEndpoint {
        WebhookEndpoint {
            deliveryMode: WebhookDeliveryMode::hmac,
            audience: None,
            secret: Some("whsec_current".to_owned()),
            ..fixture_endpoint()
        }
    }

    /// An `hmac` endpoint's body is the four-field envelope its receivers verify
    /// today, byte for byte: nothing VS-8 added (`eventId`, `appId`,
    /// `endpointId`) is in it, and the key order is the one `serde_json`'s map
    /// has always written. A receiver hashing the raw body would notice a
    /// difference, which is the whole reason this is an exact-string assertion.
    #[test]
    fn an_hmac_envelope_is_the_original_four_field_body_byte_for_byte() {
        let now = Utc.with_ymd_and_hms(2026, 7, 28, 14, 3, 11).unwrap();
        let body = build_envelope(&fixture_attempt(), &hmac_endpoint(), now).unwrap();
        assert_eq!(
            body,
            concat!(
                r#"{"data":{"messageId":"cmsg000000000000000000","to":"+2376****89"},"#,
                r#""id":"cattempt00000000000000","occurredAt":"2026-07-28T14:03:11Z","#,
                r#""type":"message.delivered"}"#
            )
        );
    }

    #[test]
    fn a_sealed_envelope_carries_the_seven_fields_of_contract_v2() {
        let body = build_envelope(&fixture_attempt(), &fixture_endpoint(), Utc::now()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let mut keys: Vec<&str> = parsed
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "appId",
                "data",
                "endpointId",
                "eventId",
                "id",
                "occurredAt",
                "type"
            ]
        );
    }

    fn header(request: &reqwest::Request, name: &str) -> String {
        request.headers()[name].to_str().unwrap().to_owned()
    }

    /// The request an `hmac` endpoint gets: the four `X-Sms-*` headers, the
    /// signature over exactly the body sent, current secret first and the
    /// previous one second while an overlap window is open.
    #[test]
    fn an_hmac_request_is_signed_with_the_current_secret_and_the_previous_one() {
        let http = build_http_client().hmac;
        let now = Utc.with_ymd_and_hms(2026, 7, 28, 14, 3, 11).unwrap();
        let attempt = fixture_attempt();
        let mut endpoint = hmac_endpoint();
        let body = build_envelope(&attempt, &endpoint, now).unwrap();

        let single = hmac_request(&http, &endpoint, &attempt, &body, now)
            .ok()
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(header(&single, "x-sms-event"), "message.delivered");
        assert_eq!(
            header(&single, "x-sms-event-id"),
            attempt.sourceEventId.to_string()
        );
        assert_eq!(
            header(&single, "x-sms-timestamp"),
            now.timestamp().to_string()
        );
        assert_eq!(header(&single, "content-type"), "application/json");
        let signature = header(&single, "x-sms-signature");
        assert_eq!(signature.matches("v1=").count(), 1, "{signature}");
        assert!(sms_webhook::verify(
            &["whsec_current"],
            now.timestamp(),
            &attempt.sourceEventId.to_string(),
            body.as_bytes(),
            &signature,
        ));

        endpoint.prevSecret = Some("whsec_previous".to_owned());
        let overlap = hmac_request(&http, &endpoint, &attempt, &body, now)
            .ok()
            .unwrap()
            .build()
            .unwrap();
        let signature = header(&overlap, "x-sms-signature");
        assert_eq!(signature.matches("v1=").count(), 2, "{signature}");
        // Each secret alone verifies it: a receiver on the old one is not cut off.
        for secret in ["whsec_current", "whsec_previous"] {
            assert!(sms_webhook::verify(
                &[secret],
                now.timestamp(),
                &attempt.sourceEventId.to_string(),
                body.as_bytes(),
                &signature,
            ));
        }
        // And the current secret comes first on the wire.
        assert!(signature.starts_with(&format!(
            "v1={}",
            sms_webhook::sign_v1(
                "whsec_current",
                now.timestamp(),
                &attempt.sourceEventId.to_string(),
                body.as_bytes()
            )
        )));
    }

    /// The base algorithm, byte for byte, for a row whose `secret` and `prevSecret`
    /// are both the empty string (owner's ruling R5: vsms serves apps beyond vaam,
    /// so the HMAC delivery stays identical to what `29f93c4` sent them). The empty
    /// key signs, and the empty `prevSecret` is a second `v1=` value, exactly as
    /// `sign_header(&[secret, prevSecret])` always produced.
    ///
    /// The expected values were computed with neither this crate nor the worker,
    /// by `openssl`, over the base's canonical string (`ts` 1785247391 is
    /// `2026-07-28T14:03:11Z`, the event id is the nil UUID):
    ///
    /// ```text
    /// printf '%s' '<the body below>' | openssl dgst -sha256 -r
    /// # 283e620100ce200bb4a99b899e1d37528acacb6f4beb82005d757aa939af3ead
    /// printf 'v1\n1785247391\n00000000-0000-0000-0000-000000000000\n283e62...3ead' \
    ///   | openssl dgst -sha256 -hmac "" -r
    /// # ca8be90f66e528653982259453d9a59d5716c29fa6d6d4b8ddaf46cde1391ab1
    /// ```
    #[test]
    fn an_empty_secret_and_an_empty_previous_secret_are_signed_as_the_base_did() {
        const EMPTY_KEY_SIGNATURE: &str =
            "ca8be90f66e528653982259453d9a59d5716c29fa6d6d4b8ddaf46cde1391ab1";
        let http = build_http_client().hmac;
        let now = Utc.with_ymd_and_hms(2026, 7, 28, 14, 3, 11).unwrap();
        let attempt = fixture_attempt();
        let endpoint = WebhookEndpoint {
            secret: Some(String::new()),
            prevSecret: Some(String::new()),
            ..hmac_endpoint()
        };
        let body = build_envelope(&attempt, &endpoint, now).unwrap();
        assert_eq!(
            body,
            concat!(
                r#"{"data":{"messageId":"cmsg000000000000000000","to":"+2376****89"},"#,
                r#""id":"cattempt00000000000000","occurredAt":"2026-07-28T14:03:11Z","#,
                r#""type":"message.delivered"}"#
            )
        );

        let request = hmac_request(&http, &endpoint, &attempt, &body, now)
            .ok()
            .expect("an empty secret is signed with, as at 29f93c4")
            .build()
            .unwrap();
        assert_eq!(
            header(&request, "x-sms-signature"),
            format!("v1={EMPTY_KEY_SIGNATURE},v1={EMPTY_KEY_SIGNATURE}")
        );
        assert_eq!(header(&request, "x-sms-timestamp"), "1785247391");
        assert_eq!(
            header(&request, "x-sms-event-id"),
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(header(&request, "x-sms-event"), "message.delivered");
        assert_eq!(header(&request, "content-type"), "application/json");
        assert_eq!(
            request.body().and_then(reqwest::Body::as_bytes),
            Some(body.as_bytes())
        );

        // An empty secret alone, with no previous one, is one value.
        let alone = WebhookEndpoint {
            prevSecret: None,
            ..endpoint
        };
        let request = hmac_request(&http, &alone, &attempt, &body, now)
            .ok()
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            header(&request, "x-sms-signature"),
            format!("v1={EMPTY_KEY_SIGNATURE}")
        );
    }

    /// What `tracing` is handed while the returned guard lives, as the text a
    /// subscriber prints (the guard is thread-local, so an async test must run on
    /// the current-thread runtime `#[tokio::test]` gives it).
    #[derive(Clone, Default)]
    struct Logs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Logs {
        fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
            let logs = Self::default();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(logs.clone())
                .with_ansi(false)
                .finish();
            (logs, tracing::subscriber::set_default(subscriber))
        }

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

    fn logged_during(run: impl FnOnce()) -> String {
        let (logs, _guard) = Logs::capture();
        run();
        logs.text()
    }

    /// An `hmac` endpoint that signs with an empty key is named at `warn`, once per
    /// interval however many events arrive, again after it, and never with a key's
    /// value in the line. An endpoint with real keys is not mentioned.
    #[test]
    fn an_empty_key_is_warned_about_loudly_once_per_interval_and_never_with_a_value() {
        use super::{EMPTY_KEY_WARNING_EVERY, Throttle, warn_empty_hmac_keys};
        let start = std::time::Instant::now();
        let throttle = Throttle::new(EMPTY_KEY_WARNING_EVERY);
        let marker = "whsec_marker_that_must_not_be_logged_0123456789";
        let weak = WebhookEndpoint {
            id: "cendpointweak0000000000".to_owned(),
            secret: Some(marker.to_owned()),
            prevSecret: Some(String::new()),
            ..hmac_endpoint()
        };
        let healthy = WebhookEndpoint {
            id: "cendpointhealthy00000000".to_owned(),
            prevSecret: Some("whsec_previous".to_owned()),
            ..hmac_endpoint()
        };

        let first = logged_during(|| {
            warn_empty_hmac_keys(&healthy, &throttle, start);
            warn_empty_hmac_keys(&weak, &throttle, start);
        });
        assert_eq!(
            first.matches("WARN").count(),
            1,
            "exactly one line, for the weak endpoint"
        );
        assert!(
            first.contains("cendpointweak0000000000"),
            "the line names the endpoint"
        );
        assert!(
            first.contains("prev_secret_empty=true"),
            "the line names which key is empty"
        );
        assert!(first.contains("secret_empty=false"), "and which is not");
        assert!(
            first.contains("rotateWebhookSecret"),
            "and tells the operator what to do"
        );
        assert!(
            !first.contains("cendpointhealthy"),
            "an endpoint with real keys is not mentioned"
        );
        // Never the line itself in the message: were a key in it, the message would repeat it.
        assert!(!first.contains(marker), "a key's value reached the log");

        let within = logged_during(|| {
            warn_empty_hmac_keys(&weak, &throttle, start + EMPTY_KEY_WARNING_EVERY / 2);
        });
        assert_eq!(
            within, "",
            "the same endpoint is not repeated inside the interval"
        );

        let after = logged_during(|| {
            warn_empty_hmac_keys(&weak, &throttle, start + EMPTY_KEY_WARNING_EVERY);
        });
        assert_eq!(after.matches("WARN").count(), 1, "{after}");
    }

    /// The warning is on the delivery path, not just in its helper: `send` to an
    /// endpoint with an empty key still makes the request, signed with that key,
    /// and says so once however many events follow.
    #[tokio::test]
    async fn sending_to_an_endpoint_with_an_empty_key_warns_and_still_sends() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(2)
            .mount(&server)
            .await;
        let endpoint = WebhookEndpoint {
            id: "cendpointemptykeysend000".to_owned(),
            url: format!("{}/hook", server.uri()),
            secret: Some(String::new()),
            ..hmac_endpoint()
        };
        let attempt = fixture_attempt();
        let http = build_http_client();
        let (logs, _guard) = Logs::capture();

        for _ in 0..2 {
            let outcome = super::send(&http, None, &endpoint, &attempt, "{}", Utc::now()).await;
            assert!(matches!(outcome, super::Outcome::Success { status: 200 }));
        }

        let text = logs.text();
        assert_eq!(text.matches("WARN").count(), 1, "{text}");
        assert!(text.contains("cendpointemptykeysend000"), "{text}");
    }

    /// A row with no secret at all (the CHECK makes one impossible) is a failed
    /// delivery like any other, never a request signed with nothing: counted,
    /// backed off, and fed to the breaker by `write_outcome`.
    #[test]
    fn an_hmac_endpoint_with_no_secret_at_all_is_an_ordinary_failure() {
        let http = build_http_client().hmac;
        let endpoint = WebhookEndpoint {
            secret: None,
            ..hmac_endpoint()
        };
        let outcome = hmac_request(&http, &endpoint, &fixture_attempt(), "{}", Utc::now());
        let Err(super::Outcome::Retryable {
            status: None,
            message,
        }) = outcome
        else {
            panic!("a NULL secret must be a retryable failure and build no request");
        };
        assert!(message.contains("rotateWebhookSecret"), "{message}");
    }

    /// A sealed attempt that reaches a worker with no signing key (the claim does
    /// not let one, [`crate::claim::ServedModes`]) is an ordinary failure that names
    /// the flag: nothing is built, and `write_outcome` counts it, backs it off and
    /// feeds the breaker like any other.
    #[tokio::test]
    async fn a_sealed_endpoint_without_a_signing_key_is_an_ordinary_failure_naming_the_flag() {
        let http = build_http_client();
        let outcome = super::send(
            &http,
            None,
            &fixture_endpoint(),
            &fixture_attempt(),
            "{}",
            Utc::now(),
        )
        .await;
        let super::Outcome::Retryable {
            status: None,
            message,
        } = outcome
        else {
            panic!("a sealed delivery with no signer must be Retryable, not a new outcome");
        };
        assert!(message.contains("--envelope-signing-key-file"), "{message}");
    }

    /// A sealed row whose URL the seal cannot bind never reaches the network and
    /// is an ordinary failure of the delivery: counted, backed off and able to go
    /// `dead`. The database no longer lets a sealed row hold such a URL, so this is
    /// the worker's second line of defence for a row it cannot trust, tested
    /// without one.
    #[tokio::test]
    async fn a_sealed_endpoint_whose_url_cannot_be_sealed_is_an_ordinary_failure() {
        let http = build_http_client();
        let sealer =
            Sealer::new(cratestack::cose::Ed25519Signer::from_seed(&[0x42; 32])).expect("a sealer");
        let endpoint = WebhookEndpoint {
            url: "https://h.example.test/hook?tenant=1".to_owned(),
            ..fixture_endpoint()
        };
        let outcome = super::send(
            &http,
            Some(&sealer),
            &endpoint,
            &fixture_attempt(),
            "{}",
            Utc::now(),
        )
        .await;
        let super::Outcome::Retryable {
            status: None,
            message,
        } = outcome
        else {
            panic!("a URL with a query cannot be sealed for: expected Retryable");
        };
        assert!(message.contains("not sealed"), "{message}");
    }
}
