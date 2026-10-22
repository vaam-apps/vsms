#![doc = include_str!("policy.md")]

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

/// How the fake answers one `SendTextMessage` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitResponse {
    /// `200` with a fresh `MessageId`.
    Accept,
    /// `200` with an empty object. AWS has taken the message, but the
    /// caller never learns its id: the adapter's `Indeterminate` case.
    AcceptWithoutMessageId,
    /// An AWS JSON 1.0 error: `x-amzn-ErrorType` plus a
    /// `{"__type", "Message", "Reason"}` body.
    Error {
        /// HTTP status.
        status: u16,
        /// Bare error type, e.g. `ThrottlingException`.
        error_type: String,
        /// The `Reason` member, when the error type has one.
        reason: Option<String>,
        /// Human-readable text.
        message: String,
    },
}

/// One delivery event to publish to the topic after a message is accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventStep {
    /// Delay measured from the moment the submit was answered.
    pub after: Duration,
    /// AWS's `eventType`, e.g. `TEXT_DELIVERED`.
    pub event_type: String,
    /// AWS's `isFinal`.
    pub is_final: bool,
}

impl EventStep {
    /// An arbitrary event.
    #[must_use]
    pub fn new(after: Duration, event_type: impl Into<String>, is_final: bool) -> Self {
        Self {
            after,
            event_type: event_type.into(),
            is_final,
        }
    }

    /// `TEXT_PENDING`, not final: progress.
    #[must_use]
    pub fn pending(after: Duration) -> Self {
        Self::new(after, "TEXT_PENDING", false)
    }

    /// `TEXT_DELIVERED`, final: a handset receipt.
    #[must_use]
    pub fn delivered(after: Duration) -> Self {
        Self::new(after, "TEXT_DELIVERED", true)
    }

    /// `TEXT_SUCCESSFUL`, final: accepted by the carrier, no handset
    /// receipt to follow.
    #[must_use]
    pub fn carrier_accepted(after: Duration) -> Self {
        Self::new(after, "TEXT_SUCCESSFUL", true)
    }

    /// `TEXT_CARRIER_UNREACHABLE`, final.
    #[must_use]
    pub fn unreachable(after: Duration) -> Self {
        Self::new(after, "TEXT_CARRIER_UNREACHABLE", true)
    }
}

/// The whole outcome of one submit: the HTTP answer, how long to wait
/// before giving it, and the events to publish if it was accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitDecision {
    /// What the caller receives.
    pub response: SubmitResponse,
    /// Published only when `response` accepted the message.
    pub events: Vec<EventStep>,
    /// Held before answering. The message is recorded (and, if accepted,
    /// its events scheduled) first, so a delay longer than the caller's
    /// timeout is a post-accept timeout, not a refusal.
    pub response_delay: Duration,
}

impl SubmitDecision {
    /// Accept, then publish `events`.
    #[must_use]
    pub fn accepted_with_events(events: Vec<EventStep>) -> Self {
        Self {
            response: SubmitResponse::Accept,
            events,
            response_delay: Duration::ZERO,
        }
    }

    /// Accept, report `TEXT_PENDING` halfway, then `TEXT_DELIVERED` after
    /// `after`. The common case, with the progress report AWS sends first.
    #[must_use]
    pub fn delivered_after(after: Duration) -> Self {
        Self::accepted_with_events(vec![
            EventStep::pending(after / 2),
            EventStep::delivered(after),
        ])
    }

    /// Accept and publish nothing: AWS took the message and never reported.
    #[must_use]
    pub fn accepted_silently() -> Self {
        Self::accepted_with_events(Vec::new())
    }

    /// `200 {}`, with `events` still published under a `MessageId` the
    /// caller never saw.
    #[must_use]
    pub fn without_message_id(events: Vec<EventStep>) -> Self {
        Self {
            response: SubmitResponse::AcceptWithoutMessageId,
            events,
            response_delay: Duration::ZERO,
        }
    }

    /// An error with nothing sent.
    #[must_use]
    pub fn error(
        status: u16,
        error_type: impl Into<String>,
        reason: Option<&str>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            response: SubmitResponse::Error {
                status,
                error_type: error_type.into(),
                reason: reason.map(str::to_owned),
                message: message.into(),
            },
            events: Vec::new(),
            response_delay: Duration::ZERO,
        }
    }

    /// `ThrottlingException`, 400.
    #[must_use]
    pub fn throttled() -> Self {
        Self::error(400, "ThrottlingException", None, "Rate exceeded")
    }

    /// The SMS sandbox: `ConflictException`,
    /// `DESTINATION_PHONE_NUMBER_NOT_VERIFIED`. What every new account
    /// answers for any number not verified in the console.
    #[must_use]
    pub fn sandbox_unverified() -> Self {
        Self::error(
            400,
            "ConflictException",
            Some("DESTINATION_PHONE_NUMBER_NOT_VERIFIED"),
            "The destination phone number is not verified for the SMS sandbox",
        )
    }

    /// `ConflictException`, `DESTINATION_PHONE_NUMBER_OPTED_OUT`.
    #[must_use]
    pub fn opted_out() -> Self {
        Self::error(
            400,
            "ConflictException",
            Some("DESTINATION_PHONE_NUMBER_OPTED_OUT"),
            "The destination phone number has opted out",
        )
    }

    /// `ServiceQuotaExceededException`: the monthly spend limit.
    #[must_use]
    pub fn spend_limit_reached() -> Self {
        Self::error(
            402,
            "ServiceQuotaExceededException",
            Some("MONTHLY_SPEND_LIMIT_REACHED_FOR_TEXT"),
            "The monthly spend limit for text messages has been reached",
        )
    }

    /// Hold the answer for `delay` (see [`Self::response_delay`]).
    #[must_use]
    pub fn delayed(mut self, delay: Duration) -> Self {
        self.response_delay = delay;
        self
    }
}

/// Which decision each submit gets.
#[derive(Debug)]
pub enum FaultPolicy {
    /// The same decision for every submit — what a demo wants.
    Always(SubmitDecision),
    /// One decision per submit, in order. An exhausted script answers
    /// `500 InternalServerException` and logs an error, rather than quietly
    /// accepting: a test that submits more often than it scripted has
    /// found something, and should see it.
    Scripted(Mutex<VecDeque<SubmitDecision>>),
}

impl FaultPolicy {
    /// See [`FaultPolicy::Always`].
    #[must_use]
    pub fn always(decision: SubmitDecision) -> Self {
        Self::Always(decision)
    }

    /// See [`FaultPolicy::Scripted`].
    #[must_use]
    pub fn scripted(decisions: impl IntoIterator<Item = SubmitDecision>) -> Self {
        Self::Scripted(Mutex::new(decisions.into_iter().collect()))
    }

    /// The next decision, or `None` when a script has run out.
    pub(crate) fn next(&self) -> Option<SubmitDecision> {
        match self {
            Self::Always(decision) => Some(decision.clone()),
            Self::Scripted(queue) => queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FaultPolicy, SubmitDecision};

    #[test]
    fn a_script_runs_out_rather_than_repeating() {
        let policy = FaultPolicy::scripted([SubmitDecision::throttled()]);
        assert_eq!(policy.next(), Some(SubmitDecision::throttled()));
        assert_eq!(policy.next(), None);
    }

    #[test]
    fn always_repeats() {
        let policy = FaultPolicy::always(SubmitDecision::accepted_silently());
        for _ in 0..3 {
            assert_eq!(policy.next(), Some(SubmitDecision::accepted_silently()));
        }
    }
}
