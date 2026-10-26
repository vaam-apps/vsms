//! `vsms-sdk-rust` - a Rust client for vsms that signs every request with an
//! Ed25519 key, so an integrator writes `client.send_message(...)` and never
//! touches a token. See <https://github.com/vymalo/vsms/issues/171> for the
//! original brief.
//!
//! # The only credential is a key
//!
//! Since vsms 0.10 there are no access tokens, refresh tokens, OAuth grants or
//! shared secrets anywhere between this crate and the gateway (owner's ruling:
//! *"keypairs everywhere"*). A request is a COSE_Sign1 message
//! ([`cratestack-cose`](https://docs.rs/cratestack-cose), ADR 0006) signed by a
//! [`SigningKey`], bound to the route, the audience [`AUDIENCE`], the op's wire
//! shape and a fresh nonce; the gateway answers with a message signed by its own
//! key, which this crate verifies against the [`ServerKeys`] it was given:
//!
//! 1. generate a key (`SigningKey::generate()`, or `openssl genpkey -algorithm
//!    ed25519`), keep the private half, and register the public half
//!    (`SigningKey::public_key_b64u`) with an operator:
//!    `sms-gateway register-client --app <slug> --public-key <b64url> --scope sms:send`;
//! 2. pin the gateway's two public keys from `sms-gateway server-keys`;
//! 3. build a [`VsmsClient`] and call it.
//!
//! The private key never reaches vsms (it stores only the public half), so
//! there is no "returned once" secret to lose, and nothing to refresh.
//!
//! # What is generated, and what is hand-written
//!
//! The typed model/input/procedure surface below (`schema` - `Message`,
//! `SendMessageInput`, `procedures::send_message`, ...) is **not**
//! hand-written. It comes from `cratestack::include_client_schema!`, the
//! HTTP-client sibling of `include_server_schema!` (`backends/crates/sms-api`
//! in the main vsms repo uses that one), and with the `cose` feature its
//! generated `Client` seals every call it makes. What is hand-written is small:
//! [`SigningKey`] and [`ServerKeys`] (the two inputs), and [`VsmsClient`], whose
//! one extra method, `send_message`, seals its own call
//! (`ClientEnvelope::seal_call`) because it needs the response headers the typed
//! wrapper discards (`Idempotency-Replayed`).
//!
//! # Why the schema is vendored into this crate, not read from `../../schema/`
//!
//! `include_client_schema!` resolves its path argument against
//! `CARGO_MANIFEST_DIR` **of the crate that invokes it, at that crate's own
//! compile time** - not against this repository's layout. This crate is
//! meant to be published to crates.io and built from a downstream
//! integrator's own `~/.cargo/registry/src/.../vsms-sdk-rust-*/` checkout,
//! where nothing above that directory exists. So `schema.cstack` lives inside
//! this crate's own directory - a plain vendored copy of
//! `schemas/vsms.cstack`, refreshed by `cargo xtask sdk-schema-vendor` and
//! held to the original by `cargo xtask sdk-schema-check` (the main repo's
//! `rules` CI job). The copy matters twice now: the op-contract digests a
//! request binds are computed from it, so a client built from a stale copy is
//! refused with `426` for exactly the ops that changed.
//!
//! # Measured build cost
//!
//! `include_client_schema!` emits no DB layer, no router, no `FromRow` impls, no
//! policy evaluator, no audit/event plumbing - only model/input/procedure
//! *stubs* - so it is a materially smaller expansion than the server's
//! `include_server_schema!`, which is memory- and time-hungry (see the main
//! repo's `AGENTS.md`).
#![allow(clippy::ptr_arg)]
#![allow(missing_docs)]

// `decimal = RustDecimal` required as of the cratestack 0.8.3 bump — see
// `sms-api`'s own `lib.rs` for the full reasoning (cratestack#609 made the
// decimal backends additive, so the macro can no longer infer one). Must
// stay in step with the server side's choice: this crate's generated
// client types are the wire mirror of that schema.
cratestack::include_client_schema!("schema.cstack", decimal = RustDecimal);

/// Shorter alias for the generated module, matching `sms-api`'s own
/// `pub use crate::cratestack_schema as schema;` convention so the two
/// generated surfaces (server, client) read the same way side by side.
pub use crate::cratestack_schema as schema;

mod client;
mod error;
mod server_keys;
mod signer;

pub use client::{SendMessageOutcome, VsmsClient, VsmsClientBuilder, AUDIENCE};
pub use error::SdkError;
pub use server_keys::ServerKeys;
pub use signer::SigningKey;
