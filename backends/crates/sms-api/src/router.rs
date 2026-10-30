#![doc = include_str!("router.md")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cratestack::CratestackError;
use cratestack::SqlxIdempotencyStore;
use cratestack::axum::Router;
use cratestack::axum::extract::{ConnectInfo, Request};
use cratestack::axum::http::Method;
use cratestack::axum::middleware::{from_fn, from_fn_with_state};
use cratestack::cose::Ed25519Signer;
use cratestack::idempotency::{IdempotencyLayer, IdempotencyStore};
use cratestack::ratelimit::{
    InMemoryRateLimitStore, RateLimitConfig, RateLimitLayer, RateLimitStore, StoreErrorPolicy,
};
use cratestack_codec_json::JsonCodec;

use crate::auth::GatewayAuth;
use crate::bearer::{ApiDoors, BearerDoorState, enforce_bearer_door};
use crate::hmac_gate::{HmacWebhooks, enforce_hmac_gate};
use crate::owner_grant::{OwnerGrantState, enforce_owner_grant_guard};
use crate::pair_scope::enforce_pair_scope_guard;
use crate::pepper::HashPepper;
use crate::procedures::Procedures;
use crate::rbac::{RbacState, RoutePermission, enforce_route_permission};
use crate::schema;
use crate::signers::SignerDirectory;

/// The one transport (codec) the generated router is built with. Named so
/// the request-body guard in [`crate::owner_grant`] decodes with the *same*
/// codec the handlers do: a body the guard cannot read is a body the handler
/// would not be reading the same way.
pub(crate) const ROUTER_CODEC: JsonCodec = JsonCodec;

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
/// `hasRole('owner') || hasRole('admin') || hasRole('operator') ||
/// hasRole('system')` — the last disjunct added by #63 for
/// `dispatch.rs`'s own circuit-breaker writes
/// (`record_provider_failure`/`reset_provider_failures`, run under this
/// crate's internal `sys()` context) — and still no `hasRole('app')`,
/// unlike `sendMessage`'s procedure-level `@allow`. `system` changes
/// nothing about this route's own perimeter: `GatewayAuth::context_for`
/// (`backends/crates/sms-api/src/auth.rs`) never produces it, and
/// `crate::signers` refuses a person whose role is `system`; it exists only
/// inside `Procedures::sys()`. Since VS-3 a paired console key makes a
/// `user` principal a real caller here: an `owner`, `admin` or `operator`
/// clears Layer 1, and `provider:update` is then what Layer 2 asks of them
/// (an `admin` whose role lacks the permission is refused by this gate
/// alone). An app key never clears Layer 1. See `rbac.rs`'s own tests plus
/// `tests/rbac_layer2_live_postgres.rs` for the sealed positive and negative
/// cases.
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

/// #54: the same "unscoped but Layer 2-gated" shape [`JOB_READ_ROUTES`]
/// established for `Job`, applied to `Provider`/`Route` — see
/// `schema.cstack`'s own comment on each model's `read` `@@allow` for the
/// full reasoning. Both models gained `auth().kind == "app"` alongside
/// their existing human roles, and neither carries an `appId` to scope a
/// row-level predicate by, so a granted `provider:read`/`route:read` scope
/// (§5.2 — already the literal `operator`'s own `perms` carry) is the real
/// perimeter standing between a provisioned app client and every
/// `Provider`/`Route` row in the system, not defense in depth.
///
/// `POST /$procs/simulateRoute` is *not* listed here — that procedure
/// reads `Route`/`Provider` under a `sys()` context internally (see
/// `Procedures::simulate`'s own doc) and gates itself with
/// `require_permission(ctx, "route:read")` inside its own body, the same
/// pattern `requeueJob`/`workerLocks` already use for a procedure rather
/// than a generated CRUD route.
const PROVIDER_ROUTE_READ_ROUTES: &[RoutePermission] = &[
    RoutePermission {
        method: Method::GET,
        path: "/providers",
        permission: "provider:read",
    },
    RoutePermission {
        method: Method::GET,
        path: "/providers/{id}",
        permission: "provider:read",
    },
    RoutePermission {
        method: Method::GET,
        path: "/routes",
        permission: "route:read",
    },
    RoutePermission {
        method: Method::GET,
        path: "/routes/{id}",
        permission: "route:read",
    },
];

