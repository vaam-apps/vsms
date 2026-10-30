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
    /// The bucket is `op.token_rate_limit` (`sms_config::Op`); the committed
    /// example states #168's budget, 10 burst and 20 events a minute, which
    /// mirrors `deploy/Caddyfile`'s `token_per_ip` figure off the same
    /// 15-minute-token-TTL caching behaviour.
    #[must_use]
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            store: Arc::new(InMemoryRateLimitStore::new()),
            config,
        }
    }
}

/// The value of the one `name` field of a form body, exactly as
/// `authkestra_axum::op::axum_token_handler`'s own `Form` extractor will parse it
/// a moment later. `form_urlencoded::parse` never fails on malformed input
/// (unlike `serde_urlencoded`, which would need a target struct).
///
/// [`Field::Absent`] for none (or an empty one), [`Field::Repeated`] for two or
/// more: the handler's form parser refuses a repeated field, so such a request
/// can only ever be refused, and it must not be keyed as though it carried a
/// real value.
fn form_field(bytes: &[u8], name: &str) -> Field {
    let mut found = form_urlencoded::parse(bytes).filter(|(key, _)| key == name);
    let Some((_, first)) = found.next() else {
        return Field::Absent;
    };
    if found.next().is_some() {
        return Field::Repeated;
    }
    if first.is_empty() {
        Field::Absent
    } else {
        Field::One(first.into_owned())
    }
}

enum Field {
    Absent,
    One(String),
    Repeated,
}

/// The `client_id` field out of a form body: `None` when absent or empty. A
/// repeated one is read as its first value, as this layer always has (the
/// handler refuses the request either way).
fn client_id_from_form_body(bytes: &[u8]) -> Option<String> {
    form_urlencoded::parse(bytes)
        .find(|(key, _)| key == "client_id")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
}

/// The bucket a `/token` request draws on.
///
/// **An `authorization_code` request is keyed on its code, never on its
/// `client_id`.** The only client that redeems a code is the console, a *public*
/// client whose id is public: every operator's redemption would share one
/// `client:sms-console` bucket, and anyone could empty it with
/// `grant_type=authorization_code&client_id=sms-console&code=x` and make every
/// real code (sixty seconds of life) expire behind a `429`. Keyed on the SHA-256
/// of the code, a bad code spends only its own bucket: a real code is a
/// different key, and a 32-byte random code is not something a retry loop guesses,
/// so the per-code budget exists to stop the *same* code being hammered, not to
/// stop the console. A request with no code, or two, shares one bucket per
/// shape, and no real redemption ever lands there.
///
/// Every other grant (`client_credentials`) is keyed on its `client_id` as
/// before, and a body with no readable `client_id` shares `no-client-id`.
fn limit_key(bytes: &[u8]) -> String {
    if let Field::One(grant) = form_field(bytes, "grant_type")
        && grant == AUTHORIZATION_CODE
    {
        return match form_field(bytes, "code") {
            Field::One(code) => format!("code:{}", hex_sha256(code.as_bytes())),
            Field::Absent => "authorization-code:no-code".to_owned(),
            Field::Repeated => "authorization-code:repeated-code".to_owned(),
        };
    }
    client_id_from_form_body(bytes).map_or_else(
        || "no-client-id".to_owned(),
        |client_id| format!("client:{client_id}"),
    )
}

/// The grant whose key is the code, not the client.
const AUTHORIZATION_CODE: &str = "authorization_code";

