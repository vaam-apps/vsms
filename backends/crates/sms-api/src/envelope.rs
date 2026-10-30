#![doc = include_str!("envelope.md")]

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use cratestack::cose::{CoseEnvelope, CoseMode, Ed25519Signer};
use cratestack::envelope_layer::{
    EnvelopeLayer, EnvelopeMode, PrincipalMapper, UnsignedRequest, VerifiedRequest, async_trait,
};
use cratestack::{CratestackContext, CratestackError, FilterExpr, NonceStore};

use crate::auth::system_context;
use crate::bearer::ApiDoors;
use crate::errors::UNIQUE_VIOLATION;
use crate::schema::{self, Cratestack, envelope_nonce};
use crate::signers::{DirectoryKeys, SignerDirectory};

/// The audience every sealed request binds: the configured name of this
/// service, never its host name (cratestack ADR 0006 section 4), so a request
/// sealed for another service, or for this one under another name, fails
/// verification. The SDK has the same constant.
pub const AUDIENCE: &str = "vsms";

/// The only payload type vsms seals, in both directions. The router's codec
/// is JSON (`router::ROUTER_CODEC`), so a CBOR payload would open fine and then
/// fail to decode; naming the one type here makes the layer refuse the other
/// with the `415` it has for a payload type outside the set, before any key is
/// looked up.
pub const PAYLOAD_TYPE: &str = "application/json";

/// DER of a PKCS#8 `PrivateKeyInfo` for Ed25519 up to its 32-byte seed
/// (RFC 8410): `SEQUENCE { INTEGER 0, SEQUENCE { OID 1.3.101.112 },
/// OCTET STRING { OCTET STRING seed } }`.
const PKCS8_ED25519_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

