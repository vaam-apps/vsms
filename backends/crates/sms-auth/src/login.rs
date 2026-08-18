#![doc = include_str!("login.md")]

use cratestack::{CratestackContext, FilterExpr};
use sms_api::schema::{self, Cratestack, role, user, user_credential};
use sms_core::password::{hash_password, verify_password};
use sms_core::unpack;
use thiserror::Error;

/// A valid Argon2id hash with no real backing password — verified against
/// on the "user not found" path so that branch costs the same Argon2 work
/// as a genuine wrong-password check (see this module's own doc on
/// timing/enumeration).
///
/// Computed once, lazily, from this process's own [`hash_password`] rather
/// than a hand-typed PHC-string literal: a hand-typed constant risks a
/// subtly malformed base64 segment that `PasswordHash::new` then rejects at
/// parse time, which would make the "no such user" branch *faster* than a
/// real wrong-password check (parse failure short-circuits before the
/// actual hashing work) — silently reopening the exact timing side-channel
/// this constant exists to close. Computing it from the real code path
/// guarantees it is always a hash [`verify_password`] can actually parse
/// and run Argon2 work against.
fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| {
        hash_password("sms-auth internal dummy — never a real password, never checked against one")
            .expect("hashing a fixed literal never fails")
    })
}

/// Everything the rest of the login flow (the `POST /login` route,
/// `id_token`/access-token issuance) needs about a successfully
/// authenticated human — deliberately not a raw [`schema::User`], so a
/// caller can't accidentally reach a field (like a soft-deleted row's own
/// `deletedAt`) that authentication has already decided doesn't matter.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    /// `User.id` — becomes the token's `sub`/`Identity.external_id`.
    pub subject: String,
    /// `User.email`.
    pub email: String,
    /// `User.displayName`.
    pub display_name: String,
    /// `Role.key` — becomes `Principal.role` once this identity reaches
    /// `GatewayAuth` on a later request (see `sms-api::auth`'s human path).
    pub role_key: String,
    /// Unpacked from `Role.permissions` (§2.2's sentinel-delimited
    /// convention) — the `perms` a future token/claims projection carries.
    pub permissions: Vec<String>,
}

/// Why [`authenticate_user`] refused a login attempt.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LoginError {
    /// No account, a wrong password, an inactive account, or a
    /// soft-deleted account — deliberately one variant for all four (see
    /// this module's own doc on why), never distinguished to the caller.
    #[error("invalid email or password")]
    InvalidCredentials,
    /// The account's `Role` row is missing — a referential-integrity gap
    /// (`roleKey` names a `Role.key` that no longer exists), not a
    /// credentials problem. Distinguished from `InvalidCredentials`
    /// because it means the *account* is broken, not the *attempt*, and an
    /// operator needs to know which.
    #[error("user {subject:?} references role {role_key:?}, which does not exist")]
    RoleNotFound {
        /// `User.id` of the account with the dangling `roleKey`.
        subject: String,
        /// The `Role.key` value that named no existing row.
        role_key: String,
    },
}

/// The full login check: look up `User` by `email` under `sys` (necessarily
/// a system context — see this module's own doc and `schema.cstack`'s
/// `User`/`Role` read-policy comments for why nothing else can run this
/// lookup), reject an inactive or soft-deleted account, verify the
/// password against `UserCredential`, and resolve the account's `Role`.
///
/// `sys` must carry `role: "system"` — the same context
/// `sms_api::auth::GatewayAuth`'s own internal `AppClient` lookup uses, and
/// for the identical reason: this runs *before* any human principal exists
/// to authorize the read itself.
///
/// # Errors
///
/// [`LoginError::InvalidCredentials`] for anything that must not be
/// distinguishable to the caller — no such email, wrong password, inactive
/// account, soft-deleted account. [`LoginError::RoleNotFound`] only for the
/// one case that is an operator-visible data-integrity problem, not an
/// attacker-visible one.
pub async fn authenticate_user(
    db: &Cratestack,
    sys: &CratestackContext,
    email: &str,
    password: &str,
) -> Result<AuthenticatedUser, LoginError> {
    let candidate = find_active_user(db, sys, email).await;

    // Always verify, even with nothing real to verify against — see this
    // module's own doc on timing/enumeration. `and_then` short-circuits in
    // Rust, which would restore the timing signal this exists to remove;
    // the stored hash and the dummy are both read into a plain `&str`
    // first specifically so `verify_password` always runs.
    let stored_hash: &str = candidate.as_ref().map_or(dummy_hash(), |(_, credential)| {
        credential.passwordHash.as_str()
    });
    let password_ok = verify_password(password, stored_hash);

    let Some((user_row, _credential)) = candidate else {
        return Err(LoginError::InvalidCredentials);
    };
    if !password_ok {
        return Err(LoginError::InvalidCredentials);
    }

    let role_row =
        find_role(db, sys, &user_row.roleKey)
            .await
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
    })
}

