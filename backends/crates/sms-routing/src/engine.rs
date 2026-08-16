//! [`select_route`] — the whole engine, one function.

use std::collections::HashMap;

use crate::types::{
    Decision, ExcludedRouteIds, PredicateFailure, ProviderRow, RouteEvaluation, RouteOutcome,
    RouteRow, RoutingCandidate, TieBreak, TieBreakRange, Winner,
};

/// Select a route for `candidate` out of `routes`, deterministically.
///
/// # Determinism vs. `weight`
///
/// §6.3 says a route's `weight` implies a *random* choice among
/// equal-priority survivors, which is in direct tension with "deterministic
/// and explainable" (#62's own framing). Resolved by injecting the
/// randomness rather than generating it here: `draw` is a single, already-
/// drawn `f64`, expected in `[0.0, 1.0)` (clamped defensively if not —
/// see below). Production draws it once per routing decision, immediately
/// before calling this function (`rand::random::<f64>()` in
/// `backends/crates/sms-worker/src/routing.rs`); a replay — #54's simulator, or a
/// test — calls this function again with the exact same `draw` and gets
/// the exact same [`Decision`], because nothing in this function ever
/// touches an RNG, a clock, or any other ambient state. This is also why
/// the function is synchronous and takes owned/borrowed data only: no I/O,
/// nothing async, nothing to mock.
///
/// # Ordering
///
/// §6.3: "sort by priority then weighted-random within a priority band".
/// Priority is the *only* cross-band ordering this function applies —
/// there is no independent "more specific route wins" tiebreak. An
/// operator who wants a narrowly-targeted route to beat a broad one must
/// give it a higher `priority`; this function does not infer specificity
/// from how many `match_*` fields are set. Within one priority band,
/// `routes` must be supplied in a stable order (the caller's own fetch
/// should sort deterministically, e.g. `id` ascending) for a replay with
/// the same `draw` to reproduce the same winner — this function does not
/// re-sort its input.
///
/// # `exclude`
///
/// Route ids in `exclude` are skipped before anything else is evaluated —
/// #63's own "give me the next route after this one failed" mechanism.
/// Passing the same `routes`/`providers`/`candidate` again with a failed
/// route's id added to `exclude` (and a fresh `draw`, since the original
/// draw's meaning no longer applies to a different candidate set) finds
/// the next-best route without this crate needing to know anything about
/// failover, circuit breakers, or attempt counts — all #63's concern.
///
/// # Provider lookup
///
/// A route whose `provider_id` has no entry in `providers` is treated the
/// same as one whose entry has `available: false` — [`RouteOutcome::ProviderUnavailable`]
/// with a generic reason. This should not happen in practice (the caller
/// fetches providers by exactly the ids `routes` references), but a
/// missing entry must never panic or silently match "anything".
///
/// `providers` takes the default `HashMap` hasher rather than a generic
/// `BuildHasher` parameter (clippy's `implicit_hasher` lint would prefer
/// one) — this crate has no performance-sensitive map usage to justify
/// that generality, and every real caller (`backends/crates/sms-worker/src/routing.rs`,
/// this crate's own tests) already builds a plain `HashMap`.
///
/// # Panics
///
/// Never, for any input — every `.expect(...)` internal to this function
/// documents an invariant this function itself establishes just before
/// calling it (e.g. "the winning band is non-empty because it was only
/// ever populated with eligible routes").
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn select_route(
    routes: &[RouteRow],
    providers: &HashMap<String, ProviderRow>,
    candidate: &RoutingCandidate<'_>,
    exclude: &ExcludedRouteIds,
    draw: f64,
) -> Decision {
    let draw = draw.clamp(0.0, 0.999_999_999_9);

    let mut evaluations: Vec<RouteEvaluation> = routes
        .iter()
        .map(|route| evaluate_one(route, providers, candidate, exclude))
        .collect();

    // The highest priority among routes that survived every filter so
    // far — the band that actually contends for the pick. `None` means
    // nothing was eligible at all.
    let winning_priority = evaluations
        .iter()
        .zip(routes)
        .filter(|(evaluation, _)| matches!(evaluation.outcome, RouteOutcome::Eligible { .. }))
        .map(|(_, route)| route.priority)
        .max();

    let Some(winning_priority) = winning_priority else {
        return Decision {
            evaluations,
            tie_break: None,
            winner: None,
        };
    };

    // Flip winning-band members' outcome to `winning_band: true`, and
    // collect them (with their originating `RouteRow`) for the draw.
    let mut band: Vec<&RouteRow> = Vec::new();
    for (evaluation, route) in evaluations.iter_mut().zip(routes) {
        if route.priority == winning_priority
            && matches!(evaluation.outcome, RouteOutcome::Eligible { .. })
        {
            evaluation.outcome = RouteOutcome::Eligible { winning_band: true };
            band.push(route);
        }
    }

    let (winner_route, tie_break) = if band.len() == 1 {
        (band[0], None)
    } else {
        let (winner_id, tie_break) = pick_weighted(&band, winning_priority, draw);
        let winner_route = band
            .iter()
            .find(|r| r.id == winner_id)
            .expect("pick_weighted always returns a route id present in band");
        (*winner_route, Some(tie_break))
    };

    Decision {
        evaluations,
        tie_break,
        winner: Some(Winner {
            route_id: winner_route.id.clone(),
            provider_id: winner_route.provider_id.clone(),
            failover_route_id: winner_route.failover_route_id.clone(),
        }),
    }
}

