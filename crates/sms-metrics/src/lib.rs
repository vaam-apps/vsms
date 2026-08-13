//! Prometheus metric definitions for #70/#71 (M6 — Observability and
//! alerting on the states that are silent by default).
//!
//! # Why this is its own crate
//!
//! `sms-gateway` and `sms-worker` are two separate OS processes (`app/`'s
//! own dependency-arrow rule: a binary may depend on any library, never the
//! reverse, and `app/sms-gateway`/`app/sms-worker` never depend on each
//! other). Each process gets its own independent copy of every
//! [`std::sync::LazyLock`] static in this crate — there is no shared memory
//! between them, and there does not need to be. Prometheus is the
//! aggregation layer: each process exposes its own `/metrics`, a scrape
//! config names both as separate targets, and an alert rule that needs a
//! fleet-wide view (e.g. "is `dispatch`'s lease held *anywhere*") uses
//! `sum(...)` across the scraped instances, not a shared counter inside the
//! application. Putting the metric *definitions* (names, help text, label
//! sets) in one crate that both `sms-api` (for [`record_sm001`], called
//! from `crates/sms-api/src/errors.rs::map_database_error`) and
//! `sms-worker` (for the other four) depend on is what stops the same
//! metric drifting into two slightly different names/labels the way a
//! hand-copied constant would — the same reasoning
//! `crates/sms-worker/src/jobs.rs`'s own backoff-schedule comment gives for
//! not sharing code across the `app`/`crates` dependency boundary, applied
//! to the one case where sharing *is* possible: `sms-worker` already
//! depends on `sms-api`, so both can depend on a third, lower crate without
//! creating a cycle.
//!
//! # Backend: Prometheus text exposition, not OTLP
//!
//! Chosen over OpenTelemetry because this system already runs behind a
//! Caddy edge with no collector infrastructure anywhere in `deploy/`, and a
//! pull-based `/metrics` endpoint needs nothing new to *receive* it — an
//! operator points a Prometheus server (or nothing at all, and reads
//! `/metrics` by hand) at two more targets. OTLP would need a collector
//! process this deployment does not have and #71 does not ask for. The
//! `prometheus` crate (`=0.14`, Apache-2.0, checked against `deny.toml`'s
//! allowed-license list) is used with `default-features = false` —
//! `TextEncoder`/`Registry`/`*Vec` are all available without it; the
//! default `protobuf` feature exists for Prometheus's older binary scrape
//! protocol, which nothing in this deployment's tooling needs, and turning
//! it off keeps this crate's own dependency footprint to the metric types
//! themselves.
//!
//! # The seven metrics, and why each is shaped the way it is
//!
//! - [`SM001_TOTAL`] — §9.1 of the design doc names this "the highest-
//!   signal metric in the list... in a correct system it is flat zero."
//!   Labelled `entity`/`from_state`/`to_state`, parsed out of the
//!   trigger's own `RAISE EXCEPTION` text
//!   (`illegal <entity> transition <from> -> <to> on <id>`,
//!   `schema/migrations/postgres/0002_bootstrap/up.sql`) by
//!   [`record_sm001`] — matching §9.1's own "`SM001` rejection count by
//!   from/to pair" line, not a plain unlabelled counter. See
//!   `crates/sms-api/src/errors.rs`'s own doc for why `map_database_error`
//!   is the single place every write in this workspace funnels an SM001
//!   through before this crate ever sees one.
//! - [`SINGLETON_LEASE_HELD`] — one gauge per `(process, role)`, set to `1`
//!   the instant a lease is acquired and `0` the instant this process is
//!   standing by, released, or fails to attempt the lock — see
//!   `crates/sms-worker/src/lib.rs::run_singleton`, the only writer. The
//!   point of writing `0`, not simply never touching the gauge while
//!   standing by, is the absent-vs-zero distinction #70 calls out as "the
//!   subtlest requirement": a process configured to run a role and merely
//!   not currently holding its lease must show up as a `0` time series
//!   (provably watching, not currently in charge), so
//!   `sum(sms_worker_singleton_lease_held{role="dispatch"}) == 0` is a real
//!   "nobody holds it" signal, not indistinguishable from "nobody is even
//!   trying." A role no process was ever configured to run (a typo'd
//!   `--roles`, or the entire fleet down) never touches this gauge at all
//!   on any process, so it is genuinely *absent* from every scrape —
//!   `deploy/prometheus/alerts.yml`'s rule for this condition checks both
//!   `== 0` and `absent(...)`, deliberately, because only one of the two
//!   is true in each failure shape and neither alone covers both.
//! - [`DISPATCH_IN_FLIGHT_SUBMITS`] — incremented immediately before
//!   `SmsProvider::submit` is called and decremented immediately after, in
//!   `crates/sms-worker/src/dispatch.rs::submit_one`. `dispatch` is a
//!   [`Cardinality::Singleton`](../sms_worker/enum.Cardinality.html) role
//!   by construction (`crates/sms-worker/src/lib.rs`) and `submit_one` is
//!   awaited sequentially inside `tick`'s own `for` loop, never spawned —
//!   so a single healthy process can never show more than `1` in flight at
//!   once. A fleet-wide `sum(...) > 1`, sustained across a scrape interval,
//!   is exactly the "two dispatch workers, heading for a blocked account"
//!   split-brain #70 names — evidence the advisory-lock exclusion itself
//!   has been defeated (a stale connection the OS hasn't yet noticed
//!   dropped, an operator running a second process by hand), not something
//!   this gauge can happen on its own under correct leader election.
//! - [`WEBHOOK_OUTBOX_OLDEST_UNDELIVERED_AGE_SECONDS`] — set from
//!   `crates/sms-worker/src/drain.rs::tick`'s own already-`pub`
//!   `oldest_undelivered_age` on every tick, reusing the exact function
//!   that module's own doc says exists "so alerting could reach it." Same
//!   absent-vs-zero technique as the lease gauge: only the process
//!   currently *holding* `drain`'s lease ever calls `tick`
//!   (`run_singleton` only invokes a role's real body after acquiring the
//!   lock), so a standby's own `/metrics` never mentions this series at
//!   all, and a `drain`-unheld cluster reports it nowhere — `absent(...)`
//!   is the correct alert for that case, the same as the lease gauge's.
//! - [`EVENT_OUTBOX_POISON_ROWS`] — set from
//!   `crates/sms-worker/src/jobs/reap_outbox.rs::ReapOutbox::run_at`'s own
//!   already-`pub` `alert_poison_rows`'s returned count, the same reuse
//!   principle as the drain gauge. `reap_outbox` is a `Job` (`Cardinality::
//!   ScaleToN`, claimed by CAS, not a singleton lease), so unlike the other
//!   four this one only refreshes once per hourly run (§7.5's cadence) and
//!   on whichever `jobs`-role process happened to win that run's claim —
//!   the value is sticky between runs (a Prometheus gauge holds its
//!   last-set value across scrapes; it does not decay), which is the
//!   correct behaviour here: "how many poison rows were last observed" is
//!   still meaningful an hour later, not stale in the way a heartbeat would
//!   be. `deploy/prometheus/alerts.yml`'s rule for this condition does not
//!   need the absent-vs-zero treatment the other two per-role gauges do —
//!   a fresh process that has simply never won a `reap_outbox` claim
//!   reports a true, correct `0` (`LazyLock` initialises every `IntGauge` at
//!   `0` on first touch), and summing across every `jobs`-role instance
//!   gives the right total either way.
//!
//! - [`ROUTE_DELIVERY_DIVERGENCE_FLAGGED`] — #64 (grey-route detection).
//!   Count of `(operator, class)` peer groups where one route's delivery
//!   rate diverged from its group's best-performing route, found on this
//!   process's most recent `grey_route_watch` job run. Same sticky-gauge
//!   shape as [`EVENT_OUTBOX_POISON_ROWS`] — `grey_route_watch` is a `Job`
//!   (CAS-claimed, not a singleton lease), so a fresh process reporting `0`
//!   because it has never won a claim is already correct, not a false
//!   all-clear. See
//!   `crates/sms-worker/src/jobs/grey_route_watch.rs`'s own module doc for
//!   what "diverged" means and why it deliberately does not fire on a small
//!   sample with a large delta — that gate is the entire point of this
//!   metric existing rather than a naive "route X is below Y% delivered."
//! - [`ROUTE_VALIDATION_OVERDUE`] — #64's other half. Count of `enabled`
//!   routes with no [`RouteValidation`](../sms_api/schema/index.html) row
//!   in the last 30 days, found on this process's most recent
//!   `grey_route_watch` run. Same sticky-gauge shape as
//!   [`ROUTE_DELIVERY_DIVERGENCE_FLAGGED`]. This metric does **not** know
//!   whether a route is actually a grey route — only whether the one thing
//!   that could tell you (a human looking at a real handset) has happened
//!   recently enough to trust. See `OPEN_QUESTIONS.md` §2.4: nothing in
//!   this crate closes the "no ground truth" gap that entry names, and this
//!   metric is deliberately scoped to not imply otherwise.
//!
//! # Deliberately not built here
//!
//! No histogram of submit/procedure latency, no balance/spend tracking —
//! §9.1's own prose lists several more metrics than #70's five originally
//! named alert conditions needed, and building ahead of a named requirement
//! is exactly what this repository's own delivery convention (`AGENTS.md`)
//! argues against. The two #64 metrics above are the one deliberate
//! exception: #64's own issue text names the alert condition directly
//! ("delivery-rate divergence between routes that should behave
//! identically"), which is a named requirement, not scope creep. Seven
//! metrics, seven alerts, no more.

