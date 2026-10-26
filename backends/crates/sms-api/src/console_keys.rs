#![doc = include_str!("console_keys.md")]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use cratestack::cose::CoseVerifyKey;
use cratestack::{CratestackContext, CratestackError, FilterExpr, Value};
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};
use tracing::info;

use crate::auth::{Principal, PrincipalKind};
use crate::errors::UNIQUE_VIOLATION;
use crate::schema::{self, console_key, federated_link, user};

/// The scope an app client must hold to call `registerConsoleKey`. Checked
/// against the token's `scope` claim only, never against a human's `perms`:
/// it is not a role permission and no seeded role may ever grant it
/// (`permission_vocabulary.rs` would demand one for a `require_permission`
/// literal, which is why this has its own check, [`require_pair_scope`]).
pub const PAIR_SCOPE: &str = "console:pair";

/// The `FederatedLink.issuer` of a person who signed in at vaam.
pub const FEDERATION_ISSUER: &str = "vaam";

/// The role a person gets the first time they pair: `admin`, never `owner`
/// (owner ruling R2, 2026-10-26). `owner` is the top of the console's role
/// ladder - only an owner may delete a `User` or edit a `Role` - and the
/// #434 incident was an admin writing `owner` into a `User`. Pairing is not
/// allowed to be a second way to do that. Two independent things hold this:
/// this constant, and [`creator_context`], whose role is the literal
/// `admin` rather than this constant so `User.create`'s own policy
/// (`roleKey != 'owner'` for an admin) still refuses `owner` if this ever
/// changes.
pub const PAIRED_ROLE: &str = "admin";

/// A proof of possession is accepted when its `issuedAt` is within this many
/// seconds of the server's clock, either way.
pub const PROOF_MAX_SKEW_SECS: i64 = 300;

/// A console key's life from the moment it is paired (owner ruling R3).
pub const KEY_LIFETIME_DAYS: i64 = 30;

/// The SEC1 uncompressed encoding of a P-256 point: `0x04 || X || Y`.
pub const P256_SEC1_LEN: usize = 65;

/// Domain-separation tag of the bytes the console key signs, so the signature
/// can stand for nothing else (vaam's admin pairing uses its own,
/// `vaam.admin-pair.v1`; a signature for one is no proof for the other).
pub const PROOF_TAG: &str = "vsms.console-pair.v1";

/// `ConsoleKey.label`'s and `User.displayName`'s `@length(max: ...)`.
const LABEL_MAX_CHARS: usize = 64;
const DISPLAY_NAME_MAX_CHARS: usize = 128;
/// `RegisterConsoleKeyInput.vaamUserId`'s `@length(max: ...)`.
const VAAM_USER_ID_MAX_CHARS: usize = 64;

/// The synthetic mailbox of a person who has no vsms-side email. `.invalid` is
/// reserved by RFC 2606 and can never resolve, so nothing mailed here arrives
/// anywhere and the address cannot collide with a real person's.
const SYNTHETIC_EMAIL_DOMAIN: &str = "vaam.invalid";

const THUMBPRINT_CONSTRAINT: &str = "console_keys_thumbprint_key";
const LINK_CONSTRAINT: &str = "federated_links_issuer_subject_key";
const USER_EMAIL_CONSTRAINT: &str = "users_email_key";

/// A validated public key and the identity derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedConsoleKey {
    /// The SEC1 uncompressed point, as stored in `ConsoleKey.publicKey`.
    pub bytes: [u8; P256_SEC1_LEN],
    /// RFC 9679 COSE key thumbprint (EC2/P-256), lowercase hex:
    /// `ConsoleKey.thumbprint`.
    pub thumbprint: String,
}

fn validation(message: impl Into<String>) -> CratestackError {
    CratestackError::Validation(message.into())
}

