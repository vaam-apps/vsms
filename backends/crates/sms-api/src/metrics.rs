//! `GET /metrics` — Prometheus text exposition. #70/#71.
//!
//! # Deliberately never merged into [`crate::router::router`]
//!
//! `health.rs`'s own module doc (`backends/apps/sms-gateway/src/health.rs`) already
//! makes this argument once for `/readyz`'s bare `SELECT 1`: an
//! unauthenticated route on the router `deploy/Caddyfile` reverse-proxies
//! wholesale (`reverse_proxy sms-gateway:8080`, no per-path allowlist —
//! confirmed by reading that file, not assumed) is public the moment it
//! exists, whether or not that was the intent. `/metrics` is a sharper
//! version of the same exposure: it is not a `DoS` amplifier the way a raw
//! `SELECT 1` could be, but it hands out `sms_dispatch_in_flight_submits`,
//! `sms_sm001_total`'s from/to-state breakdown, and every other
//! operational signal in [`sms_metrics`] to anyone who can reach the
//! public domain — reconnaissance value for an attacker, and simply not
//! this deployment's business to publish.
//!
//! [`router`] builds a **second, standalone** `Router`, meant to be bound
//! to its own listener on its own address (`backends/apps/sms-gateway/src/main.rs`'s
//! `--metrics-listen`, default `127.0.0.1:9090` — loopback-only unless an
//! operator explicitly widens it, the same "never faces the internet by
//! default" posture `main.rs`'s own `--listen` default already documents
//! for the main API port). `deploy/docker-compose.yml`'s `sms-gateway`
//! service does not publish this port to the host, and
//! `deploy/Caddyfile`'s blanket `reverse_proxy sms-gateway:8080` never
//! reaches it either, since it is a different port entirely — a real
//! Prometheus server reaches it over the compose network's own internal
//! DNS (`sms-gateway:9090`), the same way `admin` reaches the main API
//! port today. `backends/apps/sms-worker`'s own `main.rs` binds [`router`] to its own
//! second listener too (`--metrics-listen`, default `127.0.0.1:9091`) —
//! reused directly rather than duplicated, since `backends/apps/sms-worker` already
//! depends on `sms-api` (for the schema types every claim loop needs) and
//! the route is identical either way: one `GET /metrics`, no state.

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
