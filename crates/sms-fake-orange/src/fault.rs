//! What the fake decides to do with one submit call: how to answer the HTTP
//! request, and whether/when/what to POST back as a DLR. Two policies pick
//! that decision — see [`FaultPolicy`]'s own doc for the CI-determinism
//! reasoning behind having exactly two, not a spectrum.
//!
//! Connection-level nastiness (RST mid-response, refused connections,
//! byte-dribble) is deliberately not modelled here — out of scope for this
//! PR, noted as future work rather than half-done. Everything below answers
//! with a real, well-formed (if sometimes broken-on-purpose) HTTP response;
//! [`sms_provider_orange_cm`]'s own unit tests already cover the
//! connect-vs-post-connect transport-error distinction directly against
//! real refused/slow sockets, so this crate doesn't need to re-prove that
//! part.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// How the fake answers one submit HTTP call. Deliberately mirrors the
/// shapes `OrangeCmProvider::submit`/`classify_submit_error` already
/// distinguish (§6.1/§6.2) — this crate fakes the wire, not a new taxonomy.
///
/// What's absent on purpose: a "connect refused" / "timeout before any
/// response" variant. Modelling those needs to sever the connection or hold
/// it open with the response never sent — `wiremock`'s `Respond` trait can
/// only ever emit a well-formed HTTP response, delayed or not, so a
/// dedicated response-never-comes fault would need connection-level control
/// this crate explicitly doesn't take on (see the module doc). A `201`
/// response delayed past the caller's own `request_timeout`
/// ([`SubmitDecision::response_delay`]) already exercises the *safe half*
/// of that same distinction from `dispatch`'s point of view — an
/// `Indeterminate`, not-safe-to-retry outcome — which is the one this
/// design doc calls out for "real attention". The *other* half
/// (connect-phase failure, safe to retry) is already covered, directly,
/// against a real refused socket, by `sms-provider-orange-cm`'s own
/// `a_connect_refusal_is_still_unavailable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// `201` with a well-formed body and a fresh `resourceURL`. Whether this
    /// reads as "accepted" or "accepted, then timed out" to the caller is
    /// entirely a function of [`SubmitDecision::response_delay`] versus the
    /// caller's own configured `request_timeout` — Orange genuinely
    /// accepted the message either way, which is the whole point of
    /// `Indeterminate` existing as a distinct outcome from `Unavailable`.
    Accepted,
    /// `201` but the body isn't valid JSON.
    AcceptedMalformedBody,
    /// `201`, valid JSON, but `resourceURL` is empty — no id to extract.
    AcceptedMissingResourceUrl,
    /// `429` — the 5 TPS ceiling being hit.
    RateLimited,
    /// `503` — Orange's own backend degraded.
    ServerError,
    /// `400` — the request itself is invalid (unapproved sender, bad
    /// destination, ...).
    Rejected,
}

/// Orange's own DLR status vocabulary (`deliveryInfo[].deliveryStatus`),
/// exactly what `sms-provider-orange-cm`'s real `dlr::parse` reads — sending
/// these strings, not `sms_provider::DeliveryOutcome` directly, is what
/// makes a chaos run exercise the real parsing code, not a shortcut around
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlrStatus {
    /// `"DeliveredToTerminal"` → `Delivered`.
    Delivered,
    /// `"DeliveryUncertain"` → `Uncertain`.
    Uncertain,
    /// `"DeliveryImpossible"` → `Failed`.
    Failed,
    /// Any other literal string — deliberately unrecognised by
    /// `sms-provider-orange-cm`'s own `outcome_of`, so this always resolves
    /// to `DeliveryOutcome::Unknown` and proposes no transition at all. For
    /// proving a genuinely unclassifiable status doesn't get guessed into a
    /// transition, not for spelling a real Orange status this crate doesn't
    /// know about.
    Unrecognised(String),
}

impl DlrStatus {
    pub(crate) fn wire(&self) -> &str {
        match self {
            Self::Delivered => "DeliveredToTerminal",
            Self::Uncertain => "DeliveryUncertain",
            Self::Failed => "DeliveryImpossible",
            Self::Unrecognised(s) => s,
        }
    }
}

