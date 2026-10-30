#![doc = include_str!("op.md")]

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use authkestra_axum::helpers::AxumError;
use authkestra_axum::op::axum_token_handler;
use authkestra_engine::TokenManager;
use authkestra_engine::token::jwk::Jwk;
use authkestra_op::config::OpConfig;
use authkestra_op::handlers::discovery::OidcDiscovery;
use authkestra_op::handlers::jwks::JwksResponse;
use authkestra_op::{CloneableOpStore, OpStore};
use axum::extract::{FromRef, State};
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::{Json, Router};
use cratestack::CratestackContext;
use jsonwebtoken::DecodingKey;
use sms_api::schema::Cratestack;
use sms_auth::op::{OpStore as ProviderStore, ServedGrants};

use crate::grant_gate::enforce_served_grants;
use crate::token_rate_limit::{TokenRateLimitState, enforce_token_client_rate_limit};

/// What a rotation changes: the key tokens are signed with and the key set
/// `/jwks.json` publishes. They sit under two locks, so a swap is two steps, and
/// [`Self::publish`] takes them in the one order that never lets a token carry a
/// `kid` the JWKS does not advertise. Generic so a test can drive the order with
/// plain values.
pub(crate) struct Keyring<T, J> {
    tokens: RwLock<Arc<T>>,
    jwks: RwLock<Arc<J>>,
}

