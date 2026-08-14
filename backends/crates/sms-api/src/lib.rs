//! The generated `CrateStack` surface for the SMS gateway.
//!
//! This crate hosts `include_server_schema!` and the hand-written pieces that
//! plug into it: the [`AuthProvider`](cratestack::AuthProvider) that turns a
//! request into a policy-evaluable identity, and the
//! [`ProcedureRegistry`](schema::procedures::ProcedureRegistry) implementation
//! behind the seven procedures the schema declares.
//!
//! # Why the schema lives outside this crate
//!
//! `include_server_schema!` resolves its path against `CARGO_MANIFEST_DIR`, so
//! the conventional layout is to keep the `.cstack` file inside the crate that
//! expands it. Here it stays at the repository root next to
//! `backends/migrations/`, because three other things already read it — the
//! migration diff, `cargo xtask bootstrap-sql`, and (soon) `sms-worker` — and
//! splitting the schema from its own migrations to satisfy a macro's default
//! path resolution is the wrong trade. Hence the `../../` in the path below.
//!
//! # `clippy::ptr_arg`
//!
//! Generated code inside `cratestack_schema` takes `&String` in places clippy
//! flags under `-D warnings`. It is generated, not ours to fix, and an outer
//! `#[allow]` on the macro invocation does not work — the macro expands to many
//! items, so clippy reports the attribute as unused. A crate-level inner
//! attribute is the only reliable suppression, and this crate exists to host
//! the generated module, so the blast radius is exactly right.
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

pub use auth::{GatewayAuth, Principal, PrincipalKind, DEFAULT_CONSOLE_CLIENT_ID};
pub use errors::{is_illegal_transition, map_database_error};
pub use pepper::{hmac_sha256_hex, HashPepper, PepperError};
pub use procedures::Procedures;
pub use rbac::require_permission;
pub use router::{
    default_rate_limit_config, default_source_rate_limit_config, route_table, router,
    DEFAULT_IDEMPOTENCY_TTL,
};
pub use webhooks::register_subscribers;
