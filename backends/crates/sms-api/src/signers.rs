#![doc = include_str!("signers.md")]

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cratestack::cose::{CoseAlg, CoseVerifierResolver, CoseVerifyKey};
use cratestack::{CratestackContext, CratestackError, FilterExpr};

use crate::auth::{RESERVED_ROLE_KEYS, system_context};
use crate::cache::TtlCache;
use crate::schema::{self, Cratestack, app_client, client_key, console_key, role, user};

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

    /// What the key with this RFC 9679 `thumbprint` stands as at `now`.
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
        thumbprint: &[u8; 32],
        now: DateTime<Utc>,
    ) -> Result<Standing, CratestackError> {
        let hex = hex::encode(thumbprint);

        if let Some(key) = self.live_client_key(&hex).await? {
            let app = self
                .apps
                .get_or_fetch(key.appClientId, |id| {
                    let db = self.db.clone();
                    let sys = self.sys.clone();
                    async move { load_app(&db, &sys, &id).await }
                })
                .await?;
            return Ok(Standing::App(app));
        }

        if let Some(key) = self.live_console_key(&hex, now).await? {
            let user = self
                .users
                .get_or_fetch(key.userId, |id| {
                    let db = self.db.clone();
                    let sys = self.sys.clone();
                    async move { load_user(&db, &sys, &id).await }
                })
                .await?;
            return Ok(Standing::User(user));
        }

        Err(unknown())
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
}
