#![doc = include_str!("auth.md")]

use std::collections::BTreeSet;
use std::fmt;

use serde::Deserialize;

use crate::error::ConfigError;

/// One way of authenticating, as `auth.methods` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AuthMethod {
    /// A request signed by a registered Ed25519 key, and the sealed answer
    /// (`COSE_Sign1`): the only way in vsms serves today.
    Keypair,
    /// A bearer token the deployment's own `OpenID` Provider minted from a
    /// `private_key_jwt` assertion. The provider is back (VSA-3a); the API
    /// accepting the token is VSA-3b.
    MachineTokens,
    /// A webhook endpoint delivered with `X-Sms-Signature`, a signature keyed by
    /// a secret both sides hold (`WebhookEndpoint.deliveryMode = hmac`).
    HmacWebhooks,
    /// A person's password or SSO sign-in to the console. Not restored yet
    /// (VSA-4).
    HumanLogin,
}

impl AuthMethod {
    /// Every method, in the order `auth.methods` documents them.
    pub const ALL: [Self; 4] = [
        Self::Keypair,
        Self::MachineTokens,
        Self::HmacWebhooks,
        Self::HumanLogin,
    ];

    /// The YAML spelling, which is also the discovery document's.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keypair => "keypair",
            Self::MachineTokens => "machine_tokens",
            Self::HmacWebhooks => "hmac_webhooks",
            Self::HumanLogin => "human_login",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|method| method.as_str() == name)
    }

    /// Whether this release has the code behind the method. Naming one that does
    /// not is refused ([`ConfigError::AuthMethodUnavailable`]); the absent
    /// default permits it without serving it.
    const fn is_restored(self) -> bool {
        match self {
            Self::Keypair | Self::HmacWebhooks | Self::MachineTokens => true,
            Self::HumanLogin => false,
        }
    }
}

impl fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `auth:` block of a document, as written.
///
/// `methods` keeps the strings the operator wrote so that an unknown name is a
/// [`ConfigError::UnknownAuthMethod`] naming the legal set rather than a serde
/// shape error; [`Self::resolve`] is the one place they become [`AuthMethod`]s.
///
/// A YAML null reads as an **empty list**, not as absent, so `methods:` with its
/// items commented out is [`ConfigError::NoAuthMethods`] and not a silent
/// all-four.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AuthConfig {
    /// `None`, absent from the YAML, permits every method. `Some` is exactly the
    /// list, and a null in the YAML is `Some(vec![])`.
    #[serde(default, deserialize_with = "present_methods")]
    pub methods: Option<Vec<String>>,
}

/// Reads the key when it is *present*: a list as itself, a null as the empty
/// list. `#[serde(default)]` is what makes an *absent* key `None`, and never
/// reaches this function, so the two stay distinguishable.
fn present_methods<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(
        Option::<Vec<String>>::deserialize(deserializer)?.unwrap_or_default(),
    ))
}

impl AuthConfig {
    /// Resolves `auth.methods` into the set a deployment permits, and checks the
    /// rules that need nothing but the list.
    ///
    /// # Errors
    ///
    /// [`ConfigError::NoAuthMethods`], [`ConfigError::UnknownAuthMethod`],
    /// [`ConfigError::DuplicateAuthMethod`],
    /// [`ConfigError::AuthMethodUnavailable`] and [`ConfigError::NoWayIn`]
    /// (rules 1 to 5 in the module docs). The set it returns knows nothing of the
    /// blocks, so [`AuthMethods::machine_tokens_served`] is `false`: the rules
    /// that need `op:` run once the whole document is read.
    pub fn resolve(&self) -> Result<AuthMethods, ConfigError> {
        let Some(names) = &self.methods else {
            return Ok(AuthMethods {
                permitted: AuthMethod::ALL.into_iter().collect(),
                explicit: false,
                op_block: false,
            });
        };
        if names.is_empty() {
            return Err(ConfigError::NoAuthMethods);
        }
        let mut permitted = BTreeSet::new();
        for name in names {
            let method = AuthMethod::parse(name)
                .ok_or_else(|| ConfigError::UnknownAuthMethod(name.clone()))?;
            if !permitted.insert(method) {
                return Err(ConfigError::DuplicateAuthMethod(method));
            }
        }
        if let Some(method) = AuthMethod::ALL
            .into_iter()
            .find(|method| permitted.contains(method) && !method.is_restored())
        {
            return Err(ConfigError::AuthMethodUnavailable(method));
        }
        let methods = AuthMethods {
            permitted,
            explicit: true,
            op_block: false,
        };
        if !methods.can_authenticate_a_caller() {
            return Err(ConfigError::NoWayIn);
        }
        Ok(methods)
    }
}

