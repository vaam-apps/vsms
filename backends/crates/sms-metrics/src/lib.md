Prometheus metric definitions for #70/#71 (M6 — Observability and
alerting on the states that are silent by default).

# Why this is its own crate

`sms-gateway` and `sms-worker` are two separate OS processes (`app/`'s
own dependency-arrow rule: a binary may depend on any library, never the
reverse, and `backends/apps/sms-gateway`/`backends/apps/sms-worker` never depend on each
other). Each process gets its own independent copy of every
[`std::sync::LazyLock`] static in this crate — there is no shared memory
between them, and there does not need to be. Prometheus is the
aggregation layer: each process exposes its own `/metrics`, a scrape
config names both as separate targets, and an alert rule that needs a
fleet-wide view (e.g. "is `dispatch`'s lease held *anywhere*") uses
`sum(...)` across the scraped instances, not a shared counter inside the
application. Putting the metric *definitions* (names, help text, label
sets) in one crate that both `sms-api` (for [`record_sm001`], called
from `backends/crates/sms-api/src/errors.rs::map_database_error`) and
`sms-worker` (for the other four) depend on is what stops the same
metric drifting into two slightly different names/labels the way a
hand-copied constant would — the same reasoning
`backends/crates/sms-worker/src/jobs.rs`'s own backoff-schedule comment gives for
not sharing code across the `app`/`crates` dependency boundary, applied
to the one case where sharing *is* possible: `sms-worker` already
depends on `sms-api`, so both can depend on a third, lower crate without
creating a cycle.

# Backend: Prometheus text exposition, not OTLP

Chosen over OpenTelemetry because this system already runs behind a
Caddy edge with no collector infrastructure anywhere in `deploy/`, and a
pull-based `/metrics` endpoint needs nothing new to *receive* it — an
operator points a Prometheus server (or nothing at all, and reads
`/metrics` by hand) at two more targets. OTLP would need a collector
process this deployment does not have and #71 does not ask for. The
`prometheus` crate (`=0.14`, Apache-2.0, checked against `deny.toml`'s
allowed-license list) is used with `default-features = false` —
`TextEncoder`/`Registry`/`*Vec` are all available without it; the
default `protobuf` feature exists for Prometheus's older binary scrape
protocol, which nothing in this deployment's tooling needs, and turning
it off keeps this crate's own dependency footprint to the metric types
themselves.

# The seven metrics, and why each is shaped the way it is

- [`SM001_TOTAL`] — §9.1 of the design doc names this "the highest-
  signal metric in the list... in a correct system it is flat zero."
  Labelled `entity`/`from_state`/`to_state`, parsed out of the
  trigger's own `RAISE EXCEPTION` text
  (`illegal <entity> transition <from> -> <to> on <id>`,
  `backends/migrations/postgres/0002_bootstrap/up.sql`) by
  [`record_sm001`] — matching §9.1's own "`SM001` rejection count by
  from/to pair" line, not a plain unlabelled counter. See
  `backends/crates/sms-api/src/errors.rs`'s own doc for why `map_database_error`
  is the single place every write in this workspace funnels an SM001
  through before this crate ever sees one.
- [`SINGLETON_LEASE_HELD`] — one gauge per `(process, role)`, set to `1`
  the instant a lease is acquired and `0` the instant this process is
  standing by, released, or fails to attempt the lock — see
  `backends/crates/sms-worker/src/lib.rs::run_singleton`, the only writer. The
  point of writing `0`, not simply never touching the gauge while
  standing by, is the absent-vs-zero distinction #70 calls out as "the
  subtlest requirement": a process configured to run a role and merely
  not currently holding its lease must show up as a `0` time series
  (provably watching, not currently in charge), so
  `sum(sms_worker_singleton_lease_held{role="dispatch"}) == 0` is a real
  "nobody holds it" signal, not indistinguishable from "nobody is even
  trying." A role no process was ever configured to run (a typo'd
  `--roles`, or the entire fleet down) never touches this gauge at all
  on any process, so it is genuinely *absent* from every scrape —
  `deploy/prometheus/alerts.yml`'s rule for this condition checks both
  `== 0` and `absent(...)`, deliberately, because only one of the two
  is true in each failure shape and neither alone covers both.
