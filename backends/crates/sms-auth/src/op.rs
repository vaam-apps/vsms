#![doc = include_str!("op.md")]

use std::sync::Arc;

use authkestra_engine::TokenManager;
use authkestra_engine::store::memory::MemoryStore;
use authkestra_engine::token::jwk::Jwk;
use authkestra_op::config::OpConfig;
use authkestra_op::store::CompositeOpStore;
use chrono::{Duration, Utc};
use cratestack::{CratestackContext, FilterExpr};
use rand::rngs::OsRng;
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use sms_api::schema::{self, Cratestack, oauth_signing_key};

use crate::{SmsClientAssertionStore, SmsClientStore};

/// RSA modulus size for a freshly generated signing key. 2048 is the
/// smallest size still considered acceptable for RS256 in 2026 and what
/// `jsonwebtoken`'s own RS256 examples use; nothing in this deployment
/// needs the extra headroom of 3072/4096 at the cost of larger tokens.
const RSA_KEY_BITS: usize = 2048;

/// Generate a new RSA keypair, insert it as the new signing key, and
/// deactivate whatever was previously active — its row keeps publishing in
/// JWKS (`expiresAt` set to `now + overlap`) without signing anything new.
///
/// Not an HTTP route: rotation is an operator action
/// (`sms-gateway rotate-signing-key`), not a generated-CRUD write, matching
/// this codebase's existing convention for ops actions versus API surface.
///
/// # Errors
///
/// RSA key generation failure (extremely unlikely at this bit size) or any
/// `CratestackError` from the two writes.
pub async fn rotate_signing_key(
    db: &Cratestack,
    sys: &CratestackContext,
    overlap: Duration,
) -> anyhow::Result<String> {
    let mut rng = OsRng;
    let key = RsaPrivateKey::new(&mut rng, RSA_KEY_BITS)?;
    let pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|error| anyhow::anyhow!("encoding the generated key to PKCS#8 PEM: {error}"))?;

    // Created before the old key is deactivated, not after: the schema's
    // `active Boolean @default(true)` means this row is `active` from the
    // moment it exists, so there is no window where zero keys are active
    // and a concurrent `load_signing_keys` call finds nothing to sign
    // with. A brief window with *two* active rows is harmless — the tie
    // is broken deterministically (see `load_signing_keys`).
    let created = db
        .oauth_signing_key()
        .create(schema::CreateOauthSigningKeyInput {
            privateKeyPem: pem.to_string(),
            expiresAt: None,
        })
        .run(sys)
        .await?;

    let previously_active = db
        .oauth_signing_key()
        .find_many()
        .where_expr(
            FilterExpr::from(oauth_signing_key::active().is_true())
                .and(oauth_signing_key::id().ne(created.id.clone())),
        )
        .run(sys)
        .await?;

    let expires_at = Utc::now() + overlap;
    for old in previously_active {
        db.oauth_signing_key()
            .update(old.id)
            .set(schema::UpdateOauthSigningKeyInput {
                active: Some(false),
                expiresAt: Some(Some(expires_at)),
                ..Default::default()
            })
            .run(sys)
            .await?;
    }

    Ok(created.id)
}

/// Every still-valid signing key — `active`, or not active but not yet
/// past its rotation-overlap `expiresAt` — as a `TokenManager` (`kid` is
/// the row's own `id`, per its schema comment). The single `active` one is
/// what `Engine`/`axum_token_handler` sign with; the full list is what
/// `/jwks.json` publishes.
///
/// Ties on `active` (the brief window `rotate_signing_key` can leave) are
/// broken by `createdAt` descending: the most recently created active key
/// wins, matching "rotation just happened, sign with the new one" over
/// "sign with whichever happened to load first."
///
/// # Errors
///
/// Any `CratestackError` from the read, or no row is `active` at all (the
/// deployment has never rotated in a key — an operator action, not a
/// schema default, on purpose: see `rotate_signing_key`'s own doc).
pub async fn load_signing_keys(
    db: &Cratestack,
    sys: &CratestackContext,
    issuer: &str,
) -> anyhow::Result<(Arc<TokenManager>, Vec<Jwk>)> {
    let now = Utc::now();
    let rows = db
        .oauth_signing_key()
        .find_many()
        .where_expr(
            FilterExpr::from(oauth_signing_key::active().is_true())
                .or(FilterExpr::from(oauth_signing_key::expiresAt().gt(now))),
        )
        .order_by(oauth_signing_key::createdAt().desc())
        .run(sys)
        .await?;

    let mut signing: Option<Arc<TokenManager>> = None;
    let mut jwks = Vec::new();
    for row in rows {
        let manager = TokenManager::new_asymmetric(
            row.privateKeyPem.as_bytes(),
            Some(issuer.to_owned()),
            Some(row.id.clone()),
        )
        .map_err(|error| anyhow::anyhow!("building a TokenManager for key {}: {error}", row.id))?;
        if let Some(jwk) = manager.public_jwk() {
            jwks.push(jwk);
        }
        if row.active && signing.is_none() {
            signing = Some(Arc::new(manager));
        }
    }

    let signing = signing.ok_or_else(|| {
        anyhow::anyhow!(
            "no active OauthSigningKey — run `sms-gateway rotate-signing-key` before serving"
        )
    })?;
    Ok((signing, jwks))
}

