#![doc = include_str!("sso.md")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use authkestra_engine::auth::state::Identity;
use authkestra_op::handlers::authorize::{AuthorizeOutcome, AuthorizeRequest, handle_authorize};
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
use serde_json::Value;
use sms_api::schema::Cratestack;
use sms_auth::sso::flow::{self, Completed};
use sms_auth::sso::{AuthnSecrets, CALLBACK_PATH, START_PATH, SsoError, SsoProvider, store};

use crate::op::OpState;

/// Longest value any `start` query parameter may have. Real values are tens
/// of bytes (a PKCE challenge is 43); this only bounds what an
/// unauthenticated caller can make the database hold.
const MAX_PARAM_LEN: usize = 2048;

/// The in-process backstop: 30 sign-in starts in a burst, then one per
/// second, for the whole gateway. A real deployment has a handful of staff.
const RATE_BURST: u32 = 30;
const RATE_REFILL_PER_SECOND: f64 = 1.0;

/// How often the reaper looks for expired transactions.
const REAPER_INTERVAL: Duration = Duration::from_secs(60);
const REAPER_BATCH: i64 = 500;

#[derive(Clone)]
struct SsoState {
    db: Cratestack,
    sys: CratestackContext,
    op: OpState,
    provider: SsoProvider,
    limiter: Arc<dyn RateLimitStore>,
}

/// The console's authorize request, as it arrives on the query string.
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

/// The OAuth error redirect back to the console: its own validated
/// `redirect_uri`, `error`, and its own `state`. `request` is the stored
/// console authorize request (already validated at `start`).
fn console_error_redirect(request: &Value, error: &str) -> Response {
    let Some(base) = request.get("redirect_uri").and_then(Value::as_str) else {
        return plain(StatusCode::BAD_REQUEST, "Sign-in could not be completed.");
    };
    let Ok(mut url) = url::Url::parse(base) else {
        return plain(StatusCode::BAD_REQUEST, "Sign-in could not be completed.");
    };
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("error", error);
        if let Some(state) = request.get("state").and_then(Value::as_str) {
            query.append_pair("state", state);
        }
    }
    redirect(url.as_str())
}

async fn throttled(state: &SsoState, key: &str) -> Option<Response> {
    let config = RateLimitConfig::new(RATE_BURST, RATE_REFILL_PER_SECOND);
    match state.limiter.consume(key, config).await {
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

/// The console's request as the JSON an `AuthorizeRequest` deserializes
/// from. Returns `None` if a required parameter is absent or oversized.
fn console_request_json(query: StartQuery) -> Option<Value> {
    let StartQuery {
        client_id,
        redirect_uri,
        response_type,
        scope,
        state,
        nonce,
        code_challenge,
        code_challenge_method,
    } = query;
    let mut request = serde_json::Map::new();
    for (key, value, required) in [
        ("client_id", client_id, true),
        ("redirect_uri", redirect_uri, true),
        ("response_type", response_type, true),
        ("scope", scope, false),
        ("state", state, false),
        ("nonce", nonce, false),
        ("code_challenge", code_challenge, false),
        ("code_challenge_method", code_challenge_method, false),
    ] {
        match value {
            Some(v) if v.len() <= MAX_PARAM_LEN && (!required || !v.is_empty()) => {
                request.insert(key.to_owned(), Value::String(v));
            }
            Some(_) => return None,
            None if required => return None,
            None => {}
        }
    }
    Some(Value::Object(request))
}

fn placeholder_identity() -> Identity {
    Identity {
        provider_id: "sms-console".to_owned(),
        external_id: "sso-start-validation".to_owned(),
        email: None,
        username: None,
        attributes: HashMap::new(),
    }
}

async fn start_handler(State(state): State<SsoState>, Query(query): Query<StartQuery>) -> Response {
    if let Some(response) = throttled(&state, "sso:start").await {
        return response;
    }
    let Some(request_json) = console_request_json(query) else {
        return plain(StatusCode::BAD_REQUEST, "Invalid sign-in request.");
    };
    let authorize_request: AuthorizeRequest = match serde_json::from_value(request_json.clone()) {
        Ok(request) => request,
        Err(error) => {
            tracing::warn!(%error, "SSO start: the authorize request did not deserialize");
            return plain(StatusCode::BAD_REQUEST, "Invalid sign-in request.");
        }
    };

    // The dry run: the real `handle_authorize`, against a store whose code
    // slot is thrown away after this call.
    let mut dry_store =
        sms_auth::op::machine_only_store(Arc::new(state.db.clone()), state.sys.clone());
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
            return plain(StatusCode::BAD_REQUEST, "Invalid sign-in request.");
        }
        // A redirect-able problem (scope, PKCE shape): hand the browser to
        // the console exactly as the library built it, error and all.
        AuthorizeOutcome::Redirect(location) if location.contains("error=") => {
            tracing::warn!("SSO start: handle_authorize answered with an error redirect");
            return redirect(&location);
        }
        AuthorizeOutcome::Redirect(_) => {}
    }

    let secrets = AuthnSecrets::generate();
    match begin_upstream(&state, &secrets, &request_json, false).await {
        Ok(location) => redirect(&location),
        Err(error) => {
            tracing::error!(%error, "SSO start failed");
            console_error_redirect(&request_json, "temporarily_unavailable")
        }
    }
}

