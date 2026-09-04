The one circuit-breaker *decision* this crate makes twice, extracted so it
is made once. `hooks.rs` (#40, webhook delivery — 20 consecutive failures,
15-minute cool-down, `WebhookEndpoint.consecutiveFailures`/`circuitOpenUntil`)
and `dispatch.rs` (#63, provider submission — 5 consecutive `Unavailable`
failures, 60-second cool-down, `Provider.consecutiveFailures`/
`circuitOpenUntil`) each independently landed the identical shape: on
failure, increment a counter; the moment it reaches a threshold, reset the
counter to zero and stamp a cool-down timestamp; on success, reset both.
Two different threshold/duration pairs, two different generated
`UpdateXInput` types, one decision.

# What this module owns, and what it deliberately does not

[`on_failure`] and [`needs_reset`] are pure — no database, no clock inside
them (`now` is a parameter, matching `record_endpoint_failure`'s own
pre-extraction shape; `record_provider_failure` used to call
`chrono::Utc::now()` itself and still does, at its own call site, before
handing the value in). Given the same policy and the same starting count,
they always decide the same thing, which is what makes them worth unit
testing directly rather than only through a live suite.

What stays at each call site, deliberately not pulled in here: which
generated `UpdateXInput` field gets set, the `if_match(version)` CAS
write itself, and the best-effort "log a `warn!` and move on" handling of
a lost race or a denied write. That's not an oversight — `WebhookEndpoint`
and `Provider` have no common trait a shared write function could target
(different delegate, different input type, different id column), and
folding the write into this module would mean either a trait neither
model actually needs elsewhere or a closure-shaped indirection that buys
nothing a plain `match` at each call site doesn't already give for free.
The reasoning `hooks::record_endpoint_failure`'s own doc comment already
gives for best-effort writes (a lost CAS race under concurrent workers
undercounts by a small amount — acceptable for a heuristic that stops
hammering a dead endpoint/provider, not something anyone bills against)
is unchanged by this extraction and is not restated a second time here.

# Why the constants stay next to their subsystem, not in this module

`hooks::ENDPOINT_BREAKER` and `dispatch::PROVIDER_BREAKER` are each a
`BreakerPolicy` value declared beside the code that uses it, carrying its
own doc comment citing the design doc section it comes from (§8.5 for
webhooks, §6.3 for providers). Centralising the *numbers* here, alongside
the decision function, would blur a real distinction: the shape of the
breaker is shared and provider-agnostic; the threshold and cool-down are
each a specific subsystem's own spec'd value, and reading one in this
file with no context would invite silently reusing it for a third
breaker it was never chosen for.

# What must never regress here, unit-tested directly

The one property worth naming because getting it backwards is the whole
bug class this extraction guards against: the failure that *reaches* the
threshold resets the counter to zero, not to the threshold value, in the
same write that opens the circuit — see `#40`'s own note on why the
post-cooldown allowance starts fresh at zero rather than reopening on the
very next failure. `on_failure`'s own test module proves this holds at,
just under, and (defensively) past the threshold; both call sites' live
suites (`hooks_live_postgres.rs`'s circuit tests,
`dispatch_live_postgres.rs::an_open_circuit_routes_new_messages_to_the_
alternative_instead_of_rejecting`) prove the wiring around it — the write,
the `if_match`, and the claim-loop consequence of an open circuit — is
unchanged by moving the decision out from under it.