/// Why a server signing key could not be loaded. The text never quotes the
/// document.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ServerKeyError {
    /// The file is not UTF-8 text, has no `PRIVATE KEY` PEM block, or the
    /// block is not base64.
    #[error("{0}: not a PEM `PRIVATE KEY` document")]
    NotPem(&'static str),
    /// The block decodes but is not a PKCS#8 Ed25519 private key.
    #[error("{0}: not a PKCS#8 Ed25519 private key (`openssl genpkey -algorithm ed25519`)")]
    NotEd25519(&'static str),
    /// The current and next keys are the same key, so a rotation could never
    /// change what a client pins.
    #[error("the current and next envelope signing keys are the same key")]
    Identical,
}

/// The two Ed25519 keys the server holds: the one it signs every response
/// with, and the one it will sign with after the next rotation.
///
/// Clients pin **both** public keys (`{current, next}`), so a rotation is
/// three steps with no flag day: ship clients that pin the new `next`, promote
/// `next` to `current` on the server (and mint a fresh `next`), then retire the
/// old key from the clients. The server itself only ever signs with `current`.
pub struct ServerKeys {
    signer: Ed25519Signer,
    next: [u8; 32],
}

impl ServerKeys {
    /// Parses the two PKCS#8 PEM documents `--envelope-signing-key-file` and
    /// `--envelope-next-signing-key-file` name.
    ///
    /// ```
    /// use sms_api::envelope::ServerKeys;
    ///
    /// let current = b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIBYj+/M+/6TszBaUcA1r2ZWEbwRFmRk5gIV5gaAPtaOL\n-----END PRIVATE KEY-----\n";
    /// let next = b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIAgoX/+nAaZjmWgdPspG+4tKf1bYd7suk6ztD4wAKxOP\n-----END PRIVATE KEY-----\n";
    /// let keys = ServerKeys::from_pems(current, next).unwrap();
    /// assert_ne!(keys.public_keys().0, keys.public_keys().1);
    ///
    /// // The same key twice, and anything that is not an Ed25519 key, refuse to boot.
    /// assert!(ServerKeys::from_pems(current, current).is_err());
    /// assert!(ServerKeys::from_pems(b"not a pem", next).is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// [`ServerKeyError`] for a document that is not a PKCS#8 Ed25519 private
    /// key, or when both documents hold the same key.
    pub fn from_pems(current: &[u8], next: &[u8]) -> Result<Self, ServerKeyError> {
        let current = seed_from_pem(current, "the envelope signing key")?;
        let next = seed_from_pem(next, "the next envelope signing key")?;
        if current == next {
            return Err(ServerKeyError::Identical);
        }
        let signer = Ed25519Signer::from_seed(&current);
        let next = Ed25519Signer::from_seed(&next)
            .verify_key()
            .ed25519_bytes()
            .ok_or(ServerKeyError::NotEd25519("the next envelope signing key"))?;
        Ok(Self { signer, next })
    }

    /// A key pair from two raw seeds, for tests and tools that mint keys
    /// themselves.
    ///
    /// # Errors
    ///
    /// [`ServerKeyError::Identical`] when the seeds are equal.
    pub fn from_seeds(current: &[u8; 32], next: &[u8; 32]) -> Result<Self, ServerKeyError> {
        if current == next {
            return Err(ServerKeyError::Identical);
        }
        Self::from_pems(&pem_of_seed(current), &pem_of_seed(next))
    }

    /// The raw 32-byte public keys, `(current, next)`: what a client pins.
    ///
    /// # Panics
    ///
    /// Never in practice: an Ed25519 signer's verify key always has its 32
    /// bytes.
    #[must_use]
    pub fn public_keys(&self) -> ([u8; 32], [u8; 32]) {
        (
            self.signer
                .verify_key()
                .ed25519_bytes()
                .expect("an Ed25519 signer's verify key is Ed25519"),
            self.next,
        )
    }

    /// The two public keys as unpadded base64url, one per line, labelled:
    /// what `sms-gateway server-keys` prints and an integrator pins.
    ///
    /// ```
    /// use sms_api::envelope::ServerKeys;
    ///
    /// let keys = ServerKeys::from_seeds(&[1; 32], &[2; 32]).unwrap();
    /// let report = keys.report();
    /// assert!(report.starts_with("current: "));
    /// assert!(report.contains("\nnext: "));
    /// ```
    #[must_use]
    pub fn report(&self) -> String {
        let (current, next) = self.public_keys();
        format!(
            "current: {}\nnext: {}\n",
            URL_SAFE_NO_PAD.encode(current),
            URL_SAFE_NO_PAD.encode(next)
        )
    }

    /// The signer that seals every response.
    #[must_use]
    pub fn into_signer(self) -> Ed25519Signer {
        self.signer
    }
}

/// The signer in one PKCS#8 PEM Ed25519 private key: what the worker's `hooks`
/// role seals webhook deliveries with (VS-8). It is the same file the gateway
/// signs its answers with (`--envelope-signing-key-file`), so a receiver pins one
/// pair of public keys for both. Only the current key signs; the worker has no
/// use for `next`.
///
/// ```
/// use sms_api::envelope::{signer_from_pem, ServerKeys};
///
/// let current = b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIBYj+/M+/6TszBaUcA1r2ZWEbwRFmRk5gIV5gaAPtaOL\n-----END PRIVATE KEY-----\n";
/// let next = b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIAgoX/+nAaZjmWgdPspG+4tKf1bYd7suk6ztD4wAKxOP\n-----END PRIVATE KEY-----\n";
/// let signer = signer_from_pem(current).unwrap();
/// // The public half is what `sms-gateway server-keys` prints as `current`.
/// let keys = ServerKeys::from_pems(current, next).unwrap();
/// assert_eq!(signer.verify_key().ed25519_bytes().unwrap(), keys.public_keys().0);
/// assert!(signer_from_pem(b"not a pem").is_err());
/// ```
///
/// # Errors
///
/// [`ServerKeyError`] for a document that is not a PKCS#8 Ed25519 private key.
pub fn signer_from_pem(pem: &[u8]) -> Result<Ed25519Signer, ServerKeyError> {
    let seed = seed_from_pem(pem, "the envelope signing key")?;
    Ok(Ed25519Signer::from_seed(&seed))
}

/// A PKCS#8 PEM for a raw Ed25519 seed: the inverse of [`seed_from_pem`].
#[must_use]
pub fn pem_of_seed(seed: &[u8; 32]) -> Vec<u8> {
    let der = [PKCS8_ED25519_PREFIX.as_slice(), seed.as_slice()].concat();
    format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        STANDARD.encode(der)
    )
    .into_bytes()
}

