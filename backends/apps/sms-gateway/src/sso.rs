#![doc = include_str!("sso.md")]

use std::sync::Arc;
use std::time::Duration;

use authkestra_engine::auth::state::Identity;
use authkestra_op::handlers::authorize::{AuthorizeOutcome, handle_authorize};
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::Utc;
use cratestack::CratestackContext;
use cratestack::ratelimit::{
    InMemoryRateLimitStore, RateLimitConfig, RateLimitDecision, RateLimitStore,
};
use serde::Deserialize;
use sms_api::schema::Cratestack;
use sms_auth::sso::flow::{self, Completed};
use sms_auth::sso::{
    AuthnSecrets, CALLBACK_PATH, Refusal, START_PATH, SsoError, SsoProvider, store,
};
use sms_config::Bucket;

use crate::console_authorize::{ConsoleAuthorize, redirect_carries_an_error};
use crate::login::PROVIDER_ID;
use crate::op::OpState;

/// Longest value any query parameter may have. Real values are tens of bytes (a
/// PKCE challenge is 43); this only bounds what an unauthenticated caller can make
/// the database hold.
const MAX_PARAM_LEN: usize = 1024;

/// Longest upstream `code` the callback accepts. It is the one parameter the
/// identity provider, not the console, makes: some providers issue codes that are
/// themselves signed tokens of well over a kilobyte, and refusing one would fail
/// every sign-in at the last step. It is still bounded, and it is never stored (it
/// is spent at the provider's token endpoint), so a larger cap costs the database
/// nothing.
const MAX_CODE_LEN: usize = 4096;

/// How often the reaper looks for expired transactions.
const REAPER_INTERVAL: Duration = Duration::from_secs(60);
const REAPER_BATCH: i64 = 500;

/// The two buckets' keys: one global bucket for each route (`sso.md`, "One bucket
/// per route, and no bucket per source").
const START_BUCKET: &str = "sso:start";
const CALLBACK_BUCKET: &str = "sso:callback";

#[derive(Clone)]
struct SsoState {
    db: Cratestack,
    sys: CratestackContext,
    op: OpState,
    provider: SsoProvider,
    /// `human_login.console_client.client_id`: the one client `start` accepts.
    console_client_id: Arc<str>,
    /// `human_login.console_client.redirect_uris`, checked before anything is
    /// stored.
    redirect_uris: Arc<[String]>,
    limiter: Arc<dyn RateLimitStore>,
    rate: RateLimitConfig,
}

/// The console's authorize request, as it arrives on the query string: every
/// parameter optional here, so a missing one is a `400` of ours and not an
/// extractor's.
#[derive(Debug, Deserialize)]
struct StartQuery {
    client_id: Option<String>,
    redirect_uri: Option<String>,
    response_type: Option<String>,
    scope: Option<String>,
    state: Option<String>,
    nonce: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
}