/// `OpStore` for this OP: real client and assertion tracking, and inert
/// in-memory slots for the three grants nothing here mounts a handler for.
///
/// **The authorization-code, refresh-token and device-code slots are
/// placeholders.** `authkestra_engine::store::memory::MemoryStore<T>` is a real,
/// correct `KvStore<T>`, but `/authorize`, `/device_authorization` and the human
/// login are not mounted, so nothing writes to these three. They exist because
/// `CompositeOpStore` has no constructor without them. A client registered with
/// `client_credentials` only (what [`crate::provision`] writes) is refused the
/// other grants by the token handler itself, before any of these is read. The
/// human console's use of the authorization-code and refresh slots was deleted
/// with the login (#446); VSA-4 brings it back and decides there whether they
/// stay in memory.
///
/// Five explicit type parameters, not six: `CompositeOpStore` has a sixth, `P`,
/// the `DPoP`-replay-store slot, left at its default `NoDpopReplayStore`, which
/// refuses every `DPoP` proof outright. Correct here: no `DPoP` client exists in
/// this deployment (machine callers use `private_key_jwt`, not `DPoP`-bound
/// tokens), so refusing proofs this OP is never sent is a no-op.
pub type MachineOnlyOpStore = CompositeOpStore<
    SmsClientStore,
    MemoryStore<authkestra_op::code::AuthorizationCode>,
    MemoryStore<authkestra_op::refresh::RefreshToken>,
    MemoryStore<authkestra_op::device::DeviceCodeSession>,
    SmsClientAssertionStore,
>;

/// Assemble the OP's store: see [`MachineOnlyOpStore`] for what its inert slots
/// are.
///
/// `sys` is the internal `system` context, the only one `OauthClient` and
/// `ClientAssertion` admit (`system_context("sms-gateway:op")`).
#[must_use]
pub fn machine_only_store(db: Arc<Cratestack>, sys: CratestackContext) -> MachineOnlyOpStore {
    CompositeOpStore::new(
        SmsClientStore::new(db.clone(), sys.clone()),
        MemoryStore::new(),
        MemoryStore::new(),
        MemoryStore::new(),
    )
    .with_client_assertion_store(SmsClientAssertionStore::new(db, sys))
}

/// The grant types the OP advertises and its `/token` handler is configured for.
///
/// **`client_credentials` alone.** `29f93c4^` also listed `authorization_code`
/// and `refresh_token`, for the console's password login (#194). Neither is
/// served by this release (no `/authorize`, no login: VSA-4), `authkestra`'s
/// `client_credentials` handler never issues a refresh token (it returns
/// `refresh_token: None` unconditionally), and a client registered with
/// `client_credentials` only is refused any other grant by the handler. So the
/// refresh grant is not offered at all (owner ruling R5, question 8: it is
/// offered only when a legacy method that needs it is on, and machine tokens do
/// not).
pub const GRANT_TYPES_SUPPORTED: [&str; 1] = ["client_credentials"];

/// The scopes the OP advertises. Informational: a token carries the scopes the
/// client was provisioned with (`OauthClient.scopes`), whatever this lists.
pub const SCOPES_SUPPORTED: [&str; 2] = ["sms:send", "sms:read"];

/// `OpConfig` for this OP: `client_credentials` + `private_key_jwt` (machine
/// callers), nothing else. `device_code` and `token_exchange` stay
/// unadvertised: nothing here mounts either handler.
#[must_use]
pub fn machine_only_config(issuer: String) -> OpConfig {
    OpConfig {
        issuer,
        scopes_supported: SCOPES_SUPPORTED.map(str::to_owned).to_vec(),
        // The type requires them; no `/authorize` is mounted, so no
        // `response_type` is ever served.
        response_types_supported: vec![],
        grant_types_supported: GRANT_TYPES_SUPPORTED.map(str::to_owned).to_vec(),
        id_token_signing_alg: "RS256".to_owned(),
        // An authorization code is never issued; the value is not read.
        authorization_code_ttl_secs: 60,
        // Fifteen minutes (docs/architecture.md, section 4.2). The same constant
        // the configuration holds `op.signing_key.rotation_overlap_secs` above.
        access_token_ttl_secs: sms_config::ACCESS_TOKEN_TTL_SECS,
        device_code_ttl_secs: 60,
        token_exchange_enabled: false,
    }
}
