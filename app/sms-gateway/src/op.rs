//! Mounting the OP's HTTP surface. #20.
//!
//! Hand-wired to exactly the three routes a `client_credentials` +
//! `private_key_jwt` deployment needs — `/jwks.json`,
//! `/.well-known/openid-configuration`, `/token` — rather than
//! `authkestra_axum::op::OpExt::op_axum_router()`, which mounts
//! `/authorize`, `/device_authorization`, `/userinfo` and `/device/verify`
//! too. Those need `SessionStore`/`SessionConfig` wiring for flows nothing
//! in this deployment uses yet (see `sms_auth::op`'s own module doc); this
//! keeps the served surface matching what's actually implemented instead
//! of exposing routes that would always fail.
//!
//! Two places this deviates from the crate's own handlers, both because the
//! ready-made handler has no hook for what this deployment needs to do
//! differently:
//!
//! - `authkestra_axum::op::axum_jwks_handler` publishes exactly one key
//!   (whatever single `Arc<TokenManager>` the state carries) — it cannot
//!   serve an overlap-window JWKS with both the active and a still-valid
//!   previous key. [`jwks_handler`] below builds the response from the full
//!   key list `sms_auth::op::load_signing_keys` already computed instead.
//! - `authkestra_axum::op::axum_discovery_handler` builds the discovery
//!   document straight from `OidcDiscovery::from_config`, with no way to
//!   chain `.with_private_key_jwt()` onto the result — so a spec-compliant
//!   client consulting discovery would never learn this OP accepts
//!   `private_key_jwt` (#18). [`discovery_handler`] below calls it.
//!
//! **The signing key and JWKS are live-refreshed, not a startup
//! snapshot.** Found in review (#97): the first version of this module
//! captured both once at construction, so `rotate-signing-key` run against
//! an already-running server updated the database but not the process —
//! new tokens kept signing with the old key indefinitely, and `/jwks.json`
//! never gained the new one, until a restart. That defeats the point of
//! rotation (a suspected-compromised key would keep signing) and the
//! entire 30-minute overlap window (`sms_auth::op::ROTATION_OVERLAP`) was
//! only ever exercised at process start, never on a live server.
//! [`spawn_key_refresh`] closes this: a background poll reloads and
//! atomically swaps both.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use authkestra_axum::helpers::AxumError;
use authkestra_axum::op::axum_token_handler;
use authkestra_engine::token::jwk::Jwk;
use authkestra_engine::TokenManager;
use authkestra_op::config::OpConfig;
use authkestra_op::handlers::discovery::OidcDiscovery;
use authkestra_op::handlers::jwks::JwksResponse;
use authkestra_op::OpStore;
use axum::extract::{FromRef, State};
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::{Json, Router};
use cratestack::CoolContext;
use sms_api::schema::Cratestack;
use sms_auth::op::MachineOnlyOpStore;

use crate::token_rate_limit::{enforce_token_client_rate_limit, TokenRateLimitState};

/// How often a running process reloads signing keys from the database —
/// short relative to `sms_auth::op::ROTATION_OVERLAP` (30 minutes), so a
/// `rotate-signing-key` run against a live server takes effect promptly
/// rather than needing a restart.
pub const DEFAULT_KEY_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct OpState {
    store: Arc<MachineOnlyOpStore>,
    tokens: Arc<RwLock<Arc<TokenManager>>>,
    config: OpConfig,
    jwks: Arc<RwLock<Arc<Vec<Jwk>>>>,
}

impl OpState {
    #[must_use]
    pub fn new(
        store: MachineOnlyOpStore,
        tokens: Arc<TokenManager>,
        config: OpConfig,
        jwks: Vec<Jwk>,
    ) -> Self {
        Self {
            store: Arc::new(store),
            tokens: Arc::new(RwLock::new(tokens)),
            config,
            jwks: Arc::new(RwLock::new(Arc::new(jwks))),
        }
    }

