`Role::Jobs`'s real body — #35. The generic background queue: claim,
dispatch by `kind` through [`JobHandler`], transition on the outcome
per §7.5's state machine.

`Job` candidates arrive from [`crate::claim::claim_batch`] in one of two
states — see [`crate::claim::Claimable for Job`]'s own doc for why a
`pending` result means "just reclaimed, not actually claimed yet" and
must not be executed this tick.

Five [`JobHandler`]s are registered as of #64 — [`expire_stale`] (M2),
[`reap_outbox`] (#42), [`purge_retention`] (#67), [`anchor_audit`] (#68),
and [`grey_route_watch`] (#64) — proving the pipeline end to end without
depending on infrastructure this milestone doesn't build (Orange
balance/health endpoints, backup verification). The retention-law
question that used to block `purge_retention` (§7.5's own table, issue
#5) was resolved 2026-08-11: 90-day minimisation, no split ledger — see
`purge_retention`'s own module doc. [`grey_route_watch`] is not one of
§7.5's own nine named kinds at all — see its own module doc for why it
exists regardless. The remaining five `kind`s §7.5's own table names are
real, tracked gaps, not a silently dropped scope — see the module's own
issue for the follow-up.

[`JobHandler::run`] returns a typed [`JobError`], not a bare `String` —
see that type's own doc for the full reasoning (cleanup PR A, AGENTS.md's
own "`JobError` replaces the `String` job boundary" section). The short
version: `Job.lastError` is still a plain `String` column (unchanged —
this is a Rust-side typed-boundary cleanup, not a schema change), and
[`apply_outcome`] is still the one place a `JobError` is ever collapsed
back into one, via its own `Display`, reproducing the exact wording every
handler used to build by hand.
