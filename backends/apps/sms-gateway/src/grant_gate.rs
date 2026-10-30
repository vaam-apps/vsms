#![doc = include_str!("grant_gate.md")]

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sms_auth::op::ServedGrants;

/// The same ceiling `token_rate_limit` puts on a `/token` body: a real one is a
/// few hundred bytes, and this cannot itself become a memory-exhaustion vector.
const MAX_TOKEN_BODY_BYTES: usize = 64 * 1024;

/// The `grant_type` of an `application/x-www-form-urlencoded` body: `Ok(None)`
/// when there is none, [`Malformed`] when there are two (the handler's own form
/// parser refuses a repeated field, and a gate must never read a different one
/// than the handler would).
fn grant_type(body: &[u8]) -> Result<Option<String>, Malformed> {
    let mut found = form_urlencoded::parse(body).filter(|(key, _)| key == "grant_type");
    let first = found.next().map(|(_, value)| value.into_owned());
    if found.next().is_some() {
        return Err(Malformed);
    }
    Ok(first.filter(|value| !value.is_empty()))
}

struct Malformed;

/// RFC 6749 section 5.2 error body: JSON, `400`, never cached.
fn oauth_error(error: &'static str, description: &'static str) -> Response {
    let mut response = (
        StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({
            "error": error,
            "error_description": description,
        })),
    )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

/// Refuses a `/token` request whose `grant_type` this deployment does not serve,
/// with `unsupported_grant_type`, **before** the rate limiter or the provider
/// reads anything else.
///
/// Served is exactly [`ServedGrants::serves`]: `client_credentials` when
/// `machine_tokens` is served, `authorization_code` when `human_login` is, and
/// nothing else (not `refresh_token`, `device_code`, token exchange or a custom
/// grant). A request with no `grant_type`, two of them, or a body this cannot
/// read is `invalid_request`: refused, never passed on to be read differently by
/// the handler.
pub async fn enforce_served_grants(
    State(served): State<ServedGrants>,
    request: Request,
    next: Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_TOKEN_BODY_BYTES).await else {
        return oauth_error("invalid_request", "The request body could not be read");
    };
    match grant_type(&bytes) {
        Ok(Some(grant)) if served.serves(&grant) => {
            next.run(Request::from_parts(parts, Body::from(bytes)))
                .await
        }
        Ok(Some(_)) => oauth_error(
            "unsupported_grant_type",
            "This grant type is not served by this deployment",
        ),
        Ok(None) | Err(Malformed) => {
            oauth_error("invalid_request", "Exactly one grant_type is required")
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::to_bytes as read_body;
    use axum::routing::post;
    use tower::ServiceExt;

    use super::*;

    fn router(served: ServedGrants) -> Router {
        Router::new()
            .route(
                "/token",
                post(|body: axum::body::Bytes| async move { body }),
            )
            .layer(axum::middleware::from_fn_with_state(
                served,
                enforce_served_grants,
            ))
    }

    async fn post_form(router: &Router, body: &str) -> (StatusCode, serde_json::Value) {
        let response = router
            .clone()
            .oneshot(
                Request::post("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = read_body(response.into_body(), 4096).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                serde_json::Value::String(String::from_utf8_lossy(&bytes).into())
            }),
        )
    }

    const MACHINES: ServedGrants = ServedGrants {
        machine_tokens: true,
        human_login: false,
    };
    const PEOPLE: ServedGrants = ServedGrants {
        machine_tokens: false,
        human_login: true,
    };
    const BOTH: ServedGrants = ServedGrants {
        machine_tokens: true,
        human_login: true,
    };

    #[tokio::test]
    async fn a_served_grant_reaches_the_handler_with_its_body_intact() {
        let router = router(MACHINES);
        let body = "grant_type=client_credentials&client_id=appc_x&client_assertion=a.b.c";
        let response = router
            .oneshot(
                Request::post("/token")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            read_body(response.into_body(), 4096).await.unwrap(),
            body.as_bytes()
        );
    }

    #[tokio::test]
    async fn each_method_serves_its_own_grant_and_refuses_the_others() {
        for (served, grant, ok) in [
            (MACHINES, "client_credentials", true),
            (MACHINES, "authorization_code", false),
            (PEOPLE, "authorization_code", true),
            (PEOPLE, "client_credentials", false),
            (BOTH, "client_credentials", true),
            (BOTH, "authorization_code", true),
        ] {
            let (status, body) = post_form(&router(served), &format!("grant_type={grant}")).await;
            if ok {
                assert_eq!(status, StatusCode::OK, "{served:?} {grant}: {body}");
            } else {
                assert_eq!(status, StatusCode::BAD_REQUEST, "{served:?} {grant}");
                assert_eq!(
                    body["error"], "unsupported_grant_type",
                    "{served:?} {grant}"
                );
            }
        }
    }

    #[tokio::test]
    async fn the_refresh_grant_and_every_other_grant_is_unsupported_whatever_is_served() {
        for grant in [
            "refresh_token",
            "urn:ietf:params:oauth:grant-type:device_code",
            "urn:ietf:params:oauth:grant-type:token-exchange",
            "password",
            "implicit",
            "Client_Credentials",
        ] {
            let (status, body) = post_form(
                &router(BOTH),
                &format!("grant_type={}&refresh_token=x", grant.replace(':', "%3A")),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{grant}");
            assert_eq!(body["error"], "unsupported_grant_type", "{grant}");
        }
    }

    #[tokio::test]
    async fn no_grant_type_two_of_them_or_an_empty_one_is_invalid_request() {
        for body in [
            "client_id=appc_x",
            "grant_type=",
            "",
            "grant_type=client_credentials&grant_type=authorization_code",
            "grant_type=authorization_code&grant_type=authorization_code",
        ] {
            let (status, answer) = post_form(&router(BOTH), body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
            assert_eq!(answer["error"], "invalid_request", "{body:?}");
        }
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused_not_passed_on() {
        let big = format!(
            "grant_type=client_credentials&pad={}",
            "a".repeat(MAX_TOKEN_BODY_BYTES)
        );
        let (status, answer) = post_form(&router(MACHINES), &big).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(answer["error"], "invalid_request");
    }

    #[tokio::test]
    async fn a_refusal_is_never_cached() {
        let response = router(MACHINES)
            .oneshot(
                Request::post("/token")
                    .body(Body::from("grant_type=refresh_token"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }
}
