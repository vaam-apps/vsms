#![doc = include_str!("lib.md")]

mod auth;
mod cli;
mod config;
mod error;
mod human_login;
mod model;
mod op;
mod provider;
pub mod retired;
mod secret;

pub use auth::{AuthConfig, AuthMethod, AuthMethods};
pub use cli::{ConfigArgs, DEFAULT_PROFILE};
pub use config::{Binary, Config};
pub use error::ConfigError;
pub use human_login::{CLIENT_ID_LENGTH, ConsoleClient, HumanLogin, LoginRateLimit};
pub use model::{Api, Bucket, Database, Deployment, Security};
pub use op::{ACCESS_TOKEN_TTL_SECS, Op, SigningKey};
pub use provider::{
    AWS_SMS, AwsSender, AwsSms, MTN_CM, MTN_DEFAULT_BASE_URL, MtnCm, ORANGE_CM,
    ORANGE_DEFAULT_BASE_URL, OrangeCm, Providers,
};
pub use retired::{RETIRED, Retired, check_args as reject_retired_flags};
pub use secret::Secret;
