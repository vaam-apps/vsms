//! The caller's signing key: an Ed25519 seed, and the one credential a vsms
//! request carries.
//!
//! There is no token to acquire, cache or refresh. A request is a COSE_Sign1
//! message signed by this key, and the gateway accepts it if the matching
//! public key was registered for an `AppClient` (`sms-gateway register-client`
//! or the `registerAppClientKey` procedure). The private half never leaves the
//! caller: vsms stores only the public key, so unlike the RSA keypair
//! `provision-client` used to generate server-side there is nothing to return
//! once and nothing to lose.

use std::path::Path;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use cratestack::cose::Ed25519Signer;

use crate::error::SdkError;

/// DER of a PKCS#8 `PrivateKeyInfo` for Ed25519 up to its 32-byte seed
/// (RFC 8410), the framing `openssl genpkey -algorithm ed25519` writes.
const PKCS8_ED25519_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

/// An Ed25519 signing key.
///
/// Built from a PKCS#8 PEM (what `openssl genpkey -algorithm ed25519` writes),
/// a raw 32-byte seed, or freshly generated. Its `Debug` prints the public
/// key, never the seed.
#[derive(Clone)]
pub struct SigningKey {
    seed: [u8; 32],
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningKey")
            .field("public_key", &self.public_key_b64u())
            .finish_non_exhaustive()
    }
}

impl SigningKey {
    /// The key of this 32-byte seed.
    #[must_use]
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { seed }
    }

    /// A fresh key from the operating system's random source.
    ///
    /// ```
    /// let a = vsms_sdk::SigningKey::generate();
    /// let b = vsms_sdk::SigningKey::generate();
    /// assert_ne!(a.public_key_b64u(), b.public_key_b64u());
    /// ```
    #[must_use]
    pub fn generate() -> Self {
        let mut seed = [0_u8; 32];
        seed[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        seed[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        Self { seed }
    }

    /// Parses a PKCS#8 Ed25519 private key PEM.
    ///
    /// ```
    /// use vsms_sdk::SigningKey;
    ///
    /// let key = SigningKey::generate();
    /// let pem = key.to_pem();
    /// assert_eq!(SigningKey::from_pem(&pem).unwrap().public_key_b64u(), key.public_key_b64u());
    /// // Anything that is not an Ed25519 key is refused, naming no content.
    /// assert!(SigningKey::from_pem("not a pem").is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// [`SdkError::Key`] when the text has no `PRIVATE KEY` block, the block
    /// is not base64, or it is not a PKCS#8 Ed25519 key (an RSA key from the
    /// token era is refused here, not later at the gateway).
    pub fn from_pem(pem: &str) -> Result<Self, SdkError> {
        let body: String = pem
            .lines()
            .map(str::trim)
            .skip_while(|line| *line != "-----BEGIN PRIVATE KEY-----")
            .skip(1)
            .take_while(|line| *line != "-----END PRIVATE KEY-----")
            .collect();
        if body.is_empty() {
            return Err(SdkError::Key("no `PRIVATE KEY` PEM block".to_owned()));
        }
        let der = STANDARD
            .decode(body.as_bytes())
            .map_err(|_| SdkError::Key("the PEM body is not base64".to_owned()))?;
        let seed = der
            .strip_prefix(&PKCS8_ED25519_PREFIX)
            .and_then(|seed| <[u8; 32]>::try_from(seed).ok())
            .ok_or_else(|| {
                SdkError::Key(
                    "not a PKCS#8 Ed25519 private key (`openssl genpkey -algorithm ed25519`)"
                        .to_owned(),
                )
            })?;
        Ok(Self { seed })
    }

    /// Reads a PKCS#8 Ed25519 private key PEM from `path`.
    ///
    /// # Errors
    ///
    /// [`SdkError::Key`] naming the path when it cannot be read, and the
    /// errors of [`from_pem`](Self::from_pem).
    pub fn from_pem_file(path: impl AsRef<Path>) -> Result<Self, SdkError> {
        let path = path.as_ref();
        let pem = std::fs::read_to_string(path)
            .map_err(|error| SdkError::Key(format!("reading {}: {error}", path.display())))?;
        Self::from_pem(&pem).map_err(|error| match error {
            SdkError::Key(reason) => SdkError::Key(format!("{}: {reason}", path.display())),
            other => other,
        })
    }

    /// This key as a PKCS#8 PEM, the format [`from_pem`](Self::from_pem) and
    /// `openssl` read. This is the private key: write it with `0600`.
    #[must_use]
    pub fn to_pem(&self) -> String {
        let der = [PKCS8_ED25519_PREFIX.as_slice(), self.seed.as_slice()].concat();
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            STANDARD.encode(der)
        )
    }

    /// The public key as unpadded base64url: what
    /// `sms-gateway register-client --public-key` and `registerAppClientKey`
    /// take.
    ///
    /// # Panics
    ///
    /// Never: an Ed25519 signer's verify key always has its 32 bytes.
    #[must_use]
    pub fn public_key_b64u(&self) -> String {
        let public = self
            .signer()
            .verify_key()
            .ed25519_bytes()
            .expect("an Ed25519 signer's verify key is Ed25519");
        URL_SAFE_NO_PAD.encode(public)
    }

    pub(crate) fn signer(&self) -> Ed25519Signer {
        Ed25519Signer::from_seed(&self.seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8032 section 7.1, test 1: the seed and the public key it must give.
    const SEED: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
    const PUBLIC: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

    fn hex32(text: &str) -> [u8; 32] {
        let bytes: Vec<u8> = (0..32)
            .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        bytes.try_into().unwrap()
    }

    #[test]
    fn a_seed_gives_the_public_key_the_rfc_says() {
        let key = SigningKey::from_seed(hex32(SEED));
        assert_eq!(
            URL_SAFE_NO_PAD.decode(key.public_key_b64u()).unwrap(),
            hex32(PUBLIC)
        );
    }

    #[test]
    fn a_pem_round_trips_and_matches_what_openssl_writes() {
        // `openssl genpkey -algorithm ed25519` for this seed's key.
        let pem = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIJ1hsZ3v/VpguoRK9JLsLMREScVpezJpGXA7rAMcrn9g\n-----END PRIVATE KEY-----\n";
        let key = SigningKey::from_pem(pem).unwrap();
        assert_eq!(key.seed, hex32(SEED));
        assert_eq!(key.to_pem(), pem);
    }

    #[test]
    fn crlf_and_text_around_the_block_are_tolerated() {
        let key = SigningKey::generate();
        let wrapped = format!("# made by hand\r\n{}", key.to_pem().replace('\n', "\r\n"));
        assert_eq!(SigningKey::from_pem(&wrapped).unwrap().seed, key.seed);
    }

    #[test]
    fn an_rsa_key_from_the_token_era_is_refused_up_front() {
        let rsa = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYJKoZIhvcNAQEBBQAEIgQg\n-----END PRIVATE KEY-----\n";
        let error = SigningKey::from_pem(rsa).unwrap_err();
        assert!(error.to_string().contains("Ed25519"), "{error}");
        assert!(SigningKey::from_pem("").is_err());
    }

    #[test]
    fn debug_never_prints_the_seed() {
        let key = SigningKey::from_seed(hex32(SEED));
        let printed = format!("{key:?}");
        assert!(printed.contains(&key.public_key_b64u()));
        assert!(!printed.contains(SEED));
        assert!(!printed.contains("9d61"));
    }
}
