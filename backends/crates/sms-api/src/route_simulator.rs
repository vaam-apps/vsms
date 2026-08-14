//! #54: the route simulator — "given this recipient, class and app, which
//! route wins and why" without sending anything.
//!
//! # Expose the engine's `Decision`, never re-decide
//!
//! `sms_routing::select_route` (`backends/crates/sms-routing`, #62) already computes
//! the whole answer, including the explanation trail — a caller-supplied
//! `draw` is what makes it deterministic and replayable (see its own doc).
//! Everything in this module is either **fetching** the rows the engine
//! needs (identical shape to `backends/crates/sms-worker/src/routing.rs::decide`) or
//! **rendering** the `Decision` it returns onto the wire (`decision_to_wire`)
//! — never a second implementation of matching. `the_wire_result_matches_the_engines_own_decision`
//! below is the guard that proves the rendering step can't silently drift
//! from what the engine actually decided; see its own doc for how it was
//! confirmed to actually fail before being trusted.
//!
//! # Why this duplicates `sms-worker`'s own I/O glue
//!
//! `backends/crates/sms-worker/src/routing.rs` already does exactly this fetch +
//! convert dance for production dispatch. This module can't call it: `sms-
//! worker` depends on `sms-api` (for `schema::Cratestack` and friends), so
//! the dependency can't run the other way without a cycle — confirmed by
//! `sms-worker.workspace = true` sitting in `backends/crates/sms-api/Cargo.toml`'s
//! own `[dev-dependencies]` (test-only, deliberately never `[dependencies]`,
//! per that file's own comment on `worker_locks_live_postgres.rs`). The root
//! `Cargo.toml`'s own comment on the `sms-routing` dependency edge names
//! this exact situation as expected: "a future #54 simulator procedure in
//! sms-api is expected to depend on this directly too" — `sms_routing`
//! itself (the pure engine) is shared; the I/O glue around it is not, and
//! is small enough (four straightforward `match`/field-copy functions) that
//! duplicating it is cheaper than inventing a third crate beneath both just
//! to share sixty lines. If a third caller of this exact glue ever shows up,
//! that calculus changes — matching the precedent `sms-provider-mtn`'s own
//! module doc already sets for `classify_transport_error`'s identical
//! two-crate duplication.

use std::collections::HashMap;

use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_routing::{
    Decision, MessageClass, Operator, PredicateFailure, ProviderRow, RouteOutcome, RouteRow,
};

use crate::schema::{self, provider, route};

// --- I/O glue: fetch + convert, mirroring `sms-worker/src/routing.rs`. ---

/// `pub(crate)`: `Procedures::simulate_route` (`procedures.rs`) needs this
/// to build a `sms_routing::RoutingCandidate` from the already-classified
/// `schema::OperatorCode` — the candidate assembly itself stays in
/// `procedures.rs`, next to the msisdn-parsing/operator-classification steps
/// it shares with `sendMessage`, rather than duplicated a second time here.
pub(crate) fn convert_operator(value: schema::OperatorCode) -> Operator {
    match value {
        schema::OperatorCode::mtn => Operator::Mtn,
        schema::OperatorCode::orange => Operator::Orange,
        schema::OperatorCode::camtel => Operator::Camtel,
        schema::OperatorCode::nexttel => Operator::Nexttel,
        schema::OperatorCode::unknown => Operator::Unknown,
    }
}

/// See [`convert_operator`]'s doc.
pub(crate) fn convert_class(value: schema::MessageClass) -> MessageClass {
    match value {
        schema::MessageClass::otp => MessageClass::Otp,
        schema::MessageClass::transactional => MessageClass::Transactional,
        schema::MessageClass::notification => MessageClass::Notification,
        schema::MessageClass::marketing => MessageClass::Marketing,
    }
}

