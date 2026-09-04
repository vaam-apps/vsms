#![doc = include_str!("error.md")]

use cratestack::CratestackError;
use cratestack::sqlx;

/// One [`super::JobHandler`]'s own failure — see this module's own doc
/// for why it's shaped the way it is.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    /// A step failed through a `CrateStack` delegate call. See this
    /// module's own doc for the full reasoning, including why `context`
    /// is a real field rather than a transparent `#[from]`.
    #[error("{context}: {source}")]
    Database {
        /// Which step of the job failed — always one of the five job
        /// modules' own `pub(crate) const CTX_*` values, never a literal
        /// typed at the call site.
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
    /// a misconfigured deployment, not a job that ran and failed.
    /// Constructed directly by [`super::run_one`], never returned by a
    /// handler's own [`super::JobHandler::run`].
    #[error("no handler registered for kind {kind:?}")]
    NoHandler {
        /// The job row's own `kind`, verbatim.
        kind: String,
    },

    /// For test doubles only — production handlers must use a typed
    /// variant. See this module's own doc for the one real caller.
    #[error("{0}")]
    Injected(String),
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
    /// section on this cleanup for the reproduced failure. This proves
    /// the *shape* only;
    /// `jobs::tests::every_context_literal_matches_the_documented_wording`
    /// (in `jobs.rs`) is the guard that proves the thirteen real wordings
    /// themselves — see this module's own doc for why both exist.
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

    /// `Injected`'s `Display` must reproduce the string it wraps verbatim,
    /// with no added prefix — `jobs_live_postgres.rs`'s `ScriptedHandler`
    /// asserts `Job.lastError == Some("scripted failure".to_owned())`
    /// after this variant round-trips through `apply_outcome`'s own
    /// `error.to_string()`, and that assertion predates this type.
    #[test]
    fn injected_display_is_the_wrapped_string_verbatim() {
        let error = JobError::Injected("scripted failure".to_owned());
        assert_eq!(error.to_string(), "scripted failure");
    }
}