- [`DISPATCH_IN_FLIGHT_SUBMITS`] — incremented immediately before
  `SmsProvider::submit` is called and decremented immediately after, in
  `backends/crates/sms-worker/src/dispatch.rs::submit_one`. `dispatch` is a
  [`Cardinality::Singleton`](../sms_worker/enum.Cardinality.html) role
  by construction (`backends/crates/sms-worker/src/lib.rs`) and `submit_one` is
  awaited sequentially inside `tick`'s own `for` loop, never spawned —
  so a single healthy process can never show more than `1` in flight at
  once. A fleet-wide `sum(...) > 1`, sustained across a scrape interval,
  is exactly the "two dispatch workers, heading for a blocked account"
  split-brain #70 names — evidence the advisory-lock exclusion itself
  has been defeated (a stale connection the OS hasn't yet noticed
  dropped, an operator running a second process by hand), not something
  this gauge can happen on its own under correct leader election.
- [`WEBHOOK_OUTBOX_OLDEST_UNDELIVERED_AGE_SECONDS`] — set from
  `backends/crates/sms-worker/src/drain.rs::tick`'s own already-`pub`
  `oldest_undelivered_age` on every tick, reusing the exact function
  that module's own doc says exists "so alerting could reach it." Same
  absent-vs-zero technique as the lease gauge: only the process
  currently *holding* `drain`'s lease ever calls `tick`
  (`run_singleton` only invokes a role's real body after acquiring the
  lock), so a standby's own `/metrics` never mentions this series at
  all, and a `drain`-unheld cluster reports it nowhere — `absent(...)`
  is the correct alert for that case, the same as the lease gauge's.
- [`EVENT_OUTBOX_POISON_ROWS`] — set from
  `backends/crates/sms-worker/src/jobs/reap_outbox.rs::ReapOutbox::run_at`'s own
  already-`pub` `alert_poison_rows`'s returned count, the same reuse
  principle as the drain gauge. `reap_outbox` is a `Job` (`Cardinality::
  ScaleToN`, claimed by CAS, not a singleton lease), so unlike the other
  four this one only refreshes once per hourly run (§7.5's cadence) and
  on whichever `jobs`-role process happened to win that run's claim —
  the value is sticky between runs (a Prometheus gauge holds its
  last-set value across scrapes; it does not decay), which is the
  correct behaviour here: "how many poison rows were last observed" is
  still meaningful an hour later, not stale in the way a heartbeat would
  be. `deploy/prometheus/alerts.yml`'s rule for this condition does not
  need the absent-vs-zero treatment the other two per-role gauges do —
  a fresh process that has simply never won a `reap_outbox` claim
  reports a true, correct `0` (`LazyLock` initialises every `IntGauge` at
  `0` on first touch), and summing across every `jobs`-role instance
  gives the right total either way.

- [`ROUTE_DELIVERY_DIVERGENCE_FLAGGED`] — #64 (grey-route detection).
  Count of `(operator, class)` peer groups where one route's delivery
  rate diverged from its group's best-performing route, found on this
  process's most recent `grey_route_watch` job run. Same sticky-gauge
  shape as [`EVENT_OUTBOX_POISON_ROWS`] — `grey_route_watch` is a `Job`
  (CAS-claimed, not a singleton lease), so a fresh process reporting `0`
  because it has never won a claim is already correct, not a false
  all-clear. See
  `backends/crates/sms-worker/src/jobs/grey_route_watch.rs`'s own module doc for
  what "diverged" means and why it deliberately does not fire on a small
  sample with a large delta — that gate is the entire point of this
  metric existing rather than a naive "route X is below Y% delivered."
- [`ROUTE_VALIDATION_OVERDUE`] — #64's other half. Count of `enabled`
  routes with no [`RouteValidation`](../sms_api/schema/index.html) row
  in the last 30 days, found on this process's most recent
  `grey_route_watch` run. Same sticky-gauge shape as
  [`ROUTE_DELIVERY_DIVERGENCE_FLAGGED`]. This metric does **not** know
  whether a route is actually a grey route — only whether the one thing
  that could tell you (a human looking at a real handset) has happened
  recently enough to trust. See `OPEN_QUESTIONS.md` §2.4: nothing in
  this crate closes the "no ground truth" gap that entry names, and this
  metric is deliberately scoped to not imply otherwise.

# Deliberately not built here

No histogram of submit/procedure latency, no balance/spend tracking —
§9.1's own prose lists several more metrics than #70's five originally
named alert conditions needed, and building ahead of a named requirement
is exactly what this repository's own delivery convention (`AGENTS.md`)
argues against. The two #64 metrics above are the one deliberate
exception: #64's own issue text names the alert condition directly
("delivery-rate divergence between routes that should behave
identically"), which is a named requirement, not scope creep. Seven
metrics, seven alerts, no more.
