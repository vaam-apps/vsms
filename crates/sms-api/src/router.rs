//! Assembling the generated router.

use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use cratestack::axum::extract::Request;
use cratestack::axum::http::{header, Method};
use cratestack::axum::middleware::from_fn_with_state;
use cratestack::axum::Router;
use cratestack::idempotency::{IdempotencyLayer, IdempotencyStore};
use cratestack::ratelimit::{
    InMemoryRateLimitStore, RateLimitConfig, RateLimitLayer, RateLimitStore,
};
use cratestack::SqlxIdempotencyStore;
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

/// #153: the TTL a cached `Idempotency-Key` response stays replayable —
/// matches `docs/architecture.md` §4.5's own figure, 24 hours. Exposed so
/// every construction site (the real `sms-gateway serve` command and the
/// live HTTP test suites in `crates/sms-auth/tests/`) shares one source of
/// truth instead of hand-copying the duration; `serve` still lets an
/// operator override it via `--idempotency-ttl-secs`/`SMS_IDEMPOTENCY_TTL_SECS`.
pub const DEFAULT_IDEMPOTENCY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

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
/// exists to guarantee. The same reasoning weakens `RateLimitLayer`
/// equally: a caller minting a fresh token per request gets a fresh
/// bucket, hence an effectively unbounded quota, defeating "stop a buggy
/// caller flooding you" (§4.6) precisely for the caller shape most likely
/// to need throttling.
///
/// The fix: partition on the token's own `sub` claim instead — the
/// `client_id` for a service account (`auth.rs`'s own doc: "a `client_id`
/// for services"), which `GatewayAuth::authenticate` already treats as
/// this deployment's stable caller identity, and which stays the same
/// across a token refresh.
///
/// Deliberately reads the JWT **without verifying its signature** — that
/// is `GatewayAuth`'s job, running later and deeper in the same request,
/// against the OP's real JWKS. A forged `sub` here only lets an attacker
/// pick their *own* rate-limit bucket / idempotency namespace; it can
/// never borrow or collide with another caller's, since nothing
/// downstream trusts this value for anything but partitioning. That is
/// the identical trust boundary the upstream default (an unverified
/// header hash) already sits behind — this only changes *which* bytes of
/// an equally-unverified request get hashed.
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
/// call is what a request meets *first*): `RateLimitLayer` is outermost,
/// rejecting an abusive caller with a cheap `429` before any request body
/// is buffered or the idempotency store is touched; `IdempotencyLayer` is
/// next, reserving/replaying against `cratestack_idempotency`; the RBAC
/// layer and the generated router (whose own extractors run
/// [`GatewayAuth`]) sit innermost.
///
/// `SqlxIdempotencyStore` and `InMemoryRateLimitStore` are each the *only*
/// store either upstream crate ships (`docs/architecture.md` §4.5/§4.6) —
/// there is no second implementation to choose between, so `router()`
/// constructs both internally from `db`'s own pool rather than taking them
/// as parameters. `InMemoryRateLimitStore` means the rate limit is
/// per-process: correct for this deployment's single gateway replica
/// (`deploy/docker-compose.yml`), and explicitly not cluster-wide — a
/// multi-replica deployment would need a Redis/Postgres-backed
/// `RateLimitStore`, which does not exist yet (§4.6).
///
/// The `cratestack_idempotency` table itself is **not** created here —
/// `SqlxIdempotencyStore::ensure_schema()` is deliberately never called by
/// this binary. See `deploy/migrate.sql`'s own header for why: creating it
/// is treated as migration-shaped bookkeeping owned by the one-shot
/// migrate job, the same way `schema_migrations` itself is, rather than
/// DDL the serving process runs (and needs privilege for) at every start.
///
/// Both layers use [`client_id_fingerprint`], not either upstream crate's
/// own default (a raw-`Authorization`-header hash) — see that function's
/// own doc for the live-verified reason the default breaks idempotency
/// replay across a token refresh, and weakens rate limiting the same way.
// No `#[must_use]`: axum's `Router` already carries one, and doubling it is
// what `clippy::double_must_use` objects to.
pub fn router(
    db: schema::Cratestack,
    auth: GatewayAuth,
    pepper: HashPepper,
    idempotency_ttl: Duration,
    rate_limit: RateLimitConfig,
) -> Router {
    let rbac_state = RbacState {
        auth: auth.clone(),
        requirements: PROVIDER_WRITE_ROUTES,
    };
    let idempotency_store: Arc<dyn IdempotencyStore> =
        Arc::new(SqlxIdempotencyStore::new(db.pool().clone()));
    let rate_limit_store: Arc<dyn RateLimitStore> = Arc::new(InMemoryRateLimitStore::new());

    schema::axum::router(db, Procedures::new(pepper), JsonCodec, auth)
        .layer(from_fn_with_state(rbac_state, enforce_route_permission))
        .layer(
            IdempotencyLayer::new(idempotency_store, idempotency_ttl)
                .with_principal_fingerprint(client_id_fingerprint),
        )
        .layer(RateLimitLayer::new(rate_limit_store, rate_limit).with_key_fn(client_id_fingerprint))
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
}
