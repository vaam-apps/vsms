//! Assembling the generated router.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use cratestack::axum::extract::{ConnectInfo, Request, State};
use cratestack::axum::http::{header, Method};
use cratestack::axum::middleware::{from_fn_with_state, Next};
use cratestack::axum::response::Response;
use cratestack::axum::Router;
use cratestack::idempotency::{IdempotencyLayer, IdempotencyStore};
use cratestack::ratelimit::{
    InMemoryRateLimitStore, RateLimitConfig, RateLimitLayer, RateLimitStore,
};
use cratestack::{AuthProvider, RequestContext, SqlxIdempotencyStore, Value};
use cratestack_codec_json::JsonCodec;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::auth::GatewayAuth;
use crate::pepper::HashPepper;
use crate::procedures::Procedures;
use crate::rbac::{enforce_route_permission, RbacState, RoutePermission};
use crate::schema;

/// #24's concrete write-route anchor, for #25's gate test to target (see
/// `docs/architecture.md` §5.2's role table: `operator` has
/// `provider:read/update`, `developer` has no `provider:*` permission at
/// all). The permission literal enforced here is `provider:update` —
/// §5.2's own name for it, not the informal "provider:write" phrasing the
/// M1 gate description (§ milestone table) uses for the route in prose.
/// Caught live by Lightbridge's review of #112: an earlier draft of this
/// constant checked the literal `"provider:write"`, which appears nowhere
/// in §5.2's actual vocabulary — it would have silently and permanently
/// denied a legitimate `operator` token the moment a role-bearing token
/// exists to present it, since that token's `perms` would carry
/// `provider:update`, never `provider:write`.
///
/// Picked over a procedure because none of the seven exists for this —
/// `Provider.update`'s own generated route (`PATCH /providers/{id}`,
/// confirmed via `route_table()`) is the natural CRUD write action the
/// design doc's role table is describing. Its own `@@allow` (`schema.cstack`):
/// `hasRole('owner') || hasRole('admin') || hasRole('operator')` — no
/// `hasRole('app')`, unlike `sendMessage`'s procedure-level `@allow`. Combined
/// with `GatewayAuth` only ever minting `role: "app"` or `role: "system"`
/// (see its own doc — this deployment has no human-login path yet, #23/#24/
/// #25's own tracked scope cut), Layer 1 alone already refuses every token
/// this deployment can currently issue on this route, developer-shaped or
/// not. This gate is real and tested (see `rbac.rs`'s own tests plus
/// `tests/rbac_layer2_live_postgres.rs`) but is defense in depth today, not
/// yet the thing a live caller actually bounces off — it becomes load-
/// bearing the moment a role-bearing token exists to test the *positive*
/// case against, which is why this constant, not a bespoke one-off, is what
/// #25 should extend.
const PROVIDER_WRITE_ROUTES: &[RoutePermission] = &[RoutePermission {
    method: Method::PATCH,
    path: "/providers/{id}",
    permission: "provider:update",
}];

/// #56/#57: unlike [`PROVIDER_WRITE_ROUTES`], this gate is the *real*
/// perimeter, not defense in depth — see `schema.cstack`'s own comment on
/// `Job`'s `@@allow("list"/"detail", ...)` for the full reasoning. `Job`
/// carries no `appId`, so Layer 1 has no per-row predicate to scope
/// `auth().kind == "app"` by the way `Message`'s own policy scopes by
/// `appId == auth().appId`; any provisioned app client passes Layer 1
/// exactly the same as the admin console's own credential does. Only a
/// granted `job:read` scope (`AppClient.scopes`, §5.2 — the admin console's
/// own client is provisioned with it, `scripts/demo.sh`) stands between a
/// customer's app client and the whole system's job backlog. Two routes,
/// not one: `GET /jobs` (the backlog list, #56) and `GET /jobs/{id}` (the
/// generated router's own detail lookup, reachable independently of the
/// list route).
const JOB_READ_ROUTES: &[RoutePermission] = &[
    RoutePermission {
        method: Method::GET,
        path: "/jobs",
        permission: "job:read",
    },
    RoutePermission {
        method: Method::GET,
        path: "/jobs/{id}",
        permission: "job:read",
    },
];

/// #153: the TTL a cached `Idempotency-Key` response stays replayable —
/// matches `docs/architecture.md` §4.5's own figure, 24 hours. Exposed so
/// every construction site (the real `sms-gateway serve` command and the
/// live HTTP test suites in `crates/sms-auth/tests/`) shares one source of
/// truth instead of hand-copying the duration; `serve` still lets an
/// operator override it via `--idempotency-ttl-secs`/`SMS_IDEMPOTENCY_TTL_SECS`.
pub const DEFAULT_IDEMPOTENCY_TTL: Duration = Duration::from_hours(24);

/// #153: §4.5's own suggested per-principal ingress budget — burst 120,
/// refilling 2 tokens/second.
///
/// Verified against this workspace's actual live HTTP call volume, not just
/// asserted: only `crates/sms-auth/tests/{rbac_layer2_live_postgres,
/// oidc_flow_live}.rs` and `app/sms-gateway/tests/*_live_postgres.rs` ever
/// drive requests through this router over real HTTP — every other live
/// suite (the ones doing the heavy claim-loop/dispatch/chaos volume) calls
/// `Procedures` directly, bypassing this layer entirely, or hits the
/// separately-mounted `/dlr/{providerKey}` and `/token` routes, which this
/// layer never wraps (see `router()`'s own doc). Each of the suites that
/// *does* go through this router builds its own server (a fresh
/// `InMemoryRateLimitStore` per `router()` call) and sends at most a
/// handful of requests per run. 120 burst comfortably covers that; if a
/// future suite needs more, that's a sign the suite (or the token it
/// reuses) should be split, not that this default is wrong.
#[must_use]
pub fn default_rate_limit_config() -> RateLimitConfig {
    RateLimitConfig::new(120, 2.0)
}

