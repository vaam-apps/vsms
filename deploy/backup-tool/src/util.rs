//! Small pieces shared across subcommands.

use std::fs;
use std::path::PathBuf;

/// A scratch directory this process owns exclusively, removed on drop —
/// same shape `.xtask/src/migrations_current.rs::ScratchDir` already
/// uses in the main workspace's own repo-automation tool, reproduced here
/// rather than shared, since the two crates deliberately don't depend on
/// each other (see this crate's own `Cargo.toml` header). Cleans up on
/// every exit path — success, an early `?`, or a panic unwind — without
/// needing a `trap ... EXIT` the way the shell scripts this replaces did.
pub struct ScratchDir(pub PathBuf);

impl ScratchDir {
    pub fn new(prefix: &str) -> anyhow::Result<Self> {
        let mut dir = std::env::temp_dir();
        let unique = format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        dir.push(unique);
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