use std::sync::LazyLock;

use prometheus::{
    Encoder, Gauge, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry, TextEncoder,
};

/// One registry per process. Every metric in this crate registers itself
/// into this on first access (via `LazyLock`) — [`render`] gathers from
/// exactly this and nothing else, so a metric defined here but never
/// referenced by any call site in the process simply never appears in that
/// process's `/metrics` output. That is a feature, not a gap — see this
/// module's own doc on why several metrics rely on exactly that to
/// distinguish "not running" from "running and reporting zero."
pub static REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::new);

/// SQLSTATE `SM001` rejections observed by this process, labelled
/// `entity` (`message`/`job`/`webhook attempt` — whichever guard trigger
/// raised it), `from_state`, `to_state`. See [`record_sm001`].
pub static SM001_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "sms_sm001_total",
            "Illegal state-transition rejections (Postgres SQLSTATE SM001) observed by this \
             process, by entity and from/to state. Flat zero in a correct system — any non-zero \
             rate means application code proposed a transition the database's own guard trigger \
             refused; see docs/runbooks/alerting.md.",
        ),
        &["entity", "from_state", "to_state"],
    )
    .expect("sms_sm001_total: static metric definition is valid");
    REGISTRY
        .register(Box::new(counter.clone()))
        .expect("sms_sm001_total registered exactly once per process");
    counter
});