/// Persists the transaction and builds the upstream authorization URL.
async fn begin_upstream(
    state: &SsoState,
    secrets: &AuthnSecrets,
    request_json: &Value,
    is_retry: bool,
) -> Result<String, SsoError> {
    store::begin_transaction(
        &state.db,
        &state.sys,
        secrets,
        &request_json.to_string(),
        is_retry,
    )
    .await?;
    let url = state.provider.authorization_url(secrets, is_retry).await?;
    Ok(url.into())
}

async fn callback_handler(
    State(state): State<SsoState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    if let Some(response) = throttled(&state, "sso:callback").await {
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
            // Unknown, replayed or expired: indistinguishable on purpose,
            // and with no console request to trust there is nowhere to
            // redirect.
            tracing::warn!(reason = "no_such_transaction", "SSO sign-in refused");
            return plain(StatusCode::BAD_REQUEST, "Sign-in could not be completed.");
        }
        Err(error) => {
            tracing::error!(%error, "SSO callback could not look up its transaction");
            return plain(StatusCode::SERVICE_UNAVAILABLE, "Sign-in is unavailable.");
        }
    };
    let Ok(request_json) = serde_json::from_str::<Value>(&transaction.console_request) else {
        tracing::error!("SSO callback: the stored console request is not JSON");
        return plain(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Sign-in could not be completed.",
        );
    };

    if query.error.is_some() {
        tracing::warn!(reason = "upstream_denied", "SSO sign-in refused");
        return console_error_redirect(&request_json, "access_denied");
    }
    let Some(code) = query
        .code
        .filter(|c| !c.is_empty() && c.len() <= MAX_PARAM_LEN)
    else {
        tracing::warn!(reason = "no_code", "SSO sign-in refused");
        return console_error_redirect(&request_json, "access_denied");
    };

    match flow::complete(&state.provider, &state.db, &state.sys, &transaction, &code).await {
        Ok(Completed::SignedIn(user)) => {
            tracing::info!(
                user_id = %user.user_id,
                provisioned = user.provisioned,
                at = %Utc::now(),
                "SSO sign-in succeeded"
            );
            issue_console_code(&state, &request_json, user).await
        }
        Ok(Completed::RetryWithPromptLogin) => {
            tracing::info!(
                reason = "mfa_claim_missing_retrying_with_prompt_login",
                "SSO sign-in is being retried once"
            );
            let secrets = AuthnSecrets::generate();
            match begin_upstream(&state, &secrets, &request_json, true).await {
                Ok(location) => redirect(&location),
                Err(error) => {
                    tracing::error!(%error, "SSO retry could not be started");
                    console_error_redirect(&request_json, "temporarily_unavailable")
                }
            }
        }
        Err(SsoError::Refused(refusal)) => {
            tracing::warn!(reason = refusal.reason(), "SSO sign-in refused");
            console_error_redirect(&request_json, "access_denied")
        }
        Err(SsoError::Unavailable(what)) => {
            tracing::error!(what, "SSO sign-in could not be evaluated");
            console_error_redirect(&request_json, "temporarily_unavailable")
        }
    }
}

