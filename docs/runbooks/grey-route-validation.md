# Runbook: monthly handset validation per route (#64)

§6.4 of the design doc names the check directly: "Validate every route with
real handsets on each network before trusting it, and re-validate monthly."
This is what a human runs to do that, and how the result gets recorded so
`crate::jobs::grey_route_watch`'s own overdue check
(`docs/runbooks/36-handset-gate.md`'s own precedent: some things need a
human and a real phone, not a test suite) can see it happened.

**This cannot be automated, and this runbook does not try to.** Nothing
server-side can see what a recipient's handset actually displays — a grey
route silently rewrites the sender ID *after* this system has already
handed the message to a provider, and every DLR still reports `delivered`
regardless. See `OPEN_QUESTIONS.md` §2.4 for the full framing: this runbook
closes the "how does the evidence get recorded" gap, not the "no ground
truth" one.

## What counts as a route, here

One validation covers one `Route` row, tested against one operator's real
handset. A wildcard route (`matchOperator IS NULL`) that carries traffic to
more than one network needs a separate run — and a separate
`RouteValidation` row — per network it actually serves, since a grey
substitution can affect one network's leg of a route and not another's.

## Prerequisites

- A real, active SIM on the network you're validating (`mtn`/`orange`/
  `camtel`/`nexttel`), reachable and watched during the test — same
  requirement `docs/runbooks/36-handset-gate.md`'s own Test 1 has.
- The `Route.id` you're validating. `GET /routes` (or `psql`) lists them.
- Access to run `sms-gateway record-route-validation` against this
  deployment's real `DATABASE_URL` — the same access level
  `provision-user`/`seed-dispatch` already require.

## Procedure

1. **Send a real message through the specific route being validated.**
   The simplest way to guarantee a specific route handles it is a `Route`
   with a narrow `matchPrefix`/`matchOperator` that only your test MSISDN
   can hit, or a temporary priority bump — this repo has no "force this
   message down this exact route" debug flag, and building one is out of
   this ticket's scope. `crates/sms-api/examples/send_test_message.rs`
   (`docs/runbooks/36-handset-gate.md`'s own trigger) sends a real message
   through the normal `sendMessage` path; #54's route simulator
   (`admin/app/simulator`, or `simulateRoute` directly) tells you which
   route a given `(operator, class, appId, msisdn)` combination would
   actually resolve to, *before* you spend a real SMS confirming it.
2. **Watch the handset.** Record exactly what the sender ID field shows —
   not what you expect it to show. §6.4's own named symptom: "sender ID
   silently replaced with a numeric string." A grey route's DLR still comes
   back `delivered`; the handset is the only place this is visible.
3. **Record the result**, whether it passed or failed:

   ```bash
   sms-gateway record-route-validation \
       --database-url "$DATABASE_URL" \
       --route-id <the Route.id you tested> \
       --operator orange \
       --performed-by "Your Name <you@example.com>" \
       --expected-sender-id VYMALO \
       --observed-sender-id VYMALO \
       --passed \
       --notes "monthly check, handset: <model>, network: Orange CM"
   ```

   Omit `--passed` (don't just set `--observed-sender-id` to something
   different and forget the flag) for a failed check — the flag's absence
   is what the job and the record itself key on, independent of whether
   `--expected-sender-id`/`--observed-sender-id` happen to differ.

4. **On a failure**, treat the route as suspect *now*, not after a
   follow-up investigation:
   - `PATCH /routes/{id}` (or the admin console's Routes screen, once a
     write path exists for it — see `AGENTS.md`'s own #54 section for why
     that's not wired up yet) with `enabled: false` takes the route out of
     future routing decisions immediately (§6.3: `Route.enabled` is read on
     every `select_route` call).
   - File the finding — which operator, which route, what the handset
     actually showed — as a real issue, not just this one row. A grey route
     found once is worth investigating for how long it's been running
     underneath a `delivered`-reporting DLR stream that never said anything
     was wrong.

## What this does and does not close

Recording a validation makes `crate::jobs::grey_route_watch`'s overdue
check accurate for this one route, for the next `VALIDATION_INTERVAL`
window — see that job's own module doc. It does **not** mean the route is
safe for the next 30 days; it means someone looked, once, at one point in
that window. A route can turn grey the day after a passing validation and
nothing here will notice until the next scheduled check or a delivery-rate
divergence happens to cross this job's own statistical bar. That gap is
inherent to a periodic human check, not a shortcut this runbook is taking.

## Interpreting `crate::jobs::grey_route_watch`'s two alerts

- **`SMSRouteValidationOverdue`** means exactly one thing: no
  `RouteValidation` row exists for an `enabled` route within the last 30
  days. It says nothing about whether the route is actually healthy — run
  this runbook against it.
- **`SMSRouteDeliveryDivergence`** means a route's own recent delivery rate
  is statistically and practically worse than its best-performing peer
  serving the same `(operator, class)` combination — see
  `crates/sms-worker/src/jobs/grey_route_watch.rs`'s own module doc for the
  exact gates. It is evidence worth investigating, not a confirmed grey
  route; the fastest way to turn "worth investigating" into "confirmed" is
  this runbook, run against the flagged route specifically.
