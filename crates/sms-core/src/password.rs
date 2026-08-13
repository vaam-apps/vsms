//! Argon2id password hashing, and the one-time-password generator every
//! human-account provisioning path in this codebase uses.
//!
//! # Why this lives in `sms-core`, not `sms-auth` (where #194 first put it)
//!
//! `sms-auth::login::authenticate_user` is not this module's only caller
//! any more. #52/#58 add `provisionUser`, a `sms-api` procedure that lets an
//! `owner`/`admin` create a console account from the admin screens rather
//! than only from `sms-gateway provision-user`'s CLI — and `sms-api` cannot
//! depend on `sms-auth` (the dependency runs the other way: `sms-auth`
//! depends on `sms-api`, confirmed by `crates/sms-auth/Cargo.toml`'s own
//! `sms-api.workspace = true`). Duplicating the hashing call would have
//! been the cheap fix — `crates/sms-api/src/procedures.rs` already accepts
//! that tradeoff for `CLIENT_RSA_KEY_BITS`, a bare constant — but a Argon2
//! parameter choice is exactly the kind of security-sensitive logic this
//! codebase's own convention argues against duplicating (see AGENTS.md's
//! `#134` section on the `sha_of` test helper that hand-rolled a second
//! copy of a hash algorithm and silently drifted from the real one the
//! moment it changed). `sms-core` is the one crate already sitting below
//! both `sms-api` and `sms-auth` — confirmed by both crates' own
//! `sms-core.workspace = true` — so moving the hashing here, rather than
//! duplicating it, removes the drift risk entirely instead of accepting it.
//!
//! `sms-auth::login` and `app/sms-gateway`'s `provision-user` CLI command
//! both now call the functions here directly; neither keeps its own copy.
//!
//! # What moved and what didn't
//!
//! [`hash_password`], [`verify_password`] and [`generate_password`] are
//! pure — no schema, no database, no framework dependency, matching this
//! crate's own existing convention (`lib.rs`'s own doc: "conventions more
//! than one crate has to agree on"). `sms-auth::login`'s own timing-safe
//! "no such user" dummy-hash construction, and everything about *who* may
//! authenticate or be provisioned, stays in `sms-auth`/`sms-api` — this
//! module only ever answers "does this password match this hash" and
//! "generate me a fresh one," nothing about identity or authorization.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::distributions::Alphanumeric;
use rand::Rng;

/// Hash `password` with Argon2id, using the crate's own recommended default
/// parameters (19 MiB memory, 2 iterations, 1 lane — OWASP's own minimum
/// recommendation for Argon2id as of 2024) and a fresh random salt.
///
/// # Errors
///
/// Only on an internal Argon2 failure (buffer sizing) — not reachable for
/// any password this crate's own callers pass it, since none of them are
/// unbounded in length before reaching here.
pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default().hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// Verify `password` against a stored Argon2id PHC string. `false` on any
/// parse failure of `hash` too — a corrupt stored hash must never verify,
/// the fail-closed default every caller of this function leans on.
#[must_use]
pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// A fresh, random alphanumeric one-time password, `len` characters long.
///
/// 24 characters (this module's every caller's own choice, not a default
/// baked in here) is ~142 bits of entropy — comfortably more than Argon2id's
/// own hashing cost is meant to protect against a brute-force guess of, and
/// short enough an operator can read it over a phone call for a break-glass
/// first account. `rand::thread_rng()`, not a hand-rolled PRNG — the same
/// source `rsa::RsaPrivateKey::new` already trusts elsewhere in this
/// workspace for key material. Moved here from `app/sms-gateway/src/
/// main.rs`'s own `provision_user_command` (#194) so the CLI's first-ever
/// account and the console's own `provisionUser` procedure (#58) generate a
/// password the identical way — one function, not two copies that could
/// silently drift in length or character set.
#[must_use]
pub fn generate_password(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{generate_password, hash_password, verify_password};

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
    fn generated_passwords_are_the_requested_length_and_alphanumeric() {
        let password = generate_password(24);
        assert_eq!(password.len(), 24);
        assert!(password.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn two_generated_passwords_differ() {
        // Not a strong randomness proof — just a smoke test that this isn't
        // returning a fixed string.
        assert_ne!(generate_password(24), generate_password(24));
    }
}
