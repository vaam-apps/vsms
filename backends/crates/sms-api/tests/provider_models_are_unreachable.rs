//! VSA-2 brought five models back (`OauthClient`, `OauthSigningKey`,
//! `ClientAssertion`, `UserCredential`, `SsoTransaction`) as persistence only.
//! Nothing reads or writes them until VSA-3 and VSA-4, and two of them hold the
//! most sensitive material this system has had: the OP's private signing keys
//! and Argon2id password hashes. This file is what keeps them unreachable, and
//! it needs no database, so plain `just test` runs it.
//!
//! Two locks, each checked by what it does:
//!
//! - **No route.** `@@internal("all")` suppresses the generated REST routes, the
//!   RPC arms and the client stubs. The route table the router is built from
//!   names none of the five tables' paths.
//! - **No policy but `system`.** Every `@@allow` on each of the five admits
//!   `hasRole('system')` and nothing else, so a route that did come back would
//!   still answer an HTTP caller with an empty list or a refusal: `system` is
//!   constructed only inside a process (`sms_api::auth::system_context`).
//!
//! `ConsoleKey` and `FederatedLink` (system-only too, but routed) are the
//! positive controls: the scan for a route is proved able to see one.

/// The five models and the table path each would be routed at.
const MODELS: [(&str, &str); 5] = [
    ("OauthClient", "/oauth_clients"),
    ("OauthSigningKey", "/oauth_signing_keys"),
    ("ClientAssertion", "/client_assertions"),
    ("UserCredential", "/user_credentials"),
    ("SsoTransaction", "/sso_transactions"),
];

const SCHEMA: &str = include_str!("../../../../schemas/vsms.cstack");

/// The text of `model <name> { ... }` in the schema.
fn model_block(name: &str) -> &'static str {
    let opening = format!("\nmodel {name} {{");
    let start = SCHEMA
        .find(&opening)
        .unwrap_or_else(|| panic!("schemas/vsms.cstack has no model {name}"));
    let body = &SCHEMA[start + opening.len()..];
    let end = body.find("\n}").expect("a model block ends with `}`");
    &body[..end]
}

#[test]
fn no_route_is_generated_for_a_restored_provider_model() {
    let routes = sms_api::route_table();
    for (model, path) in MODELS {
        let found: Vec<_> = routes
            .iter()
            .filter(|(_, route)| *route == path || route.starts_with(&format!("{path}/")))
            .collect();
        assert_eq!(
            found.len(),
            0,
            "{model} must have no generated route, found {found:?}"
        );
    }
    // The scan can see a routed system-only model.
    for control in ["/console_keys", "/federated_links"] {
        assert!(
            routes.iter().any(|(_, route)| *route == control),
            "the control {control} is routed, so the scan above can fail"
        );
    }
}

#[test]
fn every_restored_provider_model_is_internal() {
    for (model, _) in MODELS {
        let block = model_block(model);
        assert!(
            block
                .lines()
                .any(|line| line.trim() == r#"@@internal("all")"#),
            "{model} must carry @@internal(\"all\"): VSA-3/4 take it off a model only on purpose"
        );
    }
}

#[test]
fn every_restored_provider_model_admits_the_system_context_and_no_one_else() {
    for (model, _) in MODELS {
        let policies: Vec<&str> = model_block(model)
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("@@allow(") || line.starts_with("@@deny("))
            .collect();
        assert_ne!(policies.len(), 0, "{model} declares its policies");
        for policy in policies {
            assert!(
                policy.starts_with("@@allow(")
                    && policy.ends_with(", hasRole('system'))")
                    && policy.matches("hasRole(").count() == 1
                    && !policy.contains("auth()")
                    && !policy.contains("||")
                    && !policy.contains("&&"),
                "{model}: `{policy}` admits something besides the system context"
            );
        }
    }
}
