#![doc = include_str!("lib.md")]

mod cli;
mod config;
mod error;
mod model;
mod provider;
pub mod retired;
mod secret;

pub use cli::{ConfigArgs, DEFAULT_PROFILE};
pub use config::Config;
pub use error::ConfigError;
pub use model::{Api, Bucket, Database, Deployment, Security};
pub use provider::{
    AWS_SMS, AwsSender, AwsSms, MTN_CM, MTN_DEFAULT_BASE_URL, MtnCm, ORANGE_CM,
    ORANGE_DEFAULT_BASE_URL, OrangeCm, Providers,
};
pub use retired::{RETIRED, Retired, check_args as reject_retired_flags};
pub use secret::Secret;
