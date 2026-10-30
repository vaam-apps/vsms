#![doc = include_str!("op.md")]

use std::sync::Arc;

use async_trait::async_trait;
use authkestra_engine::TokenManager;
use authkestra_engine::store::StoreError;
use authkestra_engine::store::memory::MemoryStore;
use authkestra_engine::token::jwk::Jwk;
use authkestra_op::config::OpConfig;
use authkestra_op::refresh::{RefreshToken, RefreshTokenStore};
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

/// The refresh-token slot of [`OpStore`]: **a store that refuses to hold one**.
///
/// This deployment issues no refresh token (VSA-4 plan, decision 4). The console
/// client is registered for `authorization_code` alone and never for
/// `refresh_token`, `client_credentials` never issues one, and the grant gate in
/// front of `/token` (`sms-gateway`'s `op.rs`) answers a `refresh_token` request
/// `unsupported_grant_type` before this store is asked anything. This is the
/// third, independent refusal, so that one forgotten condition cannot start
/// handing out a 30-day credential: [`store_token`](RefreshTokenStore::store_token)
/// fails (`authkestra`'s authorization-code handler then omits the token from the
/// response, because storing it failed), and every lookup finds nothing.
///
/// `authkestra`'s handler only issues a refresh token when the request's scope
/// carries `offline_access`, which `handle_authorize` refuses for a client not
/// registered with it, so on the console's registration this store is not even
/// reached. It is here for the case where it is.
#[derive(Debug, Clone, Copy, Default)]
pub struct RefusingRefreshStore;

#[async_trait]
impl RefreshTokenStore for RefusingRefreshStore {
    async fn store_token(&mut self, _token: RefreshToken) -> Result<(), StoreError> {
        Err(StoreError::Internal(
            "this deployment issues no refresh tokens".to_owned(),
        ))
    }

    async fn get_token(&mut self, _token: &str) -> Result<Option<RefreshToken>, StoreError> {
        Ok(None)
    }

    async fn revoke_token(&mut self, _token: &str) -> Result<(), StoreError> {
        Ok(())
    }

    async fn consume_token(&mut self, _token: &str) -> Result<Option<RefreshToken>, StoreError> {
        Ok(None)
    }
}

/// `OpStore` for this OP: real client and assertion tracking, an authorization-
/// code slot for the console's sign-in, a refresh slot that refuses
/// ([`RefusingRefreshStore`]), and an inert device-code slot.
///
/// **The authorization-code slot is in memory, by decision** (VSA-4 plan,
/// decision 8). A code lives at most [`AUTHORIZATION_CODE_TTL_SECS`] seconds
/// between `POST /login` and `/token` in the same process, and a code lost to a
/// restart costs the person one more sign-in. It is the same shape the pre-#446
/// OP had, and it carries the same limit: **one `serve` replica** (the Helm chart
/// pins `replicas: 1`), because a code issued by one replica is unknown to the
/// other. Moving it to Postgres is a follow-up, not a precondition. Unlike a spent
/// `jti` (`ClientAssertion`, a real table) a lost code is not a security loss: the
/// code is single-use and short-lived either way.
///
/// The device-code slot exists because `CompositeOpStore` has no constructor
/// without one; nothing here mounts a handler that writes to it.
///
/// Five explicit type parameters, not six: `CompositeOpStore` has a sixth, `P`,
/// the `DPoP`-replay-store slot, left at its default `NoDpopReplayStore`, which
/// refuses every `DPoP` proof outright. Correct here: no `DPoP` client exists in
/// this deployment, so refusing proofs this OP is never sent is a no-op.
pub type OpStore = CompositeOpStore<
    SmsClientStore,
    MemoryStore<authkestra_op::code::AuthorizationCode>,
    RefusingRefreshStore,
    MemoryStore<authkestra_op::device::DeviceCodeSession>,
    SmsClientAssertionStore,
>;

