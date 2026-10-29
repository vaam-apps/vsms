#![doc = include_str!("hmac_gate.md")]

use cratestack::axum::body::{Body, to_bytes};
use cratestack::axum::extract::{Request, State};
use cratestack::axum::http::{HeaderMap, Method};
use cratestack::axum::middleware::Next;
use cratestack::axum::response::Response;
use cratestack::{CratestackContext, CratestackError, FilterExpr, decode_transport_request_for};

use crate::owner_grant::{as_client_error, route_capabilities};
use crate::rbac::{error_response, path_matches};
use crate::router::ROUTER_CODEC;
use crate::schema::{self, Cratestack, WebhookDeliveryMode, webhook_endpoint};

/// The word a refused request leads with: the stable part of the 422's message,
/// since the envelope's `code` is `VALIDATION_ERROR` for every 422 and a client
/// switches on this instead (the way `idempotency_key_conflict` is matched).
pub const AUTH_METHOD_DISABLED: &str = "auth_method_disabled";

/// The reason the worker records on an `hmac` delivery it parks
/// (`WebhookAttempt.lastError` starts with it).
pub const DELIVERY_MODE_DISABLED: &str = "delivery_mode_disabled";

/// Whether this deployment permits `hmac_webhooks` (`auth.methods`, VSA-1).
///
/// A two-valued type rather than a `bool` because the router takes it beside
/// five other positional arguments, and `router(.., true)` does not say which
/// way it fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HmacWebhooks {
    /// `auth.methods` permits `hmac_webhooks` (the default when the key is
    /// absent): behaviour is what it was before the key existed.
    Permitted,
    /// `auth.methods` does not: nothing creates, switches to or rotates an
    /// `hmac` endpoint, and the worker sends none.
    Disabled,
}

impl HmacWebhooks {
    /// From `AuthMethods::permits(AuthMethod::HmacWebhooks)`. `sms-api` does not
    /// depend on `sms-config`, so each binary that reads the YAML makes this one
    /// call.
    #[must_use]
    pub const fn from_permitted(permitted: bool) -> Self {
        if permitted {
            Self::Permitted
        } else {
            Self::Disabled
        }
    }

    /// Whether `hmac_webhooks` is permitted.
    #[must_use]
    pub const fn is_permitted(self) -> bool {
        matches!(self, Self::Permitted)
    }

    /// `Ok` when `hmac_webhooks` is permitted; otherwise the 422 that says what
    /// was refused and how to proceed.
    ///
    /// # Errors
    ///
    /// [`CratestackError::Validation`] (HTTP 422) whose message starts with
    /// [`AUTH_METHOD_DISABLED`].
    pub fn require(self, what: &str) -> Result<(), CratestackError> {
        match self {
            Self::Permitted => Ok(()),
            Self::Disabled => Err(CratestackError::Validation(format!(
                "{AUTH_METHOD_DISABLED}: this deployment does not permit hmac_webhooks \
                 (auth.methods), so {what}. Use a sealed endpoint"
            ))),
        }
    }
}

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

/// Every generated route that writes a `WebhookEndpoint` with a caller-supplied
/// body. `DELETE /webhook_endpoints/{id}` carries none.
const GUARDED_ROUTES: &[GuardedRoute] = &[
    GuardedRoute {
        method: Method::POST,
        path: "/webhook_endpoints",
        shape: Shape::Create,
    },
    GuardedRoute {
        method: Method::PATCH,
        path: "/webhook_endpoints/{id}",
        shape: Shape::Update,
    },
];

/// Decodes `body` as the route's own input type and refuses it when it makes an
/// `hmac` endpoint. Pure: no I/O, no authentication, and no knowledge of whether
/// the method is permitted, which the caller decided.
///
/// A create is refused when it names `hmac`. An update is refused when it names
/// `hmac` **or sets a secret**: a `secret` or `prevSecret` is meaningful only on
/// an `hmac` row (a CHECK refuses one on a sealed row), so writing one is
/// configuring `hmac` whatever the body says about the mode. An update that
/// touches neither (`active`, `eventTypes`, a sealed audience) is allowed, so an
/// operator who turned the method off can still deactivate, retarget or delete
/// the endpoints it left behind.
fn refuse_hmac_body(
    route: &GuardedRoute,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), CratestackError> {
    let capabilities = route_capabilities(&route.method, route.path).ok_or_else(|| {
        // A guarded route the schema no longer generates: refuse rather than
        // forward a body this guard cannot read.
        CratestackError::Internal(format!(
            "hmac gate: no generated route {} {}",
            route.method, route.path
        ))
    })?;

    let sets_hmac = match route.shape {
        Shape::Create => {
            decode_transport_request_for::<_, schema::CreateWebhookEndpointInput>(
                &ROUTER_CODEC,
                headers,
                &capabilities,
                body,
            )
            .map_err(as_client_error)?
            .deliveryMode
                == WebhookDeliveryMode::hmac
        }
        Shape::Update => {
            let input = decode_transport_request_for::<_, schema::UpdateWebhookEndpointInput>(
                &ROUTER_CODEC,
                headers,
                &capabilities,
                body,
            )
            .map_err(as_client_error)?;
            input.deliveryMode == Some(WebhookDeliveryMode::hmac)
                || matches!(input.secret, Some(Some(_)))
                || matches!(input.prevSecret, Some(Some(_)))
        }
    };

    if sets_hmac {
        HmacWebhooks::Disabled.require(match route.shape {
            Shape::Create => "an hmac endpoint cannot be created",
            Shape::Update => "an endpoint cannot be given an hmac mode or a signing secret",
        })
    } else {
        Ok(())
    }
}

