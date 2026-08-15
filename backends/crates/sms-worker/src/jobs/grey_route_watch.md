`grey_route_watch` — #64 (grey-route detection, epic #60). Not one of
§7.5's own nine named job kinds; a new one, added because #64's own
issue text names an alert condition §7.5 never anticipated: "delivery-
rate divergence between routes that should behave identically."

# Why this is two checks in one job, not one

§6.4's own paragraph on grey routes names both halves in the same
breath — "validate every route with real handsets... and re-validate
monthly" plus "alert on delivery-rate divergence" — and #64's issue text
pairs them the same way. They share a subject (is this route trustworthy
right now?) and a schedule (neither needs finer than daily), so one job
doing both is `reap_outbox`'s own precedent (two related concerns, one
run), not a new shape.

# The asymmetry that makes this ticket different from ordinary alerting

A grey route rewrites the sender ID on the wire, after this system has
already handed the message to a provider. `DeliveryReceipt` still says
`delivered` — nothing server-side changes. The only signal this system
*can* observe is indirect: a route whose delivery outcomes look worse
than its peers', or a route nobody has checked with a real handset
recently. Neither proves a grey route exists; both are exactly the
evidence `OPEN_QUESTIONS.md` §2.4 says this system does not otherwise
have. This job does not close that gap — it makes the two proxies it
*can* compute honest, visible, and resistant to false alarms, which is
what #64 actually asks for. See that entry for what this PR does and
does not change about "no ground truth."

# Delivery-rate divergence: what "should behave identically" means

Taken directly from #64's own issue text, not invented: "two routes to
the same operator, for the same message class, should have comparable
delivery rates." So the peer group is `(Message.operator,
Message.class)` — the two fields every message already carries,
classified at send time — and the members being compared are the
distinct `Message.routeId` values that actually handled traffic in that
group. `Route.matchOperator`/`matchClass` are not used for grouping:
a wildcard route's own *match* predicate says nothing about which
operator a given message it handled actually went to, but
`Message.operator` — stamped once, at classification time, and never
touched again — always does. A message's final `routeId` also already
accounts for failover (#63's `attempt_failover` overwrites it on
reroute), so a route's outcome count reflects whichever route actually
carried the final attempt, not just the first one tried.

Terminal states only, and `uncertain` is excluded from both sides of the
ratio, not folded into either: [`aggregate_outcomes`]'s own doc explains
why. `rejected`/`cancelled` never reach this aggregation at all — a
`rejected` message never received a `routeId` (§3's own pre-routing
refusal, or #62's own "no eligible route" refusal both happen before
`Message.routeId` is ever set), and `cancelled` is an operator's own
override, not a signal about the route.

# Sample size matters more than the delta — the actual design decision

AGENTS.md's own brief for this ticket states the failure mode plainly:
"an alert that fires on tiny samples will be ignored within a week,
which is worse than no alert." [`detect_divergent_routes`] enforces this
with two independent gates, deliberately redundant with each other:

1. **[`MIN_SAMPLE`], an explicit floor.** Neither side of a comparison
   is even considered until it has at least this many terminal
   messages. This is the visible, legible policy an operator reading
   this file can point to — not an emergent property of a formula.
2. **A two-proportion z-test, [`Z_THRESHOLD`].** Even past the sample
   floor, a difference has to be statistically implausible under "both
   routes are equally good" before it counts — the standard error term
   naturally shrinks the ratio as `n` grows, which is what makes a
   50%-vs-100%-over-4-messages delta score low and a
   70%-vs-98%-over-1000-messages delta score high, without either
   number being hand-picked for the specific case.
3. **[`MIN_DELTA`], a practical floor on top.** A z-test alone can flag
   a *statistically* significant but *operationally* meaningless gap
   (98.0% vs 98.3% at huge volume) — this stops that from paging
   anyone. All three conditions must hold; any one failing means no
   finding.

The reference route in each peer group is whichever qualifying member
has the highest observed rate — every other qualifying member is
compared against it. This assumes the best-performing route in a group
is a reasonable stand-in for "healthy," which is the same assumption
§6.4's own prose makes ("Orange Developer API as primary... a reputable
aggregator... as failover"): a legitimate route degrades traffic less
than a grey one substituting a sender ID and dropping messages on a
carrier's SMS firewall.

# Handset-validation staleness: what this job does and does not know

[`RouteValidation`](sms_api::schema::RouteValidation) rows are written
by a human, per `docs/runbooks/grey-route-validation.adoc` — this job
never writes one. It only asks, per `enabled` `Route`: when was this
last checked, and is that recent enough? [`is_overdue`] is the entire
policy, and it is intentionally naive — "no evidence in the last N
days" is a staleness signal, not a health signal. A route validated
yesterday and silently turned grey today reports exactly as "fine" as
one that has never had a single problem. That is not a bug in this
check; it is the honest boundary of what a periodic human observation
can promise between observations, restated so nobody reads a `0` on
[`sms_metrics::ROUTE_VALIDATION_OVERDUE`] as "these routes are good."

# No `GROUP BY` in the delegate API — client-side aggregation, bounded

`backends/crates/sms-api`'s own `Aggregate` builder (`cratestack-sqlx`'s
`Aggregate::count`/`sum`/`avg`/`min`/`max`) has no grouped form — it
answers "how many rows match this filter," not "how many per group."
Grouping by `(operator, class, routeId)` therefore happens in this
process, over rows fetched through the ordinary `Message` delegate
(still R1-compliant — a real model, a real policy, no raw SQL), bounded
by [`FETCH_LIMIT`] the same way `reap_outbox`'s own `DELETE_BATCH`/
`ALERT_BATCH` bound their own per-run work: a deployment with more
terminal traffic than that in one [`LOOKBACK`] window gets this run's
divergence check computed over its most recent [`FETCH_LIMIT`] messages,
not an unbounded scan — correct enough for a periodic proxy signal, and
it self-corrects on the next run regardless.
