#![doc = include_str!("auth.md")]

use cratestack::{
    AuthProvider, CratestackContext, CratestackError, RequestContext, Value, VerifiedSigner,
};

use crate::bearer::BearerCaller;
use crate::signers::{SignerDirectory, Standing, UserStanding};

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

/// Turns the credential a request was admitted with into the [`Principal`] the
/// schema's policies read. This is the only `AuthProvider` the generated router
/// has.
///
/// There are two ways in, and each is verified by a layer **before** this
/// provider runs, which records the fact in the request's extensions:
///
/// - the envelope layer (`crate::envelope`) opens a `COSE_Sign1` message and
///   records a `VerifiedSigner`; [`GatewayAuth`] maps that signer through
///   [`SignerDirectory`];
/// - the bearer door (`crate::bearer`) verifies a machine's or a person's
///   token, resolves who it names from rows, and records a [`BearerCaller`]
///   holding the [`Standing`].
///
/// Both end in the same [`Standing`] and the same [`GatewayAuth::context_for`], so
/// a policy cannot tell them apart:
///
/// - a `ClientKey` of an active `AppClient`, or a token of an active
///   `OauthClient` linked to one, is an **app** principal (`sub =
///   AppClient.clientId`, `role = "app"`, the client's scopes);
/// - a paired, unexpired `ConsoleKey` of an active `User`, or an access token
///   `human_login` minted for one, is a **user** principal: that person's role
///   and the permissions of that role. **Except** that a person's token never
///   stands as an `owner` or an `admin` ([`DEVICE_ONLY_ROLES`]): those roles act
///   only from a paired console key, and the token is refused with a 403
///   ([`DEVICE_REQUIRED`]) by [`refuse_device_only_role`], at the door and again
///   here.
///
/// Nothing here reads an `Authorization` header as an identity. A request that
/// carries one beside a verified signature is refused outright (400), and one
/// that carries one the door did not record authenticates nobody. A request
/// with no recorded credential cannot reach here (the door answers it 401
/// first); if one ever did, it is refused rather than treated as anonymous,
/// because no policy in this schema admits an unauthenticated caller.
#[derive(Clone)]
pub struct GatewayAuth {
    directory: SignerDirectory,
}

impl GatewayAuth {
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
    /// use sms_api::auth::GatewayAuth;
    /// use sms_api::signers::{AppStanding, Standing};
    ///
    /// let ctx = GatewayAuth::context_for(Standing::App(AppStanding {
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

impl AuthProvider for GatewayAuth {
    type Error = CratestackError;