/// The resolved, validated set of permitted methods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethods {
    permitted: BTreeSet<AuthMethod>,
    explicit: bool,
    /// Whether the document has an `op:` block. Set by the one place that has seen the document; [`AuthConfig::resolve`] leaves
    /// it `false`.
    op_block: bool,
}

impl AuthMethods {
    /// Whether `method` is permitted.
    #[must_use]
    pub fn permits(&self, method: AuthMethod) -> bool {
        self.permitted.contains(&method)
    }

    /// Whether `auth.methods` was written. `false` is the all-four default.
    #[must_use]
    pub fn is_explicit(&self) -> bool {
        self.explicit
    }

    /// Every permitted method, in [`AuthMethod::ALL`] order.
    pub fn permitted(&self) -> impl Iterator<Item = AuthMethod> + '_ {
        AuthMethod::ALL
            .into_iter()
            .filter(|method| self.permits(*method))
    }

    /// Whether `machine_tokens` is **served**: permitted, and the `op:` block
    /// present. The one question the token routes, the discovery document, the
    /// signing-key boot check and the `provision-client` and `rotate-signing-key`
    /// commands all ask, so they cannot disagree about it.
    #[must_use]
    pub fn machine_tokens_served(&self) -> bool {
        self.permits(AuthMethod::MachineTokens) && self.op_block
    }

    /// The permitted methods this deployment actually serves: what
    /// `GET /.well-known/vsms-auth` tells a client.
    ///
    /// Permitted, minus the methods this release has no code for (`human_login`
    /// until VSA-4), and `machine_tokens` also needs its block
    /// ([`Self::machine_tokens_served`]). The absent default permits all four and
    /// serves two on a deployment that wrote no `op:`. **This is the one place
    /// that changes when `human_login` is restored.**
    pub fn served(&self) -> impl Iterator<Item = AuthMethod> + '_ {
        self.permitted().filter(|method| match method {
            AuthMethod::MachineTokens => self.machine_tokens_served(),
            other => other.is_restored(),
        })
    }

    /// The rules that need the document, given whether it has an `op:` block
    /// (`machine_tokens`' block), and the set that remembers the answer.
    ///
    /// # Errors
    ///
    /// [`ConfigError::AuthMethodNotConfigured`]: `machine_tokens` is named
    /// explicitly and `op:` is missing. [`ConfigError::AuthMethodDisabled`]: `op:`
    /// is present and `machine_tokens` is not permitted. An absent
    /// `auth.methods` raises neither: it permits the method and serves it only
    /// where the block is.
    pub(crate) fn with_op_block(mut self, op_present: bool) -> Result<Self, ConfigError> {
        let tokens = self.permits(AuthMethod::MachineTokens);
        if !tokens && op_present {
            return Err(ConfigError::AuthMethodDisabled {
                method: AuthMethod::MachineTokens,
                key: "op".to_owned(),
            });
        }
        if tokens && self.explicit && !op_present {
            return Err(ConfigError::AuthMethodNotConfigured {
                method: AuthMethod::MachineTokens,
                key: "op".to_owned(),
            });
        }
        self.op_block = op_present;
        Ok(self)
    }

    /// Whether something permitted can authenticate a caller of the API's `/v1`
    /// surface: a signed request or a token. vpay's `NoWayIn` is exactly
    /// `!keypair && !machine_tokens` and R5 holds the two repositories to the same
    /// rule, so `human_login` does not count: it signs a person in to the console,
    /// it does not authenticate an API caller. `hmac_webhooks` cannot either; it
    /// authenticates what vsms sends, never who is asking.
    fn can_authenticate_a_caller(&self) -> bool {
        [AuthMethod::Keypair, AuthMethod::MachineTokens]
            .into_iter()
            .any(|method| self.permits(method))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permitting(methods: &[AuthMethod]) -> AuthMethods {
        AuthMethods {
            permitted: methods.iter().copied().collect(),
            explicit: true,
            op_block: false,
        }
    }

    /// `resolve` refuses `human_login` as unavailable before it asks, so the list
    /// that tells the rules apart cannot be written in YAML today; this builds it.
    /// Without this test, counting `human_login` as a way in would change nothing
    /// anyone could observe until VSA-4 restores it, and then diverge from vpay.
    #[test]
    fn human_login_is_not_a_way_in_for_an_api_caller() {
        assert!(!permitting(&[AuthMethod::HumanLogin]).can_authenticate_a_caller());
        assert!(
            !permitting(&[AuthMethod::HumanLogin, AuthMethod::HmacWebhooks])
                .can_authenticate_a_caller()
        );
        assert!(!permitting(&[AuthMethod::HmacWebhooks]).can_authenticate_a_caller());
    }

    #[test]
    fn a_keypair_or_a_machine_token_is_a_way_in() {
        assert!(permitting(&[AuthMethod::Keypair]).can_authenticate_a_caller());
        assert!(permitting(&[AuthMethod::MachineTokens]).can_authenticate_a_caller());
    }
}