fn evaluate_one(
    route: &RouteRow,
    providers: &HashMap<String, ProviderRow>,
    candidate: &RoutingCandidate<'_>,
    exclude: &ExcludedRouteIds,
) -> RouteEvaluation {
    let outcome = if exclude.contains(&route.id) {
        RouteOutcome::Excluded
    } else if !route.enabled {
        RouteOutcome::Disabled
    } else if let Some(failure) = failing_predicate(route, candidate) {
        RouteOutcome::PredicateFailed(failure)
    } else {
        match providers.get(&route.provider_id) {
            Some(provider) if provider.available => RouteOutcome::Eligible {
                winning_band: false,
            },
            Some(provider) => RouteOutcome::ProviderUnavailable(
                provider
                    .reason
                    .clone()
                    .unwrap_or_else(|| "provider unavailable".to_owned()),
            ),
            None => RouteOutcome::ProviderUnavailable(format!(
                "no provider record for {}",
                route.provider_id
            )),
        }
    };

    RouteEvaluation {
        route_id: route.id.clone(),
        route_name: route.name.clone(),
        priority: route.priority,
        weight: route.weight,
        provider_id: route.provider_id.clone(),
        outcome,
    }
}

/// `None` predicates are wildcards (see [`RouteRow`]'s own doc) — the
/// first non-matching `Some` predicate wins, checked in a fixed order
/// (operator, class, app, prefix) so the same route always reports the
/// same failure reason regardless of caller-side field order.
fn failing_predicate(
    route: &RouteRow,
    candidate: &RoutingCandidate<'_>,
) -> Option<PredicateFailure> {
    if let Some(expected) = route.match_operator
        && expected != candidate.operator
    {
        return Some(PredicateFailure::Operator {
            expected,
            actual: candidate.operator,
        });
    }
    if let Some(expected) = route.match_class
        && expected != candidate.class
    {
        return Some(PredicateFailure::Class {
            expected,
            actual: candidate.class,
        });
    }
    if let Some(expected) = &route.match_app_id
        && expected != candidate.app_id
    {
        return Some(PredicateFailure::AppId {
            expected: expected.clone(),
            actual: candidate.app_id.to_owned(),
        });
    }
    if let Some(expected) = &route.match_prefix
        && !candidate.msisdn_national.starts_with(expected.as_str())
    {
        return Some(PredicateFailure::Prefix {
            expected: expected.clone(),
            msisdn_national: candidate.msisdn_national.to_owned(),
        });
    }
    None
}

