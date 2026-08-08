//! Unauthenticated liveness route for container/orchestrator health probes
//! ([#139](https://github.com/vymalo/vsms/issues/139) — this repo shipped
//! with no health endpoint for `sms-gateway` at all; `admin/app/api/health`
//! was the only one that existed).
//!
//! Deliberately a liveness check, not a readiness one: it proves the HTTP
//! listener is bound and the async runtime is scheduling tasks, nothing
//! more. It does not touch the database — a DB outage should surface as
//! *that* failing (§9.1's own metrics cover it), not as this container
//! getting killed and restarted by an orchestrator that can't fix a
//! Postgres outage anyway. Mounted the same way `op::router` and
//! `dlr::router` are — merged onto the top-level `Router` outside
//! `sms_api::router`'s own `GatewayAuth` layer — because a health probe
//! carries no bearer token any more than a provider webhook does.

use axum::routing::get;
use axum::Router;

/// `GET /healthz` → `200 OK`, body `"ok"`. No request state, no database
/// handle — there is nothing here for a probe to see beyond "the process
/// answered."
pub fn router() -> Router {
    Router::new().route("/healthz", get(|| async { "ok" }))
}
