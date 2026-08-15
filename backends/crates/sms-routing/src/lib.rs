#![doc = include_str!("lib.md")]

mod engine;
mod types;

pub use engine::select_route;
pub use types::{
    Decision, ExcludedRouteIds, MessageClass, Operator, PredicateFailure, ProviderRow,
    RouteEvaluation, RouteOutcome, RouteRow, RoutingCandidate, TieBreak, TieBreakRange, Winner,
};
