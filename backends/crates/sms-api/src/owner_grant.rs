#![doc = include_str!("owner_grant.md")]

use cratestack::axum::body::{Body, to_bytes};
use cratestack::axum::extract::{Request, State};
use cratestack::axum::http::{HeaderMap, Method};
use cratestack::axum::middleware::Next;
use cratestack::axum::response::Response;
use cratestack::{
    CratestackContext, CratestackError, RouteTransportCapabilities, Value,
    decode_transport_request_for,
};

use crate::auth::SignedAuth;
use crate::rbac::{authenticate_request, error_response, path_matches};
use crate::router::ROUTER_CODEC;
use crate::schema;

/// The role key only an `owner` may hand out. Compared exactly the way the
/// schema's `roleKey != 'owner'` clause does, plus ASCII case/whitespace
/// folding on this side (`Role.key`'s `@regex` is lowercase-only, so a folded
/// match can only ever *over*-refuse a key that cannot exist as a role).
pub const OWNER_ROLE_KEY: &str = "owner";

/// Which generated input type a guarded route decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Create,
    Update,
}

/// One guarded route: `path` uses the generated router's `{id}` wildcard
/// convention, exactly like [`crate::rbac::RoutePermission`].
struct GuardedRoute {
    method: Method,
    path: &'static str,
    shape: Shape,
}

/// Every generated route that writes a `User` row with a caller-supplied
/// body. `DELETE /users/{id}` carries no body and is owner-only in the
/// schema; `POST /$procs/provisionUser` is covered by the schema's `create`
/// clause (see the module doc).
const GUARDED_ROUTES: &[GuardedRoute] = &[
    GuardedRoute {
        method: Method::POST,
        path: "/users",
        shape: Shape::Create,
    },
    GuardedRoute {
        method: Method::PATCH,
        path: "/users/{id}",
        shape: Shape::Update,
    },
];

/// State for [`enforce_owner_grant_guard`].
#[derive(Clone)]
pub struct OwnerGrantState {
    pub auth: SignedAuth,
}

fn is_owner(ctx: &CratestackContext) -> bool {
    matches!(ctx.auth_field("role"), Some(Value::String(role)) if role == OWNER_ROLE_KEY)
}

fn names_owner(role_key: &str) -> bool {
    role_key.trim().eq_ignore_ascii_case(OWNER_ROLE_KEY)
}

/// The capabilities the generated route declares for `method path`, read from
/// the schema's own route table so the guard negotiates the request
/// `Content-Type` exactly as the handler will.
fn route_capabilities(method: &Method, path: &str) -> Option<RouteTransportCapabilities> {
    schema::axum::ROUTE_TRANSPORTS
        .iter()
        .find(|route| route.method == method.as_str() && route.path == path)
        .map(|route| route.capabilities)
}

/// Decodes `body` as the route's own input type and refuses it when it sets
/// `roleKey` to `owner`. Pure: no I/O, no authentication.
///
/// # Errors
///
/// - [`CratestackError::Forbidden`] when the body sets `roleKey` to `owner`.
/// - [`CratestackError::UnsupportedMediaType`] when the router has no
///   decoder for the request `Content-Type`.
/// - [`CratestackError::BadRequest`] for any other body that does not decode
///   (fail closed: an undecodable body is never forwarded).
fn refuse_owner_grant(
    route: &GuardedRoute,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), CratestackError> {
    let capabilities = route_capabilities(&route.method, route.path).ok_or_else(|| {
        // A guarded route the schema no longer generates: refuse rather than
        // forward a body this guard cannot read.
        CratestackError::Internal(format!(
            "owner-grant guard: no generated route {} {}",
            route.method, route.path
        ))
    })?;

    let role_key = match route.shape {
        Shape::Create => {
            decode_transport_request_for::<_, schema::CreateUserInput>(
                &ROUTER_CODEC,
                headers,
                &capabilities,
                body,
            )
            .map_err(as_client_error)?
            .roleKey
        }
        Shape::Update => decode_transport_request_for::<_, schema::UpdateUserInput>(
            &ROUTER_CODEC,
            headers,
            &capabilities,
            body,
        )
        .map_err(as_client_error)?
        .roleKey
        .unwrap_or_default(),
    };

    if names_owner(&role_key) {
        Err(CratestackError::Forbidden(
            "only an owner may assign the owner role".to_owned(),
        ))
    } else {
        Ok(())
    }
}

