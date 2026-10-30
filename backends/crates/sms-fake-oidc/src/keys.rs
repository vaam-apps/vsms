//! Throwaway signing keys for the fake, generated at run time and held for the
//! life of the process.
//!
//! None of these protects anything: the fake publishes the matching public
//! halves itself, and the point of the "wrong key" tests is exactly that a
//! token signed with a key the relying party has never seen is refused. They
//! are generated rather than committed as literals because a key committed to a
//! repository is a key a secret scanner (rightly) cannot tell from a real one:
//! a literal here would need a scanner exemption, and a generated key needs
//! none. The cost is key generation in a debug build, which the workspace
//! `[profile.dev.package]` entries for the big-integer crates pay down. A
//! `kid` stays fixed (`fake-rsa-1`...), so a test names a key and not a hash.

use std::sync::OnceLock;

use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};

const RSA_BITS: usize = 2048;

fn rsa_pem() -> String {
    let key = RsaPrivateKey::new(&mut rand::rngs::OsRng, RSA_BITS)
        .expect("generating a throwaway RSA test key");
    key.to_pkcs8_pem(LineEnding::LF)
        .expect("a fresh RSA key serialises as PKCS#8")
        .to_string()
}

fn ec_pem() -> String {
    use p256::pkcs8::EncodePrivateKey as _;
    let key = p256::SecretKey::random(&mut rand::rngs::OsRng);
    key.to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
        .expect("a fresh P-256 key serialises as PKCS#8")
        .to_string()
}

/// A throwaway RSA key (PKCS#8 PEM). `index` is 1, 2 or 3 and nothing else.
pub(crate) fn rsa(index: usize) -> &'static str {
    static KEYS: [OnceLock<String>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    KEYS[index - 1].get_or_init(rsa_pem)
}

/// The throwaway P-256 key (PKCS#8 PEM).
pub(crate) fn ec() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(ec_pem)
}