impl<T, J> Keyring<T, J> {
    pub(crate) fn new(tokens: Arc<T>, jwks: J) -> Self {
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

/// The public half of the OP's signing keys, in the two forms something asks
/// for: the JWKS `/jwks.json` publishes, and the same keys ready to verify with.
///
/// One value under the keyring's one lock, so the key a token is verified with
/// and the key the JWKS advertises are the same key set, published at the same
/// instant, and [`Keyring::publish`]'s order (key set first) holds for the
/// bearer door exactly as it does for a remote verifier reading `/jwks.json`:
/// no token can carry a `kid` the door has not got yet.
pub(crate) struct PublishedKeys {
    jwks: Vec<Jwk>,
    verify: HashMap<String, Arc<DecodingKey>>,
}

impl PublishedKeys {
    /// Builds the verification keys from `jwks`. A key with no `kid` or no RSA
    /// components is published (the JWKS is what it was) but cannot verify,
    /// which is what a token naming it deserves.
    pub(crate) fn new(jwks: Vec<Jwk>) -> Self {
        let verify = jwks
            .iter()
            .filter_map(|jwk| {
                let kid = jwk.kid.clone()?;
                let key = DecodingKey::from_rsa_components(jwk.n.as_deref()?, jwk.e.as_deref()?)
                    .inspect_err(|error| {
                        tracing::warn!(%kid, %error, "an OP signing key cannot verify tokens");
                    })
                    .ok()?;
                Some((kid, Arc::new(key)))
            })
            .collect();
        Self { jwks, verify }
    }

    /// The key a token whose header names `kid` is verified with.
    pub(crate) fn decoding_key(&self, kid: &str) -> Option<Arc<DecodingKey>> {
        self.verify.get(kid).cloned()
    }
}

/// What a rotation changes, shared with the bearer door: see [`Keyring`].
pub(crate) type OpKeyring = Keyring<TokenManager, PublishedKeys>;

impl OpKeyring {
    /// Swaps in a freshly loaded key set and signing key, the key set first
    /// ([`Keyring::publish`]).
    pub(crate) fn refresh(&self, tokens: Arc<TokenManager>, jwks: Vec<Jwk>) {
        self.publish(tokens, PublishedKeys::new(jwks));
    }

    /// The key a token naming `kid` is verified with.
    pub(crate) fn decoding_key(&self, kid: &str) -> Option<Arc<DecodingKey>> {
        self.jwks().decoding_key(kid)
    }
}

#[derive(Clone)]
pub struct OpState {
    store: Arc<ProviderStore>,
    keys: Arc<OpKeyring>,
    config: OpConfig,
    /// The gateway's own `/login/sso/start` URL, advertised in discovery when
    /// `human_login.sso` is served - see [`discovery_document`].
    sso_login_endpoint: Option<Arc<str>>,
}

impl OpState {
    #[must_use]
    pub fn new(
        store: ProviderStore,
        tokens: Arc<TokenManager>,
        config: OpConfig,
        jwks: Vec<Jwk>,
    ) -> Self {
        Self {
            store: Arc::new(store),
            keys: Arc::new(Keyring::new(tokens, PublishedKeys::new(jwks))),
            config,
            sso_login_endpoint: None,
        }
    }

    /// Advertises `endpoint` (the public `/login/sso/start` URL) in the discovery
    /// document, which is how the console learns that single sign-on exists and
    /// where to send the browser. `None` advertises nothing. Set by `serve` only
    /// when `human_login.sso` is served.
    #[must_use]
    pub fn with_sso_login_endpoint(mut self, endpoint: Option<String>) -> Self {
        self.sso_login_endpoint = endpoint.map(Arc::from);
        self
    }

    /// The OP's store, for [`crate::login`]'s own call into
    /// `authkestra_op::handlers::authorize::handle_authorize`: that function takes
    /// `&mut dyn OpStore` directly rather than going through axum's `FromRef`
    /// machinery the way the generated `/token` handler does, so this is a plain
    /// accessor.
    ///
    /// Returns an owned `Box<dyn OpStore>`, not an `Arc`: `handle_authorize` needs
    /// `&mut`, and an `Arc` only ever gives shared access. The clone is cheap and
    /// correct precisely because `ProviderStore`'s `Clone` shares state (the
    /// `Arc<Cratestack>`, and the `Arc` inside the authorization-code
    /// `MemoryStore`): **the code `handle_authorize` stores through this clone is
    /// the one the `/token` handler's own clone consumes**, which is what lets a
    /// code issued by `POST /login` be redeemed at `/token`.
    pub(crate) fn store(&self) -> Box<dyn OpStore> {
        Box::new((*self.store).clone())
    }

    /// The OP's config, for the same caller and the same reason as
    /// [`Self::store`].
    pub(crate) fn config(&self) -> OpConfig {
        self.config.clone()
    }

    /// Swaps in a freshly loaded key set and signing key, the key set first
    /// ([`Keyring::publish`]) — see [`spawn_key_refresh`], the only caller. Two
    /// steps, not one atomic swap.
    fn refresh(&self, tokens: Arc<TokenManager>, jwks: Vec<Jwk>) {
        self.keys.refresh(tokens, jwks);
    }

    /// The keyring the bearer door verifies against: the very one `/jwks.json`
    /// publishes from, so the two cannot disagree about which keys exist.
    pub(crate) fn keyring(&self) -> Arc<OpKeyring> {
        self.keys.clone()
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
    let published = state.keys.jwks();
    Json(JwksResponse::new(published.jwks.clone()))
}

/// Builds the discovery document with `private_key_jwt` advertised in
/// `token_endpoint_auth_methods_supported`, and **without** an
/// `authorization_endpoint` — see this module's own doc for why
/// `authkestra_axum::op::axum_discovery_handler` can't do either itself.
///
/// `grant_types_supported` is exactly the grants served ([`ServedGrants`]), and
/// `code_challenge_methods_supported` (`["S256"]`) is present exactly when
/// `authorization_code` is. `from_config` adds an `authorization_endpoint` for any
/// provider that lists that grant, but `GET /authorize` is **never mounted** here
/// (the console reaches `handle_authorize` through `POST /login`), and a
/// discovery document that points a client at a route that 404s is worse than one
/// that says nothing.
async fn discovery_handler(State(state): State<OpState>) -> Json<serde_json::Value> {
    Json(discovery_document(
        &state.config,
        state.sso_login_endpoint.as_deref(),
    ))
}

/// The discovery document for `config`: see [`discovery_handler`].
///
/// **`vsms_sso_login_endpoint`** is a vendor extension member (OIDC Discovery
/// permits additional members), present only when single sign-on is served: the
/// absolute URL of `GET /login/sso/start`, built from `op.issuer`. It is how the
/// console learns, from the one authority that knows, both *whether* to offer
/// "Sign in with ..." and *where* to send the browser - no console setting to
/// drift out of step with the gateway's.
pub(crate) fn discovery_document(
    config: &OpConfig,
    sso_login_endpoint: Option<&str>,
) -> serde_json::Value {
    let mut document = OidcDiscovery::from_config(config).with_private_key_jwt();
    document.authorization_endpoint = None;
    let mut document = serde_json::to_value(document).unwrap_or_default();
    if let (Some(endpoint), Some(object)) = (sso_login_endpoint, document.as_object_mut()) {
        object.insert(
            "vsms_sso_login_endpoint".to_owned(),
            serde_json::Value::String(endpoint.to_owned()),
        );
    }
    document
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
pub fn router(
    state: OpState,
    token_rate_limit: TokenRateLimitState,
    served: ServedGrants,
) -> Router {
    // Layers wrap from the inside out: the grant gate is added last so it is the
    // **outermost**, and a request for a grant this deployment does not serve is
    // refused before it takes a token from any `client_id`'s bucket.
    let token_route = Router::new()
        .route("/token", post(axum_token_handler::<OpState>))
        .layer(from_fn_with_state(
            token_rate_limit,
            enforce_token_client_rate_limit,
        ))
        .layer(from_fn_with_state(served, enforce_served_grants));

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

    use sms_auth::op::{ServedGrants, op_config};

    use super::{Keyring, discovery_document};

    fn document(machine_tokens: bool, human_login: bool) -> serde_json::Value {
        discovery_document(
            &op_config(
                "https://sms.example.org".to_owned(),
                ServedGrants {
                    machine_tokens,
                    human_login,
                },
            ),
            None,
        )
    }

    /// The extension member is there exactly when single sign-on is served.
    #[test]
    fn discovery_names_the_sso_start_url_only_when_it_is_given_one() {
        let config = op_config(
            "https://sms.example.org".to_owned(),
            ServedGrants {
                machine_tokens: false,
                human_login: true,
            },
        );
        let without = discovery_document(&config, None);
        assert!(
            without.get("vsms_sso_login_endpoint").is_none(),
            "{without}"
        );
        let with = discovery_document(&config, Some("https://sms.example.org/login/sso/start"));
        assert_eq!(
            with["vsms_sso_login_endpoint"],
            "https://sms.example.org/login/sso/start"
        );
    }

    /// Exactly the grants served, never the refresh grant, and **no
    /// `authorization_endpoint`** even when `authorization_code` is served:
    /// `GET /authorize` is not a route here, and a document that points a client at
    /// one that 404s is worse than silence.
    #[test]
    fn discovery_lists_exactly_the_served_grants_and_no_authorization_endpoint() {
        let both = document(true, true);
        assert_eq!(
            both["grant_types_supported"],
            serde_json::json!(["client_credentials", "authorization_code"])
        );
        assert_eq!(
            both["code_challenge_methods_supported"],
            serde_json::json!(["S256"])
        );
        assert!(both.get("authorization_endpoint").is_none(), "{both}");
        assert_eq!(
            both["response_types_supported"],
            serde_json::json!(["code"])
        );

        let machines = document(true, false);
        assert_eq!(
            machines["grant_types_supported"],
            serde_json::json!(["client_credentials"])
        );
        assert!(
            machines.get("code_challenge_methods_supported").is_none(),
            "no code grant, no PKCE method to advertise: {machines}"
        );
        assert_eq!(machines["response_types_supported"], serde_json::json!([]));

        let people = document(false, true);
        assert_eq!(
            people["grant_types_supported"],
            serde_json::json!(["authorization_code"])
        );
        assert_eq!(
            people["scopes_supported"],
            serde_json::json!(["openid", "profile"])
        );
        for doc in [&both, &machines, &people] {
            let grants = doc["grant_types_supported"].to_string();
            assert!(!grants.contains("refresh_token"), "{grants}");
            assert!(
                doc["token_endpoint_auth_methods_supported"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m == "private_key_jwt"),
                "{doc}"
            );
        }
    }

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
