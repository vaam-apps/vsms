#![doc = include_str!("login.md")]

use std::net::SocketAddr;
use std::sync::Arc;

use authkestra_engine::auth::state::Identity;
use authkestra_op::handlers::authorize::{AuthorizeOutcome, AuthorizeRequest, handle_authorize};
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use cratestack::ratelimit::{
    InMemoryRateLimitStore, RateLimitConfig, RateLimitDecision, RateLimitStore,
};
use serde::{Deserialize, Serialize};
use sms_api::schema::Cratestack;
use sms_auth::login::{LoginError, authenticate_user};

use crate::op::OpState;

/// A sign-in body is a few hundred bytes. The cap is checked before any password
/// is hashed, so an oversized body costs the reader and nothing more.
const MAX_LOGIN_BODY_BYTES: usize = 16 * 1024;

/// The longest email the rate-limit key keeps. A longer one is truncated in the
/// *key* only (the account lookup uses the whole value and finds nothing), so an
/// attacker cannot make the limiter store one huge key per request.
const MAX_EMAIL_KEY_BYTES: usize = 254;

/// The `provider_id` of the identity a code carries.
const PROVIDER_ID: &str = "sms-console";

#[derive(Clone)]
struct LoginState {
    db: Cratestack,
    op: OpState,
    /// `human_login.console_client.client_id`: the one id `POST /login` accepts.
    console_client_id: Arc<str>,
    limiter: Limiter,
}

/// The two buckets in front of the password check: one per email address and one
/// per source address, both from `human_login.login_rate_limit`.
#[derive(Clone)]
pub struct Limiter {
    store: Arc<dyn RateLimitStore>,
    config: RateLimitConfig,
}

impl Limiter {
    /// A limiter with its own in-memory store (bounded and swept: cratestack's).
    /// One `serve` replica, like the codes the sign-in issues.
    #[must_use]
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            store: Arc::new(InMemoryRateLimitStore::new()),
            config,
        }
    }

    /// Takes one token from `source`'s bucket, then one from `email`'s.
    async fn admit(&self, source: &str, email: &str) -> Admission {
        for key in [source_key(source), email_key(email)] {
            match self.store.consume(&key, self.config).await {
                Ok(RateLimitDecision::Allowed { .. }) => {}
                Ok(RateLimitDecision::Throttled { retry_after_secs }) => {
                    return Admission::Throttled { retry_after_secs };
                }
                Err(error) => {
                    // Fails closed, like every limiter here: a store that cannot
                    // answer admits nothing, so the password is never hashed.
                    tracing::warn!(%error, "login rate limit store error");
                    return Admission::Unavailable;
                }
            }
        }
        Admission::Allowed
    }
}

enum Admission {
    Allowed,
    Throttled { retry_after_secs: u32 },
    Unavailable,
}

/// The source bucket's key: the real TCP peer, as `sms_api::router`'s own
/// source limiter keys (a forwarded-for header is attacker-controlled unless a
/// proxy is trusted, and none is configured here). No peer at all (a server not
/// built `with_connect_info`) shares one `unverified` bucket, which is stricter,
/// not looser.
fn source_key(source: &str) -> String {
    format!("login-source:{source}")
}

/// The email bucket's key. Trimmed and lowercased so `Ana@x` and `ana@x ` share
/// one bucket, and capped (see [`MAX_EMAIL_KEY_BYTES`]).
fn email_key(email: &str) -> String {
    let mut normalised = email.trim().to_lowercase();
    if normalised.len() > MAX_EMAIL_KEY_BYTES {
        let mut end = MAX_EMAIL_KEY_BYTES;
        while !normalised.is_char_boundary(end) {
            end -= 1;
        }
        normalised.truncate(end);
    }
    format!("login-email:{normalised}")
}

