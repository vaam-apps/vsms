#![doc = include_str!("grey_route_watch.md")]

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sms_api::schema::{
    Cratestack, Job, MessageClass, MessageState, OperatorCode, message, route, route_validation,
};
use tracing::warn;

use crate::jobs::JobHandler;

/// #64's issue text, restated as a number: below this many terminal
/// messages, a route's own delivery rate is not trusted enough to serve as
/// either side of a comparison. See the module doc's "sample size matters
/// more than the delta" section for why this exists alongside, not instead
/// of, the z-test.
pub const MIN_SAMPLE: u64 = 30;

/// Practical-significance floor, in raw rate — 15 percentage points. Stops
/// a statistically real but operationally meaningless gap at huge volume
/// from flagging. See the module doc.
pub const MIN_DELTA: f64 = 0.15;

/// Two-proportion z-test threshold. `3.0` is a deliberately conservative
/// bar (roughly `p < 0.0027`, two-tailed) — this is an unattended alert,
/// not a dashboard a human is already looking at, so the cost of a false
/// positive (an alert operators learn to ignore) is judged higher here than
/// the cost of a slightly slower true positive.
pub const Z_THRESHOLD: f64 = 3.0;

/// How far back the divergence check looks for terminal messages. A
/// rolling window, recomputed fresh every run — not a "since last run"
/// delta — so a route's very recent history always dominates its own
/// comparison, and a fixed past incident ages out on its own.
pub const LOOKBACK: Duration = Duration::days(7);

/// Bounds one run's own `Message` fetch for the divergence half — see the
/// module doc's own "No GROUP BY" section.
const FETCH_LIMIT: i64 = 20_000;

/// §6.4's own cadence, verbatim: "re-validate monthly." A route with no
/// `RouteValidation` row inside this window is overdue.
pub const VALIDATION_INTERVAL: Duration = Duration::days(30);

/// One route's aggregated terminal outcomes within the window, for one
/// `(operator, class)` peer group. `delivered`/`failed` are counts, not
/// rates, so [`detect_divergent_routes`] can re-derive both a rate and a
/// sample size from the same two numbers — never store a rate without the
/// `n` it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteOutcome {
    /// `Message.operator`, the peer-group key's first half.
    pub operator: OperatorCode,
    /// `Message.class`, the peer-group key's second half.
    pub class: MessageClass,
    /// `Message.routeId` — the route this outcome is attributed to.
    pub route_id: String,
    /// Terminal messages that reached `delivered`.
    pub delivered: u64,
    /// Terminal messages that reached `undelivered`/`failed`/`expired`.
    pub failed: u64,
}

impl RouteOutcome {
    /// `delivered + failed` — the sample size a comparison actually trusts.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.delivered + self.failed
    }

    /// Delivered fraction of `total()`. Callers must check `total() > 0`
    /// first — a zero-sample route never reaches [`detect_divergent_routes`]'s
    /// own comparison logic (the `MIN_SAMPLE` gate excludes it), and this
    /// crate never calls `rate()` on one that hasn't passed that gate.
    #[must_use]
    pub fn rate(&self) -> f64 {
        // Guarded by every real caller's own MIN_SAMPLE check; NaN here
        // would only ever occur on a route with zero terminal messages,
        // which detect_divergent_routes never compares.
        #[allow(clippy::cast_precision_loss)]
        {
            self.delivered as f64 / self.total() as f64
        }
    }
}

/// One terminal `Message` row, reduced to exactly the four fields
/// [`aggregate_outcomes`] needs. A small owned struct rather than the full
/// generated `Message` type, the same "don't make this module know the
/// rest of that type's shape" reasoning `routing::Candidate`'s own doc
/// gives.
pub struct MessageOutcome {
    /// `Message.operator`.
    pub operator: OperatorCode,
    /// `Message.class`.
    pub class: MessageClass,
    /// `Message.routeId`, unwrapped — the caller only ever builds this from
    /// a row already filtered to `routeId IS NOT NULL`.
    pub route_id: String,
    /// `Message.state`.
    pub state: MessageState,
}