/// One scheduled DLR POST. `delay` is measured from the moment the fake
/// *received* the submit request, not from when it answered it — this is
/// what makes [`SubmitDecision::response_delay`] > a step's `delay` produce
/// the "DLR races the submit response" scenario the design brief calls out
/// for real attention: the DLR can land, and be dropped for lack of a
/// `providerMessageRef`/`providerMessageRefAlt` to correlate against, before
/// `dispatch` has even finished writing `routed -> submitted`.
#[derive(Debug, Clone)]
pub struct DlrStep {
    /// How long after the submit request was *received* to fire this DLR.
    pub delay: Duration,
    /// The outcome this DLR reports.
    pub status: DlrStatus,
    /// `None` correlates against the real `callbackData` the submit request
    /// carried (`Message.id`). `Some(ref)` sends an unrelated reference
    /// instead — the "DLR for an unknown ref" fault mode.
    pub reference_override: Option<String>,
}

impl DlrStep {
    /// A DLR reporting `status`, `delay` after the submit request arrived,
    /// correlated against whatever reference that request actually carried.
    #[must_use]
    pub fn after(delay: Duration, status: DlrStatus) -> Self {
        Self {
            delay,
            status,
            reference_override: None,
        }
    }

    /// A DLR reporting `status` against `fake_ref` instead of the real
    /// submission's own reference — models a DLR for a reference this
    /// deployment never issued.
    #[must_use]
    pub fn for_unknown_ref(
        delay: Duration,
        status: DlrStatus,
        fake_ref: impl Into<String>,
    ) -> Self {
        Self {
            delay,
            status,
            reference_override: Some(fake_ref.into()),
        }
    }
}

/// What the fake does with one submit call, end to end: how to answer the
/// HTTP request, and which DLRs (zero or more, any delay, any order,
/// possibly referencing something else entirely) to fire afterward.
#[derive(Debug, Clone)]
pub struct SubmitDecision {
    /// How to answer the submit HTTP request.
    pub outcome: SubmitOutcome,
    /// Delay before the submit HTTP response is sent, measured from receipt
    /// of the request. `Duration::ZERO` for an instant reply.
    pub response_delay: Duration,
    /// DLRs to schedule as a side effect of this submit call, in the order
    /// given (not necessarily the order they arrive — each is independently
    /// delayed).
    pub dlr_plan: Vec<DlrStep>,
}

impl SubmitDecision {
    /// `201 Accepted`, no DLR ever — models "the SMS never arrives" without
    /// modelling a submit-side failure at all.
    #[must_use]
    pub fn accepted() -> Self {
        Self {
            outcome: SubmitOutcome::Accepted,
            response_delay: Duration::ZERO,
            dlr_plan: Vec::new(),
        }
    }

    /// `201 Accepted`, with `dlr_plan` scheduled alongside it.
    #[must_use]
    pub fn accepted_with_dlrs(dlr_plan: Vec<DlrStep>) -> Self {
        Self {
            outcome: SubmitOutcome::Accepted,
            response_delay: Duration::ZERO,
            dlr_plan,
        }
    }

    /// Overrides [`Self::response_delay`] (builder-style) — the knob that
    /// turns a plain `Accepted` into "accepted, then the response is late or
    /// never comes within the caller's own timeout".
    #[must_use]
    pub fn response_delay(mut self, delay: Duration) -> Self {
        self.response_delay = delay;
        self
    }

    /// `429 Too Many Requests` — no body, no DLR.
    #[must_use]
    pub fn rate_limited() -> Self {
        Self {
            outcome: SubmitOutcome::RateLimited,
            response_delay: Duration::ZERO,
            dlr_plan: Vec::new(),
        }
    }

    /// `503 Service Unavailable` — no body, no DLR.
    #[must_use]
    pub fn server_error() -> Self {
        Self {
            outcome: SubmitOutcome::ServerError,
            response_delay: Duration::ZERO,
            dlr_plan: Vec::new(),
        }
    }

    /// `400 Bad Request` — no DLR; the request itself was refused.
    #[must_use]
    pub fn rejected() -> Self {
        Self {
            outcome: SubmitOutcome::Rejected,
            response_delay: Duration::ZERO,
            dlr_plan: Vec::new(),
        }
    }

    /// `201` with a body that isn't valid JSON.
    #[must_use]
    pub fn malformed_body() -> Self {
        Self {
            outcome: SubmitOutcome::AcceptedMalformedBody,
            response_delay: Duration::ZERO,
            dlr_plan: Vec::new(),
        }
    }

