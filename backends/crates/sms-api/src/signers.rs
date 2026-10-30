#![doc = include_str!("signers.md")]

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cratestack::cose::{CoseAlg, CoseVerifierResolver, CoseVerifyKey};
use cratestack::{CratestackContext, CratestackError, FilterExpr, VerifiedSigner};

use crate::auth::{RESERVED_ROLE_KEYS, system_context};
use crate::bearer::VerifiedBearer;
use crate::cache::TtlCache;
use crate::console_keys::names_pair_scope;
use crate::schema::{
    self, Cratestack, app_client, client_key, console_key, oauth_client, role, user,
};

/// How long an `AppClient` or a `User`/`Role` standing is trusted before it is
/// read again. A key's own revocation and expiry are never cached (they are
/// read on every request); this is only how long a deactivated client or user,
/// or a changed role, may keep acting. Same figure and same reasoning as the
/// token-era lookups this replaces.
const STANDING_TTL: Duration = Duration::from_mins(1);

/// The length of the `kid` a COSE message carries, in hex characters: the
/// first 8 bytes of the key's RFC 9679 thumbprint.
const KID_HEX_LEN: usize = 16;

/// What a request signed by an app client's key stands as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppStanding {
    /// `AppClient.clientId`, which becomes `Principal.sub`.
    pub client_id: String,
    /// `AppClient.appId`, which becomes `Principal.app_id`.
    pub app_id: String,
    /// `AppClient.scopes` in its stored (sentinel-delimited) form: what
    /// `require_permission` reads as the request's `scope`.
    pub scopes: String,
}

/// What a request signed by a paired console key stands as: the `User` it was
/// paired to, with its `Role`'s permissions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserStanding {
    /// `User.id`, which becomes `Principal.sub`.
    pub user_id: String,
    /// `Role.key`.
    pub role: String,
    /// The unpacked `Role.permissions`, what `require_permission` reads as
    /// the request's `perms`.
    pub perms: Vec<String>,
}

/// Who a verified key is: the answer of [`SignerDirectory::standing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// A non-revoked `ClientKey` of an active `AppClient`.
    App(AppStanding),
    /// A non-revoked, unexpired `ConsoleKey` of an active `User`.
    User(UserStanding),
}

impl Standing {
    /// The name the rate limiter and the idempotency layer charge this signer
    /// to. Two prefixes, so an app client and a person cannot share a bucket
    /// or an idempotency namespace even if their ids collide as strings.
    ///
    /// ```
    /// use sms_api::signers::{AppStanding, Standing};
    ///
    /// let standing = Standing::App(AppStanding {
    ///     client_id: "appc_demo".to_owned(),
    ///     app_id: "app1".to_owned(),
    ///     scopes: String::new(),
    /// });
    /// assert_eq!(standing.principal(), "client:appc_demo");
    /// ```
    #[must_use]
    pub fn principal(&self) -> String {
        match self {
            Standing::App(app) => format!("client:{}", app.client_id),
            Standing::User(user) => format!("user:{}", user.user_id),
        }
    }
}

/// The one lookup from a verified key to what it stands as.
///
/// Nothing here is cached except the standing of the *owner* of a key (an
/// `AppClient`, a `User` and its `Role`, for [`STANDING_TTL`]): the key rows
/// themselves are read on every request, so a revocation, an expiry or a
/// deletion takes effect on the next call on every replica, with no
/// invalidation to get wrong.
#[derive(Clone)]
pub struct SignerDirectory {
    db: Cratestack,
    sys: CratestackContext,
    apps: Arc<TtlCache<String, AppStanding>>,
    users: Arc<TtlCache<String, UserStanding>>,
}

impl SignerDirectory {
    /// A directory reading through `db` under the internal system context.
    #[must_use]
    pub fn new(db: Cratestack) -> Self {
        Self {
            db,
            sys: system_context("sms-api:signers"),
            apps: Arc::new(TtlCache::new(STANDING_TTL)),
            users: Arc::new(TtlCache::new(STANDING_TTL)),
        }
    }