/// Lowercase hex SHA-256: a code is a credential for sixty seconds, so it is
/// never the store's key itself.
fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
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
        Ok(bytes) => (limit_key(&bytes), Body::from(bytes)),
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
        // client_id, since the OP will reject the request
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
    /// **The lockout this key exists to prevent.** The console's client id is
    /// public, so if a code were limited per `client_id` one attacker spamming
    /// bad codes would 429 every operator's real redemption for as long as they
    /// kept at it. Keyed per code, the spam spends `x`'s bucket and nobody else's.
    #[tokio::test]
    async fn spamming_bad_codes_cannot_throttle_a_real_redemption() {
        let router = test_router(RateLimitConfig::new(3, 0.001));
        let mut spam = Vec::new();
        for _ in 0..40 {
            let response = router
                .clone()
                .oneshot(form_request(
                    "grant_type=authorization_code&client_id=sms-console&code=x",
                ))
                .await
                .expect("calling the test router");
            spam.push(response.status());
        }
        assert_eq!(
            spam.iter()
                .filter(|status| **status == StatusCode::TOO_MANY_REQUESTS)
                .count(),
            37,
            "the spammed code is throttled after its own burst of 3"
        );
        for real in ["a-real-code-one", "a-real-code-two"] {
            let response = router
                .clone()
                .oneshot(form_request(&format!(
                    "grant_type=authorization_code&client_id=sms-console&code={real}\
                     &code_verifier=v&redirect_uri=r"
                )))
                .await
                .expect("calling the test router");
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{real}: a real code has its own bucket, whatever was sent for `x`"
            );
        }
    }

    /// Spamming **many** distinct bad codes is no better for the attacker: each
    /// is its own bucket, so none of them touches the real one.
    #[tokio::test]
    async fn a_stream_of_distinct_bad_codes_does_not_touch_a_real_one() {
        let router = test_router(RateLimitConfig::new(1, 0.001));
        for index in 0..30 {
            let response = router
                .clone()
                .oneshot(form_request(&format!(
                    "grant_type=authorization_code&client_id=sms-console&code=bad-{index}"
                )))
                .await
                .expect("calling the test router");
            assert_eq!(response.status(), StatusCode::OK, "bad-{index}");
        }
        let real = router
            .oneshot(form_request(
                "grant_type=authorization_code&client_id=sms-console&code=the-real-one",
            ))
            .await
            .expect("calling the test router");
        assert_eq!(real.status(), StatusCode::OK);
    }

    /// The same bucket for the same code, however the body spells it, and the
    /// client id plays no part in it.
    #[test]
    fn an_authorization_code_is_keyed_on_the_code_alone() {
        let key = |body: &str| limit_key(body.as_bytes());
        assert_eq!(
            key("grant_type=authorization_code&client_id=sms-console&code=abc"),
            key("code=abc&grant_type=authorization_code&client_id=another"),
        );
        assert_eq!(
            key("grant_type=authorization_code&code=abc"),
            key("grant_type=authorization_code&code=ab%63"),
            "the percent-decoded value is what the handler reads"
        );
        assert_ne!(
            key("grant_type=authorization_code&code=abc"),
            key("grant_type=authorization_code&code=abd")
        );
        assert!(
            !key("grant_type=authorization_code&code=abc").contains("abc"),
            "the code itself is never the store's key"
        );
    }

    /// A request with no code, or two, shares a bucket of its own shape, never
    /// one a real redemption (exactly one code) can land in.
    #[test]
    fn a_missing_or_repeated_code_never_shares_a_real_codes_bucket() {
        let key = |body: &str| limit_key(body.as_bytes());
        let real = key("grant_type=authorization_code&code=abc");
        for odd in [
            "grant_type=authorization_code",
            "grant_type=authorization_code&code=",
            "grant_type=authorization_code&code=abc&code=abc",
            "grant_type=authorization_code&code=abc&code=x",
        ] {
            assert_ne!(key(odd), real, "{odd}");
            assert!(key(odd).starts_with("authorization-code:"), "{odd}");
        }
        assert_ne!(
            key("grant_type=authorization_code"),
            key("grant_type=authorization_code&code=a&code=b")
        );
    }

    /// The machine grant is untouched: still one bucket per `client_id`.
    #[test]
    fn a_client_credentials_request_is_still_keyed_on_its_client_id() {
        assert_eq!(
            limit_key(b"grant_type=client_credentials&client_id=appc_a"),
            "client:appc_a"
        );
        assert_eq!(limit_key(b"grant_type=client_credentials"), "no-client-id");
        // A `code` beside another grant does not turn it into a code request.
        assert_eq!(
            limit_key(b"grant_type=client_credentials&client_id=appc_a&code=abc"),
            "client:appc_a"
        );
    }
}