/// Wraps the whole generated router (see `router::router`). A no-op unless
/// `hmac_webhooks` is off **and** the request is a guarded `WebhookEndpoint`
/// write: then it buffers the body up to `DEFAULT_BODY_LIMIT_BYTES`, runs
/// [`refuse_hmac_body`], and forwards a request rebuilt from the original bytes.
///
/// It sits just outside the generated router and inside the RBAC layers, so a
/// caller without `webhook:manage` is told so (403) before being told anything
/// about the deployment's methods, and a caller that is not signed never gets
/// this far. It does not authenticate by itself for that reason.
pub async fn enforce_hmac_gate(
    State(hmac): State<HmacWebhooks>,
    request: Request,
    next: Next,
) -> Response {
    if hmac.is_permitted() {
        return next.run(request).await;
    }
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

    if let Err(error) = refuse_hmac_body(route, &parts.headers, &bytes) {
        return error_response(&error);
    }

    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

/// How many `hmac` endpoints exist (active or not).
///
/// # Errors
///
/// Whatever the aggregate returns.
pub async fn count_hmac_endpoints(
    db: &Cratestack,
    sys: &CratestackContext,
) -> Result<i64, CratestackError> {
    db.webhook_endpoint()
        .aggregate()
        .count()
        .where_expr(FilterExpr::from(
            webhook_endpoint::deliveryMode().eq(WebhookDeliveryMode::hmac),
        ))
        .run(sys)
        .await
}

/// What `serve` and the `hooks` worker say at boot about the `hmac` endpoints:
/// how many there are and what this process will do about them.
///
/// The words and the severity are decided here, once; each process emits them
/// itself ([`Self::is_warning`], [`Self::message`]) because a log line's target is
/// the crate that emits it, and the worker's default filter
/// (`sms_worker=info,sms_worker_bin=info`, also set by the compose file and the
/// chart) does not include `sms_api`: a line emitted from here would be filtered
/// out of exactly the process that parks the deliveries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HmacBoot {
    /// What `auth.methods` says.
    pub hmac: HmacWebhooks,
    /// How many `hmac` endpoints exist, active or not.
    pub count: i64,
}

impl HmacBoot {
    /// Counts the `hmac` endpoints and pairs the count with `hmac`.
    ///
    /// # Errors
    ///
    /// Whatever [`count_hmac_endpoints`] returns. A caller logs it and starts
    /// anyway: a boot line is not worth refusing to start for.
    pub async fn read(
        db: &Cratestack,
        sys: &CratestackContext,
        hmac: HmacWebhooks,
    ) -> Result<Self, CratestackError> {
        Ok(Self {
            hmac,
            count: count_hmac_endpoints(db, sys).await?,
        })
    }

    /// Whether an operator needs to act: `hmac` endpoints exist and the method is
    /// off, so their deliveries are parked and nothing more can be done to their
    /// mode or secret until they are switched to sealed or deleted. Every other
    /// case is information: the method permitted (so an operator sees what turning
    /// it off would cost) or off with nothing left behind.
    #[must_use]
    pub const fn is_warning(&self) -> bool {
        !self.hmac.is_permitted() && self.count > 0
    }

    /// The sentence, for `info!` or `warn!` per [`Self::is_warning`].
    #[must_use]
    pub fn message(&self) -> String {
        match (self.hmac, self.count) {
            (HmacWebhooks::Permitted, _) => {
                "hmac webhook endpoints (auth.methods permits hmac_webhooks)".to_owned()
            }
            (HmacWebhooks::Disabled, 0) => {
                "no hmac webhook endpoints exist and auth.methods does not permit hmac_webhooks"
                    .to_owned()
            }
            (HmacWebhooks::Disabled, _) => format!(
                "auth.methods does not permit hmac_webhooks but hmac webhook endpoints exist: \
                 their deliveries are parked as failed ({DELIVERY_MODE_DISABLED}), not sent. \
                 Switch them to sealed (switchWebhookDeliveryMode) or delete them; a parked \
                 delivery is replayable"
            ),
        }
    }
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

    fn create_body(mode: &str) -> Vec<u8> {
        let mut body = serde_json::json!({
            "appId": "capp0000000000000000000",
            "url": "https://example.test/hooks",
            "deliveryMode": mode,
            "eventTypes": "message.delivered",
            "maskRecipient": true,
            "maxAttempts": 8,
        });
        if mode == "hmac" {
            body["secret"] = key_of_32().into();
        } else {
            body["audience"] = "receiver.example".into();
        }
        body.to_string().into_bytes()
    }

