#![doc = include_str!("token_rate_limit.md")]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use cratestack::CratestackError;
use cratestack::ratelimit::{
    InMemoryRateLimitStore, RateLimitConfig, RateLimitDecision, RateLimitStore,
};

/// Real `/token` bodies are tiny — a handful of `application/
/// x-www-form-urlencoded` fields plus one RS256-signed `client_assertion`
/// JWT, a few hundred bytes even for a 2048-bit key. 64 KiB is generous
/// headroom over any real request and small enough that buffering it to
/// read `client_id` cannot itself become a memory-exhaustion vector — the
/// exact class of problem `deploy/Caddyfile`'s own comment on Caddy's
/// `{http.request.body}` placeholder warns "inefficient; use only for
/// debugging" about.
const MAX_TOKEN_BODY_BYTES: usize = 64 * 1024;

/// #168's own budget — see `main.rs`'s `--token-rate-limit-burst`/
/// `--token-rate-limit-refill-per-second` for the operator-facing knobs
/// this feeds, and their own doc for the reasoning behind the default
/// (mirrors `deploy/Caddyfile`'s `token_per_ip` figure, 20 events/minute,
/// off the same 15-minute-token-TTL caching behaviour).
#[must_use]
pub fn default_token_rate_limit_config() -> RateLimitConfig {
    RateLimitConfig::new(10, 20.0 / 60.0)
}

/// State for [`enforce_token_client_rate_limit`]. Its own store, never
/// shared with `sms_api::router`'s two `RateLimitLayer`s — this route sits
/// entirely outside that router (see `op.rs`'s own doc for why `/token` is
/// merged in as a sibling, never wrapped by it), so there is no shared
/// key-namespace to protect and no reason to couple the budgets.
#[derive(Clone)]
pub struct TokenRateLimitState {
    store: Arc<dyn RateLimitStore>,
    config: RateLimitConfig,
}

impl TokenRateLimitState {
    #[must_use]
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            store: Arc::new(InMemoryRateLimitStore::new()),
            config,
        }
    }
}

/// The `client_id` field out of a `application/x-www-form-urlencoded`
/// body, exactly as `authkestra_axum::op::axum_token_handler`'s own `Form`
/// extractor will parse it a moment later. `form_urlencoded::parse` never
/// fails on malformed input (unlike `serde_urlencoded`, which would need a
/// target struct); it just may not yield a `client_id` pair, which the
/// caller treats the same as an empty one.
fn client_id_from_form_body(bytes: &[u8]) -> Option<String> {
    form_urlencoded::parse(bytes)
        .find(|(key, _)| key == "client_id")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
}