/// Decodes `publicKey` (SEC1 uncompressed P-256, base64url, no padding) and
/// derives its thumbprint.
///
/// The point must be on the curve and in the uncompressed encoding, the one a
/// browser's `exportKey("raw")` returns. A compressed encoding is a valid
/// P-256 key but a second spelling of it, so it is refused rather than
/// normalised: what is stored is exactly what the caller signed
/// ([`possession_message`] names the text, not the point).
///
/// ```
/// # use sms_api::console_keys::parse_public_key;
/// // The generator of P-256: SEC1 uncompressed, base64url.
/// let g = "BGsX0fLhLEJH-Lzm5WOkQPJ3A32BLeszoPShOUXYmMKWT-NC4v4af5uO5-tKfA-eFivOM1drMV7Oy7ZAaDe_UfU";
/// let parsed = parse_public_key(g).unwrap();
/// assert_eq!(parsed.bytes[0], 0x04);
/// assert_eq!(parsed.thumbprint.len(), 64);
/// // Padding, the wrong length and a point off the curve are refused.
/// assert!(parse_public_key(&format!("{g}=")).is_err());
/// assert!(parse_public_key("AAAA").is_err());
/// ```
///
/// # Errors
///
/// [`CratestackError::Validation`] when the text is not unpadded base64url,
/// is not 65 bytes starting `0x04`, or is not a point on P-256.
pub fn parse_public_key(public_key: &str) -> Result<ParsedConsoleKey, CratestackError> {
    let decoded = URL_SAFE_NO_PAD.decode(public_key.trim()).map_err(|_| {
        validation("publicKey must be the SEC1 uncompressed P-256 point as unpadded base64url")
    })?;
    let bytes: [u8; P256_SEC1_LEN] = decoded.try_into().map_err(|decoded: Vec<u8>| {
        validation(format!(
            "publicKey must be {P256_SEC1_LEN} bytes (a SEC1 uncompressed P-256 point), got {}",
            decoded.len()
        ))
    })?;
    if bytes[0] != 0x04 {
        return Err(validation(
            "publicKey must be the uncompressed SEC1 form (first byte 0x04); a compressed point is refused",
        ));
    }
    let key = CoseVerifyKey::p256_sec1(&bytes)?;
    Ok(ParsedConsoleKey {
        bytes,
        thumbprint: hex::encode(key.thumbprint()),
    })
}

/// The exact bytes the console key signs to prove it holds the private half:
/// `"vsms.console-pair.v1\n" + publicKey + "\n" + issuedAt` (unix seconds,
/// decimal). `publicKey` is the base64url text the caller sent, not the
/// decoded point.
///
/// ```
/// # use sms_api::console_keys::possession_message;
/// assert_eq!(
///     possession_message("a2V5", 1_700_000_000),
///     b"vsms.console-pair.v1\na2V5\n1700000000".to_vec()
/// );
/// ```
#[must_use]
pub fn possession_message(public_key: &str, issued_at_unix: i64) -> Vec<u8> {
    format!("{PROOF_TAG}\n{public_key}\n{issued_at_unix}").into_bytes()
}

/// Checks a raw ECDSA-P-256-SHA-256 signature (`r || s`, 64 bytes, base64url
/// without padding - what `WebCrypto`'s `sign` returns) over `message` against
/// the registered point.
///
/// A high `s` is accepted: `WebCrypto` does not normalise it, so half of an
/// honest console's signatures have one. That is safe here because the
/// signature is not bound to anything that hashes its bytes (unlike a request
/// envelope's, where cratestack-cose requires low `s`); it proves possession
/// once, and replaying it within the window changes nothing.
fn verify_possession(
    key: &ParsedConsoleKey,
    message: &[u8],
    proof: &str,
) -> Result<(), CratestackError> {
    let refused = || validation("possessionProof does not verify under publicKey");
    let signature = URL_SAFE_NO_PAD
        .decode(proof.trim())
        .map_err(|_| validation("possessionProof must be unpadded base64url"))?;
    let signature = Signature::from_slice(&signature)
        .map_err(|_| validation("possessionProof must be a 64-byte r||s ECDSA signature"))?;
    let verifying_key = VerifyingKey::from_sec1_bytes(&key.bytes).map_err(|_| refused())?;
    verifying_key
        .verify(message, &signature)
        .map_err(|_| refused())
}

/// Everything about a pairing request that can be decided without the
/// database: the key is a P-256 point, `issuedAt` is within
/// [`PROOF_MAX_SKEW_SECS`] of `now`, and the signature over
/// [`possession_message`] verifies under that key.
///
/// The order is the order an attacker learns least in: a stale request is
/// refused before any signature work, and a bad signature says nothing about
/// whether the key is registered.
///
/// # Errors
///
/// [`CratestackError::Validation`] for a malformed key, a stale or
/// future-dated `issuedAt`, or a signature that does not verify.
pub fn check_proof(
    args: &schema::RegisterConsoleKeyInput,
    now: DateTime<Utc>,
) -> Result<ParsedConsoleKey, CratestackError> {
    let key = parse_public_key(&args.publicKey)?;
    if (now - args.issuedAt).abs() > Duration::seconds(PROOF_MAX_SKEW_SECS) {
        return Err(validation(format!(
            "the pairing request expired: issuedAt must be within {PROOF_MAX_SKEW_SECS} seconds of the server's clock"
        )));
    }
    let message = possession_message(&args.publicKey, args.issuedAt.timestamp());
    verify_possession(&key, &message, &args.possessionProof)?;
    Ok(key)
}

