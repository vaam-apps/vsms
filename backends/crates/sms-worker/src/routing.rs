#![doc = include_str!("routing.md")]

use std::collections::HashMap;

use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_api::schema::{
    self, Cratestack, MessageClass as SchemaMessageClass, OperatorCode as SchemaOperatorCode,
    provider, route,
};
use sms_msisdn::Msisdn;
use sms_routing::{Decision, ExcludedRouteIds, MessageClass, Operator, ProviderRow, RouteRow};
use tracing::warn;

/// The columns of a candidate `Message` a routing decision actually reads
/// — `operator`/`class`/`appId`/`msisdn`, §6.3's own four predicates plus
/// what `matchPrefix` needs. A small owned-reference struct rather than
/// passing `&schema::Message` directly, so this module doesn't need to
/// know the rest of that type's shape.
pub struct Candidate<'a> {
    /// `Message.operator`, classified at send time.
    pub operator: SchemaOperatorCode,
    /// `Message.class`.
    pub class: SchemaMessageClass,
    /// `Message.appId`.
    pub app_id: &'a str,
    /// `Message.msisdn` — E.164, parsed here via `sms_msisdn::Msisdn` to
    /// get the national digits `matchPrefix` predicates compare against.
    /// Never logged (it's `@pii`).
    pub msisdn: &'a str,
    /// For the one warning this module can log — never sent to
    /// `sms_routing`, and never logged itself (it's `@pii`).
    pub message_id: &'a str,
}

fn convert_operator(value: SchemaOperatorCode) -> Operator {
    match value {
        SchemaOperatorCode::mtn => Operator::Mtn,
        SchemaOperatorCode::orange => Operator::Orange,
        SchemaOperatorCode::camtel => Operator::Camtel,
        SchemaOperatorCode::nexttel => Operator::Nexttel,
        SchemaOperatorCode::unknown => Operator::Unknown,
    }
}

