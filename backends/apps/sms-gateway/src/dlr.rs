#![doc = include_str!("dlr.md")]

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use cratestack::CratestackContext;
use sms_api::schema::Cratestack;
use sms_provider::{ProviderError, RawCallback, SmsProvider};
use tracing::warn;

/// One configured adapter's own DLR-ingestion identity: the [`SmsProvider`]
/// itself (to call [`SmsProvider::parse_dlr`] against) and the `Provider`
/// row id [`sms_api::dlr::ingest`] needs to stamp onto the resulting
/// `DeliveryReceipt`/`Message` writes. [`router`] keys a caller-built
/// collection of these by each adapter's own [`SmsProvider::key`] rather
/// than trusting a caller to key it correctly by hand — the same
/// "derive the key from the value, don't ask the caller to keep two things
/// in sync" reasoning `backends/apps/sms-worker/src/main.rs`'s
/// `build_provider_registry` already applies to its own provider map.
pub struct DlrProvider {
    pub provider: Arc<dyn SmsProvider>,
    pub provider_row_id: String,
}

#[derive(Clone)]
struct DlrState {
    db: Cratestack,
    sys: CratestackContext,
    /// Keyed by each adapter's own [`SmsProvider::key`] — the
    /// `{providerKey}` path segment is looked up here directly rather than
    /// compared against one hardcoded provider, so this route now serves
    /// however many adapters `serve_command` configured (#61's MTN
    /// wiring is the first second entry; the shape supports any number).
    /// `Arc`, not owned, so `Clone` (required by axum's `State` extractor,
    /// cloned per request) doesn't deep-copy the map on every callback.
    providers: Arc<HashMap<String, DlrProvider>>,
}