/// `1` while this process holds `role`'s advisory-lock lease, `0` while it
/// is standing by (configured to run the role, not currently in charge).
/// Never touched by a process not configured to run `role` at all — see
/// this module's own doc for why that absence is load-bearing.
pub static SINGLETON_LEASE_HELD: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    let gauge = IntGaugeVec::new(
        Opts::new(
            "sms_worker_singleton_lease_held",
            "1 if this process currently holds this singleton role's advisory-lock lease, 0 \
             while configured for the role but standing by. A role with no non-zero series and \
             no series at all anywhere means it is unheld cluster-wide; see \
             docs/runbooks/alerting.md.",
        ),
        &["role"],
    )
    .expect("sms_worker_singleton_lease_held: static metric definition is valid");
    REGISTRY
        .register(Box::new(gauge.clone()))
        .expect("sms_worker_singleton_lease_held registered exactly once per process");
    gauge
});

/// In-flight `SmsProvider::submit` calls for `provider`, incremented
/// immediately before the call and decremented immediately after. A
/// healthy single `dispatch` holder never exceeds `1` — see this module's
/// own doc.
pub static DISPATCH_IN_FLIGHT_SUBMITS: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    let gauge = IntGaugeVec::new(
        Opts::new(
            "sms_dispatch_in_flight_submits",
            "In-flight provider submit calls, by provider key. A single correctly-elected \
             dispatch holder never exceeds 1 — a fleet-wide sum sustained above 1 means two \
             workers believe they hold the dispatch lease at once.",
        ),
        &["provider"],
    )
    .expect("sms_dispatch_in_flight_submits: static metric definition is valid");
    REGISTRY
        .register(Box::new(gauge.clone()))
        .expect("sms_dispatch_in_flight_submits registered exactly once per process");
    gauge
});

/// Age, in seconds, of the oldest still-undelivered `cratestack_event_outbox`
/// row, as of the most recent `drain` tick this process ran. Only ever set
/// by the process currently holding `drain`'s lease — see this module's own
/// doc.
pub static WEBHOOK_OUTBOX_OLDEST_UNDELIVERED_AGE_SECONDS: LazyLock<Gauge> = LazyLock::new(|| {
    let gauge = Gauge::new(
        "sms_webhook_outbox_oldest_undelivered_age_seconds",
        "Age in seconds of the oldest undelivered webhook outbox row, as of the drain role's \
         most recent tick on this process. Only reported by the process currently holding \
         drain's lease; absent everywhere means drain is unheld cluster-wide.",
    )
    .expect("sms_webhook_outbox_oldest_undelivered_age_seconds: static metric definition is valid");
    REGISTRY.register(Box::new(gauge.clone())).expect(
        "sms_webhook_outbox_oldest_undelivered_age_seconds registered exactly once per process",
    );
    gauge
});