/// #163: the budget for [`source_fingerprint`]'s coarser, source-scoped
/// bucket — see that function's own doc for what it keys on and why.
///
/// This is deliberately **not** sized like [`default_rate_limit_config`]'s
/// per-principal budget (burst 120, refill 2/s). That budget covers *one*
/// client's own traffic; this one has to cover *every* client sharing this
/// bucket's source at once, because in this deployment's actual topology
/// (`deploy/docker-compose.yml`) every caller's request arrives at this
/// router from one of exactly two internal peers — Caddy, proxying every
/// external caller, or `admin`, which talks to `sms-gateway` directly
/// (`SMS_API_URL: http://sms-gateway:8080` — see `source_fingerprint`'s own
/// doc for why that matters). A budget sized for one client would throttle
/// every *other* legitimate client sharing that same peer the moment any
/// one of them got busy. 10x the per-principal burst and 5x its refill
/// rate — generous enough that this deployment's own real traffic (the 14
/// live-Postgres suites and `just demo` that exercise this router over real
/// HTTP; see `default_rate_limit_config`'s own doc for which ones) never
/// trips it, while still bounding the aggregate a forged-`sub` flood can
/// reach to a fixed, finite number rather than the unbounded supply
/// [`client_id_fingerprint`] alone allows.
#[must_use]
pub fn default_source_rate_limit_config() -> RateLimitConfig {
    RateLimitConfig::new(1200, 10.0)
}

/// #153: both `IdempotencyLayer` and `RateLimitLayer` ship a default
/// principal fingerprint that hashes the raw `Authorization` header bytes.
/// **That default is wrong for this deployment** — found live while
/// verifying #153's own acceptance bar, not by reading the library's
/// source: every caller here is an `OAuth2` `client_credentials` client
/// (`auth.rs`'s `GatewayAuth`), and a freshly minted access token is a
/// *different* `Authorization` header value every time — same client,
/// different bytes. Replaying the identical `Idempotency-Key` under a
/// newly minted token (an entirely realistic sequence: a client's cached
/// token happens to expire between the original attempt and its retry, or
/// a client that doesn't cache at all mints a fresh token per call)
/// produced a **second** `Message` row rather than replaying the first
/// response, under the library's own default — the opposite of what #153
/// exists to guarantee.
///
/// **Used for `RateLimitLayer` only** — read
/// [`verified_idempotency_fingerprint`]'s own doc for why `IdempotencyLayer`
/// needs a stronger guarantee than this function gives and does *not* use
/// it. `sub` is read here **without verifying the token's signature**,
/// exactly like the upstream default this replaces (which hashes the
/// header without verifying it either) — a forged `sub` only lets an
/// attacker pick their *own* rate-limit bucket, an act of self-throttling
/// evasion, never someone else's. That is real and worth stating plainly
/// rather than glossing over: **an attacker willing to forge a `sub` claim
/// on every request gets a fresh, full bucket every time from this
/// function alone** — it only makes the limiter correctly recognise
/// *honest* retries and refreshes from the *same* real client as one
/// principal; it does not, by itself, make the limiter hostile-caller-proof.
/// Filed as [#163](https://github.com/vymalo/vsms/issues/163) — distinct
/// from `#156` (the unauthenticated `/token` edge, which this deployment's
/// own issue tracker is explicit does not cover this in-process layer).
///
/// **#163's fix does not change this function** — it adds a second,
/// coarser `RateLimitLayer` alongside it (see [`source_fingerprint`] and
/// `router()`'s own doc for the layering) rather than replacing this one.
/// This function stays exactly as bypassable as its own doc above already
/// says, on purpose: it is still what gives an *honest* client fair,
/// undiluted per-principal throughput, and nothing about closing the
/// unbounded-bucket-forging bypass requires taking that away from honest
/// traffic. What changes is that a forger can no longer turn "a fresh
/// bucket per request" into "unbounded aggregate throughput" — the second
/// layer bounds the aggregate regardless of how many buckets this one
/// function hands out.
fn client_id_fingerprint(req: &Request) -> String {
    let Some(auth_header) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return "anonymous".to_owned();
    };
    if let Some(sub) = jwt_sub_unverified(auth_header) {
        return format!("client:{sub}");
    }
    // Not a well-formed `Bearer <jwt>` we can read a `sub` out of —
    // `GatewayAuth` will reject it regardless (or it's a genuinely
    // unauthenticated request some other route tolerates), so this only
    // needs *a* stable-ish bucket, matching the upstream default exactly.
    let mut hasher = Sha256::new();
    hasher.update(auth_header.as_bytes());
    format!("auth:{:x}", hasher.finalize())
}

/// The `sub` claim out of a `Bearer <jwt>` header's payload segment, with
/// **no signature verification** — see [`client_id_fingerprint`]'s own doc
/// for why that's the correct trust level here. Returns `None` for
/// anything not shaped like a JWT bearer token (missing `Bearer ` prefix,
/// not three dot-separated segments, invalid base64url, invalid JSON, no
/// `sub`, or an empty one), letting the caller fall back to hashing the
/// raw header instead.
fn jwt_sub_unverified(auth_header: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct SubOnly {
        sub: Option<String>,
    }

    let token = auth_header.strip_prefix("Bearer ")?;
    let payload_segment = token.split('.').nth(1)?;
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload_segment).ok()?;
    let claims: SubOnly = serde_json::from_slice(&payload_bytes).ok()?;
    claims.sub.filter(|sub| !sub.is_empty())
}

