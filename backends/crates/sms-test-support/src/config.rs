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
//! does not: it never loads `security.*`. The suites that spawn one do not set
//! the variable, and say so by removing [`PEPPER_VAR`] from the child's
//! environment (`env_remove`), because they inherit their own: a developer's
//! shell may export it, and a worker that started on a pepper it was handed
//! would prove nothing.
//!
//! The directory a [`TestConfig`] writes is removed when it is dropped, so the
//! value has to outlive the process that reads it. A suite that spawns a
//! long-lived child keeps the `TestConfig` beside the child; one that waits for
//! the child to serve (it has read its configuration by then) may let it go.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// The profile the overlay is written as.
pub const PROFILE: &str = "test";

/// The variable the base file reads the pepper from.
pub const PEPPER_VAR: &str = "SMS_HASH_PEPPER";

/// A pepper that clears `HashPepper`'s minimum length. Not a secret.
pub const PEPPER: &str = "sms-test-support-hash-pepper-comfortably-over-the-minimum";

/// Makes two `TestConfig`s with the same tag in one process different
/// directories, so that dropping one cannot remove the other's.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory holding the base file and one overlay, and the flags that name
/// them. The directory is removed on drop.
#[derive(Debug)]
pub struct TestConfig {
    dir: PathBuf,
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
    /// `tag` and this process, inside the target directory
    /// (`CARGO_TARGET_TMPDIR`, which `cargo test` sets; `target/tmp` beside the
    /// workspace otherwise) and not in the system's temporary directory, where
    /// nothing would ever remove it.
    ///
    /// # Panics
    ///
    /// On an I/O error: a suite that cannot write its configuration has nothing
    /// to run.
    #[must_use]
    pub fn write(tag: &str, overlay: &str) -> Self {
        let repo_base =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/application.yml");
        let root = std::env::var_os("CARGO_TARGET_TMPDIR").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/tmp"),
            PathBuf::from,
        );
        let dir = root.join(format!(
            "sms-test-config-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("creating the test configuration directory");
        let base = dir.join("application.yml");
        std::fs::copy(&repo_base, &base).expect("copying config/application.yml");
        std::fs::write(dir.join(format!("application-{PROFILE}.yml")), overlay)
            .expect("writing the test overlay");
        Self { dir, base }
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

impl Drop for TestConfig {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_is_removed_when_the_config_is_dropped() {
        let config = TestConfig::write("drop", &no_providers_overlay());
        let dir = config.dir.clone();
        assert!(dir.join("application.yml").is_file());
        assert!(dir.join("application-test.yml").is_file());
        drop(config);
        assert!(!dir.exists(), "{} outlived its TestConfig", dir.display());
    }

    #[test]
    fn two_configs_with_one_tag_do_not_share_a_directory() {
        let first = TestConfig::write("twin", &no_providers_overlay());
        let second = TestConfig::write("twin", &no_providers_overlay());
        let kept = second.dir.clone();
        drop(first);
        assert!(
            kept.join("application.yml").is_file(),
            "dropping one config removed another's directory"
        );
    }
}
