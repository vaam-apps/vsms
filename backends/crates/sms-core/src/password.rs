#![doc = include_str!("password.md")]

use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Argon2, password_hash};
use rand::Rng;
use rand::distributions::Alphanumeric;

/// Hash `password` with Argon2id, using the crate's own recommended default
/// parameters (19 MiB memory, 2 iterations, 1 lane: OWASP's minimum for
/// Argon2id) and a fresh random salt, as a PHC string.
///
/// # Errors
///
/// Only on an internal Argon2 or random-source failure; not reachable for any
/// password this crate's callers pass it.
pub fn hash_password(password: &str) -> Result<String, password_hash::Error> {
    Ok(Argon2::default()
        .hash_password(password.as_bytes())?
        .to_string())
}

/// Verify `password` against a stored Argon2id PHC string. `false` on any
/// parse failure of `hash` too: a corrupt stored hash must never verify, the
/// fail-closed default every caller of this function leans on.
///
/// ```
/// use sms_core::password::{hash_password, verify_password};
///
/// let hash = hash_password("correct horse battery staple").unwrap();
/// assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
/// assert!(verify_password("correct horse battery staple", &hash));
/// assert!(!verify_password("wrong password entirely", &hash));
///
/// // A corrupt/non-PHC stored hash never verifies: it does not panic or
/// // propagate a parse error, it fails closed.
/// assert!(!verify_password("anything", "not a valid phc string"));
/// ```
#[must_use]
pub fn verify_password(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

/// A fresh, random alphanumeric one-time password, `len` characters long.
///
/// 24 characters (every caller's own choice, not a default baked in here) is
/// about 142 bits of entropy, comfortably more than Argon2id's cost is meant to
/// protect against a guess of, and short enough an operator can read it over a
/// phone call for a break-glass first account. `rand::thread_rng()` is the same
/// source `rsa::RsaPrivateKey::new` already trusts elsewhere in this workspace
/// for key material.
///
/// ```
/// use sms_core::password::generate_password;
///
/// let password = generate_password(24);
/// assert_eq!(password.len(), 24);
/// assert!(password.chars().all(|c| c.is_ascii_alphanumeric()));
/// assert_ne!(password, generate_password(24)); // not a fixed string
/// ```
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
        // A fresh random salt every time: proves this isn't a bare unsalted
        // digest, which would make two accounts sharing a password produce
        // identical stored hashes.
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

    /// A PHC string this module did not produce verifies, and only for the
    /// right password. This is the reference implementation's own test vector
    /// (phc-winner-argon2 `src/test.c`, as the `argon2` crate vendors it), not
    /// output of this crate's hasher, so the test cannot agree with a drifted
    /// one. It is what lets a deployment import the `user_credentials` rows it
    /// exported across 0009 and 0012 (docs/runbooks/deployment.adoc): the hash
    /// carries its own parameters, so a hash made with other ones verifies too.
    #[test]
    fn a_phc_string_from_the_reference_implementation_verifies() {
        const REFERENCE: &str = "$argon2id$v=19$m=65536,t=2,p=1$c29tZXNhbHQ$CTFhFdXPJO1aFaMaO6Mm5c8y7cJHAph8ArZWb2GRPPc";
        assert!(verify_password("password", REFERENCE));
        assert!(!verify_password("sassword", REFERENCE));
    }

    #[test]
    fn generated_passwords_are_the_requested_length_and_alphanumeric() {
        let password = generate_password(24);
        assert_eq!(password.len(), 24);
        assert!(password.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn two_generated_passwords_differ() {
        assert_ne!(generate_password(24), generate_password(24));
    }
}
