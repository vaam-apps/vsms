# Alerting — the seven conditions that are silent by default

[#70](https://github.com/vymalo/vsms/issues/70)/[#71](https://github.com/vymalo/vsms/issues/71),
epic [#66](https://github.com/vymalo/vsms/issues/66), plus two more from
[#64](https://github.com/vymalo/vsms/issues/64) (grey-route detection,
epic [#60](https://github.com/vymalo/vsms/issues/60)). §9.1 of
[the design doc](../architecture.md#91-observability) is the spec #70/#71
implement; [`backends/crates/sms-metrics/src/lib.rs`](../../backends/crates/sms-metrics/src/lib.rs)'s
own module doc is the authoritative reference for what each metric
measures and why it's shaped the way it is. This file is the operator-
facing half: what fires, and what to do about it.

## What "alerting" means in this repository

Nothing here can page anyone. `deploy/prometheus/alerts.yml` is a real,
loadable Prometheus rule file — the rules genuinely evaluate, and a real
Prometheus (stood up as the `prometheus` service in
`deploy/docker-compose.yml`) shows firing state on its own `/alerts` page —
but there is no Alertmanager, no receiver, no Slack/PagerDuty integration
anywhere in this tree. Building one was explicitly out of scope for #70:
the honest deliverable is the metrics and the rules that would fire, not a
bespoke in-process alerting engine. Point a real Alertmanager at the
Prometheus instance (or configure `alerting:` in
`deploy/prometheus/prometheus.yml`) to actually get paged; that
infrastructure choice belongs to whoever operates this deployment, not to
this repository.

## Correlating a message end to end

The other half of #71 — before getting into the five alerts, since this is
what makes any of them actionable rather than just a number on a graph.
Three log lines, three different processes, joined by one value:

1. **`sms-gateway`**, inside `sendMessage` (`backends/crates/sms-api/src/
   procedures.rs::send`): `info!(message_id, app_id, client_ref,
   cratestack_request_id, state, "message accepted")`. The framework's own
   generated `cratestack_procedure`/`cratestack_request_id`-carrying log
   lines (`cratestack-macros`'s `invoke_with_db` wrapper) sit around this
   one in the same request — `cratestack_request_id` ties every log line
   for *this one HTTP request* together, whether that request came in with
   its own `X-Request-Id` header (honoured verbatim — see
   `backends/crates/sms-api/src/auth.rs::request_id_from`) or got a freshly minted
   one.
2. **`sms-worker`**, inside `dispatch::submit_one`
   (`backends/crates/sms-worker/src/dispatch.rs`): `info!(message_id, provider,
   provider_ref, "message submitted")`. No `cratestack_request_id` here —
   this loop runs under an internal `system` context this crate mints
   itself, never derived from the HTTP request that originally created the
   message, so there is nothing request-scoped to carry forward.
3. **`sms-gateway`** again, inside DLR ingestion (`backends/crates/sms-api/src/
   dlr.rs::ingest_one`): `info!(message_id, from_state, to_state,
   provider_ref, "DLR applied")`. Also no `cratestack_request_id` — a DLR
   arrives on its own unauthenticated HTTP connection with no bearer token
   and therefore no per-request `CoolContext` (`dlr.rs`'s own module doc).

**`message_id` (`Message.id`) is the join key across all three, not a
`traceparent` or a shared span context** — these are three separate
processes (or, for lines 1 and 3, the same process at two points separated
by minutes and an intervening database write), and the worker picks a
message up from a Postgres row, not from an in-process call. Do not claim
this is end-to-end distributed tracing in the OpenTelemetry sense: there is
no shared span tree, no exporter, no trace visualisation tool wired up.
What exists is three structured log lines that share one grep-able field,
which is what actually answers "why did this OTP never arrive" — pull
`sms-gateway`'s logs for `message_id=<id>` and `sms-worker`'s for the same,
and the whole lifecycle is there in the order it happened. A worked
example, given `sms-gateway=info,sms_worker_bin=info` (the default
`RUST_LOG` both binaries ship with):

```bash
grep '"message_id":"msg_abc123"' /var/log/sms-gateway.log /var/log/sms-worker.log | sort -k1
```

## The seven alerts

### SM001 {#sm001}

**`SMS001IllegalTransitionRateNonZero`** — `increase(sms_sm001_total[15m]) > 0`.
Flat zero in a correct system (§9.1's own framing: "the trigger is a
backstop, not a control path"). If this fires:

1. Read the alert's own `entity`/`from_state`/`to_state` labels — they name
   exactly which guard trigger rejected which edge
   (`messages_guard_transition`/`jobs_guard_transition`/
   `attempts_guard_transition`, `backends/migrations/postgres/
   0002_bootstrap/up.sql`).
2. Grep both `sms-gateway` and `sms-worker` logs for `cratestack_error` /
   `illegal ... transition ... on <id>` around the alert's firing time —
   the specific row id is in the trigger's own message.
3. This is a code bug, not an operational incident to route around: some
   write site proposed a transition the transition table doesn't allow.
   Compare the offending `(from_state, to_state)` pair against
   `message_state_transitions`/`job_state_transitions`/
   `attempt_state_transitions` (§2.10 of the design doc) and the
   corresponding `stateDiagram-v2` block (§7.4/§8.5) — one of the two has
   drifted from the other, and `cargo xtask parity` should
   have caught it before merge if the drift is in the diagram/table pair
   itself; a drift in application code proposing a state neither models
   won't be caught by that check.

### Singleton unheld {#singleton-unheld}

**`SMSDispatchSingletonUnheld`** / **`SMSDrainSingletonUnheld`** /
**`SMSSchedulerSingletonUnheld`** — see `deploy/prometheus/alerts.yml` for
the exact expressions. All three follow the same shape: `sum(...) == 0`
(every process configured for the role is reporting "standing by") *or*
`absent(...)` (no process anywhere is even attempting the role).

1. Check every `sms-worker` process's own `--roles`/`SMS_WORKER_ROLES` —
   the `absent(...)` half of this alert most often means a typo or a
   deployment that dropped the role from every instance's configuration
   entirely, not a crash.
2. If the role is configured somewhere, check that process's logs for
   `RoleLease`/`run_singleton` errors (`backends/crates/sms-worker/src/lease.rs`) —
   a connection failure attempting `pg_try_advisory_lock` is the "actually
   broken," not "someone else has it," case §7.2/`lease.rs`'s own doc
   names as the one worth alerting on.
3. `dispatch`/`drain` are `severity: critical` — no messages submit, or
   webhook events stop draining on a timer (writers' own automatic
   post-commit drain still delivers most events; only retries of a
   previously-failed delivery stop). `scheduler` is `severity: warning` —
   nothing already enqueued is lost, only delayed.

### Concurrent dispatch submits {#concurrent-dispatch}

**`SMSConcurrentDispatchSubmits`** — `sum(sms_dispatch_in_flight_submits) > 1`
sustained for a minute. `dispatch` is a singleton role and submits
sequentially within one process (`backends/crates/sms-worker/src/dispatch.rs::tick`'s
own `for` loop, never spawned) — a sustained fleet-wide sum above `1` means
the advisory-lock exclusion itself has been defeated, not routine
concurrency.

1. Check for a second `sms-worker` process running `--roles` including
   `dispatch` that shouldn't be — a manual/scripted deploy step that
   started an extra instance, or two orchestrator replicas both configured
   with `dispatch` when only one should be.
2. Check Postgres's own `pg_locks` for the `dispatch` advisory lock
   (`SELECT * FROM pg_locks WHERE locktype = 'advisory'` —
   `backends/crates/sms-worker/src/lease.rs`'s `NS`/`advisory_lock_key` constants
   give the exact `(classid, objid)` pair to look for) — more than one
   session holding it is the smoking gun; if only one session holds it but
   the metric still shows concurrency, the more likely explanation is a
   stale connection the OS hasn't yet noticed dropped (see `lease.rs`'s own
   module doc on why a dropped `PgConnection` still releases the lock
   promptly — this would be a narrow timing window, not a sustained one).
3. Every submission this deployment makes carries no idempotency key to
   Orange (`AGENTS.md`'s own #36 gate note: "a crash in this exact window
   produces two real submissions") — treat this alert as a real risk of
   duplicate SMS sends to real handsets, not just a metrics anomaly.

### Webhook outbox stalled {#outbox-stalled}

**`SMSWebhookOutboxStalled`** — the oldest still-undelivered
`cratestack_event_outbox` row has been waiting past `drain`'s own 2-minute
stalled threshold (`backends/crates/sms-worker/src/drain.rs::STALLED_THRESHOLD`), or
the metric is absent entirely (which means `drain` is unheld — see the
`SMSDrainSingletonUnheld` alert, which will also be firing).

1. If `SMSDrainSingletonUnheld` is also firing, that's the root cause —
   fix that first.
2. If `drain` is held but this still fires, check `sms-worker` logs for
   `"draining the event outbox failed"` (`drain.rs`'s own `tick`) — a
   database error on every tick would explain a stuck row despite the role
   being held.
3. Otherwise: a subscriber is failing repeatedly on the same event
   (§8.2's "short-circuits on the first failing handler, retried from the
   top on every drain"). Cross-reference with the `SMSEventOutboxPoisonRows`
   alert below — a row stalled long enough usually also crosses
   `reap_outbox`'s own `attempts > 5` threshold eventually.

### Poison event-outbox rows {#poison-rows}

**`SMSEventOutboxPoisonRows`** — `sum(sms_event_outbox_poison_rows) > 0`.
Reuses `backends/crates/sms-worker/src/jobs/reap_outbox.rs`'s own
`POISON_ATTEMPTS_THRESHOLD` (5) — any non-zero value here already means a
row has retried past that threshold with no successful delivery.

1. Read the per-row `warn!` logs `reap_outbox`'s own `alert_poison_rows`
   emits every run (hourly, §7.5's cadence) — `model`/`operation`/
   `last_error` name the actual subscriber bug.
2. The row is never deleted automatically (`reap_outbox.rs`'s own module
   doc: deleting it would silently drop the event it was trying to
   redeliver). Fixing the underlying subscriber bug and letting the next
   `drain` tick retry the row is the intended remediation, not manual
   deletion.
3. If the fix genuinely requires discarding the event (it will never
   become deliverable — e.g. the target `WebhookEndpoint` was deleted),
   that is a deliberate, manual `DELETE` an operator runs by hand against
   `cratestack_event_outbox`, not something this job or any generated
   route does automatically.

### Route delivery-rate divergence {#route-divergence}

**`SMSRouteDeliveryDivergence`** — `sum(sms_route_delivery_divergence_flagged) > 0`,
debounced 5m. Set by `crate::jobs::grey_route_watch`'s daily run (see
`backends/crates/sms-worker/src/jobs/grey_route_watch.rs`'s own module doc for the
full mechanism): a route's own delivery rate, over the last 7 days, is both
statistically implausible (a two-proportion z-test past a conservative
threshold) and practically meaningful (at least a 15-point gap) worse than
the best-performing route serving the same `(operator, class)` pair, with
both sides carrying at least 30 terminal messages.

1. This is a proxy signal, not a confirmed grey route — §6.4's own
   framing: a grey route "looks fine in every metric except delivery
   quality," and this alert is built entirely from delivery-outcome
   counts, never from what a handset actually displayed.
2. Read the per-pair `warn!` log `grey_route_watch`'s own `check_divergence`
   emits — `operator`/`class`/`reference_route_id`/`route_id`/`rate`/`n`/
   `z_score` name exactly which route diverged from which, and by how much.
3. Run `docs/runbooks/grey-route-validation.md` against the flagged
   `route_id` specifically — a real handset check is the only way to turn
   "worth investigating" into "confirmed."
4. If confirmed, disable the route (`Route.enabled = false`) immediately
   rather than waiting for a scheduled fix — §6.3 already excludes a
   disabled route from every future routing decision.

### Route validation overdue {#route-validation-overdue}

**`SMSRouteValidationOverdue`** — `sum(sms_route_validation_overdue) > 0`,
debounced 5m. Set by the same `grey_route_watch` run: an `enabled` `Route`
with no `RouteValidation` row in the last 30 days.

1. This metric knows nothing about whether the route is actually healthy —
   only that nobody has looked recently. It fires identically for a route
   that has never had a single problem and one that turned grey the day
   after its last check.
2. Run `docs/runbooks/grey-route-validation.md` against every route named
   in the per-route `warn!` log (`route_id`/`route_name`/`last_validated`).
3. A permanently retired route that will never carry traffic again should
   be `enabled = false`, not left `enabled` and perpetually overdue — this
   alert has no way to distinguish "forgotten" from "decommissioned but
   never disabled."

## Running this locally

```bash
cd deploy
docker compose --env-file .env up -d prometheus
# ... send some traffic, or force a condition ...
curl -s localhost:9099/api/v1/alerts | jq '.data.alerts[] | {alertname: .labels.alertname, state}'
```

`prometheus` depends on `sms-gateway` being healthy but not on `caddy` or
`admin` — `docker compose up -d prometheus` brings up exactly what it
needs, per Compose's own transitive `depends_on` resolution.