async fn dlr_handler(
    Path(provider_key): Path<String>,
    State(state): State<DlrState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // A key that matches no configured adapter is the only 404 case now —
    // previously this compared against exactly one hardcoded provider, so
    // any *other* real key 404'd even if it were configured, making
    // `{providerKey}` decorative for every deployment with only one
    // adapter wired up. With N configured adapters, "not in the map" is
    // the only thing "not one of ours" can mean.
    let Some(entry) = state.providers.get(&provider_key) else {
        return StatusCode::NOT_FOUND;
    };

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
        entry.provider.as_ref(),
        &entry.provider_row_id,
        &raw,
    )
    .await
    {
        // Orange's own published contract
        // (https://developer.orange.com/apis/sms/getting-started, §4)
        // states the partner endpoint "must return an HTTP 200 OK in
        // order to acknowledge the receipt" and shows `HTTP/1.1 200 OK`
        // as the worked example response — not `202`. §3.2's own
        // "return 202, never 200" rule is about `sendMessage`, this API's
        // *outbound* send endpoint ("you have not sent anything yet");
        // it does not apply here, where the request being acknowledged
        // is an *inbound* provider webhook reporting something that
        // already happened. `docs/architecture.md` §3.2/§4.1/§6.2 already
        // record this distinction explicitly (and note this exact rule
        // was once mis-cited the other way to justify `202` here) — this
        // handler previously disagreed with its own design doc.
        Ok(()) => StatusCode::OK,
        // This is the one case `ingest` itself propagates rather than
        // logging and continuing (see its own doc) — a body `parse_dlr`
        // cannot make sense of at all, not one update among several that
        // fails to match.
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
/// rest of the app's already-stated `Router`s. `providers` is keyed by
/// each entry's own [`SmsProvider::key`] internally (see [`DlrProvider`]'s
/// own doc) rather than trusted from the caller's own map keys, so a
/// caller cannot accidentally register an adapter under the wrong path
/// segment.
// No `#[must_use]`: axum's `Router` already carries one — same reasoning
// as `sms_api::router`'s and `op::router`'s own doc comments on this.
pub fn router(db: Cratestack, sys: CratestackContext, providers: Vec<DlrProvider>) -> Router {
    let providers = providers
        .into_iter()
        .map(|entry| (entry.provider.key().to_owned(), entry))
        .collect();
    let state = DlrState {
        db,
        sys,
        providers: Arc::new(providers),
    };
    Router::new()
        .route("/dlr/{providerKey}", post(dlr_handler))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    /// A pool that only ever parses its own URL — `connect_lazy` opens no
    /// connection until something actually queries through it, and a
    /// well-formed DLR body in the tests below deliberately drives it far
    /// enough to try and fail, proving the *success* path (`ingest`
    /// swallowing that failure and returning `Ok(())`, per its own doc)
    /// without a live Postgres anywhere in this crate's own test suite.
    /// Port 1 (a privileged, essentially never-bound port) rather than a
    /// bare hostname with the default 5432, so this can't accidentally
    /// succeed — or hang waiting on a real handshake — against a Postgres
    /// that happens to be running locally for an unrelated live suite.
    fn lazy_db() -> Cratestack {
        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            // sqlx's pool retries a failed connection attempt for its own
            // `acquire_timeout` (30s by default) before giving up — even
            // though the underlying TCP connect to an unbound port refuses
            // near-instantly — so without this, each test below that
            // actually queries through this pool takes ~30s just to reach
            // the `Err` this crate's own `dlr::ingest` is proven to
            // swallow. A short, explicit timeout is enough: these tests
            // only need *an* error, not a realistic one.
            .acquire_timeout(std::time::Duration::from_millis(500))
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/none")
            .expect("a lazy pool only parses the URL, never opens a connection");
        Cratestack::builder(pool).build()
    }

    fn orange_dlr_provider() -> DlrProvider {
        let mut config = sms_provider_orange_cm::OrangeCmConfig::production(
            "test-client-id".to_owned(),
            "test-client-secret".to_owned(),
            "+237677000000".to_owned(),
        );
        // Never dialled in these tests — `parse_dlr` never makes an
        // outbound call — but kept `.invalid` (RFC 2606) regardless of
        // that, on principle.
        config.base_url = "https://api.orange.invalid".to_owned();
        DlrProvider {
            provider: Arc::new(sms_provider_orange_cm::OrangeCmProvider::new(config)),
            provider_row_id: "orange-provider-row-id".to_owned(),
        }
    }

    fn mtn_dlr_provider() -> DlrProvider {
        let config = sms_provider_mtn::MtnAggregatorConfig {
            api_key: "test-api-key".to_owned(),
            sender_id: "SENDER".to_owned(),
            base_url: "https://aggregator.invalid".to_owned(),
            tps_ceiling: 20.0,
            cost_per_segment_xaf: rust_decimal::Decimal::new(15, 0),
            supports_alphanumeric_sender: false,
            connect_timeout: std::time::Duration::from_secs(2),
            request_timeout: std::time::Duration::from_secs(2),
        };
        DlrProvider {
            provider: Arc::new(sms_provider_mtn::MtnAggregatorProvider::new(config)),
            provider_row_id: "mtn-provider-row-id".to_owned(),
        }
    }

    /// Both adapters configured at once — the actual shape #61 wires up in
    /// production, not a one-provider stand-in.
    fn two_provider_router() -> Router {
        router(
            lazy_db(),
            sms_api::system_context("dlr-test"),
            vec![orange_dlr_provider(), mtn_dlr_provider()],
        )
    }

    async fn post_dlr(app: Router, path: &str, body: &str) -> axum::http::StatusCode {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::from(body.to_owned()))
                    .expect("building a minimal test request"),
            )
            .await
            .expect("the router itself never errors — only the handler's own response does");
        response.status()
    }

    /// **Guard-failure proof #1** (this PR's own required proof): before
    /// this PR, `dlr_handler` compared `provider_key` against exactly one
    /// hardcoded `Arc<dyn SmsProvider>` and 404'd everything else — so a
    /// callback to `/dlr/mtn_aggregator` would have 404'd even with MTN
    /// genuinely configured, since only Orange's key could ever match.
    /// Reverting `dlr_handler`'s `state.providers.get(&provider_key)`
    /// lookup to that old single-provider shape and re-running this test
    /// reproduces exactly that 404; restoring the map-based lookup
    /// restores this pass. `400`, not `404`, proves the key *was* found —
    /// the request reached MTN's own `parse_dlr`, which then correctly
    /// refuses a body that isn't JSON at all.
    #[tokio::test]
    async fn a_dlr_posted_to_the_mtn_key_reaches_the_mtn_adapter_not_a_404() {
        let status = post_dlr(
            two_provider_router(),
            "/dlr/mtn_aggregator",
            "not json at all",
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_dlr_posted_to_the_orange_key_still_reaches_the_orange_adapter() {
        let status = post_dlr(two_provider_router(), "/dlr/orange_cm", "not json at all").await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_key_matching_no_configured_adapter_is_404() {
        let status = post_dlr(
            two_provider_router(),
            "/dlr/some_unconfigured_provider",
            "not json at all",
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    }

    /// **Guard-failure proof #2** (this PR's own required proof): a
    /// successful ingest answers `200 OK`, not `202 Accepted` — see
    /// `dlr_handler`'s own `Ok(()) => StatusCode::OK` comment for why.
    /// Reverting that one arm back to `StatusCode::ACCEPTED` and
    /// re-running this test reproduces a real, direct assertion failure
    /// (`left: 202, right: 200`); restoring `StatusCode::OK` restores this
    /// pass. This exercises the genuine success path — a well-formed body
    /// `parse_dlr` accepts, producing at least one update — not a
    /// shortcut: `ingest_one`'s own database call fails against this
    /// test's deliberately unreachable pool, but `sms_api::dlr::ingest`
    /// logs and swallows a per-update database error rather than
    /// propagating it (see that function's own doc comment), so `Ok(())`
    /// reaches the handler the same way it would after a real write
    /// succeeds.
    #[tokio::test]
    async fn a_successfully_ingested_mtn_dlr_answers_200_not_202() {
        let status = post_dlr(
            two_provider_router(),
            "/dlr/mtn_aggregator",
            r#"{"messageId":"mtn-ref-1","status":"DELIVERED"}"#,
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn a_successfully_ingested_orange_dlr_answers_200_not_202() {
        let status = post_dlr(
            two_provider_router(),
            "/dlr/orange_cm",
            r#"{"deliveryInfoNotification":{"callbackData":"orange-ref-1","deliveryInfo":{"deliveryStatus":"DeliveredToTerminal"}}}"#,
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
    }
}
