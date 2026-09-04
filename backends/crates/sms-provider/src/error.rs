use std::time::Duration;

/// A type-erased, real cause behind a [`ProviderError`] — a `reqwest::Error`,
/// a `url::ParseError`, whatever the transport underneath happened to
/// return — so `Unavailable`/`Indeterminate` don't reduce a real error chain
/// to a formatted string the moment they cross into this crate's own
/// vocabulary. `dyn Error + Send + Sync + 'static`, not a generic parameter:
/// this crate stays adapter-agnostic (one `ProviderError` enum shared by
/// every HTTP adapter, and eventually SMPP), so it cannot name a concrete
/// transport error type without coupling itself to one adapter's transport
/// — see the crate's own module doc on staying pure. Boxing costs one
/// allocation per real failure, never per success; that's the trade this
/// crate already makes for `code`/`message` being `String` rather than a
/// borrowed `&str`.
pub type BoxDynError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// What went wrong submitting to, or asking about, a provider.
///
/// §6.1's own framing is the reason there are exactly four real variants,
/// not one generic `Error(String)`: *"Most gateway failover bugs are really
/// error-classification bugs: a provider returns a 400 that actually means
/// 'your sender ID isn't approved' and the router faithfully retries it on
/// three more providers, burning credit each time."* Each variant carries
/// exactly one [`RoutingConsequence`] — see [`ProviderError::routing`] —
/// so that mapping is a match a compiler checks, not a comment a future
/// adapter author has to remember to honour.
///
/// `code`/`retry_after`/`message` already give `Permanent`, `Transient` and
/// `Rejected` a structured surface — a caller reads `code` without parsing
/// `message`. `Unavailable` and `Indeterminate` had no such structure before
/// the DRY-up that consolidated every HTTP adapter's transport-error
/// classification into one place (`sms-provider-http`): both were
/// constructed straight from a real `std::error::Error` (a `reqwest::Error`
/// failing to connect, to finish a body, or to parse a response) and
/// immediately flattened it into `message`, discarding the original error
/// and its own `source()` chain. `source` on both fixes that — `#[source]`,
/// so `std::error::Error::source()` on a `ProviderError` genuinely returns
/// it, and a caller building a `tracing`/`anyhow` report gets the real chain
/// rather than a second copy of the same text `message` already holds.
/// `None` where no real error object exists (a status-code-and-body
/// response, or a purely local check like "this URL parsed but isn't a
/// base") — there's nothing to chain to, so the field says so honestly
/// rather than being forced to hold a synthetic placeholder.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// This specific submission cannot succeed on this provider, ever, but
    /// nothing is wrong with the caller's request — a sender ID that isn't
    /// approved here, an unsupported feature. Route to a different
    /// provider; retrying this one is pointless.
    #[error("{code}: {message}")]
    Permanent {
        /// The provider's own error code, for `Message.stateReason` and logs.
        code: String,
        /// A human-readable description, for logs — never shown to the
        /// message's ultimate recipient.
        message: String,
    },

    /// A transient condition on the provider's side — rate limiting, a
    /// momentary backend error. Retry this same provider after the stated
    /// delay; do not fail over.
    #[error("retry after {retry_after:?}: {message}")]
    Transient {
        /// How long to wait before retrying this same provider.
        retry_after: Duration,
        /// A human-readable description, for logs.
        message: String,
    },

    /// The provider is unreachable or answering with server errors broadly,
    /// not about this one message. Mark it degraded, fail this submission
    /// over to the next route, and let the circuit breaker (§6.3: five
    /// consecutive `Unavailable` opens it for 60s) decide when to trust it
    /// again.
    #[error("provider unavailable: {message}")]
    Unavailable {
        /// A human-readable description, for logs.
        message: String,
        /// The real error behind `message`, when one exists — see the enum's
        /// own doc on why this is `Option<BoxDynError>` rather than a bare
        /// `BoxDynError`.
        #[source]
        source: Option<BoxDynError>,
    },

    /// The request itself is invalid in a way no retry or failover fixes —
    /// a malformed destination, a body the provider refuses outright. Fail
    /// the message; sending it to another provider would just fail there
    /// too, for the same reason.
    #[error("{code}: {message}")]
    Rejected {
        /// The provider's own error code, for `Message.stateReason` and logs.
        code: String,
        /// A human-readable description, for logs.
        message: String,
    },

    /// This provider doesn't implement the operation at all (the default
    /// [`crate::SmsProvider::poll_status`], an SMPP-only adapter's
    /// `parse_dlr`). Not a failure of this attempt — the caller asked for
    /// something this adapter was never going to be able to do.
    #[error("not supported by this provider")]
    Unsupported,

    /// The request reached the provider, or may have — a response/read
    /// timeout after the request was already written, a connection reset
    /// while awaiting or reading the response, or a `2xx` response we
    /// cannot make sense of (unparseable body, missing the field a
    /// provider ref is extracted from). In every one of these cases the
    /// provider may already have accepted the submission and sent the SMS;
    /// there is simply no way to tell from here.
    ///
    /// Distinct from [`Self::Unavailable`] in exactly the respect that
    /// matters: `Unavailable` means the request never got anywhere near
    /// being accepted (the provider refused the connection, or answered
    /// broadly with server errors), so retrying — this provider or the
    /// next — is safe. `Indeterminate` means retrying is *not* safe,
    /// because a retry that lands on a provider that already sent the
    /// first attempt is a duplicate SMS to a real handset. Callers must
    /// not fail this submission over to another route and must not retry
    /// it; the message should move to a state that waits for a delivery
    /// receipt to resolve the ambiguity (or ages out), never one that
    /// re-attempts sending.
    #[error("submission outcome unknown, possibly sent: {message}")]
    Indeterminate {
        /// A human-readable description, for logs.
        message: String,
        /// The real error behind `message`, when one exists — same
        /// reasoning as [`Self::Unavailable`]'s own `source` field.
        #[source]
        source: Option<BoxDynError>,
    },
}

