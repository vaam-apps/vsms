//! `vsms-sdk-rust` — a Rust client for vsms that owns the `private_key_jwt`
//! credential lifecycle, so an integrator writes `client.send_message(...)`
//! and never touches a JWT. See
//! <https://github.com/vymalo/vsms/issues/171> for the full brief.
//!
//! # What is generated, and what is hand-written
//!
//! The typed model/input/procedure surface below (`schema` — `Message`,
//! `SendMessageInput`, `procedures::send_message`, ...) is **not**
//! hand-written. It comes from `cratestack::include_client_schema!`, the
//! HTTP-client sibling of `include_server_schema!` (`crates/sms-api` in the
//! main vsms repo uses that one). An earlier version of this crate's brief
//! said "there is no Rust generator" and proposed hand-writing this surface
//! — that was wrong. There is no `cratestack generate-rust` **CLI
//! subcommand**, which is why `cratestack --help` shows nothing, but the
//! generator exists as a proc macro in `cratestack-macros`, and this crate
//! uses it exactly the way `sms-api` uses its server-side counterpart.
//!
//! What genuinely has no generator, and is hand-written here, is the **auth
//! layer**: [`token`]'s `TokenStore`/`PrivateKeyJwtTokenStore` (the
//! `private_key_jwt` credential lifecycle — assertion signing, `jti`
//! uniqueness, expiry-margin caching) and [`authorizer`]'s
//! `GatewayAuthorizer` (the `cratestack::client_rust::RequestAuthorizer`
//! impl that plugs the token store into the generated client's request
//! pipeline). That was always the valuable, error-prone part; it is now
//! the *only* part this crate writes by hand. [`client`] is a thin
//! convenience wrapper around the generated `schema::client::Client` that
//! adds the bounded-refresh-on-401 behaviour issue #171 requires — nothing
//! in it hand-duplicates a generated type.
//!
//! # Why the schema is vendored into this crate, not read from `../../schema/`
//!
//! `include_client_schema!` resolves its path argument against
//! `CARGO_MANIFEST_DIR` **of the crate that invokes it, at that crate's own
//! compile time** (see `cratestack-macros`' `include::parse::
//! parse_schema_literal`) — not against this repository's layout. Inside
//! this monorepo that would resolve fine with a `../../../schema/
//! schema.cstack` climb, exactly like `sms-api` does with its own `../../`.
//! But this crate is meant to be published to crates.io and built from a
//! downstream integrator's own `~/.cargo/registry/src/.../vsms-sdk-rust-*/`
//! checkout, where nothing above that directory exists. So `schema.cstack`
//! lives inside this crate's own directory — a plain vendored copy of
//! `schema/schema.cstack`, refreshed by `cargo xtask sdk-schema-vendor`
//! (`sdks/rust/vsms-sdk-rust/vendor-schema.sh` before the maintainer's
//! "no bash scripts" cutover) and committed in the same change as any
//! schema edit this SDK's surface should track. **Correction: this module
//! doc used to say there was no drift-detection gate over that copy —
//! wrong even before the sentence above, and stale regardless of it.**
//! `cargo xtask sdk-schema-check` (`ci/assert-sdk-schema-current.sh`
//! before the same cutover) has been the main repo's `rules` CI job's own
//! gate for this since before this correction was written; the true gap
//! is narrower than the original sentence claimed — that gate is real and
//! wired into CI, it just doesn't run as part of *this* crate's own
//! `cargo test`, the way `packages/sms-client`'s `client-check` is wired
//! into the `js` CI job.
//!
//! # Measured build cost
//!
//! `AGENTS.md` is emphatic that expanding `schema.cstack` through
//! `include_server_schema!` is memory- and time-hungry — outright
//! impossible on a 32 GB machine below cratestack 0.5.0. `include_client_
//! schema!` emits no DB layer, no router, no `FromRow` impls, no policy
//! evaluator, no audit/event plumbing — only model/input/procedure
//! *stubs* — so it is a materially smaller expansion, and that is borne
//! out, measured on this same machine: a clean `cargo build -p
//! vsms-sdk-rust` from an empty `target/` (all crates.io dependencies
//! already downloaded, nothing compiled) finished in **19.3s wall /
//! ~90.7s total CPU across cores / ~665MB peak RSS** for the whole build,
//! and with dependencies already built, checking just this crate's own
//! `include_client_schema!` expansion took **4.5s / ~83MB peak**. Neither
//! comes close to the multi-GB, multi-minute figures `include_server_
//! schema!` produced pre-0.5.0.
//!
//! # Two workarounds this crate no longer needs, as of cratestack 0.7.10
//!
//! Through cratestack 0.7.8, `include_client_schema!` was only reachable
//! through `cratestack-pg` (aliased `cratestack` — generated code emits
//! absolute `::cratestack::*` paths, so the rename is mandatory), which
//! unconditionally depended on `cratestack-axum` — a full axum server
//! framework — regardless of its own `postgres` feature flag (`pub use
//! cratestack_axum::*` in its `lib.rs` was not feature-gated). Filed as
//! [cratestack#490](https://github.com/cratestack/cratestack/issues/490)
//! and fixed in
//! [cratestack#492](https://github.com/cratestack/cratestack/pull/492):
//! `cratestack-client`, a fourth, client-only facade that re-exports
//! `include_client_schema!` and the generated Rust client runtime with
//! `cratestack-axum` structurally absent from its dependency graph. This
//! crate now depends on `cratestack-client` instead of `cratestack-pg` —
//! confirmed via `cargo tree`: `axum`/`tower`/`hyper`/`cratestack-axum` are
//! gone from this crate's own dependency tree entirely, not merely
//! feature-gated off.
//!
//! Separately — also found live while building this crate against 0.7.8,
//! not by inspection — the generated client's default `Accept` header made
//! a real, JSON-only `sms-gateway` return `406 Not Acceptable`
//! (`cratestack::client_rust::JsonCodec::accept_header_value()`
//! unconditionally advertises `application/cbor` too, and the server used
//! to pick it over `application/json` despite never having a CBOR encoder
//! registered). Filed as
//! [cratestack#489](https://github.com/cratestack/cratestack/issues/489)
//! and fixed in
//! [cratestack#491](https://github.com/cratestack/cratestack/pull/491):
//! response content-type negotiation is now codec-aware, filtering
//! candidates through what the router's actual transport can genuinely
//! encode. [`client`] no longer overrides `Accept` on any call — removed
//! along with the `JSON_ONLY_ACCEPT` const that used to force it.
#![allow(clippy::ptr_arg)]
#![allow(missing_docs)]

cratestack::include_client_schema!("schema.cstack");

/// Shorter alias for the generated module, matching `sms-api`'s own
/// `pub use crate::cratestack_schema as schema;` convention so the two
/// generated surfaces (server, client) read the same way side by side.
pub use crate::cratestack_schema as schema;

mod authorizer;
mod client;
mod error;
mod token;

pub use authorizer::GatewayAuthorizer;
pub use client::{SendMessageOutcome, VsmsClient, VsmsClientBuilder};
pub use error::SdkError;
pub use token::{PrivateKeyJwtConfig, PrivateKeyJwtTokenStore, TokenAudience, TokenStore};
