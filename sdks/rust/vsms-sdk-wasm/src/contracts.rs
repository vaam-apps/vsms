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
/// comparing the two. When two templates match, the gateway's router would
/// pick the one with a literal segment at the first position where they
/// differ, and so does this ([`resolve`]).
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
    let templates: Vec<&str> = operations()
        .into_iter()
        .filter(|(m, _)| *m == method)
        .map(|(_, template)| template)
        .collect();
    resolve(&templates, path)
}

fn is_parameter(segment: &str) -> bool {
    segment.starts_with('{') && segment.ends_with('}')
}

/// The template of `templates` that `path` is, with the decoded values of its
/// parameters.
///
/// Several can match (`/a/{x}/c` and `/{y}/b/c` both match `/a/b/c`). The
/// router the gateway serves with (axum, over matchit) walks a tree in which a
/// literal segment is tried before a parameter at every step, so the winner is
/// the template that is literal at the **first** segment where the two differ
/// in kind: not the one with fewer parameters, which would pick the other here
/// and sign a route the gateway does not run.
fn resolve(templates: &[&str], path: &str) -> Option<(String, Vec<String>)> {
    let segments: Vec<&str> = path.split('/').collect();
    templates
        .iter()
        .filter_map(|template| {
            let parts: Vec<&str> = template.split('/').collect();
            if parts.len() != segments.len() {
                return None;
            }
            let mut params = Vec::new();
            for (part, segment) in parts.iter().zip(&segments) {
                if is_parameter(part) {
                    let decoded = percent_decode_str(segment).decode_utf8().ok()?.into_owned();
                    if decoded.is_empty() || decoded == "." || decoded == ".." {
                        return None;
                    }
                    params.push(decoded);
                } else if part != segment {
                    return None;
                }
            }
            Some(((*template).to_owned(), params))
        })
        .min_by(|(a, _), (b, _)| literal_first(a, b))
}

/// `Less` when `a` is literal where `b` is a parameter at the first segment in
/// which they differ in kind, `Greater` for the reverse.
fn literal_first(a: &str, b: &str) -> std::cmp::Ordering {
    for (x, y) in a.split('/').zip(b.split('/')) {
        match (is_parameter(x), is_parameter(y)) {
            (false, true) => return std::cmp::Ordering::Less,
            (true, false) => return std::cmp::Ordering::Greater,
            _ => {}
        }
    }
    std::cmp::Ordering::Equal
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_route_without_parameters_is_found_as_itself() {
        // No two templates collide in today's schema, so the tie-break has
        // nothing to decide yet; `a_literal_segment_beats_a_parameter_at_the_first_difference`
        // is what keeps a future literal sibling (`/messages/export`) from
        // being read as an id.
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

    #[test]
    fn a_literal_segment_beats_a_parameter_at_the_first_difference() {
        let templates = ["/a/{x}/c", "/{y}/b/c"];
        // Both match. The router tries the literal `a` before a parameter at
        // the first segment, so `/a/{x}/c` runs, though it has the same number
        // of parameters as the other.
        assert_eq!(
            resolve(&templates, "/a/b/c"),
            Some(("/a/{x}/c".to_owned(), vec!["b".to_owned()]))
        );
        // The order the table lists them in does not matter.
        let reversed = ["/{y}/b/c", "/a/{x}/c"];
        assert_eq!(resolve(&reversed, "/a/b/c").unwrap().0, "/a/{x}/c");
        // Where only the second matches, it is the second.
        assert_eq!(
            resolve(&templates, "/z/b/c"),
            Some(("/{y}/b/c".to_owned(), vec!["z".to_owned()]))
        );
    }

    #[test]
    fn fewer_parameters_does_not_decide_when_the_first_difference_says_otherwise() {
        // `/{a}/b/c` has one parameter and `/x/{b}/{c}` has two, and both match
        // `/x/b/c`: the first segment is where they differ, and a literal wins
        // there, so the template with more parameters is the one that runs.
        let templates = ["/{a}/b/c", "/x/{b}/{c}"];
        assert_eq!(resolve(&templates, "/x/b/c").unwrap().0, "/x/{b}/{c}");
    }

    #[test]
    fn a_literal_sibling_is_not_read_as_an_id() {
        let templates = ["/messages/{id}", "/messages/export"];
        assert_eq!(
            resolve(&templates, "/messages/export").unwrap(),
            ("/messages/export".to_owned(), Vec::new())
        );
        assert_eq!(
            resolve(&templates, "/messages/m1").unwrap().1,
            vec!["m1".to_owned()]
        );
    }
}