    /// What the verified `signer` stands as at `now`.
    ///
    /// The algorithm the message was verified with decides which table is
    /// asked: Ed25519 reads `ClientKey` only and ESP256 `ConsoleKey` only. A
    /// thumbprint names one key of one algorithm, so a client key's thumbprint
    /// presented under ESP256 (or the reverse) is a contradiction and is refused
    /// instead of being resolved by whichever table happens to hold it.
    ///
    /// An unknown, revoked or (for a console key) expired key, and a key whose
    /// client or person is inactive, retired, deleted or holds a reserved
    /// role, are all [`CratestackError::Unauthorized`] with the same wording
    /// class: the envelope layer answers every one with its coarse unsigned
    /// 401, so a caller learns nothing about which it was.
    ///
    /// # Errors
    ///
    /// [`CratestackError::Unauthorized`] as above; any other error is a
    /// database failure and surfaces as a 5xx, never as a refusal.
    pub async fn standing(
        &self,
        signer: &VerifiedSigner,
        now: DateTime<Utc>,
    ) -> Result<Standing, CratestackError> {
        let hex = hex::encode(signer.thumbprint());

        match CoseAlg::from_id(signer.alg()) {
            Some(CoseAlg::Ed25519) => {
                let key = self.live_client_key(&hex).await?.ok_or_else(unknown)?;
                let app = self
                    .apps
                    .get_or_fetch(key.appClientId, |id| {
                        let db = self.db.clone();
                        let sys = self.sys.clone();
                        async move { load_app(&db, &sys, &id).await }
                    })
                    .await?;
                Ok(Standing::App(app))
            }
            Some(CoseAlg::Esp256) => {
                let key = self
                    .live_console_key(&hex, now)
                    .await?
                    .ok_or_else(unknown)?;
                let user = self
                    .users
                    .get_or_fetch(key.userId, |id| {
                        let db = self.db.clone();
                        let sys = self.sys.clone();
                        async move { load_user(&db, &sys, &id).await }
                    })
                    .await?;
                Ok(Standing::User(user))
            }
            // vsms accepts no shared secret and no other signature algorithm.
            _ => Err(unknown()),
        }
    }

    /// What a verified bearer token stands as: the other way into [`Standing`].
    ///
    /// The token only **names** a client (`sub`); everything it may do is read
    /// here, from rows, every time:
    ///
    /// 1. the `OauthClient` with that `clientId` exists and is `active` (read on
    ///    every request, never cached, so deactivating a client ends its
    ///    outstanding tokens on the next call rather than after their 15
    ///    minutes);
    /// 2. it is linked to an `AppClient` (`appClientId`), which is `active` and
    ///    not retired ([`load_app`], cached for [`STANDING_TTL`] like the signed
    ///    door's), and whose `clientId` is the token's `sub`;
    /// 3. the scope is [`effective_scopes`]: the `AppClient`'s own, kept only
    ///    where the `OauthClient` row also lists it, narrowed by the token's
    ///    claim when it carries one, and never `console:pair`.
    ///
    /// Same [`Standing`] as a signed request from the same client, so
    /// [`crate::auth::GatewayAuth::context_for`] builds the same context and
    /// every policy answers the same.
    ///
    /// # Errors
    ///
    /// [`CratestackError::Unauthorized`] for an unknown or inactive client, one
    /// with no `AppClient`, or a retired or inactive `AppClient`; any other
    /// error is a database failure and surfaces as a 5xx.
    pub async fn bearer_standing(
        &self,
        bearer: &VerifiedBearer,
    ) -> Result<Standing, CratestackError> {
        let client = self
            .db
            .oauth_client()
            .find_many()
            .where_expr(
                FilterExpr::from(oauth_client::clientId().eq(bearer.client_id.clone()))
                    .and(oauth_client::active().is_true()),
            )
            .limit(1)
            .run(&self.sys)
            .await?
            .into_iter()
            .next()
            .ok_or_else(unknown)?;
        let app_client_id = client.appClientId.ok_or_else(unknown)?;
        let app = self
            .apps
            .get_or_fetch(app_client_id, |id| {
                let db = self.db.clone();
                let sys = self.sys.clone();
                async move { load_app(&db, &sys, &id).await }
            })
            .await?;
        if app.client_id != bearer.client_id {
            return Err(unknown());
        }
        Ok(Standing::App(AppStanding {
            scopes: effective_scopes(&app.scopes, &client.scopes, bearer.scope.as_deref()),
            ..app
        }))
    }

    async fn live_client_key(
        &self,
        hex: &str,
    ) -> Result<Option<schema::ClientKey>, CratestackError> {
        Ok(self
            .db
            .client_key()
            .find_many()
            .where_expr(
                FilterExpr::from(client_key::thumbprint().eq(hex.to_owned()))
                    .and(client_key::revokedAt().is_null()),
            )
            .limit(1)
            .run(&self.sys)
            .await?
            .into_iter()
            .next())
    }