/// The sign-in request: credentials and the authorization request, in one body.
///
/// `snake_case`, the OAuth parameter names themselves (`client_id`,
/// `redirect_uri`, `code_challenge`, ...), pinned by a test. (`29f93c4^` used
/// camelCase here, for a console that no longer exists; the repository's rule is
/// that a shape it owns is `snake_case` on the wire.)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct LoginRequest {
    email: String,
    password: String,
    client_id: String,
    redirect_uri: String,
    response_type: String,
    scope: String,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
    nonce: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct LoginResponse {
    /// Where the console sends the browser next: `{redirect_uri}?code=...&state=...`.
    /// Never followed by this process: this is a JSON API.
    redirect: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct LoginFailure {
    error: &'static str,
}

/// `AuthorizeRequest` is `#[non_exhaustive]` (authkestra-op 0.8.0), so it cannot
/// be built with a struct literal from outside the crate, and has no constructor;
/// it derives `Deserialize`, so the wire shape is built by hand and deserialised.
/// A future release that renames a field would not fail this for an `Option`
/// field (it would silently become `None`): `the_authorize_request_keeps_its_optional_fields`
/// below and the live suite's `state`/`nonce` round trip are what catch that.
fn build_authorize_request(request: &LoginRequest) -> Result<AuthorizeRequest, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "client_id": request.client_id,
        "redirect_uri": request.redirect_uri,
        "response_type": request.response_type,
        "scope": request.scope,
        "state": request.state,
        "code_challenge": request.code_challenge,
        "code_challenge_method": request.code_challenge_method,
        "nonce": request.nonce,
    }))
}

