//! Turning a request into a policy-evaluable identity.
//!
//! Every `@@allow` in the schema resolves against the four fields of the
//! `auth Principal` block — `sub`, `kind`, `role`, `appId`. `hasRole('admin')`
//! reads `role`; `auth().kind == "app"` reads `kind`; `appId == auth().appId`
//! reads `appId`. [`Principal::into_context`] is the single place those names
//! are produced, so a rename in the schema breaks exactly one function.

use cratestack::{AuthProvider, CoolContext, CoolError, RequestContext, Value};

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

use authkestra_engine::strategy::utils::extract_bearer_token;
use authkestra_engine::strategy::AuthenticationStrategy;
use authkestra_resource::jwt::{JwtStrategy, ValidationConfig};
use cratestack::FilterExpr;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct OidcValidator {
    strategy: Arc<JwtStrategy<OidcClaims>>,
    db: Arc<crate::schema::Cratestack>,
    sys: CoolContext,
}

#[derive(Deserialize)]
pub struct OidcClaims {
    pub sub: String,
    pub kind: Option<String>,
    pub role: Option<String>,
    pub app_id: Option<String>,
}

impl OidcValidator {
    #[must_use]
    pub fn new(issuer: &str, db: Arc<crate::schema::Cratestack>, sys: CoolContext) -> Self {
        let config = ValidationConfig::builder()
            .jwks_url(format!("{issuer}/jwks.json"))
            .refresh_interval(std::time::Duration::from_secs(3600))
            .issuer(issuer)
            .build();

        Self {
            strategy: Arc::new(JwtStrategy::new(config)),
            db,
            sys,
        }
    }
}

impl AuthProvider for OidcValidator {
    type Error = CoolError;

    fn authenticate(
        &self,
        request: &RequestContext<'_>,
    ) -> impl core::future::Future<Output = Result<CoolContext, Self::Error>> + Send {
        let strategy = self.strategy.clone();
        let db = self.db.clone();
        let sys = self.sys.clone();
        let token = extract_bearer_token(request.headers).map(|s: &str| s.to_string());

        async move {
            let token =
                token.ok_or_else(|| CoolError::Unauthorized("missing bearer token".to_owned()))?;

            // Build dummy Parts to pass to authenticate
            let mut req = cratestack::axum::http::Request::new(());
            req.headers_mut().insert(
                cratestack::axum::http::header::AUTHORIZATION,
                cratestack::axum::http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
            let (parts, ()) = req.into_parts();

            let claims = AuthenticationStrategy::<OidcClaims>::authenticate(&*strategy, &parts)
                .await
                .map_err(|e| CoolError::Unauthorized(format!("token validation failed: {e}")))?
                .ok_or_else(|| CoolError::Unauthorized("invalid token".to_owned()))?;

            let sub = claims.sub.clone();

            // If the OP provided custom claims (e.g. Keycloak), use them.
            // Otherwise, we fallback to a DB lookup.
            if let (Some(kind), Some(role), Some(app_id)) =
                (claims.kind, claims.role, claims.app_id)
            {
                let kind_enum = if kind == "user" {
                    PrincipalKind::User
                } else {
                    PrincipalKind::App
                };
                return Ok(Principal {
                    sub,
                    kind: kind_enum,
                    role,
                    app_id,
                }
                .into_context());
            }

            // Fallback for authkestra-op which lacks custom claims mapping in client_credentials
            let clients = db
                .oauth_client()
                .find_many()
                .where_expr(FilterExpr::from(
                    crate::schema::oauth_client::clientId().eq(sub.clone()),
                ))
                .limit(1)
                .run(&sys)
                .await
                .map_err(|e| CoolError::Internal(format!("db error: {e}")))?;

            let client = clients
                .into_iter()
                .next()
                .ok_or_else(|| CoolError::Unauthorized("unknown client".to_owned()))?;

            if let Some(app_client_id) = client.appClientId {
                // It's a machine caller (App)
                let app_clients = db
                    .app_client()
                    .find_many()
                    .where_expr(FilterExpr::from(
                        crate::schema::app_client::id().eq(app_client_id.clone()),
                    ))
                    .limit(1)
                    .run(&sys)
                    .await
                    .map_err(|e| CoolError::Internal(format!("db error: {e}")))?;

                let app_client = app_clients
                    .into_iter()
                    .next()
                    .ok_or_else(|| CoolError::Unauthorized("unknown app_client".to_owned()))?;

                Ok(Principal {
                    sub,
                    kind: PrincipalKind::App,
                    role: "system".to_owned(), // Service accounts act with system privileges initially
                    app_id: app_client.appId,
                }
                .into_context())
            } else {
                // Human caller (User)
                // In milestone 0/1, we only have machine callers really, but we can default.
                Ok(Principal {
                    sub,
                    kind: PrincipalKind::User,
                    role: "admin".to_owned(),
                    app_id: String::new(),
                }
                .into_context())
            }
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

    #[tokio::test]
    async fn placeholder_test() {
        // Validation tested in integration tests
    }
}
