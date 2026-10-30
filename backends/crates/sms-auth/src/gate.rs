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

/// `rotate-signing-key` was run against a deployment that mounts no `OpenID`
/// Provider, so there is no key to rotate. Leads with `auth_method_disabled`
/// like [`TokensNotServed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "auth_method_disabled: this deployment mounts no OpenID Provider ({reason}), so it has no \
     signing key to rotate; the provider is mounted when machine_tokens or human_login is \
     served (a machine client can register a signing key with register-client instead)"
)]
pub struct ProviderNotServed {
    reason: &'static str,
}

/// Whether the provider is mounted (`AuthMethods::op_served`: `op:` is written
/// and `machine_tokens` or `human_login` is served). The question
/// `rotate-signing-key` asks: the signing key signs a machine's access token and a
/// person's `id_token` alike, so either method is a reason to rotate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// `/token` and `/jwks.json` are mounted.
    Served,
    /// Nothing is mounted; the command refuses.
    NotServed {
        /// Why, in words an operator can act on.
        reason: &'static str,
    },
}

impl Provider {
    /// The gate for a resolved `auth.methods` and its blocks.
    #[must_use]
    pub fn from_methods(methods: &sms_config::AuthMethods) -> Self {
        if methods.op_served() {
            Self::Served
        } else if !methods.permits(sms_config::AuthMethod::MachineTokens)
            && !methods.permits(sms_config::AuthMethod::HumanLogin)
        {
            Self::NotServed {
                reason: "auth.methods permits neither machine_tokens nor human_login",
            }
        } else {
            Self::NotServed {
                reason: "no op: block in the configuration",
            }
        }
    }

    /// `Ok` when the provider is mounted.
    ///
    /// # Errors
    ///
    /// [`ProviderNotServed`], whose message leads with `auth_method_disabled`.
    pub fn require(self) -> Result<(), ProviderNotServed> {
        match self {
            Self::Served => Ok(()),
            Self::NotServed { reason } => Err(ProviderNotServed { reason }),
        }
    }
}

/// A command that sets a person's password or registers the console client was
/// run against a deployment that does not serve `human_login`.
///
/// Leads with `auth_method_disabled` like [`TokensNotServed`], so a script switches
/// on one string for every disabled method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "auth_method_disabled: this deployment signs no person in ({reason}); a person gets in by \
     pairing a browser key (pair-console-key)"
)]
pub struct LoginNotServed {
    reason: &'static str,
}

/// Whether this deployment serves `human_login` (`auth.methods` permits it, the
/// `human_login:` block is written **and** `op:` is:
/// `sms_config::AuthMethods::human_login_served`).
///
/// A two-valued type for the reason [`MachineTokens`] is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanLogin {
    /// `POST /login` and the `authorization_code` grant are mounted;
    /// `seed-console-client`, `provision-user --password` and
    /// `bootstrap --owner-password-out` run.
    Served,
    /// Nothing is mounted; they refuse.
    NotServed {
        /// Why, in words an operator can act on.
        reason: &'static str,
    },
}

impl HumanLogin {
    /// The gate for a resolved `auth.methods` and its blocks.
    #[must_use]
    pub fn from_methods(methods: &sms_config::AuthMethods) -> Self {
        if methods.human_login_served() {
            Self::Served
        } else if !methods.permits(sms_config::AuthMethod::HumanLogin) {
            Self::NotServed {
                reason: "auth.methods does not permit human_login",
            }
        } else {
            // Permitted by the absent default and not served: a block is
            // missing. `human_login:` without `op:` is refused at load, so
            // the one that can be missing here is `human_login:` itself.
            Self::NotServed {
                reason: "no human_login: block in the configuration",
            }
        }
    }

    /// `Ok` when the sign-in is served.
    ///
    /// # Errors
    ///
    /// [`LoginNotServed`], whose message leads with `auth_method_disabled`.
    pub fn require(self) -> Result<(), LoginNotServed> {
        match self {
            Self::Served => Ok(()),
            Self::NotServed { reason } => Err(LoginNotServed { reason }),
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

    #[test]
    fn the_provider_gate_is_open_for_either_method_and_shut_for_neither() {
        let tokens_only = methods(Some(&["keypair", "machine_tokens"]));
        assert!(!tokens_only.op_served(), "no block, nothing is mounted");
        let message = Provider::from_methods(&methods(Some(&["keypair"])))
            .require()
            .unwrap_err()
            .to_string();
        assert!(
            message.starts_with(&format!("{AUTH_METHOD_DISABLED}:")),
            "{message}"
        );
        assert!(
            message.contains("neither machine_tokens nor human_login"),
            "{message}"
        );
        let absent = Provider::from_methods(&methods(None))
            .require()
            .unwrap_err()
            .to_string();
        assert!(absent.contains("no op: block"), "{absent}");
    }

    #[test]
    fn the_login_gate_refuses_with_the_stable_word_and_says_which_piece_is_missing() {
        let keypair_only = HumanLogin::from_methods(&methods(Some(&["keypair"])))
            .require()
            .unwrap_err()
            .to_string();
        assert!(
            keypair_only.starts_with(&format!("{AUTH_METHOD_DISABLED}:")),
            "{keypair_only}"
        );
        assert!(
            keypair_only.contains("does not permit human_login"),
            "{keypair_only}"
        );
        assert!(keypair_only.contains("pair-console-key"), "{keypair_only}");

        // The absent default permits human_login; with no block it serves nothing.
        let absent = HumanLogin::from_methods(&methods(None))
            .require()
            .unwrap_err()
            .to_string();
        assert!(absent.contains("no human_login: block"), "{absent}");
    }
}
