//! #72: opt-in consent, the classification that decides whether it's
//! required, and self-imposed marketing quiet hours.
//!
//! Three separate concerns share this module because `sendMessage` gates
//! all three on the identical question — [`MessageClass`] — and keeping
//! that single source of truth in one place is the whole point: the two
//! `match` functions below are exhaustive, so a fifth `MessageClass`
//! variant is a compile error here, not a silent "forgot to decide" gap.
//!
//! # What this system can actually prove about a message's classification,
//! and what it cannot
//!
//! Per `docs/architecture.md` §10: *"Opt-out honoured at send time... for
//! `marketing` and `notification`. OTP and transactional are exempt in
//! most regimes but keep the audit trail proving the classification."*
//! That sentence has a real trap in it, and the honest answer has two
//! halves that must never be conflated:
//!
//! - **What this system proves:** `Message.class` carries `@@audit`
//!   (`schema.cstack`), so every `sendMessage` call writes a framework
//!   audit row, in the same transaction as the `Message` insert, capturing
//!   the exact `class` value, the authenticated caller's claims
//!   (`client_id`/`kind`/`role`, from the JWT `GatewayAuth` already
//!   verified), and a server-stamped `occurred_at` — none of it
//!   caller-editable after the fact, because nothing in this codebase
//!   exposes a write path to the audit table at all. That is a real,
//!   contemporaneous, tamper-resistant-absent-DB-compromise record of
//!   *which class was declared, by whom, when*. It is what you hand a
//!   regulator when the classification is challenged.
//! - **What this system does not prove:** that the declared class was
//!   *true*. Nothing here inspects a message's `body` and independently
//!   decides "this reads like marketing copy, not an OTP." Content
//!   classification from free text is not machine-verifiable the way a
//!   state transition is — there is no oracle to check `class` against.
//!   A caller willing to mislabel marketing traffic as `transactional`
//!   bypasses every check in this module, and the audit trail records
//!   that they did so accurately, not that they did so honestly. Closing
//!   that gap needs either a manual compliance review process reading the
//!   audit trail (this system's actual answer today) or an independent
//!   content classifier (not built, and not a small addition — see
//!   `AGENTS.md`'s open-questions entry for #72).
//!
//! Put differently: this module makes misclassification *consequential*
//! (an honestly-labelled `marketing` message is genuinely blocked without
//! consent or outside quiet hours; a dishonestly-labelled `transactional`
//! one is not) and *attributable* (the audit trail says who labelled it
//! what). It does not make misclassification *detectable* on its own.
//!
//! # Enforcement scope
//!
//! [`requires_recipient_consent_controls`] governs *two* independent
//! checks in `Procedures::send` — opt-out honouring (`OptOut`, pre-existing)
//! and consent-on-file (`ConsentRecord`, new here) — because
//! `docs/architecture.md` §10 names the identical class pair for both:
//! *"for `marketing` and `notification`. OTP and transactional are
//! exempt."* Before this module existed, `sendMessage`'s opt-out check ran
//! unconditionally for every class, which was *more* restrictive than the
//! design doc ever asked for (an opted-out recipient could not receive an
//! OTP either) — accidentally safe, but not what was specified, and not
//! what this module now does.
//!
//! [`subject_to_quiet_hours`] is deliberately narrower — `marketing` only.
//! §10's quiet-hours bullet names only `marketing`; `notification` is not
//! mentioned there at all, so it is treated as *not* time-restricted,
//! distinct from being merely consent-exempt.
//!
//! # Quiet hours are a policy knob, not a statute
//!
//! *"No Cameroon-specific statutory rule was found; this is best
//! practice."* [`MARKETING_QUIET_HOURS_START_WAT`] and
//! [`MARKETING_QUIET_HOURS_END_WAT`] are named and documented as exactly
//! that — a self-imposed operational choice, not a legal requirement this
//! codebase is obligated to encode. They are `pub const` rather than
//! buried inline specifically so the next reader sees them as a value to
//! reconsider, not a rule to trust. Not wired to an environment variable
//! or a `Settings` model: no such model exists yet (the admin console's
//! own feature list names a future "Settings" screen this could live
//! behind), and this repo's own standing preference is a hard-coded,
//! visible decision over a half-built configurability seam nobody asked
//! for. Revisit if an operator genuinely needs a different window.
//!
//! # Where this is enforced, and why
//!
//! At `sendMessage` (accept time), not at `dispatch` (submit time). Two
//! reasons, not one: `send`'s own doc comment already frames this
//! procedure's job as "the decision, not delivery" — opt-out and quota are
//! both decided here, and quiet hours/consent are the same shape of
//! decision. And a "hold and release" design at `dispatch` would leave a
//! message accepted at 19:59 sitting silently in `queued` for up to
//! twelve hours with no caller-visible signal beyond an eventual DLR or
//! timeout — accept-time rejection instead gives the caller a synchronous,
//! immediate, explainable answer ("come back after 08:00 WAT") they can
//! act on. The tradeoff: a marketing campaign that wants to *schedule* a
//! send for the next quiet-hours window has no mechanism here — `dispatch`
//! never sees a message this module rejected. `SendMessageInput` already
//! carries `scheduledAt`, so a caller can retry there itself; a
//! server-side "hold until quiet hours end" is a real, reasonable
//! alternative design this PR does not build.

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
