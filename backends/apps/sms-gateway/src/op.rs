#![doc = include_str!("op.md")]

use std::sync::{Arc, RwLock};
use std::time::Duration;

use authkestra_axum::helpers::AxumError;
use authkestra_axum::op::axum_token_handler;
use authkestra_engine::TokenManager;
use authkestra_engine::token::jwk::Jwk;
use authkestra_op::OpStore;
use authkestra_op::config::OpConfig;
use authkestra_op::handlers::discovery::OidcDiscovery;
use authkestra_op::handlers::jwks::JwksResponse;
use axum::extract::{FromRef, State};
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::{Json, Router};
use cratestack::CratestackContext;
use sms_api::schema::Cratestack;
use sms_auth::op::MachineOnlyOpStore;

use crate::token_rate_limit::{TokenRateLimitState, enforce_token_client_rate_limit};

/// How often a running process reloads signing keys from the database —
/// short relative to `sms_auth::op::ROTATION_OVERLAP` (30 minutes), so a
/// `rotate-signing-key` run against a live server takes effect promptly
/// rather than needing a restart.
pub const DEFAULT_KEY_REFRESH_INTERVAL: Duration = Duration::from_mins(1);

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

    /// The OP's store, for [`crate::login`]'s own call into
    /// `authkestra_op::handlers::authorize::handle_authorize` — that
    /// function takes `&dyn OpStore` directly rather than going through
    /// axum's `FromRef`/`State` machinery the way the generated
    /// `/token` handler does, so this is a plain accessor rather than a
    /// second `FromRef` impl.
    pub(crate) fn store(&self) -> Arc<dyn OpStore> {
        self.store.clone()
    }

    /// The OP's config, for the same caller and the same reason as
    /// [`Self::store`].
    pub(crate) fn config(&self) -> OpConfig {
        self.config.clone()
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
    sys: CratestackContext,
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
