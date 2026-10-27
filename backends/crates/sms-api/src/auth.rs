#![doc = include_str!("auth.md")]

use cratestack::{
    AuthProvider, CratestackContext, CratestackError, RequestContext, Value, VerifiedSigner,
};

use crate::signers::{SignerDirectory, Standing};

/// Who a request is. Mirrors the `auth Principal` block in `schema.cstack`
/// field for field — the names below are load-bearing, not cosmetic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// Subject. A user id for humans, a `client_id` for services.
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
    /// A human, authenticated by a paired console key.
    User,
    /// A service account, authenticated by a registered client key.
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
    pub fn into_context(self) -> CratestackContext {
        CratestackContext::authenticated([
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

/// Turns the key that signed a request into the [`Principal`] the schema's
/// policies read. This is the only `AuthProvider` the generated router has.
///
/// There is no credential in a request but its signature. The envelope layer
/// (`crate::envelope`) has already verified it and recorded a
/// `VerifiedSigner` in the request extensions; [`SignedAuth`] maps that signer
/// through [`SignerDirectory`] and reads nothing else, so there is no token to
/// validate, no JWKS to fetch and no `Authorization` header to trust:
///
/// - a `ClientKey` of an active `AppClient` is an **app** principal
///   (`sub = AppClient.clientId`, `role = "app"`, the client's scopes);
/// - a paired, unexpired `ConsoleKey` of an active `User` is a **user**
///   principal, that person's role and the permissions of that role.
///
/// A request that carries an `Authorization` header is refused outright, even
/// beside a good signature, rather than ignored: a stale client must not
/// believe a bearer still counts, and nothing downstream may start reading
/// one. A request with no verified signer cannot reach here (the envelope is
/// `Required` for every op); if one ever did, it is refused rather than
/// treated as anonymous, because no policy in this schema admits an
/// unauthenticated caller.
#[derive(Clone)]
pub struct SignedAuth {
    directory: SignerDirectory,
}

impl SignedAuth {
    /// An auth provider answering from `directory`.
    #[must_use]
    pub fn new(directory: SignerDirectory) -> Self {
        Self { directory }
    }

    /// The context of `standing`, with Layer 2's `perms` and `scope` set.
    ///
    /// `perms` and `scope` are stashed in `extensions`, not folded into
    /// [`Principal::into_context`]'s four `auth.fields`: those are Layer 1's
    /// vocabulary (`hasRole`, `inTenant`, `appId ==`), fixed by the schema's
    /// own `auth Principal` block, and the extras are read only by
    /// `require_permission`. Keeping them out of `auth.fields` means a typo'd
    /// extra can never start matching a `hasRole(...)` clause it was never
    /// meant to.
    ///
    /// ```
    /// use sms_api::auth::SignedAuth;
    /// use sms_api::signers::{AppStanding, Standing};
    ///
    /// let ctx = SignedAuth::context_for(Standing::App(AppStanding {
    ///     client_id: "appc_demo".to_owned(),
    ///     app_id: "app1".to_owned(),
    ///     scopes: " sms:send ".to_owned(),
    /// }));
    /// assert!(sms_api::require_permission(&ctx, "sms:send").is_ok());
    /// assert!(sms_api::require_permission(&ctx, "job:read").is_err());
    /// ```
    #[must_use]
    pub fn context_for(standing: Standing) -> CratestackContext {
        let (principal, perms, scope) = match standing {
            Standing::App(app) => (
                Principal {
                    sub: app.client_id,
                    kind: PrincipalKind::App,
                    // Never "system": that role is minted only by
                    // `system_context`, never from a signer. `"app"` is a
                    // sentinel that matches no `hasRole(...)` clause on any
                    // model: a machine caller's only access is the
                    // procedures, never generated CRUD.
                    role: "app".to_owned(),
                    app_id: app.app_id,
                },
                Vec::new(),
                Value::String(app.scopes),
            ),
            Standing::User(person) => (
                Principal {
                    sub: person.user_id,
                    kind: PrincipalKind::User,
                    role: person.role,
                    app_id: String::new(),
                },
                person.perms,
                Value::Null,
            ),
        };
        let mut ctx = principal.into_context();
        ctx.extensions.insert(
            "perms".to_owned(),
            Value::List(perms.into_iter().map(Value::String).collect()),
        );
        ctx.extensions.insert("scope".to_owned(), scope);
        ctx
    }
}

/// A caller-supplied `X-Request-Id` is bounded and reused verbatim — the
/// same trace id a caller already emits into its own logs should be the
/// one that shows up in this system's, not a second, unrelated one, and
/// honouring it is what makes end-to-end tracing actually end to end
/// rather than starting at this system's own edge. Absent, empty, or
/// implausibly long (a client bug, or a header this deployment should not
/// trust verbatim into every `cratestack_*` log line) falls back to a
/// freshly minted one instead of guessing or truncating.
const MAX_INBOUND_REQUEST_ID_LEN: usize = 200;

/// See [`MAX_INBOUND_REQUEST_ID_LEN`]'s own doc.
fn request_id_from(headers: &cratestack::axum::http::HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= MAX_INBOUND_REQUEST_ID_LEN)
        .map_or_else(
            || cratestack::uuid::Uuid::new_v4().to_string(),
            str::to_owned,
        )
}

impl AuthProvider for SignedAuth {
    type Error = CratestackError;

    fn authenticate(
        &self,
        request: &RequestContext<'_>,
    ) -> impl core::future::Future<Output = Result<CratestackContext, Self::Error>> + Send {
        // Read out of the borrow first: the future owns what it needs.
        let bearer = request
            .headers
            .contains_key(cratestack::axum::http::header::AUTHORIZATION);
        let signer = request.extensions.get::<VerifiedSigner>().cloned();
        // #71: see this module's own doc.
        let request_id = request_id_from(request.headers);
        let directory = self.directory.clone();

        async move {
            if bearer {
                return Err(CratestackError::BadRequest(
                    "bearer credentials are not accepted: sign the request (`COSE_Sign1`)"
                        .to_owned(),
                ));
            }
            let signer = signer
                .ok_or_else(|| CratestackError::Unauthorized("no verified signer".to_owned()))?;
            let standing = directory.standing(&signer, chrono::Utc::now()).await?;
            // #71: `with_request_id` returns `Self`, so it is the last thing
            // done to the context.
            Ok(Self::context_for(standing).with_request_id(request_id))
        }
    }
}

/// The one place that mints the `system`-role context — the most
/// privileged construct in this system (it is what every registered
/// `ClientKey` and `ConsoleKey` write is gated behind). Every internal
/// system-context write in this workspace —
/// `Procedures::sys()`, `webhooks.rs`'s own subscriber context, the
/// signer directory's key and standing lookups, `sms-worker`'s `jobs` role,
/// `sms-gateway`'s operator commands, and `send_test_message.rs`'s
/// standalone tool — calls this function rather than hand-rolling the
/// same [`Principal`] literal, so the invariant below has exactly one
/// place to hold, not six independently-drifting copies of it.
///
/// **The invariant: this must never be reachable from an HTTP request.**
/// Nothing in [`SignedAuth::authenticate`] ever produces `role: "system"`
/// for a real signer — [`SignedAuth::context_for`] hardcodes `role: "app"`
/// for an app key (see the comment there on why `"system"` specifically
/// must never slip), and `crate::signers` refuses a person whose
/// `Role.key` is `"system"` (or `"app"`) before a `Principal` is ever built
/// from it, backed a second, independent way by
/// the database itself (`roles_key_not_reserved_check`, a `CHECK`
/// constraint on `roles.key` — see [`RESERVED_ROLE_KEYS`]'s own doc for
/// both layers). This function is the only source of a `role: "system"`
/// context anywhere in this codebase; it exists in a process's own
/// internal wiring, never behind a route a token can reach.
///
/// `sub` is the one thing that legitimately differs per caller — a
/// human-readable label naming which subsystem is acting, not a secret or
/// a policy input. It shows up verbatim in `cratestack_audit.actor.sub`
/// on every write this context makes, which is the entire reason each
/// call site passes its own value (`"sms-api:procedures"`,
/// `"sms-worker:jobs:{worker}"`, …) rather than a shared constant — an
/// audit row naming which caller performed a system-level write is worth
/// more than one fewer function argument.
#[must_use]
pub fn system_context(sub: impl Into<String>) -> CratestackContext {
    Principal {
        sub: sub.into(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// `Role.key` values no human account may ever carry, checked in
/// `crate::signers` and enforced a second, independent way by a
/// `CHECK` constraint on `roles.key` (`backends/migrations/postgres/0002_bootstrap`,
/// generated from `docs/architecture.md` §2.10 — never hand-edited).
///
/// - `"system"` is the one that matters: it's the literal `hasRole(...)`
///   compares against for the synthetic internal context [`system_context`]
///   constructs — see this file's own [`PrincipalKind`] doc and §5.2 of
///   the design doc. A `Role` row keyed `"system"` would let
///   `hasRole('system')` match for a real human.
/// - `"app"` is reserved too, defensively, even though it is not
///   currently a privilege escalation: [`SignedAuth::context_for`]'s
///   `role: "app"` sentinel matches no `hasRole(...)` clause in the schema,
///   so a human
///   `Role` keyed `"app"` would be confusing, not exploitable — reserved
///   to keep it that way rather than relying on that fact never changing.
pub const RESERVED_ROLE_KEYS: &[&str] = &["system", "app"];

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

    /// A lazy pool only parses the URL and never connects. The tests below
    /// all fail before the directory is asked anything, so no database is
    /// reached.
    fn signed_auth() -> SignedAuth {
        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        SignedAuth::new(SignerDirectory::new(
            crate::schema::Cratestack::builder(pool).build(),
        ))
    }

    fn request<'a>(
        headers: &'a cratestack::axum::http::HeaderMap,
        extensions: &'a cratestack::axum::http::Extensions,
    ) -> RequestContext<'a> {
        RequestContext {
            method: "GET",
            path: "/messages",
            query: None,
            headers,
            body: &[],
            extensions,
        }
    }

    #[tokio::test]
    async fn a_request_with_no_verified_signer_is_unauthorized() {
        let headers = cratestack::axum::http::HeaderMap::new();
        let extensions = cratestack::axum::http::Extensions::new();
        let error = signed_auth()
            .authenticate(&request(&headers, &extensions))
            .await
            .unwrap_err();
        assert!(matches!(error, CratestackError::Unauthorized(_)));
    }

    /// One caller, one credential: a bearer is refused even beside a verified
    /// signature, and even from a caller who signed nothing, so no code path
    /// can come to treat an `Authorization` header as an identity again.
    #[tokio::test]
    async fn an_authorization_header_is_refused_with_or_without_a_signer() {
        let mut headers = cratestack::axum::http::HeaderMap::new();
        headers.insert(
            cratestack::axum::http::header::AUTHORIZATION,
            "Bearer anything".parse().unwrap(),
        );
        for signed in [true, false] {
            let mut extensions = cratestack::axum::http::Extensions::new();
            if signed {
                extensions.insert(VerifiedSigner::new(vec![0_u8; 8], [7_u8; 32], -19));
            }
            let error = signed_auth()
                .authenticate(&request(&headers, &extensions))
                .await
                .unwrap_err();
            assert!(
                matches!(&error, CratestackError::BadRequest(message)
                    if message.contains("bearer credentials are not accepted")),
                "{error:?}"
            );
        }
    }

    #[test]
    fn an_app_key_is_an_app_principal_with_its_scopes_and_no_permissions() {
        let ctx = SignedAuth::context_for(Standing::App(crate::signers::AppStanding {
            client_id: "appc_demo".to_owned(),
            app_id: "app1".to_owned(),
            scopes: "message:send job:read".to_owned(),
        }));
        assert_eq!(
            ctx.auth_field("sub"),
            Some(&Value::String("appc_demo".to_owned()))
        );
        assert_eq!(
            ctx.auth_field("kind"),
            Some(&Value::String("app".to_owned()))
        );
        assert_eq!(
            ctx.auth_field("role"),
            Some(&Value::String("app".to_owned()))
        );
        assert_eq!(
            ctx.auth_field("appId"),
            Some(&Value::String("app1".to_owned()))
        );
        assert!(crate::rbac::require_permission(&ctx, "job:read").is_ok());
        assert!(crate::rbac::require_permission(&ctx, "provider:update").is_err());
    }

    #[test]
    fn a_console_key_is_a_user_principal_with_its_roles_permissions_and_no_scope() {
        let ctx = SignedAuth::context_for(Standing::User(crate::signers::UserStanding {
            user_id: "usr1".to_owned(),
            role: "operator".to_owned(),
            perms: vec!["provider:update".to_owned()],
        }));
        assert_eq!(
            ctx.auth_field("sub"),
            Some(&Value::String("usr1".to_owned()))
        );
        assert_eq!(
            ctx.auth_field("kind"),
            Some(&Value::String("user".to_owned()))
        );
        assert_eq!(
            ctx.auth_field("role"),
            Some(&Value::String("operator".to_owned()))
        );
        assert_eq!(ctx.auth_field("appId"), Some(&Value::String(String::new())));
        assert!(crate::rbac::require_permission(&ctx, "provider:update").is_ok());
        assert!(crate::rbac::require_permission(&ctx, "job:read").is_err());
    }

    /// `system` is minted by [`system_context`] and by nothing a signer can
    /// reach: neither standing produces it, whatever the row said.
    #[test]
    fn no_standing_ever_produces_the_system_role() {
        for ctx in [
            SignedAuth::context_for(Standing::App(crate::signers::AppStanding {
                client_id: "system".to_owned(),
                app_id: "system".to_owned(),
                scopes: String::new(),
            })),
            SignedAuth::context_for(Standing::User(crate::signers::UserStanding {
                user_id: "system".to_owned(),
                role: "operator".to_owned(),
                perms: Vec::new(),
            })),
        ] {
            assert_ne!(
                ctx.auth_field("role"),
                Some(&Value::String("system".to_owned()))
            );
        }
    }
}