/// #53/#55: `SenderId`/`SenderIdRegistration`/`WebhookEndpoint` writes are
/// already Layer-1-restricted to human roles that must carry
/// `sender:manage`/`webhook:manage` (§5.2) — none of their
/// `@@allow("create"/"update"/"delete", ...)` clauses admit
/// `auth().kind == "app"`, unlike `Job`/`Provider`/`Route`'s own *read*
/// policies above. This is [`PROVIDER_WRITE_ROUTES`]'s own defense-in-depth
/// shape, applied to two more resources — same reasoning, same "Layer 2
/// narrows; it never widens" invariant (`rbac.rs`'s own module doc).
///
/// A signed-in `owner`/`admin`/`operator` (for the sender routes) or
/// `owner`/`admin`/`developer` (for the webhook routes) reaches these routes
/// as a `user` principal, by a paired console key.
///
/// `GET /sender_ids`, `GET /sender_id_registrations`, `GET
/// /webhook_endpoints`, `GET /webhook_attempts` are deliberately absent —
/// none of the four models' `read` `@@allow` admits `auth().kind == "app"`
/// unscoped the way `Job`/`Provider`/`Route` do (`WebhookEndpoint.read` is
/// narrower still, `owner`/`admin`/`developer`/`system` only, since #187), so
/// unlike [`PROVIDER_ROUTE_READ_ROUTES`]/[`JOB_READ_ROUTES`] there is no
/// machine-credential read path here for Layer 2 to be the real perimeter
/// for — a signed-in human's own `kind == "user"` already clears Layer 1 on
/// its own.
const SENDER_AND_WEBHOOK_WRITE_ROUTES: &[RoutePermission] = &[
    RoutePermission {
        method: Method::POST,
        path: "/sender_ids",
        permission: "sender:manage",
    },
    RoutePermission {
        method: Method::PATCH,
        path: "/sender_ids/{id}",
        permission: "sender:manage",
    },
    RoutePermission {
        method: Method::POST,
        path: "/sender_id_registrations",
        permission: "sender:manage",
    },
    RoutePermission {
        method: Method::PATCH,
        path: "/sender_id_registrations/{id}",
        permission: "sender:manage",
    },
    RoutePermission {
        method: Method::POST,
        path: "/webhook_endpoints",
        permission: "webhook:manage",
    },
    RoutePermission {
        method: Method::PATCH,
        path: "/webhook_endpoints/{id}",
        permission: "webhook:manage",
    },
    RoutePermission {
        method: Method::DELETE,
        path: "/webhook_endpoints/{id}",
        permission: "webhook:manage",
    },
];

/// #153: the TTL a cached `Idempotency-Key` response stays replayable —
/// matches `docs/architecture.md` §4.5's own figure, 24 hours. Exposed so
/// every construction site (the real `sms-gateway serve` command and the
/// live HTTP test suites in `backends/crates/sms-api/tests/`) shares one source of
/// truth instead of hand-copying the duration; `serve` still lets an
/// operator override it with `api.idempotency_ttl_secs` in `config/application*.yml`.
pub const DEFAULT_IDEMPOTENCY_TTL: Duration = Duration::from_hours(24);

/// #153: §4.5's own suggested per-principal ingress budget — burst 120,
/// refilling 2 tokens/second.
///
/// Verified against this workspace's actual live HTTP call volume, not just
/// asserted: only `backends/crates/sms-api/tests/rbac_layer2_live_postgres.rs`
/// and `backends/apps/sms-gateway/tests/*_live_postgres.rs` ever
/// drive requests through this router over real HTTP, sealed — every other live
/// suite (the ones doing the heavy claim-loop/dispatch/chaos volume) calls
/// `Procedures` directly, bypassing this layer entirely, or hits the
/// separately-mounted `/dlr/{providerKey}` route, which this
/// layer never wraps (see `router()`'s own doc). Each of the suites that
/// *does* go through this router builds its own server (a fresh
/// `InMemoryRateLimitStore` per `router()` call) and sends at most a
/// handful of requests per run. 120 burst comfortably covers that; if a
/// future suite needs more, that's a sign the suite should be split, not
/// that this default is wrong.
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
/// trips it, while still bounding what a flood of unauthenticated, unverifiable
/// messages can cost to a fixed, finite number: they never earn a verified
/// principal, so the per-principal budget inside the envelope never sees them.
#[must_use]
pub fn default_source_rate_limit_config() -> RateLimitConfig {
    RateLimitConfig::new(1200, 10.0)
}

