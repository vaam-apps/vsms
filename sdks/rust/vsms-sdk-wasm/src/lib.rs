//! The signed-transport half of the vsms SDK, for Node.js and the browser.
//!
//! `@vaam-apps/vsms-node` is TypeScript, and a COSE_Sign1 envelope is not
//! something to reimplement beside the Rust one: the binding bytes (audience,
//! method, route template, path parameters, canonical query, the operation's
//! contract digest, payload type, `Idempotency-Key`) are the whole security of
//! the transport, and two implementations of them would eventually disagree
//! about one byte. So the Node SDK does not seal anything itself. It calls
//! into this crate, compiled to WebAssembly (`just wasm-build`), which is
//! `vsms-sdk-rust`'s own sealing path over cratestack's own `ClientEnvelope`:
//!
//! ```text
//! Node (http)  -- sealCall(method, route, params, body, query, key) --> wasm
//!              <-- { body, headers, target, openResponse } ------------
//! Node (http)  -- send the sealed bytes to `target` ---------------> gateway
//!              <-- sealed answer ------------------------------------
//! Node         -- openResponse(status, headers, bytes) -------------> wasm
//!              <-- the JSON text, verified --------------------------
//! ```
//!
//! The HTTP stays in Node: the caller's agent, timeouts, proxies. The key
//! either goes in as a seed (this crate signs), or stays outside behind a
//! callback (`crypto.sign(null, tbs, key)` in Node, a KMS, a WebCrypto
//! non-extractable key in the browser), in which case this crate only ever
//! sees the bytes to sign and the signature, and checks the signature against
//! the registered public key before using it.
//!
//! Two algorithms, because the gateway holds two kinds of key: Ed25519 for an
//! app client (`Sealer.fromSeed`, `Sealer.fromCallback`), and ESP256 (P-256)
//! for the operator console's paired browser key (`Sealer.fromEsp256Callback`).
//!
//! The operation table is the vendored schema's `OP_CONTRACTS`, expanded by
//! `include_client_schema!` over the same file the Rust SDK uses, so the two
//! SDKs cannot hold two tables.

mod contracts;
mod core;

// The JavaScript surface exists only where there is a JavaScript to bind to.
// On a host the crate is `core` alone, which is what `cargo test` compiles and
// runs, so the sealing path has native tests and the crate builds on a
// machine with no wasm toolchain.
#[cfg(target_arch = "wasm32")]
mod js;

#[cfg(any(test, feature = "responder"))]
pub mod responder;

#[cfg(all(target_arch = "wasm32", feature = "responder"))]
mod responder_js;

#[cfg(test)]
mod tests;

pub use crate::contracts::{contract_digest, operations};
pub use crate::core::{
    Algorithm, OpenError, SealError, Sealed, Sealer, SealerKey, ServerKeys, AUDIENCE,
};
