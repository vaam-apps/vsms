[`super::JobHandler`]'s own failure — the typed replacement for the
`Result<(), String>` every handler used to return (cleanup PR A,
AGENTS.md's own "`JobError` replaces the `String` job boundary" section).

# Why not `#[error(transparent)] Database(#[from] CratestackError)`

A bare transparent wrapper was the first shape considered, and it was
rejected on inspection, not by preference: every one of these call sites
was already writing its own context prefix by hand —
`format!("expiring stale submitted messages: {error}")`, not just
`format!("{error}")` — because `Job.lastError` is the *only* place an
operator sees why a job died (this crate has no separate span/field for
"which step"), and a bare `CratestackError`'s own `Display` says nothing
about which of a job's several database calls actually failed. A
transparent `#[from]` wrapper would have silently dropped that prefix the
moment every call site's `format!` was deleted in favour of `?`.
[`JobError::Database`]/[`JobError::Sql`] instead carry the prefix as a
real field (`context`), with `#[error("{context}: {source}")]`
reproducing the exact same string `format!("{context}: {source}")`
already produced — so `Job.lastError` reads identically to before this
type existed, and `std::error::Error::source()` now also works, which
string concatenation never allowed.

# What every job's own failure actually is

Grepped across all five [`super::JobHandler`]s registered in
[`super::default_registry`] before writing this type, not assumed: every
failure any of them can produce is either a `CratestackError` from a
`CrateStack` delegate call, or (`reap_outbox`/`anchor_audit` only) a raw
`sqlx::Error` from one of the two R1-exempt queries against
`cratestack_event_outbox`/`cratestack_audit` — neither table has a schema
model, so neither has a delegate to return a `CratestackError` from in
the first place (see `CONTRIBUTING.md`'s own R1 exceptions table). No job
produces a [`sms_provider::ProviderError`] — that's `dispatch.rs`'s own
boundary, not this one. [`JobError::Injected`] is the one catch-all
variant this type does carry, deliberately named to refuse casual
production use rather than `Other`: it exists for exactly one real caller
today, `sms-worker`'s own `jobs_live_postgres.rs` live-test suite's
`ScriptedHandler`, which injects an arbitrary, source-less failure
through it to exercise the generic backoff/`dead` state machine
(`apply_failure`) independently of any particular job's own database
calls. No [`super::JobHandler`] registered in [`super::default_registry`]
constructs one — `cfg(test)` can't reach an integration test, so the
variant itself has to exist on the real, production `JobError`, and its
name is the only thing standing between that and a handler reaching for
the easy, untyped escape hatch.

# Two independent guards, not one — know which is which

This module's own `#[cfg(test)] mod tests` proves the four `Display`
*shapes* are right (`"{context}: {source}"`, `"{context}: {source}"`,
`"no handler registered for kind {kind:?}"`, and the bare wrapped
string) — but each of those tests supplies its own `context` literal by
hand, so on its own it cannot catch a real call site's wording drifting
out from under it. `jobs::tests::every_context_literal_matches_the_documented_wording`
(in `jobs.rs`) is the second, independent guard that closes exactly that
gap: every job module's own `pub(crate) const CTX_*` — the same constant
its real call site passes as `context` — is asserted
against a hardcoded expected literal that exists only in that test. A
one-character edit to any of the thirteen wordings now has to touch that
table in the same diff, or the table test fails; before this, it could
pass `--lib` and both live suites outright, because the four tests below
had no coupling to the real call sites at all.
