#![doc = include_str!("client_keys.md")]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use cratestack::cose::CoseVerifyKey;
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use tracing::info;

use crate::errors::UNIQUE_VIOLATION;
use crate::schema::{self, app_client, client_key};

/// An Ed25519 public key is 32 bytes, exactly. The `client_keys` table has a
/// CHECK for the same number (`0006_client_keys`).
pub const ED25519_PUBLIC_KEY_LEN: usize = 32;

/// `ClientKey.label`'s `@length(max: ...)`.
const LABEL_MAX_CHARS: usize = 64;

/// The constraint a concurrent registration of one thumbprint loses on.
const THUMBPRINT_CONSTRAINT: &str = "client_keys_thumbprint_key";

/// A validated public key and the identity derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPublicKey {
    /// The raw 32-byte Ed25519 point, as stored in `ClientKey.publicKey`.
    pub bytes: [u8; ED25519_PUBLIC_KEY_LEN],
    /// RFC 9679 COSE key thumbprint, lowercase hex: `ClientKey.thumbprint`.
    pub thumbprint: String,
}

/// Decodes `publicKey` (base64url, no padding) and derives its thumbprint.
///
/// The thumbprint comes from `cratestack::cose`, never from the caller and
/// never from a copy of its encoding here: `CoseVerifyKey::ed25519` rejects
/// bytes that are not a point encoding and computes the thumbprint with the
/// crate's own `thumbprint` module, so the value stored is exactly the one
/// `Opened::key_thumbprint` reports for a request signed by that key.
///
/// ```
/// # use sms_api::client_keys::parse_public_key;
/// // RFC 8032 test vector 1's public key, base64url.
/// let parsed = parse_public_key("11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo").unwrap();
/// assert_eq!(parsed.bytes[0], 0xd7);
/// assert_eq!(parsed.thumbprint.len(), 64);
/// // Padding, the wrong length, and the standard (non-url) alphabet are refused.
/// assert!(parse_public_key("11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo=").is_err());
/// assert!(parse_public_key("AAAA").is_err());
/// assert!(parse_public_key("11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo").is_err());
/// ```
///
/// # Errors
///
/// [`CratestackError::Validation`] when the text is not unpadded base64url,
/// does not decode to 32 bytes, or is not a valid Ed25519 point encoding.
pub fn parse_public_key(public_key: &str) -> Result<ParsedPublicKey, CratestackError> {
    let decoded = URL_SAFE_NO_PAD.decode(public_key.trim()).map_err(|_| {
        CratestackError::Validation(
            "publicKey must be the raw Ed25519 public key as unpadded base64url".to_owned(),
        )
    })?;
    let bytes: [u8; ED25519_PUBLIC_KEY_LEN] = decoded.try_into().map_err(|decoded: Vec<u8>| {
        CratestackError::Validation(format!(
            "publicKey must be {ED25519_PUBLIC_KEY_LEN} bytes (a raw Ed25519 public key), got {}",
            decoded.len()
        ))
    })?;
    let key = CoseVerifyKey::ed25519(&bytes)?;
    Ok(ParsedPublicKey {
        bytes,
        thumbprint: hex::encode(key.thumbprint()),
    })
}

fn clean_label(label: Option<String>) -> Result<Option<String>, CratestackError> {
    let Some(label) = label else { return Ok(None) };
    let trimmed = label.trim();
    if trimmed.is_empty() || trimmed.chars().count() > LABEL_MAX_CHARS {
        return Err(CratestackError::Validation(format!(
            "label must be 1-{LABEL_MAX_CHARS} characters"
        )));
    }
    Ok(Some(trimmed.to_owned()))
}

/// `registerAppClientKey`: see the module doc. `sys` is the internal system
/// context the writes run under; the caller's own `owner`/`admin` role was
/// already checked by the procedure's `@allow`.
pub async fn register(
    db: &schema::IsolatedCratestack,
    sys: &CratestackContext,
    args: schema::RegisterClientKeyInput,
) -> Result<schema::ClientKey, CratestackError> {
    let parsed = parse_public_key(&args.publicKey)?;
    let label = clean_label(args.label)?;
    let app_client_id = args.appClientId;

    // Two registrations of one new thumbprint race on the unique index: the
    // loser's INSERT fails with 23505 and aborts its transaction, so it is
    // answered by running the whole check again - which now finds the winner's
    // row and takes the idempotent branch below - rather than by a 500.
    let mut attempt = 0;
    loop {
        attempt += 1;
        let outcome = db
            .transaction(async |tx| {
                let client = db
                    .app_client()
                    .find_many()
                    .where_expr(FilterExpr::from(app_client::id().eq(app_client_id.clone())))
                    .limit(1)
                    .run_in_tx(tx, sys)
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        CratestackError::NotFound(format!("no AppClient with id {app_client_id}"))
                    })?;
                if !client.active || client.retiredAt.is_some() {
                    return Err(CratestackError::Validation(format!(
                        "AppClient {} is retired or inactive",
                        client.id
                    )));
                }

                let existing = db
                    .client_key()
                    .find_many()
                    .where_expr(FilterExpr::from(
                        client_key::thumbprint().eq(parsed.thumbprint.clone()),
                    ))
                    .limit(1)
                    .run_in_tx(tx, sys)
                    .await?
                    .into_iter()
                    .next();
                if let Some(existing) = existing {
                    return same_registration(existing, &app_client_id).map(|key| (key, false));
                }

                let created = db
                    .client_key()
                    .create(schema::CreateClientKeyInput {
                        appClientId: app_client_id.clone(),
                        alg: schema::ClientKeyAlg::ed25519,
                        publicKey: parsed.bytes.to_vec(),
                        thumbprint: parsed.thumbprint.clone(),
                        label: label.clone(),
                        revokedAt: None,
                    })
                    .run_in_tx(tx, sys)
                    .await?
                    .value;
                Ok((created, true))
            })
            .await;

        match outcome {
            Ok((key, created)) => {
                info!(
                    key_id = %key.id,
                    app_client_id = %key.appClientId,
                    thumbprint = %key.thumbprint,
                    created,
                    "client key registered"
                );
                return Ok(key);
            }
            Err(error)
                if attempt < 2
                    && error.db_sqlstate() == Some(UNIQUE_VIOLATION)
                    && error.db_constraint() == Some(THUMBPRINT_CONSTRAINT) => {}
            Err(error) => return Err(error),
        }
    }
}

