#![doc = include_str!("cli.md")]

use std::path::PathBuf;

use crate::{Config, ConfigError};

/// The two options every binary that reads the YAML takes. Flattened into
/// `sms-gateway serve`, `sms-gateway mtn-subscribe-dlr` and `sms-worker`; the
/// other subcommands read no configuration and take neither.
#[derive(Debug, Clone, clap::Args)]
pub struct ConfigArgs {
    /// Path to the base YAML configuration. There is no default: a deployment
    /// names its file. The images bake `/config/application.yml` and set this.
    #[arg(long, env = "SMS_CONFIG")]
    pub config: Option<PathBuf>,

    /// Deployment profile: overlays `application-<profile>.yml`, beside the
    /// base file, when that file exists. It selects a file and never a code
    /// path.
    #[arg(long, env = "SMS_PROFILE", default_value = DEFAULT_PROFILE)]
    pub profile: String,
}

/// The profile a binary loads when `--profile` and `SMS_PROFILE` are both
/// absent: the developer stack, which points at the local fakes and is never
/// livemode.
pub const DEFAULT_PROFILE: &str = "dev";

impl ConfigArgs {
    /// Loads the configuration these arguments name, against the real
    /// environment.
    ///
    /// # Errors
    ///
    /// See [`ConfigError`].
    pub fn load(&self) -> Result<Config, ConfigError> {
        Config::load(self.config.as_deref(), &self.profile)
    }
}
