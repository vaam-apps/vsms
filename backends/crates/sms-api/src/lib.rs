#![doc = include_str!("lib.md")]
#![allow(clippy::ptr_arg)]
// The generated module is not ours to document.
#![allow(missing_docs)]

cratestack::include_server_schema!("../../../schemas/vsms.cstack", db = Postgres);

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
