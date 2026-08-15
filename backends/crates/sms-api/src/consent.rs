#![doc = include_str!("consent.md")]

use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};

use crate::schema::MessageClass;

/// Marketing quiet hours start, in West Africa Time (UTC+1, no DST) — see
/// this module's own doc for why this is a policy knob, not a statute.
pub const MARKETING_QUIET_HOURS_START_WAT: u32 = 8;

/// Marketing quiet hours end (exclusive), in West Africa Time.
pub const MARKETING_QUIET_HOURS_END_WAT: u32 = 20;

/// Cameroon observes WAT (UTC+1) year-round — no DST transitions to track.
const WAT_OFFSET_HOURS: i64 = 1;

/// Classes that require both an honoured `OptOut` and a matching
/// `ConsentRecord` before `sendMessage` will persist a row. Exhaustive on
/// purpose (see this module's own doc) — a fifth `MessageClass` variant
/// fails to compile here rather than silently defaulting either way.
#[must_use]
pub const fn requires_recipient_consent_controls(class: MessageClass) -> bool {
    match class {
        MessageClass::otp | MessageClass::transactional => false,
        MessageClass::notification | MessageClass::marketing => true,
    }
}

/// Classes subject to the self-imposed marketing quiet-hours window.
/// Deliberately narrower than [`requires_recipient_consent_controls`] —
/// `notification` needs consent but is not time-restricted; see this
/// module's own "Enforcement scope" doc.
#[must_use]
pub const fn subject_to_quiet_hours(class: MessageClass) -> bool {
    match class {
        MessageClass::marketing => true,
        MessageClass::otp | MessageClass::transactional | MessageClass::notification => false,
    }
}

/// Is `now_utc` inside the `[08:00, 20:00)` WAT marketing send window?
///
/// Pure and clock-injected on purpose — the one thing that makes this
/// testable at every boundary without waiting for the wall clock to reach
/// them (`backends/crates/sms-api/tests/consent.rs` asserts 07:59/08:00/19:59/20:00
/// WAT explicitly) and makes `Procedures::send`'s own live test able to
/// assert against `is_within_marketing_quiet_hours(Utc::now())` and be
/// correct no matter what time it actually runs.
#[must_use]
pub fn is_within_marketing_quiet_hours(now_utc: DateTime<Utc>) -> bool {
    let wat_hour = (now_utc + ChronoDuration::hours(WAT_OFFSET_HOURS)).hour();
    (MARKETING_QUIET_HOURS_START_WAT..MARKETING_QUIET_HOURS_END_WAT).contains(&wat_hour)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn utc_at(hour: u32, minute: u32) -> DateTime<Utc> {
        // WAT is UTC+1, so 08:00 WAT is 07:00 UTC.
        Utc.with_ymd_and_hms(2026, 8, 12, hour, minute, 0).unwrap()
    }

    #[test]
    fn requires_consent_controls_matches_docs_architecture_10() {
        assert!(!requires_recipient_consent_controls(MessageClass::otp));
        assert!(!requires_recipient_consent_controls(
            MessageClass::transactional
        ));
        assert!(requires_recipient_consent_controls(
            MessageClass::notification
        ));
        assert!(requires_recipient_consent_controls(MessageClass::marketing));
    }

    #[test]
    fn only_marketing_is_subject_to_quiet_hours() {
        assert!(!subject_to_quiet_hours(MessageClass::otp));
        assert!(!subject_to_quiet_hours(MessageClass::transactional));
        assert!(!subject_to_quiet_hours(MessageClass::notification));
        assert!(subject_to_quiet_hours(MessageClass::marketing));
    }

    #[test]
    fn just_before_08_00_wat_is_still_quiet_hours() {
        // 07:59 WAT == 06:59 UTC.
        assert!(!is_within_marketing_quiet_hours(utc_at(6, 59)));
    }

    #[test]
    fn exactly_08_00_wat_opens_the_window() {
        // 08:00 WAT == 07:00 UTC — inclusive start.
        assert!(is_within_marketing_quiet_hours(utc_at(7, 0)));
    }

    #[test]
    fn just_before_20_00_wat_is_still_open() {
        // 19:59 WAT == 18:59 UTC.
        assert!(is_within_marketing_quiet_hours(utc_at(18, 59)));
    }

    #[test]
    fn exactly_20_00_wat_closes_the_window() {
        // 20:00 WAT == 19:00 UTC — exclusive end.
        assert!(!is_within_marketing_quiet_hours(utc_at(19, 0)));
    }

    #[test]
    fn the_middle_of_the_night_is_quiet_hours() {
        // 02:00 WAT == 01:00 UTC.
        assert!(!is_within_marketing_quiet_hours(utc_at(1, 0)));
    }

    #[test]
    fn midday_is_never_quiet_hours() {
        // 13:00 WAT == 12:00 UTC.
        assert!(is_within_marketing_quiet_hours(utc_at(12, 0)));
    }
}