    /// Atomically swaps in a freshly loaded signing key and key set — see
    /// [`spawn_key_refresh`], the only caller.
    fn refresh(&self, tokens: Arc<TokenManager>, jwks: Vec<Jwk>) {
        *self
            .tokens
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = tokens;
        *self
            .jwks
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(jwks);
    }
}

impl FromRef<OpState> for Result<Arc<dyn OpStore>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state.store.clone() as Arc<dyn OpStore>)
    }
}

impl FromRef<OpState> for Result<Arc<TokenManager>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state
            .tokens
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }
}

impl FromRef<OpState> for OpConfig {
    fn from_ref(state: &OpState) -> Self {
        state.config.clone()
    }
}

async fn jwks_handler(State(state): State<OpState>) -> Json<JwksResponse> {
    let jwks = state
        .jwks
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    Json(JwksResponse::new((*jwks).clone()))
}

/// Builds the discovery document with `private_key_jwt` advertised in
/// `token_endpoint_auth_methods_supported` — see this module's own doc for
/// why `authkestra_axum::op::axum_discovery_handler` can't do this itself.
async fn discovery_handler(State(state): State<OpState>) -> Json<OidcDiscovery> {
    Json(OidcDiscovery::from_config(&state.config).with_private_key_jwt())
}

/// The OP's routes, already `.with_state(...)` — mergeable directly with
/// `sms_api::router`'s own already-stated `Router`.
///
/// `token_rate_limit` (#168) wraps **only** `/token`, never `/jwks.json` or
/// discovery — those are cacheable, unauthenticated, read-only documents a
/// client (and this module's own `spawn_key_refresh` machinery, and every
/// `/token` caller's own discovery lookup) may fetch far more often than
/// any real token request, and throttling them buys nothing `token_per_ip`/
/// `token_global` at the Caddy edge don't already cover for `/token`
/// itself. Built as its own small sub-router with the layer applied before
/// `.merge()`, the standard axum pattern for "this middleware applies to
/// one route, not every route this function mounts" — see
/// `token_rate_limit`'s own module doc for what the layer does and why it
/// lives here rather than in `sms_api::router` (which never wraps `/token`
/// at all) or `deploy/Caddyfile` (which structurally cannot key on
/// `client_id` — that module's doc has the receipts).
// No `#[must_use]`: axum's `Router` already carries one, and doubling it is
// what `clippy::double_must_use` objects to — same reasoning as
// `sms_api::router`'s own doc comment on this.
pub fn router(state: OpState, token_rate_limit: TokenRateLimitState) -> Router {
    let token_route = Router::new()
        .route("/token", post(axum_token_handler::<OpState>))
        .layer(from_fn_with_state(
            token_rate_limit,
            enforce_token_client_rate_limit,
        ));

    Router::new()
        .route("/jwks.json", get(jwks_handler))
        .route("/.well-known/openid-configuration", get(discovery_handler))
        .merge(token_route)
        .with_state(state)
}

/// Periodically reloads signing keys from `db` and swaps them into `state`
/// — see this module's own doc for why. Never returns; the caller drops
/// the spawned task on shutdown, the same convention every other
/// background loop in this workspace follows (`sms-worker`'s roles, most
/// directly).
///
/// `interval` is a parameter rather than always
/// [`DEFAULT_KEY_REFRESH_INTERVAL`] so a test can use a short one and
/// observe a refresh without waiting a full production cycle.
pub fn spawn_key_refresh(
    state: OpState,
    db: Cratestack,
    sys: CoolContext,
    issuer: String,
    interval: Duration,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            match sms_auth::op::load_signing_keys(&db, &sys, &issuer).await {
                Ok((tokens, jwks)) => state.refresh(tokens, jwks),
                Err(error) => {
                    tracing::error!(
                        %error,
                        "reloading OP signing keys failed; continuing to sign with the \
                         previously loaded key"
                    );
                }
            }
        }
    });
}
