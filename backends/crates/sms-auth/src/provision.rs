#![doc = include_str!("provision.md")]

use std::fmt;

use authkestra_engine::TokenManager;
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use rand::rngs::OsRng;
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use sms_api::schema::{self, Cratestack, app};
use sms_core::{EMPTY, pack};

/// RSA modulus size for a client keypair: the same 2048 bits the OP's own
/// signing keys use, the smallest size still acceptable for RS256.
const CLIENT_RSA_KEY_BITS: usize = 2048;

/// What provisioning a machine client returns.
///
/// The private key is **returned exactly once and stored nowhere**: the caller
/// writes it to a file and drops this. `Debug` is written by hand and shows the
/// client id only, so a stray `{:?}` cannot put a key in a log.
pub struct ProvisionedClient {
    /// `appc_<uuid>`: the `client_id` of `/token` and the assertion's `iss` and
    /// `sub`. Not secret.
    pub client_id: String,
    /// The client's PKCS#8 PEM private key.
    pub private_key_pem: String,
}

impl fmt::Debug for ProvisionedClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProvisionedClient")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

/// Why a machine client was not provisioned.
#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    /// The scopes do not pack (an empty one, or one containing the delimiter).
    #[error("scopes: {0}")]
    Scopes(String),
    /// `--scope` named the console pairing scope, which only `register-client`
    /// grants.
    #[error("{0}")]
    PairScope(String),
    /// No `App` has this id.
    #[error("no App with id {0}")]
    AppNotFound(String),
    /// The `App` exists and is switched off.
    #[error("App {0} is not active")]
    AppInactive(String),
    /// Generating or encoding the keypair failed.
    #[error("generating the client keypair: {0}")]
    Key(String),
    /// A delegate call failed.
    #[error("database: {0}")]
    Database(#[from] CratestackError),
}

/// Provisions a machine client of `app_id`: a fresh RSA keypair, an `AppClient`
/// (the principal a verified token will resolve to) and an `OauthClient` holding
/// only the **public** JWK, in one transaction.
///
/// `sys` is the internal `system` context. The caller has already established
/// that the deployment serves `machine_tokens` ([`crate::gate`]) and, for the
/// CLI, that the operator acts as `owner` or `admin`.
///
/// # Errors
///
/// [`ProvisionError`].
pub async fn provision_token_client(
    db: &Cratestack,
    sys: &CratestackContext,
    app_id: &str,
    label: &str,
    scopes: &[String],
) -> Result<ProvisionedClient, ProvisionError> {
    let scopes_packed = pack(scopes).map_err(|error| ProvisionError::Scopes(error.to_string()))?;
    // `console:pair` lets a client attach a console key to any linked person, so
    // handing it out here is a way to become someone else.
    sms_api::pair_scope::refuse_pair_scope(&scopes_packed)
        .map_err(|error| ProvisionError::PairScope(error.to_string()))?;

    // The app must exist and be active before a client may speak for it.
    let found = db
        .app()
        .find_many()
        .where_expr(FilterExpr::from(app::id().eq(app_id.to_owned())))
        .limit(1)
        .run(sys)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ProvisionError::AppNotFound(app_id.to_owned()))?;
    if !found.active {
        return Err(ProvisionError::AppInactive(found.id));
    }

    // Caller-visible immediately (it is the token endpoint's `client_id` and the
    // assertion's `iss`/`sub`), so generated here rather than left to a default.
    let client_id = format!("appc_{}", cratestack::uuid::Uuid::new_v4().simple());

    let key = RsaPrivateKey::new(&mut OsRng, CLIENT_RSA_KEY_BITS)
        .map_err(|error| ProvisionError::Key(error.to_string()))?;
    let pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|error| ProvisionError::Key(error.to_string()))?;

    // `authkestra_op::client_assertion::verify_client_assertion` reads a JWK Set
    // (`{"keys": [...]}`); `TokenManager` derives n/e without hand-rolled base64.
    let manager = TokenManager::new_asymmetric(pem.as_bytes(), None, Some(client_id.clone()))
        .map_err(|error| ProvisionError::Key(error.to_string()))?;
    let public_jwk = manager.public_jwk().ok_or_else(|| {
        ProvisionError::Key("no public JWK for a freshly generated asymmetric key".to_owned())
    })?;
    let jwks = serde_json::json!({ "keys": [public_jwk] }).to_string();

    // `client_credentials` only: the one grant a machine caller uses.
    let grant_types =
        pack(["client_credentials"]).map_err(|error| ProvisionError::Scopes(error.to_string()))?;

    db.transaction(async |tx| {
        let app_client = db
            .app_client()
            .create(schema::CreateAppClientInput {
                appId: found.id.clone(),
                clientId: client_id.clone(),
                label: label.to_owned(),
                scopes: scopes_packed.clone(),
                lastUsedAt: None,
                retiredAt: None,
            })
            .run_in_tx(tx, sys)
            .await?
            .value;

        db.oauth_client()
            .create(schema::CreateOauthClientInput {
                clientId: client_id.clone(),
                appClientId: Some(app_client.id),
                tokenEndpointAuthMethod: schema::ClientAuthMethod::private_key_jwt,
                jwks: Some(jwks.clone()),
                grantTypes: grant_types.clone(),
                scopes: scopes_packed.clone(),
                redirectUris: EMPTY.to_owned(),
                requirePkce: false,
            })
            .run_in_tx(tx, sys)
            .await?;
        Ok(())
    })
    .await?;

    Ok(ProvisionedClient {
        client_id,
        private_key_pem: pem.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::ProvisionedClient;

    #[test]
    fn debug_shows_the_client_id_and_never_the_key() {
        let shown = format!(
            "{:?}",
            ProvisionedClient {
                client_id: "appc_abc".to_owned(),
                private_key_pem: "-----BEGIN PRIVATE KEY-----".to_owned(),
            }
        );
        assert!(shown.contains("appc_abc"), "{shown}");
        assert!(!shown.contains("PRIVATE"), "{shown}");
    }
}