/// The key function of the coarse, outermost `RateLimitLayer` (#163, kept by
/// VS-3): the real TCP peer, which nothing in a request can change.
///
/// It sits **outside** the envelope layer on purpose, and that is why it
/// survives the move to signed requests. Verifying a sealed request costs real
/// work before any per-principal limiter can run: a key lookup, a signature
/// check and a nonce write. cratestack says so itself ("put an IP-level limiter
/// outside this layer to bound what an unauthenticated flood can cost"), and
/// this deployment has exactly the flood to fear: anyone who can reach the port
/// can send garbage COSE messages with fresh `kid`s at no cost to themselves.
/// The per-principal limiter inside the envelope keys on the *verified*
/// principal (`client:<clientId>` / `user:<id>`), which a flood of forged
/// messages never earns, so only this layer bounds them. The old forged-`sub`
/// problem (#163) is gone with the bearer token that carried it; the coarse
/// bucket stays for the cost reason above.
///
/// **Chosen over trusting `X-Forwarded-For`, deliberately — this is the
/// trap to check, not assume away.** `AGENTS.md` documents `sms-gateway
/// serve`'s listen address as loopback by default, with TLS terminating at
/// a Caddy edge in front of it (`backends/apps/sms-gateway/src/main.rs`'s own
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
/// Build the HTTP surface: generated model CRUD plus every declared
/// procedure, behind the signed-transport envelope.
///
/// `signer` is the Ed25519 key that seals every response (`ServerKeys` loads
/// it from `--envelope-signing-key-file`); the keys requests are verified
/// with are read from `ClientKey` and `ConsoleKey` on every request. Fails
/// only if the envelope layer refuses its own configuration.
///
/// `hmac_webhooks` is `auth.methods`' answer for `hmac_webhooks`
/// ([`HmacWebhooks`]), read once by `sms-gateway serve`. It decides the generated
/// `WebhookEndpoint` writes here and the procedures that mint a signing secret.
///
/// `doors` is which ways in `auth.methods` opened ([`ApiDoors`]): the signed
/// envelope, a machine token, or both. `sms-gateway serve` builds it from
/// `permits(keypair)` and `machine_tokens_served()`; a router with neither door
/// cannot be built.
///
/// `pepper` is real secret material (#134) — see `pepper.rs`'s module doc for
/// the scheme and the rotation consequence. `sms-gateway serve`'s CLI
/// parsing rejects a missing or too-short pepper before this is ever
/// called, so this function itself never needs to.
///
/// # Layering, outermost first
///
/// `.layer()` calls compose outside-in, so the *last* call is what a request
/// meets *first*:
///
/// 1. a coarse `RateLimitLayer` keyed on the real TCP peer
///    ([`source_fingerprint`]) — outside the envelope on purpose, because
///    verifying a sealed message (a key lookup, a signature, a nonce write)
///    costs real work, and only a limiter outside it bounds what a flood of
///    unverifiable messages can spend;
/// 2. the envelope layer (`crate::envelope::layer`), whose mode follows the
///    doors (`Required`, `Optional` or `Off`: `crate::envelope::mode_for`): it
///    opens the `COSE_Sign1` message, checks the signature against the
///    `ClientKey`/`ConsoleKey` rows, records the nonce, names the principal
///    (`client:<clientId>` or `user:<id>`), hands the plain JSON request
///    inward and seals the response. Where the signed door is the only one, an
///    unsigned request is refused here with the coarse unsigned 401 and nothing
///    below it ever sees one;
/// 3. the bearer door ([`crate::bearer::enforce_bearer_door`]): refuses a signed
///    request that also carries `Authorization` (400), and for an unsigned one
///    verifies the machine token, resolves its client from rows and records the
///    same `client:<clientId>` principal a signature from that client gets. A
///    token caller gets plain JSON: no response seal, no contract-lock 426;
/// 4. a per-principal `RateLimitLayer` and the `IdempotencyLayer` (#153), both
///    keyed by cratestack's own default on the *verified* principal the
///    envelope or the door recorded - which replaces the two hand-written
///    fingerprint functions and the second signature check the token era
///    needed, because the principal can no longer be forged by a header, and
///    puts both doors in **one** bucket and **one** idempotency namespace;
/// 5. the Layer 2 guards: [`enforce_owner_grant_guard`] and the four
///    [`enforce_route_permission`] instances, which read the verified signer
///    through [`GatewayAuth`], and, innermost of them, [`enforce_hmac_gate`]
///    (VSA-1), which refuses a webhook endpoint write that makes an `hmac`
///    endpoint when `auth.methods` does not permit `hmac_webhooks` and is a no-op
///    otherwise: placed under the RBAC layers so a caller who lacks
///    `webhook:manage` hears that first;
/// 6. the generated router, whose own extractors run [`GatewayAuth`] a last
///    time.
///
/// What is **not** wrapped, because `sms-gateway serve` merges it in
/// afterwards as a sibling outside this `Router`'s layers: the provider
/// callbacks `POST /dlr/{providerKey}` (a provider cannot sign, and each
/// authenticates its own way — a shared secret, or SNS's X.509 signature),
/// `/healthz` and `/readyz` (a liveness probe throttled or sealed into failure
/// gets the container killed). There is no token endpoint, key set, discovery
/// document or login route to leave out: VS-4 deleted them, and a request for
/// one is answered by the router's not-found.
///
/// A sealed request must name `application/json` as its payload type
/// (`Cratestack-Payload-Type`) and ask for it back: the router's codec is
/// JSON, and the layer refuses any other type before a key is looked up.
///
/// `SqlxIdempotencyStore` and `InMemoryRateLimitStore` are each the *only*
/// store either upstream crate ships (`docs/architecture.md` §4.5/§4.6) —
/// there is no second implementation to choose between, so `router()`
/// constructs both internally from `db`'s own pool rather than taking them
/// as parameters — including a **second, independent**
/// `InMemoryRateLimitStore` for [`source_fingerprint`]'s layer, so the two
/// budgets can never interact. `InMemoryRateLimitStore` means the rate limit
/// is per-process: correct for this deployment's single gateway replica, and
/// explicitly not cluster-wide (§4.6). The nonce store, by contrast, is
/// Postgres, so a replay is caught whichever replica receives it.
///
/// The `cratestack_idempotency` table itself is **not** created here —
/// `SqlxIdempotencyStore::ensure_schema()` is deliberately never called by
/// this binary. See `deploy/migrate.sql`'s own header for why.
///
/// `RateLimitLayer` has no keyspace bound at this pin for a custom key
/// function (`RateLimitBucketBudget` shipped in 0.11.1); the per-principal
/// layer now uses the default, whose `princ:` bucket is unbudgeted *because*
/// a verified principal is not caller-mintable, and the source layer's keys are
/// bounded by the number of peers. Neither can be inflated by a forger.
///
/// # Errors
///
/// [`CratestackError`] when the envelope layer refuses its configuration.
// No `#[must_use]`: axum's `Router` already carries one, and doubling it is
// what `clippy::double_must_use` objects to.
pub fn router(
    db: schema::Cratestack,
    signer: Ed25519Signer,
    pepper: HashPepper,
    idempotency_ttl: Duration,
    rate_limit: RateLimitConfig,
    source_rate_limit: RateLimitConfig,
    hmac_webhooks: HmacWebhooks,
    doors: ApiDoors,
) -> Result<Router, CratestackError> {
    let directory = SignerDirectory::new(db.clone());
    let auth = GatewayAuth::new(directory.clone());
    let envelope = crate::envelope::layer(signer, &db, directory.clone(), &doors)?;
    let bearer_door_state = BearerDoorState { doors, directory };

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
    // #54: a third, independent instance; same reasoning.
    let provider_route_rbac_state = RbacState {
        auth: auth.clone(),
        requirements: PROVIDER_ROUTE_READ_ROUTES,
    };
    // #53/#55: a fourth, independent instance; same reasoning.
    let sender_and_webhook_rbac_state = RbacState {
        auth: auth.clone(),
        requirements: SENDER_AND_WEBHOOK_WRITE_ROUTES,
    };
    // Who may hand out `owner`: the write-path half of the guard whose
    // schema half is `User`'s `@@allow("create"/"update")` — see
    // `owner_grant.md`. Another independent layer, for the reason every
    // `RbacState` above is one.
    let owner_grant_state = OwnerGrantState { auth: auth.clone() };
    let idempotency_store: Arc<dyn IdempotencyStore> =
        Arc::new(SqlxIdempotencyStore::new(db.pool().clone()));
    let rate_limit_store: Arc<dyn RateLimitStore> = Arc::new(InMemoryRateLimitStore::new());
    // #163: a second, independent store — the two budgets must not interact.
    let source_rate_limit_store: Arc<dyn RateLimitStore> = Arc::new(InMemoryRateLimitStore::new());

    // cratestack 0.7.12 (cratestack#413): the generated `router()` has a
    // required `body_limit_bytes` parameter, once axum's implicit 2 MiB.
    // `DEFAULT_BODY_LIMIT_BYTES` is that same figure; no body this system
    // handles is remotely close. cratestack 0.11.0 (cratestack#328): the
    // `resolvers` parameter is `()` because `schemas/vsms.cstack` declares
    // no `@computed` field.
    Ok(schema::axum::router(
        db,
        Procedures::new(pepper, hmac_webhooks),
        (),
        ROUTER_CODEC,
        auth,
        cratestack::DEFAULT_BODY_LIMIT_BYTES,
    )
    // VSA-1: innermost, so every RBAC layer below has already spoken.
    .layer(from_fn_with_state(hmac_webhooks, enforce_hmac_gate))
    .layer(from_fn_with_state(rbac_state, enforce_route_permission))
    .layer(from_fn_with_state(job_rbac_state, enforce_route_permission))
    .layer(from_fn_with_state(
        provider_route_rbac_state,
        enforce_route_permission,
    ))
    .layer(from_fn_with_state(
        sender_and_webhook_rbac_state,
        enforce_route_permission,
    ))
    .layer(from_fn_with_state(
        owner_grant_state,
        enforce_owner_grant_guard,
    ))
    .layer(from_fn(enforce_pair_scope_guard))
    .layer(IdempotencyLayer::new(idempotency_store, idempotency_ttl))
    // cratestack 0.11.0 (cratestack#846): `RateLimitLayer` defaults to
    // `StoreErrorPolicy::Allow`, right for a capacity control and wrong for a
    // security one. Set to `Deny` on both layers so the choice survives a
    // future switch away from `InMemoryRateLimitStore`.
    .layer(
        RateLimitLayer::new(rate_limit_store, rate_limit)
            .with_store_error_policy(StoreErrorPolicy::Deny),
    )
    // VSA-3b: the second door. Outside the per-principal limiter, which keys on
    // the principal this records, and inside the envelope, which has already
    // said whether the request was signed.
    .layer(from_fn_with_state(bearer_door_state, enforce_bearer_door))
    .layer(envelope)
    .layer(
        RateLimitLayer::new(source_rate_limit_store, source_rate_limit)
            .with_key_fn(source_fingerprint)
            .with_store_error_policy(StoreErrorPolicy::Deny),
    ))
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
    use cratestack::axum::http::header;

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

    fn lazy_db() -> schema::Cratestack {
        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        schema::Cratestack::builder(pool).build()
    }

    fn test_router(rate_limit: RateLimitConfig, source_rate_limit: RateLimitConfig) -> Router {
        let pepper = HashPepper::new("a".repeat(crate::pepper::MIN_PEPPER_BYTES)).unwrap();
        router(
            lazy_db(),
            Ed25519Signer::from_seed(&[1; 32]),
            pepper,
            DEFAULT_IDEMPOTENCY_TTL,
            rate_limit,
            source_rate_limit,
            HmacWebhooks::Permitted,
            ApiDoors::keypair_only(),
        )
        .expect("the envelope layer accepts its own configuration")
    }

    // Async because a lazy pool still spawns a background task on construction,
    // so `connect_lazy` panics with "this functionality requires a Tokio
    // context" outside a runtime — even though it never opens a connection.
    #[tokio::test]
    async fn router_builds_without_a_live_database() {
        let _router = test_router(
            default_rate_limit_config(),
            default_source_rate_limit_config(),
        );
    }

    async fn serve(app: Router) -> SocketAddr {
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
        addr
    }

    /// Every generated route is sealed: a plain request, with or without a
    /// bearer, is refused by the envelope with the coarse unsigned 401 before
    /// any key lookup, so no database is touched (the pool here is lazy and
    /// points nowhere).
    #[tokio::test]
    async fn a_plain_request_to_a_generated_route_is_refused_unsigned() {
        let addr = serve(test_router(
            default_rate_limit_config(),
            default_source_rate_limit_config(),
        ))
        .await;
        let client = reqwest::Client::new();
        for bearer in [None, Some("Bearer anything")] {
            let mut request = client.get(format!("http://{addr}/messages"));
            if let Some(bearer) = bearer {
                request = request.header(header::AUTHORIZATION, bearer);
            }
            let response = request.send().await.expect("sending the request");
            assert_eq!(response.status().as_u16(), 401, "bearer: {bearer:?}");
            assert_ne!(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                Some("application/cose"),
                "a refusal the layer makes itself is never sealed"
            );
            // The layer's own coarse answer: with `EnvelopeMode::Optional` the
            // request would reach the auth provider, which also says 401 but in
            // its own words, so only the body tells the modes apart.
            let body = response.text().await.expect("reading the refusal");
            assert!(
                body.contains("request could not be authenticated"),
                "bearer {bearer:?}: the envelope refused it itself, got {body}"
            );
        }
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
    fn source_fingerprint_is_unaffected_by_anything_the_caller_sends() {
        // The point of keying on the peer: nothing in the request, an
        // `Authorization` header or a forged `X-Forwarded-For` included,
        // changes the key.
        let mut a = connect_info_request("10.0.0.1:1".parse().unwrap());
        a.headers_mut()
            .insert(header::AUTHORIZATION, "Bearer forged-a".parse().unwrap());
        a.headers_mut()
            .insert("x-forwarded-for", "203.0.113.7".parse().unwrap());
        let mut b = connect_info_request("10.0.0.1:2".parse().unwrap());
        b.headers_mut()
            .insert(header::AUTHORIZATION, "Bearer forged-b".parse().unwrap());
        assert_eq!(source_fingerprint(&a), source_fingerprint(&b));
        assert_eq!(source_fingerprint(&a), "ip:10.0.0.1");
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

    /// #163's acceptance bar, kept: the coarse bucket throttles a flood
    /// against a real, bound HTTP server, not just in isolation. Every
    /// request below carries a *different* unverifiable `Authorization`
    /// value; the per-principal budget is set wide open so it can never be
    /// what throttles this test, and the source-keyed layer must throttle
    /// the requests collectively because they share one real `ConnectInfo`
    /// peer.
    #[tokio::test]
    async fn a_flood_of_unverifiable_requests_is_throttled_collectively_by_the_source_layer() {
        let addr = serve(test_router(
            RateLimitConfig::new(1000, 1000.0),
            RateLimitConfig::new(3, 0.01),
        ))
        .await;

        let client = reqwest::Client::new();
        let mut statuses = Vec::new();
        for i in 0..5 {
            let response = client
                .get(format!("http://{addr}/definitely-not-a-real-route"))
                .header(header::AUTHORIZATION, format!("Bearer forged-{i}"))
                .send()
                .await
                .expect("sending the request");
            statuses.push(response.status().as_u16());
        }

        // The bogus path never matches a generated route, and nothing signed it,
        // so a request that clears the limiter is refused by the bearer door
        // (401; it was axum's own 404 before VSA-3b, which the door now answers
        // first for every unsigned request). The first 3 (the source bucket's
        // burst) get through to it; the 4th and 5th still get 429.
        assert_eq!(
            statuses,
            vec![401, 401, 401, 429, 429],
            "varying the credential must not buy a fresh bucket from the \
             source-keyed layer, got {statuses:?}"
        );
    }

    /// The other half of that bar: one honest caller, well inside both
    /// layers' *default* production budgets, is never throttled.
    #[tokio::test]
    async fn a_legitimate_single_client_is_unaffected_by_the_source_layer_at_default_budgets() {
        let addr = serve(test_router(
            default_rate_limit_config(),
            default_source_rate_limit_config(),
        ))
        .await;

        let client = reqwest::Client::new();
        for _ in 0..10 {
            let response = client
                .get(format!("http://{addr}/definitely-not-a-real-route"))
                .send()
                .await
                .expect("sending the request");
            assert_eq!(
                response.status().as_u16(),
                401,
                "a lone honest client well inside both default budgets must \
                 never see a 429 from either layer"
            );
        }
    }
}
