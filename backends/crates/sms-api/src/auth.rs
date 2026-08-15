//! Turning a request into a policy-evaluable identity.
//!
//! Every `@@allow` in the schema resolves against the four fields of the
//! `auth Principal` block — `sub`, `kind`, `role`, `appId`. `hasRole('admin')`
//! reads `role`; `auth().kind == "app"` reads `kind`; `appId == auth().appId`
//! reads `appId`. [`Principal::into_context`] is the single place those names
//! are produced, so a rename in the schema breaks exactly one function.
//!
//! # #71: this is also where the correlation id enters `CoolContext`
//!
//! `cratestack_core::CoolContext::request_id`/`with_request_id` has existed
//! since before this milestone (`cratestack-core`'s own doc: "Surfaces in
//! tracing spans and is recorded on audit events"), and every generated
//! CRUD/procedure route already logs `cratestack_request_id =
//! ctx.request_id().unwrap_or("")` (`cratestack-macros`'s
//! `list_result_log_tokens`/`dispatch_tail.rs`) — but nothing in this
//! deployment ever called `with_request_id`, so that field had been empty
//! on every single one of those log lines since the router first existed.
//! [`GatewayAuth::authenticate`] is the one place a [`CoolContext`] is
//! constructed per inbound HTTP request, so it is the natural, and only,
//! place to close that gap: honour an inbound `X-Request-Id` if the caller
//! sent one (so a client's own trace id survives into this system's logs
//! unchanged), otherwise mint a fresh one. Either way, every
//! `cratestack_*`-logged event for one HTTP request now shares one
//! `cratestack_request_id` — the correlation this crate's own custom
//! `message_id`-keyed events (`procedures.rs::send`) sit alongside, not a
//! replacement for them: a request id ties together everything logged
//! *within* this one process for *this* request; `message_id` is what
//! survives into `sms-worker`'s dispatch and the DLR ingestion path,
//! neither of which shares this process or this request. See
//! `docs/runbooks/alerting.md`'s own "Correlating a message end to end"
//! section for the worked example joining both.

use std::time::Duration;

use authkestra_engine::auth::strategy::utils::extract_bearer_token;
use authkestra_engine::token::Claims;
use authkestra_resource::jwt::{validate_jwt_generic, JwksCache};
use cratestack::{AuthProvider, CoolContext, CoolError, FilterExpr, RequestContext, Value};
use jsonwebtoken::{Algorithm, Validation};

use crate::cache::TtlCache;
use crate::schema::{self, app_client, role, user, Cratestack};

/// The `OauthClient.clientId` #194's own `sms-console` provisioning
/// (`sms-gateway seed-console-client`) registers under, and the default
/// every construction site passes as `GatewayAuth::new`'s `human_client_id`
/// unless a deployment genuinely runs a different console client id. Named
/// once here rather than duplicated as a string literal at each of the
/// (as of this PR) seven call sites across three crates.
pub const DEFAULT_CONSOLE_CLIENT_ID: &str = "sms-console";

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
/// **Both realms as of #194.** A service-account (`client_credentials`)
/// token, per `authkestra-op`'s real `handle_client_credentials` path,
/// never carries `identity` — that remains the discriminator this uses, not
/// a `kind` claim (nothing mints one on that path; see the design doc
/// §5.3's own token shape and `authkestra_engine::token::Claims`'s real
/// fields). A human (`authorization_code`) token always has one — see
/// `sms-auth::login`'s own module doc for how the identity behind it was
/// established, and `sms-auth::op::machine_only_config` for why the OP now
/// advertises that grant at all.
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
    // #194: a human token's role/perms are looked up here rather than
    // baked into the token at issuance — see this field's own doc on
    // `human_client_id` below for why `authkestra-op`'s real
    // `handle_authorization_code` made that decision for this crate, not
    // the other way around. Keyed by `User.id` (== `Identity.external_id`
    // == the token's `sub`), same TTL and same reasoning as `app_cache`.
    user_cache: std::sync::Arc<TtlCache<String, HumanPrincipal>>,
    /// The one `OauthClient.clientId` a human (`authorization_code`) token
    /// may legitimately carry as `aud` — `sms-console`, in practice. Real
    /// audience validation for that realm, closing the gap this type's own
    /// previous revision flagged: `validation.validate_aud` stays `false`
    /// globally (service-account tokens have no fixed audience to check —
    /// `aud == sub == client_id`, §4.2), so this is a manual, post-decode
    /// check that runs *only* on the `claims.identity.is_some()` branch,
    /// where a fixed audience genuinely exists to check against.
    human_client_id: String,
    db: Cratestack,
    sys: CoolContext,
}

