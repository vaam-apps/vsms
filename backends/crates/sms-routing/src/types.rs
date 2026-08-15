#![doc = include_str!("types.md")]

use std::collections::HashSet;

/// Mirrors `schema.cstack`'s `OperatorCode` enum, verbatim variant names.
/// A local copy, not a re-export of the schema type, because this crate is
/// pure (§6.3: no cratestack, no `include_server_schema!` expansion) — the
/// same reason [`sms_provider::Capabilities`] doesn't reuse `ProviderState`
/// either. The caller converts at the boundary
/// (`backends/crates/sms-worker/src/routing.rs`), the same pattern `dispatch.rs`'s
/// own `decode_encoding` already uses for `Encoding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operator {
    /// MTN Cameroon.
    Mtn,
    /// Orange Cameroon.
    Orange,
    /// Cameroon Telecommunications (Camtel).
    Camtel,
    /// Nexttel (Viettel Cameroon).
    Nexttel,
    /// Not classified — `sms_msisdn::OperatorPrefixTable` found no match,
    /// or the value hasn't been classified at all.
    Unknown,
}

/// Mirrors `schema.cstack`'s `MessageClass` enum, verbatim variant names.
/// See [`Operator`]'s doc for why this is a local copy, not a re-export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageClass {
    /// A one-time password / verification code.
    Otp,
    /// A transactional message (receipts, confirmations).
    Transactional,
    /// A service notification.
    Notification,
    /// A marketing message.
    Marketing,
}

/// One `Route` row, exactly the columns [`select_route`](crate::select_route)
/// reads. `priority`/`weight` are `i64`, matching this workspace's own
/// "`Int` is `i64`" convention (`AGENTS.md`'s verified toolchain API
/// section) — `schema.cstack` declares both as `Int @range(min: 0, max:
/// 1000)`.
///
/// `None` on any `match_*` field means "matches anything" — a wildcard, not
/// "matches only a `NULL` value". This is a deliberate reading of §6.3's
/// "filter `Route` rows where every non-null `match*` field matches": a
/// route with no `matchOperator` set routes every operator, not none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRow {
    /// `Route.id`.
    pub id: String,
    /// `Route.name` — carried through purely for the explanation trail
    /// (a human-readable label in [`RouteEvaluation`]), never matched on.
    pub name: String,
    /// `Route.priority`. The *only* cross-band ordering — see
    /// [`crate::select_route`]'s own doc.
    pub priority: i64,
    /// `Route.weight`. Only consulted among routes that already share the
    /// winning [`priority`](Self::priority) — see the weighted-draw
    /// section of [`crate::select_route`]'s doc.
    pub weight: i64,
    /// `Route.enabled` — `false` excludes this route outright, before any
    /// predicate is even checked.
    pub enabled: bool,
    /// `Route.matchOperator`. `None` matches every [`Operator`].
    pub match_operator: Option<Operator>,
    /// `Route.matchClass`. `None` matches every [`MessageClass`].
    pub match_class: Option<MessageClass>,
    /// `Route.matchAppId`. `None` matches every app.
    pub match_app_id: Option<String>,
    /// A national-number prefix (e.g. `"677"`), the same convention
    /// `OperatorPrefixRule.prefix` and `sms_msisdn::OperatorPrefixTable`
    /// already use — matched against
    /// [`RoutingCandidate::msisdn_national`] via `starts_with`, not a
    /// hand-rolled E.164 parse (the whole point of reusing
    /// `sms_msisdn::Msisdn::national()` at the call site rather than
    /// re-deriving the national form here). `None` matches every number.
    pub match_prefix: Option<String>,
    /// `Route.providerId` — looked up in the `providers` map
    /// [`crate::select_route`] is given.
    pub provider_id: String,
    /// `Route.failoverRouteId` — #63's own concern (out of scope here).
    /// Carried through so a caller can walk the failover chain without a
    /// second lookup. Never read by [`crate::select_route`] itself.
    pub failover_route_id: Option<String>,
}