fn throttled_response(retry_after_secs: u32) -> Response {
    let mut response = (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
    if let Ok(value) = header::HeaderValue::from_str(&retry_after_secs.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// Mirrors `cratestack_axum::ratelimit::RateLimitService`'s own `Err` arm
/// exactly (`crates/cratestack-axum/src/ratelimit/layer.rs`, pinned
/// `=0.7.8`) — a store error (in practice, `InMemoryRateLimitStore`'s
/// mutex poisoned by an earlier panic while held) fails **closed**, using
/// the error's own status code, the same as `sms_api::router`'s two
/// `RateLimitLayer`s already do. Consistency with that established
/// precedent, not a fresh design decision.
fn store_error_response(error: &CratestackError) -> Response {
    tracing::warn!(%error, "token client_id rate limit store error");
    let mut response = Response::new(Body::from(error.public_message().into_owned()));
    *response.status_mut() = error.status_code();
    response
}

/// Buffers the request body (bounded — see [`MAX_TOKEN_BODY_BYTES`]),
/// reads `client_id` out of it, consumes one token from that `client_id`'s
/// own bucket (or a shared fallback bucket if no `client_id` could be
/// read — see below), and reconstructs the request with the *original*
/// body bytes before calling `next` — the real handler downstream sees an
/// intact, never-consumed body regardless of which branch this function
/// takes.
///
/// **A body this function cannot fully read (oversized, or a read error)
/// still gets rate-limited, under a shared `"oversized-or-unreadable"`
/// bucket** — not skipped — so a caller can't dodge this layer by sending
/// a body this function refuses to buffer. What such a request forwards
/// downstream is necessarily an empty body (there is nothing intact left
/// to reconstruct from); the real handler answers its own ordinary parse
/// error for it, same as it would for any other malformed `/token`
/// request. This never happens for a real exchange — see
/// [`MAX_TOKEN_BODY_BYTES`]'s own doc for why 64 KiB is nowhere near a
/// genuine request's size.
///
/// **No `client_id` field present in an otherwise-readable body** shares a
/// different fixed bucket, `"no-client-id"` — again bucketed, not
/// bypassed, and again distinguishable in logs/metrics from the
/// oversized/unreadable case above.
pub async fn enforce_token_client_rate_limit(
    State(state): State<TokenRateLimitState>,
    req: Request,
    next: Next,
) -> Response {
    let (parts, body) = req.into_parts();
    let (key, rebuilt_body) = match to_bytes(body, MAX_TOKEN_BODY_BYTES).await {
        Ok(bytes) => {
            let key = client_id_from_form_body(&bytes).map_or_else(
                || "no-client-id".to_owned(),
                |client_id| format!("client:{client_id}"),
            );
            (key, Body::from(bytes))
        }
        Err(_) => ("oversized-or-unreadable-body".to_owned(), Body::empty()),
    };

    match state.store.consume(&key, state.config).await {
        Ok(RateLimitDecision::Allowed { .. }) => {
            let rebuilt = Request::from_parts(parts, rebuilt_body);
            next.run(rebuilt).await
        }
        Ok(RateLimitDecision::Throttled { retry_after_secs }) => {
            throttled_response(retry_after_secs)
        }
        Err(error) => store_error_response(&error),
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::to_bytes as read_body;
    use axum::routing::post;
    use tower::ServiceExt;

    use super::*;

    fn test_router(config: RateLimitConfig) -> Router {
        let state = TokenRateLimitState::new(config);
        Router::new()
            .route(
                "/token",
                post(|body: axum::body::Bytes| async move {
                    // Echoes the exact bytes it received, so the test
                    // suite can assert the real body reached here
                    // unmodified — the specific failure mode this
                    // middleware's own doc warns against.
                    body
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state,
                enforce_token_client_rate_limit,
            ))
    }

    fn form_request(body: &str) -> Request {
        Request::builder()
            .method("POST")
            .uri("/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body.to_owned()))
            .expect("building a minimal test request")
    }

    #[test]
    fn client_id_from_form_body_reads_the_field() {
        assert_eq!(
            client_id_from_form_body(b"grant_type=client_credentials&client_id=appc_abc123"),
            Some("appc_abc123".to_owned())
        );
    }

    #[test]
    fn client_id_from_form_body_is_none_when_absent_or_empty() {
        assert_eq!(
            client_id_from_form_body(b"grant_type=client_credentials"),
            None
        );
        assert_eq!(client_id_from_form_body(b"client_id="), None);
        assert_eq!(client_id_from_form_body(b""), None);
    }

    #[tokio::test]
    async fn the_real_body_reaches_the_inner_handler_unmodified() {
        // #168's own explicit warning: a /token handler that receives a
        // consumed body breaks every real exchange. Proven directly, not
        // assumed: the echo handler above must see byte-for-byte the same
        // body this test sent, having passed through this middleware.
        let body = "grant_type=client_credentials&client_id=appc_real&client_assertion=abc.def.ghi";
        let router = test_router(RateLimitConfig::new(100, 100.0));
        let response = router
            .oneshot(form_request(body))
            .await
            .expect("calling the test router");
        assert_eq!(response.status(), StatusCode::OK);
        let echoed = read_body(response.into_body(), 1024)
            .await
            .expect("reading the echoed body");
        assert_eq!(echoed, body.as_bytes());
    }

    #[tokio::test]
    async fn a_burst_of_the_same_client_id_is_throttled() {
        let router = test_router(RateLimitConfig::new(3, 0.01));
        let mut statuses = Vec::new();
        for _ in 0..5 {
            let response = router
                .clone()
                .oneshot(form_request(
                    "grant_type=client_credentials&client_id=appc_target",
                ))
                .await
                .expect("calling the test router");
            statuses.push(response.status().as_u16());
        }
        assert_eq!(
            statuses,
            vec![200, 200, 200, 429, 429],
            "repeated requests for the same client_id must exhaust its own \
             bucket regardless of how many separate connections/requests \
             they arrive as, got {statuses:?}"
        );
    }

    #[tokio::test]
    async fn a_different_client_id_has_its_own_independent_bucket() {
        let router = test_router(RateLimitConfig::new(1, 0.01));
        // Exhaust appc_a's single-token bucket.
        let first = router
            .clone()
            .oneshot(form_request(
                "grant_type=client_credentials&client_id=appc_a",
            ))
            .await
            .expect("calling the test router");
        assert_eq!(first.status(), StatusCode::OK);
        let second = router
            .clone()
            .oneshot(form_request(
                "grant_type=client_credentials&client_id=appc_a",
            ))
            .await
            .expect("calling the test router");
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);

        // A genuinely different client_id, sent immediately after, must be
        // completely unaffected by appc_a's own exhausted bucket.
        let other = router
            .oneshot(form_request(
                "grant_type=client_credentials&client_id=appc_b",
            ))
            .await
            .expect("calling the test router");
        assert_eq!(
            other.status(),
            StatusCode::OK,
            "a different client_id must not share appc_a's throttled bucket"
        );
    }

    #[tokio::test]
    async fn requests_with_no_client_id_still_share_a_throttled_bucket() {
        // Not skipped — a caller can't dodge this layer by omitting
        // client_id, since GatewayAuth/authkestra will reject the request
        // anyway; bucketing it (rather than letting it through free) keeps
        // that rejection path itself from becoming an amplifier.
        let router = test_router(RateLimitConfig::new(1, 0.01));
        let first = router
            .clone()
            .oneshot(form_request("grant_type=client_credentials"))
            .await
            .expect("calling the test router");
        assert_eq!(first.status(), StatusCode::OK);
        let second = router
            .oneshot(form_request("grant_type=client_credentials"))
            .await
            .expect("calling the test router");
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
