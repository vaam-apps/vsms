#![doc = include_str!("health.md")]

use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use sms_api::schema::Cratestack;

/// Wall-clock budget for `/readyz`'s database round-trip. Chosen to sit
/// comfortably under every `timeoutSeconds` this route is configured with
/// (3s in both `deploy/docker-compose.yml` and
/// `deploy/charts/vsms/values.yaml`), so a stalled probe reads as "database
/// not reachable" (this timeout firing, an explicit `503`) rather than "the
/// probe itself timed out" (the orchestrator's own timeout firing, which
/// looks identical to a hung process from outside).
const READINESS_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
struct ReadyState {
    db: Cratestack,
}

/// `db` is the same pooled handle `Serve` builds once at startup and hands
/// to every other router — no second connection, no second pool, just one
/// more consumer of the one this process already holds.
pub fn router(db: Cratestack) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .with_state(ReadyState { db })
}

/// `GET /readyz` → `200 OK` if a trivial query round-trips the pool inside
/// [`READINESS_TIMEOUT`], `503 Service Unavailable` otherwise — a database
/// outage, an exhausted pool, or a round-trip slow enough to be
/// indistinguishable from either.
async fn readyz(State(state): State<ReadyState>) -> (StatusCode, &'static str) {
    // R1 exception — see this module's own doc for why a raw query is
    // correct here rather than a CrateStack delegate call.
    // `cargo xtask no-raw-sqlx` allowlists this file by path.
    let probe = cratestack::sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(state.db.pool());

    match tokio::time::timeout(READINESS_TIMEOUT, probe).await {
        Ok(Ok(_)) => (StatusCode::OK, "ready"),
        Ok(Err(_)) => (StatusCode::SERVICE_UNAVAILABLE, "not ready: database error"),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "not ready: database timeout",
        ),
    }
}
