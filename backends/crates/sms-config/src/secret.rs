//! A string that never reaches a log line.

use serde::Deserialize;

/// A secret read from the configuration: the hash pepper, an OAuth client
/// secret, an IAM secret access key.
///
/// `Debug` prints `<redacted>`, so a `{config:?}` in a startup trace, an
/// `anyhow` context or a test failure cannot leak one. [`Secret::expose`] is the
/// only way to the text, and it is called where the adapter's own config is
/// built, nowhere else.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// The secret itself. Call it only to hand the value to the adapter that
    /// authenticates with it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the secret is blank (after trimming).
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn debug_never_prints_the_value() {
        let secret: Secret = serde_yaml_ng::from_str("hunter2-hunter2-hunter2").unwrap();
        let printed = format!("{secret:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert_eq!(secret.expose(), "hunter2-hunter2-hunter2");
    }
}