/// Weighted draw over `band` (all same priority, `len() >= 2` — the
/// `len() == 1` case is handled by the caller without invoking this at
/// all, since there's no tie to break). Cumulative ranges are built in
/// `band`'s own order — the caller's `routes` input order restricted to
/// this priority — so the same `draw` against the same `band` always
/// yields the same winner.
///
/// A band where every member has `weight == 0` falls back to a uniform
/// draw (each member gets an equal share) rather than dividing by zero —
/// `weight == 0` is a legal value (`@range(min: 0, ...)`), and "nobody in
/// this band has a positive weight" should behave like "no preference
/// stated", not "nothing here is ever selectable".
fn pick_weighted(band: &[&RouteRow], priority: i64, draw: f64) -> (String, TieBreak) {
    let total_weight: i64 = band.iter().map(|r| r.weight.max(0)).sum();
    let uniform = total_weight == 0;
    #[allow(clippy::cast_precision_loss)]
    let denom = if uniform {
        band.len() as f64
    } else {
        total_weight as f64
    };

    let mut ranges = Vec::with_capacity(band.len());
    let mut acc = 0.0_f64;
    let mut winner: Option<String> = None;

    for route in band {
        #[allow(clippy::cast_precision_loss)]
        let share = if uniform {
            1.0
        } else {
            route.weight.max(0) as f64
        };
        let low = acc / denom;
        acc += share;
        let high = acc / denom;
        if winner.is_none() && draw < high {
            winner = Some(route.id.clone());
        }
        ranges.push(TieBreakRange {
            route_id: route.id.clone(),
            weight: route.weight,
            low,
            high,
        });
    }

    // Floating-point slop guard: `draw` was clamped below 1.0 and every
    // range's `high` should reach 1.0 by construction, but if rounding
    // ever left `draw` unmatched, the last member is the correct fallback
    // — it owns the range up to (and, practically, including) 1.0.
    let winner = winner.unwrap_or_else(|| {
        band.last()
            .expect("pick_weighted is only ever called with a non-empty band")
            .id
            .clone()
    });

    (
        winner.clone(),
        TieBreak {
            priority,
            draw,
            ranges,
            winner_route_id: winner,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::select_route;
    use crate::types::{
        ExcludedRouteIds, MessageClass, Operator, ProviderRow, RouteOutcome, RouteRow,
        RoutingCandidate,
    };
    use std::collections::HashMap;

    fn route(id: &str, priority: i64, weight: i64, provider_id: &str) -> RouteRow {
        RouteRow {
            id: id.to_owned(),
            name: id.to_owned(),
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

    #[test]
    fn no_routes_at_all_means_no_winner() {
        let providers = HashMap::new();
        let decision = select_route(&[], &providers, &candidate(), &ExcludedRouteIds::new(), 0.5);
        assert!(decision.winner.is_none());
        assert!(decision.evaluations.is_empty());
    }

    #[test]
    fn a_null_predicate_is_a_wildcard_not_a_null_match() {
        let routes = vec![route("r1", 0, 1, "p1")];
        let providers = HashMap::from([available_provider("p1")]);
        let decision = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.5,
        );
        assert_eq!(decision.winner.unwrap().route_id, "r1");
    }

    /// The one matching rule this ticket explicitly asks to be proven
    /// breakable-and-fixable in the PR: a non-null `matchOperator` that
    /// disagrees with the candidate must exclude the route, with the
    /// exact predicate failure recorded (not just "not eligible").
    #[test]
    fn a_mismatched_operator_predicate_excludes_the_route_with_the_reason_recorded() {
        let mut r = route("r1", 0, 1, "p1");
        r.match_operator = Some(Operator::Orange);
        let providers = HashMap::from([available_provider("p1")]);
        let decision = select_route(
            &[r],
            &providers,
            &candidate(), // candidate.operator == Mtn
            &ExcludedRouteIds::new(),
            0.5,
        );
        assert!(decision.winner.is_none());
        assert_eq!(decision.evaluations.len(), 1);
        match &decision.evaluations[0].outcome {
            RouteOutcome::PredicateFailed(super::PredicateFailure::Operator {
                expected,
                actual,
            }) => {
                assert_eq!(*expected, Operator::Orange);
                assert_eq!(*actual, Operator::Mtn);
            }
            other => panic!("expected an Operator predicate failure, got {other:?}"),
        }
    }

    #[test]
    fn class_app_and_prefix_predicates_each_exclude_on_mismatch() {
        let providers = HashMap::from([available_provider("p1")]);

        let mut by_class = route("class", 0, 1, "p1");
        by_class.match_class = Some(MessageClass::Marketing);
        let decision = select_route(
            &[by_class],
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.5,
        );
        assert!(decision.winner.is_none());

        let mut by_app = route("app", 0, 1, "p1");
        by_app.match_app_id = Some("some-other-app".to_owned());
        let decision = select_route(
            &[by_app],
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.5,
        );
        assert!(decision.winner.is_none());

        let mut by_prefix = route("prefix", 0, 1, "p1");
        by_prefix.match_prefix = Some("699".to_owned()); // candidate is 677...
        let decision = select_route(
            &[by_prefix],
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.5,
        );
        assert!(decision.winner.is_none());
    }

    #[test]
    fn a_matching_prefix_is_eligible() {
        let mut r = route("r1", 0, 1, "p1");
        r.match_prefix = Some("677".to_owned());
        let providers = HashMap::from([available_provider("p1")]);
        let decision = select_route(
            &[r],
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.5,
        );
        assert_eq!(decision.winner.unwrap().route_id, "r1");
    }

    #[test]
    fn a_disabled_route_is_excluded_even_if_it_would_otherwise_match() {
        let mut r = route("r1", 0, 1, "p1");
        r.enabled = false;
        let providers = HashMap::from([available_provider("p1")]);
        let decision = select_route(
            &[r],
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.5,
        );
        assert!(decision.winner.is_none());
        assert_eq!(decision.evaluations[0].outcome, RouteOutcome::Disabled);
    }

    #[test]
    fn an_unavailable_provider_excludes_its_route() {
        let routes = vec![route("r1", 0, 1, "p1")];
        let providers = HashMap::from([(
            "p1".to_owned(),
            ProviderRow {
                id: "p1".to_owned(),
                available: false,
                reason: Some("state=disabled".to_owned()),
            },
        )]);
        let decision = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.5,
        );
        assert!(decision.winner.is_none());
        assert_eq!(
            decision.evaluations[0].outcome,
            RouteOutcome::ProviderUnavailable("state=disabled".to_owned())
        );
    }

    #[test]
    fn a_route_referencing_an_unknown_provider_is_unavailable_not_a_panic() {
        let routes = vec![route("r1", 0, 1, "ghost")];
        let providers = HashMap::new();
        let decision = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.5,
        );
        assert!(decision.winner.is_none());
        assert!(matches!(
            decision.evaluations[0].outcome,
            RouteOutcome::ProviderUnavailable(_)
        ));
    }

    /// Priority is the only cross-band ordering — a huge weight on a
    /// lower-priority route must never beat a smaller-weight
    /// higher-priority one.
    #[test]
    fn higher_priority_always_beats_higher_weight() {
        let low_priority_huge_weight = route("low", 0, 1000, "p1");
        let high_priority_tiny_weight = route("high", 10, 1, "p1");
        let providers = HashMap::from([available_provider("p1")]);
        let decision = select_route(
            &[low_priority_huge_weight, high_priority_tiny_weight],
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.999,
        );
        assert_eq!(decision.winner.unwrap().route_id, "high");
    }

    #[test]
    fn a_single_member_band_has_no_tie_break_and_wins_regardless_of_draw() {
        let routes = vec![route("only", 0, 5, "p1")];
        let providers = HashMap::from([available_provider("p1")]);
        for draw in [0.0, 0.5, 0.999] {
            let decision = select_route(
                &routes,
                &providers,
                &candidate(),
                &ExcludedRouteIds::new(),
                draw,
            );
            assert_eq!(decision.winner.as_ref().unwrap().route_id, "only");
            assert!(decision.tie_break.is_none());
        }
    }

    /// Weight 1 vs weight 3 splits `[0.0, 1.0)` into `[0, 0.25)` and
    /// `[0.25, 1.0)` — exact boundaries, checked directly, so a future
    /// change to the draw formula fails loudly here rather than only in a
    /// statistical test.
    #[test]
    fn a_weighted_draw_lands_on_the_expected_route_at_exact_boundaries() {
        let routes = vec![route("light", 0, 1, "p1"), route("heavy", 0, 3, "p1")];
        let providers = HashMap::from([available_provider("p1")]);

        let just_below = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.24,
        );
        assert_eq!(just_below.winner.unwrap().route_id, "light");

        let at_boundary = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.25,
        );
        assert_eq!(at_boundary.winner.unwrap().route_id, "heavy");

        let near_one = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.999,
        );
        assert_eq!(near_one.winner.unwrap().route_id, "heavy");

        let tie_break = at_boundary.tie_break.expect("a two-member band ties");
        assert_eq!(tie_break.winner_route_id, "heavy");
        assert_eq!(tie_break.ranges.len(), 2);
    }

    #[test]
    fn an_all_zero_weight_band_falls_back_to_a_uniform_draw() {
        let routes = vec![route("a", 0, 0, "p1"), route("b", 0, 0, "p1")];
        let providers = HashMap::from([available_provider("p1")]);

        let first_half = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.1,
        );
        assert_eq!(first_half.winner.unwrap().route_id, "a");

        let second_half = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.9,
        );
        assert_eq!(second_half.winner.unwrap().route_id, "b");
    }

    #[test]
    fn the_same_inputs_and_draw_always_produce_the_same_decision() {
        let routes = vec![route("a", 0, 1, "p1"), route("b", 0, 1, "p1")];
        let providers = HashMap::from([available_provider("p1")]);
        let one = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.3,
        );
        let two = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.3,
        );
        assert_eq!(one, two);
    }

    /// #63's own mechanism: excluding the would-be winner falls through to
    /// the next eligible route rather than reporting "no winner" outright.
    #[test]
    fn excluding_the_winner_falls_through_to_the_next_eligible_route() {
        let routes = vec![route("a", 10, 1, "p1"), route("b", 0, 1, "p1")];
        let providers = HashMap::from([available_provider("p1")]);
        let first = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            0.5,
        );
        assert_eq!(first.winner.as_ref().unwrap().route_id, "a");

        let mut exclude = ExcludedRouteIds::new();
        exclude.insert("a".to_owned());
        let second = select_route(&routes, &providers, &candidate(), &exclude, 0.5);
        assert_eq!(second.winner.unwrap().route_id, "b");
        assert_eq!(second.evaluations[0].outcome, RouteOutcome::Excluded);
    }

    /// A single-member band never actually consumes `draw` (see
    /// [`a_single_member_band_has_no_tie_break_and_wins_regardless_of_draw`]),
    /// so this only proves "does not panic" for an out-of-range or `NaN`
    /// draw — the real "clamped, not panicking, and still produces a real
    /// tie-break range" claim is covered by a two-member band below.
    #[test]
    fn an_out_of_range_or_nan_draw_never_panics() {
        let routes = vec![route("only", 0, 1, "p1")];
        let providers = HashMap::from([available_provider("p1")]);
        for draw in [-5.0, 1.0, 100.0, f64::NAN] {
            let decision = select_route(
                &routes,
                &providers,
                &candidate(),
                &ExcludedRouteIds::new(),
                draw,
            );
            assert_eq!(decision.winner.unwrap().route_id, "only");
        }
    }

    /// The two-member case an out-of-range draw actually exercises the
    /// clamp against: `1.0` and `100.0` must both behave like "as close to
    /// certain as possible" (the highest-weighted / last member), not
    /// panic on an out-of-bounds range lookup, and `-5.0` must behave like
    /// `0.0` (the first member), not underflow anything.
    #[test]
    fn an_out_of_range_draw_clamps_into_the_valid_band_for_a_real_tie() {
        let routes = vec![route("light", 0, 1, "p1"), route("heavy", 0, 3, "p1")];
        let providers = HashMap::from([available_provider("p1")]);

        let below = select_route(
            &routes,
            &providers,
            &candidate(),
            &ExcludedRouteIds::new(),
            -5.0,
        );
        assert_eq!(below.winner.unwrap().route_id, "light");

        for draw in [1.0, 100.0] {
            let above = select_route(
                &routes,
                &providers,
                &candidate(),
                &ExcludedRouteIds::new(),
                draw,
            );
            assert_eq!(above.winner.unwrap().route_id, "heavy");
        }
    }
}