/// Poison `cratestack_event_outbox` rows (`delivered_at IS NULL`, `attempts`
/// past `reap_outbox`'s threshold) found on this process's most recent
/// `reap_outbox` run, if any. See this module's own doc for why this one
/// does not need the absent-vs-zero treatment the other two per-role
/// gauges do.
pub static EVENT_OUTBOX_POISON_ROWS: LazyLock<IntGauge> = LazyLock::new(|| {
    let gauge = IntGauge::new(
        "sms_event_outbox_poison_rows",
        "Poison event-outbox rows (undelivered, attempts past reap_outbox's threshold) found on \
         this process's most recent reap_outbox run.",
    )
    .expect("sms_event_outbox_poison_rows: static metric definition is valid");
    REGISTRY
        .register(Box::new(gauge.clone()))
        .expect("sms_event_outbox_poison_rows registered exactly once per process");
    gauge
});

/// `(operator, class)` peer groups with a divergent route, found on this
/// process's most recent `grey_route_watch` run. See this module's own doc.
pub static ROUTE_DELIVERY_DIVERGENCE_FLAGGED: LazyLock<IntGauge> = LazyLock::new(|| {
    let gauge = IntGauge::new(
        "sms_route_delivery_divergence_flagged",
        "Route pairs where one route's delivery rate diverged from its (operator, class) peer \
         group's best route, found on this process's most recent grey_route_watch run. Gated on \
         sample size and statistical significance — see crates/sms-worker/src/jobs/\
         grey_route_watch.rs.",
    )
    .expect("sms_route_delivery_divergence_flagged: static metric definition is valid");
    REGISTRY
        .register(Box::new(gauge.clone()))
        .expect("sms_route_delivery_divergence_flagged registered exactly once per process");
    gauge
});

/// `enabled` routes with no handset validation in the last 30 days, found on
/// this process's most recent `grey_route_watch` run. See this module's own
/// doc.
pub static ROUTE_VALIDATION_OVERDUE: LazyLock<IntGauge> = LazyLock::new(|| {
    let gauge = IntGauge::new(
        "sms_route_validation_overdue",
        "Enabled routes with no RouteValidation row in the last 30 days, found on this \
         process's most recent grey_route_watch run. Staleness of the human handset check, not \
         evidence of an actual grey route — see OPEN_QUESTIONS.md §2.4.",
    )
    .expect("sms_route_validation_overdue: static metric definition is valid");
    REGISTRY
        .register(Box::new(gauge.clone()))
        .expect("sms_route_validation_overdue registered exactly once per process");
    gauge
});

