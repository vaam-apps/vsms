`Role::Dispatch`'s real body — #33. Drives `routed -> submitted` and
`routed`'s failure edges per §7.4; `accepted -> queued` (and its own
`-> rejected` edge) already lives in [`crate::claim`]'s `take_lease`,
since that hop is atomic with the claim itself, not a separate step
this loop drives.

Polls rather than reacting to a notification, on a short interval —
comfortably under the M2 gate's <15s delivery target even allowing for
one full miss between a message becoming claimable and this loop
noticing.

# #63 — failover and the provider circuit breaker

Landed on top of #33's own single-provider-deployment placeholder,
which collapsed every [`sms_provider::error::RoutingConsequence`] onto
whichever §7.4 edge fit because no second provider existed yet to fail
over to (#61/#62 built the second adapter and the routing engine that
picks between routes; this is the piece that reacts to one failing).

[`ProviderError::routing`] is the one source of truth for which errors
trigger a failover attempt and which are terminal-for-this-message —
see [`handle_submit_error`]'s own doc for the table. The two traps this
ticket exists to close, both already proven live before this landed
(see this crate's own tests and `backends/crates/sms-provider/src/error.rs`'s):

- **`ProviderError::Indeterminate` must never fail over.** The request
  may already have reached the provider; failing it over to a different
  provider risks a second, real SMS. `routing()` maps it to
  `HoldIndeterminate`, and [`handle_submit_error`] never even considers
  failover for that arm — this is unchanged from before #63, see
  [`terminal_outcome`]'s own doc.
- **A circuit breaker opening on provider A must route new work to B,
  never just reject it.** `backends/crates/sms-worker/src/routing.rs::convert_provider`
  is where this actually holds: it treats an open circuit exactly like
  `state != active` (unavailable, with a reason), so *every* future
  routing decision — not just the one message whose failure tripped the
  breaker — naturally skips A and picks whatever's left eligible.

**"Failover must not double-send: the claim loop's lease is what
prevents it"** (the issue's own words) is the property
[`attempt_failover`] is built around: a failover reroute never calls
[`sms_provider::SmsProvider::submit`] a second time inline. It writes
`routed -> queued` with a *new* `providerId`/`routeId` stamped on the
same CAS'd row and returns — the next claim (this tick or the next)
picks it up through the ordinary `queued -> routed -> submit` path,
under the same `if_match(version)` discipline every other reclaim in
this crate already relies on. This is only safe because every
`RoutingConsequence` that reaches [`attempt_failover`]
(`TryNextRoute`/`OpenCircuitAndTryNextRoute`, i.e. `Permanent`/
`Unavailable`) means *nothing was ever accepted by the provider* — see
`backends/crates/sms-provider/src/error.rs`'s own doc on why `Indeterminate` is
the one variant that can't make that claim, and why it never reaches
this function.

# #70/#71

[`submit_one`] is [`sms_metrics::DISPATCH_IN_FLIGHT_SUBMITS`]'s one
writer (incremented immediately before, decremented immediately after,
`SmsProvider::submit`) and the second correlation event in the
send-path chain — see `backends/crates/sms-api/src/procedures.rs`'s own module
doc for the first, and `sms_metrics`'s own doc for why a fleet-wide sum
sustained above `1` on that gauge is the split-brain signal #70 names.
Every write in this module also now routes its resulting `CoolError`
through `sms_api::map_database_error` before logging — the same reason
`crate::claim`, `crate::jobs`, `crate::hooks`, and `crate::jobs::
expire_stale` all do the same: that function is `sms_sm001_total`'s one
recording site (`backends/crates/sms-api/src/errors.rs`), and a write whose
error was never mapped could still hit an illegal edge without ever
being counted.
