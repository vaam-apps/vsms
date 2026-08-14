//! Mounting the raw `POST /login` route (#194).
//!
//! Not CrateStack-routed, for the same structural reason `dlr.rs`'s own
//! route isn't: a login attempt carries no bearer token to validate
//! against `GatewayAuth` — proving who the caller is is the entire point
//! of this route.
//!
//! # What this is, precisely
//!
//! `authkestra_op::handlers::authorize::handle_authorize` needs an already-
//! established `authkestra_engine::auth::state::Identity` before it will
//! issue an authorization code — it has no login form of its own, and
//! §4.3/#194 chose local password auth over federating to an external `IdP`
//! (see `sms_auth::login`'s own module doc for the full reasoning). So this
//! route collapses what a spec-shaped OIDC deployment would split across a
//! `GET /authorize` redirect and a separately-hosted login page into one
//! step: the caller (`admin`'s own `/login` page and `/api/auth/login`
//! route handler — never a browser talking to this route directly) submits
//! credentials *and* the full `AuthorizeRequest` shape in one POST, and
//! this handler does both jobs — verify the password, then run the real
//! `handle_authorize` — in one call.
//!
//! **Every OAuth2/OIDC security property `#194` requires is still enforced
//! by the real library code, not reimplemented here:** PKCE (S256 only),
//! `redirect_uri` exact-match, `response_type`, grant-type admission, and
//! authorization-code issuance are all `handle_authorize`'s own logic
//! (`authkestra_op::handlers::authorize`), unmodified. This route's own
//! job is exactly one thing `handle_authorize` cannot do for itself:
//! decide *who the caller is* before calling it.
//!
//! `state`/`nonce` pass through untouched to `handle_authorize`/the
//! resulting authorization code — this route neither generates nor
//! verifies either; that's `admin`'s own job, both ends (see
//! `frontends/apps/admin/app/login/page.tsx` for where they're minted and
//! `frontends/apps/admin/app/api/auth/callback/route.ts` for where they're checked against
//! the redirect this route ultimately returns).

use authkestra_engine::auth::state::Identity;
use authkestra_op::handlers::authorize::{handle_authorize, AuthorizeOutcome, AuthorizeRequest};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use cratestack::CoolContext;
use serde::{Deserialize, Serialize};
use sms_api::schema::Cratestack;

use crate::op::OpState;

#[derive(Clone)]
struct LoginState {
    db: Cratestack,
    sys: CoolContext,
    op: OpState,
}

/// The full `AuthorizeRequest` shape plus credentials, in one body — see
/// this module's own doc for why. Field names match `admin`'s own
/// `oidcTxn`/login-form shape exactly, not `AuthorizeRequest`'s Rust names,
/// since this is the wire contract between the two.
#[derive(Debug, Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
    #[serde(rename = "clientId")]
    client_id: String,
    #[serde(rename = "redirectUri")]
    redirect_uri: String,
    #[serde(rename = "responseType")]
    response_type: String,
    scope: String,
    state: String,
    #[serde(rename = "codeChallenge")]
    code_challenge: String,
    #[serde(rename = "codeChallengeMethod")]
    code_challenge_method: String,
    nonce: String,
}

#[derive(Debug, Serialize)]
struct LoginResponse {
    /// Where `admin`'s own `/api/auth/login` route handler should send the
    /// browser next — `{redirect_uri}?code=...&state=...` on success. Never
    /// followed by this process itself: this is a JSON API, not a redirect
    /// response, because the caller is `admin`'s own server, not a browser.
    redirect: String,
}

#[derive(Debug, Serialize)]
struct LoginError {
    error: &'static str,
}

