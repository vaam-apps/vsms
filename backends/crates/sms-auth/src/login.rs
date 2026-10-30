#![doc = include_str!("login.md")]

use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sms_api::schema::{self, Cratestack, UpdateUserInput, role, user, user_credential};
use sms_core::password::{generate_password, hash_password, verify_password};
use sms_core::unpack;
use thiserror::Error;

/// The actor a password login runs as: the `sub` of its `system` context, which
/// lands verbatim in `cratestack_audit.actor.sub` for the `lastLoginAt` stamp.
pub const LOGIN_ACTOR: &str = "sms-gateway:login";

/// A valid Argon2id hash of a password nobody has, verified against on the "no
/// such account" path so that branch costs the same Argon2 work as a genuine
/// wrong-password check.
///
/// Computed once, at first use, from this process's own [`hash_password`] over a
/// **random throwaway password that is dropped at once**, so there is no fixed
/// value anywhere that could ever be a password (a hash of a literal would be a
/// known credential; this one opens nothing). Not a typed PHC literal either: a
/// hand-typed constant risks a malformed segment that `verify_password` rejects
/// at parse time, which would make the "no such account" branch **faster** than a
/// real check and quietly reopen the timing side channel this exists to close.
/// [`warm_up`] computes it at boot so the first unknown email does not pay for two
/// hashes.
fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| {
        hash_password(&generate_password(DUMMY_PASSWORD_LENGTH))
            .expect("hashing a generated password never fails")
    })
}

/// How long the throwaway behind [`dummy_hash`] is: long enough that guessing it
/// is not a thing, and it is never stored or logged.
const DUMMY_PASSWORD_LENGTH: usize = 48;

/// A `User.id` no row has (a `cuid` is 25 characters), searched for in
/// `user_credentials` when no account matched, so the unknown-email path does the
/// same database work as the others (see [`authenticate_user`]).
const NO_SUCH_USER_ID: &str = "no-such-user";

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
    /// The `@version` the account was read at: what the `lastLoginAt` stamp is
    /// conditioned on.
    version: i64,
}

impl AuthenticatedUser {
    /// What [`stamp_last_login`] needs to record that this person signed in. Taken
    /// by the caller *before* it moves this value apart, and used only once a code
    /// has been issued.
    #[must_use]
    pub fn last_login_stamp(&self) -> LastLoginStamp {
        LastLoginStamp {
            user_id: self.subject.clone(),
            version: self.version,
        }
    }
}

/// Which account to stamp, and the `@version` it was read at. See
/// [`stamp_last_login`].
#[derive(Debug, Clone)]
pub struct LastLoginStamp {
    user_id: String,
    version: i64,
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
    /// The database could not answer. Not a refusal of the person: reported as an
    /// outage so a login that failed for infrastructure is not read as a wrong
    /// password. It reveals nothing about whether the account exists.
    #[error("the account could not be read: {0}")]
    Unavailable(#[source] CratestackError),
}

/// The full password check: look the `User` up by `email` under the `system`
/// context [`LOGIN_ACTOR`] (nothing else can run this read, since no person
/// exists yet to authorize it), refuse an inactive or soft-deleted account,
/// verify the password against `UserCredential`, and resolve the `Role`.
///
/// **A real Argon2 verification runs on every call**, against the stored hash
/// or, when there is no usable account, against a dummy one ([`dummy_hash`]). So
/// "no such account", "no password set", "inactive" and "wrong password" cost the
/// same and return the same [`LoginError::InvalidCredentials`]. The hashing runs
/// on the blocking pool: it is tens of milliseconds of CPU and must not stall the
/// executor under a burst.
///
/// **The database work is the same too**: the `User` read and the `UserCredential`
/// read both run whether or not an account matched (the second looks for
/// [`NO_SUCH_USER_ID`] when none did), so the one signal left once the hashing is
/// equal, an existing account costing a round trip more, is gone.
///
/// This does **not** stamp `lastLoginAt`: a right password is not yet a sign-in,
/// and the caller still has an authorization request to satisfy. It calls
/// [`stamp_last_login`] once a code has been issued.
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

