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
//! The one place this deviates from the crate's own handlers:
//! `authkestra_axum::op::axum_jwks_handler` publishes exactly one key
//! (whatever single `Arc<TokenManager>` the state carries) — it cannot
//! serve an overlap-window JWKS with both the active and a still-valid
//! previous key. [`jwks_handler`] below builds the response from the full
//! key list `sms_auth::op::load_signing_keys` already computed instead.

use std::sync::Arc;

use authkestra_axum::helpers::AxumError;
use authkestra_axum::op::{axum_discovery_handler, axum_token_handler};
use authkestra_engine::token::jwk::Jwk;
use authkestra_engine::TokenManager;
use authkestra_op::config::OpConfig;
use authkestra_op::handlers::jwks::JwksResponse;
use authkestra_op::OpStore;
use axum::extract::{FromRef, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use sms_auth::op::MachineOnlyOpStore;

#[derive(Clone)]
pub struct OpState {
    store: Arc<MachineOnlyOpStore>,
    tokens: Arc<TokenManager>,
    config: OpConfig,
    jwks: Vec<Jwk>,
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
            tokens,
            config,
            jwks,
        }
    }
}

impl FromRef<OpState> for Result<Arc<dyn OpStore>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state.store.clone() as Arc<dyn OpStore>)
    }
}

impl FromRef<OpState> for Result<Arc<TokenManager>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state.tokens.clone())
    }
}

impl FromRef<OpState> for OpConfig {
    fn from_ref(state: &OpState) -> Self {
        state.config.clone()
    }
}

async fn jwks_handler(State(state): State<OpState>) -> Json<JwksResponse> {
    Json(JwksResponse::new(state.jwks.clone()))
}

/// The OP's routes, already `.with_state(...)` — mergeable directly with
/// `sms_api::router`'s own already-stated `Router`.
// No `#[must_use]`: axum's `Router` already carries one, and doubling it is
// what `clippy::double_must_use` objects to — same reasoning as
// `sms_api::router`'s own doc comment on this.
pub fn router(state: OpState) -> Router {
    Router::new()
        .route("/jwks.json", get(jwks_handler))
        .route(
            "/.well-known/openid-configuration",
            get(axum_discovery_handler::<OpState>),
        )
        .route("/token", post(axum_token_handler::<OpState>))
        .with_state(state)
}
