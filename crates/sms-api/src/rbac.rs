//! Layer 2 (#24, §5.1 of the design doc): permission/scope enforcement that
//! runs in Rust *before* a procedure body, or a generated route's handler,
//! ever executes — on top of whatever Layer 1's `@@allow` already decided
//! for the caller's role.
//!
//! One primitive, [`require_permission`], two call sites:
//!
//! - a procedure calls it directly, at the top of its body, before doing
//!   anything else (see `Procedures::send`, gated on `"sms:send"`) — the
//!   "in procedures" half of §5.1's "in procedures and a Tower layer";
//! - [`enforce_route_permission`] wraps it in an axum middleware for
//!   *generated* CRUD routes, which have no procedure body to call it
//!   from — the "a Tower layer" half. `router::PROVIDER_WRITE_ROUTES` is the
//!   one route this milestone wires it onto (`PATCH /providers/{id}`, the
//!   `provider:write` anchor #24 was asked to leave in place for #25); a
//!   second route needs only another entry in that slice, not a new
//!   mechanism.
//!
//! # Fail closed, per §5.2's own words
//!
//! *"An omitted `scope` yields `scope: None`, which your check must treat
//! as denial."* [`require_permission`] extends that same rule to `perms`:
//! a caller with neither claim present — or present but not containing the
//! literal required — is denied. There is no default-allow path.
//!
//! # Layer 1 stays the real perimeter
//!
//! §5.1's invariant: *"every permission checked in layer 2 sits behind a
//! role gate in layer 1 that is at least as restrictive. Layer 2 narrows;
//! it never widens."* Nothing here ever grants an operation Layer 1's own
//! `@@allow` would deny — `enforce_route_permission` only ever adds a
//! 401/403 *before* the generated router's own policy check, never a
//! bypass of it. See `PROVIDER_WRITE_ROUTES`'s own doc in `router.rs` for the
//! concrete consequence: this deployment's `GatewayAuth` never issues a
//! caller any role but `"app"` or `"system"` (see its own doc), and
//! `Provider.update`'s `@@allow` admits neither — so Layer 1 alone already
//! fully closes that route to every token this deployment can mint today.
//! Layer 2's gate on it is real, tested, and load-bearing the moment a
//! role-bearing (human) token exists; until then it is defense in depth,
//! not the thing actually stopping a live request.

use cratestack::axum::extract::{Request, State};
use cratestack::axum::http::Method;
use cratestack::axum::middleware::Next;
use cratestack::axum::response::{IntoResponse, Response};
use cratestack::axum::Json;
use cratestack::{AuthProvider, CoolContext, CoolError, CoolErrorResponse, RequestContext, Value};

use crate::auth::GatewayAuth;

/// Check the caller's `perms` (§5.2's human/role-token vocabulary) and
/// `scope` (§5.2's service-account vocabulary) against `required`. Either
/// containing the literal is sufficient; neither containing it — including
/// neither being present at all — is denial.
///
/// # Errors
///
/// [`CoolError::Forbidden`] when `required` is in neither claim.
pub fn require_permission(ctx: &CoolContext, required: &str) -> Result<(), CoolError> {
    let has_perm = matches!(
        ctx.extensions.get("perms"),
        Some(Value::List(items))
            if items.iter().any(|item| matches!(item, Value::String(s) if s == required))
    );
    let has_scope = matches!(
        ctx.extensions.get("scope"),
        Some(Value::String(scope)) if sms_core::contains(scope, required)
    );

    if has_perm || has_scope {
        Ok(())
    } else {
        Err(CoolError::Forbidden(format!(
            "missing required permission {required:?}"
        )))
    }
}

/// One HTTP route gated beyond whatever Layer 1's `@@allow` already
/// decides for it. `path` is matched segment-by-segment against the raw
/// URI path; a `{...}` segment (matching the generated router's own
/// `{id}` convention — see `router::route_table`) is a wildcard.
pub struct RoutePermission {
    pub method: Method,
    pub path: &'static str,
    pub permission: &'static str,
}