    Ok(AuthenticatedUser {
        subject: user_row.id,
        email: user_row.email,
        display_name: user_row.displayName,
        role_key: role_row.key,
        permissions: unpack(&role_row.permissions)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        version: user_row.version,
    })
}

/// `lastLoginAt = now`, under the `@version` the row was read at. Called by
/// `POST /login` only after the authorization code is issued.
///
/// Best effort: it needs `User.update` to admit `system` (restored by VSA-4a) and
/// the row's `@version` to be unchanged, and a sign-in that was otherwise good is
/// not refused because a concurrent edit moved the version. A failed stamp is
/// logged at `warn`.
pub async fn stamp_last_login(db: &Cratestack, stamp: &LastLoginStamp) {
    let sys = sms_api::system_context(LOGIN_ACTOR);
    let stamped = db
        .user()
        .update(stamp.user_id.clone())
        .set(UpdateUserInput {
            lastLoginAt: Some(Some(chrono::Utc::now())),
            ..Default::default()
        })
        .if_match(stamp.version)
        .run(&sys)
        .await;
    if let Err(error) = stamped {
        tracing::warn!(%error, subject = %stamp.user_id, "login: stamping lastLoginAt failed");
    }
}

/// `User` and the hash of its `UserCredential` for `email`, only if the account
/// is usable: `active` and not soft-deleted (`@@soft_delete` already hides a
/// deleted row from `find_many`; `active` is the check the framework cannot do
/// for free, since an operator can deactivate an account without deleting it).
/// `Ok(None)` is "no usable account", including an account with no credential.
///
/// **Always two reads**: the credential read runs even when no account matched,
/// for [`NO_SUCH_USER_ID`], so the absence of an account is not visible as a
/// round trip saved.
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
    let user_row = users.into_iter().next();

    let looked_up = user_row
        .as_ref()
        .map_or_else(|| NO_SUCH_USER_ID.to_owned(), |row| row.id.clone());
    let credentials = db
        .user_credential()
        .find_many()
        .where_expr(FilterExpr::from(user_credential::userId().eq(looked_up)))
        .limit(1)
        .run(sys)
        .await
        .map_err(LoginError::Unavailable)?;
    let credential = credentials.into_iter().next();

    Ok(user_row
        .zip(credential)
        .map(|(user_row, credential)| (user_row, credential.passwordHash)))
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
    use super::{dummy_hash, verify_password};
    use sms_core::password::{generate_password, hash_cost, hash_password};

    /// `authenticate_user`'s timing argument depends on this value parsing and
    /// costing what a real hash costs: if it did not parse, `verify_password`
    /// would return `false` on the parse failure and the "no such account" path
    /// would run faster than a real wrong-password check; if it were cheaper (a
    /// lower memory or iteration cost) the same gap would open by a margin.
    /// Nobody knows the password behind it, so "verifies for the password it was
    /// made from" can no longer be the proof; its recorded cost being exactly a
    /// real hash's is.
    #[test]
    fn the_dummy_hash_is_a_valid_phc_string_that_costs_what_a_real_hash_costs() {
        let real = hash_password(&generate_password(24)).expect("hashing");
        let dummy_cost = hash_cost(dummy_hash());
        assert!(
            dummy_cost.is_some(),
            "the dummy hash must parse, or the unknown-account path skips the Argon2 work"
        );
        assert_eq!(dummy_cost, hash_cost(&real), "the same variant and cost");
    }

    #[test]
    fn the_dummy_hash_is_computed_once() {
        assert_eq!(dummy_hash(), dummy_hash());
    }

    /// No fixed value: the hash is of a password drawn at first use and dropped,
    /// so nothing a request, a test or a source file could say opens it.
    #[test]
    fn nothing_a_request_could_send_verifies_against_the_dummy_hash() {
        for _ in 0..3 {
            assert!(!verify_password(&generate_password(24), dummy_hash()));
        }
        assert!(!verify_password(&generate_password(0), dummy_hash()));
    }
}