/// Assemble the OP's store: see [`OpStore`] for what each slot is.
///
/// `sys` is the internal `system` context, the only one `OauthClient` and
/// `ClientAssertion` admit (`system_context("sms-gateway:op")`).
#[must_use]
pub fn op_store(db: Arc<Cratestack>, sys: CratestackContext) -> OpStore {
    CompositeOpStore::new(
        SmsClientStore::new(db.clone(), sys.clone()),
        MemoryStore::new(),
        RefusingRefreshStore,
        MemoryStore::new(),
    )
    .with_client_assertion_store(SmsClientAssertionStore::new(db, sys))
}

/// How long an authorization code lives between `POST /login` and `/token`.
/// A same-process, seconds-long handoff: sixty seconds is generous, not tight.
pub const AUTHORIZATION_CODE_TTL_SECS: i64 = 60;

/// The grant a machine client uses: `private_key_jwt` assertion for an access
/// token, no refresh token.
pub const CLIENT_CREDENTIALS: &str = "client_credentials";

/// The grant the console's sign-in uses: a code from `POST /login`, redeemed with
/// its PKCE verifier. Mandatory PKCE, S256 only.
pub const AUTHORIZATION_CODE: &str = "authorization_code";

/// The only PKCE method the provider accepts (`handle_authorize` refuses any
/// other, and `plain` in particular). Advertised in discovery as
/// `code_challenge_methods_supported`.
pub const CODE_CHALLENGE_METHOD: &str = "S256";

/// The scopes a machine client may hold, advertised. Informational: a token
/// carries the scopes the client was provisioned with (`OauthClient.scopes`),
/// whatever this lists.
pub const MACHINE_SCOPES_SUPPORTED: [&str; 2] = ["sms:send", "sms:read"];

/// The scopes the console's sign-in asks for: `openid` makes the provider issue
/// an `id_token`, `profile` its name. `offline_access` is **not** among them, so
/// no refresh token is ever asked for.
pub const CONSOLE_SCOPES: [&str; 2] = ["openid", "profile"];

/// Which grants a deployment serves, from `auth.methods` and the blocks
/// (`AuthMethods::machine_tokens_served`, `AuthMethods::human_login_served`).
/// Two named fields and not a pair of `bool`s, for the reason every gate in this
/// workspace is: `ServedGrants::new(true, false)` does not say which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServedGrants {
    /// `client_credentials`: `machine_tokens` is served.
    pub machine_tokens: bool,
    /// `authorization_code`: `human_login` is served.
    pub human_login: bool,
}

impl ServedGrants {
    /// What a resolved `auth.methods` and its blocks serve.
    #[must_use]
    pub fn from_methods(methods: &sms_config::AuthMethods) -> Self {
        Self {
            machine_tokens: methods.machine_tokens_served(),
            human_login: methods.human_login_served(),
        }
    }

    /// The `grant_types_supported` of the discovery document: **exactly the
    /// grants served**, in a fixed order. Never `refresh_token`.
    #[must_use]
    pub fn grant_types(self) -> Vec<&'static str> {
        let mut grants = Vec::new();
        if self.machine_tokens {
            grants.push(CLIENT_CREDENTIALS);
        }
        if self.human_login {
            grants.push(AUTHORIZATION_CODE);
        }
        grants
    }

    /// Whether `/token` may run `grant_type` at all. Anything not listed by
    /// [`Self::grant_types`] is not served: not the other method's grant, not
    /// `refresh_token`, not `device_code`, not `token-exchange`, not a custom
    /// one.
    #[must_use]
    pub fn serves(self, grant_type: &str) -> bool {
        self.grant_types().contains(&grant_type)
    }
}