/// `registerConsoleKey`'s Layer 2: the caller's token must carry
/// [`PAIR_SCOPE`]. Layer 1 (`@allow(auth().kind == "app")`) cannot say it - a
/// scope is not a field of `auth Principal` - and it also admits the internal
/// system context, which has no scope extension and so fails here, as it
/// should: nothing internal pairs a console on a person's behalf.
///
/// Unlike `require_permission` this does not consult `perms`, so no role a
/// human holds can ever stand in for the scope.
///
/// # Errors
///
/// [`CratestackError::Forbidden`] when the scope is absent.
pub fn require_pair_scope(ctx: &CratestackContext) -> Result<(), CratestackError> {
    match ctx.extensions.get("scope") {
        Some(Value::String(scope)) if sms_core::contains(scope, PAIR_SCOPE) => Ok(()),
        _ => Err(CratestackError::Forbidden(format!(
            "registerConsoleKey needs the {PAIR_SCOPE} scope"
        ))),
    }
}

fn clean_label(label: Option<String>) -> Result<Option<String>, CratestackError> {
    let Some(label) = label else { return Ok(None) };
    let trimmed = label.trim();
    if trimmed.is_empty() || trimmed.chars().count() > LABEL_MAX_CHARS {
        return Err(validation(format!(
            "label must be 1-{LABEL_MAX_CHARS} characters"
        )));
    }
    Ok(Some(trimmed.to_owned()))
}

/// The id of a person at vaam, restated here because a caller that does not
/// come through the generated route (the tests, a future in-process one)
/// skips the input type's `@regex`. It also becomes the local part of a
/// synthetic email, so it may carry nothing a mailbox cannot.
fn clean_vaam_user_id(id: &str) -> Result<&str, CratestackError> {
    if id.is_empty()
        || id.chars().count() > VAAM_USER_ID_MAX_CHARS
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(validation(format!(
            "vaamUserId must be 1-{VAAM_USER_ID_MAX_CHARS} characters of A-Z a-z 0-9 _ -"
        )));
    }
    Ok(id)
}

