#![doc = include_str!("login.md")]

use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sms_api::auth::RESERVED_ROLE_KEYS;
use sms_api::schema::{self, Cratestack, UpdateUserInput, role, user, user_credential};
use sms_core::password::{hash_password, verify_password};
use sms_core::unpack;
use thiserror::Error;

/// The actor a password login runs as: the `sub` of its `system` context, which
/// lands verbatim in `cratestack_audit.actor.sub` for the `lastLoginAt` stamp.
pub const LOGIN_ACTOR: &str = "sms-gateway:login";

/// A valid Argon2id hash with no real backing password, verified against on the
/// "no such account" path so that branch costs the same Argon2 work as a genuine
/// wrong-password check.
///
/// Computed once, from this process's own [`hash_password`], and not a typed
/// PHC literal: a hand-typed constant risks a malformed segment that
/// `verify_password` rejects at parse time, which would make the "no such
/// account" branch **faster** than a real check and quietly reopen the timing
/// side channel this exists to close. [`warm_up`] computes it at boot so the first
/// unknown email does not pay for two hashes.
fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY
        .get_or_init(|| hash_password(DUMMY_PASSWORD).expect("hashing a fixed literal never fails"))
}

/// What [`dummy_hash`] hashes. Not a secret and not a credential: it backs no
/// account, so [`authenticate_user`] refuses whoever sends it for an unknown
/// email exactly like anyone else.
const DUMMY_PASSWORD: &str =
    "sms-auth internal dummy: never a real password, never checked against one";

/// Computes the dummy hash now. `serve` calls it once at boot.
pub fn warm_up() {
    let _ = dummy_hash();
}

/// What the rest of the sign-in (`POST /login`, the authorization code) needs
/// about a successfully authenticated person. Deliberately not a raw
/// [`schema::User`], so a caller cannot reach a field authentication has already
/// decided does not matter.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    /// `User.id`: the code's `Identity.external_id`, and the token's `sub`.
    pub subject: String,
    /// `User.email`.
    pub email: String,
    /// `User.displayName`.
    pub display_name: String,
    /// `Role.key`.
    pub role_key: String,
    /// `Role.permissions`, unpacked.
    pub permissions: Vec<String>,
}

