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

/// The milestone-0 authenticator: refuses everything.
///
/// Token validation is milestone 1 (`sms-auth`: JWKS, RS256, the custom
/// `ClientStore` that works around the `GrantType` serde bug). Until that
/// exists there is no way to establish who a caller is, and the only correct
/// answer to "who is this?" is that we cannot tell.
///
/// This is deliberately not a header-trusting development provider. One that
/// reads `x-role: owner` would make the whole policy layer decorative, and
/// would be exactly the kind of thing that survives to production because
/// nothing fails when it does.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAll;

impl AuthProvider for DenyAll {
    type Error = CoolError;

    fn authenticate(
        &self,
        _request: &RequestContext<'_>,
    ) -> impl core::future::Future<Output = Result<CoolContext, Self::Error>> + Send {
        core::future::ready(Err(CoolError::Unauthorized(
            "authentication is not configured; sms-auth lands in milestone 1".to_owned(),
        )))
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
    async fn deny_all_rejects_even_a_well_formed_request() {
        let headers = cratestack::axum::http::HeaderMap::new();
        let request = RequestContext {
            method: "GET",
            path: "/messages",
            query: None,
            headers: &headers,
            body: &[],
        };
        let error = DenyAll.authenticate(&request).await.unwrap_err();
        assert!(matches!(error, CoolError::Unauthorized(_)));
    }
}
