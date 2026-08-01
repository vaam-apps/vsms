use std::sync::Arc;

use authkestra_axum::op::OpExt;
use authkestra_axum::AxumError;
use authkestra_engine::store::memory::MemoryStore;
use authkestra_engine::{SessionConfig, TokenManager};
use authkestra_op::config::OpConfig;
use authkestra_op::store::CompositeOpStore;
use cratestack::axum::extract::FromRef;

use cratestack::{CoolContext, FilterExpr};

use crate::schema::{oauth_signing_key, Cratestack};
use crate::SmsClientStore;

#[derive(Clone)]
pub struct OpState {
    pub db: Arc<Cratestack>,
    pub sys: CoolContext,
    pub token_manager: Arc<TokenManager>,
    pub op_store: Arc<dyn authkestra_op::OpStore>,
    pub op_config: OpConfig,
    pub session_store: Arc<dyn authkestra_engine::SessionStore>,
    pub session_config: SessionConfig,
}

impl FromRef<OpState> for OpConfig {
    fn from_ref(state: &OpState) -> Self {
        state.op_config.clone()
    }
}

impl FromRef<OpState> for Result<Arc<TokenManager>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state.token_manager.clone())
    }
}

impl FromRef<OpState> for Result<Arc<dyn authkestra_op::OpStore>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state.op_store.clone())
    }
}

impl FromRef<OpState> for Result<Arc<dyn authkestra_engine::SessionStore>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state.session_store.clone())
    }
}

impl FromRef<OpState> for SessionConfig {
    fn from_ref(state: &OpState) -> Self {
        state.session_config.clone()
    }
}

pub async fn setup_op_state(
    db: Arc<Cratestack>,
    sys: CoolContext,
    issuer: String,
) -> anyhow::Result<OpState> {
    // 1. Get or create signing key
    let keys = db
        .oauth_signing_key()
        .find_many()
        .where_expr(FilterExpr::from(oauth_signing_key::active().is_true()))
        .order_by(oauth_signing_key::createdAt().desc())
        .run(&sys)
        .await?;

    let key = if let Some(first) = keys.into_iter().next() {
        first
    } else {
        // Generate new key
        let mut rng = rand::thread_rng();
        let rsa_key = rsa::RsaPrivateKey::new(&mut rng, 2048)?;
        use rsa::pkcs8::EncodePrivateKey;
        let pem = rsa_key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)?;
        let pem_str = pem.as_str().to_string();

        db.oauth_signing_key()
            .create(crate::schema::CreateOauthSigningKeyInput {
                privateKeyPem: pem_str,
                expiresAt: None,
            })
            .run(&sys)
            .await?
    };

    // 2. Token Manager
    let token_manager = TokenManager::new_asymmetric(
        key.privateKeyPem.as_bytes(),
        Some(issuer.clone()),
        Some(key.id),
    )
    .map_err(|e| anyhow::anyhow!("TokenManager error: {:?}", e))?;
    let token_manager = Arc::new(token_manager);

    // 3. Op Store
    let client_store = SmsClientStore::new(db.clone(), sys.clone());
    let code_store = MemoryStore::<authkestra_op::code::AuthorizationCode>::new();
    let refresh_store = MemoryStore::<authkestra_op::refresh::RefreshToken>::new();
    let device_store = MemoryStore::<authkestra_op::device::DeviceCodeSession>::new();

    let op_store = CompositeOpStore::new(client_store, code_store, refresh_store, device_store);
    let op_store: Arc<dyn authkestra_op::OpStore> = Arc::new(op_store);

    // 4. Session Store (just memory for now)
    let session_store = MemoryStore::<authkestra_engine::session::Session>::new();
    let session_store: Arc<dyn authkestra_engine::SessionStore> = Arc::new(session_store);

    // 5. Configs
    let op_config = OpConfig {
        issuer,
        scopes_supported: vec![],
        response_types_supported: vec![],
        grant_types_supported: vec![],
        id_token_signing_alg: "RS256".to_string(),
        authorization_code_ttl_secs: 60,
        access_token_ttl_secs: 900, // 15-minute token TTL
        device_code_ttl_secs: 300,
        token_exchange_enabled: false,
    };

    let session_config = SessionConfig {
        cookie_name: "auth_session".to_string(),
        max_age: None,
        secure: true,
        http_only: true,
        same_site: authkestra_engine::auth::SameSite::Lax,
        path: "/".to_string(),
        state_encryption_key: [0u8; 32],
    };

    Ok(OpState {
        db,
        sys,
        token_manager,
        op_store,
        op_config,
        session_store,
        session_config,
    })
}

pub fn op_router(state: OpState) -> cratestack::axum::Router {
    cratestack::axum::Router::new()
        .merge(state.op_axum_router())
        .with_state(state)
}
