//! Unauthenticated liveness *and* readiness routes for container/orchestrator
//! probes.
//!
//! `/healthz` predates this module doc's rewrite (#139 — this repo shipped
//! with no health endpoint for `sms-gateway` at all; `admin/app/api/health`
//! was the only one that existed) and is unchanged: deliberately a liveness
//! check, not a readiness one. It proves the HTTP listener is bound and the
//! async runtime is scheduling tasks, nothing more. It does not touch the
//! database — a DB outage should surface as *that* failing (§9.1's own
//! metrics cover it), not as this container getting killed and restarted by
//! an orchestrator that can't fix a Postgres outage anyway. **Do not wire a
//! database check into this route** — see #157, which added `/readyz`
//! specifically so this one wouldn't have to change.
//!
//! `/readyz` is new (#157). The gap it closes: boot-time database loss is
//! already well guarded — `sms-gateway serve` hard-fails before it ever
//! binds the listener if it can't load an active signing key or resolve the
//! configured `Provider` row (see `main.rs`'s own comments on both). What
//! was never guarded is *post-boot* database loss — a pool that exhausts or
//! a Postgres that goes away after the process is already serving — and
//! that's the case a long-running process is statistically most likely to
//! hit. Both `deploy/docker-compose.yml`'s health check and
//! `deploy/charts/vsms/values.yaml`'s Helm `readinessProbe` point at this
//! route now, `livenessProbe` stays on `/healthz`.
//!
//! Two deliberate choices on `/readyz`, both direct answers to traps #157
//! names by name:
//!
//! - **The probe is `SELECT 1` on the pool, not a `CrateStack` delegate
//!   call.** No model read here would be free of `@@allow` policy
//!   evaluation, audit-row bookkeeping, or (worse) real table I/O — exactly
//!   the "unauthenticated query amplifier" #157 warns against, on a route
//!   that carries no bearer token and is reachable by anything that can
//!   reach this port. A trivial round-trip on an already-pooled connection
//!   has none of that: it reads no application row, evaluates no policy,
//!   and is the same cost as the connection-pool traffic this process
//!   already generates continuously. That is also why this file joins
//!   R1's three existing named exceptions (DDL/migrations, advisory
//!   locks, `LISTEN`/`NOTIFY`) as a fourth — both in
//!   `cargo xtask no-raw-sqlx`'s allowlist and in `CONTRIBUTING.md`'s own
//!   exceptions table — rather than routing through a delegate: there is
//!   no model this check is about, so there is nothing for R1's
//!   policy/audit/outbox/version guarantees to apply to.
//! - **The query runs under an explicit, short timeout, not the pool's own
//!   (much longer) acquire timeout.** The failure mode this route exists to
//!   catch — an exhausted pool — is exactly the scenario where an
//!   unbounded `.fetch_one(pool)` would itself hang for the pool's full
//!   `acquire_timeout` (30s, `sqlx`'s own default; `Serve` never overrides
//!   it) before answering at all, which would make the probe's own
//!   `timeoutSeconds` the thing that fails it — accurate, but slow and
//!   opaque to whoever's reading probe logs. [`READINESS_TIMEOUT`] gives an
//!   explicit, fast "not ready" instead, well inside every
//!   `timeoutSeconds` this route is configured with in either deploy path.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
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