/// Whatever the caller already knows about a `Provider` row's live
/// availability, reduced to the one bit routing needs plus a human reason
/// for the explanation trail. Computed by the caller (`ProviderState` +
/// `healthy`, the same two columns `cheapest_active_provider` used to
/// read) — this crate never interprets provider state itself, the same
/// "ask a bool, not an identity" discipline
/// [`sms_provider::Capabilities`] documents for capability checks.
///
/// Capability fit (UCS-2 support, alphanumeric sender) and remaining
/// TPS/daily budget — §6.3 names both as further post-ranking filters — are
/// deliberately not modelled here: no second provider with materially
/// different capabilities exists yet to make either concrete (#61/#63).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRow {
    /// `Provider.id` — matched against [`RouteRow::provider_id`].
    pub id: String,
    /// `true` iff this provider is usable right now (roughly: `state ==
    /// active`, `healthy`). When `false`, [`reason`](Self::reason) explains
    /// why, for the explanation trail.
    pub available: bool,
    /// Human-readable reason, used only when [`available`](Self::available)
    /// is `false` — surfaced verbatim in
    /// [`RouteOutcome::ProviderUnavailable`].
    pub reason: Option<String>,
}

/// What's known about the message being routed — the four columns §6.3
/// and this ticket name explicitly (operator, class, app, prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutingCandidate<'a> {
    /// Compared against [`RouteRow::match_operator`].
    pub operator: Operator,
    /// Compared against [`RouteRow::match_class`].
    pub class: MessageClass,
    /// Compared against [`RouteRow::match_app_id`].
    pub app_id: &'a str,
    /// National-number digits (`sms_msisdn::Msisdn::national()`), e.g.
    /// `"677123456"` for `+237677123456` — never the E.164 form, matching
    /// [`RouteRow::match_prefix`]'s own doc.
    pub msisdn_national: &'a str,
}

/// Why one route's `match*` predicates didn't all pass. Exactly one
/// variant fires per route — the first predicate checked that fails; a
/// route failing on operator *and* class only reports the operator
/// failure, since fixing it wouldn't make the route eligible anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateFailure {
    /// `RouteRow::match_operator` was `Some` and disagreed with
    /// [`RoutingCandidate::operator`].
    Operator {
        /// What the route required.
        expected: Operator,
        /// What the candidate actually was.
        actual: Operator,
    },
    /// `RouteRow::match_class` was `Some` and disagreed with
    /// [`RoutingCandidate::class`].
    Class {
        /// What the route required.
        expected: MessageClass,
        /// What the candidate actually was.
        actual: MessageClass,
    },
    /// `RouteRow::match_app_id` was `Some` and disagreed with
    /// [`RoutingCandidate::app_id`].
    AppId {
        /// What the route required.
        expected: String,
        /// What the candidate actually was.
        actual: String,
    },
    /// `RouteRow::match_prefix` was `Some` and the candidate's national
    /// number didn't start with it.
    Prefix {
        /// The prefix the route required.
        expected: String,
        /// The candidate's actual national-number digits.
        msisdn_national: String,
    },
}

/// Why one input route did or didn't take part in the final selection.
/// Every route [`select_route`](crate::select_route) is given produces
/// exactly one [`RouteEvaluation`] with exactly one of these — this is the
/// "which routes were considered, which predicates each failed on" half of
/// #62's explainability requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteOutcome {
    /// Named in the caller's `exclude` set — #63's own mechanism for "give
    /// me the next route after this one failed": pass the failed route's
    /// id back in on the retry and it's skipped here rather than won
    /// again.
    Excluded,
    /// `Route.enabled == false`.
    Disabled,
    /// A `match_*` predicate didn't pass. See [`PredicateFailure`].
    PredicateFailed(PredicateFailure),
    /// The route's own `provider_id` wasn't found in the `providers` map
    /// at all, or was found with `available: false`. The `String` is
    /// [`ProviderRow::reason`] when present, else a generic explanation.
    ProviderUnavailable(String),
    /// Survived every filter above.
    Eligible {
        /// `true` only for the route that actually entered the draw (the
        /// highest-priority band with at least one eligible member) — a
        /// lower-priority band's members are marked eligible too (they
        /// would win if the top band were empty) but never drawn against.
        winning_band: bool,
    },
}

