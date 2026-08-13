# Runbook: #65 — kill Orange in staging (Milestone 5's own gate)

Per [#65](https://github.com/vymalo/vsms/issues/65) and §12 of `docs/architecture.md`
("Kill Orange in staging; MTN unaffected, Orange fails over cleanly"): take a
real staging deployment's Orange connectivity down, with real MTN-via-aggregator
capacity configured alongside it, and confirm by observation that:

1. MTN traffic keeps flowing.
2. Orange-destined traffic fails over to MTN and still reaches `submitted`.
3. Nothing double-sends.
4. The circuit breaker reopens once Orange is reachable again.

Same split as `docs/runbooks/36-handset-gate.md` (M2's own gate, and the model
this document follows): **the automatable half is proven rigorously; the half
that needs a real staging deployment and a real Orange/aggregator account is
this runbook.** Which is which is stated plainly in both places, and neither
substitutes for the other.

`crates/sms-worker/tests/kill_orange_gate_live_postgres.rs` is the automated
half — one live-Postgres test, `orange_outage_fails_over_mtn_stays_up_
nothing_double_sends_and_the_breaker_reopens`, that walks a single realistic
timeline (Orange healthy → killed → revived) against **real** adapter code:
`sms_provider_orange_cm::OrangeCmProvider` and `sms_provider_mtn::
MtnAggregatorProvider`, not hand-rolled fakes. It "kills" Orange by dropping a
real, running `sms_fake_orange::FakeOrange` HTTP server, so the very next
submit gets a genuine OS-level `ECONNREFUSED` — the same transport failure a
real outage produces, not an injected `ProviderError`. It "revives" Orange by
binding a second `FakeOrange` to the exact same local port. Proof that
"nothing double-sends" comes from the providers' own request logs (`FakeOrange::
ledger()`, `MockServer::received_requests()`), never from this system's own
`Message` rows — see that test file's own module doc for the full reasoning,
including why one scenario proves all four clauses together rather than four
disjoint tests each reconstructing the same timeline. Run it before this
runbook, as a fast sanity check that the mechanism still works:

```bash
cargo test -p sms-worker --test kill_orange_gate_live_postgres -- --ignored
```

It does not replace this runbook. Nothing about it touches a real Orange
account, a real MTN-aggregator contract, or a real staging network — see
"What the automated suite already proved (and what it can't)" below.

## Prerequisites

- **A real staging deployment** — `deploy/`'s compose stack or Helm chart,
  reachable, with `sms-gateway` and `sms-worker` (`--roles dispatch,scheduler,jobs`
  at minimum) both running against a real Postgres.
- **Orange Cameroon SMS API credentials** on that deployment, exactly as
  `docs/runbooks/36-handset-gate.md`'s own prerequisites describe — this
  repo has never had a real contract, and every provider-facing detail beyond
  §6.2's documented shapes is inference from public `OneAPI` docs.
- **MTN-via-aggregator credentials**, configured against `sms-provider-mtn`.
  **No real aggregator contract exists anywhere in this repo** — the crate's
  own module doc, and `OPEN_QUESTIONS.md` §2.1, are explicit that its
  request/response shape (`POST /v1/messages`, Bearer API-key auth, a
  `messageId` in the response) is an **invented placeholder**, chosen to
  match the common pattern across the aggregators §6.2 names as candidates,
  not transcribed from a real vendor's Swagger. If a real aggregator
  contract's shape differs, this crate's request/response structs need
  updating before this runbook can be run for real — its transport-error
  classification (connect-vs-read) is provider-agnostic and needs no change
  regardless.
- **Two `Route` rows** configured against these two `Provider`s, shaped like
  the automated test's own fixture: an operator-scoped Orange route at a
  higher priority, and a wildcard (no `matchOperator`) MTN route at a lower
  priority — the wildcard is what makes Orange-destined failover to MTN
  possible at all, and what makes MTN-destined traffic never consider
  Orange in the first place. `sms-gateway seed-dispatch` seeds a single
  catch-all route by default; this gate needs a second, operator-scoped one
  added on top, by hand or via the admin console's Routes screen (#54).
- **A way to actually interrupt Orange connectivity from this deployment's
  own network path** — see "Interrupting Orange" below; which mechanism is
  available depends entirely on how this specific staging deployment reaches
  Orange's API (a NAT egress rule, a firewall, `deploy/Caddyfile`'s own
  reverse-proxy config, or literally revoking the credential at Orange's own
  developer portal).
- **A human watching the admin console's Messages screen (#50) and the
  Providers screen (#54)**, or `psql` access to watch `messages`/`providers`
  directly — either works; the console is the point of #50/#54 existing at
  all, so preferring it over raw SQL matches this repo's own M4 gate
  ("diagnose a failed message without touching SQL").

## Interrupting Orange — pick one, in order of preference

Real staging environments differ enough that this runbook can't prescribe
one mechanism. In order of how cleanly each isolates "Orange specifically,"
not the whole network:

1. **Revoke or rotate the Orange `client_id`/`client_secret`** at the
   credential source this deployment reads them from, without restarting
   `sms-worker`. Orange's own token endpoint then answers `401`, which
   `sms-provider-orange-cm`'s own `bad_credentials_at_the_token_endpoint_
   are_permanent` test already proves maps to `ProviderError::Permanent` —
   **this exercises a different `RoutingConsequence` than the automated
   suite's connection-refused scenario** (`TryNextRoute`, not
   `OpenCircuitAndTryNextRoute` — see `dispatch.rs`'s own table). Failover
   still happens, but the circuit breaker never opens this way (`Permanent`
   never opens it, by design — `crates/sms-provider/src/error.rs`'s own
   `permanent_never_opens_the_circuit_breaker`). **This mechanism alone
   cannot prove clause 4** (the breaker reopening) — pair it with one of the
   two below, or run this one for clauses 1–3 and a different one for 4.
2. **A firewall rule or NAT egress block** against Orange's API host from
   this deployment's own network path — the closest real-world match to the
   automated suite's own connection-refused mechanism, and the one that
   *does* open the circuit breaker (`ProviderError::Unavailable` →
   `OpenCircuitAndTryNextRoute`).
3. **Point `--orange-base-url` at an address nothing listens on**, restart
   `sms-worker` — the bluntest option, and the one furthest from "Orange
   itself broke" (it proves the mechanism, not a real-world failure mode of
   Orange's own infrastructure) — prefer option 2 if at all available.

Whichever mechanism is used, **write down which one it was** in the record
of this run — clause 4's own "reopens" observation only means what it claims
to mean if reversing the *same* mechanism is what's checked in the recovery
step.

## Running the gate

1. **Send a burst of MTN-destined traffic** (`operator: mtn` in the send
   request, or a recipient whose MSISDN classifies as MTN) and confirm it
   reaches `delivered` normally — this is the pre-outage baseline for
   clause 1, the same role the automated suite's own baseline phase plays.
2. **Send a burst of Orange-destined traffic** and confirm it reaches
   `delivered` normally too — the pre-outage baseline for clause 2.
3. **Interrupt Orange** (see above), noting the wall-clock time.
4. **Immediately send a fresh burst of MTN-destined traffic.** Watch it
   reach `submitted`/`delivered` on the same timeline as before the outage —
   clause 1. If it stalls or slows, that is a real finding, not something to
   explain away — see "What would falsify this" below.
5. **Immediately send a fresh burst of Orange-destined traffic.** Watch each
   message's `providerId` (Messages detail screen, #50) move from the Orange
   provider to the MTN provider, and the message still reach `submitted` —
   clause 2. `stateReason` should mention a failover reroute, matching the
   automated suite's own assertion.
6. **Watch the Providers screen (#54).** After enough consecutive failures
   (§6.3: five) against the interrupted provider, its circuit should show
   as open, with the remaining wait time visible.
7. **Check for duplicates the way the automated suite does — against
   Orange's own side, not this system's database.** If option 1 (credential
   revocation) was used, Orange's own developer-portal usage dashboard or
   support channel is the closest real analogue to `FakeOrange::ledger()`.
   If option 2 or 3 was used (nothing ever accepted the connection), there
   is nothing on Orange's side to check — the same property the automated
   suite's own `orange_ledger.submits().len()` assertion proves formally
   for its equivalent scenario, and the reason clause 3 is the one this
   runbook can add the least beyond what's already automated.
8. **Restore Orange connectivity** — reverse whichever mechanism was used in
   step 3.
9. **Send a fresh burst of Orange-destined traffic** once the circuit's
   cool-down has elapsed (§6.3: 60 seconds after it opened, not after step 8
   — check the Providers screen's own countdown rather than guessing). It
   should route straight back to Orange — `providerId` on the new messages
   should show the Orange provider, not MTN — and reach `submitted` there.
   This is clause 4.

## What "pass" looks like

Every one of the four clauses observed as described above, with the same
distinction the automated suite's own assertions draw: clause 2 requires
seeing the message actually routed through MTN (`providerId` changed), not
merely that it "eventually succeeded" by some other means; clause 4 requires
seeing fresh routing decisions pick Orange again, not merely that some
already-failed-over message eventually got there.

**A message reaching MTN that would otherwise have gone to Orange is not a
bug — it's clause 2 doing its job.** Don't be alarmed by recipients whose
network shows a different sending number or route characteristics during the
outage window; that is the expected, intended behaviour of a working
failover, and the same "clean failover, not silent loss" this repo's own M2
handset-gate runbook already had to make explicit for its own double-send
risk.

**If clause 3 cannot be checked against Orange's own side at all** (option 2
or 3 was used, and Orange's support channel has no usage dashboard to
consult), that is an accepted gap in *this* runbook, not a failed gate — say
so plainly in the record of the run, the same way `docs/runbooks/36-handset-
gate.md` names what its own dry run could and couldn't prove.

## What would falsify this

- MTN traffic stalling, slowing materially, or failing during the outage —
  the accepted, documented limitation `dispatch.rs`'s own module doc names
  (`total_tps_ceiling` sums every registered provider's budget, so a burst
  routed to a struggling provider can consume claim slots a healthy one
  could have used) becoming *visible* in practice, not just theoretical.
- Orange-destined traffic reaching `failed`/`rejected` instead of failing
  over, or reaching `submitted` with `providerId` still pointing at Orange
  (meaning the "outage" wasn't actually interrupting submission — check the
  chosen mechanism actually reached the code path that matters).
- A recipient receiving the same message twice with **both** deliveries
  traceable to two separate accepted submissions on Orange's own side (not
  one accepted submission plus an unrelated delivery-report duplicate,
  which is a different, already-known and separately-tracked risk — see
  `OPEN_QUESTIONS.md` §2.3, about a crash *mid*-submit, not this gate's own
  outage-then-failover scenario).
- The circuit breaker never reopening once Orange connectivity is restored
  and the cool-down has genuinely elapsed — check the Providers screen's own
  reported `circuitOpenUntil`, not a guess at timing.

## What the automated suite already proved (and what it can't)

Before this runbook was written, `kill_orange_gate_live_postgres.rs` was run
against a real, disposable Postgres, with real `OrangeCmProvider`/
`MtnAggregatorProvider` adapter code and a real, genuinely-killed-and-revived
`FakeOrange` HTTP server. It proved:

- MTN-destined messages reach `submitted`, through the real MTN adapter,
  while Orange is unreachable — and the assertion is sensitive to this: a
  guard-failure proof (temporarily making provider resolution ignore the
  routing decision and always resolve to Orange) reproduced a real failure
  (`left: queued, right: submitted`) before being reverted.
- Orange-destined messages fail over to the MTN route specifically
  (`providerId` changed, `stateReason` names the reroute) and reach
  `submitted` — a guard-failure proof (temporarily disabling failover
  outright) reproduced the identical failure shape before being reverted.
- Nothing double-sends, checked against `FakeOrange`'s own request ledger
  (zero new entries during the whole outage — every attempted connection
  failed at the transport level before ever reaching Orange's request
  handler) and against the revived Orange's own ledger post-recovery (exactly
  one submission per message). A guard-failure proof (temporarily leaving a
  successfully-submitted message's own row un-finalized, so its already-
  expired lease made it look abandoned and eligible for a genuine resubmit)
  reproduced a real failure — at the suite's own pre-outage sanity check,
  before the outage/recovery phases even ran, since a submit that never
  marks itself done is indistinguishable from a broken send at any point
  downstream. That is a *stronger* result than failing deep in the
  double-send-specific assertions would have been, not a weaker one: the
  mechanism this clause depends on is foundational enough that breaking it
  is caught immediately, not only in the one place it was purpose-built to
  check.
- The circuit breaker opens after five consecutive connection-refused
  failures, and **genuinely reopens** — a fresh routing decision, after the
  cool-down passes, picks Orange again rather than staying on MTN. A
  guard-failure proof (temporarily making the breaker ignore
  `circuitOpenUntil`'s expiry, so once open it never closes) reproduced a
  real failure (`providerId` stayed on MTN instead of returning to Orange)
  before being reverted.

What it **cannot** prove, because nothing about it is real: whether MTN
capacity bought through a real, contracted aggregator behaves the way the
placeholder request/response shape in `sms-provider-mtn` assumes (see that
crate's own module doc and `OPEN_QUESTIONS.md` §2.1); whether Orange's real
failure modes under a genuine network interruption match the transport-level
`ECONNREFUSED` this suite injects, as opposed to a slow timeout, a
mid-response reset, or a `5xx` from a degraded-but-reachable backend; whether
a real staging network's own path to either provider has other failure
characteristics (DNS, TLS, a proxy) this suite's direct-loopback HTTP servers
don't exercise; and, as with every gate in this repo that a real handset
would settle, whether a real recipient's phone actually receives the
failed-over SMS at all. That is what this runbook's own steps are for.
