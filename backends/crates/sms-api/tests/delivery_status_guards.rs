//! The delivery-status lookup is the one place a person who may not read a message
//! is told what happened to it. These checks need no database, so plain `just test`
//! runs them, and each one reads a file rather than a type: a field that creeps
//! back into the answer, a claim that widens `GET /messages`, or an audit model that
//! gains a route is a text change a compiler would accept.
//!
//! - **The answer has no body.** `DeliveryStatus` is pinned field for field, and the
//!   names it must never carry are checked on their own, so the failure says which
//!   rule was broken and not only that a list changed.
//! - **No policy knows `delivery`.** `Message`'s `@@allow` asks for `messageRead`;
//!   a `deliveryRead` claim there would let everyone holding `delivery:read` list
//!   `GET /messages`, bodies included. The gate is the procedure's own check.
//! - **The audit model is unreachable.** `DeliveryLookup` is `@@internal("all")`,
//!   `@@audit`, system-only, and has no route.
//! - **The check is the first statement** of the procedure's body.

const SCHEMA: &str = include_str!("../../../../schemas/vsms.cstack");
const PROCEDURES: &str = include_str!("../src/procedures.rs");
const AUTH: &str = include_str!("../src/auth.rs");

/// Everything a delivery status must never carry, and why.
const NEVER_IN_A_DELIVERY_STATUS: [(&str, &str); 14] = [
    ("body", "the message text"),
    (
        "bodyHash",
        "an OTP is brute-forceable from its hash if the pepper leaks",
    ),
    ("bodyLength", "a length narrows a guess at the text"),
    ("msisdn", "the full number, even when the caller typed it"),
    (
        "msisdnHash",
        "a stable identifier for the number that survives a purge",
    ),
    ("costXaf", "billing, not delivery"),
    ("stateReason", "free text a provider's error put there"),
    (
        "clientRef",
        "the caller's own reference, purged at 90 days as personal data",
    ),
    ("idempotencyKey", "a verbatim copy of clientRef"),
    ("providerMessageRef", "the provider's id for the message"),
    (
        "providerMessageRefAlt",
        "the provider's second id for the message",
    ),
    ("routeId", "routing internals"),
    ("excludedRouteIds", "routing internals"),
    ("leaseOwner", "worker internals"),
];

/// The fields of `type <name> { ... }`, in order.
fn type_fields(name: &str) -> Vec<String> {
    let opening = format!("\ntype {name} {{");
    let start = SCHEMA
        .find(&opening)
        .unwrap_or_else(|| panic!("schemas/vsms.cstack has no type {name}"));
    let body = &SCHEMA[start + opening.len()..];
    let end = body.find("\n}").expect("a type block ends with `}`");
    body[..end]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .map(|line| line.split_whitespace().next().unwrap().to_owned())
        .collect()
}

fn model_block(name: &str) -> &'static str {
    let opening = format!("\nmodel {name} {{");
    let start = SCHEMA
        .find(&opening)
        .unwrap_or_else(|| panic!("schemas/vsms.cstack has no model {name}"));
    let body = &SCHEMA[start + opening.len()..];
    let end = body.find("\n}").expect("a model block ends with `}`");
    &body[..end]
}

/// The body of `async fn <name>` in `procedures.rs`: from its opening brace to
/// the next function at the same indent.
fn function_body(name: &str) -> &'static str {
    let opening = format!("    async fn {name}(");
    let start = PROCEDURES
        .find(&opening)
        .unwrap_or_else(|| panic!("procedures.rs has no `{name}`"));
    let rest = &PROCEDURES[start..];
    let brace = rest.find("{\n").expect("a function opens with `{`") + 2;
    let end = rest[brace..]
        .find("\n    }\n")
        .expect("a method ends at its own indent");
    &rest[brace..brace + end]
}

#[test]
fn a_delivery_status_is_pinned_field_for_field() {
    assert_eq!(
        type_fields("DeliveryStatus"),
        [
            "messageId",
            "appId",
            "recipient",
            "operator",
            "senderIdValue",
            "class",
            "encoding",
            "segments",
            "state",
            "attempts",
            "acceptedAt",
            "scheduledAt",
            "submittedAt",
            "finalizedAt",
            "expiresAt",
            "purged",
            "lastReceipt",
        ],
        "a field added to DeliveryStatus is a field every `delivery:read` holder reads: \
         decide it here, deliberately, and say in the PR why it is not a body"
    );
}

#[test]
fn a_delivery_status_never_carries_what_a_message_keeps_private() {
    let fields = type_fields("DeliveryStatus");
    for (forbidden, why) in NEVER_IN_A_DELIVERY_STATUS {
        assert!(
            !fields.iter().any(|field| field == forbidden),
            "DeliveryStatus carries `{forbidden}` ({why})"
        );
    }
    // The types it nests are held to the same rule.
    let receipt = type_fields("DeliveryReceiptSummary");
    assert!(
        !receipt.iter().any(|field| field == "rawPayload"),
        "DeliveryReceiptSummary carries the provider's raw callback"
    );
    for (forbidden, why) in NEVER_IN_A_DELIVERY_STATUS {
        assert!(
            !receipt.iter().any(|field| field == forbidden),
            "DeliveryReceiptSummary carries `{forbidden}` ({why})"
        );
    }
    let query_and_result: Vec<String> = type_fields("DeliveryStatusResult");
    assert_eq!(query_and_result, ["statuses", "truncated"]);
}

