#![doc = include_str!("gate.md")]

/// A command that mints or rotates the OP's credentials was run against a
/// deployment that does not serve `machine_tokens`.
///
/// Its message leads with `auth_method_disabled` ([`sms_api::AUTH_METHOD_DISABLED`]), the
/// stable word the 422 for a disabled `hmac_webhooks` leads with, so a script
/// switches on one string for both. (A literal in the attribute because
/// `thiserror` reads `{name}` as a field; a test holds it to the constant.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "auth_method_disabled: this deployment issues no tokens ({reason}); use register-client \
     to register a signing key instead"
)]
pub struct TokensNotServed {
    reason: &'static str,
}

/// Whether this deployment serves `machine_tokens` (`auth.methods` permits it
/// **and** `op:` is present: `sms_config::AuthMethods::machine_tokens_served`).
///
/// A two-valued type, not a `bool`, for the reason `HmacWebhooks` is one: the
/// commands take it beside other arguments, and `run(.., true)` does not say
/// which way it fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineTokens {
    /// `/token` is mounted; `provision-client` and `rotate-signing-key` run.
    Served,
    /// Nothing is mounted; they refuse.
    NotServed {
        /// Why, in words an operator can act on.
        reason: &'static str,
    },
}

impl MachineTokens {
    /// The gate for a resolved `auth.methods` and `op:`.
    #[must_use]
    pub fn from_methods(methods: &sms_config::AuthMethods) -> Self {
        if methods.machine_tokens_served() {
            Self::Served
        } else if methods.permits(sms_config::AuthMethod::MachineTokens) {
            Self::NotServed {
                reason: "no op: block in the configuration",
            }
        } else {
            Self::NotServed {
                reason: "auth.methods does not permit machine_tokens",
            }
        }
    }

    /// `Ok` when tokens are served.
    ///
    /// # Errors
    ///
    /// [`TokensNotServed`], whose message leads with `auth_method_disabled`.
    pub fn require(self) -> Result<(), TokensNotServed> {
        match self {
            Self::Served => Ok(()),
            Self::NotServed { reason } => Err(TokensNotServed { reason }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sms_api::AUTH_METHOD_DISABLED;
    use sms_config::{AuthConfig, AuthMethod};

    fn methods(written: Option<&[&str]>) -> sms_config::AuthMethods {
        AuthConfig {
            methods: written.map(|names| names.iter().map(|name| (*name).to_owned()).collect()),
        }
        .resolve()
        .expect("a legal list")
    }

    #[test]
    fn a_method_off_refuses_with_the_stable_word_and_the_way_out() {
        let gate = MachineTokens::from_methods(&methods(Some(&["keypair"])));
        let message = gate.require().unwrap_err().to_string();
        assert!(
            message.starts_with(&format!("{AUTH_METHOD_DISABLED}:")),
            "{message}"
        );
        assert!(message.contains("issues no tokens"), "{message}");
        assert!(message.contains("register-client"), "{message}");
        assert!(message.contains("auth.methods"), "{message}");
    }

    #[test]
    fn permitted_without_a_block_is_off_and_says_the_block_is_missing() {
        // The absent default permits `machine_tokens`; no `op:` means no tokens.
        let all = methods(None);
        assert!(all.permits(AuthMethod::MachineTokens));
        let message = MachineTokens::from_methods(&all)
            .require()
            .unwrap_err()
            .to_string();
        assert!(message.contains("no op: block"), "{message}");
    }
}