fn seed_from_pem(pem: &[u8], name: &'static str) -> Result<[u8; 32], ServerKeyError> {
    let text = std::str::from_utf8(pem).map_err(|_| ServerKeyError::NotPem(name))?;
    let body: String = text
        .lines()
        .map(str::trim)
        .skip_while(|line| *line != "-----BEGIN PRIVATE KEY-----")
        .skip(1)
        .take_while(|line| *line != "-----END PRIVATE KEY-----")
        .collect();
    if body.is_empty() {
        return Err(ServerKeyError::NotPem(name));
    }
    let der = STANDARD
        .decode(body.as_bytes())
        .map_err(|_| ServerKeyError::NotPem(name))?;
    let seed = der
        .strip_prefix(&PKCS8_ED25519_PREFIX)
        .ok_or(ServerKeyError::NotEd25519(name))?;
    seed.try_into()
        .map_err(|_| ServerKeyError::NotEd25519(name))
}

/// The envelope's replay cache over the `EnvelopeNonce` table.
///
/// Atomicity is the unique index: `create` either inserts or fails with
/// SQLSTATE 23505, so two replicas receiving one request at once record it
/// exactly once between them. A conflicting row that has *expired* (the worker
/// has not swept it yet) is deleted and the insert tried once more, so a nonce
/// space reused after its window is not refused until the next sweep; the
/// request would fail the layer's `iat` freshness check if it were stale, so
/// this only keeps the table from refusing a legitimate reuse.
pub struct PgNonceStore {
    db: Cratestack,
    sys: CratestackContext,
}

impl PgNonceStore {
    /// A store writing through `db` under the internal system context.
    #[must_use]
    pub fn new(db: Cratestack) -> Self {
        Self {
            db,
            sys: system_context("sms-api:nonces"),
        }
    }

    async fn insert(&self, nonce: &str, expires_at: DateTime<Utc>) -> Result<(), CratestackError> {
        self.db
            .envelope_nonce()
            .create(schema::CreateEnvelopeNonceInput {
                nonce: nonce.to_owned(),
                expiresAt: expires_at,
            })
            .run(&self.sys)
            .await
            .map(|_| ())
    }
}

fn is_duplicate(error: &CratestackError) -> bool {
    error.db_sqlstate() == Some(UNIQUE_VIOLATION)
}

