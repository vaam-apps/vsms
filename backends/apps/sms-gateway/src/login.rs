#![doc = include_str!("login.md")]

use std::sync::Arc;

use authkestra_engine::auth::state::Identity;
use authkestra_op::handlers::authorize::{AuthorizeOutcome, handle_authorize};
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use cratestack::ratelimit::{
    InMemoryRateLimitStore, RateLimitConfig, RateLimitDecision, RateLimitStore,
};
use serde::{Deserialize, Serialize};
use sms_api::schema::Cratestack;
use sms_auth::login::{LoginError, authenticate_user, stamp_last_login};
use sms_config::LoginRateLimit;

use crate::console_authorize::{ConsoleAuthorize, redirect_carries_an_error};
use crate::op::OpState;

/// A sign-in body is a few hundred bytes. The cap is checked before any password
/// is hashed, so an oversized body costs the reader and nothing more.
const MAX_LOGIN_BODY_BYTES: usize = 16 * 1024;

/// The longest email the rate-limit key keeps. A longer one is truncated in the
/// *key* only (the account lookup uses the whole value and finds nothing), so an
/// attacker cannot make the limiter store one huge key per request.
const MAX_EMAIL_KEY_BYTES: usize = 254;

/// The `provider_id` of the identity a code carries, for a password sign-in and
/// for single sign-on alike: a person is the same person whichever way they
/// proved it, and the bearer door keys the person realm on the `sub`, not on this.
pub(crate) const PROVIDER_ID: &str = "sms-console";

#[derive(Clone)]
struct LoginState {
    db: Cratestack,
    op: OpState,
    /// `human_login.console_client.client_id`: the one id `POST /login` accepts.
    console_client_id: Arc<str>,
    /// `human_login.console_client.redirect_uris`: the only places a code may be
    /// sent, checked before the limiter and the password.
    redirect_uris: Arc<[String]>,
    limiter: Limiter,
}

/// The two buckets in front of the password check, both from
/// `human_login.login_rate_limit`: one per email address, and one for every
/// sign-in together.
///
/// **There is no bucket per source address**, and that is deliberate (`login.md`,
/// "No bucket per source address"): the only address this process can see is the
/// TCP peer, and behind Caddy or the console's own server that is the same
/// address for every person, so a "per source" bucket is a global one that one
/// bad request every ten seconds keeps empty.
#[derive(Clone)]
pub struct Limiter {
    store: Arc<dyn RateLimitStore>,
    per_email: RateLimitConfig,
    aggregate: RateLimitConfig,
}

impl Limiter {
    /// A limiter with its own in-memory store (bounded and swept: cratestack's).
    /// One `serve` replica, like the codes the sign-in issues.
    #[must_use]
    pub fn new(limits: &LoginRateLimit) -> Self {
        Self {
            store: Arc::new(InMemoryRateLimitStore::new()),
            per_email: RateLimitConfig::new(
                limits.per_email.burst,
                limits.per_email.refill_per_second,
            ),
            aggregate: RateLimitConfig::new(
                limits.aggregate.burst,
                limits.aggregate.refill_per_second,
            ),
        }
    }

