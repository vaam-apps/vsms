#![doc = include_str!("sso.md")]

pub mod config;
pub mod error;
pub mod flow;
pub mod id_token;
pub mod policy;
pub mod provider;
pub mod store;

pub use config::{
    CALLBACK_PATH, ConfigError, DEFAULT_GROUPS_CLAIM, DEFAULT_JIT_ROLE, DEFAULT_SCOPES,
    RawSsoConfig, START_PATH, SsoConfig, require_secure_url,
};
pub use error::{Refusal, SsoError};
pub use id_token::IdClaims;
pub use provider::{AuthnSecrets, SsoProvider};
pub use sms_api::auth::RESERVED_ROLE_KEYS;
