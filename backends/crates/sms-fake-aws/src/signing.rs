#![doc = include_str!("signing.md")]

use base64::Engine as _;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use sha2::{Digest, Sha256};

/// A key and the self-signed certificate around it, made at start-up.
pub(crate) struct SigningIdentity {
    key: ring::signature::RsaKeyPair,
    rng: ring::rand::SystemRandom,
    cert_pem: String,
    /// `SimpleNotificationService-<32 hex digits>.pem`, the file name the
    /// certificate is served under and every notification names.
    cert_file: String,
}

impl SigningIdentity {
    /// A fresh 2048-bit RSA key and a certificate for it.
    ///
    /// # Panics
    ///
    /// Only if RSA key generation or a self-signature over a key this
    /// function just made fails, neither of which depends on input.
    pub(crate) fn generate() -> Self {
        let private = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)
            .expect("generating a 2048-bit RSA key");
        let pkcs8_pem = private
            .to_pkcs8_pem(LineEnding::LF)
            .expect("encoding a freshly generated RSA key as PKCS#8");
        let pkcs8_der = private
            .to_pkcs8_der()
            .expect("encoding a freshly generated RSA key as PKCS#8");

        let key_pair = rcgen::KeyPair::from_pem_and_sign_algo(&pkcs8_pem, &rcgen::PKCS_RSA_SHA256)
            .expect("rcgen accepts a PKCS#8 RSA key");
        let mut params = rcgen::CertificateParams::new(vec!["sms-fake-aws.invalid".to_owned()])
            .expect("a fixed, valid DNS name");
        params.distinguished_name.push(
            rcgen::DnType::CommonName,
            "sms-fake-aws - not an Amazon SNS certificate",
        );
        let cert = params
            .self_signed(&key_pair)
            .expect("self-signing with a key this function just made");

        let fingerprint = Sha256::digest(cert.der());
        let cert_file = format!(
            "SimpleNotificationService-{}.pem",
            hex::encode(&fingerprint[..16])
        );
        let key = ring::signature::RsaKeyPair::from_pkcs8(pkcs8_der.as_bytes())
            .expect("ring accepts a 2048-bit PKCS#8 RSA key");
        Self {
            key,
            rng: ring::rand::SystemRandom::new(),
            cert_pem: cert.pem(),
            cert_file,
        }
    }

    pub(crate) fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    pub(crate) fn cert_file(&self) -> &str {
        &self.cert_file
    }

    /// Base64 `RSA PKCS#1 v1.5 SHA-256` over `data`: SNS's
    /// `SignatureVersion` 2.
    pub(crate) fn sign(&self, data: &[u8]) -> String {
        let mut signature = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(
                &ring::signature::RSA_PKCS1_SHA256,
                &self.rng,
                data,
                &mut signature,
            )
            .expect("signing with a valid RSA key into a modulus-sized buffer");
        base64::engine::general_purpose::STANDARD.encode(signature)
    }
}

/// The exact text SNS signs: `name\nvalue\n` for each field in byte order
/// of name. Written from AWS's documentation, not copied from the
/// receiver, for the reason `sigv4.md` gives.
pub(crate) fn string_to_sign(fields: &[(&str, &str)]) -> String {
    let mut sorted = fields.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut out = String::new();
    for (name, value) in sorted {
        out.push_str(name);
        out.push('\n');
        out.push_str(value);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{SigningIdentity, string_to_sign};

    #[test]
    fn the_certificate_file_name_has_the_shape_sns_uses() {
        let identity = SigningIdentity::generate();
        let id = identity
            .cert_file()
            .strip_prefix("SimpleNotificationService-")
            .and_then(|rest| rest.strip_suffix(".pem"))
            .unwrap();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(
            identity
                .cert_pem()
                .starts_with("-----BEGIN CERTIFICATE-----")
        );
    }

    #[test]
    fn fields_are_signed_in_name_order() {
        assert_eq!(
            string_to_sign(&[
                ("Type", "Notification"),
                ("Message", "m"),
                ("MessageId", "i")
            ]),
            "Message\nm\nMessageId\ni\nType\nNotification\n"
        );
    }
}