    async fn live_console_key(
        &self,
        hex: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<schema::ConsoleKey>, CratestackError> {
        Ok(self
            .db
            .console_key()
            .find_many()
            .where_expr(
                FilterExpr::from(console_key::thumbprint().eq(hex.to_owned()))
                    .and(console_key::revokedAt().is_null())
                    .and(console_key::expiresAt().gt(now)),
            )
            .limit(1)
            .run(&self.sys)
            .await?
            .into_iter()
            .next())
    }
}

/// The scopes a bearer token acts with: `granted` (the `AppClient`'s own,
/// packed) kept only where `registered` (the `OauthClient`'s own, packed) also
/// lists it and, when the token carries a scope claim, where `claimed` names it
/// too; never `console:pair`.
///
/// The `OauthClient` row is the ceiling of every token it mints: `/token`
/// refuses a requested scope the row does not list, but a token asked for no
/// scope carries none, and without this intersection it would act with every
/// scope the `AppClient` holds, including one an admin added to the
/// `AppClient` alone. The claim then **narrows and never grants**: a scope the
/// token names that the client does not hold is ignored, and a claim that is
/// absent leaves the intersection of the two rows. `console:pair` is dropped
/// whatever any side says: it is the scope that lets a caller pair a console
/// key to a person, the one grant a bearer must not carry (the pairing is a
/// signed act, `docs/architecture.md` section 4.9). Compared the way
/// [`names_pair_scope`] does, ASCII case-insensitively.
///
/// Returned in the packed, sentinel-delimited form `require_permission` reads.
pub(crate) fn effective_scopes(granted: &str, registered: &str, claimed: Option<&str>) -> String {
    let registered = sms_core::unpack(registered);
    let kept = sms_core::unpack(granted).into_iter().filter(|scope| {
        !names_pair_scope(scope)
            && registered.contains(scope)
            && claimed.is_none_or(|claim| claim.split_whitespace().any(|named| named == *scope))
    });
    // The scopes came out of a packed column, so each is already a legal member.
    sms_core::pack(kept).unwrap_or_default()
}

fn unknown() -> CratestackError {
    CratestackError::Unauthorized("unknown signer".to_owned())
}

/// The `AppClient` behind a `ClientKey`, if it may still act.
async fn load_app(
    db: &Cratestack,
    sys: &CratestackContext,
    app_client_id: &str,
) -> Result<AppStanding, CratestackError> {
    let client = db
        .app_client()
        .find_many()
        .where_expr(
            FilterExpr::from(app_client::id().eq(app_client_id.to_owned()))
                .and(app_client::active().is_true())
                .and(app_client::retiredAt().is_null()),
        )
        .limit(1)
        .run(sys)
        .await?
        .into_iter()
        .next()
        .ok_or_else(unknown)?;
    Ok(AppStanding {
        client_id: client.clientId,
        app_id: client.appId,
        scopes: client.scopes,
    })
}

/// A person holding a reserved role key is never a signer. The database
/// refuses to create such a role, so this is reachable only if that `CHECK`
/// is ever dropped, which is exactly when it matters.
fn refuse_reserved_role(role_key: &str) -> Result<(), CratestackError> {
    if RESERVED_ROLE_KEYS.contains(&role_key) {
        return Err(unknown());
    }
    Ok(())
}

/// The `User` behind a `ConsoleKey` and its `Role`'s permissions, if the
/// person may still act (active, not soft-deleted, a role that exists and is
/// not reserved).
///
/// The reserved-role check is the second of two independent guards, kept from
/// the token era on purpose: `Role.key`'s regex accepts the literal `system`,
/// and a human holding it would match `hasRole('system')` exactly as the
/// synthetic internal context does. The first guard is `roles_key_not_reserved_check`
/// in the database; see [`RESERVED_ROLE_KEYS`].
async fn load_user(
    db: &Cratestack,
    sys: &CratestackContext,
    user_id: &str,
) -> Result<UserStanding, CratestackError> {
    let person = db
        .user()
        .find_many()
        .where_expr(
            FilterExpr::from(user::id().eq(user_id.to_owned())).and(user::active().is_true()),
        )
        .limit(1)
        .run(sys)
        .await?
        .into_iter()
        .next()
        .ok_or_else(unknown)?;

    let role_row = db
        .role()
        .find_many()
        .where_expr(FilterExpr::from(role::key().eq(person.roleKey)))
        .limit(1)
        .run(sys)
        .await?
        .into_iter()
        .next()
        .ok_or_else(unknown)?;

    refuse_reserved_role(&role_row.key)?;

    Ok(UserStanding {
        user_id: person.id,
        role: role_row.key,
        perms: sms_core::unpack(&role_row.permissions)
            .into_iter()
            .map(str::to_owned)
            .collect(),
    })
}

/// Finds the public keys that may have signed a message: `ClientKey` rows for
/// an Ed25519 message and unexpired `ConsoleKey` rows for an ESP256 one, never
/// a revoked row of either.
///
/// Unlike a resolver that keeps a revoked key resolving so the refusal can be
/// sealed, this one stops resolving it, so a revoked or expired key fails
/// verification and gets the coarse unsigned 401 the envelope layer gives every
/// verification failure. A person holding a revoked key learns nothing from the
/// answer, which is the point; they re-pair.
pub struct DirectoryKeys {
    db: Cratestack,
    sys: CratestackContext,
}

impl DirectoryKeys {
    /// A resolver reading through `db` under the internal system context.
    #[must_use]
    pub fn new(db: Cratestack) -> Self {
        Self {
            db,
            sys: system_context("sms-api:key-resolver"),
        }
    }
}

#[async_trait::async_trait]
impl CoseVerifierResolver for DirectoryKeys {
    async fn resolve(
        &self,
        kid: &[u8],
        alg: CoseAlg,
    ) -> Result<Vec<CoseVerifyKey>, CratestackError> {
        let prefix = hex::encode(kid);
        if prefix.len() != KID_HEX_LEN {
            return Ok(Vec::new());
        }
        match alg {
            CoseAlg::Ed25519 => {
                let rows = self
                    .db
                    .client_key()
                    .find_many()
                    .where_expr(
                        FilterExpr::from(client_key::thumbprint().starts_with(prefix))
                            .and(client_key::revokedAt().is_null()),
                    )
                    .limit(8)
                    .run(&self.sys)
                    .await?;
                Ok(rows
                    .iter()
                    .filter_map(|row| {
                        let bytes: [u8; 32] = row.publicKey.as_slice().try_into().ok()?;
                        CoseVerifyKey::ed25519(&bytes).ok()
                    })
                    .collect())
            }
            CoseAlg::Esp256 => {
                let rows = self
                    .db
                    .console_key()
                    .find_many()
                    .where_expr(
                        FilterExpr::from(console_key::thumbprint().starts_with(prefix))
                            .and(console_key::revokedAt().is_null())
                            .and(console_key::expiresAt().gt(Utc::now())),
                    )
                    .limit(8)
                    .run(&self.sys)
                    .await?;
                Ok(rows
                    .iter()
                    .filter_map(|row| CoseVerifyKey::p256_sec1(&row.publicKey).ok())
                    .collect())
            }
            // vsms accepts no shared secret: an HMAC message has no key here.
            _ => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reserved_role_key_is_never_a_signer() {
        for key in RESERVED_ROLE_KEYS {
            assert!(
                matches!(
                    refuse_reserved_role(key),
                    Err(CratestackError::Unauthorized(_))
                ),
                "{key} must not sign"
            );
        }
        assert!(refuse_reserved_role("admin").is_ok());
        assert!(refuse_reserved_role("operator").is_ok());
    }

    #[test]
    fn a_claim_narrows_the_clients_scopes_and_never_grants() {
        let granted = " sms:send sms:read job:read ";
        // The OAuth client lists all three, so only the claim narrows.
        let registered = granted;
        // Absent: the client's own.
        assert_eq!(effective_scopes(granted, registered, None), granted);
        // The intersection, in the client's order.
        assert_eq!(
            effective_scopes(granted, registered, Some("sms:read sms:send")),
            " sms:send sms:read "
        );
        // A scope the client does not hold is not granted by naming it.
        assert_eq!(
            effective_scopes(granted, registered, Some("sms:send provider:update")),
            " sms:send "
        );
        assert_eq!(
            effective_scopes(granted, registered, Some("provider:update")),
            " "
        );
        // An empty claim narrows to nothing; it does not widen to everything.
        assert_eq!(effective_scopes(granted, registered, Some("")), " ");
        // Whole scopes, not substrings.
        assert_eq!(
            effective_scopes(" sms:send ", " sms:send ", Some("sms:sendall")),
            " "
        );
    }

    #[test]
    fn the_oauth_client_row_caps_a_token_that_names_no_scope() {
        // An admin added job:read to the AppClient alone: the OauthClient row,
        // which /token checks a requested scope against, never listed it.
        let app = " sms:send sms:read job:read ";
        let oauth = " sms:send sms:read ";
        assert_eq!(effective_scopes(app, oauth, None), " sms:send sms:read ");
        // Naming it in the claim does not get it past the row either.
        assert_eq!(
            effective_scopes(app, oauth, Some("sms:send job:read")),
            " sms:send "
        );
        // And the row cannot grant what the AppClient lacks.
        assert_eq!(
            effective_scopes(" sms:send ", " sms:send job:read ", None),
            " sms:send "
        );
        // A row that lists nothing grants nothing.
        assert_eq!(effective_scopes(app, " ", None), " ");
    }

    #[test]
    fn console_pair_never_rides_a_token() {
        let granted = " sms:send console:pair ";
        assert_eq!(effective_scopes(granted, granted, None), " sms:send ");
        assert_eq!(
            effective_scopes(granted, granted, Some("sms:send console:pair")),
            " sms:send "
        );
        assert_eq!(
            effective_scopes(" Console:Pair ", " Console:Pair ", Some("console:pair")),
            " "
        );
    }
}