/// Parse a guard trigger's own `RAISE EXCEPTION` text —
/// `illegal <entity> transition <from> -> <to> on <id>`, the exact format
/// all three of `messages_guard_transition`/`jobs_guard_transition`/
/// `attempts_guard_transition` share
/// (`schema/migrations/postgres/0002_bootstrap/up.sql`) — into
/// `(entity, from_state, to_state)`. `entity` is one or two words
/// (`"message"`, `"job"`, `"webhook attempt"`), which is why this splits on
/// the literal `" transition "` marker rather than assuming a fixed word
/// count. Falls back to `"unknown"` for anything that doesn't match — a
/// parse failure must never panic or drop the observation, since a
/// changed/unexpected trigger message would otherwise take the metric
/// itself down along with it.
fn parse_illegal_transition(detail: &str) -> (&str, &str, &str) {
    let Some((before, after)) = detail.split_once(" transition ") else {
        return ("unknown", "unknown", "unknown");
    };
    // `before` is "illegal <entity>" with no guaranteed prefix — a raw
    // sqlx/Postgres error's own `Display` text may prepend driver-specific
    // framing ("database: ...", "error returned from database: ...")
    // ahead of the trigger's literal message, the same reason
    // `illegal_transition_message` (this workspace's other consumer of
    // this exact trigger text) searches for the substring rather than
    // assuming it starts the string.
    let entity = before
        .split_once("illegal")
        .map(|(_, rest)| rest.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown");

    let Some((from_to, _id_part)) = after.split_once(" on ") else {
        return (entity, "unknown", "unknown");
    };
    match from_to.split_once(" -> ") {
        Some((from_state, to_state)) => (entity, from_state.trim(), to_state.trim()),
        None => (entity, "unknown", "unknown"),
    }
}

/// Record one SM001 rejection. `detail` is the raw database error text —
/// `CoolError::DatabaseTyped`'s own `detail` field, or equivalently
/// `CoolError::to_string()` — not yet mapped or truncated. Called from
/// exactly one place, `crates/sms-api/src/errors.rs::map_database_error`,
/// so every SM001 this workspace ever sees, from either process, passes
/// through here exactly once.
pub fn record_sm001(detail: &str) {
    let (entity, from_state, to_state) = parse_illegal_transition(detail);
    SM001_TOTAL
        .with_label_values(&[entity, from_state, to_state])
        .inc();
}

/// Render every metric registered in [`REGISTRY`] as Prometheus text
/// exposition format — the body `GET /metrics` returns verbatim.
///
/// # Errors
///
/// Only if the underlying encoder itself fails, which the `prometheus`
/// crate's own docs describe as effectively unreachable for the text
/// format (no I/O, no fallible metric state at this point) — surfaced as a
/// `Result` anyway rather than a `.expect()`, since this is reachable from
/// a live HTTP handler and an operator is better served by a `500` than a
/// crashed process.
///
/// # Panics
///
/// Only if the encoder ever produced non-UTF-8 bytes, which the
/// `prometheus` crate's text format cannot: every byte it writes comes from
/// metric names/labels (validated ASCII-ish identifiers at registration —
/// see every `LazyLock` above's own `.expect(...)`) or `Display`-formatted
/// numbers.
pub fn render() -> Result<String, prometheus::Error> {
    let families = REGISTRY.gather();
    let encoder = TextEncoder::new();
    let mut buf = Vec::new();
    encoder.encode(&families, &mut buf)?;
    Ok(String::from_utf8(buf).expect("prometheus's TextEncoder always emits valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::{parse_illegal_transition, record_sm001, render, SM001_TOTAL};

    #[test]
    fn parses_the_message_triggers_exact_text() {
        let detail = "database: illegal message transition delivered -> queued on abc123def";
        assert_eq!(
            parse_illegal_transition(detail),
            ("message", "delivered", "queued")
        );
    }

    #[test]
    fn parses_the_job_triggers_exact_text() {
        let detail = "illegal job transition running -> failed on job_xyz";
        assert_eq!(
            parse_illegal_transition(detail),
            ("job", "running", "failed")
        );
    }

    /// The one entity name that is two words, not one — the reason this
    /// parses on the literal `" transition "` marker instead of assuming a
    /// fixed word count before it.
    #[test]
    fn parses_the_webhook_attempt_triggers_two_word_entity() {
        let detail = "illegal webhook attempt transition succeeded -> pending on wha_123";
        assert_eq!(
            parse_illegal_transition(detail),
            ("webhook attempt", "succeeded", "pending")
        );
    }

    #[test]
    fn an_unrecognised_shape_falls_back_to_unknown_rather_than_panicking() {
        assert_eq!(
            parse_illegal_transition("something unrelated happened"),
            ("unknown", "unknown", "unknown")
        );
    }

    #[test]
    fn record_sm001_increments_the_labelled_counter() {
        let before = SM001_TOTAL
            .with_label_values(&["message", "delivered", "queued"])
            .get();
        record_sm001("database: illegal message transition delivered -> queued on abc123");
        let after = SM001_TOTAL
            .with_label_values(&["message", "delivered", "queued"])
            .get();
        assert_eq!(after, before + 1);
    }

    #[test]
    fn render_produces_prometheus_text_naming_every_metric() {
        // Touch every static at least once so it is guaranteed registered
        // before gathering — mirroring what the real call sites do in
        // normal operation (each one is touched by its own owning code
        // path long before any scrape could race it).
        record_sm001("illegal message transition accepted -> failed on mid_render_test");
        super::SINGLETON_LEASE_HELD
            .with_label_values(&["dispatch"])
            .set(1);
        super::DISPATCH_IN_FLIGHT_SUBMITS
            .with_label_values(&["orange_cm"])
            .set(0);
        super::WEBHOOK_OUTBOX_OLDEST_UNDELIVERED_AGE_SECONDS.set(0.0);
        super::EVENT_OUTBOX_POISON_ROWS.set(0);
        super::ROUTE_DELIVERY_DIVERGENCE_FLAGGED.set(0);
        super::ROUTE_VALIDATION_OVERDUE.set(0);

        let body = render().expect("rendering the registry must not fail");
        for name in [
            "sms_sm001_total",
            "sms_worker_singleton_lease_held",
            "sms_dispatch_in_flight_submits",
            "sms_webhook_outbox_oldest_undelivered_age_seconds",
            "sms_event_outbox_poison_rows",
            "sms_route_delivery_divergence_flagged",
            "sms_route_validation_overdue",
        ] {
            assert!(
                body.contains(name),
                "{name} missing from rendered output:\n{body}"
            );
        }
    }
}
