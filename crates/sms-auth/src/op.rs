//! Standing up the OP itself: RS256 key management with an overlap-window
//! rotation, [`CompositeOpStore`] assembly, and [`OpConfig`]. #20.
//!
//! Scoped to `client_credentials` + `private_key_jwt` only — the only
//! caller type that exists in this system (no admin console yet, #49-59,
//! so no human ever authenticates via the authorization-code flow). See
//! this crate's own module doc and the design doc §4.2/§4.3 for why that's
//! not a corner cut on what #20 actually asks for.
//!
//! # API reality check (verified against vendored `authkestra-op`/
//! `authkestra-engine`/`authkestra-axum` 0.3.2 source, not the design doc,
//! which was written for 0.2.3)
//!
//! - **No `SigningKey`/rotation type exists.** [`TokenManager::new_asymmetric`]
//!   holds exactly one RS256 key. An overlap window means running one
//!   [`TokenManager`] per still-valid [`schema::OauthSigningKey`] row and
//!   merging their [`TokenManager::public_jwk`]s ourselves — see
//!   [`load_signing_keys`].
//! - **`authkestra-axum`'s own `axum_jwks_handler` publishes exactly one
//!   key** — whichever single `Arc<TokenManager>` the axum state carries —
//!   so it cannot serve an overlap-window JWKS on its own. `sms-gateway`
//!   builds its `/jwks.json` response directly from
//!   [`load_signing_keys`]'s full key list instead of using that handler.
//! - **`CompositeOpStore::new` takes 4 positional stores** (client,
//!   authorization-code, refresh-token, device-code), not `with_*`
//!   builders — `with_client_assertion_store` is the one real builder
//!   method. The authorization-code/refresh-token/device-code slots are
//!   wired to [`authkestra_engine::store::memory::MemoryStore`] — genuinely
//!   inert placeholders, since nothing calls `/authorize` on this
//!   client-credentials-only deployment, not a corner cut with anything
//!   real behind it to lose.
//! - **`state_encryption_key`/`SessionConfig` is not part of the OP at
//!   all** — it belongs to `authkestra_engine`'s *relying-party* flow
//!   (this system consuming an external `IdP`), which nothing here does.
//! - **No `Engine` is ever constructed.** The obvious-looking path —
//!   `Engine::builder().session_store(...).token_manager(...).build()`,
//!   then handing that to `Op::builder()` — turned out to be unnecessary:
//!   `app/sms-gateway`'s own `op.rs` hand-wires `axum_token_handler`/
//!   `axum_discovery_handler` directly, and those only need `Arc<dyn
//!   OpStore>` + `Arc<TokenManager>` + `OpConfig` via `FromRef`, never a
//!   full `Op`/`Engine`. An earlier version of this module built the
//!   `Engine` anyway, unused — removed in review (#97) rather than kept
//!   "for a future caller."

use std::sync::Arc;

use authkestra_engine::store::memory::MemoryStore;
use authkestra_engine::token::jwk::Jwk;
use authkestra_engine::TokenManager;
use authkestra_op::config::OpConfig;
use authkestra_op::store::CompositeOpStore;
use chrono::{Duration, Utc};
use cratestack::{CoolContext, FilterExpr};
use rand::rngs::OsRng;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::RsaPrivateKey;
use sms_api::schema::{self, oauth_signing_key, Cratestack};

use crate::{SmsClientAssertionStore, SmsClientStore};

/// RSA modulus size for a freshly generated signing key. 2048 is the
/// smallest size still considered acceptable for RS256 in 2026 and what
/// `jsonwebtoken`'s own RS256 examples use; nothing in this deployment
/// needs the extra headroom of 3072/4096 at the cost of larger tokens.
const RSA_KEY_BITS: usize = 2048;

/// How long a rotated-out key keeps publishing in JWKS after it stops
/// signing new tokens. Comfortably more than the 15-minute access-token
/// TTL (§4.2) — every token signed by the old key has expired long before
/// this window closes, so validation never needs the old key past it.
pub const ROTATION_OVERLAP: Duration = Duration::minutes(30);

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
/// `CoolError` from the two writes.
pub async fn rotate_signing_key(
    db: &Cratestack,
    sys: &CoolContext,
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
/// Any `CoolError` from the read, or no row is `active` at all (the
/// deployment has never rotated in a key — an operator action, not a
/// schema default, on purpose: see `rotate_signing_key`'s own doc).
pub async fn load_signing_keys(
    db: &Cratestack,
    sys: &CoolContext,
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

/// `OpStore` for a `client_credentials`-only deployment: the real
/// `SmsClientStore`/`SmsClientAssertionStore` (#19), and inert in-memory
/// placeholders for the three grant types nothing here uses. See this
/// module's own doc for why that's a documented scope cut, not a silent
/// gap.
pub type MachineOnlyOpStore = CompositeOpStore<
    SmsClientStore,
    MemoryStore<authkestra_op::code::AuthorizationCode>,
    MemoryStore<authkestra_op::refresh::RefreshToken>,
    MemoryStore<authkestra_op::device::DeviceCodeSession>,
    SmsClientAssertionStore,
>;

/// Assemble the OP's store: real client + assertion tracking (#19), memory
/// placeholders for the unused grant types (see the module doc).
#[must_use]
pub fn machine_only_store(db: Arc<Cratestack>, sys: CoolContext) -> MachineOnlyOpStore {
    CompositeOpStore::new(
        SmsClientStore::new(db.clone(), sys.clone()),
        MemoryStore::new(),
        MemoryStore::new(),
        MemoryStore::new(),
    )
    .with_client_assertion_store(SmsClientAssertionStore::new(db, sys))
}

/// `OpConfig` for the `client_credentials` + `private_key_jwt` surface —
/// `grant_types_supported` names exactly that grant, not the full set
/// `authkestra-op` can serve, since nothing else is wired.
#[must_use]
pub fn machine_only_config(issuer: String) -> OpConfig {
    OpConfig {
        issuer,
        scopes_supported: vec!["sms:send".to_owned(), "sms:read".to_owned()],
        response_types_supported: vec![],
        grant_types_supported: vec!["client_credentials".to_owned()],
        id_token_signing_alg: "RS256".to_owned(),
        authorization_code_ttl_secs: 60,
        // 15 minutes — §4.2.
        access_token_ttl_secs: 900,
        device_code_ttl_secs: 60,
        token_exchange_enabled: false,
    }
}
