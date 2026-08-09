//! Turning a request into a policy-evaluable identity.
//!
//! Every `@@allow` in the schema resolves against the four fields of the
//! `auth Principal` block — `sub`, `kind`, `role`, `appId`. `hasRole('admin')`
//! reads `role`; `auth().kind == "app"` reads `kind`; `appId == auth().appId`
//! reads `appId`. [`Principal::into_context`] is the single place those names
//! are produced, so a rename in the schema breaks exactly one function.

use std::time::Duration;

use authkestra_engine::auth::strategy::utils::extract_bearer_token;
use authkestra_engine::token::Claims;
use authkestra_resource::jwt::{validate_jwt_generic, JwksCache};
use cratestack::{AuthProvider, CoolContext, CoolError, FilterExpr, RequestContext, Value};
use jsonwebtoken::{Algorithm, Validation};

use crate::cache::TtlCache;
use crate::schema::{self, app_client, Cratestack};

/// Who a request is. Mirrors the `auth Principal` block in `schema.cstack`
/// field for field — the names below are load-bearing, not cosmetic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// OIDC subject. A user id for humans, a `client_id` for services.
    pub sub: String,
    /// `"user"` or `"app"`. Splits human callers from machine callers in
    /// policy; `"system"` is the worker's role, not a kind.
    pub kind: PrincipalKind,
    /// Role key — `owner`, `admin`, `operator`, `auditor`, `developer`,
    /// `system`. Matched by `hasRole(...)`.
    pub role: String,
    /// The app a machine caller acts for. Empty for human callers, which never
    /// match an `appId == auth().appId` clause.
    pub app_id: String,
}

/// The two kinds of caller the policies distinguish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalKind {
    /// A human, authenticated by authorization code + PKCE.
    User,
    /// A service account, authenticated by `client_credentials`.
    App,
}

impl PrincipalKind {
    /// The literal the schema compares against.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            PrincipalKind::User => "user",
            PrincipalKind::App => "app",
        }
    }
}

impl Principal {
    /// Project into the context the policy engine evaluates.
    ///
    /// Note the interaction with `@@allow("create", hasRole('system'))`: a
    /// worker must set `role = "system"` *and* a `kind`, because several models
    /// gate reads on `auth().kind` and writes on `hasRole('system')`. Setting
    /// only one of the two denies every message write, which is the failure
    /// mode §13 of the architecture doc calls out by name.
    #[must_use]
    pub fn into_context(self) -> CoolContext {
        CoolContext::authenticated([
            ("sub".to_owned(), Value::String(self.sub)),
            (
                "kind".to_owned(),
                Value::String(self.kind.as_str().to_owned()),
            ),
            ("role".to_owned(), Value::String(self.role)),
            ("appId".to_owned(), Value::String(self.app_id)),
        ])
    }
}

/// JWKS-backed token validation, replacing the milestone-0 `DenyAll`. #21.
///
/// Scoped to `client_credentials` + `private_key_jwt` — the only caller
/// type that exists in this system (no admin console yet, so no human ever
/// authenticates via the authorization-code flow; see `sms-auth::op`'s own
/// module doc for the full reasoning). A service-account token, per
/// `authkestra-op`'s real `handle_client_credentials` path, never carries
/// `identity` — that's the discriminator this uses, not a `kind` claim
/// (nothing mints one on this path; see the design doc §5.3's own token
/// shape and `authkestra_engine::token::Claims`'s real fields).
///
/// Builds its own [`JwksCache`]/[`Validation`] directly rather than going
/// through `authkestra_resource::jwt::JwtStrategy` — verified against
/// vendored 0.3.2 source that `JwtStrategy`'s cache/validation fields are
/// private, with no accessor, so its own `AuthenticationStrategy::authenticate`
/// (which wants an `http::request::Parts`, not this crate's
/// [`RequestContext`]) is the only way to reach them through that type.
/// [`extract_bearer_token`] plus [`validate_jwt_generic`] are the same two
/// calls `JwtStrategy` itself makes internally, just callable directly
/// against a `&HeaderMap`.
#[derive(Clone)]
pub struct GatewayAuth {
    jwks: std::sync::Arc<JwksCache>,
    validation: Validation,
    app_cache: std::sync::Arc<TtlCache<String, schema::AppClient>>,
    db: Cratestack,
    sys: CoolContext,
}