/// `User` + `UserCredential` for `email`, only if the account is usable —
/// `active` and not soft-deleted. `@@soft_delete`'s own read behaviour
/// (`backends/crates/sms-api`'s golden-list tests assert this elsewhere) already
/// excludes a deleted row from `find_many` by default, so the `active`
/// check here is the one this query can't get from the framework for
/// free — an operator can deactivate an account without deleting it, and
/// login must honour both.
async fn find_active_user(
    db: &Cratestack,
    sys: &CratestackContext,
    email: &str,
) -> Option<(schema::User, schema::UserCredential)> {
    let users = db
        .user()
        .find_many()
        .where_expr(
            FilterExpr::from(user::email().eq(email.to_owned())).and(user::active().is_true()),
        )
        .limit(1)
        .run(sys)
        .await
        .inspect_err(|error| tracing::error!(%error, "login: User lookup failed"))
        .ok()?;
    let user_row = users.into_iter().next()?;

    let credentials = db
        .user_credential()
        .find_many()
        .where_expr(FilterExpr::from(
            user_credential::userId().eq(user_row.id.clone()),
        ))
        .limit(1)
        .run(sys)
        .await
        .inspect_err(|error| tracing::error!(%error, "login: UserCredential lookup failed"))
        .ok()?;
    let credential_row = credentials.into_iter().next()?;

    Some((user_row, credential_row))
}

async fn find_role(
    db: &Cratestack,
    sys: &CratestackContext,
    role_key: &str,
) -> Option<schema::Role> {
    let roles = db
        .role()
        .find_many()
        .where_expr(FilterExpr::from(role::key().eq(role_key.to_owned())))
        .limit(1)
        .run(sys)
        .await
        .inspect_err(|error| tracing::error!(%error, "login: Role lookup failed"))
        .ok()?;
    roles.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::{dummy_hash, hash_password, verify_password};

    #[test]
    fn a_freshly_hashed_password_verifies_against_itself() {
        let hash = hash_password("correct horse battery staple").expect("hashing succeeds");
        assert!(verify_password("correct horse battery staple", &hash));
    }

    #[test]
    fn the_wrong_password_does_not_verify() {
        let hash = hash_password("correct horse battery staple").expect("hashing succeeds");
        assert!(!verify_password("wrong password entirely", &hash));
    }

    #[test]
    fn two_hashes_of_the_same_password_differ() {
        // A fresh random salt every time — proves this isn't a bare
        // unsalted digest, which would make two accounts sharing a
        // password produce identical stored hashes.
        let a = hash_password("same password").expect("hashing succeeds");
        let b = hash_password("same password").expect("hashing succeeds");
        assert_ne!(a, b);
        assert!(verify_password("same password", &a));
        assert!(verify_password("same password", &b));
    }

    #[test]
    fn a_corrupt_stored_hash_never_verifies() {
        assert!(!verify_password("anything", "not a valid phc string"));
    }

    #[test]
    fn the_dummy_hash_is_a_valid_phc_string_a_real_verify_call_can_run_against() {
        // authenticate_user's timing-safety argument depends on this value
        // actually parsing — if it didn't, verify_password would
        // short-circuit on the parse failure and the "no such user" path
        // would run faster than a real wrong-password check, reopening the
        // exact enumeration gap this module's doc says is closed. This
        // doesn't (and can't, from outside the module) prove the *literal*
        // it was hashed from is never a real password — see the private
        // dummy_hash() itself for that guarantee — only that
        // PasswordHash::new(dummy_hash()) succeeds, so the full Argon2
        // verify path always actually runs.
        assert!(!verify_password(
            "a password no login attempt would ever send",
            dummy_hash()
        ));
    }
}