/// #163's actual fix: the key function for a *second*, coarser
/// `RateLimitLayer` mounted alongside [`client_id_fingerprint`]'s own (see
/// `router()`'s own doc for the layering) — closes the gap that function's
/// doc names plainly: an attacker willing to forge a fresh `sub` on every
/// request gets a fresh, full bucket from that function alone, so nothing
/// bounds the *aggregate* throughput a flood of forged identities can
/// reach. This function does, by keying on something a forged `sub` cannot
/// change: the real TCP peer this request arrived on.
///
/// **Chosen over trusting `X-Forwarded-For`, deliberately — this is the
/// trap to check, not assume away.** `AGENTS.md` documents `sms-gateway
/// serve`'s listen address as loopback by default, with TLS terminating at
/// a Caddy edge in front of it (`app/sms-gateway/src/main.rs`'s own
/// `--listen` doc) — a topology where trusting a *specific, known* upstream
/// hop's `X-Forwarded-For` can be a defensible, deliberate choice. This
/// deployment does not make that choice, for a concrete reason found while
/// designing this fix, not a hypothetical one: `deploy/docker-compose.yml`
/// has **two** internal callers of this router, not one — Caddy, fronting
/// every external caller (`deploy/Caddyfile`'s `reverse_proxy
/// sms-gateway:8080`), *and* `admin`, which talks to `sms-gateway` directly
/// over the same compose network (`SMS_API_URL: http://sms-gateway:8080`,
/// bypassing Caddy entirely) to serve its own server-side API calls on
/// behalf of a browser session. A blanket "trust `X-Forwarded-For`"
/// config can't tell those two apart — it would either trust `admin`'s own
/// requests to forge an arbitrary claimed origin (`admin` has no reason to
/// ever set the header today, but nothing stops a future change from
/// adding one, and this function has no way to know that didn't happen),
/// or need a second axis of configuration (a trusted-peer allowlist) this
/// deployment has no static-IP infrastructure to make reliable — Docker
/// Compose assigns container IPs dynamically, not pinned. Building that
/// well is real, separate infrastructure work, not a fingerprint-function
/// one-liner; until it exists, reading a client-supplied header here would
/// be **worse than not using IP at all** — exactly the failure mode this
/// doc was asked to check for: it would let an attacker both dodge this
/// bucket (claim a fresh forged IP per request, same shape as the `sub`
/// bypass this function exists to close) and attribute their traffic to a
/// victim's real IP, throttling someone who never sent a request.
///
/// **What this function uses instead, and why it still closes the bypass
/// even though it can't distinguish individual external clients from each
/// other.** `ConnectInfo<SocketAddr>` — populated by axum from the actual
/// accepted socket when this router is served through
/// `into_make_service_with_connect_info::<SocketAddr>()` (`main.rs` does;
/// see `router()`'s own doc) — is the real TCP peer and cannot be spoofed
/// by anything in the request itself, the same property
/// `cratestack_axum::ratelimit`'s own upstream default key function
/// already relies on for its unauthenticated-request fallback. In this
/// deployment's real topology that peer is Caddy's own container address
/// for every external caller, or `admin`'s for the console's own traffic —
/// coarse, not per-external-client, but **bounded**: however many distinct
/// forged `sub` claims an attacker mints, every one of those requests
/// still arrives over the same accepted connection pool from the same
/// peer, so they all still draw from the *same* bucket here. That is
/// exactly what closes #163's bypass — the aggregate a flood of forged
/// identities can reach is now a fixed number
/// ([`default_source_rate_limit_config`]'s own budget), not an unbounded
/// supply of fresh buckets. A deployment that later adds a specific,
/// pinned, single trusted proxy hop could extend this function to read
/// `X-Forwarded-For` from it and regain per-external-client granularity;
/// this one does not have that hop pinned down today, so it does not
/// pretend to.
///
/// Falls back to a single shared `"unverified"` bucket when `ConnectInfo`
/// is absent (the router served some other way — a future embedding, or a
/// test harness not wired through connect-info) rather than skipping this
/// layer's protection entirely; matches the shape of every other shared
/// fallback bucket in this file (`client_id_fingerprint`'s `"anonymous"`,
/// `verified_idempotency_fingerprint`'s `"unverified"`).
fn source_fingerprint(req: &Request) -> String {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(
            || "unverified".to_owned(),
            |ConnectInfo(addr)| format!("ip:{}", addr.ip()),
        )
}

/// Request-extension marker [`verify_idempotency_principal`] stamps once a
/// bearer token has been **cryptographically verified** — see that
/// function's own doc for the vulnerability this exists to close. A
/// dedicated wrapper type, not a bare `String`, so it can't be confused
/// with (or accidentally overwritten by) some other `String` a future
/// middleware inserts into the same extensions map.
#[derive(Clone)]
struct VerifiedIdempotencyPrincipal(String);

/// State for [`verify_idempotency_principal`]: a `GatewayAuth` to run a
/// real verification against. Same shape as `rbac::RbacState`, and the
/// same precedent that module already establishes — `enforce_route_permission`
/// re-runs `GatewayAuth::authenticate` in a Tower layer ahead of the
/// generated router's own copy, calling that "cheap — cached JWKS and a
/// 60s-TTL `AppClient` lookup." This does the same thing, scoped even
/// narrower (see [`verify_idempotency_principal`]'s own doc).
#[derive(Clone)]
struct IdempotencyAuthState {
    auth: GatewayAuth,
}