fn display_name_for(
    display_name: Option<String>,
    vaam_user_id: &str,
) -> Result<String, CratestackError> {
    let Some(name) = display_name else {
        return Ok(format!("vaam {vaam_user_id}"));
    };
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > DISPLAY_NAME_MAX_CHARS {
        return Err(validation(format!(
            "displayName must be 1-{DISPLAY_NAME_MAX_CHARS} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

/// The context `User.create` is written under. Its role is the literal
/// `admin` - the weakest role `User.create`'s policy admits - and
/// deliberately not [`PAIRED_ROLE`], so that policy's own
/// `roleKey != 'owner'` clause stays an independent second guard.
fn creator_context() -> CratestackContext {
    Principal {
        sub: "sms-api:console-pairing".to_owned(),
        kind: PrincipalKind::User,
        role: "admin".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// The `ConsoleKey` row for a key paired `now`.
fn new_key_input(
    user_id: &str,
    key: &ParsedConsoleKey,
    label: Option<String>,
    now: DateTime<Utc>,
) -> schema::CreateConsoleKeyInput {
    schema::CreateConsoleKeyInput {
        userId: user_id.to_owned(),
        alg: schema::ConsoleKeyAlg::esp256,
        publicKey: key.bytes.to_vec(),
        thumbprint: key.thumbprint.clone(),
        label,
        pairedAt: now,
        expiresAt: now + Duration::days(KEY_LIFETIME_DAYS),
        revokedAt: None,
    }
}

/// The answer for a thumbprint that is already registered: the same row again
/// when it is this person's and still live (so a retry or a second CLI run is
/// not an error), a conflict when it belongs to another user, was revoked, or
/// has expired. A key is never reinstated and its 30 days are never extended;
/// the console generates a new key.
fn same_registration(
    existing: schema::ConsoleKey,
    user_id: &str,
    now: DateTime<Utc>,
) -> Result<schema::ConsoleKey, CratestackError> {
    if existing.userId != user_id {
        return Err(CratestackError::Conflict(
            "this public key is already registered to another user".to_owned(),
        ));
    }
    if existing.revokedAt.is_some() {
        return Err(CratestackError::Conflict(
            "this public key was revoked; generate a new keypair, a revoked key is never reinstated"
                .to_owned(),
        ));
    }
    if existing.expiresAt <= now {
        return Err(CratestackError::Conflict(
            "this public key has expired; generate a new keypair, an expired key is never extended"
                .to_owned(),
        ));
    }
    Ok(existing)
}

fn deactivated() -> CratestackError {
    CratestackError::Forbidden("this person's vsms account is deactivated".to_owned())
}

/// First pairing of a person: their `User` (written under `creator`, the
/// weakest role `User.create` admits) and the `FederatedLink` that names them,
/// in the caller's transaction. Returns the new `User.id`.
async fn create_user_and_link(
    db: &schema::IsolatedCratestack,
    tx: &mut cratestack::Tx,
    sys: &CratestackContext,
    creator: &CratestackContext,
    vaam_user_id: &str,
    display_name: &str,
) -> Result<String, CratestackError> {
    let created = db
        .user()
        .create(schema::CreateUserInput {
            // `subject` is vsms's own OP subject: the row's own id, as
            // `provision-user` and the SSO broker write it. The id is not
            // known before the insert, so a unique placeholder is written and
            // corrected in this same transaction.
            subject: format!("pending-{}", cratestack::uuid::Uuid::new_v4()),
            email: format!("{vaam_user_id}@{SYNTHETIC_EMAIL_DOMAIN}"),
            displayName: display_name.to_owned(),
            roleKey: PAIRED_ROLE.to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run_in_tx(tx, creator)
        .await?
        .value;
    db.user()
        .update(created.id.clone())
        .set(schema::UpdateUserInput {
            subject: Some(created.id.clone()),
            ..Default::default()
        })
        .if_match(created.version)
        .run_in_tx(tx, creator)
        .await?;
    db.federated_link()
        .create(schema::CreateFederatedLinkInput {
            issuer: FEDERATION_ISSUER.to_owned(),
            subject: vaam_user_id.to_owned(),
            userId: created.id.clone(),
        })
        .run_in_tx(tx, sys)
        .await?;
    Ok(created.id)
}

/// `registerConsoleKey`: see the module doc. `sys` is the internal system
/// context the key and link writes run under; the caller's `console:pair`
/// scope was already checked by [`require_pair_scope`].
///
/// # Errors
///
/// [`CratestackError::Validation`] for a bad key, proof or `issuedAt`;
/// [`CratestackError::Forbidden`] for a deactivated account;
/// [`CratestackError::Conflict`] for a key that is another user's, revoked or
/// expired.
pub async fn register(
    db: &schema::IsolatedCratestack,
    sys: &CratestackContext,
    args: schema::RegisterConsoleKeyInput,
    now: DateTime<Utc>,
) -> Result<schema::ConsoleKey, CratestackError> {
    let parsed = check_proof(&args, now)?;
    let vaam_user_id = clean_vaam_user_id(&args.vaamUserId)?.to_owned();
    let display_name = display_name_for(args.displayName, &vaam_user_id)?;
    let creator = creator_context();

    // Two first pairings of one person (or one key) race on a unique index:
    // the loser's INSERT fails with 23505 and aborts its transaction, so it is
    // answered by running the whole decision again - which now finds the
    // winner's rows and takes the idempotent branch - rather than by a 500.
    let mut attempt = 0;
    loop {
        attempt += 1;
        let outcome = db
            .transaction(async |tx| {
                let link = db
                    .federated_link()
                    .find_many()
                    .where_expr(
                        FilterExpr::from(federated_link::issuer().eq(FEDERATION_ISSUER.to_owned()))
                            .and(federated_link::subject().eq(vaam_user_id.clone())),
                    )
                    .limit(1)
                    .run_in_tx(tx, sys)
                    .await?
                    .into_iter()
                    .next();
                let existing = db
                    .console_key()
                    .find_many()
                    .where_expr(FilterExpr::from(
                        console_key::thumbprint().eq(parsed.thumbprint.clone()),
                    ))
                    .limit(1)
                    .run_in_tx(tx, sys)
                    .await?
                    .into_iter()
                    .next();

                if let Some(existing) = existing {
                    let Some(link) = &link else {
                        return Err(CratestackError::Conflict(
                            "this public key is already registered to another user".to_owned(),
                        ));
                    };
                    let key = same_registration(existing, &link.userId, now)?;
                    return Ok((key, false));
                }

                let user_id = if let Some(link) = link {
                    let active = db
                        .user()
                        .find_many()
                        .where_expr(
                            FilterExpr::from(user::id().eq(link.userId.clone()))
                                .and(user::active().is_true()),
                        )
                        .limit(1)
                        .run_in_tx(tx, sys)
                        .await?
                        .into_iter()
                        .next()
                        .ok_or_else(deactivated)?;
                    active.id
                } else {
                    create_user_and_link(db, tx, sys, &creator, &vaam_user_id, &display_name)
                        .await?
                };

                let key = db
                    .console_key()
                    .create(new_key_input(&user_id, &parsed, None, now))
                    .run_in_tx(tx, sys)
                    .await?
                    .value;
                Ok((key, true))
            })
            .await;

        match outcome {
            Ok((key, created)) => {
                info!(
                    key_id = %key.id,
                    user_id = %key.userId,
                    thumbprint = %key.thumbprint,
                    created,
                    "console key registered"
                );
                return Ok(key);
            }
            Err(error)
                if attempt < 2
                    && error.db_sqlstate() == Some(UNIQUE_VIOLATION)
                    && matches!(
                        error.db_constraint(),
                        Some(THUMBPRINT_CONSTRAINT | LINK_CONSTRAINT | USER_EMAIL_CONSTRAINT)
                    ) => {}
            Err(error) => return Err(error),
        }
    }
}

/// The break-glass half of pairing (`sms-gateway pair-console-key`): attaches
/// `public_key` to an **existing, active** user, with no proof of possession -
/// the operator who can run the command inside the cluster against the
/// database is the authority, as for `provision-user`.
///
/// It creates no user and changes no role, so it cannot mint an owner; it can
/// only give an existing person (an owner locked out of the console included)
/// a way back in. Idempotent on the thumbprint exactly as [`register`] is.
///
/// # Errors
///
/// [`CratestackError::NotFound`] when no active user matches `user`;
/// [`CratestackError::Validation`] for a bad key or label;
/// [`CratestackError::Conflict`] for a key that is another user's, revoked or
/// expired.
pub async fn pair_existing_user(
    db: &schema::Cratestack,
    sys: &CratestackContext,
    user: &str,
    public_key: &str,
    label: Option<String>,
    now: DateTime<Utc>,
) -> Result<(schema::ConsoleKey, schema::User, bool), CratestackError> {
    let parsed = parse_public_key(public_key)?;
    let label = clean_label(label)?;

    // `@` is not allowed in an id (a Cuid) and is required in an email.
    let selector = if user.contains('@') {
        FilterExpr::from(user::email().eq(user.trim().to_owned()))
    } else {
        FilterExpr::from(user::id().eq(user.trim().to_owned()))
    };
    let found = db
        .user()
        .find_many()
        .where_expr(selector.and(user::active().is_true()))
        .limit(1)
        .run(sys)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| {
            CratestackError::NotFound(format!("no active user matches {user:?} (by email or id)"))
        })?;

    let mut attempt = 0;
    loop {
        attempt += 1;
        let existing = db
            .console_key()
            .find_many()
            .where_expr(FilterExpr::from(
                console_key::thumbprint().eq(parsed.thumbprint.clone()),
            ))
            .limit(1)
            .run(sys)
            .await?
            .into_iter()
            .next();
        if let Some(existing) = existing {
            let key = same_registration(existing, &found.id, now)?;
            return Ok((key, found, false));
        }
        match db
            .console_key()
            .create(new_key_input(&found.id, &parsed, label.clone(), now))
            .run(sys)
            .await
        {
            Ok(created) => {
                info!(
                    key_id = %created.id,
                    user_id = %found.id,
                    thumbprint = %created.thumbprint,
                    "console key registered by the operator"
                );
                return Ok((created, found, true));
            }
            Err(error)
                if attempt < 2
                    && error.db_sqlstate() == Some(UNIQUE_VIOLATION)
                    && error.db_constraint() == Some(THUMBPRINT_CONSTRAINT) => {}
            Err(error) => return Err(error),
        }
    }
}

/// `revokeConsoleKey`: stamps `revokedAt` once. Revoking an already revoked
/// key returns it unchanged, so the first revocation time is never moved. Who
/// may call it was decided before this runs (`@allow` and `@authorize`).
///
/// # Errors
///
/// [`CratestackError::NotFound`] when no key has the id.
pub async fn revoke(
    db: &schema::IsolatedCratestack,
    sys: &CratestackContext,
    args: schema::RevokeConsoleKeyInput,
) -> Result<schema::ConsoleKey, CratestackError> {
    let key_id = args.keyId;
    let revoked = db
        .transaction(async |tx| {
            let key = db
                .console_key()
                .find_many()
                .where_expr(FilterExpr::from(console_key::id().eq(key_id.clone())))
                .limit(1)
                .run_in_tx(tx, sys)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CratestackError::NotFound(format!("no ConsoleKey with id {key_id}"))
                })?;
            if key.revokedAt.is_some() {
                return Ok(key);
            }
            let updated = db
                .console_key()
                .update(key_id.clone())
                .set(schema::UpdateConsoleKeyInput {
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
        user_id = %revoked.userId,
        thumbprint = %revoked.thumbprint,
        "console key revoked"
    );
    Ok(revoked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::SigningKey;
    use p256::ecdsa::signature::Signer as _;
    use sha2::{Digest, Sha256};

    /// A fixed, non-random scalar: a deterministic fixture beats an RNG-drawn
    /// one, and `p256` here is built without its `rand_core` feature.
    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_slice(&[seed; 32]).expect("a non-zero scalar below the order")
    }

    fn public_key_text(key: &SigningKey) -> String {
        URL_SAFE_NO_PAD.encode(key.verifying_key().to_sec1_point(false).as_bytes())
    }

    fn request(key: &SigningKey, issued_at: DateTime<Utc>) -> schema::RegisterConsoleKeyInput {
        let public_key = public_key_text(key);
        let signature: Signature =
            key.sign(&possession_message(&public_key, issued_at.timestamp()));
        schema::RegisterConsoleKeyInput {
            vaamUserId: "usr_42".to_owned(),
            displayName: None,
            publicKey: public_key,
            possessionProof: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
            issuedAt: issued_at,
        }
    }

    #[test]
    fn the_paired_role_is_admin_and_the_creator_is_an_independent_admin() {
        assert_eq!(PAIRED_ROLE, "admin");
        assert_ne!(PAIRED_ROLE, "owner");
        // The creator's role is its own literal, not PAIRED_ROLE, so that
        // User.create's `roleKey != 'owner'` clause is a second guard.
        let creator = creator_context();
        let role = creator
            .auth
            .as_ref()
            .and_then(|auth| auth.fields.get("role"));
        assert_eq!(role, Some(&Value::String("admin".to_owned())));
    }

    #[test]
    fn a_key_lasts_thirty_days_from_the_moment_it_is_paired() {
        let key = parse_public_key(&public_key_text(&signing_key(1))).unwrap();
        let now = Utc::now();
        let input = new_key_input("usr", &key, None, now);
        assert_eq!(input.pairedAt, now);
        assert_eq!(input.expiresAt - input.pairedAt, Duration::days(30));
        assert_eq!(input.alg, schema::ConsoleKeyAlg::esp256);
        assert_eq!(input.publicKey.len(), 65);
        assert!(input.revokedAt.is_none());
    }

    /// RFC 9679 section 4 for an EC2 P-256 key, spelled out by hand: the
    /// deterministically encoded map {1: 2, -1: 1, -2: x, -3: y} is
    /// `a4 01 02 20 01 21 58 20 || x || 22 58 20 || y`, and the thumbprint is
    /// its SHA-256. The production path asks `cratestack::cose`, so agreement
    /// here proves the stored value is the standard's, not merely
    /// self-consistent.
    fn thumbprint_by_hand(sec1: &[u8]) -> String {
        let mut cbor = vec![0xa4, 0x01, 0x02, 0x20, 0x01, 0x21, 0x58, 0x20];
        cbor.extend_from_slice(&sec1[1..33]);
        cbor.extend_from_slice(&[0x22, 0x58, 0x20]);
        cbor.extend_from_slice(&sec1[33..65]);
        hex::encode(Sha256::digest(&cbor))
    }

    #[test]
    fn the_thumbprint_is_rfc_9679s_not_a_hash_of_the_raw_point() {
        let parsed = parse_public_key(&public_key_text(&signing_key(2))).unwrap();
        assert_eq!(parsed.thumbprint, thumbprint_by_hand(&parsed.bytes));
        assert_ne!(parsed.thumbprint, hex::encode(Sha256::digest(parsed.bytes)));
    }

    #[test]
    fn a_malformed_key_is_a_validation_error() {
        let good = signing_key(3).verifying_key().to_sec1_point(false);
        let good = good.as_bytes();
        let compressed = signing_key(3).verifying_key().to_sec1_point(true);
        let mut off_curve = good.to_vec();
        off_curve[64] ^= 0x01;
        let mut wrong_tag = good.to_vec();
        wrong_tag[0] = 0x02;
        for (name, bad) in [
            ("empty", String::new()),
            ("not base64", "not base64 !".to_owned()),
            (
                "32 bytes (an Ed25519 key)",
                URL_SAFE_NO_PAD.encode([9_u8; 32]),
            ),
            ("one byte short", URL_SAFE_NO_PAD.encode(&good[..64])),
            (
                "one byte long",
                URL_SAFE_NO_PAD.encode([good, &[0][..]].concat()),
            ),
            (
                "compressed (33 bytes)",
                URL_SAFE_NO_PAD.encode(compressed.as_bytes()),
            ),
            ("tag 0x02 at 65 bytes", URL_SAFE_NO_PAD.encode(&wrong_tag)),
            ("a point off the curve", URL_SAFE_NO_PAD.encode(&off_curve)),
            (
                "standard alphabet",
                base64::engine::general_purpose::STANDARD.encode(good),
            ),
            ("padded", format!("{}=", URL_SAFE_NO_PAD.encode(good))),
        ] {
            assert!(
                matches!(parse_public_key(&bad), Err(CratestackError::Validation(_))),
                "{name}: {bad:?} must be refused"
            );
        }
        assert!(parse_public_key(&URL_SAFE_NO_PAD.encode(good)).is_ok());
    }

    #[test]
    fn a_fresh_proof_by_the_key_is_accepted() {
        let now = Utc::now();
        let key = signing_key(4);
        let parsed = check_proof(&request(&key, now), now).expect("an honest pairing");
        assert_eq!(
            parsed.bytes.as_slice(),
            key.verifying_key().to_sec1_point(false).as_bytes()
        );
    }

    fn refusal(args: &schema::RegisterConsoleKeyInput, now: DateTime<Utc>) -> String {
        match check_proof(args, now) {
            Err(CratestackError::Validation(message)) => message,
            other => panic!("expected a validation refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_proof_by_another_key_is_refused() {
        let now = Utc::now();
        let mut args = request(&signing_key(5), now);
        // The right message, signed by someone who does not hold the key.
        let impostor = signing_key(6);
        let signature: Signature =
            impostor.sign(&possession_message(&args.publicKey, now.timestamp()));
        args.possessionProof = URL_SAFE_NO_PAD.encode(signature.to_bytes());
        assert!(refusal(&args, now).contains("does not verify"));
    }

    #[test]
    fn a_proof_for_a_different_issued_at_or_key_text_is_refused() {
        let now = Utc::now();
        let key = signing_key(7);
        let mut args = request(&key, now);
        args.issuedAt += Duration::seconds(1);
        assert!(refusal(&args, now).contains("does not verify"));

        let mut args = request(&key, now);
        // Same point, other spelling of the text (the standard alphabet's
        // padding is not base64url-no-pad, so use an equivalent: trailing
        // whitespace is trimmed by the parser but signed over as sent).
        args.publicKey.push(' ');
        assert!(refusal(&args, now).contains("does not verify"));
    }

    #[test]
    fn a_signature_over_another_domains_message_is_refused() {
        let now = Utc::now();
        let key = signing_key(8);
        let mut args = request(&key, now);
        // vaam's own pairing message, signed by the same key: valid ECDSA,
        // wrong domain.
        let other = format!(
            "vaam.admin-pair.v1
{}
{}
label",
            args.publicKey,
            now.timestamp()
        );
        let signature: Signature = key.sign(other.as_bytes());
        args.possessionProof = URL_SAFE_NO_PAD.encode(signature.to_bytes());
        assert!(refusal(&args, now).contains("does not verify"));
    }

    #[test]
    fn a_stale_or_future_request_is_refused_before_its_signature_is_looked_at() {
        // Whole seconds, so the boundary cases below are exact.
        let now = DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap();
        let key = signing_key(9);
        for (name, issued_at) in [
            ("301 s old", now - Duration::seconds(301)),
            ("an hour old", now - Duration::hours(1)),
            ("301 s in the future", now + Duration::seconds(301)),
        ] {
            let message = refusal(&request(&key, issued_at), now);
            assert!(message.contains("expired"), "{name}: {message}");
        }
        for (name, issued_at) in [
            ("exactly 300 s old", now - Duration::seconds(300)),
            ("299 s old", now - Duration::seconds(299)),
            ("now", now),
            ("299 s ahead", now + Duration::seconds(299)),
        ] {
            // issuedAt reaches the message in whole seconds, as a console's QR does.
            let issued_at = DateTime::from_timestamp(issued_at.timestamp(), 0).unwrap();
            check_proof(&request(&key, issued_at), now).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }

    #[test]
    fn a_malformed_proof_is_refused_not_a_panic() {
        let now = Utc::now();
        let mut args = request(&signing_key(10), now);
        for bad in [
            "",
            "%%%",
            "AAAA",
            &"A".repeat(86),
            &URL_SAFE_NO_PAD.encode([0_u8; 64]),
            &URL_SAFE_NO_PAD.encode([1_u8; 65]),
        ] {
            args.possessionProof = bad.to_owned();
            assert!(
                matches!(check_proof(&args, now), Err(CratestackError::Validation(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_high_s_signature_is_as_good_as_a_low_s_one() {
        // WebCrypto does not normalise `s`, so an honest console signs high
        // about half the time.
        let now = Utc::now();
        let key = signing_key(11);
        let mut args = request(&key, now);
        let signature: Signature = key.sign(&possession_message(&args.publicKey, now.timestamp()));
        let (r, s) = signature.split_scalars();
        let flipped = Signature::from_scalars(r, -s).expect("a valid scalar pair");
        assert_ne!(flipped, signature);
        for each in [signature, flipped] {
            args.possessionProof = URL_SAFE_NO_PAD.encode(each.to_bytes());
            check_proof(&args, now).expect("either half of the (r, s) / (r, n-s) pair verifies");
        }
    }

    fn context_with_scope(scope: Option<&str>) -> CratestackContext {
        let mut ctx = Principal {
            sub: "appc_vaam".to_owned(),
            kind: PrincipalKind::App,
            role: "app".to_owned(),
            app_id: "app".to_owned(),
        }
        .into_context();
        if let Some(scope) = scope {
            ctx.extensions
                .insert("scope".to_owned(), Value::String(scope.to_owned()));
        }
        ctx
    }

    #[test]
    fn only_the_exact_pair_scope_opens_registration() {
        for granted in [" console:pair ", " sms:send console:pair ", "console:pair"] {
            require_pair_scope(&context_with_scope(Some(granted)))
                .unwrap_or_else(|e| panic!("{granted:?}: {e:?}"));
        }
        for denied in [
            None,
            Some(""),
            Some(" sms:send "),
            Some(" console:pair2 "),
            Some(" console:pai "),
            Some(" xconsole:pair "),
        ] {
            assert!(
                matches!(
                    require_pair_scope(&context_with_scope(denied)),
                    Err(CratestackError::Forbidden(_))
                ),
                "{denied:?}"
            );
        }
        // `perms` (a human role's vocabulary) never stands in for the scope.
        let mut ctx = context_with_scope(None);
        ctx.extensions.insert(
            "perms".to_owned(),
            Value::List(vec![Value::String(PAIR_SCOPE.to_owned())]),
        );
        assert!(require_pair_scope(&ctx).is_err());
    }

    fn row(
        user_id: &str,
        expires_in: Duration,
        revoked: bool,
        now: DateTime<Utc>,
    ) -> schema::ConsoleKey {
        schema::ConsoleKey {
            id: "key".to_owned(),
            userId: user_id.to_owned(),
            alg: schema::ConsoleKeyAlg::esp256,
            publicKey: vec![4; 65],
            thumbprint: "0".repeat(64),
            label: None,
            pairedAt: now - Duration::days(1),
            expiresAt: now + expires_in,
            revokedAt: revoked.then_some(now),
            createdAt: now,
            updatedAt: now,
        }
    }

    #[test]
    fn an_existing_key_is_returned_only_to_its_own_user_while_live() {
        let now = Utc::now();
        assert!(same_registration(row("u1", Duration::days(5), false, now), "u1", now).is_ok());
        for (name, existing, who) in [
            (
                "another user's",
                row("u1", Duration::days(5), false, now),
                "u2",
            ),
            ("revoked", row("u1", Duration::days(5), true, now), "u1"),
            (
                "expired",
                row("u1", Duration::seconds(-1), false, now),
                "u1",
            ),
            (
                "expiring this instant",
                row("u1", Duration::zero(), false, now),
                "u1",
            ),
        ] {
            assert!(
                matches!(
                    same_registration(existing, who, now),
                    Err(CratestackError::Conflict(_))
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn ids_names_and_labels_are_bounded() {
        assert!(clean_vaam_user_id("usr_42-A").is_ok());
        for bad in [
            "",
            "a b",
            "a@b",
            "a
b",
            "é",
            &"x".repeat(65),
        ] {
            assert!(clean_vaam_user_id(bad).is_err(), "{bad:?}");
        }
        assert_eq!(display_name_for(None, "usr_42").unwrap(), "vaam usr_42");
        assert_eq!(
            display_name_for(Some("  Ada ".to_owned()), "x").unwrap(),
            "Ada"
        );
        assert!(display_name_for(Some("  ".to_owned()), "x").is_err());
        assert!(display_name_for(Some("x".repeat(129)), "x").is_err());
        assert_eq!(
            clean_label(Some(" Chrome ".to_owned())).unwrap().as_deref(),
            Some("Chrome")
        );
        assert!(clean_label(Some("x".repeat(65)).clone()).is_err());
        assert!(clean_label(None).unwrap().is_none());
    }
}