/// The inverse of [`convert_operator`] — needed here (and not in
/// `sms-worker/src/routing.rs`, which never renders a `sms_routing::Operator`
/// back out) to turn a `PredicateFailure::Operator`'s `expected`/`actual`
/// into the wire's plain display strings. Renders through the wire's own
/// lowercase-verbatim convention, matching `procedures.rs`'s
/// `operator_code_str` for `schema::OperatorCode` — a second small string
/// table over a different type, not a shared function, for the same reason
/// this module's other `convert_*` functions are duplicated rather than
/// imported.
fn operator_str(value: Operator) -> &'static str {
    match value {
        Operator::Mtn => "mtn",
        Operator::Orange => "orange",
        Operator::Camtel => "camtel",
        Operator::Nexttel => "nexttel",
        Operator::Unknown => "unknown",
    }
}

/// The inverse of [`convert_class`] — see [`operator_str`]'s doc for why
/// this exists here rather than being shared.
fn class_str(value: MessageClass) -> &'static str {
    match value {
        MessageClass::Otp => "otp",
        MessageClass::Transactional => "transactional",
        MessageClass::Notification => "notification",
        MessageClass::Marketing => "marketing",
    }
}

fn convert_route(row: &schema::Route) -> RouteRow {
    RouteRow {
        id: row.id.clone(),
        name: row.name.clone(),
        priority: row.priority,
        weight: row.weight,
        enabled: row.enabled,
        match_operator: row.matchOperator.map(convert_operator),
        match_class: row.matchClass.map(convert_class),
        match_app_id: row.matchAppId.clone(),
        match_prefix: row.matchPrefix.clone(),
        provider_id: row.providerId.clone(),
        failover_route_id: row.failoverRouteId.clone(),
    }
}

/// Identical reasoning to `sms-worker/src/routing.rs::convert_provider`:
/// `Provider.healthy` is never set anywhere in this codebase yet (its only
/// would-be writer, §7.5's `probe_providers` job, doesn't exist), so
/// availability stays `state == active` alone — the same check production
/// routing uses, not a stricter one the simulator would then disagree with
/// production about.
fn convert_provider(row: &schema::Provider) -> ProviderRow {
    let available = row.state == schema::ProviderState::active;
    let reason = (!available).then(|| format!("provider state is {:?}, not active", row.state));
    ProviderRow {
        id: row.id.clone(),
        available,
        reason,
    }
}

/// Fetch every `Route` row (same deterministic ordering
/// `backends/crates/sms-worker/src/routing.rs::decide` uses — `priority` desc then
/// `id` asc — so a caller-supplied `draw` reproduces the same winner on a
/// replay) plus the `Provider` rows they reference, converted onto
/// `sms_routing`'s pure types. Never touches `sms_routing::select_route`
/// itself — that's the caller's job, so [`the_wire_result_matches_the_engines_own_decision`]
/// below can exercise the engine and [`decision_to_wire`] without a
/// database at all.
pub(crate) async fn fetch_routes_and_providers(
    db: &schema::Cratestack,
    sys: &CoolContext,
) -> Result<(Vec<RouteRow>, HashMap<String, ProviderRow>), CoolError> {
    let routes = db
        .route()
        .find_many()
        .order_by(route::priority().desc())
        .order_by(route::id().asc())
        .run(sys)
        .await?;

    let mut provider_ids: Vec<String> = routes.iter().map(|r| r.providerId.clone()).collect();
    provider_ids.sort_unstable();
    provider_ids.dedup();

    let providers: HashMap<String, ProviderRow> = if provider_ids.is_empty() {
        HashMap::new()
    } else {
        db.provider()
            .find_many()
            .where_expr(FilterExpr::from(provider::id().in_(provider_ids)))
            .run(sys)
            .await?
            .iter()
            .map(|row| (row.id.clone(), convert_provider(row)))
            .collect()
    };

    let route_rows: Vec<RouteRow> = routes.iter().map(convert_route).collect();
    Ok((route_rows, providers))
}

