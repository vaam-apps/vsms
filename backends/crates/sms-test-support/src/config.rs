//! A YAML configuration for a binary a live suite spawns (VSC-1).
//!
//! `sms-gateway serve` and `sms-worker` read `config/application.yml` plus a
//! profile overlay. A suite that spawns one writes only the overlay - the
//! providers it needs, with literal sandbox credentials - and this copies the
//! real base file next to it, so the spawned process runs on the repository's
//! own defaults rather than a second copy of them that could drift.
//!
//! The base file reads the pepper as `${SMS_HASH_PEPPER}`, so a spawned
//! `sms-gateway` needs [`PEPPER_VAR`] set to [`PEPPER`]. A spawned `sms-worker`
//! does not: it never loads `security.*`, and the suites that spawn one leave the
//! variable out on purpose, which is what keeps that true.

use std::path::{Path, PathBuf};

/// The profile the overlay is written as.
pub const PROFILE: &str = "test";

/// The variable the base file reads the pepper from.
pub const PEPPER_VAR: &str = "SMS_HASH_PEPPER";

/// A pepper that clears `HashPepper`'s minimum length. Not a secret.
pub const PEPPER: &str = "sms-test-support-hash-pepper-comfortably-over-the-minimum";

/// A directory holding the base file and one overlay, and the flags that name
/// them.
#[derive(Debug, Clone)]
pub struct TestConfig {
    base: PathBuf,
}

/// An `orange_cm` provider entry with sandbox credentials, pointed at
/// `base_url` (a fake Orange, or a port nothing listens on).
#[must_use]
pub fn orange_overlay(base_url: &str) -> String {
    format!(
        "providers:\n  - code: orange_cm\n    host:\n      url: {base_url}\n    credentials:\n      \
         client_id: test-orange-client-id\n      client_secret: test-orange-client-secret\n    \
         settings:\n      sender_number: \"+237600000000\"\n"
    )
}

/// An overlay with no provider at all: what a `hooks`-only worker needs, since
/// the loader accepts a document with none.
#[must_use]
pub fn no_providers_overlay() -> String {
    "providers: []\n".to_owned()
}

impl TestConfig {
    /// Writes `application.yml` (a copy of the repository's) and
    /// `application-test.yml` (`overlay`) under a fresh directory named for
    /// `tag` and this process.
    ///
    /// # Panics
    ///
    /// On an I/O error: a suite that cannot write its configuration has nothing
    /// to run.
    #[must_use]
    pub fn write(tag: &str, overlay: &str) -> Self {
        let repo_base =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/application.yml");
        let dir =
            std::env::temp_dir().join(format!("sms-test-config-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("creating the test configuration directory");
        let base = dir.join("application.yml");
        std::fs::copy(&repo_base, &base).expect("copying config/application.yml");
        std::fs::write(dir.join(format!("application-{PROFILE}.yml")), overlay)
            .expect("writing the test overlay");
        Self { base }
    }

    /// `--config <base> --profile test`, for the spawned command line.
    #[must_use]
    pub fn args(&self) -> [String; 4] {
        [
            "--config".to_owned(),
            self.base.display().to_string(),
            "--profile".to_owned(),
            PROFILE.to_owned(),
        ]
    }
}