/// The schema's generated enums derive `PartialEq`/`Eq`/`Debug` but not
/// `Hash` (`include_server_schema!`'s own emission — not something this
/// crate controls), so they can't be a `HashMap` key directly. A stable
/// string tag is the cheapest way around it; `Debug`'s own output would do
/// too, but a hand-matched tag doesn't silently change if a variant's
/// derived `Debug` formatting ever does.
fn operator_tag(value: OperatorCode) -> &'static str {
    match value {
        OperatorCode::mtn => "mtn",
        OperatorCode::orange => "orange",
        OperatorCode::camtel => "camtel",
        OperatorCode::nexttel => "nexttel",
        OperatorCode::unknown => "unknown",
    }
}

fn class_tag(value: MessageClass) -> &'static str {
    match value {
        MessageClass::otp => "otp",
        MessageClass::transactional => "transactional",
        MessageClass::notification => "notification",
        MessageClass::marketing => "marketing",
    }
}

/// Fold raw per-message rows into one [`RouteOutcome`] per
/// `(operator, class, routeId)`.
///
/// # `uncertain` is excluded from both sides of the ratio — the specific
/// requirement this function exists to get right
///
/// A message in `uncertain` had its outcome genuinely never learned (#119's
/// own `Indeterminate` design) — it is not evidence the route delivered,
/// and it is not evidence the route failed. Counting it as a failure would
/// make every route with a slow-to-respond provider look like a grey route
/// purely from timeout volume, which is exactly the false-positive shape
/// AGENTS.md's own brief for this ticket warns against. Counting it as a
/// success would be equally wrong in the other direction. So it — and every
/// other non-terminal state (`accepted`/`queued`/`routed`/`submitted`) —
/// contributes to neither `delivered` nor `failed`, and therefore does not
/// change `total()` at all. Only `delivered` (success) and
/// `undelivered`/`failed`/`expired` (failure) are counted; `rejected`/
/// `cancelled` never reach this function per the module doc's own
/// reasoning.
#[must_use]
pub fn aggregate_outcomes(rows: impl IntoIterator<Item = MessageOutcome>) -> Vec<RouteOutcome> {
    let mut totals: HashMap<(&'static str, &'static str, String), RouteOutcome> = HashMap::new();
    for row in rows {
        let key = (
            operator_tag(row.operator),
            class_tag(row.class),
            row.route_id.clone(),
        );
        let entry = totals.entry(key).or_insert_with(|| RouteOutcome {
            operator: row.operator,
            class: row.class,
            route_id: row.route_id,
            delivered: 0,
            failed: 0,
        });
        match row.state {
            MessageState::delivered => entry.delivered += 1,
            MessageState::undelivered | MessageState::failed | MessageState::expired => {
                entry.failed += 1;
            }
            // uncertain, and every non-terminal state: neither side.
            _ => {}
        }
    }
    totals.into_values().collect()
}

/// One route's delivery rate diverging from its `(operator, class)` peer
/// group's own best-performing route — the payload of one `warn!` line and
/// one unit of [`sms_metrics::ROUTE_DELIVERY_DIVERGENCE_FLAGGED`].
#[derive(Debug, Clone, PartialEq)]
pub struct DivergenceFinding {
    /// The peer group's own `Message.operator`.
    pub operator: OperatorCode,
    /// The peer group's own `Message.class`.
    pub class: MessageClass,
    /// The group's best-performing qualifying route — what `route_id` was
    /// compared against.
    pub reference_route_id: String,
    /// `reference_route_id`'s own delivery rate.
    pub reference_rate: f64,
    /// `reference_route_id`'s own sample size.
    pub reference_n: u64,
    /// The route flagged as diverging from the reference.
    pub route_id: String,
    /// `route_id`'s own delivery rate.
    pub rate: f64,
    /// `route_id`'s own sample size.
    pub n: u64,
    /// `reference_rate - rate`, always `>= MIN_DELTA` for a finding to
    /// exist at all.
    pub delta: f64,
    /// The two-proportion z-score behind the finding, always
    /// `>= Z_THRESHOLD` in absolute value.
    pub z_score: f64,
}