/// Why [`authenticate_user`] refused a sign-in.
#[derive(Debug, Error)]
pub enum LoginError {
    /// No account, no password on it, a wrong password, an inactive account or a
    /// soft-deleted one: one variant for all of them, never told apart to the
    /// caller.
    #[error("invalid email or password")]
    InvalidCredentials,
    /// The password was right and the account's `Role` row is missing: a
    /// referential gap. The *account* is broken, not the attempt, and an operator
    /// needs to know which; the caller still hears the generic refusal.
    #[error("user {subject:?} references role {role_key:?}, which does not exist")]
    RoleNotFound {
        /// `User.id` of the account with the dangling `roleKey`.
        subject: String,
        /// The `Role.key` that named no row.
        role_key: String,
    },
    /// The password was right and the account's role is one a person must never
    /// hold (`system`, `app`: [`RESERVED_ROLE_KEYS`]). The database refuses to
    /// create such a role; this is the second, independent refusal, at the point
    /// a code would be issued. The caller still hears the generic refusal.
    #[error("user {subject:?} holds the reserved role {role_key:?}")]
    ReservedRole {
        /// `User.id` of the account.
        subject: String,
        /// The reserved key.
        role_key: String,
    },
    /// The database could not answer. Not a refusal of the person: reported as an
    /// outage so a login that failed for infrastructure is not read as a wrong
    /// password. It reveals nothing about whether the account exists.
    #[error("the account could not be read: {0}")]
    Unavailable(#[source] CratestackError),
}

/// The full password check: look the `User` up by `email` under the `system`
/// context [`LOGIN_ACTOR`] (nothing else can run this read, since no person
/// exists yet to authorize it), refuse an inactive or soft-deleted account,
/// verify the password against `UserCredential`, resolve the `Role`, and stamp
/// `lastLoginAt`.
///
/// **A real Argon2 verification runs on every call**, against the stored hash
/// or, when there is no usable account, against a dummy one ([`dummy_hash`]). So
/// "no such account", "no password set", "inactive" and "wrong password" cost the
/// same and return the same [`LoginError::InvalidCredentials`]. The hashing runs
/// on the blocking pool: it is tens of milliseconds of CPU and must not stall the
/// executor under a burst.
///
/// The `lastLoginAt` stamp is best effort: it needs `User.update` to admit
/// `system` (restored by VSA-4a) and the row's `@version` to be unchanged, and a
/// login that was otherwise good is not refused because a concurrent edit moved
/// the version. A failed stamp is logged at `warn`.
///
/// # Errors
///
/// [`LoginError`].
pub async fn authenticate_user(
    db: &Cratestack,
    email: &str,
    password: &str,
) -> Result<AuthenticatedUser, LoginError> {
    let sys = sms_api::system_context(LOGIN_ACTOR);
    let candidate = find_active_user(db, &sys, email).await?;

    // Always verify, even with nothing real to verify against. `Option::and_then`
    // would short-circuit and restore the timing signal this exists to remove, so
    // the stored hash and the dummy are both read into one string first.
    let stored_hash = candidate
        .as_ref()
        .map_or_else(|| dummy_hash().to_owned(), |(_, hash)| hash.clone());
    let password = password.to_owned();
    let password_ok = tokio::task::spawn_blocking(move || verify_password(&password, &stored_hash))
        .await
        .unwrap_or_else(|error| {
            tracing::error!(%error, "login: the password verification task failed");
            false
        });

    let Some((user_row, _)) = candidate else {
        return Err(LoginError::InvalidCredentials);
    };
    if !password_ok {
        return Err(LoginError::InvalidCredentials);
    }

    let role_row = find_role(db, &sys, &user_row.roleKey)
        .await?
        .ok_or_else(|| LoginError::RoleNotFound {
            subject: user_row.id.clone(),
            role_key: user_row.roleKey.clone(),
        })?;
    if RESERVED_ROLE_KEYS.contains(&role_row.key.as_str()) {
        return Err(LoginError::ReservedRole {
            subject: user_row.id,
            role_key: role_row.key,
        });
    }

    stamp_last_login(db, &sys, &user_row).await;

    Ok(AuthenticatedUser {
        subject: user_row.id,
        email: user_row.email,
        display_name: user_row.displayName,
        role_key: role_row.key,
        permissions: unpack(&role_row.permissions)
            .into_iter()
            .map(str::to_owned)
            .collect(),
    })
}

/// `lastLoginAt = now`, under the `@version` the row was read at.
async fn stamp_last_login(db: &Cratestack, sys: &CratestackContext, row: &schema::User) {
    let stamped = db
        .user()
        .update(row.id.clone())
        .set(UpdateUserInput {
            lastLoginAt: Some(Some(chrono::Utc::now())),
            ..Default::default()
        })
        .if_match(row.version)
        .run(sys)
        .await;
    if let Err(error) = stamped {
        tracing::warn!(%error, subject = %row.id, "login: stamping lastLoginAt failed");
    }
}

/// `User` and the hash of its `UserCredential` for `email`, only if the account
/// is usable: `active` and not soft-deleted (`@@soft_delete` already hides a
/// deleted row from `find_many`; `active` is the check the framework cannot do
/// for free, since an operator can deactivate an account without deleting it).
/// `Ok(None)` is "no usable account", including an account with no credential.
async fn find_active_user(
    db: &Cratestack,
    sys: &CratestackContext,
    email: &str,
) -> Result<Option<(schema::User, String)>, LoginError> {
    let users = db
        .user()
        .find_many()
        .where_expr(
            FilterExpr::from(user::email().eq(email.to_owned())).and(user::active().is_true()),
        )
        .limit(1)
        .run(sys)
        .await
        .map_err(LoginError::Unavailable)?;
    let Some(user_row) = users.into_iter().next() else {
        return Ok(None);
    };

    let credentials = db
        .user_credential()
        .find_many()
        .where_expr(FilterExpr::from(
            user_credential::userId().eq(user_row.id.clone()),
        ))
        .limit(1)
        .run(sys)
        .await
        .map_err(LoginError::Unavailable)?;
    Ok(credentials
        .into_iter()
        .next()
        .map(|credential| (user_row, credential.passwordHash)))
}

async fn find_role(
    db: &Cratestack,
    sys: &CratestackContext,
    role_key: &str,
) -> Result<Option<schema::Role>, LoginError> {
    let roles = db
        .role()
        .find_many()
        .where_expr(FilterExpr::from(role::key().eq(role_key.to_owned())))
        .limit(1)
        .run(sys)
        .await
        .map_err(LoginError::Unavailable)?;
    Ok(roles.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::{DUMMY_PASSWORD, dummy_hash, verify_password};

    /// `authenticate_user`'s timing argument depends on this value parsing: if it
    /// did not, `verify_password` would return `false` on the parse failure and
    /// the "no such account" path would run faster than a real wrong-password
    /// check. `true` for the password it was made from is what separates "it ran
    /// Argon2" from "it never did": a wrong password is `false` either way.
    #[test]
    fn the_dummy_hash_is_a_valid_phc_string_a_real_verify_call_can_run_against() {
        assert!(
            verify_password(DUMMY_PASSWORD, dummy_hash()),
            "the dummy hash must parse and verify, or the unknown-account path skips the Argon2 work"
        );
        assert!(!verify_password(
            "a password no login attempt would ever send",
            dummy_hash()
        ));
    }

    #[test]
    fn the_dummy_hash_is_computed_once() {
        assert_eq!(dummy_hash(), dummy_hash());
    }
}