fn predicate_kind(failure: &PredicateFailure) -> schema::PredicateKind {
    match failure {
        PredicateFailure::Operator { .. } => schema::PredicateKind::operator,
        PredicateFailure::Class { .. } => schema::PredicateKind::class,
        PredicateFailure::AppId { .. } => schema::PredicateKind::app_id,
        PredicateFailure::Prefix { .. } => schema::PredicateKind::prefix,
    }
}

fn predicate_expected_actual(failure: &PredicateFailure) -> (String, String) {
    match failure {
        PredicateFailure::Operator { expected, actual } => (
            operator_str(*expected).to_owned(),
            operator_str(*actual).to_owned(),
        ),
        PredicateFailure::Class { expected, actual } => (
            class_str(*expected).to_owned(),
            class_str(*actual).to_owned(),
        ),
        PredicateFailure::AppId { expected, actual } => (expected.clone(), actual.clone()),
        PredicateFailure::Prefix {
            expected,
            msisdn_national,
        } => (expected.clone(), msisdn_national.clone()),
    }
}

/// One `sms_routing::RouteEvaluation` rendered onto the wire — split out of
/// [`decision_to_wire`] purely to stay under `clippy::too_many_lines`; the
/// two together are still one straight-line rendering pass, no behaviour
/// split across the boundary.
fn evaluation_to_wire(evaluation: &sms_routing::RouteEvaluation) -> schema::RouteEvaluationInfo {
    let (
        outcome,
        winning_band,
        predicate_kind,
        predicate_expected,
        predicate_actual,
        unavailable_reason,
    ) = match &evaluation.outcome {
        RouteOutcome::Excluded => (
            schema::RouteOutcomeKind::excluded,
            false,
            None,
            None,
            None,
            None,
        ),
        RouteOutcome::Disabled => (
            schema::RouteOutcomeKind::disabled,
            false,
            None,
            None,
            None,
            None,
        ),
        RouteOutcome::PredicateFailed(failure) => {
            let (expected, actual) = predicate_expected_actual(failure);
            (
                schema::RouteOutcomeKind::predicate_failed,
                false,
                Some(predicate_kind(failure)),
                Some(expected),
                Some(actual),
                None,
            )
        }
        RouteOutcome::ProviderUnavailable(reason) => (
            schema::RouteOutcomeKind::provider_unavailable,
            false,
            None,
            None,
            None,
            Some(reason.clone()),
        ),
        RouteOutcome::Eligible { winning_band } => (
            schema::RouteOutcomeKind::eligible,
            *winning_band,
            None,
            None,
            None,
            None,
        ),
    };

    schema::RouteEvaluationInfo {
        routeId: evaluation.route_id.clone(),
        routeName: evaluation.route_name.clone(),
        priority: evaluation.priority,
        weight: evaluation.weight,
        providerId: evaluation.provider_id.clone(),
        outcome,
        winningBand: winning_band,
        predicateKind: predicate_kind,
        predicateExpected: predicate_expected,
        predicateActual: predicate_actual,
        unavailableReason: unavailable_reason,
    }
}

/// Purely presentational: every field here is read straight off `decision`
/// (or a per-route `RouteOutcome`/`PredicateFailure` inside it, via
/// [`evaluation_to_wire`]) — nothing here re-evaluates a predicate,
/// re-ranks a priority band, or re-draws a weighted pick.
/// `operator`/`msisdn_national`/`no_routes_configured` are context
/// [`crate::procedures::Procedures::simulate_route`] already computed
/// before calling `sms_routing::select_route`, threaded through rather
/// than recomputed here.
pub(crate) fn decision_to_wire(
    decision: &Decision,
    operator: schema::OperatorCode,
    msisdn_national: &str,
    no_routes_configured: bool,
) -> schema::SimulateRouteResult {
    let evaluations = decision
        .evaluations
        .iter()
        .map(evaluation_to_wire)
        .collect();

    let tie_break = decision
        .tie_break
        .as_ref()
        .map(|tie_break| schema::TieBreakInfo {
            priority: tie_break.priority,
            draw: tie_break.draw,
            ranges: tie_break
                .ranges
                .iter()
                .map(|range| schema::TieBreakRangeInfo {
                    routeId: range.route_id.clone(),
                    weight: range.weight,
                    low: range.low,
                    high: range.high,
                })
                .collect(),
            winnerRouteId: tie_break.winner_route_id.clone(),
        });

    let winner = decision
        .winner
        .as_ref()
        .map(|winner| schema::RouteWinnerInfo {
            routeId: winner.route_id.clone(),
            providerId: winner.provider_id.clone(),
            failoverRouteId: winner.failover_route_id.clone(),
        });

    schema::SimulateRouteResult {
        operator,
        msisdnNational: msisdn_national.to_owned(),
        noRoutesConfigured: no_routes_configured,
        evaluations,
        tieBreak: tie_break,
        winner,
    }
}

