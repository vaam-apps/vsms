use cratestack::CratestackError;
use cratestack::sqlx;

/// One [`super::JobHandler`]'s own failure — the typed replacement for the
/// `Result<(), String>` every handler used to return (cleanup PR A,
/// AGENTS.md's own "`JobError` replaces the `String` job boundary"
/// section).
///
/// # Why not `#[error(transparent)] Database(#[from] CratestackError)`
///
/// A bare transparent wrapper was the first shape considered, and it was
/// rejected on inspection, not by preference: every one of these call
/// sites was already writing its own context prefix by hand —
/// `format!("expiring stale submitted messages: {error}")`, not just
/// `format!("{error}")` — because `Job.lastError` is the *only* place an
/// operator sees why a job died (this crate has no separate span/field for
/// "which step"), and a bare `CratestackError`'s own `Display` says nothing
/// about which of a job's several database calls actually failed. A
/// transparent `#[from]` wrapper would have silently dropped that prefix
/// the moment every call site's `format!` was deleted in favour of `?`.
/// [`JobError::Database`]/[`JobError::Sql`] instead carry the prefix as a
/// real field (`context`), with `#[error("{context}: {source}")]`
/// reproducing the exact same string `format!("{context}: {source}")`
/// already produced — so `Job.lastError` reads identically to before this
/// type existed, and `std::error::Error::source()` now also works, which
/// string concatenation never allowed.
///
/// # What every job's own failure actually is
///
/// Grepped across all five [`super::JobHandler`]s registered in
/// [`super::default_registry`] before writing this type, not assumed: every
/// failure any of them can produce is either a `CratestackError` from a
/// `CrateStack` delegate call, or (`reap_outbox`/`anchor_audit` only) a raw
/// `sqlx::Error` from one of the two R1-exempt queries against
/// `cratestack_event_outbox`/`cratestack_audit` — neither table has a
/// schema model, so neither has a delegate to return a `CratestackError`
/// from in the first place (see `CONTRIBUTING.md`'s own R1 exceptions
/// table). No job produces a [`sms_provider::ProviderError`] — that's
/// `dispatch.rs`'s own boundary, not this one. [`JobError::Other`] is the
/// one catch-all variant this type does carry, and it exists for exactly
/// one real caller today: `sms-worker`'s own `jobs_live_postgres.rs`
/// live-test suite injects an arbitrary, source-less failure through it to
/// exercise the generic backoff/`dead` state machine (`apply_failure`)
/// independently of any particular job's own database calls — no
/// [`super::JobHandler`] registered in [`super::default_registry`]
/// constructs one.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    /// A step failed through a `CrateStack` delegate call — a real `@@allow`
    /// policy denial, an unswallowed `if_match` CAS loss (see
    /// [`super::swallow_stale_write`]'s own doc for which shapes *are*
    /// swallowed before ever reaching a `JobHandler`), or any other
    /// `CratestackError` a delegate can produce.
    #[error("{context}: {source}")]
    Database {
        /// Which step of the job failed, e.g. "expiring stale submitted
        /// messages". Free text, not a further enum: the five handlers
        /// share no vocabulary of steps worth naming as variants, and
        /// forcing one wouldn't buy anything `#[source]` doesn't already
        /// give a caller that wants to match on the real underlying cause.
        context: &'static str,
        /// The delegate's own error.
        #[source]
        source: CratestackError,
    },

    /// A raw-SQL step failed — only reachable from `reap_outbox`'s and
    /// `anchor_audit`'s own R1-exempt queries. See [`JobError::Database`]'s
    /// own doc for why `context` exists.
    #[error("{context}: {source}")]
    Sql {
        /// Same role as [`JobError::Database`]'s own `context`.
        context: &'static str,
        /// The raw `sqlx` query's own error.
        #[source]
        source: sqlx::Error,
    },

    /// No [`super::JobHandler`] is registered for this job's own `kind` —
    /// a misconfigured deployment (a `Job` row inserted, or scheduled,
    /// with a `kind` no [`super::default_registry`] entry claims), not a
    /// job that ran and failed. Constructed directly by
    /// [`super::run_one`], never returned by a handler's own
    /// [`super::JobHandler::run`].
    #[error("no handler registered for kind {kind:?}")]
    NoHandler {
        /// The job row's own `kind`, verbatim — `{kind:?}` reproduces the
        /// exact `format!("no handler registered for kind {:?}", job.kind)`
        /// this variant replaces, `Debug`-quoted the same way.
        kind: String,
    },

    /// A job failed for a reason with no `CratestackError`/`sqlx::Error`
    /// (or any other [`std::error::Error`]) to wrap — see this type's own
    /// doc for why the one real caller today is a live-test double, not a
    /// registered [`super::JobHandler`]. `#[error("{0}")]` rather than
    /// `#[source]`-carrying: there is nothing to chain, by construction.
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::JobError;
    use cratestack::CratestackError;

    /// `JobError::Database`'s `Display` must reproduce exactly the string
    /// every call site used to build by hand — this is the guard that
    /// makes the refactor behaviour-preserving, not just type-preserving.
    /// `Job.lastError` is operator-facing (the Jobs console renders it
    /// verbatim); a silently reworded string is exactly what this test
    /// exists to catch. Broken and restored once — see AGENTS.md's own
    /// section on this cleanup for the reproduced failure.
    #[test]
    fn database_display_matches_the_pre_existing_hand_built_format() {
        let source = CratestackError::Unauthorized("read policy denied this operation".to_owned());
        let expected = format!("expiring stale submitted messages: {source}");
        let error = JobError::Database {
            context: "expiring stale submitted messages",
            source,
        };
        assert_eq!(error.to_string(), expected);
    }

    /// Same proof, for the raw-`sqlx` variant `reap_outbox`/`anchor_audit`
    /// actually produce.
    #[test]
    fn sql_display_matches_the_pre_existing_hand_built_format() {
        // sqlx::Error has no public constructor cheap enough to build a
        // realistic instance from in a unit test; `Protocol` (a bare
        // `String` message, no I/O, no live connection) is the one variant
        // that is. What's under test is the format string, not any
        // particular `sqlx::Error` payload.
        let source = cratestack::sqlx::Error::Protocol("connection reset".to_owned());
        let expected = format!("reaping delivered event outbox rows: {source}");
        let error = JobError::Sql {
            context: "reaping delivered event outbox rows",
            source,
        };
        assert_eq!(error.to_string(), expected);
    }

    /// `NoHandler`'s `Display` must reproduce the exact
    /// `format!("no handler registered for kind {:?}", job.kind)` string
    /// `run_one` used to build directly, `Debug`-quoting included.
    #[test]
    fn no_handler_display_matches_the_pre_existing_hand_built_format() {
        let kind = "not_a_real_kind".to_owned();
        let expected = format!("no handler registered for kind {kind:?}");
        let error = JobError::NoHandler { kind };
        assert_eq!(error.to_string(), expected);
    }

    /// `Other`'s `Display` must reproduce the string it wraps verbatim,
    /// with no added prefix — `jobs_live_postgres.rs`'s `ScriptedHandler`
    /// asserts `Job.lastError == Some("scripted failure".to_owned())`
    /// after this variant round-trips through `apply_outcome`'s own
    /// `error.to_string()`, and that assertion predates this type.
    #[test]
    fn other_display_is_the_wrapped_string_verbatim() {
        let error = JobError::Other("scripted failure".to_owned());
        assert_eq!(error.to_string(), "scripted failure");
    }
}
