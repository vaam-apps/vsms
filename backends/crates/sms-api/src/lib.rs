#![doc = include_str!("lib.md")]
#![allow(clippy::ptr_arg)]
// The generated module is not ours to document.
#![allow(missing_docs)]

// `decimal = RustDecimal` is required, not decorative, and is new as of the
// cratestack 0.8.3 bump. cratestack#609 made `decimal-rust-decimal` and
// `decimal-bigdecimal` additive rather than mutually exclusive — they used
// to be enforced apart by a `compile_error!`, which meant two independent
// crates in one dependency graph, each making a legitimate backend choice,
// produced a build neither author could fix. The cost of fixing that is
// that the backend can no longer be inferred from features alone, so any
// schema declaring a `Decimal` field must name one here. This schema has
// three (`Provider.costPerSegmentXaf`, `Message.costXaf`,
// `SendMessageResult.estimatedCostXaf`). `RustDecimal` matches what
// `cratestack-pg`'s own `default` feature set already selected, so this
// declares what was already true — no money type in this tree changed
// shape. Omitting it is a compile error, not a silent fallback.
cratestack::include_server_schema!(
    "../../../schemas/vsms.cstack",
    db = Postgres,
    decimal = RustDecimal
);

/// Shorter alias for the generated module, so consumers write
/// `sms_api::schema::{Message, Cratestack, ...}`.
pub use crate::cratestack_schema as schema;

pub mod audit_log;
pub mod auth;
mod cache;
pub mod consent;
pub mod dlr;
pub mod errors;
pub mod metrics;
pub mod pepper;
pub mod procedures;
pub mod rbac;
pub mod route_simulator;
pub mod router;
pub mod webhooks;
pub mod worker_locks;
pub mod worker_roles;

pub use auth::{DEFAULT_CONSOLE_CLIENT_ID, GatewayAuth, Principal, PrincipalKind};
pub use errors::{is_illegal_transition, map_database_error};
pub use pepper::{HashPepper, PepperError, hmac_sha256_hex};
pub use procedures::Procedures;
pub use rbac::require_permission;
pub use router::{
    DEFAULT_IDEMPOTENCY_TTL, default_rate_limit_config, default_source_rate_limit_config,
    route_table, router,
};
pub use webhooks::register_subscribers;