impl GatewayAuth {
    /// `jwks_url` is the OP's own `/jwks.json` (`sms-auth::op` stands it
    /// up); `issuer` must match `OpConfig.issuer` exactly, or every token
    /// this OP mints fails `iss` validation against itself.
    #[must_use]
    pub fn new(db: Cratestack, jwks_url: String, issuer: String) -> Self {
        // `require_kid(true)`: §5.4's own reasoning, re-verified against
        // 0.3.2 — the default `false` falls back to `jwks.keys[0]` on a
        // missing `kid`, which the moment two keys are published during
        // rotation (`sms-auth::op::ROTATION_OVERLAP`) picks wrong close to
        // half the time.
        let jwks =
            std::sync::Arc::new(JwksCache::new(jwks_url, Duration::from_mins(5)).require_kid(true));
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[issuer]);
        // A service-account token's `aud` echoes `sub` (the client_id,
        // §4.2) — there is no fixed value to check it against. Leaving
        // `set_audience` uncalled is *not* enough to skip the check:
        // verified live that `jsonwebtoken::Validation`'s default
        // `validate_aud: true` still rejects a token carrying an `aud`
        // claim with `InvalidAudience` when `self.aud` is empty, rather
        // than treating "nothing configured" as "nothing to check" — the
        // explicit `false` below is the actual off switch.
        //
        // This disables audience checking for every token this
        // `Validation` ever sees, not just service-account ones — a
        // correctness gap only if a human-login flow starts issuing
        // tokens with a real `aud` (that flow doesn't exist yet, see this
        // type's own doc). It's not a live hole today: `authenticate`
        // rejects any token carrying `identity` before audience would
        // ever matter, on a completely separate check. Revisit this line
        // — restore a real per-audience check, most likely via a second
        // `Validation` for the human-login path — before wiring
        // `identity`-bearing tokens up to anything.
        validation.validate_aud = false;
        Self {
            jwks,
            validation,
            // 60s: short enough that a revoked/retired AppClient stops
            // being usable promptly, long enough that a hot client_id
            // isn't a database hit per request — same tradeoff and TTL
            // `Procedures::resolve_app` already makes for the same lookup
            // shape.
            app_cache: std::sync::Arc::new(TtlCache::new(Duration::from_mins(1))),
            db,
            sys: system_context(),
        }
    }
}