    /// `201` with valid JSON but an empty `resourceURL`.
    #[must_use]
    pub fn missing_resource_url() -> Self {
        Self {
            outcome: SubmitOutcome::AcceptedMissingResourceUrl,
            response_delay: Duration::ZERO,
            dlr_plan: Vec::new(),
        }
    }
}

/// Three policies. The first two, deliberately, are not a spectrum of
/// knobs:
///
/// - [`Self::Scripted`] — an exact, ordered queue. What the CI gate's
///   deterministic tests use: assert one specific outcome, not a
///   distribution.
/// - [`Self::Seeded`] — a seeded PRNG picks a [`SubmitDecision`] per submit
///   call, weighted toward the realistic mix (§6.1's own framing: most
///   submissions succeed; failures are the tail). Reproducible by
///   construction: the same seed, replayed against the same sequence of
///   calls, always draws the same decisions — a failing seed is always
///   replayable by naming it, never by re-running and hoping.
///
/// Never unseeded randomness anywhere in this crate — that would manufacture
/// exactly the CI flake this workspace has already spent real effort
/// removing (see `crates/sms-worker/tests/dispatch_live_postgres.rs`'s own
/// `TEST_MUTEX`/`clear_claimable_backlog` history).
///
/// The third, [`Self::Always`], answers a different need entirely: neither
/// a CI test's exact expectations nor a fuzz sweep's realistic mix, but a
/// long-lived demo/dev process (`app/sms-fake-orange`) that must never run
/// out of scripted decisions the way [`Self::Scripted`] does (see
/// [`FaultPolicy::next`]'s own doc on that fallback) — it repeats one fixed
/// decision forever, by construction, rather than approximating "forever"
/// with a very long scripted queue.
pub enum FaultPolicy {
    /// An exact, ordered queue of decisions, one per submit call.
    Scripted(Mutex<VecDeque<SubmitDecision>>),
    /// A seeded PRNG that draws a weighted decision per submit call. Boxed
    /// — `StdRng`'s own state is large enough that an unboxed variant would
    /// make every `FaultPolicy` pay for it, `Scripted` included.
    Seeded(Box<Mutex<StdRng>>),
    /// The same [`SubmitDecision`] repeated indefinitely, never exhausted.
    Always(SubmitDecision),
}

impl FaultPolicy {
    /// An exact, ordered sequence of decisions — one per expected submit
    /// call, in order.
    #[must_use]
    pub fn scripted(decisions: impl IntoIterator<Item = SubmitDecision>) -> Self {
        Self::Scripted(Mutex::new(decisions.into_iter().collect()))
    }

    /// A seeded PRNG policy. Replaying the same `seed` against the same
    /// sequence of submit calls always draws the same decisions.
    #[must_use]
    pub fn seeded(seed: u64) -> Self {
        Self::Seeded(Box::new(Mutex::new(StdRng::seed_from_u64(seed))))
    }

    /// Repeats `decision` forever — every submit call gets an identical
    /// clone of it. The right policy for a long-lived server: pass
    /// [`SubmitDecision::accepted_with_dlrs`] with a single
    /// [`DlrStep::after`] `DlrStatus::Delivered` for the happy-path demo
    /// default.
    #[must_use]
    pub fn always(decision: SubmitDecision) -> Self {
        Self::Always(decision)
    }

    /// The next decision. A `Scripted` policy that runs out of entries falls
    /// back to a plain accept with no DLR — a test that scripts exactly as
    /// many entries as it expects submit calls never observes this; it
    /// exists so an unexpected *extra* call (a retry the test didn't
    /// anticipate) degrades to something harmless and self-resolving rather
    /// than panicking the whole suite. `Always` never hits this fallback —
    /// it has no end to run out of.
    pub(crate) fn next(&self) -> SubmitDecision {
        match self {
            Self::Scripted(queue) => queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or_else(SubmitDecision::accepted),
            Self::Seeded(rng) => seeded_decision(
                &mut rng
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            ),
            Self::Always(decision) => decision.clone(),
        }
    }
}