/// The one routing decision a [`ProviderError`] variant implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingConsequence {
    /// Try the same provider again after the given delay. Do not advance to
    /// the route's `failoverRouteId`.
    RetryThisProvider {
        /// How long to wait before retrying.
        after: Duration,
    },
    /// This provider is out for this message. Advance to the next route
    /// (§6.3: capped at two failover hops).
    TryNextRoute,
    /// This provider is out for *everything* until its circuit breaker
    /// half-opens. Advance to the next route, and record the failure toward
    /// the five-in-a-row threshold that opens the breaker.
    OpenCircuitAndTryNextRoute,
    /// Nothing about retrying or rerouting helps. Fail the message outright.
    FailMessage,
    /// The submission's outcome is unknown and may already have reached
    /// the recipient. Do not retry this provider, do not fail over to
    /// another, and do not fail the message — any of those three risk a
    /// second real SMS. The only safe move is to stop touching this
    /// message from the send path entirely and let a delivery receipt (or
    /// the grace-period expiry job, absent one) resolve it later.
    HoldIndeterminate,
}

impl ProviderError {
    /// The routing consequence this error implies — see the module doc for
    /// why this is a total, compiler-checked match rather than callers
    /// re-deriving the same four-way decision from string-matching a
    /// provider's error code.
    #[must_use]
    pub const fn routing(&self) -> RoutingConsequence {
        match self {
            Self::Permanent { .. } => RoutingConsequence::TryNextRoute,
            Self::Transient { retry_after, .. } => RoutingConsequence::RetryThisProvider {
                after: *retry_after,
            },
            Self::Unavailable { .. } => RoutingConsequence::OpenCircuitAndTryNextRoute,
            Self::Rejected { .. } | Self::Unsupported => RoutingConsequence::FailMessage,
            Self::Indeterminate { .. } => RoutingConsequence::HoldIndeterminate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderError, RoutingConsequence};
    use std::time::Duration;

    #[test]
    fn every_variant_maps_to_exactly_the_consequence_section_61_documents() {
        let cases = [
            (
                ProviderError::Permanent {
                    code: "SENDER_ID_NOT_APPROVED".to_owned(),
                    message: "sender id not approved".to_owned(),
                },
                RoutingConsequence::TryNextRoute,
            ),
            (
                ProviderError::Transient {
                    retry_after: Duration::from_secs(5),
                    message: "rate limited".to_owned(),
                },
                RoutingConsequence::RetryThisProvider {
                    after: Duration::from_secs(5),
                },
            ),
            (
                ProviderError::Unavailable {
                    message: "connection refused".to_owned(),
                    source: None,
                },
                RoutingConsequence::OpenCircuitAndTryNextRoute,
            ),
            (
                ProviderError::Rejected {
                    code: "INVALID_DESTINATION".to_owned(),
                    message: "malformed msisdn".to_owned(),
                },
                RoutingConsequence::FailMessage,
            ),
            (ProviderError::Unsupported, RoutingConsequence::FailMessage),
            (
                ProviderError::Indeterminate {
                    message: "read timeout after the request was sent".to_owned(),
                    source: None,
                },
                RoutingConsequence::HoldIndeterminate,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(error.routing(), expected, "{error:?}");
        }
    }

    /// The specific bug §6.1 warns about: a `Permanent` misrouted as if it
    /// were `Unavailable` would open the circuit breaker on healthy traffic
    /// instead of just skipping to the next route for this one message.
    #[test]
    fn permanent_never_opens_the_circuit_breaker() {
        let error = ProviderError::Permanent {
            code: "X".to_owned(),
            message: "x".to_owned(),
        };
        assert_ne!(
            error.routing(),
            RoutingConsequence::OpenCircuitAndTryNextRoute
        );
    }

    /// The bug this whole ticket exists to prevent: an `Indeterminate`
    /// routed as if it were `Unavailable`, `Permanent`, or `Transient`
    /// would retry or fail over a submission that may have already sent
    /// a real SMS — a duplicate to the recipient's handset.
    #[test]
    fn indeterminate_never_retries_or_fails_over() {
        let error = ProviderError::Indeterminate {
            message: "x".to_owned(),
            source: None,
        };
        assert_eq!(error.routing(), RoutingConsequence::HoldIndeterminate);
        assert_ne!(error.routing(), RoutingConsequence::TryNextRoute);
        assert_ne!(
            error.routing(),
            RoutingConsequence::OpenCircuitAndTryNextRoute
        );
        assert!(!matches!(
            error.routing(),
            RoutingConsequence::RetryThisProvider { .. }
        ));
        assert_ne!(error.routing(), RoutingConsequence::FailMessage);
    }

    /// `#[source]` is only useful if `std::error::Error::source()` genuinely
    /// returns the boxed cause, not merely if the field compiles — thiserror
    /// generates that impl, but nothing else here proves it wires up
    /// correctly for a field typed `Option<BoxDynError>` rather than a bare
    /// `BoxDynError`. `Unavailable` and `Indeterminate` are both checked:
    /// one real underlying error, one `None`, so both branches of the
    /// `Option` are exercised.
    #[test]
    fn source_reaches_the_real_cause_when_one_was_recorded() {
        let underlying = std::io::Error::other("socket reset");
        let with_source = ProviderError::Unavailable {
            message: "submit request failed: socket reset".to_owned(),
            source: Some(Box::new(underlying)),
        };
        let cause = std::error::Error::source(&with_source)
            .expect("a recorded source must be returned by source()");
        assert_eq!(cause.to_string(), "socket reset");

        let without_source = ProviderError::Indeterminate {
            message: "no resource id in the response".to_owned(),
            source: None,
        };
        assert!(
            std::error::Error::source(&without_source).is_none(),
            "a None source must not be reported as a cause"
        );
    }
}