/// The `system`-role context this provider's own `AppClient` lookup runs
/// under — never handed to a caller, only used internally the same way
/// `Procedures::sys()` is.
fn system_context() -> CoolContext {
    Principal {
        sub: "sms-api:auth".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// The `perms` claim (§5.3's human/role-token shape), pulled out of
/// [`Claims::extra`] — it isn't a first-class field on that type, only
/// `scope` is. Not reachable from any token this deployment can currently
/// issue (see [`GatewayAuth`]'s own doc: every accepted token is a
/// `client_credentials` one, and nothing in `sms_auth::op` ever sets a
/// `perms` claim), but extracted unconditionally rather than special-cased
/// away — a future human-login issuer only has to start setting the claim,
/// not wait on a second change here. Absent, or present but not an array of
/// strings, both become `vec![]`: [`require_permission`]'s job is to treat
/// "no perms" as denial, not this extraction's.
fn extract_perms(extra: &std::collections::HashMap<String, serde_json::Value>) -> Vec<String> {
    extra
        .get("perms")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

impl AuthProvider for GatewayAuth {
    type Error = CoolError;

    fn authenticate(
        &self,
        request: &RequestContext<'_>,
    ) -> impl core::future::Future<Output = Result<CoolContext, Self::Error>> + Send {
        let token = extract_bearer_token(request.headers).map(str::to_owned);
        let jwks = self.jwks.clone();
        let validation = self.validation.clone();
        let app_cache = self.app_cache.clone();
        let db = self.db.clone();
        let sys = self.sys.clone();

        async move {
            let token =
                token.ok_or_else(|| CoolError::Unauthorized("no credentials".to_owned()))?;
            let claims: Claims = validate_jwt_generic(&token, &jwks, &validation)
                .await
                .map_err(|error| CoolError::Unauthorized(format!("invalid token: {error}")))?;

            if claims.identity.is_some() {
                // Nothing on the client_credentials path this OP serves
                // ever sets `identity` (see this type's own doc) — a token
                // that has one was minted by something else, or isn't one
                // this deployment should trust.
                return Err(CoolError::Unauthorized(
                    "human authentication is not wired up in this deployment".to_owned(),
                ));
            }

            // Layer 2 (#24, §5.1): read now, while `claims` is still whole —
            // `scope` is a first-class field, `perms` lives in `extra`. Both
            // survive into the returned `CoolContext` below regardless of
            // whether either is actually present; an absent claim becoming
            // an absent/empty context field, rather than the request
            // failing here, is what lets `require_permission` be the one
            // place that decides what "no permission" means.
            let scope = claims.scope.clone();
            let perms = extract_perms(&claims.extra);

            let client_id = claims.sub;
            let app_client = app_cache
                .get_or_fetch(client_id.clone(), |client_id| {
                    let db = db.clone();
                    let sys = sys.clone();
                    async move {
                        db.app_client()
                            .find_many()
                            .where_expr(
                                FilterExpr::from(app_client::clientId().eq(client_id))
                                    .and(app_client::active().is_true()),
                            )
                            .limit(1)
                            .run(&sys)
                            .await?
                            .into_iter()
                            .next()
                            .ok_or_else(|| CoolError::Unauthorized("unknown client".to_owned()))
                    }
                })
                .await?;

            let mut ctx = Principal {
                sub: client_id,
                kind: PrincipalKind::App,
                // Never "system" — that role is constructed exactly once,
                // inside `Procedures::sys()`, never from a token. `"app"`
                // is a sentinel that matches no `hasRole(...)` clause on
                // any model in the schema: a machine caller's only access
                // is the seven procedures, never generated CRUD. See
                // `OauthSigningKey`'s own schema comment for why this
                // specifically must never slip.
                role: "app".to_owned(),
                app_id: app_client.appId,
            }
            .into_context();

            // Stashed in `extensions`, not folded into `into_context`'s own
            // four `auth.fields` — those four are Layer 1's vocabulary
            // (`hasRole`/`inTenant`/`appId ==`), fixed by the schema's own
            // `auth Principal` block (auth.rs's module doc). `perms`/`scope`
            // are Layer 2's, read only by `require_permission`, never by a
            // generated SQL policy — keeping them out of `auth.fields` means
            // a typo'd extra claim can never accidentally start matching a
            // `hasRole(...)` clause it was never meant to.
            ctx.extensions.insert(
                "perms".to_owned(),
                Value::List(perms.into_iter().map(Value::String).collect()),
            );
            ctx.extensions
                .insert("scope".to_owned(), scope.map_or(Value::Null, Value::String));

            Ok(ctx)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal() -> Principal {
        Principal {
            sub: "user_abc".to_owned(),
            kind: PrincipalKind::User,
            role: "admin".to_owned(),
            app_id: String::new(),
        }
    }

    #[test]
    fn context_carries_every_field_the_schema_policies_reference() {
        let ctx = principal().into_context();
        for field in ["sub", "kind", "role", "appId"] {
            assert!(
                ctx.auth_field(field).is_some(),
                "policies read auth().{field}"
            );
        }
    }

    #[test]
    fn role_is_readable_the_way_has_role_reads_it() {
        // `hasRole('x')` is `ctx.auth_field("role") == Value::String("x")`.
        let ctx = principal().into_context();
        assert_eq!(
            ctx.auth_field("role"),
            Some(&Value::String("admin".to_owned()))
        );
    }

    #[test]
    fn kind_serialises_to_the_literals_the_schema_compares_against() {
        assert_eq!(PrincipalKind::User.as_str(), "user");
        assert_eq!(PrincipalKind::App.as_str(), "app");
    }

    /// A lazy pool only parses the URL — never connects — matching the
    /// same pattern `router.rs`'s own test uses. Neither test below ever
    /// reaches a database query: both fail during token validation itself,
    /// before `authenticate` would ever touch `AppClient`.
    fn gateway_auth() -> GatewayAuth {
        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        GatewayAuth::new(
            schema::Cratestack::builder(pool).build(),
            "https://auth.invalid/jwks.json".to_owned(),
            "https://auth.invalid".to_owned(),
        )
    }

    #[tokio::test]
    async fn a_request_with_no_bearer_token_is_unauthorized() {
        let headers = cratestack::axum::http::HeaderMap::new();
        let request = RequestContext {
            method: "GET",
            path: "/messages",
            query: None,
            headers: &headers,
            body: &[],
        };
        let error = gateway_auth().authenticate(&request).await.unwrap_err();
        assert!(matches!(error, CoolError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn a_malformed_bearer_token_is_unauthorized_without_a_jwks_fetch() {
        // "not-a-jwt" fails `decode_header` before validate_jwt_generic
        // ever reaches the network for JWKS — this test would hang against
        // "https://auth.invalid" otherwise, which is the point: a garbage
        // token must be rejected on its own shape, not by however the
        // network call to a real OP happens to fail.
        let mut headers = cratestack::axum::http::HeaderMap::new();
        headers.insert(
            cratestack::axum::http::header::AUTHORIZATION,
            "Bearer not-a-jwt".parse().unwrap(),
        );
        let request = RequestContext {
            method: "GET",
            path: "/messages",
            query: None,
            headers: &headers,
            body: &[],
        };
        let error = gateway_auth().authenticate(&request).await.unwrap_err();
        assert!(matches!(error, CoolError::Unauthorized(_)));
    }
}
