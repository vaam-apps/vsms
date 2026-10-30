#![doc = include_str!("keypair_gate.md")]

use cratestack::CratestackError;
use cratestack::axum::extract::{Request, State};
use cratestack::axum::http::Method;
use cratestack::axum::middleware::Next;
use cratestack::axum::response::Response;

use crate::hmac_gate::AUTH_METHOD_DISABLED;
use crate::rbac::error_response;

/// Whether this deployment permits `keypair` (`auth.methods`).
///
/// A two-valued type for the reason `HmacWebhooks` is one: `require(..)` and the
/// router take it beside other arguments, and a bare `bool` does not say which
/// way it fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keypair {
    /// `auth.methods` permits `keypair` (the default when the key is absent).
    Permitted,
    /// It does not: no request is accepted signed, so no key can be used.
    Disabled,
}

/// The routes that register a signing key, each with what a refusal says.
///
/// Registration only. `revokeAppClientKey` and `revokeConsoleKey` stay open on a
/// deployment that turned the method off, so an operator can still retire the
/// keys it left behind, the way an endpoint left over from `hmac_webhooks` can
/// still be deactivated.
const REGISTRATION_ROUTES: &[(&str, &str)] = &[
    (
        "/$procs/registerAppClientKey",
        "a signing key cannot be registered; register a machine client with provision-client",
    ),
    (
        "/$procs/registerConsoleKey",
        "a console key cannot be paired; pairing a console returns with VSA-5",
    ),
];

impl Keypair {
    /// From `AuthMethods::permits(AuthMethod::Keypair)`.
    #[must_use]
    pub const fn from_permitted(permitted: bool) -> Self {
        if permitted {
            Self::Permitted
        } else {
            Self::Disabled
        }
    }

    /// Whether `keypair` is permitted.
    #[must_use]
    pub const fn is_permitted(self) -> bool {
        matches!(self, Self::Permitted)
    }

    /// `Ok` when `keypair` is permitted; otherwise the 422 that says what was
    /// refused and why.
    ///
    /// # Errors
    ///
    /// [`CratestackError::Validation`] (HTTP 422) whose message starts with
    /// [`AUTH_METHOD_DISABLED`].
    pub fn require(self, what: &str) -> Result<(), CratestackError> {
        match self {
            Self::Permitted => Ok(()),
            Self::Disabled => Err(CratestackError::Validation(format!(
                "{AUTH_METHOD_DISABLED}: this deployment does not permit keypair (auth.methods), \
                 so {what}"
            ))),
        }
    }
}

/// Refuses a key registration (422, `auth_method_disabled`) when `keypair` is
/// not permitted; a no-op for every other request.
///
/// It sits just outside the generated router and inside the RBAC layers, with
/// [`crate::hmac_gate::enforce_hmac_gate`]: a caller who may not call the
/// procedure at all hears that first, and one who is not authenticated never gets
/// this far. It does not authenticate by itself for that reason.
pub async fn enforce_keypair_gate(
    State(keypair): State<Keypair>,
    request: Request,
    next: Next,
) -> Response {
    if keypair.is_permitted() || request.method() != Method::POST {
        return next.run(request).await;
    }
    let Some((_, what)) = REGISTRATION_ROUTES
        .iter()
        .find(|(path, _)| *path == request.uri().path())
    else {
        return next.run(request).await;
    };
    match keypair.require(what) {
        Ok(()) => next.run(request).await,
        Err(error) => error_response(&error),
    }
}

#[cfg(test)]
mod tests {
    use cratestack::axum::Router;
    use cratestack::axum::body::Body;
    use cratestack::axum::http::{Request as HttpRequest, StatusCode};
    use cratestack::axum::middleware::from_fn_with_state;
    use cratestack::axum::routing::{get, post};
    use tower::ServiceExt;

    use super::*;

    fn app(keypair: Keypair) -> Router {
        Router::new()
            .route("/$procs/registerAppClientKey", post(|| async { "ran" }))
            .route("/$procs/registerConsoleKey", post(|| async { "ran" }))
            .route("/$procs/revokeAppClientKey", post(|| async { "ran" }))
            .route("/$procs/revokeConsoleKey", post(|| async { "ran" }))
            .route("/$procs/registerAppClientKey", get(|| async { "read" }))
            .layer(from_fn_with_state(keypair, enforce_keypair_gate))
    }

    async fn call(app: Router, method: &str, path: &str) -> (StatusCode, String) {
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = cratestack::axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn with_keypair_off_both_registrations_are_a_422_that_says_why() {
        for (path, mention) in [
            ("/$procs/registerAppClientKey", "provision-client"),
            ("/$procs/registerConsoleKey", "VSA-5"),
        ] {
            let (status, body) = call(app(Keypair::Disabled), "POST", path).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path}: {body}");
            assert!(body.contains(AUTH_METHOD_DISABLED), "{path}: {body}");
            assert!(body.contains(mention), "{path}: {body}");
        }
    }

    #[tokio::test]
    async fn with_keypair_on_nothing_is_refused() {
        for path in ["/$procs/registerAppClientKey", "/$procs/registerConsoleKey"] {
            let (status, body) = call(app(Keypair::Permitted), "POST", path).await;
            assert_eq!((status, body.as_str()), (StatusCode::OK, "ran"), "{path}");
        }
    }

    /// The gate is registration only: a revocation still runs, so an operator can
    /// retire the keys a turned-off method left behind, and nothing else changes.
    #[tokio::test]
    async fn revocations_and_other_methods_still_run_with_keypair_off() {
        for path in ["/$procs/revokeAppClientKey", "/$procs/revokeConsoleKey"] {
            let (status, _) = call(app(Keypair::Disabled), "POST", path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
        }
        let (status, body) = call(
            app(Keypair::Disabled),
            "GET",
            "/$procs/registerAppClientKey",
        )
        .await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "read"));
    }
}