/// #153 review fix: closes a cache-poisoning vector found in this PR's own
/// review, not by inspection, in the first version of this router's
/// `IdempotencyLayer` wiring.
///
/// **The vulnerability.** `IdempotencyLayer`/`RateLimitLayer` both sit
/// *outside* the generated router — including outside its own
/// `GatewayAuth` extractor — so by the time either layer's principal
/// fingerprint closure runs, no signature has been checked yet.
/// `cratestack-axum`'s `buffer_and_persist_response` caches a handler's
/// response under **any** status code, success or failure, with no
/// success check (verified against the pinned `cratestack-axum =0.7.8`
/// source, `src/idempotency/complete.rs`). Combine those two facts with
/// [`client_id_fingerprint`]'s *unverified* `sub` read and the attack is:
/// send `Authorization: Bearer <unsigned JWT, sub = victim_client_id>`
/// plus `Idempotency-Key: <a key the victim will use>` — `clientRef`
/// doubles as that key in `sendMessage`, and `examples/` demonstrates
/// human-chosen, guessable values like `rust-example-1`, so key
/// predictability is a weak barrier. `GatewayAuth` rejects the forged
/// token with `401` deep in the router, but `IdempotencyLayer` has
/// already reserved — and then caches — that `401` under
/// `client:victim_client_id` + that key. The victim's own later,
/// correctly signed request with the same key replays the cached `401`
/// instead of ever running `sendMessage`: a targeted denial of service on
/// a specific caller, requiring no valid credential at all. This is worse
/// than the duplicate-send bug #153 exists to fix — a stranger suppressing
/// *your* message is a worse trade than the duplicate #153 prevents.
///
/// **Why `RateLimitLayer` doesn't need this fix.** An unverified `sub`
/// there only lets an attacker inflate or evade *their own* rate-limit
/// budget (self-throttling evasion) — it can never touch another
/// caller's bucket in a way that harms them, because `RateLimitLayer`
/// only ever *consumes* a token from a bucket, never *writes a value
/// into it that a later request reads back* the way `IdempotencyLayer`'s
/// cached response is. See [`client_id_fingerprint`]'s own doc for that
/// distinction stated plainly, including the bypassability this
/// deliberately still leaves in place.
///
/// **The fix.** Mirrors `rbac::enforce_route_permission`'s own precedent
/// in this exact file: a Tower middleware holding a real [`GatewayAuth`],
/// re-running [`GatewayAuth::authenticate`] — full JWKS-backed signature
/// verification, the same call the generated router's own extractor is
/// about to make a moment later on the same request — and stamping the
/// **verified** `sub` into a request extension only on success. Nothing
/// here ever rejects a request itself; that stays `GatewayAuth`'s and
/// Layer 2's job, run again deeper in the same pipeline exactly as
/// before. A request whose token fails this verification (or carries no
/// token at all) simply gets no verified-principal extension, and
/// [`verified_idempotency_fingerprint`] below buckets it under a single
/// shared `"unverified"` partition instead — attacker-uncontrollable,
/// because nothing about *which* shared bucket a forged token lands in
/// depends on the (still-unverified, still-forgeable) `sub` it claims.
/// Only a request that presents a **real, validly signed** token for a
/// given `client_id` can ever cause a write into that `client_id`'s own
/// idempotency bucket.
///
/// **Cost, measured rather than assumed.** Scoped two ways to only pay
/// this at all when it can possibly matter: only for requests whose
/// method is one `cratestack_axum::idempotency::is_idempotent_target_method`
/// (POST/PATCH/PUT/DELETE) admits — reused directly from that crate so
/// this gate can't drift from `IdempotencyService::call`'s own — and only
/// when an `Idempotency-Key` header is actually present, since
/// `IdempotencyService::call` never invokes the principal-fingerprint
/// closure at all otherwise (it bypasses the store before ever reading
/// that closure), so verifying for every other request would be pure
/// waste for no benefit. For the requests that do qualify, this is a
/// *second* `authenticate` call on top of the generated router's own (a
/// *third*, on the one `PROVIDER_WRITE_ROUTES` route that also carries
/// Layer 2 — already an accepted, documented double-verification today).
///
/// Measured live against a real gateway with a warm JWKS/`AppClient`
/// cache (not assumed): 30 real `POST /$procs/sendMessage` calls with a
/// valid token and no `Idempotency-Key` averaged **9.75ms**; 30 more,
/// identical except for a fresh `Idempotency-Key` each time, averaged
/// **13.54ms** — a **~3.8ms** delta, consistent with one extra
/// cached-JWKS RS256 verify plus a 60s-TTL-cached `AppClient` lookup, not
/// a network round trip. See `docs/architecture.md` §4.5 for the same
/// figures in context.
async fn verify_idempotency_principal(
    State(state): State<IdempotencyAuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    // Mirrors `cratestack_axum::idempotency::is_idempotent_target_method`
    // exactly (reused, not reimplemented, so the two can't drift) —
    // `IdempotencyService::call` checks the method *before* it ever looks
    // at the `Idempotency-Key` header, bypassing the store entirely for
    // anything that isn't POST/PATCH/PUT/DELETE. Matching that gate here
    // means a GET carrying a stray `Idempotency-Key` header never pays a
    // verification cost that would end up unused regardless.
    if cratestack::idempotency::is_idempotent_target_method(&method)
        && request.headers().get("Idempotency-Key").is_some()
    {
        let path = request.uri().path().to_owned();
        let headers = request.headers().clone();
        let query = request.uri().query().map(str::to_owned);
        let request_ctx = RequestContext {
            method: method.as_str(),
            path: &path,
            query: query.as_deref(),
            headers: &headers,
            // `GatewayAuth::authenticate` never reads the body — see
            // `rbac::enforce_route_permission`'s identical comment on
            // this exact point.
            body: &[],
        };
        if let Ok(ctx) = state.auth.authenticate(&request_ctx).await {
            if let Some(Value::String(sub)) = ctx.auth_field("sub") {
                request
                    .extensions_mut()
                    .insert(VerifiedIdempotencyPrincipal(format!("client:{sub}")));
            }
        }
        // A verification failure is deliberately silent here: this
        // middleware never rejects anything itself (see this function's
        // own doc), it only decides which idempotency bucket a request is
        // *allowed* to write into — an unverified request simply gets no
        // extension, and falls into the shared fallback bucket below.
    }
    next.run(request).await
}

/// Read by `IdempotencyLayer::with_principal_fingerprint` only — see
/// [`verify_idempotency_principal`]'s own doc for why `IdempotencyLayer`
/// cannot use the cheaper, unverified [`client_id_fingerprint`]
/// `RateLimitLayer` uses. Returns the verified `client:{sub}` fingerprint
/// [`verify_idempotency_principal`] stamped when (and only when) real
/// signature verification succeeded; otherwise a single shared
/// `"unverified"` bucket — deliberately *not* a per-request-unique value,
/// matching the shape of the upstream library's own `"anonymous"`
/// fallback, and safe for the identical reason: nothing about which
/// shared bucket a failing-verification request lands in is
/// attacker-choosable in a way that targets a specific victim.
fn verified_idempotency_fingerprint(req: &Request) -> String {
    req.extensions()
        .get::<VerifiedIdempotencyPrincipal>()
        .map_or_else(|| "unverified".to_owned(), |principal| principal.0.clone())
}

