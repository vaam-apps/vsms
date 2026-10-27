//! The operations a signature can be bound to, and the digest each one binds.
//!
//! The table is `OP_CONTRACTS`, which `include_client_schema!` computes from the
//! schema `vsms-sdk-rust` vendors (`sdks/rust/vsms-sdk-rust/schema.cstack`, held
//! to `schemas/vsms.cstack` by `cargo xtask sdk-schema-check`). It is the same
//! macro, over the same file, as the Rust SDK's generated client, so the two
//! SDKs cannot bind different bytes for one operation. Nothing else in this
//! crate knows a route.

use percent_encoding::percent_decode_str;

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

/// The template and decoded parameters of the operation `method path` is,
/// found by matching `path` (no query) segment by segment against each route
/// the schema serves under `method`.
///
/// This is how a caller that holds a concrete path (the operator console's
/// `fetch`-shaped data layer) names the route a signature binds, without
/// parsing it back itself: the template and parameters returned here are what
/// `Sealer.sealCall` takes, and its `target` is rebuilt from them, so a
/// mismatch between the path asked for and the path signed is caught by
/// comparing the two. When two templates match (a literal segment and a
/// parameter in the same place), the one with fewer parameters wins.
///
/// ```
/// # use vsms_sdk_wasm::route_of;
/// let (template, params) = route_of("GET", "/messages/m%2F1").unwrap();
/// assert_eq!(template, "/messages/{id}");
/// assert_eq!(params, ["m/1"]);
/// assert_eq!(route_of("POST", "/$procs/sendMessage").unwrap().1, Vec::<String>::new());
/// // Nothing the schema does not serve, and no `..` that a parser would resolve.
/// assert!(route_of("GET", "/nope").is_none());
/// assert!(route_of("GET", "/messages/..").is_none());
/// ```
#[must_use]
pub fn route_of(method: &str, path: &str) -> Option<(String, Vec<String>)> {
    let segments: Vec<&str> = path.split('/').collect();
    operations()
        .into_iter()
        .filter(|(m, _)| *m == method)
        .filter_map(|(_, template)| {
            let parts: Vec<&str> = template.split('/').collect();
            if parts.len() != segments.len() {
                return None;
            }
            let mut params = Vec::new();
            for (part, segment) in parts.iter().zip(&segments) {
                if part.starts_with('{') && part.ends_with('}') {
                    let decoded = percent_decode_str(segment).decode_utf8().ok()?.into_owned();
                    if decoded.is_empty() || decoded == "." || decoded == ".." {
                        return None;
                    }
                    params.push(decoded);
                } else if part != segment {
                    return None;
                }
            }
            Some((template.to_owned(), params))
        })
        .min_by_key(|(_, params)| params.len())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_route_without_parameters_is_found_as_itself() {
        // No two templates collide in today's schema, so the "fewer parameters
        // wins" rule has nothing to decide yet; it is what keeps a future literal
        // sibling (`/messages/export`) from being read as an id.
        let operations = operations();
        for (method, template) in &operations {
            if template.contains('{') {
                continue;
            }
            let (found, params) = route_of(method, template).unwrap();
            assert_eq!(&found, template);
            assert!(params.is_empty());
        }
    }

    #[test]
    fn every_template_is_found_from_its_own_concrete_path() {
        for (method, template) in operations() {
            let concrete = template
                .split('/')
                .map(|part| if part.starts_with('{') { "x1" } else { part })
                .collect::<Vec<_>>()
                .join("/");
            let (found, params) = route_of(method, &concrete).unwrap();
            assert_eq!(found, template, "{method} {concrete}");
            assert!(params.iter().all(|p| p == "x1"));
        }
    }

    #[test]
    fn a_dot_segment_or_an_empty_one_is_not_a_parameter() {
        assert!(route_of("GET", "/messages/..").is_none());
        assert!(route_of("GET", "/messages/.").is_none());
        assert!(route_of("GET", "/messages/").is_none());
    }

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