    fn authenticate(
        &self,
        request: &RequestContext<'_>,
    ) -> impl core::future::Future<Output = Result<CratestackContext, Self::Error>> + Send {
        // Read out of the borrow first: the future owns what it needs.
        let authorization = request
            .headers
            .contains_key(cratestack::axum::http::header::AUTHORIZATION);
        let signer = request.extensions.get::<VerifiedSigner>().cloned();
        let bearer = request.extensions.get::<BearerCaller>().cloned();
        // #71: see this module's own doc.
        let request_id = request_id_from(request.headers);
        let directory = self.directory.clone();

        async move {
            let standing = match (signer, bearer) {
                // Both doors vouched for one request: neither is believed. The
                // bearer door never records a caller for a signed request, so
                // this is a bug somewhere above, not a client to be helped.
                (Some(_), Some(_)) => {
                    return Err(CratestackError::Unauthorized(
                        "more than one verified credential".to_owned(),
                    ));
                }
                // One caller, one credential: refused beside a good signature,
                // the same answer the bearer door gives first. Defence in depth
                // for a request that reaches an extractor some other way.
                (Some(_), None) if authorization => {
                    return Err(CratestackError::BadRequest(
                        "a signed request cannot also carry an Authorization header: send one \
                         credential"
                            .to_owned(),
                    ));
                }
                (Some(signer), None) => directory.standing(&signer, chrono::Utc::now()).await?,
                // Resolved by the bearer door from rows, not from the token.
                // A person's token never carries a device-only role. Guard 1
                // (`SignerDirectory::person_standing`) already runs on the value
                // read from the standing cache, which the signed door shares and
                // where an admin is legitimate, so it covers that cache. This is
                // guard 2, the same function again, a safety net: if guard 1 were
                // ever "simplified" into the cache loader, a standing a key just
                // cached would be served to a token from the cache and only this
                // would catch it (proven by moving guard 1 into the loader). Keep
                // both, and keep guard 1 after the cache read.
                (None, Some(caller)) => {
                    if let Standing::User(person) = &caller.0 {
                        refuse_device_only_role(person)?;
                    }
                    caller.0
                }
                (None, None) => {
                    return Err(CratestackError::Unauthorized(
                        "no verified credential".to_owned(),
                    ));
                }
            };
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
/// Nothing in [`GatewayAuth::authenticate`] ever produces `role: "system"`
/// for a real signer — [`GatewayAuth::context_for`] hardcodes `role: "app"`
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
///   currently a privilege escalation: [`GatewayAuth::context_for`]'s
///   `role: "app"` sentinel matches no `hasRole(...)` clause in the schema,
///   so a human
///   `Role` keyed `"app"` would be confusing, not exploitable — reserved
///   to keep it that way rather than relying on that fact never changing.
pub const RESERVED_ROLE_KEYS: &[&str] = &["system", "app"];

/// `Role.key` values that act only from a paired console key, never from a
/// person's access token (VSA-4b).
///
/// A user has exactly one `roleKey` and `hasRole(...)` matches one string, so a
/// token cannot be handed "the role minus `owner` and `admin`": it is the
/// person's role or nothing. A password proves one factor and a token outlives
/// the page that holds it; `owner` and `admin` provision clients, pair keys and
/// promote people, so those roles ask for the device that holds a key the
/// server has seen: a paired `ConsoleKey`. Every other role a person's token carries as its row says.
pub const DEVICE_ONLY_ROLES: &[&str] = &["owner", "admin"];

/// The 403 body's text for a person whose role acts only from a paired console
/// key. It names no role: the caller has just proved who they are, and the
/// answer says what to do, not how the policy is laid out.
pub const DEVICE_REQUIRED: &str =
    "this account's role acts only from a paired console key; a signed-in session cannot carry it";

/// Refuses a person's token standing when the role is one of
/// [`DEVICE_ONLY_ROLES`].
///
/// The **one** function behind both guards: `SignerDirectory::bearer_standing`
/// calls it when it builds the standing, and [`GatewayAuth::authenticate`]
/// calls it again on the standing the door recorded. It is
/// [`CratestackError::Forbidden`] and not `Unauthorized`: the person is who they
/// say, and the door answers 403 with [`DEVICE_REQUIRED`], where a refused
/// credential gets the coarse 401.
///
/// ```
/// use sms_api::auth::{DEVICE_REQUIRED, refuse_device_only_role};
/// use sms_api::signers::UserStanding;
///
/// let person = |role: &str| UserStanding {
///     user_id: "usr1".to_owned(),
///     role: role.to_owned(),
///     perms: Vec::new(),
/// };
/// assert!(refuse_device_only_role(&person("operator")).is_ok());
/// assert_eq!(
///     refuse_device_only_role(&person("admin")).unwrap_err().to_string(),
///     cratestack::CratestackError::Forbidden(DEVICE_REQUIRED.to_owned()).to_string(),
/// );
/// ```
///
/// # Errors
///
/// [`CratestackError::Forbidden`] carrying [`DEVICE_REQUIRED`].
pub fn refuse_device_only_role(person: &UserStanding) -> Result<(), CratestackError> {
    if DEVICE_ONLY_ROLES.contains(&person.role.as_str()) {
        return Err(CratestackError::Forbidden(DEVICE_REQUIRED.to_owned()));
    }
    Ok(())
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

    fn person(role: &str) -> UserStanding {
        UserStanding {
            user_id: "usr1".to_owned(),
            role: role.to_owned(),
            perms: vec!["sms:read".to_owned()],
        }
    }

    /// The function both guards call: `owner` and `admin` are refused with the
    /// 403 text, every other role passes, and the 403 names no role.
    #[test]
    fn a_device_only_role_is_refused_and_every_other_passes() {
        assert_eq!(DEVICE_ONLY_ROLES, ["owner", "admin"]);
        for role in DEVICE_ONLY_ROLES {
            assert!(
                matches!(
                    refuse_device_only_role(&person(role)),
                    Err(CratestackError::Forbidden(text)) if text == DEVICE_REQUIRED
                ),
                "{role}"
            );
            assert!(!DEVICE_REQUIRED.contains(role), "the text names no role");
        }
        for role in [
            "operator",
            "auditor",
            "developer",
            "support",
            "Admin",
            "owner ",
        ] {
            assert!(refuse_device_only_role(&person(role)).is_ok(), "{role}");
        }
    }

    /// The second guard, on its own: a standing the door recorded for an `admin`
    /// is a 403 at the provider even though nothing upstream stopped it, and an
    /// `operator`'s authenticates as that person.
    #[tokio::test]
    async fn the_provider_refuses_a_recorded_admin_standing_on_its_own() {
        use cratestack::axum::http::{Extensions, HeaderMap};

        async fn authenticate(
            auth: &GatewayAuth,
            role: &str,
        ) -> Result<CratestackContext, CratestackError> {
            let mut extensions = Extensions::new();
            extensions.insert(BearerCaller(Standing::User(person(role))));
            let headers = HeaderMap::new();
            let request = RequestContext {
                method: "GET",
                path: "/probe",
                query: None,
                headers: &headers,
                body: &[],
                extensions: &extensions,
            };
            auth.authenticate(&request).await
        }

        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        let auth = GatewayAuth::new(SignerDirectory::new(
            crate::schema::Cratestack::builder(pool).build(),
        ));
        assert!(matches!(
            authenticate(&auth, "admin").await,
            Err(CratestackError::Forbidden(text)) if text == DEVICE_REQUIRED
        ));
        assert!(matches!(
            authenticate(&auth, "owner").await,
            Err(CratestackError::Forbidden(_))
        ));
        let ctx = authenticate(&auth, "operator")
            .await
            .expect("an operator acts");
        assert_eq!(
            ctx.auth_field("role"),
            Some(&Value::String("operator".to_owned()))
        );
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
    fn signed_auth() -> GatewayAuth {
        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        GatewayAuth::new(SignerDirectory::new(
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

    /// One caller, one credential: an `Authorization` header beside a verified
    /// signature is refused, whichever door the header was meant for.
    #[tokio::test]
    async fn an_authorization_header_beside_a_signature_is_refused() {
        let mut headers = cratestack::axum::http::HeaderMap::new();
        headers.insert(
            cratestack::axum::http::header::AUTHORIZATION,
            "Bearer anything".parse().unwrap(),
        );
        let mut extensions = cratestack::axum::http::Extensions::new();
        extensions.insert(VerifiedSigner::new(vec![0_u8; 8], [7_u8; 32], -19));
        let error = signed_auth()
            .authenticate(&request(&headers, &extensions))
            .await
            .unwrap_err();
        assert!(
            matches!(&error, CratestackError::BadRequest(message)
                if message.contains("cannot also carry an Authorization header")),
            "{error:?}"
        );
    }

    /// A header is not an identity: with nothing in the extensions the door
    /// recorded, `Authorization: Bearer ...` authenticates nobody, so no code
    /// path can come to read the header itself.
    #[tokio::test]
    async fn an_authorization_header_alone_authenticates_nobody() {
        let mut headers = cratestack::axum::http::HeaderMap::new();
        headers.insert(
            cratestack::axum::http::header::AUTHORIZATION,
            "Bearer anything".parse().unwrap(),
        );
        let extensions = cratestack::axum::http::Extensions::new();
        let error = signed_auth()
            .authenticate(&request(&headers, &extensions))
            .await
            .unwrap_err();
        assert!(
            matches!(error, CratestackError::Unauthorized(_)),
            "{error:?}"
        );
    }

    /// Two credentials vouched for one request: neither is believed.
    #[tokio::test]
    async fn a_signer_and_a_bearer_caller_together_are_refused() {
        let headers = cratestack::axum::http::HeaderMap::new();
        let mut extensions = cratestack::axum::http::Extensions::new();
        extensions.insert(VerifiedSigner::new(vec![0_u8; 8], [7_u8; 32], -19));
        extensions.insert(BearerCaller(app_standing("appc_demo", "sms:send")));
        let error = signed_auth()
            .authenticate(&request(&headers, &extensions))
            .await
            .unwrap_err();
        assert!(
            matches!(error, CratestackError::Unauthorized(_)),
            "{error:?}"
        );
    }

    fn app_standing(client_id: &str, scopes: &str) -> Standing {
        Standing::App(crate::signers::AppStanding {
            client_id: client_id.to_owned(),
            app_id: "app1".to_owned(),
            scopes: format!(" {scopes} "),
        })
    }

    /// The bearer door's caller is the same context a signed request from the
    /// same client gets: one `context_for`, so no policy can tell the doors
    /// apart. Compared field by field and through Layer 2.
    #[tokio::test]
    async fn a_bearer_caller_is_the_context_a_signature_from_the_same_client_gets() {
        let standing = app_standing("appc_demo", "sms:send job:read");
        let headers = cratestack::axum::http::HeaderMap::new();
        let mut extensions = cratestack::axum::http::Extensions::new();
        extensions.insert(BearerCaller(standing.clone()));
        let from_bearer = signed_auth()
            .authenticate(&request(&headers, &extensions))
            .await
            .expect("a recorded bearer caller authenticates");
        let from_standing = GatewayAuth::context_for(standing);
        for field in ["sub", "kind", "role", "appId"] {
            assert_eq!(
                from_bearer.auth_field(field),
                from_standing.auth_field(field)
            );
        }
        // The rest of the extensions are Layer 2's; the request id is the one
        // thing that is per request, and each context mints its own.
        for key in ["perms", "scope"] {
            assert_eq!(
                from_bearer.extensions.get(key),
                from_standing.extensions.get(key)
            );
        }
        assert!(crate::rbac::require_permission(&from_bearer, "job:read").is_ok());
        assert!(crate::rbac::require_permission(&from_bearer, "provider:update").is_err());
    }

    #[test]
    fn an_app_key_is_an_app_principal_with_its_scopes_and_no_permissions() {
        let ctx = GatewayAuth::context_for(Standing::App(crate::signers::AppStanding {
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
        let ctx = GatewayAuth::context_for(Standing::User(crate::signers::UserStanding {
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
            GatewayAuth::context_for(Standing::App(crate::signers::AppStanding {
                client_id: "system".to_owned(),
                app_id: "system".to_owned(),
                scopes: String::new(),
            })),
            GatewayAuth::context_for(Standing::User(crate::signers::UserStanding {
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