/// Build the HTTP surface: generated model CRUD plus the seven procedures.
///
/// `auth` validates every request against the OP's own published JWKS —
/// see [`GatewayAuth`]'s own documentation for what it accepts and why.
///
/// `pepper` is real secret material (#134) — see `pepper.rs`'s module doc
/// for the scheme and the rotation consequence. `sms-gateway serve`'s CLI
/// parsing rejects a missing or too-short pepper before this is ever
/// called, so this function itself never needs to.
///
/// Wrapped in [`enforce_route_permission`] (Layer 2, #24) gating
/// [`PROVIDER_WRITE_ROUTES`] — every other route passes through unchanged;
/// see that constant's own doc and `rbac.rs`'s module doc for the full
/// reasoning.
///
/// #153 mounts the two Tower layers `docs/architecture.md` §4.5 always
/// documented but this function never actually built:
/// [`IdempotencyLayer`] (replaying a cached response for a repeated
/// `Idempotency-Key`, so a client retrying a timed-out `sendMessage` call
/// cannot cause a second SMS the way `ProviderError::Indeterminate` — #119
/// — already refuses to on the provider side) and [`RateLimitLayer`] (a
/// per-principal token bucket, `429` + `Retry-After` once exhausted). Both
/// wrap *only* the routes this function builds — the generated CRUD/
/// procedure surface — never the OP's `/token`/JWKS routes, the DLR
/// webhook, or `/healthz`, all of which `app/sms-gateway/src/main.rs`
/// `.merge()`s in afterwards as siblings, outside this `Router`'s own
/// layer stack:
///
/// - `/token` is explicitly out of scope for in-process rate limiting per
///   §4.2 — it is a reverse-proxy concern, and `authkestra_op` already
///   requires no shared secret this layer could help protect.
/// - The DLR webhook (`POST /dlr/{providerKey}`) is a provider-initiated
///   callback with no `Idempotency-Key` header a webhook sender would ever
///   send, and `dlr.rs`'s own idempotent-by-construction handling
///   (matching by `providerMessageRef`) already covers replay.
/// - `/healthz` must never be rate-limited or held behind an idempotency
///   reservation: a liveness probe throttled into failure gets the
///   container killed, which is the opposite of what a health check is
///   for.
///
/// Layering order (`.layer()` calls compose outside-in, so the *last*
/// call is what a request meets *first*): the two `RateLimitLayer`s are
/// outermost — [`source_fingerprint`]'s coarser, `ConnectInfo`-keyed one
/// first, then [`client_id_fingerprint`]'s per-principal one — rejecting
/// an abusive caller with a cheap `429` before any request body is
/// buffered, any signature verified, or the idempotency store touched;
/// [`verify_idempotency_principal`] is next — a real, verified-signature
/// re-authentication, but *only* for requests carrying an `Idempotency-Key`
/// header (see that function's own doc for why, and for the
/// cache-poisoning vector this closes); `IdempotencyLayer` is next,
/// reserving/replaying against `cratestack_idempotency` keyed by whatever
/// that verification established; the RBAC layer and the generated router
/// (whose own extractors run [`GatewayAuth`] a further time) sit innermost.
///
/// **#163: two `RateLimitLayer`s, not one, and that's deliberate — see
/// [`source_fingerprint`]'s own doc for the full reasoning.** The
/// per-principal one alone (`client_id_fingerprint`) is bypassable by a
/// caller willing to forge a fresh, unverified `sub` claim per request —
/// each forged identity gets its own fresh, full bucket. The second,
/// coarser layer bounds the *aggregate* a flood of such forgeries can
/// reach, keyed on the real TCP peer (`ConnectInfo<SocketAddr>`, populated
/// because `app/sms-gateway/src/main.rs` serves this router through
/// `into_make_service_with_connect_info::<SocketAddr>()` — required for
/// this layer to do anything at all; see that function's doc for what
/// happens without it) rather than anything request-content-derived a
/// forger could vary. Order between the two barely matters for
/// correctness (both are cheap token-bucket consumes); the coarser one
/// runs first on the theory that a flood large enough to matter trips it
/// before ever reaching the finer-grained one.
///
/// `SqlxIdempotencyStore` and `InMemoryRateLimitStore` are each the *only*
/// store either upstream crate ships (`docs/architecture.md` §4.5/§4.6) —
/// there is no second implementation to choose between, so `router()`
/// constructs both internally from `db`'s own pool rather than taking them
/// as parameters — including a **second, independent**
/// `InMemoryRateLimitStore` for [`source_fingerprint`]'s layer, so the two
/// `RateLimitLayer`s' budgets can never interact even though their key
/// namespaces (`"client:"`/`"auth:"`/`"anonymous"` vs. `"ip:"`/
/// `"unverified"`) already can't collide. `InMemoryRateLimitStore` means
/// the rate limit is per-process: correct for this deployment's single
/// gateway replica (`deploy/docker-compose.yml`), and explicitly not
/// cluster-wide — a multi-replica deployment would need a
/// Redis/Postgres-backed `RateLimitStore`, which does not exist yet
/// (§4.6).
///
/// The `cratestack_idempotency` table itself is **not** created here —
/// `SqlxIdempotencyStore::ensure_schema()` is deliberately never called by
/// this binary. See `deploy/migrate.sql`'s own header for why: creating it
/// is treated as migration-shaped bookkeeping owned by the one-shot
/// migrate job, the same way `schema_migrations` itself is, rather than
/// DDL the serving process runs (and needs privilege for) at every start.
///
/// **Neither layer uses either upstream crate's own default fingerprint**
/// (a raw-`Authorization`-header hash) — but the two now use *different*
/// replacements, not the same one, and that split is itself load-bearing:
/// `RateLimitLayer` uses [`client_id_fingerprint`] (an *unverified* `sub`
/// read — cheap, run on every request, and its own doc states plainly
/// what that leaves open); `IdempotencyLayer` uses
/// [`verified_idempotency_fingerprint`] (backed by
/// [`verify_idempotency_principal`]'s *verified* `sub`) — see that
/// function's own doc for why `IdempotencyLayer` specifically cannot
/// tolerate the cheaper, unverified version `RateLimitLayer` still uses.
// No `#[must_use]`: axum's `Router` already carries one, and doubling it is
// what `clippy::double_must_use` objects to.
pub fn router(
    db: schema::Cratestack,
    auth: GatewayAuth,
    pepper: HashPepper,
    idempotency_ttl: Duration,
    rate_limit: RateLimitConfig,
    source_rate_limit: RateLimitConfig,
) -> Router {
    let rbac_state = RbacState {
        auth: auth.clone(),
        requirements: PROVIDER_WRITE_ROUTES,
    };
    // #56/#57: a second, independent `enforce_route_permission` instance
    // rather than merging into `rbac_state` above — `RbacState.requirements`
    // is one `&'static [RoutePermission]`, and stacking a second Tower layer
    // is simpler than hand-concatenating two const slices at compile time
    // for no behavioural difference (`enforce_route_permission` is a no-op
    // for any request that doesn't match its own `requirements`, so two
    // layers with disjoint route sets compose exactly like one layer with
    // the union would).
    let job_rbac_state = RbacState {
        auth: auth.clone(),
        requirements: JOB_READ_ROUTES,
    };
    let idempotency_auth_state = IdempotencyAuthState { auth: auth.clone() };
    let idempotency_store: Arc<dyn IdempotencyStore> =
        Arc::new(SqlxIdempotencyStore::new(db.pool().clone()));
    let rate_limit_store: Arc<dyn RateLimitStore> = Arc::new(InMemoryRateLimitStore::new());
    // #163: a second, independent store — see router()'s own doc for why
    // this must not share the per-principal layer's store.
    let source_rate_limit_store: Arc<dyn RateLimitStore> = Arc::new(InMemoryRateLimitStore::new());

    schema::axum::router(db, Procedures::new(pepper), JsonCodec, auth)
        .layer(from_fn_with_state(rbac_state, enforce_route_permission))
        .layer(from_fn_with_state(job_rbac_state, enforce_route_permission))
        .layer(
            IdempotencyLayer::new(idempotency_store, idempotency_ttl)
                .with_principal_fingerprint(verified_idempotency_fingerprint),
        )
        .layer(from_fn_with_state(
            idempotency_auth_state,
            verify_idempotency_principal,
        ))
        .layer(RateLimitLayer::new(rate_limit_store, rate_limit).with_key_fn(client_id_fingerprint))
        .layer(
            RateLimitLayer::new(source_rate_limit_store, source_rate_limit)
                .with_key_fn(source_fingerprint),
        )
}

