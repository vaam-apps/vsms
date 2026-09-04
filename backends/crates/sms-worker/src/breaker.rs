#![doc = include_str!("breaker.md")]

use chrono::{DateTime, Duration, Utc};

/// The two fixed parameters of one breaker instance — how many consecutive
/// failures trip it, and how long it stays open once tripped. See
/// `breaker.md` for why the two live instances (`hooks::ENDPOINT_BREAKER`,
/// `dispatch::PROVIDER_BREAKER`) each declare their own `BreakerPolicy`
/// value rather than sharing one defined here.
#[derive(Debug, Clone, Copy)]
pub struct BreakerPolicy {
    /// Consecutive failures before the breaker trips.
    pub failure_threshold: i64,
    /// How long a tripped circuit stays open once it trips.
    pub open_duration: Duration,
}

/// What one more failure decides, in policy-agnostic terms. A caller
/// always writes `consecutive_failures`; it writes `circuit_open_until`
/// only when this is the failure that tripped the breaker — the same
/// "only set it on the tick that trips the breaker" shape both
/// `record_endpoint_failure` and `record_provider_failure` already had
/// before this was pulled out from under them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureDecision {
    /// The value to write to `consecutiveFailures` — always set,
    /// regardless of whether this failure tripped the breaker.
    pub consecutive_failures: i64,
    /// The value to write to `circuitOpenUntil` — `Some` only on the
    /// failure that tripped the breaker; `None` otherwise, meaning
    /// "leave this column as it was."
    pub circuit_open_until: Option<DateTime<Utc>>,
}

impl FailureDecision {
    /// Whether this failure is the one that tripped the breaker — the
    /// condition both call sites use to decide whether to log a `warn!`
    /// and to set `circuit_open_until` on the write at all.
    #[must_use]
    pub fn opened_circuit(&self) -> bool {
        self.circuit_open_until.is_some()
    }
}

/// One more failure against a breaker currently at `consecutive_before`
/// consecutive failures. The counter always increments; the moment it
/// reaches `policy.failure_threshold` the breaker trips — the counter
/// resets to zero (never to the threshold value) and `circuit_open_until`
/// is stamped `policy.open_duration` past `now`. Never resets the counter
/// on any other tick.
#[must_use]
pub fn on_failure(
    policy: &BreakerPolicy,
    consecutive_before: i64,
    now: DateTime<Utc>,
) -> FailureDecision {
    let consecutive = consecutive_before + 1;
    let opening_circuit = consecutive >= policy.failure_threshold;
    FailureDecision {
        consecutive_failures: if opening_circuit { 0 } else { consecutive },
        circuit_open_until: opening_circuit.then(|| now + policy.open_duration),
    }
}

/// Whether a successful delivery/submit needs a reset write at all — both
/// call sites skip the write entirely when there is nothing to reset, so
/// a healthy endpoint/provider's every single success doesn't cost a
/// pointless `UPDATE`.
#[must_use]
pub fn needs_reset(consecutive_failures: i64, circuit_open_until: Option<DateTime<Utc>>) -> bool {
    consecutive_failures != 0 || circuit_open_until.is_some()
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, TimeZone, Utc};

    use super::{BreakerPolicy, needs_reset, on_failure};

    fn policy() -> BreakerPolicy {
        BreakerPolicy {
            failure_threshold: 3,
            open_duration: Duration::seconds(60),
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    #[test]
    fn a_failure_below_threshold_just_counts_and_never_trips() {
        let decision = on_failure(&policy(), 1, now());
        assert_eq!(decision.consecutive_failures, 2);
        assert_eq!(decision.circuit_open_until, None);
        assert!(!decision.opened_circuit());
    }

    /// The property `breaker.md` names as the one that must never
    /// regress: the failure that *reaches* the threshold resets the
    /// counter to zero, not to the threshold value, in the same tick that
    /// opens the circuit.
    #[test]
    fn the_failure_that_reaches_threshold_trips_and_resets_the_counter_to_zero() {
        let decision = on_failure(&policy(), 2, now());
        assert_eq!(decision.consecutive_failures, 0);
        assert_eq!(
            decision.circuit_open_until,
            Some(now() + Duration::seconds(60))
        );
        assert!(decision.opened_circuit());
    }

    /// Defensive, not a state this system should ever reach in practice
    /// (an already-open circuit's endpoint/provider is excluded from
    /// `claim.rs`'s own candidate set, so nothing should call `on_failure`
    /// against one) — but the decision must not panic or under-report if
    /// it ever is.
    #[test]
    fn a_failure_already_past_threshold_still_trips_and_resets() {
        let decision = on_failure(&policy(), 10, now());
        assert_eq!(decision.consecutive_failures, 0);
        assert!(decision.opened_circuit());
    }

    #[test]
    fn needs_reset_is_false_when_there_is_nothing_to_reset() {
        assert!(!needs_reset(0, None));
    }

    #[test]
    fn needs_reset_is_true_when_the_counter_is_nonzero() {
        assert!(needs_reset(1, None));
    }

    #[test]
    fn needs_reset_is_true_when_the_circuit_is_open() {
        assert!(needs_reset(0, Some(now())));
    }
}
