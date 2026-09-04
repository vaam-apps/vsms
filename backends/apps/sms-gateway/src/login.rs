#![doc = include_str!("login.md")]

use authkestra_engine::auth::state::Identity;
use authkestra_op::handlers::authorize::{AuthorizeOutcome, AuthorizeRequest, handle_authorize};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use cratestack::CratestackContext;
use serde::{Deserialize, Serialize};
use sms_api::schema::Cratestack;

use crate::op::OpState;

#[derive(Clone)]
struct LoginState {
    db: Cratestack,
    sys: CratestackContext,
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

/// `AuthorizeRequest` is `#[non_exhaustive]` as of authkestra-op 0.8.0 (see
/// AGENTS.md's authkestra-0.8 section, item A7), so the struct literal this
/// used to build no longer compiles from outside the crate (E0639: cannot
/// construct a non-exhaustive struct using struct-literal syntax). No
/// public constructor or builder exists for it — checked directly against
/// the vendored 0.8.0 source, not assumed — but the type still derives
/// `serde::Deserialize` (it's designed to be deserialized straight off an
/// inbound `/authorize` request in the first place), so building the wire
/// shape by hand and deserializing it is the construction path the type
/// itself leaves open. This is a real behavioural difference worth being
/// honest about, not just a syntax workaround: a future authkestra release
/// narrowing or retyping a field used to be a compile error here (a struct
/// literal with the wrong field types fails to build); it is now a runtime
/// `Err` on the very next login attempt instead, caught by
/// `login_flow_live_postgres.rs`'s live suite rather than by `cargo check`.
///
/// **That `Err` only fires for a required field.** `AuthorizeRequest`
/// carries no `#[serde(deny_unknown_fields)]`, and four of its eight
/// fields (`state`, `code_challenge`, `code_challenge_method`, `nonce`)
/// are `Option<String>` — plain serde behaviour treats a missing key as
/// `None` for an `Option` field, not an error, and `scope` carries its own
/// `#[serde(default)]` with the identical silent-empty-string shape. So a
/// future authkestra release that renames, say, `nonce` to `oidcNonce`
/// would not fail this deserialization at all: it would silently produce
/// `nonce: None` instead, `Ok`, and this handler would carry on issuing a
/// token whose `id_token` never carries the nonce the caller sent — a
/// class of bug this crate's own `cargo check` and this very `Result`
/// handling are both structurally blind to, since nothing here is wrong
/// from Rust's point of view. Only `client_id`/`redirect_uri`/
/// `response_type` (plain, non-`Option`, no `#[serde(default)]`) fail
/// loudly on a rename. Two things stand between this and going unnoticed
/// in production: `tests::the_authorize_request_json_keeps_its_optional_
/// fields_present` below, a fast unit test with no database that asserts
/// this exact JSON shape deserializes with `state`/`code_challenge`/
/// `nonce` all `Some` (so a rename shows up the moment this crate is
/// built, not just the moment a login happens against a live OP); and
/// `login_flow_live_postgres.rs`'s own `nonce`/`state` round-trip
/// assertions, which read the values back off the real redirect and
/// `id_token` rather than trusting this function's own `Ok` at all.
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

    let authorize_request: AuthorizeRequest = match build_authorize_request(&request) {
        Ok(authorize_request) => authorize_request,
        Err(error) => {
            tracing::error!(
                %error,
                "login: building AuthorizeRequest from the login form's own fields failed"
            );
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
        }
    };

    let config = state.op.config();
    // An owned, mutable store — not `Arc<dyn OpStore>` any more.
    // `handle_authorize` takes `&mut dyn OpStore` as of authkestra-op
    // 0.8.0 (every `OpStore` method does — AGENTS.md item A1), and an
    // `Arc` only ever gives shared access to what it points at. See
    // `OpState::store`'s own doc for why `Box<dyn OpStore>` from a fresh
    // clone is the right shape here.
    let mut store = state.op.store();
    match handle_authorize(authorize_request, identity, &config, store.as_mut()).await {
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
pub fn router(db: Cratestack, sys: CratestackContext, op: OpState) -> Router {
    let state = LoginState { db, sys, op };
    Router::new()
        .route("/login", post(login_handler))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cheap, no-database half of the guard `build_authorize_request`'s
    /// own doc comment names: proves the fields most exposed to a silent
    /// rename (the four `Option<String>` ones this handler's own JSON
    /// shape feeds) actually arrive `Some` for realistic input, so a
    /// future authkestra rename that would otherwise deserialize silently
    /// to `None` fails *this* assertion the moment the workspace is built,
    /// not just the moment a live login runs against a real OP. Doesn't
    /// prove authkestra's own field names haven't drifted — only a live
    /// suite against the real `AuthorizeRequest` type can prove that (see
    /// `login_flow_live_postgres.rs`'s own `nonce`/`state` round-trip
    /// assertions) — this proves this function's own JSON construction
    /// keeps sending them, which is the half a fast, database-free test
    /// can actually check.
    #[test]
    fn the_authorize_request_json_keeps_its_optional_fields_present() {
        let request = LoginRequest {
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
        };

        let authorize_request =
            build_authorize_request(&request).expect("this hand-built JSON always deserializes");

        assert!(
            authorize_request.state.is_some(),
            "state must survive the AuthorizeRequest rebuild"
        );
        assert!(
            authorize_request.code_challenge.is_some(),
            "code_challenge must survive the AuthorizeRequest rebuild"
        );
        assert!(
            authorize_request.nonce.is_some(),
            "nonce must survive the AuthorizeRequest rebuild"
        );
        assert_eq!(authorize_request.state.as_deref(), Some("state-abc123"));
        assert_eq!(
            authorize_request.code_challenge.as_deref(),
            Some("challenge-abc123")
        );
        assert_eq!(authorize_request.nonce.as_deref(), Some("nonce-abc123"));
    }
}
