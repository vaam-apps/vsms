//! The operations a signature can be bound to, and the digest each one binds.
//!
//! The table is `OP_CONTRACTS`, which `include_client_schema!` computes from the
//! schema `vsms-sdk-rust` vendors (`sdks/rust/vsms-sdk-rust/schema.cstack`, held
//! to `schemas/vsms.cstack` by `cargo xtask sdk-schema-check`). It is the same
//! macro, over the same file, as the Rust SDK's generated client, so the two
//! SDKs cannot bind different bytes for one operation. Nothing else in this
//! crate knows a route.

use crate::core::SealError;

// The macro emits the whole typed client beside the table; none of it is used
// here, and it is written for a lint set this crate's hand-written code is held
// to and it is not.
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    dead_code,
    missing_docs,
    missing_debug_implementations,
    unreachable_pub
)]
mod schema {
    cratestack::include_client_schema!("../vsms-sdk-rust/schema.cstack", decimal = RustDecimal);
}

/// The digest `method template` binds, or [`SealError::Unbound`] when the
/// schema serves no such operation. `template` is the schema's own route
/// (`/messages/{id}`, `/$procs/sendMessage`), never a concrete path.
///
/// ```
/// # use vsms_sdk_wasm::contract_digest;
/// assert!(contract_digest("POST", "/$procs/sendMessage").is_ok());
/// assert!(contract_digest("GET", "/messages/{id}").is_ok());
/// // A concrete path is not a template.
/// assert!(contract_digest("GET", "/messages/m1").is_err());
/// // Neither is a table key: the method is not part of the route.
/// assert!(contract_digest("GET", "GET /messages/{id}").is_err());
/// ```
///
/// # Errors
///
/// [`SealError::Unbound`] naming the method and the template.
pub fn contract_digest(method: &str, template: &str) -> Result<[u8; 32], SealError> {
    // `find_contract` also matches a key that *is* the route, which is how an
    // RPC op id is found; a REST template starts with `/`, so requiring that
    // keeps `"GET /messages/{id}"` from being read as a route of its own.
    if !template.starts_with('/') {
        return Err(unbound(method, template));
    }
    cratestack::find_contract(schema::cratestack_schema::OP_CONTRACTS, method, template)
        .copied()
        .ok_or_else(|| unbound(method, template))
}

fn unbound(method: &str, template: &str) -> SealError {
    SealError::Unbound(format!(
        "the schema serves no operation `{method} {template}`"
    ))
}

/// Every REST operation as `(method, template)`, for the responder's path
/// matching and for the tests that walk the table.
#[must_use]
pub fn operations() -> Vec<(&'static str, &'static str)> {
    schema::cratestack_schema::OP_CONTRACTS
        .iter()
        .filter_map(|(key, _)| key.split_once(' '))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_table_holds_the_three_operations_the_node_sdk_calls() {
        for (method, template) in [
            ("POST", "/$procs/sendMessage"),
            ("POST", "/$procs/previewMessage"),
            ("GET", "/messages/{id}"),
        ] {
            assert!(
                operations().contains(&(method, template)),
                "{method} {template}"
            );
        }
    }

    #[test]
    fn a_digest_is_per_operation() {
        let send = contract_digest("POST", "/$procs/sendMessage").unwrap();
        let preview = contract_digest("POST", "/$procs/previewMessage").unwrap();
        assert_ne!(send, preview);
    }

    #[test]
    fn a_method_the_template_is_not_served_under_is_unbound() {
        let error = contract_digest("DELETE", "/$procs/sendMessage").unwrap_err();
        assert!(matches!(error, SealError::Unbound(_)), "{error:?}");
    }
}