/// What `GatewayAuth` needs about a human token's subject, cached under
/// [`GatewayAuth::user_cache`] — deliberately not a raw [`schema::User`],
/// mirroring `sms-auth::login::AuthenticatedUser`'s own reasoning (a
/// cache should hold exactly what its one caller needs, not an entire row
/// a future field addition would silently start caching).
#[derive(Debug, Clone)]
struct HumanPrincipal {
    role_key: String,
    perms: Vec<String>,
}

impl GatewayAuth {
    /// `jwks_url` is the OP's own `/jwks.json` (`sms-auth::op` stands it
    /// up); `issuer` must match `OpConfig.issuer` exactly, or every token
    /// this OP mints fails `iss` validation against itself. `human_client_id`
    /// is the `OauthClient.clientId` the console's `authorization_code`
    /// flow registers under (see [`GatewayAuth`]'s own field doc).
    #[must_use]
    pub fn new(db: Cratestack, jwks_url: String, issuer: String, human_client_id: String) -> Self {
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
        // §4.2) — there is no fixed value to check it against, so this
        // stays `false` globally rather than set via `set_audience`. A
        // human token's `aud` **is** fixed (`human_client_id`, always
        // "sms-console" in practice) and **is** checked — just not here:
        // `jsonwebtoken::Validation` applies one audience list to every
        // token it decodes, and this type decodes both realms with one
        // `Validation`, so a per-realm check has to run after decode, in
        // `authenticate` itself, once `claims.identity` says which realm
        // this token is. Verified live (not merely inferred from the
        // library's docs) that leaving `validate_aud` at its `true`
        // default while never calling `set_audience` does NOT skip the
        // check the way "nothing configured" might suggest —
        // `jsonwebtoken::Validation`'s default still rejects any token
        // carrying an `aud` claim with `InvalidAudience` when `self.aud`
        // is empty. The explicit `false` below is the only real off
        // switch, and `authenticate`'s own manual check is what restores
        // the guarantee for the one realm that has a fixed audience.
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
            // Same 60s TTL and reasoning as app_cache — a deactivated or
            // role-changed User takes effect within one TTL window rather
            // than waiting out the access token's own 15-minute lifetime,
            // which is *more* responsive than §5.3's own "roles resolve at
            // issuance... up to 15 minutes to bite" framing describes.
            // That framing assumed role/perms baked into the token at
            // issuance (`issue_user_token_with_extra`); see
            // `authenticate`'s own doc on why the real library shape ruled
            // that out and made this per-request lookup the mechanism
            // instead, not a choice made lightly.
            user_cache: std::sync::Arc::new(TtlCache::new(Duration::from_mins(1))),
            human_client_id,
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
    type Error = CoolError;

    fn authenticate(
        &self,
        request: &RequestContext<'_>,
    ) -> impl core::future::Future<Output = Result<CoolContext, Self::Error>> + Send {
        let token = extract_bearer_token(request.headers).map(str::to_owned);
        // #71: extracted synchronously, alongside `token` above, for the
        // same reason — `request` itself is a borrow tied to this call's
        // stack frame and cannot cross into the `async move` block below.
        let request_id = request_id_from(request.headers);
        let jwks = self.jwks.clone();
        let validation = self.validation.clone();
        let app_cache = self.app_cache.clone();
        let user_cache = self.user_cache.clone();
        let human_client_id = self.human_client_id.clone();
        let db = self.db.clone();
        let sys = self.sys.clone();

        async move {
            let token =
                token.ok_or_else(|| CoolError::Unauthorized("no credentials".to_owned()))?;
            let claims: Claims = validate_jwt_generic(&token, &jwks, &validation)
                .await
                .map_err(|error| CoolError::Unauthorized(format!("invalid token: {error}")))?;

            // Layer 2 (#24, §5.1): read now, while `claims` is still whole —
            // `scope` is a first-class field, `perms` lives in `extra`. Both
            // survive into the returned `CoolContext` below regardless of
            // whether either is actually present; an absent claim becoming
            // an absent/empty context field, rather than the request
            // failing here, is what lets `require_permission` be the one
            // place that decides what "no permission" means.
            let scope = claims.scope.clone();

            let ctx = if claims.identity.is_some() {
                authenticate_human(claims, &human_client_id, &user_cache, &db, &sys).await?
            } else {
                authenticate_app(claims, &app_cache, &db, &sys).await?
            };

            let mut ctx = ctx;
            let perms = match ctx.extensions.remove("__perms") {
                Some(Value::List(items)) => items,
                _ => Vec::new(),
            };
            // Stashed in `extensions`, not folded into `into_context`'s own
            // four `auth.fields` — those four are Layer 1's vocabulary
            // (`hasRole`/`inTenant`/`appId ==`), fixed by the schema's own
            // `auth Principal` block (auth.rs's module doc). `perms`/`scope`
            // are Layer 2's, read only by `require_permission`, never by a
            // generated SQL policy — keeping them out of `auth.fields` means
            // a typo'd extra claim can never accidentally start matching a
            // `hasRole(...)` clause it was never meant to.
            ctx.extensions
                .insert("perms".to_owned(), Value::List(perms));
            ctx.extensions
                .insert("scope".to_owned(), scope.map_or(Value::Null, Value::String));

            // #71: see this module's own doc. `with_request_id` returns
            // `Self`, so this has to be the last thing done to `ctx` —
            // consistent with `into_context()` already being called first
            // above.
            let ctx = ctx.with_request_id(request_id);

            Ok(ctx)
        }
    }
}

/// The service-account (`client_credentials`) path — unchanged behaviour
/// from before #194, pulled out of `authenticate` so that function reads as
/// "pick a realm, then project it" rather than one long branch. `perms` for
/// this realm is `extra["perms"]` (never actually set on this path today —
/// see [`extract_perms`]'s own doc), stashed under the same `"__perms"`
/// sentinel key `authenticate_human` uses, for `authenticate` to promote
/// into the real `"perms"` extension alongside `scope` once both realms
/// have returned.
async fn authenticate_app(
    claims: Claims,
    app_cache: &TtlCache<String, schema::AppClient>,
    db: &Cratestack,
    sys: &CoolContext,
) -> Result<CoolContext, CoolError> {
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
        // Never "system" — that role is constructed exactly once, inside
        // `Procedures::sys()`, never from a token. `"app"` is a sentinel
        // that matches no `hasRole(...)` clause on any model in the
        // schema: a machine caller's only access is the seven procedures,
        // never generated CRUD. See `OauthSigningKey`'s own schema comment
        // for why this specifically must never slip.
        role: "app".to_owned(),
        app_id: app_client.appId,
    }
    .into_context();
    ctx.extensions.insert(
        "__perms".to_owned(),
        Value::List(perms.into_iter().map(Value::String).collect()),
    );
    Ok(ctx)
}

