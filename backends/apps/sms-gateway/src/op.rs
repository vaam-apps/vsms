#![doc = include_str!("op.md")]

use std::sync::{Arc, RwLock};
use std::time::Duration;

use authkestra_axum::helpers::AxumError;
use authkestra_axum::op::axum_token_handler;
use authkestra_engine::TokenManager;
use authkestra_engine::token::jwk::Jwk;
use authkestra_op::CloneableOpStore;
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

/// What a rotation changes: the key tokens are signed with and the key set
/// `/jwks.json` publishes. They sit under two locks, so a swap is two steps, and
/// [`Self::publish`] takes them in the one order that never lets a token carry a
/// `kid` the JWKS does not advertise. Generic so a test can drive the order with
/// plain values.
struct Keyring<T, J> {
    tokens: RwLock<Arc<T>>,
    jwks: RwLock<Arc<J>>,
}

impl<T, J> Keyring<T, J> {
    fn new(tokens: Arc<T>, jwks: J) -> Self {
        Self {
            tokens: RwLock::new(tokens),
            jwks: RwLock::new(Arc::new(jwks)),
        }
    }

    fn tokens(&self) -> Arc<T> {
        self.tokens
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn jwks(&self) -> Arc<J> {
        self.jwks
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Publishes the new key set **before** the new signing key. The two writes
    /// are not one atomic step, so the order is the guarantee: between them
    /// `/jwks.json` already advertises the new `kid` while tokens are still signed
    /// with the old one (every such token verifies), whereas the other order has a
    /// window in which a token carries a `kid` the JWKS does not list yet.
    fn publish(&self, tokens: Arc<T>, jwks: J) {
        *self
            .jwks
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(jwks);
        *self
            .tokens
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = tokens;
    }
}

#[derive(Clone)]
pub struct OpState {
    store: Arc<MachineOnlyOpStore>,
    keys: Arc<Keyring<TokenManager, Vec<Jwk>>>,
    config: OpConfig,
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
            keys: Arc::new(Keyring::new(tokens, jwks)),
            config,
        }
    }

    /// Swaps in a freshly loaded key set and signing key, the key set first
    /// ([`Keyring::publish`]) — see [`spawn_key_refresh`], the only caller. Two
    /// steps, not one atomic swap.
    fn refresh(&self, tokens: Arc<TokenManager>, jwks: Vec<Jwk>) {
        self.keys.publish(tokens, jwks);
    }
}

// `Arc<dyn CloneableOpStore>`, not `Arc<dyn OpStore>` — authkestra-axum
// 0.8.0's `axum_token_handler` requires
// `Result<Arc<dyn authkestra_op::CloneableOpStore>, AxumError>: FromRef<AppState>`
// (see AGENTS.md's authkestra-0.8 section, item A2), so it can clone its
// own short-lived, mutable, per-request store handle
// (`CloneableOpStore::clone_op_store`) out of the one shared `Arc` this
// impl hands it, rather than needing `&mut` access to shared state
// itself. `MachineOnlyOpStore` (`= CompositeOpStore<...>`) satisfies
// `CloneableOpStore` via its blanket impl for any `OpStore + Clone +
// 'static` — see `SmsClientStore`'s/`SmsClientAssertionStore`'s own
// `#[derive(Clone)]` doc for why that `Clone` is correct to rely on here.
impl FromRef<OpState> for Result<Arc<dyn CloneableOpStore>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state.store.clone() as Arc<dyn CloneableOpStore>)
    }
}

impl FromRef<OpState> for Result<Arc<TokenManager>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state.keys.tokens())
    }
}

impl FromRef<OpState> for OpConfig {
    fn from_ref(state: &OpState) -> Self {
        state.config.clone()
    }
}

async fn jwks_handler(State(state): State<OpState>) -> Json<JwksResponse> {
    let jwks = state.keys.jwks();
    Json(JwksResponse::new((*jwks).clone()))
}

/// Builds the discovery document with `private_key_jwt` advertised in
/// `token_endpoint_auth_methods_supported` — see this module's own doc for
/// why `authkestra_axum::op::axum_discovery_handler` can't do this itself.
async fn discovery_handler(State(state): State<OpState>) -> Json<serde_json::Value> {
    Json(
        serde_json::to_value(OidcDiscovery::from_config(&state.config).with_private_key_jwt())
            .unwrap_or_default(),
    )
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
/// `interval` is `op.signing_key.refresh_interval_secs` of the configuration
/// (`sms_config::SigningKey`); a test passes a short one and observes a refresh
/// without waiting a full production cycle.
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::Keyring;

    /// The new key set is visible while the signing key is still the old one.
    /// Deterministic, no race harness: the test holds the signing-key lock, so
    /// `publish` can finish its first write and no more, and the test reads what
    /// that first write was. In the other order `publish` blocks on the held lock
    /// before touching the JWKS, which then never changes.
    #[test]
    fn the_key_set_is_published_before_the_signing_key() {
        let keyring = std::sync::Arc::new(Keyring::new(std::sync::Arc::new("old"), vec!["old"]));
        let (blocked_tx, blocked_rx) = mpsc::channel();
        let held = keyring.tokens.write().expect("lock");
        let publisher = {
            let keyring = keyring.clone();
            std::thread::spawn(move || {
                keyring.publish(std::sync::Arc::new("new"), vec!["old", "new"]);
                blocked_tx.send(()).ok();
            })
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while keyring.jwks().len() != 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            keyring.jwks().len(),
            2,
            "the new key set must be published first, while the signing key is still held back"
        );
        assert!(
            blocked_rx.try_recv().is_err(),
            "the signing key is still the old one"
        );
        drop(held);
        publisher.join().expect("publish finishes");
        assert_eq!(*keyring.tokens(), "new");
        assert_eq!(*keyring.jwks(), vec!["old", "new"]);
    }
}
