#![doc = include_str!("password.md")]

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::Rng;
use rand::distributions::Alphanumeric;

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
/// workspace for key material. Moved here from `backends/apps/sms-gateway/src/
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