/// The last step, identical to the password path's: the same `Identity`
/// shape into the same unmodified `handle_authorize`, now with the real
/// store, so the console gets a real authorization code.
async fn issue_console_code(
    state: &SsoState,
    request_json: &Value,
    user: store::ResolvedUser,
) -> Response {
    let authorize_request: AuthorizeRequest = match serde_json::from_value(request_json.clone()) {
        Ok(request) => request,
        Err(error) => {
            tracing::error!(%error, "SSO callback: the stored authorize request no longer deserializes");
            return plain(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Sign-in could not be completed.",
            );
        }
    };
    let identity = Identity {
        provider_id: "sms-console".to_owned(),
        external_id: user.user_id,
        email: Some(user.email),
        username: Some(user.display_name),
        attributes: HashMap::new(),
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
        AuthorizeOutcome::Redirect(location) => redirect(&location),
        AuthorizeOutcome::DirectError(error) => {
            // The console client or its redirect changed between `start` and
            // now. Nothing trustworthy to redirect to.
            tracing::error!(%error, "SSO callback: handle_authorize refused the stored request");
            plain(StatusCode::BAD_REQUEST, "Sign-in could not be completed.")
        }
    }
}

/// The SSO routes, already `.with_state(...)`.
// No `#[must_use]`: axum's `Router` already carries one - same reasoning as
// `login::router`.
pub fn router(
    db: Cratestack,
    sys: CratestackContext,
    op: OpState,
    provider: SsoProvider,
) -> Router {
    let state = SsoState {
        db,
        sys,
        op,
        provider,
        limiter: Arc::new(InMemoryRateLimitStore::new()),
    };
    Router::new()
        .route(START_PATH, get(start_handler))
        .route(CALLBACK_PATH, get(callback_handler))
        .with_state(state)
}

/// Deletes expired, never-consumed transactions every minute. Never
/// returns; dropped with the runtime on shutdown, like the other background
/// loops in this binary. Several replicas may run it: a delete that loses
/// the race is not an error.
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
    use serde_json::json;

    use super::*;

    fn query(redirect_uri: Option<&str>) -> StartQuery {
        StartQuery {
            client_id: Some("sms-console".to_owned()),
            redirect_uri: redirect_uri.map(str::to_owned),
            response_type: Some("code".to_owned()),
            scope: Some("openid profile".to_owned()),
            state: Some("s".to_owned()),
            nonce: Some("n".to_owned()),
            code_challenge: Some("c".to_owned()),
            code_challenge_method: Some("S256".to_owned()),
        }
    }

    #[test]
    fn a_complete_request_becomes_the_json_an_authorize_request_reads() {
        let json = console_request_json(query(Some("https://admin.example/api/auth/callback")))
            .expect("complete");
        let request: AuthorizeRequest = serde_json::from_value(json).expect("deserializes");
        assert_eq!(request.client_id, "sms-console");
        assert_eq!(request.state.as_deref(), Some("s"));
        assert_eq!(request.nonce.as_deref(), Some("n"));
        assert_eq!(request.code_challenge_method.as_deref(), Some("S256"));
    }

    #[test]
    fn a_missing_or_oversized_parameter_is_refused_before_any_row_is_written() {
        assert!(console_request_json(query(None)).is_none());
        let mut huge = query(Some("https://admin.example/cb"));
        huge.state = Some("x".repeat(MAX_PARAM_LEN + 1));
        assert!(console_request_json(huge).is_none());
    }

    #[test]
    fn the_error_redirect_carries_the_consoles_own_state_and_nothing_else() {
        let request =
            json!({"redirect_uri": "https://admin.example/api/auth/callback", "state": "abc"});
        let response = console_error_redirect(&request, "access_denied");
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response.headers()[header::LOCATION].to_str().unwrap();
        assert_eq!(
            location,
            "https://admin.example/api/auth/callback?error=access_denied&state=abc"
        );
    }
}