#[cfg(test)]
mod tests {
    use super::decision_to_wire;
    use crate::schema;
    use sms_routing::{
        select_route, ExcludedRouteIds, MessageClass, Operator, ProviderRow, RouteOutcome,
        RouteRow, RoutingCandidate,
    };
    use std::collections::HashMap;

    fn route(id: &str, priority: i64, weight: i64, provider_id: &str) -> RouteRow {
        RouteRow {
            id: id.to_owned(),
            name: format!("route-{id}"),
            priority,
            weight,
            enabled: true,
            match_operator: None,
            match_class: None,
            match_app_id: None,
            match_prefix: None,
            provider_id: provider_id.to_owned(),
            failover_route_id: None,
        }
    }

    fn available_provider(id: &str) -> (String, ProviderRow) {
        (
            id.to_owned(),
            ProviderRow {
                id: id.to_owned(),
                available: true,
                reason: None,
            },
        )
    }

    fn candidate() -> RoutingCandidate<'static> {
        RoutingCandidate {
            operator: Operator::Mtn,
            class: MessageClass::Otp,
            app_id: "app1",
            msisdn_national: "677123456",
        }
    }

    /// **The guard #54 asks for**: the rendered `SimulateRouteResult` must
    /// match what `sms_routing::select_route` actually decided, for every
    /// field a caller can act on — not just "produces something". Confirmed
    /// live to actually fail, not just pass by construction: temporarily
    /// changing `decision_to_wire`'s `Eligible` arm to hardcode
    /// `winning_band` to `false` regardless of the engine's own value
    /// reproduced a real assertion failure here (`left: false, right: true`
    /// on the winning route's `winningBand`) before being reverted — see
    /// this PR's own description for the exact command and output.
    ///
    /// `clippy::float_cmp` is silenced deliberately, not blanket-ignored:
    /// every float compared below is a straight copy through
    /// `decision_to_wire` (`wire_tie_break.draw` is the exact same `f64`
    /// as `tie_break.draw`, never independently recomputed), so bit-exact
    /// equality is the correct assertion, not an approximation that would
    /// mask real drift.
    #[test]
    #[allow(clippy::float_cmp)]
    fn the_wire_result_matches_the_engines_own_decision() {
        let routes = vec![
            route("r-low", 0, 1, "p1"),
            route("r-high-a", 10, 1, "p1"),
            route("r-high-b", 10, 3, "p1"),
        ];
        let providers = HashMap::from([available_provider("p1")]);
        let exclude = ExcludedRouteIds::new();
        let draw = 0.9; // lands in the heavier r-high-b's [0.25, 1.0) share.

        let decision = select_route(&routes, &providers, &candidate(), &exclude, draw);
        let wire = decision_to_wire(&decision, schema::OperatorCode::mtn, "677123456", false);

        assert_eq!(wire.evaluations.len(), decision.evaluations.len());
        assert_eq!(
            wire.winner.as_ref().map(|w| w.routeId.clone()),
            decision.winner.as_ref().map(|w| w.route_id.clone())
        );
        assert_eq!(
            wire.winner.as_ref().map(|w| w.providerId.clone()),
            decision.winner.as_ref().map(|w| w.provider_id.clone())
        );

        for (wire_eval, engine_eval) in wire.evaluations.iter().zip(decision.evaluations.iter()) {
            assert_eq!(wire_eval.routeId, engine_eval.route_id);
            assert_eq!(wire_eval.priority, engine_eval.priority);
            assert_eq!(wire_eval.weight, engine_eval.weight);
            match &engine_eval.outcome {
                RouteOutcome::Eligible { winning_band } => {
                    assert_eq!(wire_eval.outcome, schema::RouteOutcomeKind::eligible);
                    assert_eq!(wire_eval.winningBand, *winning_band);
                }
                RouteOutcome::Excluded => {
                    assert_eq!(wire_eval.outcome, schema::RouteOutcomeKind::excluded);
                }
                RouteOutcome::Disabled => {
                    assert_eq!(wire_eval.outcome, schema::RouteOutcomeKind::disabled);
                }
                RouteOutcome::PredicateFailed(_) => {
                    assert_eq!(
                        wire_eval.outcome,
                        schema::RouteOutcomeKind::predicate_failed
                    );
                    assert!(wire_eval.predicateKind.is_some());
                }
                RouteOutcome::ProviderUnavailable(reason) => {
                    assert_eq!(
                        wire_eval.outcome,
                        schema::RouteOutcomeKind::provider_unavailable
                    );
                    assert_eq!(
                        wire_eval.unavailableReason.as_deref(),
                        Some(reason.as_str())
                    );
                }
            }
        }

        let tie_break = decision
            .tie_break
            .expect("a two-member 10-priority band ties");
        let wire_tie_break = wire.tieBreak.expect("wire must carry the tie-break too");
        assert_eq!(wire_tie_break.priority, tie_break.priority);
        assert_eq!(wire_tie_break.draw, tie_break.draw);
        assert_eq!(wire_tie_break.winnerRouteId, tie_break.winner_route_id);
        assert_eq!(wire_tie_break.ranges.len(), tie_break.ranges.len());
        for (wire_range, engine_range) in wire_tie_break.ranges.iter().zip(tie_break.ranges.iter())
        {
            assert_eq!(wire_range.routeId, engine_range.route_id);
            assert_eq!(wire_range.weight, engine_range.weight);
            assert_eq!(wire_range.low, engine_range.low);
            assert_eq!(wire_range.high, engine_range.high);
        }
    }

    #[test]
    fn a_mismatched_operator_predicate_renders_its_expected_and_actual_as_lowercase_strings() {
        let mut r = route("r1", 0, 1, "p1");
        r.match_operator = Some(Operator::Orange);
        let providers = HashMap::from([available_provider("p1")]);
        let decision = select_route(
            &[r],
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.5,
        );
        let wire = decision_to_wire(&decision, schema::OperatorCode::mtn, "677123456", false);

        assert_eq!(wire.evaluations.len(), 1);
        let evaluation = &wire.evaluations[0];
        assert_eq!(
            evaluation.outcome,
            schema::RouteOutcomeKind::predicate_failed
        );
        assert_eq!(
            evaluation.predicateKind,
            Some(schema::PredicateKind::operator)
        );
        assert_eq!(evaluation.predicateExpected.as_deref(), Some("orange"));
        assert_eq!(evaluation.predicateActual.as_deref(), Some("mtn"));
    }

    #[test]
    fn zero_routes_at_all_is_distinct_from_zero_eligible_routes() {
        let empty: Vec<RouteRow> = vec![];
        let providers = HashMap::new();
        let decision = select_route(
            &empty,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.5,
        );
        let wire = decision_to_wire(&decision, schema::OperatorCode::mtn, "677123456", true);

        assert!(wire.noRoutesConfigured);
        assert!(wire.evaluations.is_empty());
        assert!(wire.winner.is_none());
    }
}