    /// 32 characters, the least a key written through the API may have.
    fn key_of_32() -> String {
        "k".repeat(32)
    }

    fn refusal(error: &CratestackError) -> &str {
        match error {
            CratestackError::Validation(message) => message,
            other => panic!("expected the 422, got {other:?}"),
        }
    }

    #[test]
    fn a_create_naming_hmac_is_a_422_that_leads_with_the_stable_word() {
        let error = refuse_hmac_body(
            create_route(),
            &headers("application/json"),
            &create_body("hmac"),
        )
        .unwrap_err();
        assert_eq!(error.status_code().as_u16(), 422);
        assert!(
            refusal(&error).starts_with("auth_method_disabled: "),
            "{error}"
        );
    }

    #[test]
    fn a_sealed_create_is_allowed() {
        assert!(
            refuse_hmac_body(
                create_route(),
                &headers("application/json"),
                &create_body("sealed")
            )
            .is_ok()
        );
    }

    #[test]
    fn an_update_that_names_hmac_or_sets_a_secret_is_refused() {
        let key = key_of_32();
        for body in [
            r#"{"deliveryMode":"hmac"}"#.to_owned(),
            format!(r#"{{"secret":"{key}"}}"#),
            format!(r#"{{"prevSecret":"{key}"}}"#),
            format!(r#"{{"deliveryMode":"hmac","secret":"{key}"}}"#),
        ] {
            let error = refuse_hmac_body(
                update_route(),
                &headers("application/json"),
                body.as_bytes(),
            )
            .unwrap_err();
            assert!(
                refusal(&error).starts_with("auth_method_disabled: "),
                "{body}: {error}"
            );
        }
    }

    #[test]
    fn an_update_that_touches_neither_the_mode_nor_a_secret_is_allowed() {
        // What an operator who turned the method off still needs: to deactivate,
        // retarget or re-subscribe the endpoints it left behind, and to move one
        // to sealed.
        for body in [
            br#"{"active":false}"#.as_slice(),
            br#"{"eventTypes":"message.delivered"}"#,
            br#"{"url":"https://example.test/other"}"#,
            br#"{"deliveryMode":"sealed","audience":"receiver.example","secret":null}"#,
            br"{}",
        ] {
            assert!(
                refuse_hmac_body(update_route(), &headers("application/json"), body).is_ok(),
                "{} was refused",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn an_undecodable_body_is_refused_not_forwarded() {
        for body in [
            b"".as_slice(),
            b"not json",
            b"[1,2]",
            br#"{"deliveryMode":7}"#,
        ] {
            assert!(
                matches!(
                    refuse_hmac_body(update_route(), &headers("application/json"), body),
                    Err(CratestackError::BadRequest(_))
                ),
                "{} was not refused with 400",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn a_missing_content_type_is_refused() {
        assert!(matches!(
            refuse_hmac_body(update_route(), &HeaderMap::new(), br#"{"active":false}"#),
            Err(CratestackError::UnsupportedMediaType(_))
        ));
    }

    #[test]
    fn a_permitted_deployment_requires_nothing() {
        assert!(HmacWebhooks::Permitted.require("anything").is_ok());
        assert!(HmacWebhooks::Permitted.is_permitted());
        assert!(!HmacWebhooks::Disabled.is_permitted());
    }

    #[test]
    fn the_boot_line_warns_only_when_endpoints_are_left_behind_with_the_method_off() {
        let boot = |hmac, count| HmacBoot { hmac, count };
        assert!(boot(HmacWebhooks::Disabled, 3).is_warning());
        assert!(!boot(HmacWebhooks::Disabled, 0).is_warning());
        assert!(!boot(HmacWebhooks::Permitted, 3).is_warning());
        assert!(!boot(HmacWebhooks::Permitted, 0).is_warning());
        let parked = boot(HmacWebhooks::Disabled, 3).message();
        assert!(parked.contains(DELIVERY_MODE_DISABLED), "{parked}");
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

    /// The reason `GUARDED_ROUTES` is a list nobody can forget to extend: a new
    /// generated route that writes a `WebhookEndpoint` (a batch or upsert route,
    /// say) fails here until it is guarded or explicitly classified.
    #[test]
    fn every_generated_webhook_endpoint_write_route_is_guarded() {
        // Routes under `/webhook_endpoints` that write but carry no body to read:
        // DELETE removes the row, and the attempts go with it.
        const BODYLESS: &[(&str, &str)] = &[("DELETE", "/webhook_endpoints/{id}")];

        for (method, path) in crate::router::route_table() {
            let is_endpoint_path =
                path == "/webhook_endpoints" || path.starts_with("/webhook_endpoints/");
            if !is_endpoint_path || method == "GET" {
                continue;
            }
            let guarded = GUARDED_ROUTES
                .iter()
                .any(|route| route.method.as_str() == method && route.path == path);
            let bodyless = BODYLESS.contains(&(method, path));
            assert!(
                guarded || bodyless,
                "{method} {path} writes a WebhookEndpoint and is neither guarded nor classified"
            );
        }
    }
}
