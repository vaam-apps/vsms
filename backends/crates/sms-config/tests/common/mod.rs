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
            let dir =
                std::env::temp_dir().join(format!("sms-config-base-alone-{}", std::process::id()));
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