/// A deliberately narrow, fixed error vocabulary: never `OpError`'s `Display` and
/// never [`LoginError`]'s message. This route sits in front of a password check,
/// and why a sign-in failed is exactly what it must not say.
fn failure(status: StatusCode, error: &'static str) -> Response {
    let mut response = (status, Json(LoginFailure { error })).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The one answer to every refusal of the person or the attempt: an unknown
/// account, a wrong password, an inactive or deleted account, no password set, a
/// broken role, **and a `client_id` that is not the console's**.
fn refused() -> Response {
    failure(StatusCode::UNAUTHORIZED, "invalid_credentials")
}

async fn login_handler(
    State(state): State<LoginState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    body: Result<Json<LoginRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return failure(StatusCode::BAD_REQUEST, "invalid_request");
    };

    // Not the console: the one client this route serves. Before the limiter, so a
    // caller with another id cannot spend anyone's email bucket, and the same
    // answer as a wrong password, so it says nothing about what ids exist.
    if request.client_id != *state.console_client_id {
        return refused();
    }

    // Before any Argon2 work: the limiter exists to bound how many password
    // hashes a caller can make this process run.
    let source = peer.map_or_else(
        || "unverified".to_owned(),
        |Extension(ConnectInfo(addr))| format!("ip:{}", addr.ip()),
    );
    match state.limiter.admit(&source, &request.email).await {
        Admission::Allowed => {}
        Admission::Throttled { retry_after_secs } => {
            let mut response = failure(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
            if let Ok(value) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            return response;
        }
        Admission::Unavailable => {
            return failure(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
        }
    }

    let authenticated = match authenticate_user(&state.db, &request.email, &request.password).await
    {
        Ok(authenticated) => authenticated,
        Err(LoginError::InvalidCredentials) => return refused(),
        Err(error @ LoginError::RoleNotFound { .. }) => {
            // An operator-visible data-integrity problem, not a credentials one:
            // logged with its detail, answered exactly as a wrong password.
            tracing::error!(%error, "login refused for a correct password");
            return refused();
        }
        Err(LoginError::Unavailable(error)) => {
            tracing::error!(%error, "login: the account could not be read");
            return failure(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
        }
    };

    // Only what the code must carry to survive to token issuance. Role and
    // permissions are deliberately NOT stashed in `attributes`: the API resolves
    // them from rows per request.
    let identity = Identity {
        provider_id: PROVIDER_ID.to_owned(),
        external_id: authenticated.subject,
        email: Some(authenticated.email),
        username: Some(authenticated.display_name),
        attributes: std::collections::HashMap::new(),
    };

    let authorize_request = match build_authorize_request(&request) {
        Ok(authorize_request) => authorize_request,
        Err(error) => {
            tracing::error!(%error, "login: building the AuthorizeRequest failed");
            return failure(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
        }
    };

    // The real `handle_authorize`: PKCE (S256 only), exact `redirect_uri` match,
    // `response_type`, scope and grant admission, and the code itself are the
    // library's, unmodified. This route's one job is deciding who the caller is.
    let config = state.op.config();
    let mut store = state.op.store();
    match handle_authorize(authorize_request, identity, &config, store.as_mut()).await {
        AuthorizeOutcome::Redirect(url) if redirect_carries_an_error(&url) => {
            // The library answers a request it refused (a method other than S256,
            // a scope the client may not hold, a response type that is not
            // `code`) with a redirect to the client carrying `error=`. The
            // console must never be handed that as though it were a sign-in.
            tracing::warn!("login: handle_authorize refused the request");
            failure(StatusCode::BAD_REQUEST, "invalid_request")
        }
        AuthorizeOutcome::Redirect(redirect) => {
            let mut response = (StatusCode::OK, Json(LoginResponse { redirect })).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        // A `redirect_uri` the console client is not registered with, or a client
        // that does not exist: never redirected to, never echoed.
        AuthorizeOutcome::DirectError(error) => {
            tracing::warn!(%error, "login: handle_authorize refused the request directly");
            failure(StatusCode::BAD_REQUEST, "invalid_request")
        }
    }
}

/// Whether `redirect` is the library's error redirect (`?error=...`) and not a
/// code.
fn redirect_carries_an_error(redirect: &str) -> bool {
    match url::Url::parse(redirect) {
        Ok(url) => {
            let has = |name: &str| url.query_pairs().any(|(key, _)| key == name);
            has("error") || !has("code")
        }
        // Not a URL at all is certainly not a code.
        Err(_) => true,
    }
}

/// `POST /login`, already `.with_state(...)`: mergeable directly with the rest of
/// `serve`'s routers, outside the envelope layer (a person signing in has no key
/// to sign with yet; proving who they are is the whole point).
///
/// `console_client_id` is `human_login.console_client.client_id`. Mounted only
/// when `human_login` is served.
// No `#[must_use]`: axum's `Router` already carries one.
pub fn router(db: Cratestack, op: OpState, console_client_id: &str, limiter: Limiter) -> Router {
    Router::new()
        .route("/login", post(login_handler))
        .layer(DefaultBodyLimit::max(MAX_LOGIN_BODY_BYTES))
        .with_state(LoginState {
            db,
            op,
            console_client_id: Arc::from(console_client_id),
            limiter,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> LoginRequest {
        LoginRequest {
            email: "operator@example.test".to_owned(),
            password: "does-not-matter-here".to_owned(),
            client_id: "sms-console".to_owned(),
            redirect_uri: "https://admin.example/api/auth/callback".to_owned(),
            response_type: "code".to_owned(),
            scope: "openid profile".to_owned(),
            state: "state-abc123".to_owned(),
            code_challenge: "challenge-abc123".to_owned(),
            code_challenge_method: "S256".to_owned(),
            nonce: "nonce-abc123".to_owned(),
        }
    }

    /// The cheap, no-database half of the guard `build_authorize_request`'s doc
    /// names: the four `Option<String>` fields of `AuthorizeRequest` a rename
    /// would silently turn into `None` arrive `Some` for realistic input.
    #[test]
    fn the_authorize_request_keeps_its_optional_fields() {
        let authorize = build_authorize_request(&request()).expect("this JSON always deserializes");
        assert_eq!(authorize.state.as_deref(), Some("state-abc123"));
        assert_eq!(
            authorize.code_challenge.as_deref(),
            Some("challenge-abc123")
        );
        assert_eq!(authorize.code_challenge_method.as_deref(), Some("S256"));
        assert_eq!(authorize.nonce.as_deref(), Some("nonce-abc123"));
        assert_eq!(authorize.scope, "openid profile");
    }

    /// The wire shape is pinned: `snake_case` OAuth names, no unknown field.
    #[test]
    fn the_request_wire_shape_is_snake_case_and_refuses_unknown_fields() {
        let json = serde_json::json!({
            "email": "a@b.test", "password": "p", "client_id": "sms-console",
            "redirect_uri": "https://x/cb", "response_type": "code", "scope": "openid",
            "state": "s", "code_challenge": "c", "code_challenge_method": "S256",
            "nonce": "n",
        });
        assert!(serde_json::from_value::<LoginRequest>(json.clone()).is_ok());
        let mut camel = json.clone();
        camel["clientId"] = camel["client_id"].take();
        camel.as_object_mut().unwrap().remove("client_id");
        assert!(serde_json::from_value::<LoginRequest>(camel).is_err());
        let mut extra = json;
        extra["remember_me"] = true.into();
        assert!(serde_json::from_value::<LoginRequest>(extra).is_err());
    }

    #[test]
    fn the_response_and_failure_shapes_are_pinned() {
        assert_eq!(
            serde_json::to_value(LoginResponse {
                redirect: "https://x/cb?code=c".to_owned()
            })
            .unwrap(),
            serde_json::json!({ "redirect": "https://x/cb?code=c" })
        );
        assert_eq!(
            serde_json::to_value(LoginFailure {
                error: "invalid_credentials"
            })
            .unwrap(),
            serde_json::json!({ "error": "invalid_credentials" })
        );
    }

    #[test]
    fn an_error_redirect_is_not_a_sign_in() {
        assert!(redirect_carries_an_error(
            "https://x/cb?error=invalid_request&error_description=nope&state=s"
        ));
        assert!(
            redirect_carries_an_error("https://x/cb?state=s"),
            "no code is no sign-in"
        );
        assert!(!redirect_carries_an_error("https://x/cb?code=abc&state=s"));
        assert!(redirect_carries_an_error("not a url"));
    }

    #[test]
    fn the_email_key_ignores_case_and_surrounding_space_and_is_bounded() {
        assert_eq!(
            email_key("Ana@Example.test"),
            email_key("  ana@example.test ")
        );
        assert_ne!(email_key("ana@example.test"), email_key("bob@example.test"));
        assert!(email_key(&"a".repeat(10_000)).len() <= MAX_EMAIL_KEY_BYTES + "login-email:".len());
        // A multi-byte character straddling the cap must not panic the truncation.
        let straddle = format!("{}é", "a".repeat(MAX_EMAIL_KEY_BYTES - 1));
        assert!(email_key(&straddle).is_char_boundary(email_key(&straddle).len()));
    }

    #[tokio::test]
    async fn the_limiter_takes_a_token_from_the_source_and_from_the_email() {
        let limiter = Limiter::new(RateLimitConfig::new(2, 0.001));
        // The same email from two sources: the email bucket (2) runs out first.
        assert!(matches!(
            limiter.admit("ip:1.1.1.1", "a@x.test").await,
            Admission::Allowed
        ));
        assert!(matches!(
            limiter.admit("ip:2.2.2.2", "a@x.test").await,
            Admission::Allowed
        ));
        assert!(matches!(
            limiter.admit("ip:3.3.3.3", "A@x.test ").await,
            Admission::Throttled { .. }
        ));
        // Another email from a source that has used one token: still admitted
        // once, then the source bucket (2) is the limit.
        assert!(matches!(
            limiter.admit("ip:1.1.1.1", "b@x.test").await,
            Admission::Allowed
        ));
        assert!(matches!(
            limiter.admit("ip:1.1.1.1", "c@x.test").await,
            Admission::Throttled { .. }
        ));
    }
}