/// A decode failure is the caller's fault: keep 415, report everything else
/// as 400 rather than whatever internal class the codec used.
fn as_client_error(error: CratestackError) -> CratestackError {
    match error {
        CratestackError::UnsupportedMediaType(_) | CratestackError::BadRequest(_) => error,
        _ => CratestackError::BadRequest("request body could not be decoded".to_owned()),
    }
}

/// Wraps the whole generated router (see `router::router`). A no-op for any
/// request that is not a guarded `User` write.
///
/// For a match: authenticates the request (`authenticate_request` — the
/// same call `enforce_route_permission` makes) and, unless the principal's
/// role is `owner`, buffers the body up to `DEFAULT_BODY_LIMIT_BYTES`, runs
/// `refuse_owner_grant`, and forwards a request rebuilt from the original
/// bytes. Authentication failures short-circuit with the same 401 the
/// generated router would have produced.
pub async fn enforce_owner_grant_guard(
    State(state): State<OwnerGrantState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(route) = GUARDED_ROUTES.iter().find(|route| {
        route.method == *request.method() && path_matches(route.path, request.uri().path())
    }) else {
        return next.run(request).await;
    };

    let ctx = match authenticate_request(&state.auth, &request).await {
        Ok(ctx) => ctx,
        Err(error) => return error_response(&error),
    };

    if is_owner(&ctx) {
        return next.run(request).await;
    }

    let (parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, cratestack::DEFAULT_BODY_LIMIT_BYTES).await else {
        return error_response(&CratestackError::BadRequest(
            "request body is too large or unreadable".to_owned(),
        ));
    };

    if let Err(error) = refuse_owner_grant(route, &parts.headers, &bytes) {
        return error_response(&error);
    }

    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cratestack::axum::http::{HeaderValue, header};

    fn headers(content_type: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(content_type).unwrap(),
        );
        headers
    }

    fn create_route() -> &'static GuardedRoute {
        &GUARDED_ROUTES[0]
    }

    fn update_route() -> &'static GuardedRoute {
        &GUARDED_ROUTES[1]
    }

    fn create_body(role_key: &str) -> Vec<u8> {
        serde_json::json!({
            "subject": "s", "email": "a@example.test", "displayName": "d",
            "roleKey": role_key,
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn an_update_naming_owner_is_forbidden() {
        let body = br#"{"roleKey":"owner"}"#;
        assert!(matches!(
            refuse_owner_grant(update_route(), &headers("application/json"), body),
            Err(CratestackError::Forbidden(_))
        ));
    }

    #[test]
    fn a_create_naming_owner_is_forbidden() {
        assert!(matches!(
            refuse_owner_grant(
                create_route(),
                &headers("application/json"),
                &create_body("owner")
            ),
            Err(CratestackError::Forbidden(_))
        ));
    }

    #[test]
    fn case_and_whitespace_variants_of_owner_are_forbidden() {
        for variant in ["Owner", "OWNER", " owner", "owner "] {
            let body = serde_json::json!({ "roleKey": variant })
                .to_string()
                .into_bytes();
            assert!(
                matches!(
                    refuse_owner_grant(update_route(), &headers("application/json"), &body),
                    Err(CratestackError::Forbidden(_))
                ),
                "{variant:?} slipped through"
            );
        }
    }

    #[test]
    fn a_content_type_with_parameters_is_still_decoded() {
        let body = br#"{"roleKey":"owner"}"#;
        assert!(matches!(
            refuse_owner_grant(
                update_route(),
                &headers("application/json; charset=utf-8"),
                body
            ),
            Err(CratestackError::Forbidden(_))
        ));
    }

    #[test]
    fn other_roles_and_an_absent_role_key_are_allowed() {
        for body in [
            br#"{"roleKey":"operator"}"#.as_slice(),
            br#"{"displayName":"x"}"#,
            br#"{"active":false}"#,
            br"{}",
        ] {
            assert!(
                refuse_owner_grant(update_route(), &headers("application/json"), body).is_ok(),
                "{} was refused",
                String::from_utf8_lossy(body)
            );
        }
        assert!(
            refuse_owner_grant(
                create_route(),
                &headers("application/json"),
                &create_body("admin")
            )
            .is_ok()
        );
    }

    #[test]
    fn an_undecodable_body_is_refused_not_forwarded() {
        for body in [b"".as_slice(), b"not json", b"[1,2]", br#"{"roleKey":7}"#] {
            assert!(
                matches!(
                    refuse_owner_grant(update_route(), &headers("application/json"), body),
                    Err(CratestackError::BadRequest(_))
                ),
                "{} was not refused with 400",
                String::from_utf8_lossy(body)
            );
        }
        // A create body missing required fields does not decode either.
        assert!(matches!(
            refuse_owner_grant(
                create_route(),
                &headers("application/json"),
                br#"{"roleKey":"admin"}"#
            ),
            Err(CratestackError::BadRequest(_))
        ));
    }

    #[test]
    fn a_duplicate_role_key_is_refused_like_the_handler_would() {
        // The generated input type rejects a duplicate field, so the guard
        // and the handler agree it is not a usable body.
        let body = br#"{"roleKey":"owner","roleKey":"operator"}"#;
        assert!(refuse_owner_grant(update_route(), &headers("application/json"), body).is_err());
    }

    #[test]
    fn a_cbor_body_is_refused_while_the_router_is_json_only() {
        // 0xA1 0x67 "roleKey" 0x65 "owner": {"roleKey": "owner"} in CBOR.
        let mut body = vec![0xA1, 0x67];
        body.extend_from_slice(b"roleKey");
        body.push(0x65);
        body.extend_from_slice(b"owner");
        assert!(matches!(
            refuse_owner_grant(update_route(), &headers("application/cbor"), &body),
            Err(CratestackError::UnsupportedMediaType(_))
        ));
    }

    #[test]
    fn a_missing_content_type_is_refused() {
        assert!(matches!(
            refuse_owner_grant(
                update_route(),
                &HeaderMap::new(),
                br#"{"roleKey":"operator"}"#
            ),
            Err(CratestackError::UnsupportedMediaType(_))
        ));
    }

    #[test]
    fn only_an_owner_role_is_exempt() {
        let ctx = |role: &str| {
            CratestackContext::authenticated([("role".to_owned(), Value::String(role.to_owned()))])
        };
        assert!(is_owner(&ctx("owner")));
        for role in ["admin", "system", "app", "Owner", ""] {
            assert!(!is_owner(&ctx(role)), "{role:?} treated as owner");
        }
        assert!(!is_owner(&CratestackContext::authenticated([])));
    }

    #[test]
    fn every_guarded_route_exists_in_the_generated_route_table() {
        for route in GUARDED_ROUTES {
            assert!(
                route_capabilities(&route.method, route.path).is_some(),
                "{} {} is not a generated route",
                route.method,
                route.path
            );
        }
    }

    /// The reason `GUARDED_ROUTES` is a list nobody can forget to extend: a
    /// new generated route that writes a `User` (a batch or upsert route, say)
    /// fails here until it is guarded or explicitly classified.
    #[test]
    fn every_generated_user_write_route_is_guarded() {
        // Routes under `/users` that write but need no body guard:
        // DELETE carries no body and is owner-only in the schema.
        const BODYLESS: &[(&str, &str)] = &[("DELETE", "/users/{id}")];

        for (method, path) in crate::router::route_table() {
            let is_user_path = path == "/users" || path.starts_with("/users/");
            let writes = method != "GET";
            if !is_user_path || !writes {
                continue;
            }
            let guarded = GUARDED_ROUTES
                .iter()
                .any(|route| route.method.as_str() == method && route.path == path);
            let bodyless = BODYLESS.contains(&(method, path));
            assert!(
                guarded || bodyless,
                "{method} {path} writes a User and is neither guarded nor classified"
            );
        }
    }
}
