#![doc = include_str!("metrics.md")]

use cratestack::axum::http::StatusCode;
use cratestack::axum::response::IntoResponse;
use cratestack::axum::routing::get;
use cratestack::axum::Router;

/// A standalone router carrying exactly one route, `GET /metrics` — see
/// this module's own doc for why it is never `.merge()`d into the main
/// public router.
#[must_use = "build this into a listener, or the metrics route serves nothing"]
pub fn router() -> Router {
    Router::new().route("/metrics", get(handler))
}

async fn handler() -> impl IntoResponse {
    match sms_metrics::render() {
        Ok(body) => (StatusCode::OK, body),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to render metrics: {error}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use cratestack::axum::body::Body;
    use cratestack::axum::http::Request;
    use tower::ServiceExt as _;

    use super::router;

    #[tokio::test]
    async fn metrics_route_returns_200_and_prometheus_text() {
        sms_metrics::SINGLETON_LEASE_HELD
            .with_label_values(&["dispatch"])
            .set(1);

        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .expect("building the request"),
            )
            .await
            .expect("the router never fails synchronously");

        assert_eq!(response.status(), 200);
        let body = cratestack::axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("reading the response body");
        let text = String::from_utf8(body.to_vec()).expect("prometheus text is UTF-8");
        assert!(text.contains("sms_worker_singleton_lease_held"), "{text}");
    }

    #[tokio::test]
    async fn a_route_other_than_metrics_is_not_mounted() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("building the request"),
            )
            .await
            .expect("the router never fails synchronously");
        assert_eq!(response.status(), 404);
    }
}