/// Two-proportion z-test, pooled standard error — the textbook form for
/// "are these two observed rates different enough that chance alone is an
/// implausible explanation." Returns `0.0` (never flags) when the pooled
/// proportion is exactly `0` or `1`: both routes agreeing completely (all
/// delivered, or none delivered) is not divergence between them, whatever
/// it might say about the pair as a whole.
fn two_proportion_z(reference: &RouteOutcome, candidate: &RouteOutcome) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let (n1, n2) = (reference.total() as f64, candidate.total() as f64);
    let pooled = {
        #[allow(clippy::cast_precision_loss)]
        {
            (reference.delivered + candidate.delivered) as f64 / (n1 + n2)
        }
    };
    let se = (pooled * (1.0 - pooled) * (1.0 / n1 + 1.0 / n2)).sqrt();
    if se == 0.0 {
        return 0.0;
    }
    (reference.rate() - candidate.rate()) / se
}

/// The actual divergence check: group by `(operator, class)`, keep only
/// members with at least [`MIN_SAMPLE`] terminal messages, pick the
/// highest-rate qualifying member as the group's reference, and flag every
/// other qualifying member whose gap from the reference clears both
/// [`MIN_DELTA`] and [`Z_THRESHOLD`]. See the module doc for the full
/// reasoning behind each gate.
#[must_use]
pub fn detect_divergent_routes(outcomes: &[RouteOutcome]) -> Vec<DivergenceFinding> {
    // Keyed on a hashable tag pair, not the enums directly — see
    // `operator_tag`/`class_tag`'s own doc for why. The real `OperatorCode`/
    // `MessageClass` values are recovered from the group's own members
    // below (every member shares the same pair by construction), never
    // reconstructed from the tag.
    let mut groups: HashMap<(&'static str, &'static str), Vec<&RouteOutcome>> = HashMap::new();
    for outcome in outcomes {
        groups
            .entry((operator_tag(outcome.operator), class_tag(outcome.class)))
            .or_default()
            .push(outcome);
    }

    let mut findings = Vec::new();
    for members in groups.into_values() {
        let mut qualifying: Vec<&RouteOutcome> = members
            .into_iter()
            .filter(|route| route.total() >= MIN_SAMPLE)
            .collect();
        // Fewer than two qualifying members: nothing to compare, regardless
        // of how many low-sample routes exist in this group.
        if qualifying.len() < 2 {
            continue;
        }
        qualifying.sort_by(|a, b| {
            b.rate()
                .partial_cmp(&a.rate())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let reference = qualifying[0];
        for candidate in &qualifying[1..] {
            let delta = reference.rate() - candidate.rate();
            if delta < MIN_DELTA {
                continue;
            }
            let z_score = two_proportion_z(reference, candidate);
            if z_score.abs() < Z_THRESHOLD {
                continue;
            }
            findings.push(DivergenceFinding {
                operator: reference.operator,
                class: reference.class,
                reference_route_id: reference.route_id.clone(),
                reference_rate: reference.rate(),
                reference_n: reference.total(),
                route_id: candidate.route_id.clone(),
                rate: candidate.rate(),
                n: candidate.total(),
                delta,
                z_score,
            });
        }
    }
    findings
}

/// `true` when `last_validated` is missing entirely, or older than
/// [`VALIDATION_INTERVAL`]. A route validated *exactly* on the boundary is
/// not yet overdue — the comparison is strict `>`, matching #67's own
/// documented inclusive-boundary convention for `purge_retention`'s cutoff
/// (there `<=` purges at the boundary; here the equivalent "still within
/// the window" reading is not-yet-overdue at the boundary), proven by this
/// module's own boundary test.
#[must_use]
pub fn is_overdue(last_validated: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match last_validated {
        None => true,
        Some(at) => now - at > VALIDATION_INTERVAL,
    }
}

/// The `grey_route_watch` [`JobHandler`] — see the module doc for both
/// halves of what it checks.
pub struct GreyRouteWatch;

impl GreyRouteWatch {
    /// The testable core — same seam every other job in this crate uses
    /// (`ReapOutbox::run_at`, `ExpireStale::run_at`): a caller-supplied
    /// `now` rather than an internal `Utc::now()`, so a live test can prove
    /// "no validation ever" and "validated long enough ago" without
    /// depending on wall-clock time actually elapsing.
    pub async fn run_at(
        &self,
        db: &Cratestack,
        sys: &CratestackContext,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let flagged = self
            .check_divergence(db, sys, now)
            .await
            .map_err(|error| format!("checking route delivery-rate divergence: {error}"))?;
        sms_metrics::ROUTE_DELIVERY_DIVERGENCE_FLAGGED
            .set(i64::try_from(flagged).unwrap_or(i64::MAX));

        let overdue = self
            .check_overdue_validations(db, sys, now)
            .await
            .map_err(|error| format!("checking overdue route validations: {error}"))?;
        sms_metrics::ROUTE_VALIDATION_OVERDUE.set(i64::try_from(overdue).unwrap_or(i64::MAX));

        Ok(())
    }

    /// Fetch, aggregate, detect, and log — the divergence half. `pub(crate)`
    /// would do; `pub` for the same reason `reap_outbox::alert_poison_rows`
    /// is — a live test asserts the exact count directly, rather than
    /// scraping this function's own `warn!` output.
    pub async fn check_divergence(
        &self,
        db: &Cratestack,
        sys: &CratestackContext,
        now: DateTime<Utc>,
    ) -> Result<usize, CratestackError> {
        let cutoff = now - LOOKBACK;
        let rows = db
            .message()
            .find_many()
            .where_expr(
                FilterExpr::from(message::routeId().is_not_null())
                    .and(message::createdAt().gt(cutoff))
                    .and(message::state().in_([
                        MessageState::delivered,
                        MessageState::undelivered,
                        MessageState::failed,
                        MessageState::expired,
                    ])),
            )
            .order_by(message::createdAt().desc())
            .limit(FETCH_LIMIT)
            .run(sys)
            .await?;

        let outcomes = aggregate_outcomes(rows.into_iter().filter_map(|row| {
            row.routeId.map(|route_id| MessageOutcome {
                operator: row.operator,
                class: row.class,
                route_id,
                state: row.state,
            })
        }));

        let findings = detect_divergent_routes(&outcomes);
        for finding in &findings {
            warn!(
                operator = ?finding.operator,
                class = ?finding.class,
                reference_route_id = finding.reference_route_id,
                reference_rate = finding.reference_rate,
                reference_n = finding.reference_n,
                route_id = finding.route_id,
                rate = finding.rate,
                n = finding.n,
                delta = finding.delta,
                z_score = finding.z_score,
                "route delivery-rate divergence flagged — possible grey route; see \
                 docs/runbooks/grey-route-validation.adoc"
            );
        }
        Ok(findings.len())
    }

    /// Every `enabled` route, checked against its own most recent
    /// [`RouteValidation`](sms_api::schema::RouteValidation) row — the
    /// staleness half. `pub` for the same reason [`check_divergence`] is.
    pub async fn check_overdue_validations(
        &self,
        db: &Cratestack,
        sys: &CratestackContext,
        now: DateTime<Utc>,
    ) -> Result<usize, CratestackError> {
        let routes = db
            .route()
            .find_many()
            .where_expr(FilterExpr::from(route::enabled().is_true()))
            .run(sys)
            .await?;

        let mut overdue = 0usize;
        for candidate in routes {
            let last_validated = db
                .route_validation()
                .find_many()
                .where_expr(FilterExpr::from(
                    route_validation::routeId().eq(candidate.id.clone()),
                ))
                .order_by(route_validation::performedAt().desc())
                .limit(1)
                .run(sys)
                .await?
                .into_iter()
                .next()
                .map(|row| row.performedAt);

            if is_overdue(last_validated, now) {
                overdue += 1;
                warn!(
                    route_id = candidate.id,
                    route_name = candidate.name,
                    last_validated = ?last_validated,
                    "route overdue for handset validation (#64) — no RouteValidation in the \
                     last 30 days; see docs/runbooks/grey-route-validation.adoc"
                );
            }
        }
        Ok(overdue)
    }
}

#[async_trait]
impl JobHandler for GreyRouteWatch {
    fn kind(&self) -> &'static str {
        "grey_route_watch"
    }

    async fn run(
        &self,
        db: &Cratestack,
        sys: &CratestackContext,
        _job: &Job,
    ) -> Result<(), String> {
        self.run_at(db, sys, Utc::now()).await
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GreyRouteWatch, MIN_DELTA, MIN_SAMPLE, MessageOutcome, RouteOutcome, VALIDATION_INTERVAL,
        Z_THRESHOLD, aggregate_outcomes, detect_divergent_routes, is_overdue,
    };
    use crate::jobs::JobHandler;
    use chrono::{Duration, Utc};
    use sms_api::schema::{MessageClass, MessageState, OperatorCode};

    fn outcome(route_id: &str, delivered: u64, failed: u64) -> RouteOutcome {
        RouteOutcome {
            operator: OperatorCode::orange,
            class: MessageClass::otp,
            route_id: route_id.to_owned(),
            delivered,
            failed,
        }
    }

    #[test]
    fn kind_matches_the_scheduler_and_design_docs_naming() {
        assert_eq!(GreyRouteWatch.kind(), "grey_route_watch");
    }

    #[test]
    fn thresholds_match_this_files_own_documented_reasoning() {
        assert_eq!(MIN_SAMPLE, 30);
        assert!((MIN_DELTA - 0.15).abs() < f64::EPSILON);
        assert!((Z_THRESHOLD - 3.0).abs() < f64::EPSILON);
        assert_eq!(VALIDATION_INTERVAL, Duration::days(30));
    }

    // --- The highest-value guard AGENTS.md's own brief asks for: a large,
    // genuinely divergent sample is flagged; a tiny sample with a *worse*
    // ratio is not. ---

    #[test]
    fn a_large_divergent_sample_is_flagged() {
        // 98% over 1000 vs 70% over 1000 — a real, sizeable gap at a
        // trustworthy sample size on both sides.
        let outcomes = vec![outcome("route-a", 980, 20), outcome("route-b", 700, 300)];
        let findings = detect_divergent_routes(&outcomes);
        assert_eq!(
            findings.len(),
            1,
            "expected exactly one finding: {findings:?}"
        );
        let finding = &findings[0];
        assert_eq!(finding.reference_route_id, "route-a");
        assert_eq!(finding.route_id, "route-b");
        assert!(finding.z_score.abs() >= Z_THRESHOLD);
        assert!(finding.delta >= MIN_DELTA);
    }

    #[test]
    fn a_tiny_sample_with_a_worse_ratio_is_not_flagged() {
        // 100% over 4 vs 25% over 4 — a 75-point gap, far past MIN_DELTA,
        // on a sample far below MIN_SAMPLE. This is the exact "4 messages
        // at 50%" noise case AGENTS.md's own brief names. At n=4 the
        // z-test alone already can't clear Z_THRESHOLD (the largest
        // possible two-proportion z at n=4 vs n=4 is ~2.83, below the 3.0
        // bar) — see the next test for a sample that isolates MIN_SAMPLE's
        // own, independent contribution instead.
        let outcomes = vec![outcome("route-a", 4, 0), outcome("route-b", 1, 3)];
        let findings = detect_divergent_routes(&outcomes);
        assert!(
            findings.is_empty(),
            "a sample this small must never be flagged, regardless of delta: {findings:?}"
        );
    }

    #[test]
    fn min_sample_is_load_bearing_independently_of_the_z_test() {
        // 100% over 20 vs 50% over 20: both n are below MIN_SAMPLE (30),
        // but the z-score this pair produces (~3.65) clears Z_THRESHOLD on
        // its own — proving MIN_SAMPLE is doing real, independent work
        // here, not merely restating what the z-test would already refuse.
        let outcomes = vec![outcome("route-a", 20, 0), outcome("route-b", 10, 10)];
        const {
            assert!(
                20 < MIN_SAMPLE,
                "this test's own premise: both routes must be below MIN_SAMPLE"
            );
        }
        let findings = detect_divergent_routes(&outcomes);
        assert!(
            findings.is_empty(),
            "below MIN_SAMPLE must never be flagged even when the z-test alone would allow it: \
             {findings:?}"
        );
    }

    #[test]
    fn a_large_sample_with_a_small_delta_is_not_flagged() {
        // 98.3% vs 98.0% over 5000 each — plausibly statistically real,
        // but not the kind of gap #64 exists to catch. MIN_DELTA is the
        // gate that stops this specifically.
        let outcomes = vec![outcome("route-a", 4915, 85), outcome("route-b", 4900, 100)];
        let findings = detect_divergent_routes(&outcomes);
        assert!(
            findings.is_empty(),
            "a practically-irrelevant delta must not be flagged even at huge n: {findings:?}"
        );
    }

    #[test]
    fn exactly_one_qualifying_route_in_a_group_has_nothing_to_compare_against() {
        let outcomes = vec![outcome("route-a", 900, 100), outcome("route-b", 4, 0)];
        let findings = detect_divergent_routes(&outcomes);
        assert!(
            findings.is_empty(),
            "one qualifying route is not a comparison: {findings:?}"
        );
    }

    #[test]
    fn different_operator_or_class_peer_groups_are_never_compared_to_each_other() {
        let mut worse_group = outcome("route-b", 700, 300);
        worse_group.operator = OperatorCode::mtn; // different peer group
        let outcomes = vec![outcome("route-a", 980, 20), worse_group];
        let findings = detect_divergent_routes(&outcomes);
        assert!(
            findings.is_empty(),
            "routes serving different operators are not peers: {findings:?}"
        );
    }

    #[test]
    fn two_equally_bad_routes_are_not_divergent_from_each_other() {
        // Both routes at the same low rate — nothing to flag between them,
        // whatever it might say about the pair overall (a different alert's
        // job, not this one's).
        let outcomes = vec![outcome("route-a", 300, 700), outcome("route-b", 305, 695)];
        let findings = detect_divergent_routes(&outcomes);
        assert!(
            findings.is_empty(),
            "equally bad is not divergent: {findings:?}"
        );
    }

    // --- uncertain must not be counted as a delivery failure. ---

    #[test]
    fn uncertain_messages_are_excluded_from_both_the_numerator_and_the_denominator() {
        let mut rows = Vec::new();
        for _ in 0..30 {
            rows.push(MessageOutcome {
                operator: OperatorCode::orange,
                class: MessageClass::otp,
                route_id: "route-a".to_owned(),
                state: MessageState::delivered,
            });
        }
        for _ in 0..30 {
            rows.push(MessageOutcome {
                operator: OperatorCode::orange,
                class: MessageClass::otp,
                route_id: "route-a".to_owned(),
                state: MessageState::uncertain,
            });
        }
        let aggregated = aggregate_outcomes(rows);
        assert_eq!(aggregated.len(), 1);
        let route = &aggregated[0];
        assert_eq!(route.delivered, 30, "uncertain must not count as delivered");
        assert_eq!(route.failed, 0, "uncertain must not count as failed");
        assert_eq!(
            route.total(),
            30,
            "uncertain must not inflate the denominator"
        );
    }

    #[test]
    fn non_terminal_in_flight_states_are_also_excluded() {
        let mut rows = vec![MessageOutcome {
            operator: OperatorCode::orange,
            class: MessageClass::otp,
            route_id: "route-a".to_owned(),
            state: MessageState::delivered,
        }];
        for state in [
            MessageState::accepted,
            MessageState::queued,
            MessageState::routed,
            MessageState::submitted,
        ] {
            rows.push(MessageOutcome {
                operator: OperatorCode::orange,
                class: MessageClass::otp,
                route_id: "route-a".to_owned(),
                state,
            });
        }
        let aggregated = aggregate_outcomes(rows);
        assert_eq!(aggregated[0].total(), 1, "only the one terminal row counts");
    }

    // --- overdue validation staleness. ---

    #[test]
    fn a_route_never_validated_is_overdue() {
        assert!(is_overdue(None, Utc::now()));
    }

    #[test]
    fn a_route_validated_recently_is_not_overdue() {
        let now = Utc::now();
        assert!(!is_overdue(Some(now - Duration::days(10)), now));
    }

    #[test]
    fn a_route_validated_just_past_the_window_is_overdue() {
        let now = Utc::now();
        assert!(is_overdue(
            Some(now - VALIDATION_INTERVAL - Duration::seconds(1)),
            now
        ));
    }

    #[test]
    fn a_route_validated_exactly_at_the_boundary_is_not_yet_overdue() {
        let now = Utc::now();
        assert!(!is_overdue(Some(now - VALIDATION_INTERVAL), now));
    }
}