/// `OpConfig` for this OP, advertising exactly the grants in `served`.
///
/// `device_code` and `token_exchange` stay unadvertised: nothing here mounts
/// either handler. `response_types_supported` is `["code"]` only when the
/// sign-in is served, and no `authorization_endpoint` is advertised in any case
/// (the discovery handler drops it): `GET /authorize` is never mounted, because
/// the console is the one client this deployment has and it reaches
/// `handle_authorize` through `POST /login`.
#[must_use]
pub fn op_config(issuer: String, served: ServedGrants) -> OpConfig {
    let mut scopes: Vec<String> = Vec::new();
    if served.machine_tokens {
        scopes.extend(MACHINE_SCOPES_SUPPORTED.map(str::to_owned));
    }
    if served.human_login {
        scopes.extend(CONSOLE_SCOPES.map(str::to_owned));
    }
    OpConfig {
        issuer,
        scopes_supported: scopes,
        response_types_supported: if served.human_login {
            vec!["code".to_owned()]
        } else {
            vec![]
        },
        grant_types_supported: served
            .grant_types()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        id_token_signing_alg: "RS256".to_owned(),
        authorization_code_ttl_secs: AUTHORIZATION_CODE_TTL_SECS,
        // Fifteen minutes (docs/architecture.md, section 4.2). The same constant
        // the configuration holds `op.signing_key.rotation_overlap_secs` above.
        access_token_ttl_secs: sms_config::ACCESS_TOKEN_TTL_SECS,
        device_code_ttl_secs: 60,
        token_exchange_enabled: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants(machine_tokens: bool, human_login: bool) -> ServedGrants {
        ServedGrants {
            machine_tokens,
            human_login,
        }
    }

    #[test]
    fn a_grant_is_served_only_by_the_method_that_owns_it() {
        assert!(grants(true, false).serves(CLIENT_CREDENTIALS));
        assert!(!grants(true, false).serves(AUTHORIZATION_CODE));
        assert!(grants(false, true).serves(AUTHORIZATION_CODE));
        assert!(!grants(false, true).serves(CLIENT_CREDENTIALS));
        assert!(!grants(false, false).serves(CLIENT_CREDENTIALS));
        assert!(!grants(false, false).serves(AUTHORIZATION_CODE));
    }

    #[test]
    fn no_combination_serves_the_refresh_grant_or_anything_else() {
        for machine_tokens in [false, true] {
            for human_login in [false, true] {
                let served = grants(machine_tokens, human_login);
                for other in [
                    "refresh_token",
                    "urn:ietf:params:oauth:grant-type:device_code",
                    "urn:ietf:params:oauth:grant-type:token-exchange",
                    "password",
                ] {
                    assert!(!served.serves(other), "{served:?} served {other}");
                }
                assert_eq!(
                    served.grant_types().len(),
                    usize::from(machine_tokens) + usize::from(human_login)
                );
            }
        }
    }

    #[test]
    fn the_config_advertises_what_is_served_and_the_codes_short_life() {
        let config = op_config("https://sms.example.org".to_owned(), grants(false, true));
        assert_eq!(config.grant_types_supported, ["authorization_code"]);
        assert_eq!(config.response_types_supported, ["code"]);
        assert_eq!(
            config.authorization_code_ttl_secs,
            AUTHORIZATION_CODE_TTL_SECS
        );
        assert_eq!(
            config.access_token_ttl_secs,
            sms_config::ACCESS_TOKEN_TTL_SECS
        );
        assert!(!config.token_exchange_enabled);
        assert!(
            !config
                .scopes_supported
                .iter()
                .any(|s| s == "offline_access")
        );
        let machines = op_config("https://sms.example.org".to_owned(), grants(true, false));
        assert_eq!(machines.scopes_supported, ["sms:send", "sms:read"]);
        assert_eq!(machines.response_types_supported, Vec::<String>::new());
    }

    /// The refresh slot cannot hold a token, whatever asks it to.
    #[tokio::test]
    async fn the_refresh_store_refuses_to_hold_a_token_and_finds_none() {
        let mut store = RefusingRefreshStore;
        let token = RefreshToken::new(
            "t".to_owned(),
            "client".to_owned(),
            authkestra_engine::auth::state::Identity {
                provider_id: "p".to_owned(),
                external_id: "u".to_owned(),
                email: None,
                username: None,
                attributes: std::collections::HashMap::new(),
            },
            "openid offline_access".to_owned(),
            Utc::now() + Duration::days(30),
            None,
        );
        assert!(store.store_token(token).await.is_err());
        assert!(store.get_token("t").await.unwrap().is_none());
        assert!(store.consume_token("t").await.unwrap().is_none());
        assert!(store.revoke_token("t").await.is_ok());
    }
}