/// Every route the schema generated, for `sms-gateway routes`.
///
/// Enumerated rather than hardcoded because `pluralize()` is naive
/// (`ends_with('s') ? +"es" : +"s"`) and there is no `@@map`, so the only
/// reliable statement of a path is the one the macro emitted.
#[must_use]
pub fn route_table() -> Vec<(&'static str, &'static str)> {
    schema::axum::ROUTE_TRANSPORTS
        .iter()
        .map(|route| (route.method, route.path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_generated_a_route_surface() {
        let routes = route_table();
        assert!(!routes.is_empty(), "no routes were generated");
    }

    #[test]
    fn messages_are_reachable_under_a_pluralised_path() {
        // Guards the `pluralize()` trap: if the emitter's naive pluralisation
        // ever changes, this fails here rather than in the admin console.
        let routes = route_table();
        assert!(
            routes.iter().any(|(_, path)| path.contains("messages")),
            "expected a /messages path, got: {routes:?}"
        );
    }

    // Async because a lazy pool still spawns a background task on construction,
    // so `connect_lazy` panics with "this functionality requires a Tokio
    // context" outside a runtime — even though it never opens a connection.
    #[tokio::test]
    async fn router_builds_without_a_live_database() {
        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        let db = schema::Cratestack::builder(pool).build();
        let auth = GatewayAuth::new(
            db.clone(),
            "https://auth.invalid/jwks.json".to_owned(),
            "https://auth.invalid".to_owned(),
        );
        let pepper = HashPepper::new("a".repeat(crate::pepper::MIN_PEPPER_BYTES)).unwrap();
        let _router = router(
            db,
            auth,
            pepper,
            DEFAULT_IDEMPOTENCY_TTL,
            default_rate_limit_config(),
            default_source_rate_limit_config(),
        );
    }

    fn bearer_request(auth_header: &str) -> Request {
        cratestack::axum::extract::Request::builder()
            .header(header::AUTHORIZATION, auth_header)
            .body(cratestack::axum::body::Body::empty())
            .expect("building a minimal test request")
    }

    fn unsigned_jwt_with_sub(sub: &str) -> String {
        unsigned_jwt_with_sub_and_signature(sub, "not-a-real-signature")
    }

    /// Same shape as [`unsigned_jwt_with_sub`], but with a caller-chosen
    /// signature segment — used to model two genuinely distinct token
    /// mints (different `iat`/`exp`/`jti`/signature bytes in a real JWT)
    /// that nonetheless carry the same `sub`, the exact case #153's own
    /// live finding turned up.
    fn unsigned_jwt_with_sub_and_signature(sub: &str, signature: &str) -> String {
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"sub":"{sub}"}}"#));
        format!("eyJhbGciOiJSUzI1NiJ9.{payload}.{signature}")
    }

    #[test]
    fn jwt_sub_unverified_reads_the_sub_claim_with_no_signature_check() {
        // #153's own live-verified reason this function exists: a real
        // GatewayAuth-issued token's `sub` is the client_id, and this must
        // read it back out without needing (or being able to reach) a
        // JWKS — see this function's own doc.
        let token = unsigned_jwt_with_sub("appc_abc123");
        assert_eq!(
            jwt_sub_unverified(&format!("Bearer {token}")),
            Some("appc_abc123".to_owned())
        );
    }

    #[test]
    fn jwt_sub_unverified_returns_none_for_anything_not_bearer_jwt_shaped() {
        assert_eq!(jwt_sub_unverified("Basic dXNlcjpwYXNz"), None, "not Bearer");
        assert_eq!(
            jwt_sub_unverified("Bearer only-one-segment"),
            None,
            "no dot"
        );
        assert_eq!(
            jwt_sub_unverified("Bearer aGVhZGVy.not-valid-base64url!!!.sig"),
            None,
            "payload segment isn't valid base64url"
        );
        let not_json_payload = URL_SAFE_NO_PAD.encode(b"not json");
        assert_eq!(
            jwt_sub_unverified(&format!("Bearer h.{not_json_payload}.s")),
            None,
            "payload decodes but isn't the expected JSON shape"
        );
        let empty_sub_payload = URL_SAFE_NO_PAD.encode(br#"{"sub":""}"#);
        assert_eq!(
            jwt_sub_unverified(&format!("Bearer h.{empty_sub_payload}.s")),
            None,
            "an empty sub is treated the same as a missing one"
        );
    }

    #[test]
    fn client_id_fingerprint_partitions_by_sub_not_by_the_raw_token_bytes() {
        // The live-verified bug #153 exists to guard against: two requests
        // from the *same* client, carrying two *different* freshly minted
        // tokens (a real retry-after-refresh sequence), must land in the
        // same idempotency/rate-limit bucket — the upstream default (hash
        // the raw header) puts them in different buckets instead.
        let token_a = unsigned_jwt_with_sub_and_signature("appc_same_client", "mint-one");
        let token_b = unsigned_jwt_with_sub_and_signature("appc_same_client", "mint-two");
        assert_ne!(
            token_a, token_b,
            "the two tokens must differ, as two real mints would"
        );
        let req_a = bearer_request(&format!("Bearer {token_a}"));
        let req_b = bearer_request(&format!("Bearer {token_b}"));
        assert_eq!(client_id_fingerprint(&req_a), client_id_fingerprint(&req_b));
        assert_eq!(client_id_fingerprint(&req_a), "client:appc_same_client");
    }

    #[test]
    fn client_id_fingerprint_distinguishes_different_clients() {
        let req_a = bearer_request(&format!("Bearer {}", unsigned_jwt_with_sub("appc_a")));
        let req_b = bearer_request(&format!("Bearer {}", unsigned_jwt_with_sub("appc_b")));
        assert_ne!(client_id_fingerprint(&req_a), client_id_fingerprint(&req_b));
    }

    #[test]
    fn client_id_fingerprint_falls_back_to_anonymous_with_no_auth_header() {
        let req = cratestack::axum::extract::Request::builder()
            .body(cratestack::axum::body::Body::empty())
            .expect("building a minimal test request");
        assert_eq!(client_id_fingerprint(&req), "anonymous");
    }

    #[test]
    fn client_id_fingerprint_hashes_a_non_jwt_bearer_token_as_a_fallback() {
        let req = bearer_request("Bearer opaque-non-jwt-token");
        let fingerprint = client_id_fingerprint(&req);
        assert!(
            fingerprint.starts_with("auth:"),
            "expected the raw-header hash fallback, got {fingerprint:?}"
        );
    }

    #[test]
    fn verified_idempotency_fingerprint_falls_back_to_a_shared_unverified_bucket() {
        // No `VerifiedIdempotencyPrincipal` extension present — the shape
        // every request has *before* `verify_idempotency_principal` runs,
        // and the shape a failed verification leaves behind. Must not be
        // "anonymous" (client_id_fingerprint's own fallback) or any other
        // value derivable from request content alone — see this function's
        // own doc for why "shared and attacker-uncontrollable" is the
        // property that matters here, not the specific string.
        let req = bearer_request("Bearer irrelevant-unverified-token");
        assert_eq!(verified_idempotency_fingerprint(&req), "unverified");
    }

    #[test]
    fn verified_idempotency_fingerprint_reads_back_a_stamped_extension() {
        let mut req = bearer_request("Bearer irrelevant");
        req.extensions_mut().insert(VerifiedIdempotencyPrincipal(
            "client:appc_verified".to_owned(),
        ));
        assert_eq!(
            verified_idempotency_fingerprint(&req),
            "client:appc_verified"
        );
    }

    #[test]
    fn a_forged_sub_never_reaches_the_verified_fingerprint_without_the_extension() {
        // The exact security property this PR's review found missing:
        // `client_id_fingerprint` (rate-limit only) happily reads an
        // attacker-forged `sub` straight out of an unsigned token — that's
        // its documented, accepted tradeoff. `verified_idempotency_fingerprint`
        // must NOT do the same for the identical request: without
        // `verify_idempotency_principal` (the real signature check) having
        // run and stamped the extension, a forged `sub = victim_client_id`
        // must land in the shared fallback bucket, never in
        // "client:victim_client_id" — otherwise an attacker could still
        // write into (and poison) that victim's own idempotency cache.
        let forged = bearer_request(&format!(
            "Bearer {}",
            unsigned_jwt_with_sub("victim_client_id")
        ));
        assert_eq!(
            client_id_fingerprint(&forged),
            "client:victim_client_id",
            "sanity check: the unverified reader does trust the forged sub"
        );
        assert_eq!(
            verified_idempotency_fingerprint(&forged),
            "unverified",
            "the verified reader must not, absent a stamped extension"
        );
    }

    fn connect_info_request(addr: SocketAddr) -> Request {
        let mut req = cratestack::axum::extract::Request::builder()
            .body(cratestack::axum::body::Body::empty())
            .expect("building a minimal test request");
        req.extensions_mut().insert(ConnectInfo(addr));
        req
    }

    #[test]
    fn source_fingerprint_keys_on_the_real_connect_info_peer_address() {
        let a = connect_info_request("10.0.0.1:54321".parse().unwrap());
        let b = connect_info_request("10.0.0.1:9999".parse().unwrap());
        let c = connect_info_request("10.0.0.2:54321".parse().unwrap());
        // Same peer IP, different port (a second real connection from the
        // same source) — same bucket.
        assert_eq!(source_fingerprint(&a), source_fingerprint(&b));
        assert_eq!(source_fingerprint(&a), "ip:10.0.0.1");
        // Different peer IP — different bucket.
        assert_ne!(source_fingerprint(&a), source_fingerprint(&c));
    }

    #[test]
    fn source_fingerprint_is_unaffected_by_a_forged_sub_claim() {
        // The whole point of #163's fix: unlike client_id_fingerprint,
        // nothing about this function's key depends on request content a
        // forger controls.
        let mut a = connect_info_request("10.0.0.1:1".parse().unwrap());
        a.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {}", unsigned_jwt_with_sub("forged_a"))
                .parse()
                .unwrap(),
        );
        let mut b = connect_info_request("10.0.0.1:2".parse().unwrap());
        b.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {}", unsigned_jwt_with_sub("forged_b"))
                .parse()
                .unwrap(),
        );
        assert_ne!(client_id_fingerprint(&a), client_id_fingerprint(&b));
        assert_eq!(source_fingerprint(&a), source_fingerprint(&b));
    }

    #[test]
    fn source_fingerprint_falls_back_to_a_shared_bucket_without_connect_info() {
        // The router not being served through
        // `into_make_service_with_connect_info` (a misconfiguration this
        // deployment's own `main.rs` avoids — see `router()`'s own doc)
        // must not silently disable this layer's protection; every such
        // request shares one bucket instead.
        let req = cratestack::axum::extract::Request::builder()
            .body(cratestack::axum::body::Body::empty())
            .expect("building a minimal test request");
        assert_eq!(source_fingerprint(&req), "unverified");
    }

    /// #163's own acceptance bar: prove the bypass is closed against a
    /// real, bound HTTP server — not just that `source_fingerprint` reads
    /// the right extension in isolation. Every request below carries a
    /// *different* forged `sub`, so `client_id_fingerprint` alone would
    /// hand each one a fresh 1000-request bucket (its budget here is set
    /// wide open specifically so it can never be the thing that throttles
    /// this test) — the source-keyed layer must throttle them collectively
    /// instead, because every one of these requests shares the same real
    /// `ConnectInfo` peer: this test's own client.
    #[tokio::test]
    async fn a_flood_of_forged_subs_is_throttled_collectively_by_the_source_layer() {
        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        let db = schema::Cratestack::builder(pool).build();
        let auth = GatewayAuth::new(
            db.clone(),
            "https://auth.invalid/jwks.json".to_owned(),
            "https://auth.invalid".to_owned(),
        );
        let pepper = HashPepper::new("a".repeat(crate::pepper::MIN_PEPPER_BYTES)).unwrap();
        let app = router(
            db,
            auth,
            pepper,
            DEFAULT_IDEMPOTENCY_TTL,
            RateLimitConfig::new(1000, 1000.0),
            RateLimitConfig::new(3, 0.01),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding an ephemeral port");
        let addr = listener.local_addr().expect("reading the bound address");
        tokio::spawn(async move {
            let _ = cratestack::axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });

        let client = reqwest::Client::new();
        let mut statuses = Vec::new();
        for i in 0..5 {
            let token = unsigned_jwt_with_sub(&format!("forged_client_{i}"));
            let response = client
                .get(format!("http://{addr}/definitely-not-a-real-route"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .send()
                .await
                .expect("sending the request");
            statuses.push(response.status().as_u16());
        }

        // The bogus path never matches a generated route, so a request
        // that clears both RateLimitLayers reaches axum's own 404
        // fallback — proof it wasn't rejected by either layer, not proof
        // of anything about routing. The first 3 (the source bucket's
        // burst) get through; the 4th and 5th, with brand-new forged subs
        // no per-sub bucket has ever seen, still get 429 — the aggregate
        // is genuinely bounded regardless of how many distinct subs are
        // presented.
        assert_eq!(
            statuses,
            vec![404, 404, 404, 429, 429],
            "forging a fresh sub per request must not buy a fresh bucket \
             from the source-keyed layer, got {statuses:?}"
        );
    }

    /// The other half of #163's acceptance bar: a legitimate caller — one
    /// real client, one real `sub`, well inside both layers' *default*
    /// production budgets — must be completely unaffected by the new
    /// layer. Proves the fix doesn't trade the forged-sub bypass for a
    /// false-positive throttle on honest traffic.
    #[tokio::test]
    async fn a_legitimate_single_client_is_unaffected_by_the_source_layer_at_default_budgets() {
        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        let db = schema::Cratestack::builder(pool).build();
        let auth = GatewayAuth::new(
            db.clone(),
            "https://auth.invalid/jwks.json".to_owned(),
            "https://auth.invalid".to_owned(),
        );
        let pepper = HashPepper::new("a".repeat(crate::pepper::MIN_PEPPER_BYTES)).unwrap();
        let app = router(
            db,
            auth,
            pepper,
            DEFAULT_IDEMPOTENCY_TTL,
            default_rate_limit_config(),
            default_source_rate_limit_config(),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding an ephemeral port");
        let addr = listener.local_addr().expect("reading the bound address");
        tokio::spawn(async move {
            let _ = cratestack::axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });

        let client = reqwest::Client::new();
        let token = unsigned_jwt_with_sub("honest_client");
        for _ in 0..10 {
            let response = client
                .get(format!("http://{addr}/definitely-not-a-real-route"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .send()
                .await
                .expect("sending the request");
            assert_eq!(
                response.status().as_u16(),
                404,
                "a lone honest client well inside both default budgets must \
                 never see a 429 from either layer"
            );
        }
    }
}