fn path_matches(template: &str, actual: &str) -> bool {
    let mut template_segments = template.split('/');
    let mut actual_segments = actual.split('/');
    loop {
        match (template_segments.next(), actual_segments.next()) {
            (Some(t), Some(a)) => {
                let is_wildcard = t.starts_with('{') && t.ends_with('}');
                if !is_wildcard && t != a {
                    return false;
                }
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

fn error_response(error: &CoolError) -> Response {
    let status = error.status_code();
    let body = CoolErrorResponse {
        code: error.code().to_owned(),
        message: error.public_message().into_owned(),
        details: None,
    };
    (status, Json(body)).into_response()
}

/// State for [`enforce_route_permission`] — a `GatewayAuth` to
/// (re-)authenticate against, plus the fixed set of routes this middleware
/// instance gates.
#[derive(Clone)]
pub struct RbacState {
    pub auth: GatewayAuth,
    pub requirements: &'static [RoutePermission],
}

/// Wraps the whole generated router (see `router::router`). A no-op for
/// any request that doesn't match one of `state.requirements` — every
/// other route passes straight through to Layer 1 exactly as before this
/// existed.
///
/// For a match: re-runs [`GatewayAuth::authenticate`] (cheap — cached JWKS
/// and a 60s-TTL `AppClient` lookup, the same cost the generated router's
/// own auth is about to pay a moment later on the same request) and
/// [`require_permission`], short-circuiting with 401/403 *before* the
/// request ever reaches the generated router's own SQL-policy check. A
/// request that passes both still goes through Layer 1 afterward — see
/// this module's own doc for why that's `Layer 2 narrows; it never
/// widens`, not the reverse.
pub async fn enforce_route_permission(
    State(state): State<RbacState>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();

    let Some(requirement) = state
        .requirements
        .iter()
        .find(|requirement| requirement.method == method && path_matches(requirement.path, &path))
    else {
        return next.run(request).await;
    };

    let headers = request.headers().clone();
    let query = request.uri().query().map(str::to_owned);
    let request_ctx = RequestContext {
        method: method.as_str(),
        path: &path,
        query: query.as_deref(),
        headers: &headers,
        // `GatewayAuth::authenticate` never reads the body — only the
        // generated router's own handler does, and this middleware leaves
        // `request`'s body untouched for it.
        body: &[],
    };

    let ctx = match state.auth.authenticate(&request_ctx).await {
        Ok(ctx) => ctx,
        Err(error) => return error_response(&error),
    };

    if let Err(error) = require_permission(&ctx, requirement.permission) {
        return error_response(&error);
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(perms: Option<&[&str]>, scope: Option<&str>) -> CoolContext {
        let mut ctx = CoolContext::authenticated([(
            "sub".to_owned(),
            Value::String("test-caller".to_owned()),
        )]);
        if let Some(perms) = perms {
            ctx.extensions.insert(
                "perms".to_owned(),
                Value::List(
                    perms
                        .iter()
                        .map(|p| Value::String((*p).to_owned()))
                        .collect(),
                ),
            );
        }
        ctx.extensions.insert(
            "scope".to_owned(),
            scope.map_or(Value::Null, |s| Value::String(s.to_owned())),
        );
        ctx
    }

    #[test]
    fn a_caller_with_neither_claim_present_is_denied() {
        let ctx = CoolContext::authenticated([]);
        assert!(matches!(
            require_permission(&ctx, "sms:send"),
            Err(CoolError::Forbidden(_))
        ));
    }

    #[test]
    fn an_absent_scope_is_denied_not_defaulted_through() {
        // §5.2, verbatim: "An omitted scope yields scope: None ... treat
        // missing scope as denial."
        let ctx = ctx_with(None, None);
        assert!(matches!(
            require_permission(&ctx, "sms:send"),
            Err(CoolError::Forbidden(_))
        ));
    }

    #[test]
    fn an_empty_perms_list_is_denied() {
        let ctx = ctx_with(Some(&[]), None);
        assert!(matches!(
            require_permission(&ctx, "provider:write"),
            Err(CoolError::Forbidden(_))
        ));
    }

    #[test]
    fn a_scope_that_does_not_contain_the_required_literal_is_denied() {
        let ctx = ctx_with(None, Some("sms:read"));
        assert!(matches!(
            require_permission(&ctx, "sms:send"),
            Err(CoolError::Forbidden(_))
        ));
    }

    #[test]
    fn a_scope_carrying_the_required_literal_among_others_is_allowed() {
        let ctx = ctx_with(None, Some("sms:read sms:send"));
        assert!(require_permission(&ctx, "sms:send").is_ok());
    }

    #[test]
    fn a_prefix_scope_does_not_false_match_a_longer_literal() {
        // The exact trap `sms_core::contains` exists to close — see its own
        // doc. "sms:sendall" must not satisfy a "sms:send" requirement.
        let ctx = ctx_with(None, Some("sms:sendall"));
        assert!(matches!(
            require_permission(&ctx, "sms:send"),
            Err(CoolError::Forbidden(_))
        ));
    }

    #[test]
    fn perms_carrying_the_required_literal_is_allowed() {
        let ctx = ctx_with(Some(&["message:send", "message:read"]), None);
        assert!(require_permission(&ctx, "message:send").is_ok());
    }

    #[test]
    fn perms_without_the_required_literal_is_denied() {
        // §5.2's own example: a `developer` role's perms
        // (`app:read`/`webhook:manage`/`message:read`/`message:send`)
        // contain no `provider:*` permission at all.
        let ctx = ctx_with(
            Some(&["app:read", "webhook:manage", "message:read", "message:send"]),
            None,
        );
        assert!(matches!(
            require_permission(&ctx, "provider:write"),
            Err(CoolError::Forbidden(_))
        ));
    }

    #[test]
    fn an_exact_path_matches_itself() {
        assert!(path_matches("/providers/{id}", "/providers/{id}"));
    }

    #[test]
    fn a_wildcard_segment_matches_any_concrete_value() {
        assert!(path_matches("/providers/{id}", "/providers/abc123"));
        assert!(!path_matches("/providers/{id}", "/providers/abc123/extra"));
        assert!(!path_matches("/providers/{id}", "/providers"));
    }

    #[test]
    fn a_different_literal_segment_does_not_match() {
        assert!(!path_matches("/providers/{id}", "/apps/abc123"));
    }
}