async fn login_handler(
    State(state): State<LoginState>,
    Json(request): Json<LoginRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let authenticated = match sms_auth::login::authenticate_user(
        &state.db,
        &state.sys,
        &request.email,
        &request.password,
    )
    .await
    {
        Ok(authenticated) => authenticated,
        Err(sms_auth::login::LoginError::InvalidCredentials) => {
            return error_response(StatusCode::UNAUTHORIZED, "invalid_credentials");
        }
        Err(sms_auth::login::LoginError::RoleNotFound { subject, role_key }) => {
            // An operator-visible data-integrity problem, not a
            // credentials one — see LoginError's own doc. Logged with
            // detail; the caller gets the same opaque shape either way,
            // since "your account is misconfigured" is not
            // meaningfully more useful to an attacker-adjacent caller
            // than "invalid credentials", and this route has no
            // audience that could act on the distinction.
            tracing::error!(subject, role_key, "login: user's role no longer exists");
            return error_response(StatusCode::UNAUTHORIZED, "invalid_credentials");
        }
    };

    // Identity carries only what authkestra_op's own AuthorizationCode
    // needs to survive to token issuance (§4.3's id_token/access_token,
    // via issue_id_token/issue_user_token) — role/perms are deliberately
    // NOT stashed in `attributes`. sms_api::auth::GatewayAuth's own human
    // path re-resolves role/perms from User/Role per request instead (see
    // that module's own doc for why the real authkestra-op library shape
    // ruled out baking them into the token at issuance).
    let identity = Identity {
        provider_id: "sms-console".to_owned(),
        external_id: authenticated.subject,
        email: Some(authenticated.email),
        username: Some(authenticated.display_name),
        attributes: std::collections::HashMap::new(),
    };

    let authorize_request = AuthorizeRequest {
        client_id: request.client_id,
        redirect_uri: request.redirect_uri,
        response_type: request.response_type,
        scope: request.scope,
        state: Some(request.state),
        code_challenge: Some(request.code_challenge),
        code_challenge_method: Some(request.code_challenge_method),
        nonce: Some(request.nonce),
    };

    let config = state.op.config();
    let store = state.op.store();
    match handle_authorize(authorize_request, identity, &config, store.as_ref()).await {
        AuthorizeOutcome::Redirect(url) => (
            StatusCode::OK,
            Json(serde_json::to_value(LoginResponse { redirect: url }).unwrap_or_default()),
        ),
        // A client_id/redirect_uri/PKCE-shape problem — this deployment's
        // own console client is the only thing that should ever hit this
        // path, so a DirectError here means misconfiguration
        // (`seed-console-client` output not matching `admin`'s own env),
        // not an attacker. Still never echoes the library's own error
        // detail to the caller — see error_response's own doc.
        AuthorizeOutcome::DirectError(error) => {
            tracing::warn!(%error, "login: handle_authorize refused the request directly");
            error_response(StatusCode::BAD_REQUEST, "invalid_request")
        }
    }
}

/// A deliberately narrow, fixed error vocabulary — never
/// `authkestra_op::OpError`'s own `Display`, and never
/// `sms_auth::login::LoginError`'s own message text either, past what's
/// already logged server-side. This route sits in front of a password
/// check; leaking *why* a login failed (no such account vs. wrong
/// password vs. a library-internal detail) is exactly the class of
/// information disclosure `sms_auth::login`'s own module doc already
/// closed at the `authenticate_user` layer — this is that same posture
/// carried through to the HTTP response.
fn error_response(
    status: StatusCode,
    error: &'static str,
) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::to_value(LoginError { error }).unwrap_or_default()),
    )
}

/// The login route, already `.with_state(...)` — mergeable directly with
/// the rest of the app's already-stated `Router`s, matching `dlr::router`'s
/// own shape.
// No `#[must_use]`: axum's `Router` already carries one — same reasoning as
// `sms_api::router`'s and `op::router`'s own doc comments on this.
pub fn router(db: Cratestack, sys: CoolContext, op: OpState) -> Router {
    let state = LoginState { db, sys, op };
    Router::new()
        .route("/login", post(login_handler))
        .with_state(state)
}