impl StartQuery {
    /// The request, if every parameter is present, non-empty and within
    /// [`MAX_PARAM_LEN`].
    fn into_request(self) -> Option<ConsoleAuthorize> {
        let take = |value: Option<String>| {
            value.filter(|value| !value.is_empty() && value.len() <= MAX_PARAM_LEN)
        };
        Some(ConsoleAuthorize {
            client_id: take(self.client_id)?,
            redirect_uri: take(self.redirect_uri)?,
            response_type: take(self.response_type)?,
            scope: take(self.scope)?,
            state: take(self.state)?,
            code_challenge: take(self.code_challenge)?,
            code_challenge_method: take(self.code_challenge_method)?,
            nonce: take(self.nonce)?,
        })
    }
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

fn no_store(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn plain(status: StatusCode, message: &'static str) -> Response {
    no_store((status, message).into_response())
}

fn redirect(location: &str) -> Response {
    match HeaderValue::from_str(location) {
        Ok(value) => no_store((StatusCode::FOUND, [(header::LOCATION, value)]).into_response()),
        Err(_) => plain(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Sign-in could not be completed.",
        ),
    }
}

/// The OAuth error redirect back to the console: its own registered
/// `redirect_uri` (checked at `start`), `error`, and its own `state`.
fn console_error_redirect(request: &ConsoleAuthorize, error: &str) -> Response {
    let Ok(mut url) = url::Url::parse(&request.redirect_uri) else {
        return plain(StatusCode::BAD_REQUEST, "Sign-in could not be completed.");
    };
    url.query_pairs_mut()
        .append_pair("error", error)
        .append_pair("state", &request.state);
    redirect(url.as_str())
}

async fn throttled(state: &SsoState, key: &str) -> Option<Response> {
    match state.limiter.consume(key, state.rate).await {
        Ok(RateLimitDecision::Allowed { .. }) => None,
        Ok(RateLimitDecision::Throttled { retry_after_secs }) => {
            let mut response = plain(StatusCode::TOO_MANY_REQUESTS, "Too many sign-in attempts.");
            if let Ok(value) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            Some(response)
        }
        // Fail closed, like the other limiters in this binary.
        Err(error) => {
            tracing::warn!(%error, "SSO rate limit store error");
            Some(plain(
                StatusCode::SERVICE_UNAVAILABLE,
                "Sign-in is unavailable.",
            ))
        }
    }
}

fn placeholder_identity() -> Identity {
    Identity {
        provider_id: PROVIDER_ID.to_owned(),
        external_id: "sso-start-validation".to_owned(),
        email: None,
        username: None,
        attributes: std::collections::HashMap::new(),
    }
}

/// The dry run: the library's real `handle_authorize` for this request, against a
/// store whose code slot is thrown away when the call returns, so nothing it
/// issues can be redeemed. `true` only for an answer that would have issued a
/// code.
///
/// **It is the library that decides, not a copy of its rules**: a scope the
/// console may not hold, a response type that is not `code` and a client or
/// redirect it does not know are all refused here, at `start`, before a
/// transaction is stored or a person is sent upstream.
async fn would_issue_a_code(state: &SsoState, request: &ConsoleAuthorize) -> bool {
    let authorize_request = match request.authorize_request() {
        Ok(authorize_request) => authorize_request,
        Err(error) => {
            tracing::warn!(%error, "SSO start: the authorize request did not deserialize");
            return false;
        }
    };
    // A fresh store each call: `op_store` builds its own in-memory code slot, which
    // is not the one `/token` redeems (that is `OpState::store`).
    let mut dry_store = sms_auth::op::op_store(Arc::new(state.db.clone()), state.sys.clone());
    match handle_authorize(
        authorize_request,
        placeholder_identity(),
        &state.op.config(),
        &mut dry_store,
    )
    .await
    {
        AuthorizeOutcome::DirectError(error) => {
            tracing::warn!(%error, "SSO start: handle_authorize refused the console's request");
            false
        }
        // A request the library would answer with an error redirect. Never
        // forwarded: the console is not handed an error it did not provoke from a
        // route it did not call, and nothing here is worth a transaction.
        AuthorizeOutcome::Redirect(location) if redirect_carries_an_error(&location) => {
            tracing::warn!("SSO start: handle_authorize answered with an error redirect");
            false
        }
        AuthorizeOutcome::Redirect(_) => true,
    }
}

async fn start_handler(State(state): State<SsoState>, Query(query): Query<StartQuery>) -> Response {
    // 1. The bucket, before anything is read or written.
    if let Some(response) = throttled(&state, START_BUCKET).await {
        return response;
    }
    // 2. Every parameter present, non-empty and bounded.
    let Some(request) = query.into_request() else {
        return plain(StatusCode::BAD_REQUEST, "Invalid sign-in request.");
    };
    // 3. The console, and only the console: the one client a transaction is
    //    stored for.
    if request.client_id != *state.console_client_id {
        tracing::warn!("SSO start: the client_id is not the console's");
        return plain(StatusCode::BAD_REQUEST, "Invalid sign-in request.");
    }
    // 4. What the request alone proves wrong, the same checks `POST /login`
    //    makes.
    if let Some(field) = request.malformed(&state.redirect_uris) {
        tracing::warn!(field, "SSO start: the request is malformed");
        return plain(StatusCode::BAD_REQUEST, "Invalid sign-in request.");
    }
    // 5. The real `handle_authorize`, as a dry run.
    if !would_issue_a_code(&state, &request).await {
        return plain(StatusCode::BAD_REQUEST, "Invalid sign-in request.");
    }
    // 6. Only now a row, and the browser upstream.
    let secrets = AuthnSecrets::generate();
    match begin_upstream(&state, &secrets, &request, false).await {
        Ok(location) => redirect(&location),
        Err(error) => {
            tracing::error!(%error, "SSO start failed");
            console_error_redirect(&request, "temporarily_unavailable")
        }
    }
}

/// Persists the transaction and builds the upstream authorization URL.
async fn begin_upstream(
    state: &SsoState,
    secrets: &AuthnSecrets,
    request: &ConsoleAuthorize,
    is_retry: bool,
) -> Result<String, SsoError> {
    let stored = serde_json::to_string(request)
        .map_err(|_| SsoError::Unavailable("serialising the console request failed".to_owned()))?;
    store::begin_transaction(&state.db, &state.sys, secrets, &stored, is_retry).await?;
    let url = state.provider.authorization_url(secrets, is_retry).await?;
    Ok(url.into())
}

async fn callback_handler(
    State(state): State<SsoState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    if let Some(response) = throttled(&state, CALLBACK_BUCKET).await {
        return response;
    }
    let Some(returned_state) = query
        .state
        .filter(|s| !s.is_empty() && s.len() <= MAX_PARAM_LEN)
    else {
        return plain(StatusCode::BAD_REQUEST, "Sign-in could not be completed.");
    };
    let transaction = match store::consume_transaction(&state.db, &state.sys, &returned_state).await
    {
        Ok(Some(transaction)) => transaction,
        Ok(None) => {
            // Unknown, replayed or expired: indistinguishable on purpose, and with
            // no console request to trust there is nowhere to redirect.
            tracing::warn!(
                reason = Refusal::NoSuchTransaction.reason(),
                "SSO sign-in refused"
            );
            return plain(StatusCode::BAD_REQUEST, "Sign-in could not be completed.");
        }
        Err(error) => {
            tracing::error!(%error, "SSO callback could not look up its transaction");
            return plain(StatusCode::SERVICE_UNAVAILABLE, "Sign-in is unavailable.");
        }
    };
    let Ok(request) = serde_json::from_str::<ConsoleAuthorize>(&transaction.console_request) else {
        tracing::error!("SSO callback: the stored console request is not a console request");
        return plain(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Sign-in could not be completed.",
        );
    };

    // The stored `redirect_uri` was registered when the row was written. The console
    // client's list can have changed since (a redeploy with another callback), and
    // an error redirect is still a redirect: only to a URI that is registered now.
    if !state.redirect_uris.contains(&request.redirect_uri) {
        tracing::error!("SSO callback: the stored redirect_uri is no longer a registered one");
        return plain(StatusCode::BAD_REQUEST, "Sign-in could not be completed.");
    }

    if query.error.is_some() {
        tracing::warn!(
            reason = Refusal::UpstreamDenied.reason(),
            "SSO sign-in refused"
        );
        return console_error_redirect(&request, "access_denied");
    }
    let Some(code) = query
        .code
        .filter(|c| !c.is_empty() && c.len() <= MAX_CODE_LEN)
    else {
        tracing::warn!(reason = Refusal::NoCode.reason(), "SSO sign-in refused");
        return console_error_redirect(&request, "access_denied");
    };

    match flow::complete(&state.provider, &state.db, &state.sys, &transaction, &code).await {
        Ok(Completed::SignedIn(user)) => {
            tracing::info!(
                provisioned = user.provisioned,
                at = %Utc::now(),
                "SSO sign-in verified"
            );
            issue_console_code(&state, &request, user).await
        }
        Ok(Completed::RetryWithPromptLogin) => {
            tracing::info!(
                reason = "mfa_claim_missing_retrying_with_prompt_login",
                "SSO sign-in is being retried once"
            );
            let secrets = AuthnSecrets::generate();
            match begin_upstream(&state, &secrets, &request, true).await {
                Ok(location) => redirect(&location),
                Err(error) => {
                    tracing::error!(%error, "SSO retry could not be started");
                    console_error_redirect(&request, "temporarily_unavailable")
                }
            }
        }
        Err(SsoError::Refused(refusal)) => {
            tracing::warn!(reason = refusal.reason(), "SSO sign-in refused");
            console_error_redirect(&request, "access_denied")
        }
        Err(SsoError::Unavailable(what)) => {
            tracing::error!(what, "SSO sign-in could not be evaluated");
            console_error_redirect(&request, "temporarily_unavailable")
        }
    }
}

/// The last step, identical to the password path's: the same `Identity` shape into
/// the same unmodified `handle_authorize`, now with the real store, so the console
/// gets a real authorization code - and the same person realm and the same
/// owner/admin cap at `/token` and at the API apply to it unchanged.
///
/// **`lastLoginAt` is stamped last**, once a code has been issued, as
/// `POST /login` does: a verified token whose authorization request the library
/// then refuses signed nobody in.
async fn issue_console_code(
    state: &SsoState,
    request: &ConsoleAuthorize,
    user: store::ResolvedUser,
) -> Response {
    let authorize_request = match request.authorize_request() {
        Ok(authorize_request) => authorize_request,
        Err(error) => {
            tracing::error!(%error, "SSO callback: the stored authorize request no longer deserializes");
            return plain(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Sign-in could not be completed.",
            );
        }
    };
    let user_id = user.user_id.clone();
    let identity = Identity {
        provider_id: PROVIDER_ID.to_owned(),
        external_id: user.user_id,
        email: Some(user.email),
        username: Some(user.display_name),
        attributes: std::collections::HashMap::new(),
    };
    let mut op_store = state.op.store();
    match handle_authorize(
        authorize_request,
        identity,
        &state.op.config(),
        op_store.as_mut(),
    )
    .await
    {
        AuthorizeOutcome::Redirect(location) if redirect_carries_an_error(&location) => {
            // The console client or its scopes changed between `start` and now.
            // Never handed to the browser as a sign-in.
            tracing::warn!("SSO callback: handle_authorize refused the stored request");
            plain(StatusCode::BAD_REQUEST, "Sign-in could not be completed.")
        }
        AuthorizeOutcome::Redirect(location) => {
            store::stamp_last_login(&state.db, &state.sys, &user_id).await;
            // A browser navigation: `302` to the console with the code, unlike
            // `POST /login`, which answers the console's server in JSON.
            redirect(&location)
        }
        AuthorizeOutcome::DirectError(error) => {
            // The console client or its redirect changed between `start` and now.
            // Nothing trustworthy to redirect to.
            tracing::error!(%error, "SSO callback: handle_authorize refused the stored request");
            plain(StatusCode::BAD_REQUEST, "Sign-in could not be completed.")
        }
    }
}

/// The SSO routes, already `.with_state(...)`. Mounted only when
/// `human_login.sso` is served.
// No `#[must_use]`: axum's `Router` already carries one - same reasoning as
// `login::router`.
pub fn router(
    db: Cratestack,
    sys: CratestackContext,
    op: OpState,
    provider: SsoProvider,
    console_client_id: &str,
    redirect_uris: &[String],
    rate: Bucket,
) -> Router {
    let state = SsoState {
        db,
        sys,
        op,
        provider,
        console_client_id: Arc::from(console_client_id),
        redirect_uris: redirect_uris.into(),
        limiter: Arc::new(InMemoryRateLimitStore::new()),
        rate: RateLimitConfig::new(rate.burst, rate.refill_per_second),
    };
    Router::new()
        .route(START_PATH, get(start_handler))
        .route(CALLBACK_PATH, get(callback_handler))
        .with_state(state)
}

/// Deletes expired, never-consumed transactions every minute. Never returns;
/// dropped with the runtime on shutdown, like the other background loops in this
/// binary. Several replicas may run it: a delete that loses the race is not an
/// error. Spawned by `serve` only when SSO is served.
pub fn spawn_transaction_reaper(db: Cratestack, sys: CratestackContext) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(REAPER_INTERVAL);
        loop {
            ticker.tick().await;
            match store::reap_expired(&db, &sys, Utc::now(), REAPER_BATCH).await {
                Ok(0) => {}
                Ok(reaped) => tracing::debug!(reaped, "reaped expired SSO transactions"),
                Err(error) => tracing::warn!(%error, "reaping expired SSO transactions failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> StartQuery {
        StartQuery {
            client_id: Some("sms-console".to_owned()),
            redirect_uri: Some("https://admin.example/api/auth/callback".to_owned()),
            response_type: Some("code".to_owned()),
            scope: Some("openid profile".to_owned()),
            state: Some("s".to_owned()),
            nonce: Some("n".to_owned()),
            code_challenge: Some("c".to_owned()),
            code_challenge_method: Some("S256".to_owned()),
        }
    }

    #[test]
    fn a_complete_query_becomes_the_console_request() {
        let request = query().into_request().expect("complete");
        assert_eq!(request.client_id, "sms-console");
        assert_eq!(request.state, "s");
        assert_eq!(request.nonce, "n");
        assert_eq!(request.code_challenge_method, "S256");
    }

    #[test]
    fn a_missing_empty_or_oversized_parameter_is_refused_before_any_row_is_written() {
        for blank in [None, Some(String::new())] {
            let mut no_redirect = query();
            no_redirect.redirect_uri = blank.clone();
            assert!(no_redirect.into_request().is_none());
            let mut no_nonce = query();
            no_nonce.nonce = blank.clone();
            assert!(no_nonce.into_request().is_none(), "nonce is required");
            let mut no_state = query();
            no_state.state = blank;
            assert!(no_state.into_request().is_none(), "state is required");
        }
        let mut huge = query();
        huge.scope = Some("x".repeat(MAX_PARAM_LEN + 1));
        assert!(huge.into_request().is_none());
    }

    #[test]
    fn the_error_redirect_carries_the_consoles_own_state_and_nothing_else() {
        let request = ConsoleAuthorize {
            client_id: "sms-console".to_owned(),
            redirect_uri: "https://admin.example/api/auth/callback".to_owned(),
            response_type: "code".to_owned(),
            scope: "openid".to_owned(),
            state: "abc".to_owned(),
            code_challenge: "c".to_owned(),
            code_challenge_method: "S256".to_owned(),
            nonce: "n".to_owned(),
        };
        let response = console_error_redirect(&request, "access_denied");
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response.headers()[header::LOCATION].to_str().unwrap();
        assert_eq!(
            location,
            "https://admin.example/api/auth/callback?error=access_denied&state=abc"
        );
    }
}