/// The human (`authorization_code`) path, new in #194.
///
/// # Why this looks up `User`/`Role` per request rather than reading role
/// and `perms` straight off the token, the way §5.3's own token-shape
/// table describes
///
/// Read directly, not assumed: `authkestra-op` 0.3.3's
/// `handle_authorization_code` (`authkestra_op::handlers::token`) issues
/// the access token with plain `tokens.issue_user_token(auth_code.identity,
/// ...)` — never `issue_user_token_with_extra`, the only method that can
/// stamp additional claims onto a token at issuance. So there is no library
/// hook this crate can reach to put `role`/`perms` on the token the way the
/// design doc's own worked example assumed; forking that handler to add one
/// would mean re-implementing its PKCE/redirect-uri/client-binding checks
/// ourselves, exactly the kind of security-critical duplication this
/// codebase avoids elsewhere (see `sms-auth::login`'s own module doc on the
/// same tradeoff, made the same way). A per-request lookup, cached the same
/// way `app_cache` already caches `AppClient`, needs none of that — and, as
/// a real upside rather than a consolation, makes a role change or account
/// deactivation take effect within one cache TTL instead of waiting out the
/// access token's full 15-minute lifetime, which is *more* responsive than
/// what baking into the token would have given.
///
/// # Fail closed on every branch
///
/// A human token whose `aud` doesn't match `human_client_id`, whose
/// `identity.external_id` names no `User` row, or whose account is
/// inactive (or soft-deleted — excluded from the read itself, see
/// `schema.cstack`'s own `@@soft_delete` note and this crate's read-source
/// verification) is `Unauthorized`. There is no fallback role.
async fn authenticate_human(
    claims: Claims,
    human_client_id: &str,
    user_cache: &TtlCache<String, HumanPrincipal>,
    db: &Cratestack,
    sys: &CoolContext,
) -> Result<CoolContext, CoolError> {
    // Real, per-realm audience validation — see `GatewayAuth::new`'s own
    // doc on why this can't live in the shared `Validation` both realms
    // decode through.
    //
    // `Audience::contains`, not string equality. authkestra 0.5 changed
    // `Claims::aud` from `Option<String>` to `Option<Audience>`, an
    // untagged `Single(String) | Multiple(Vec<String>)`, and its own doc
    // says `contains` is "the 'matches ANY' membership test that replaces
    // exact-string-equality comparisons against `aud`".
    //
    // That is a real semantic change and it is the correct one: RFC 7519
    // §4.1.3 makes a token valid when its `aud` *contains* the intended
    // recipient, and `aud` is permitted to be an array. Under 0.3.3's
    // `Option<String>` an array-valued `aud` did not deserialize at all,
    // so such a token was rejected as malformed rather than evaluated —
    // accidental strictness, not a policy this repo chose. Membership is
    // both spec-correct and strictly better-defined.
    //
    // It does mean a token minted for `["sms-console", "something-else"]`
    // now authenticates here. That is intended: such a token *was* issued
    // for this audience. Nothing in this deployment mints a multi-audience
    // token today (`sms_auth::op` issues one `aud` per client), so the
    // widening is theoretical against our own OP and correct against any
    // future one.
    if !claims
        .aud
        .as_ref()
        .is_some_and(|aud| aud.contains(human_client_id))
    {
        return Err(CoolError::Unauthorized(
            "human token audience mismatch".to_owned(),
        ));
    }

    let identity = claims
        .identity
        .ok_or_else(|| CoolError::Unauthorized("human token missing identity".to_owned()))?;
    let subject = identity.external_id;

    let principal = user_cache
        .get_or_fetch(subject.clone(), |subject| {
            let db = db.clone();
            let sys = sys.clone();
            async move { load_human_principal(&db, &sys, &subject).await }
        })
        .await?;

    let mut ctx = Principal {
        sub: subject,
        kind: PrincipalKind::User,
        role: principal.role_key,
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions.insert(
        "__perms".to_owned(),
        Value::List(principal.perms.into_iter().map(Value::String).collect()),
    );
    Ok(ctx)
}

/// `User` (active, not soft-deleted — see [`authenticate_human`]'s own
/// doc) by id, plus its `Role`'s unpacked `permissions` — the one query
/// [`authenticate_human`]'s cache actually runs on a miss.
///
/// # Errors
///
/// [`CoolError::Unauthorized`] for anything that must not distinguish
/// "no such user" from "deactivated" from "role deleted out from under
/// them" to the caller — a revoked/renamed account must fail exactly like
/// one that never existed, the same fail-closed posture
/// `sms-auth::login::authenticate_user` already takes for the initial
/// login itself.
async fn load_human_principal(
    db: &Cratestack,
    sys: &CoolContext,
    subject: &str,
) -> Result<HumanPrincipal, CoolError> {
    let user_row = db
        .user()
        .find_many()
        .where_expr(
            FilterExpr::from(user::id().eq(subject.to_owned())).and(user::active().is_true()),
        )
        .limit(1)
        .run(sys)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| CoolError::Unauthorized("unknown or inactive user".to_owned()))?;

    let role_row = db
        .role()
        .find_many()
        .where_expr(FilterExpr::from(role::key().eq(user_row.roleKey)))
        .limit(1)
        .run(sys)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| CoolError::Unauthorized("user's role no longer exists".to_owned()))?;

    // Fail closed at the point of use — the second of two independent
    // guards, deliberately not the only one (see RESERVED_ROLE_KEYS' own
    // doc for the first, a database-level CHECK). `Role.key`'s @regex
    // (`^[a-z][a-z0-9_]{2,31}$`) happily accepts the literal "system": a
    // Role row keyed "system", assigned to a human User via ordinary
    // owner/admin-level generated CRUD (Role.create is hasRole('owner'),
    // User.update is owner/admin), would otherwise project straight into
    // Principal.role, and hasRole('system') would then match for that
    // human exactly the way it matches for the real, synthetic system
    // context — reading OauthSigningKey.privateKeyPem (the key that signs
    // every token this system issues) and every UserCredential.passwordHash
    // through generated CRUD. AGENTS.md's own §5.2 states the invariant
    // this exists to protect: "system is not a row in roles... constructible
    // only inside a process." This check is what makes that true even if
    // the CHECK constraint below is ever bypassed (a raw migration, a
    // future admin tool that writes past R1) — redundant with the CHECK
    // on the common path, not redundant with the invariant itself, so
    // don't remove either half without re-reading this comment.
    if RESERVED_ROLE_KEYS.contains(&role_row.key.as_str()) {
        return Err(CoolError::Unauthorized(
            "unknown or inactive user".to_owned(),
        ));
    }

    Ok(HumanPrincipal {
        role_key: role_row.key,
        perms: sms_core::unpack(&role_row.permissions)
            .into_iter()
            .map(str::to_owned)
            .collect(),
    })
}