/// Request-timeout margin every chaos test configures its `OrangeCmProvider`
/// with (see `chaos_live_postgres.rs`'s own `CHAOS_REQUEST_TIMEOUT`). Fault
/// delays below are chosen relative to it: comfortably under for a normal
/// reply, comfortably over for a deliberate timeout, and the DLR that
/// resolves a timed-out submission is delayed past *that*, so the
/// `routed -> uncertain` write has time to land before the DLR arrives —
/// otherwise the DLR races a write that hasn't happened yet for a second,
/// uninteresting reason (test timing, not the real race this suite exists
/// to prove).
const NORMAL_DLR_DELAY_MIN_MS: u64 = 10;
const NORMAL_DLR_DELAY_MAX_MS: u64 = 90;
const TIMEOUT_RESPONSE_DELAY_MS: u64 = 300;
const RESOLVING_DLR_DELAY_MS: u64 = 450;
const RACE_RESPONSE_DELAY_MS: u64 = 80;

fn random_delay(rng: &mut StdRng) -> Duration {
    Duration::from_millis(rng.gen_range(NORMAL_DLR_DELAY_MIN_MS..=NORMAL_DLR_DELAY_MAX_MS))
}

/// Weighted, reproducible draw for [`FaultPolicy::Seeded`]. Weights are
/// deliberately realistic-skewed (§6.1: most gateway failover bugs hide in
/// the tail, so the tail needs real coverage, but the mix should still look
/// like production traffic, not an adversarial worst case every call).
fn seeded_decision(rng: &mut StdRng) -> SubmitDecision {
    let roll: f64 = rng.gen_range(0.0..1.0);
    if roll < 0.55 {
        accepted_with_random_dlr(rng)
    } else if roll < 0.66 {
        SubmitDecision::rate_limited()
    } else if roll < 0.76 {
        SubmitDecision::server_error()
    } else if roll < 0.84 {
        SubmitDecision::rejected()
    } else if roll < 0.92 {
        timeout_decision(rng)
    } else if roll < 0.96 {
        SubmitDecision::malformed_body()
    } else {
        SubmitDecision::missing_resource_url()
    }
}

/// The `Accepted` branch's own sub-distribution: no DLR ever, a normal
/// single delivery, a normal single failure, a duplicate `delivered`, an
/// out-of-order pair (a retryable failure DLR then a `delivered` DLR that
/// the transition table must refuse — `undelivered -> delivered` isn't a
/// legal edge), or a DLR that races the submit response.
fn accepted_with_random_dlr(rng: &mut StdRng) -> SubmitDecision {
    let roll: f64 = rng.gen_range(0.0..1.0);
    if roll < 0.15 {
        SubmitDecision::accepted()
    } else if roll < 0.65 {
        SubmitDecision::accepted_with_dlrs(vec![DlrStep::after(
            random_delay(rng),
            DlrStatus::Delivered,
        )])
    } else if roll < 0.78 {
        SubmitDecision::accepted_with_dlrs(vec![DlrStep::after(
            random_delay(rng),
            DlrStatus::Failed,
        )])
    } else if roll < 0.87 {
        SubmitDecision::accepted_with_dlrs(vec![
            DlrStep::after(random_delay(rng), DlrStatus::Delivered),
            DlrStep::after(
                random_delay(rng) + Duration::from_millis(60),
                DlrStatus::Delivered,
            ),
        ])
    } else if roll < 0.94 {
        SubmitDecision::accepted_with_dlrs(vec![
            DlrStep::after(random_delay(rng), DlrStatus::Failed),
            DlrStep::after(
                random_delay(rng) + Duration::from_millis(60),
                DlrStatus::Delivered,
            ),
        ])
    } else {
        SubmitDecision::accepted_with_dlrs(vec![DlrStep::after(
            Duration::ZERO,
            DlrStatus::Delivered,
        )])
        .response_delay(Duration::from_millis(RACE_RESPONSE_DELAY_MS))
    }
}

/// A submit that Orange accepts but never answers within the caller's
/// timeout — `Indeterminate` from `dispatch`'s point of view. About half the
/// time, a DLR eventually resolves it (the "closes the loop" scenario);
/// otherwise it's left to `expire_stale`'s 6h grace, which the chaos test
/// forces forward rather than waiting out for real.
fn timeout_decision(rng: &mut StdRng) -> SubmitDecision {
    let dlr_plan = if rng.gen_bool(0.5) {
        vec![DlrStep::after(
            Duration::from_millis(RESOLVING_DLR_DELAY_MS),
            DlrStatus::Delivered,
        )]
    } else {
        Vec::new()
    };
    SubmitDecision {
        outcome: SubmitOutcome::Accepted,
        response_delay: Duration::from_millis(TIMEOUT_RESPONSE_DELAY_MS),
        dlr_plan,
    }
}
