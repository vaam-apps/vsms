//! Shared by the tests that load the committed `config/application.yml`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The committed base file on its own: copied to a directory that holds no
/// profile overlay, because a profile with no file is refused unless it is the
/// default, and the default's overlay (`application-dev.yml`) sits beside the
/// original.
pub fn committed_base_alone() -> PathBuf {
    static ALONE: OnceLock<PathBuf> = OnceLock::new();
    ALONE
        .get_or_init(|| {
            // Cargo's own scratch directory for integration tests, inside the
            // target directory: nothing to clean up in the system temp dir.
            let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sms-config-base-alone");
            std::fs::create_dir_all(&dir).unwrap();
            let target = dir.join("application.yml");
            std::fs::copy(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/application.yml"),
                &target,
            )
            .unwrap();
            target
        })
        .clone()
}