/// `Role.key` values no human account may ever carry, checked in
/// [`load_human_principal`] and enforced a second, independent way by a
/// `CHECK` constraint on `roles.key` (`backends/migrations/postgres/0002_bootstrap`,
/// generated from `docs/architecture.md` §2.10 — never hand-edited).
///
/// - `"system"` is the one that matters: it's the literal `hasRole(...)`
///   compares against for the synthetic internal context every procedure's
///   own `sys()`/`system_context()` constructs — see this file's own
///   [`PrincipalKind`] doc and §5.2 of the design doc. A `Role` row keyed
///   `"system"` would let `hasRole('system')` match for a real human.
/// - `"app"` is reserved too, defensively, even though it is not
///   currently a privilege escalation: [`authenticate_app`]'s own
///   `role: "app"` sentinel matches no `hasRole(...)` clause in the schema
///   (`OauthSigningKey`'s own comment is explicit about this), so a human
///   `Role` keyed `"app"` would be confusing, not exploitable — reserved
///   to keep it that way rather than relying on that fact never changing.
const RESERVED_ROLE_KEYS: &[&str] = &["system", "app"];

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
            DEFAULT_CONSOLE_CLIENT_ID.to_owned(),
        )
    }

    #[tokio::test]
    async fn a_request_with_no_bearer_token_is_unauthorized() {
        let headers = cratestack::axum::http::HeaderMap::new();
        // cratestack 0.7.13 (cratestack#552): `RequestContext` gained an
        // `extensions` field. Nothing in this test needs one to exist
        // (`GatewayAuth::authenticate` never reads it), so an empty value
        // is fine — see `rbac.rs`'s identical comment for the live version
        // of this same construction.
        let extensions = cratestack::axum::http::Extensions::new();
        let request = RequestContext {
            method: "GET",
            path: "/messages",
            query: None,
            headers: &headers,
            body: &[],
            extensions: &extensions,
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
        // cratestack 0.7.13 (cratestack#552): see the identical comment on
        // the test above.
        let extensions = cratestack::axum::http::Extensions::new();
        let request = RequestContext {
            method: "GET",
            path: "/messages",
            query: None,
            headers: &headers,
            body: &[],
            extensions: &extensions,
        };
        let error = gateway_auth().authenticate(&request).await.unwrap_err();
        assert!(matches!(error, CoolError::Unauthorized(_)));
    }
}
