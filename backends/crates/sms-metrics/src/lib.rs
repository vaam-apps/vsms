#![doc = include_str!("lib.md")]

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
             refused; see docs/runbooks/alerting.adoc.",
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
             docs/runbooks/alerting.adoc.",
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
         sample size and statistical significance — see backends/crates/sms-worker/src/jobs/\
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
/// (`backends/migrations/postgres/0002_bootstrap/up.sql`) — into
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
/// `CratestackError::DatabaseTyped`'s own `detail` field, or equivalently
/// `CratestackError::to_string()` — not yet mapped or truncated. Called from
/// exactly one place, `backends/crates/sms-api/src/errors.rs::map_database_error`,
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
    use super::{SM001_TOTAL, parse_illegal_transition, record_sm001, render};

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