/// One input route's full evaluation record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEvaluation {
    /// `Route.id`.
    pub route_id: String,
    /// `Route.name`, for a human-readable explanation trail.
    pub route_name: String,
    /// `Route.priority`, echoed from the input row.
    pub priority: i64,
    /// `Route.weight`, echoed from the input row.
    pub weight: i64,
    /// `Route.providerId`, echoed from the input row.
    pub provider_id: String,
    /// Why this route did or didn't win.
    pub outcome: RouteOutcome,
}

/// One member of the winning priority band, as it entered the weighted
/// draw.
#[derive(Debug, Clone, PartialEq)]
pub struct TieBreakRange {
    /// `Route.id` of this band member.
    pub route_id: String,
    /// `Route.weight` of this band member, echoed for display.
    pub weight: i64,
    /// The low end of this member's `[low, high)` share of `[0.0, 1.0)`.
    /// [`TieBreak::draw`] landing in `[low, high)` is what made this route
    /// the winner. `low`/`high` are `f64`s, not `Eq`-friendly in general,
    /// but every value here is derived from integer weights via exact,
    /// reproducible arithmetic given the same input order — see
    /// [`crate::select_route`]'s own doc on why `PartialEq` (not `Eq`) is
    /// what this type actually needs.
    pub low: f64,
    /// The high end of this member's `[low, high)` share of `[0.0, 1.0)`.
    pub high: f64,
}

/// How a tie between same-priority routes was broken. Present iff the
/// winning priority band had more than one eligible member — a band with
/// exactly one member has nothing to draw between, so no [`TieBreak`] is
/// produced for it (see [`RouteOutcome::Eligible`]'s doc).
#[derive(Debug, Clone, PartialEq)]
pub struct TieBreak {
    /// The priority value of the winning band.
    pub priority: i64,
    /// The caller-supplied draw this decision was made with — see
    /// [`crate::select_route`]'s own doc for where this comes from in
    /// production vs. a replay.
    pub draw: f64,
    /// One range per band member, in the order they were evaluated
    /// (`routes` input order, restricted to this band) — the caller
    /// supplying `routes` in a stable order (e.g. `id` ascending) is what
    /// makes a replay with the same `draw` reproduce the same winner.
    pub ranges: Vec<TieBreakRange>,
    /// `Route.id` of the route [`draw`](Self::draw) landed on.
    pub winner_route_id: String,
}

/// The final pick, if any route was eligible at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Winner {
    /// `Route.id` of the winning route.
    pub route_id: String,
    /// `Route.providerId` of the winning route — what a caller actually
    /// submits through.
    pub provider_id: String,
    /// `Route.failoverRouteId` of the winning route, for #63's own use.
    pub failover_route_id: Option<String>,
}

/// The full, replayable answer to "given this candidate and this route
/// set, which route wins and why" — #62's own two words. `evaluations`
/// covers every input route unconditionally (including excluded/disabled/
/// ineligible ones) so a caller never has to re-derive "why wasn't route X
/// picked" from anything but this struct.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// One entry per input route, in input order.
    pub evaluations: Vec<RouteEvaluation>,
    /// How a tie in the winning priority band was broken, if the band had
    /// more than one eligible member.
    pub tie_break: Option<TieBreak>,
    /// The final pick, or `None` if nothing was eligible.
    pub winner: Option<Winner>,
}

/// A route id set to skip — #63's own "give me the next route after this
/// one failed" mechanism. Empty for a first pick.
pub type ExcludedRouteIds = HashSet<String>;
