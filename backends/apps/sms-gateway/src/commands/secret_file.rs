//! The one way this binary writes a secret an operator must be handed once: a new
//! file, `0600`, never overwriting, never printed. `provision-client` writes a
//! private key this way and `provision-user --password` writes a password.

use std::io::Write as _;
use std::path::Path;

use anyhow::{Context, Result};

/// Writes `contents` to a freshly created file at `path`, `0600` on Unix,
/// refusing to overwrite an existing file (`O_EXCL` via
/// [`std::fs::OpenOptions::create_new`]). Both the mode and the exclusivity are
/// applied atomically at `open(2)` time, so there is no window in which the file
/// exists with looser permissions or already-visible contents.
///
/// `what` names the secret in the error ("private key", "password"): a message
/// that says which thing a file may still hold is the reason an operator does not
/// `rm` it. The secret itself is never in an error.
pub(crate) fn write_new_secret_file(path: &Path, contents: &str, what: &str) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| {
        format!(
            "creating {} - refusing to overwrite an existing file, since it may hold a {what} \
             still in use",
            path.display()
        )
    })?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("writing the {what} to {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flushing the {what} to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sms-secret-file-{name}-{}",
            cratestack::uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn a_new_file_is_mode_0600_and_holds_exactly_the_secret() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let path = dir.join("secret");
        write_new_secret_file(&path, "s3cret", "password").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "s3cret");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_existing_file_is_refused_and_left_untouched() {
        let dir = scratch("excl");
        let path = dir.join("secret");
        std::fs::write(&path, "the one in use").unwrap();
        let error = write_new_secret_file(&path, "new", "password").unwrap_err();
        assert!(
            format!("{error:#}").contains("refusing to overwrite"),
            "{error:#}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the one in use");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_error_never_carries_the_secret() {
        let dir = scratch("leak");
        let path = dir.join("missing-dir").join("secret");
        let error = write_new_secret_file(&path, "hunter2-hunter2", "password").unwrap_err();
        assert!(!format!("{error:#}").contains("hunter2"), "{error:#}");
        std::fs::remove_dir_all(dir).ok();
    }
}