fn convert_class(value: SchemaMessageClass) -> MessageClass {
    match value {
        SchemaMessageClass::otp => MessageClass::Otp,
        SchemaMessageClass::transactional => MessageClass::Transactional,
        SchemaMessageClass::notification => MessageClass::Notification,
        SchemaMessageClass::marketing => MessageClass::Marketing,
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

/// `Provider.healthy` is never set to `true` anywhere in this codebase —
/// its only would-be writer, §7.5's `probe_providers` job, is explicitly
/// out of scope (not one of #35's built job kinds). Gating availability on
/// it would make every provider permanently unavailable and every message
/// reject, which is strictly worse than the M2 placeholder this replaces.
/// So availability stays exactly `cheapest_active_provider`'s own check —
/// `state == active` — plus one addition, #63: `state == active` alone is
/// no longer sufficient once a provider's circuit breaker can be open.
///
/// This is the "must not fail a message a healthy alternative could carry"
/// half of #63's own acceptance criterion, and it is what makes that
/// property hold for free, for *every* future `accepted` message — not
/// just the one whose failed submit tripped the breaker — because every
/// fresh routing decision re-reads `Provider` and re-runs this check.
/// `backends/crates/sms-worker/src/dispatch.rs`'s `record_provider_failure`/
/// `reset_provider_failures` are the only writers of `consecutiveFailures`/
/// `circuitOpenUntil`; this is their one reader. Same shape
/// `claim.rs::filter_by_endpoint_health` already uses for
/// `WebhookEndpoint.circuitOpenUntil` — a breaker with different semantics
/// here would be a second thing to remember, not a better one.
fn convert_provider(row: &schema::Provider, now: chrono::DateTime<chrono::Utc>) -> ProviderRow {
    let circuit_open = row.circuitOpenUntil.is_some_and(|until| until > now);
    let available = row.state == schema::ProviderState::active && !circuit_open;
    let reason = if circuit_open {
        Some("circuit breaker open after consecutive Unavailable failures".to_owned())
    } else if !available {
        Some(format!("provider state is {:?}, not active", row.state))
    } else {
        None
    };
    ProviderRow {
        id: row.id.clone(),
        available,
        reason,
    }
}

/// Fetch every `Route` row (deterministically ordered — `priority` desc
/// then `id` asc — so a replay with the same `draw` reproduces the same
/// winner, per [`sms_routing::select_route`]'s own doc on why input order
/// matters) plus the `Provider` rows they reference, convert both onto
/// `sms_routing`'s pure types, draw the one random `f64` production needs,
/// and hand everything to [`sms_routing::select_route`].
///
/// This function is the entire I/O boundary — `sms_routing` itself never
/// touches a database, a clock, or an RNG. `exclude` is threaded straight
/// through to the pure engine — #63's own mechanism
/// (`backends/crates/sms-worker/src/dispatch.rs::attempt_failover`) is the one
/// caller that ever populates it with more than an empty set, by adding
/// the route(s) a message has already been rerouted away from.
pub async fn decide(
    db: &Cratestack,
    sys: &CoolContext,
    candidate: &Candidate<'_>,
    exclude: &ExcludedRouteIds,
) -> Result<Decision, CoolError> {
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

    let now = chrono::Utc::now();
    let providers: HashMap<String, ProviderRow> = if provider_ids.is_empty() {
        HashMap::new()
    } else {
        db.provider()
            .find_many()
            .where_expr(FilterExpr::from(provider::id().in_(provider_ids)))
            .run(sys)
            .await?
            .iter()
            .map(|row| (row.id.clone(), convert_provider(row, now)))
            .collect()
    };

    let route_rows: Vec<RouteRow> = routes.iter().map(convert_route).collect();

    // `Message.msisdn` was already normalised and validated by
    // sendMessage's own `Msisdn::parse_mobile` call before the row ever
    // existed (§3.2's pre-persistence steps), so a parse failure here is a
    // genuine anomaly, not an expected case — fail closed (no prefix
    // predicate can ever match an empty national number) and log without
    // the raw value, which is `@pii`.
    let parsed = Msisdn::parse(candidate.msisdn);
    let national_owned;
    let msisdn_national: &str = match &parsed {
        Ok(msisdn) => msisdn.national(),
        Err(error) => {
            warn!(
                message_id = candidate.message_id,
                %error,
                "routing candidate's msisdn failed to parse; matchPrefix predicates will not match"
            );
            national_owned = String::new();
            &national_owned
        }
    };

    let routing_candidate = sms_routing::RoutingCandidate {
        operator: convert_operator(candidate.operator),
        class: convert_class(candidate.class),
        app_id: candidate.app_id,
        msisdn_national,
    };

    Ok(sms_routing::select_route(
        &route_rows,
        &providers,
        &routing_candidate,
        exclude,
        rand::random(),
    ))
}

/// Renders a [`Decision`] with no winner into a `Message.stateReason` an
/// operator can read without reaching for #54's simulator. Summary-shaped
/// — the full `Decision` (every predicate failure, the tie-break, if any)
/// is only ever reconstructed by calling [`decide`] again with the same
/// inputs, per this module's own explainability design; `stateReason` is
/// an operational note, not a full audit record.
#[must_use]
pub fn explain_no_route(decision: &Decision) -> String {
    if decision.evaluations.is_empty() {
        return "no eligible route: no Route rows are configured".to_owned();
    }
    let reasons: Vec<String> = decision
        .evaluations
        .iter()
        .map(|evaluation| format!("{} ({:?})", evaluation.route_name, evaluation.outcome))
        .collect();
    format!(
        "no eligible route: {} route(s) evaluated, 0 eligible — {}",
        decision.evaluations.len(),
        reasons.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::{convert_class, convert_operator, explain_no_route};
    use sms_api::schema::{MessageClass as SchemaMessageClass, OperatorCode as SchemaOperatorCode};
    use sms_routing::{Decision, MessageClass, Operator};

    #[test]
    fn every_schema_operator_variant_converts() {
        assert_eq!(convert_operator(SchemaOperatorCode::mtn), Operator::Mtn);
        assert_eq!(
            convert_operator(SchemaOperatorCode::orange),
            Operator::Orange
        );
        assert_eq!(
            convert_operator(SchemaOperatorCode::camtel),
            Operator::Camtel
        );
        assert_eq!(
            convert_operator(SchemaOperatorCode::nexttel),
            Operator::Nexttel
        );
        assert_eq!(
            convert_operator(SchemaOperatorCode::unknown),
            Operator::Unknown
        );
    }

    #[test]
    fn every_schema_message_class_variant_converts() {
        assert_eq!(convert_class(SchemaMessageClass::otp), MessageClass::Otp);
        assert_eq!(
            convert_class(SchemaMessageClass::transactional),
            MessageClass::Transactional
        );
        assert_eq!(
            convert_class(SchemaMessageClass::notification),
            MessageClass::Notification
        );
        assert_eq!(
            convert_class(SchemaMessageClass::marketing),
            MessageClass::Marketing
        );
    }

    #[test]
    fn explaining_zero_configured_routes_names_that_specifically() {
        let decision = Decision {
            evaluations: vec![],
            tie_break: None,
            winner: None,
        };
        assert_eq!(
            explain_no_route(&decision),
            "no eligible route: no Route rows are configured"
        );
    }
}
