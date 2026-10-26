#![doc = include_str!("pair_scope.md")]

use cratestack::axum::body::{Body, to_bytes};
use cratestack::axum::extract::Request;
use cratestack::axum::http::{HeaderMap, Method};
use cratestack::axum::middleware::Next;
use cratestack::axum::response::Response;
use cratestack::{CratestackError, decode_transport_request_for};

use crate::console_keys::{PAIR_SCOPE, names_pair_scope};
use crate::owner_grant::{as_client_error, route_capabilities};
use crate::rbac::{error_response, path_matches};
use crate::router::ROUTER_CODEC;
use crate::schema;

/// Which generated input type a guarded route decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Create,
    Update,
}

struct GuardedRoute {
    method: Method,
    path: &'static str,
    shape: Shape,
}

/// Every generated route that writes `AppClient.scopes` with a caller-supplied
/// body. `DELETE /app_clients/{id}` carries none.
const GUARDED_ROUTES: &[GuardedRoute] = &[
    GuardedRoute {
        method: Method::POST,
        path: "/app_clients",
        shape: Shape::Create,
    },
    GuardedRoute {
        method: Method::PATCH,
        path: "/app_clients/{id}",
        shape: Shape::Update,
    },
];

/// Refuses a packed scope column that names `console:pair`.
///
/// ```
/// use sms_api::pair_scope::refuse_pair_scope;
///
/// assert!(refuse_pair_scope(" sms:send sms:read ").is_ok());
/// assert!(refuse_pair_scope(" sms:send console:pair ").is_err());
/// ```
///
/// # Errors
///
/// [`CratestackError::Forbidden`] when `scopes` names the pairing scope.
pub fn refuse_pair_scope(scopes: &str) -> Result<(), CratestackError> {
    if names_pair_scope(scopes) {
        Err(CratestackError::Forbidden(format!(
            "{PAIR_SCOPE} is granted only by `sms-gateway register-client`"
        )))
    } else {
        Ok(())
    }
}

/// Decodes `body` as the route's own input type and refuses it when it sets
/// `scopes` to something naming `console:pair`. Pure: no I/O.
fn refuse_pair_grant(
    route: &GuardedRoute,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), CratestackError> {
    let capabilities = route_capabilities(&route.method, route.path).ok_or_else(|| {
        CratestackError::Internal(format!(
            "pair-scope guard: no generated route {} {}",
            route.method, route.path
        ))
    })?;
    let scopes = match route.shape {
        Shape::Create => {
            decode_transport_request_for::<_, schema::CreateAppClientInput>(
                &ROUTER_CODEC,
                headers,
                &capabilities,
                body,
            )
            .map_err(as_client_error)?
            .scopes
        }
        Shape::Update => decode_transport_request_for::<_, schema::UpdateAppClientInput>(
            &ROUTER_CODEC,
            headers,
            &capabilities,
            body,
        )
        .map_err(as_client_error)?
        .scopes
        .unwrap_or_default(),
    };
    refuse_pair_scope(&scopes)
}

/// Wraps the whole generated router. A no-op for any request that is not an
/// `AppClient` write with a body; for one, buffers the body up to
/// `DEFAULT_BODY_LIMIT_BYTES`, refuses it when it grants `console:pair`, and
/// forwards a request rebuilt from the original bytes. No caller is exempt: the
/// scope is not an `owner`'s to grant over HTTP.
pub async fn enforce_pair_scope_guard(request: Request, next: Next) -> Response {
    let Some(route) = GUARDED_ROUTES.iter().find(|route| {
        route.method == *request.method() && path_matches(route.path, request.uri().path())
    }) else {
        return next.run(request).await;
    };

    let (parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, cratestack::DEFAULT_BODY_LIMIT_BYTES).await else {
        return error_response(&CratestackError::BadRequest(
            "request body is too large or unreadable".to_owned(),
        ));
    };
    if let Err(error) = refuse_pair_grant(route, &parts.headers, &bytes) {
        return error_response(&error);
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cratestack::axum::http::{HeaderValue, header};

    fn json() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers
    }

    fn update() -> &'static GuardedRoute {
        &GUARDED_ROUTES[1]
    }

    fn create() -> &'static GuardedRoute {
        &GUARDED_ROUTES[0]
    }

    #[test]
    fn an_update_naming_the_pair_scope_is_forbidden() {
        for scopes in [" sms:send console:pair ", "console:pair", "CONSOLE:PAIR"] {
            let body = serde_json::json!({ "scopes": scopes }).to_string();
            assert!(
                matches!(
                    refuse_pair_grant(update(), &json(), body.as_bytes()),
                    Err(CratestackError::Forbidden(_))
                ),
                "{scopes:?}"
            );
        }
    }

    #[test]
    fn other_updates_pass() {
        for body in [
            r#"{"scopes":" sms:send sms:read "}"#,
            r#"{"scopes":" console:pair2 "}"#,
            r#"{"label":"renamed"}"#,
            r#"{"active":false}"#,
        ] {
            assert!(
                refuse_pair_grant(update(), &json(), body.as_bytes()).is_ok(),
                "{body}"
            );
        }
    }

    #[test]
    fn a_create_naming_the_pair_scope_is_forbidden() {
        let body = serde_json::json!({
            "appId": "a", "clientId": "appc_12345678", "label": "x",
            "scopes": " console:pair ",
        })
        .to_string();
        assert!(matches!(
            refuse_pair_grant(create(), &json(), body.as_bytes()),
            Err(CratestackError::Forbidden(_))
        ));
    }

    #[test]
    fn an_undecodable_body_is_refused_not_forwarded() {
        assert!(matches!(
            refuse_pair_grant(update(), &json(), b"{not json"),
            Err(CratestackError::BadRequest(_))
        ));
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

    /// A new generated route that writes an `AppClient` fails here until it is
    /// guarded or classified.
    #[test]
    fn every_generated_app_client_write_route_is_guarded() {
        const BODYLESS: &[(&str, &str)] = &[("DELETE", "/app_clients/{id}")];

        for (method, path) in crate::router::route_table() {
            let is_path = path == "/app_clients" || path.starts_with("/app_clients/");
            if !is_path || method == "GET" {
                continue;
            }
            let guarded = GUARDED_ROUTES
                .iter()
                .any(|route| route.method.as_str() == method && route.path == path);
            assert!(
                guarded || BODYLESS.contains(&(method, path)),
                "{method} {path} writes an AppClient and is neither guarded nor classified"
            );
        }
    }
}
