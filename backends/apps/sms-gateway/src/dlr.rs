//! Mounting the raw DLR webhook route. #34.
//!
//! `POST /dlr/{providerKey}` — not CrateStack-routed (see
//! `sms_api::dlr`'s own module doc for why: a provider webhook carries no
//! bearer token to validate against `GatewayAuth`). The one access control
//! this route has is the path segment matching the configured provider's
//! own `key()` — everything past that is §9.2's own stated external
//! constraint: Orange will only call a webhook on HTTPS 443 with a
//! CA-signed cert, whitelisted per a manual support ticket. No app-level
//! signature verification is implemented — `RawCallback` already carries
//! the exact, unmodified bytes a future one would need, but no provider's
//! real signature scheme is documented yet to verify against (Orange's
//! own DLR shape is itself unverified against a live sandbox — see
//! `sms-provider-orange-cm`'s own `dlr` module).

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use cratestack::CoolContext;
use sms_api::schema::Cratestack;
use sms_provider::{ProviderError, RawCallback, SmsProvider};
use tracing::warn;

#[derive(Clone)]
struct DlrState {
    db: Cratestack,
    sys: CoolContext,
    provider: Arc<dyn SmsProvider>,
    provider_row_id: String,
}

async fn dlr_handler(
    Path(provider_key): Path<String>,
    State(state): State<DlrState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if provider_key != state.provider.key() {
        return StatusCode::NOT_FOUND;
    }

    let raw = RawCallback {
        headers: headers
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    value.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect(),
        body: body.to_vec(),
    };

    match sms_api::dlr::ingest(
        &state.db,
        &state.sys,
        state.provider.as_ref(),
        &state.provider_row_id,
        &raw,
    )
    .await
    {
        Ok(()) => StatusCode::ACCEPTED,
        // §7's own instruction: "return 202, never 200 — you have not sent
        // anything yet." The inverse holds for a genuinely malformed
        // callback body: this is the one case `ingest` itself propagates
        // rather than logging and continuing (see its own doc) — a body
        // `parse_dlr` cannot make sense of at all, not one update among
        // several that fails to match.
        Err(ProviderError::Rejected { code, message }) => {
            warn!(
                provider_key,
                code, message, "rejecting a malformed DLR callback body"
            );
            StatusCode::BAD_REQUEST
        }
        Err(error) => {
            warn!(provider_key, %error, "DLR callback failed for a reason other than a malformed body");
            StatusCode::BAD_REQUEST
        }
    }
}

/// The DLR route, already `.with_state(...)` — mergeable directly with the
/// rest of the app's already-stated `Router`s.
// No `#[must_use]`: axum's `Router` already carries one — same reasoning
// as `sms_api::router`'s and `op::router`'s own doc comments on this.
pub fn router(
    db: Cratestack,
    sys: CoolContext,
    provider: Arc<dyn SmsProvider>,
    provider_row_id: String,
) -> Router {
    let state = DlrState {
        db,
        sys,
        provider,
        provider_row_id,
    };
    Router::new()
        .route("/dlr/{providerKey}", post(dlr_handler))
        .with_state(state)
}