#[async_trait]
impl NonceStore for PgNonceStore {
    async fn record_if_unseen(
        &self,
        nonce: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<bool, CratestackError> {
        match self.insert(nonce, expires_at).await {
            Ok(()) => return Ok(true),
            Err(error) if is_duplicate(&error) => {}
            Err(error) => return Err(error),
        }

        // Seen. Unless the entry's window has closed, that is the answer.
        let existing = self
            .db
            .envelope_nonce()
            .find_many()
            .where_expr(FilterExpr::from(
                envelope_nonce::nonce().eq(nonce.to_owned()),
            ))
            .limit(1)
            .run(&self.sys)
            .await?
            .into_iter()
            .next();
        let Some(existing) = existing else {
            // Swept between the two statements: nobody holds it now.
            return self.insert_once_more(nonce, expires_at).await;
        };
        if existing.expiresAt > Utc::now() {
            return Ok(false);
        }
        match self
            .db
            .envelope_nonce()
            .delete(existing.id)
            .run(&self.sys)
            .await
        {
            Ok(_) | Err(CratestackError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
        self.insert_once_more(nonce, expires_at).await
    }
}

impl PgNonceStore {
    /// The second and last insert: losing it to a concurrent taker is a
    /// replay, not an error.
    async fn insert_once_more(
        &self,
        nonce: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<bool, CratestackError> {
        match self.insert(nonce, expires_at).await {
            Ok(()) => Ok(true),
            Err(error) if is_duplicate(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

/// Names the principal a verified request is charged to, for the rate limiter
/// and the idempotency layer: `client:<clientId>` or `user:<id>`.
///
/// It is also where an inactive signer is refused. The key resolved (so the
/// signature is good), but a deactivated client, a retired one, or a person
/// who may no longer sign in is [`CratestackError::Unauthorized`] here, which
/// the layer answers with its coarse unsigned 401: the same answer as a
/// forgery, so nothing is learnt from it.
pub struct SignerPrincipal {
    directory: SignerDirectory,
}

impl SignerPrincipal {
    /// A mapper reading standing through `directory`.
    #[must_use]
    pub fn new(directory: SignerDirectory) -> Self {
        Self { directory }
    }
}

#[async_trait]
impl PrincipalMapper for SignerPrincipal {
    async fn principal(&self, verified: &VerifiedRequest<'_>) -> Result<String, CratestackError> {
        self.directory
            .standing(verified.signer(), Utc::now())
            .await
            .map(|standing| standing.principal())
    }
}

/// The mode every op runs in, from the doors a deployment opened.
///
/// | signed envelope | a token served | mode       | what the layer does                                          |
/// | --------------- | -------------- | ---------- | ------------------------------------------------------------ |
/// | on              | off            | `Required` | refuses anything unsigned with the coarse 401                |
/// | on              | on             | `Optional` | opens a signed request; lets a plain one through to the door |
/// | off             | on             | `Off`      | passes plain traffic; refuses a COSE body with the 415       |
///
/// "A token served" is `machine_tokens` or `human_login` (a person's access
/// token, VSA-4b): which realms the door accepts is the verifier's business, not
/// the mode's.
///
/// `Off` is what "keypair is off" has to mean: a request with a COSE body is
/// never forwarded unverified, so a deployment that shut the signed door answers
/// a signed request with the layer's own refusal and not with a quiet 401 that
/// looks like a bad key.
#[must_use]
pub(crate) fn mode_for(doors: &ApiDoors) -> EnvelopeMode {
    match (doors.keypair(), doors.bearer()) {
        (true, false) => EnvelopeMode::Required,
        (true, true) => EnvelopeMode::Optional,
        // `ApiDoors` cannot be built with neither door open.
        (false, _) => EnvelopeMode::Off,
    }
}

/// Builds the envelope layer for the generated router: keys and nonces from
/// Postgres, JSON payloads only, and a mode that follows the doors
/// ([`mode_for`]).
///
/// Three settings do not follow the mode, and each is what keeps the second door
/// from weakening the first:
///
/// - **`unresolved_mode(Required)`.** A bare `EnvelopeMode` used as the policy
///   answers for the routes the layer cannot bind to an op with its own value, so
///   under `Optional` or `Off` a route the resolver does not know would pass
///   through unsigned and unchecked. Pinned to `Required` it fails closed (a
///   500) whatever the doors are.
/// - **`response_seal_policy(false)`.** Under `Optional` the default seals the
///   answer to an unsigned request that names `application/cose` in `Accept`.
///   A token caller is not a key holder: it gets plain JSON, and with it no
///   response seal and no contract-lock 426, which only a sealed exchange
///   carries. A signed request is always sealed, and that is not this policy's
///   call.
/// - **the principal mapper** names `client:<clientId>` for a signer, the same
///   string the bearer door records for the same client, so the doors share one
///   rate-limit bucket and one idempotency namespace.
///
/// It must be applied as the router's last `.layer(..)` apart from the coarse
/// source limiter (see [`crate::router::router`]).
///
/// # Errors
///
/// [`CratestackError`] when the layer refuses its own configuration.
pub fn layer(
    signer: Ed25519Signer,
    db: &Cratestack,
    directory: SignerDirectory,
    doors: &ApiDoors,
) -> Result<EnvelopeLayer, CratestackError> {
    let envelope = CoseEnvelope::server(
        CoseMode::Sign1,
        Arc::new(signer),
        Arc::new(DirectoryKeys::new(db.clone())),
        Arc::new(PgNonceStore::new(db.clone())),
    )
    .build()?;
    schema::axum::envelope_layer(envelope, mode_for(doors), AUDIENCE)
        .unresolved_mode(EnvelopeMode::Required)
        .response_seal_policy(|_: &UnsignedRequest<'_>| false)
        .payload_media_types([PAYLOAD_TYPE], [PAYLOAD_TYPE])
        .principal_mapper(SignerPrincipal::new(directory))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURRENT: &[u8] = b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIBYj+/M+/6TszBaUcA1r2ZWEbwRFmRk5gIV5gaAPtaOL\n-----END PRIVATE KEY-----\n";
    const NEXT: &[u8] = b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIAgoX/+nAaZjmWgdPspG+4tKf1bYd7suk6ztD4wAKxOP\n-----END PRIVATE KEY-----\n";

    fn doors(keypair: bool, bearer: bool) -> ApiDoors {
        struct Never;
        #[async_trait::async_trait]
        impl crate::bearer::BearerVerifier for Never {
            async fn verify(
                &self,
                _token: &str,
            ) -> Result<crate::bearer::VerifiedBearer, crate::bearer::BearerRefusal> {
                Err(crate::bearer::BearerRefusal::Malformed)
            }
        }
        ApiDoors::new(
            keypair,
            bearer.then(|| Arc::new(Never) as Arc<dyn crate::bearer::BearerVerifier>),
        )
        .unwrap()
    }

    /// The table in [`mode_for`]'s doc, row by row.
    #[test]
    fn the_doors_set_the_envelope_mode() {
        assert_eq!(mode_for(&doors(true, false)), EnvelopeMode::Required);
        assert_eq!(mode_for(&doors(true, true)), EnvelopeMode::Optional);
        assert_eq!(mode_for(&doors(false, true)), EnvelopeMode::Off);
    }

    /// A route the router matched but the schema does not generate: under a bare
    /// `Optional` or `Off` policy the layer would let it through unsigned. The
    /// layer built here fails it closed (a 500) in every mode.
    #[tokio::test]
    async fn a_route_the_layer_cannot_bind_fails_closed_in_every_mode() {
        use cratestack::axum::body::Body;
        use cratestack::axum::http::Request;
        use cratestack::axum::routing::get;
        use tower::ServiceExt;

        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        let db = Cratestack::builder(pool).build();
        for (keypair, bearer) in [(true, false), (true, true), (false, true)] {
            let layer = layer(
                Ed25519Signer::from_seed(&[1; 32]),
                &db,
                SignerDirectory::new(db.clone()),
                &doors(keypair, bearer),
            )
            .unwrap();
            let app = cratestack::axum::Router::new()
                .route(
                    "/not-in-the-schema",
                    get(|| async { "reached the handler" }),
                )
                .layer(layer);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/not-in-the-schema")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status().as_u16(),
                500,
                "keypair {keypair}, bearer {bearer}"
            );
        }
    }

    #[test]
    fn one_file_is_one_key() {
        let a = ServerKeys::from_pems(CURRENT, NEXT).unwrap();
        let b = ServerKeys::from_pems(CURRENT, NEXT).unwrap();
        assert_eq!(a.public_keys(), b.public_keys());
    }

    #[test]
    fn crlf_endings_and_text_around_the_block_are_tolerated() {
        let crlf = String::from_utf8(CURRENT.to_vec())
            .unwrap()
            .replace('\n', "\r\n");
        let wrapped = format!("# written by dev-keys\r\n{crlf}");
        let a = ServerKeys::from_pems(wrapped.as_bytes(), NEXT).unwrap();
        let b = ServerKeys::from_pems(CURRENT, NEXT).unwrap();
        assert_eq!(a.public_keys(), b.public_keys());
    }

    #[test]
    fn the_same_key_twice_is_refused_even_spelled_differently() {
        assert_eq!(
            ServerKeys::from_pems(CURRENT, CURRENT).err(),
            Some(ServerKeyError::Identical)
        );
        assert_eq!(
            ServerKeys::from_seeds(&[9; 32], &[9; 32]).err(),
            Some(ServerKeyError::Identical)
        );
    }

    #[test]
    fn a_key_of_another_algorithm_is_refused() {
        // An RSA-shaped header (`rsaEncryption`, OID 1.2.840.113549.1.1.1).
        let rsa = b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYJKoZIhvcNAQEBBQAEIgQg\n-----END PRIVATE KEY-----\n";
        assert!(matches!(
            ServerKeys::from_pems(rsa, NEXT),
            Err(ServerKeyError::NotEd25519(_))
        ));
        assert!(matches!(
            ServerKeys::from_pems(b"not a pem", NEXT),
            Err(ServerKeyError::NotPem(_))
        ));
        assert!(matches!(
            ServerKeys::from_pems(
                CURRENT,
                b"-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"
            ),
            Err(ServerKeyError::NotEd25519(_))
        ));
    }

    #[test]
    fn a_pem_made_from_a_seed_reads_back_as_that_seed() {
        let seed = [7_u8; 32];
        assert_eq!(seed_from_pem(&pem_of_seed(&seed), "k").unwrap(), seed);
    }

    #[test]
    fn the_report_names_both_keys_and_never_a_seed() {
        let keys = ServerKeys::from_pems(CURRENT, NEXT).unwrap();
        let report = keys.report();
        assert!(report.starts_with("current: "));
        assert!(report.contains("\nnext: "));
        assert!(!report.contains("PRIVATE"));
        assert_eq!(report.lines().count(), 2);
    }
}