/// The answer for a thumbprint that is already registered: the same row again
/// when it is this client's and still live (so a retry or a second CLI run is
/// not an error), a conflict when it was revoked or belongs to another client.
fn same_registration(
    existing: schema::ClientKey,
    app_client_id: &str,
) -> Result<schema::ClientKey, CratestackError> {
    if existing.appClientId != app_client_id {
        return Err(CratestackError::Conflict(
            "this public key is already registered to another client".to_owned(),
        ));
    }
    if existing.revokedAt.is_some() {
        return Err(CratestackError::Conflict(
            "this public key was revoked; generate a new keypair, a revoked key is never reinstated"
                .to_owned(),
        ));
    }
    Ok(existing)
}

/// `revokeAppClientKey`: stamps `revokedAt` once. Revoking an already revoked
/// key returns it unchanged, so the first revocation time is never moved.
pub async fn revoke(
    db: &schema::IsolatedCratestack,
    sys: &CratestackContext,
    args: schema::RevokeClientKeyInput,
) -> Result<schema::ClientKey, CratestackError> {
    let key_id = args.keyId;
    let revoked = db
        .transaction(async |tx| {
            let key = db
                .client_key()
                .find_many()
                .where_expr(FilterExpr::from(client_key::id().eq(key_id.clone())))
                .limit(1)
                .run_in_tx(tx, sys)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CratestackError::NotFound(format!("no ClientKey with id {key_id}"))
                })?;
            if key.revokedAt.is_some() {
                return Ok(key);
            }
            let updated = db
                .client_key()
                .update(key_id.clone())
                .set(schema::UpdateClientKeyInput {
                    revokedAt: Some(Some(Utc::now())),
                    ..Default::default()
                })
                .run_in_tx(tx, sys)
                .await?
                .value;
            Ok(updated)
        })
        .await?;
    info!(
        key_id = %revoked.id,
        app_client_id = %revoked.appClientId,
        thumbprint = %revoked.thumbprint,
        "client key revoked"
    );
    Ok(revoked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// RFC 8032 section 7.1 test 1.
    const PUBLIC: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

    /// RFC 9679 section 4 for an OKP key, spelled out by hand: the
    /// deterministically encoded map {1: 1, -1: 6, -2: x} is
    /// `a3 01 01 20 06 21 58 20 || x`, and the thumbprint is its SHA-256.
    /// The production path never does this - it asks `cratestack::cose` - so
    /// agreement here proves the stored value is the standard's, not
    /// merely self-consistent.
    fn thumbprint_by_hand(x: &[u8]) -> String {
        let mut cbor = vec![0xa3, 0x01, 0x01, 0x20, 0x06, 0x21, 0x58, 0x20];
        cbor.extend_from_slice(x);
        hex::encode(Sha256::digest(&cbor))
    }

    #[test]
    fn the_thumbprint_is_rfc_9679s_not_a_hash_of_the_raw_key() {
        let raw = hex::decode(PUBLIC).unwrap();
        let parsed = parse_public_key(&URL_SAFE_NO_PAD.encode(&raw)).unwrap();
        assert_eq!(parsed.bytes.as_slice(), raw.as_slice());
        assert_eq!(parsed.thumbprint, thumbprint_by_hand(&raw));
        assert_ne!(parsed.thumbprint, hex::encode(Sha256::digest(&raw)));
    }

    #[test]
    fn the_thumbprint_is_the_one_the_crate_reports_for_a_signed_request() {
        let raw: [u8; 32] = hex::decode(PUBLIC).unwrap().try_into().unwrap();
        let from_crate = cratestack::cose::thumbprint::okp_ed25519_thumbprint(&raw);
        let parsed = parse_public_key(&URL_SAFE_NO_PAD.encode(raw)).unwrap();
        assert_eq!(parsed.thumbprint, hex::encode(from_crate));
    }

    #[test]
    fn a_malformed_key_is_a_validation_error() {
        let raw = hex::decode(PUBLIC).unwrap();
        for bad in [
            String::new(),
            "not base64 !".to_owned(),
            URL_SAFE_NO_PAD.encode(&raw[..31]),
            URL_SAFE_NO_PAD.encode([&raw[..], &[0]].concat()),
            // Standard alphabet and padding are not base64url-no-pad.
            base64::engine::general_purpose::STANDARD.encode(&raw),
            format!("{}=", URL_SAFE_NO_PAD.encode(&raw)),
        ] {
            assert!(
                matches!(parse_public_key(&bad), Err(CratestackError::Validation(_))),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn a_label_is_trimmed_and_bounded() {
        assert_eq!(clean_label(None).unwrap(), None);
        assert_eq!(
            clean_label(Some("  console composer ".to_owned())).unwrap(),
            Some("console composer".to_owned())
        );
        assert!(clean_label(Some("   ".to_owned())).is_err());
        assert!(clean_label(Some("x".repeat(LABEL_MAX_CHARS + 1))).is_err());
        assert!(clean_label(Some("x".repeat(LABEL_MAX_CHARS))).is_ok());
    }
}