#[test]
fn the_query_names_three_keys_a_window_and_nothing_else() {
    assert_eq!(
        type_fields("DeliveryStatusQuery"),
        [
            "messageId",
            "appId",
            "clientRef",
            "msisdn",
            "since",
            "until",
            "limit"
        ]
    );
}

#[test]
fn no_policy_in_the_schema_mentions_delivery() {
    let policies: Vec<&str> = SCHEMA
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("@@allow(")
                || line.starts_with("@@deny(")
                || line.starts_with("@allow(")
                || line.starts_with("@deny(")
        })
        .collect();
    assert!(
        policies.len() > 50,
        "the scan found only {} policies: the scanner has drifted",
        policies.len()
    );
    for policy in policies {
        assert!(
            !policy.to_ascii_lowercase().contains("delivery"),
            "a policy asks about delivery: {policy}\n\
             `delivery:read` is checked by lookupDeliveryStatus's own body. A claim in a model \
             policy would widen that model's routes to everyone who holds the permission."
        );
    }
    assert!(
        !AUTH.contains("deliveryRead") && !AUTH.contains("delivery_read"),
        "auth.rs grows a delivery claim: Message's policy could then ask for it"
    );
}

#[test]
fn the_lookup_procedure_admits_a_person_or_an_app_key_on_one_line() {
    let at = SCHEMA
        .find("\nprocedure lookupDeliveryStatus(")
        .expect("the procedure is declared");
    let attributes: Vec<&str> = SCHEMA[at + 1..]
        .lines()
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        attributes,
        [r#"  @allow(auth().kind == "user" || auth().kind == "app")"#],
        "no @authorize (it runs the caller's Message policy, which delivery:read does not satisfy) \
         and no other attribute: the permission is the body's first statement"
    );
}

#[test]
fn the_procedure_checks_delivery_read_before_anything_else() {
    let body = function_body("delivery_status");
    let first = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("//"))
        .expect("the body has a statement");
    assert_eq!(first, r#"require_permission(ctx, "delivery:read")?;"#);
}

#[test]
fn the_audit_model_has_no_route_no_other_reader_and_is_audited() {
    let block = model_block("DeliveryLookup");
    let lines: Vec<&str> = block.lines().map(str::trim).collect();
    assert!(
        lines.contains(&r#"@@internal("all")"#),
        "DeliveryLookup must be @@internal(\"all\")"
    );
    assert!(lines.contains(&"@@audit"), "DeliveryLookup must be @@audit");
    assert!(
        !lines.iter().any(|line| line.starts_with("@@retain")),
        "DeliveryLookup is kept indefinitely (owner's default)"
    );
    let policies: Vec<&&str> = lines
        .iter()
        .filter(|line| line.starts_with("@@allow(") || line.starts_with("@@deny("))
        .collect();
    assert_eq!(
        policies.len(),
        3,
        "read, create and update (the settling of a pending row), nothing else: {policies:?}"
    );
    for action in ["read", "create", "update"] {
        assert!(
            policies
                .iter()
                .any(|policy| policy.starts_with(&format!(r#"@@allow("{action}", "#))),
            "DeliveryLookup has no {action} policy"
        );
    }
    for policy in policies {
        assert!(
            policy.starts_with("@@allow(")
                && policy.ends_with(", hasRole('system'))")
                && policy.matches("hasRole(").count() == 1
                && !policy.contains("auth()")
                && !policy.contains("||")
                && !policy.contains("&&"),
            "DeliveryLookup: `{policy}` admits something besides the system context"
        );
    }
    assert!(
        !lines
            .iter()
            .any(|line| line.contains(r#"@@allow("delete""#)),
        "a lookup row is never removed; it is only settled once, from pending"
    );
}

#[test]
fn the_procedure_is_routed_and_its_audit_table_is_not() {
    let routes = sms_api::route_table();
    assert!(
        routes
            .iter()
            .any(|(method, path)| *method == "POST" && *path == "/$procs/lookupDeliveryStatus"),
        "lookupDeliveryStatus is not in the route table"
    );
    let leaked: Vec<_> = routes
        .iter()
        .filter(|(_, path)| path.starts_with("/delivery_lookups"))
        .collect();
    assert!(leaked.is_empty(), "DeliveryLookup has a route: {leaked:?}");
    // The scan can see a routed procedure.
    assert!(
        routes
            .iter()
            .any(|(_, path)| *path == "/$procs/listMessageReceipts")
    );
}

#[test]
fn the_budget_is_sixty_lookups_by_number_or_reference_an_hour() {
    assert_eq!(sms_api::delivery_status::BUDGETED_LOOKUPS_PER_HOUR, 60);
}