    /// Takes one token from `email`'s bucket, then one from the aggregate.
    ///
    /// **The email bucket is first, and that order is load-bearing.** A request
    /// the email bucket refuses never reaches the aggregate, so hammering one
    /// address spends only that address's tokens and cannot empty the bucket
    /// every other person's sign-in draws on.
    async fn admit(&self, email: &str) -> Admission {
        for (key, config) in [
            (email_key(email), self.per_email),
            (AGGREGATE_KEY.to_owned(), self.aggregate),
        ] {
            match self.store.consume(&key, config).await {
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

/// The aggregate bucket's one key.
const AGGREGATE_KEY: &str = "login-aggregate";

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

impl LoginRequest {
    /// The OAuth half of the body, which `POST /login` and `GET /login/sso/start`
    /// share (`console_authorize.rs`).
    fn console(&self) -> ConsoleAuthorize {
        ConsoleAuthorize {
            client_id: self.client_id.clone(),
            redirect_uri: self.redirect_uri.clone(),
            response_type: self.response_type.clone(),
            scope: self.scope.clone(),
            state: self.state.clone(),
            code_challenge: self.code_challenge.clone(),
            code_challenge_method: self.code_challenge_method.clone(),
            nonce: self.nonce.clone(),
        }
    }
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

    // The checks that need no account and no secret, before the limiter and
    // before any Argon2 work: a request that can never succeed spends nothing.
    let console = request.console();
    if let Some(field) = console.malformed(&state.redirect_uris) {
        tracing::warn!(field, "login: the request is malformed");
        return failure(StatusCode::BAD_REQUEST, "invalid_request");
    }

    // Before any Argon2 work: the limiter exists to bound how many password
    // hashes a caller can make this process run.
    match state.limiter.admit(&request.email).await {
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
    let stamp = authenticated.last_login_stamp();
    let identity = Identity {
        provider_id: PROVIDER_ID.to_owned(),
        external_id: authenticated.subject,
        email: Some(authenticated.email),
        username: Some(authenticated.display_name),
        attributes: std::collections::HashMap::new(),
    };

    let authorize_request = match console.authorize_request() {
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
            // Only now, with a code issued: `lastLoginAt` says the person
            // signed in, not that their password was once right.
            stamp_last_login(&state.db, &stamp).await;
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

/// `POST /login`, already `.with_state(...)`: mergeable directly with the rest of
/// `serve`'s routers, outside the envelope layer (a person signing in has no key
/// to sign with yet; proving who they are is the whole point).
///
/// `console_client_id` is `human_login.console_client.client_id`. Mounted only
/// when `human_login` is served.
// No `#[must_use]`: axum's `Router` already carries one.
pub fn router(
    db: Cratestack,
    op: OpState,
    console_client_id: &str,
    redirect_uris: &[String],
    limiter: Limiter,
) -> Router {
    Router::new()
        .route("/login", post(login_handler))
        .layer(DefaultBodyLimit::max(MAX_LOGIN_BODY_BYTES))
        .with_state(LoginState {
            db,
            op,
            console_client_id: Arc::from(console_client_id),
            redirect_uris: redirect_uris.into(),
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

    /// The OAuth half of the body is exactly `ConsoleAuthorize`: a field added to
    /// one and not the other would reach `handle_authorize` as a default.
    #[test]
    fn the_console_half_of_the_body_is_every_field_but_the_credentials() {
        let mut body = serde_json::to_value(serde_json::json!({
            "email": request().email,
            "password": request().password,
        }))
        .unwrap();
        let console = serde_json::to_value(request().console()).unwrap();
        body.as_object_mut()
            .unwrap()
            .extend(console.as_object().unwrap().clone());
        let parsed: LoginRequest = serde_json::from_value(body).expect("the same eight fields");
        assert_eq!(parsed.console(), request().console());
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

    fn limits(per_email: u32, aggregate: u32) -> LoginRateLimit {
        let bucket = |burst| sms_config::Bucket {
            burst,
            refill_per_second: 0.001,
        };
        LoginRateLimit {
            per_email: bucket(per_email),
            aggregate: bucket(aggregate),
        }
    }

    /// The email bucket is one address's own: spending it never touches another
    /// address, and case and surrounding space do not make a second one.
    #[tokio::test]
    async fn the_email_bucket_is_one_addresses_own() {
        let limiter = Limiter::new(&limits(2, 100));
        assert!(matches!(
            limiter.admit("a@x.test").await,
            Admission::Allowed
        ));
        assert!(matches!(
            limiter.admit("A@x.test ").await,
            Admission::Allowed
        ));
        assert!(matches!(
            limiter.admit("a@x.test").await,
            Admission::Throttled { .. }
        ));
        assert!(
            matches!(limiter.admit("b@x.test").await, Admission::Allowed),
            "another address is untouched by a throttled one"
        );
    }

    /// The aggregate bucket is every sign-in's together: distinct addresses spend
    /// one shared budget, which is what bounds the Argon2 work whoever asks.
    #[tokio::test]
    async fn the_aggregate_bucket_is_shared_by_every_address() {
        let limiter = Limiter::new(&limits(100, 3));
        for email in ["a@x.test", "b@x.test", "c@x.test"] {
            assert!(matches!(limiter.admit(email).await, Admission::Allowed));
        }
        assert!(
            matches!(limiter.admit("d@x.test").await, Admission::Throttled { .. }),
            "a fourth address finds the shared budget spent"
        );
    }

    /// The order the doc on `admit` argues for, observed: an address the email
    /// bucket refuses never reaches the aggregate, so hammering one address
    /// cannot empty the bucket everyone else draws on.
    #[tokio::test]
    async fn a_throttled_address_does_not_spend_the_aggregate() {
        let limiter = Limiter::new(&limits(1, 3));
        assert!(matches!(
            limiter.admit("victim@x.test").await,
            Admission::Allowed
        ));
        for _ in 0..50 {
            assert!(matches!(
                limiter.admit("victim@x.test").await,
                Admission::Throttled { .. }
            ));
        }
        // The aggregate holds 3: one went to the first request, so two are left.
        assert!(matches!(
            limiter.admit("other-1@x.test").await,
            Admission::Allowed
        ));
        assert!(
            matches!(limiter.admit("other-2@x.test").await, Admission::Allowed),
            "fifty refusals of one address must not have drained the aggregate"
        );
    }
}
