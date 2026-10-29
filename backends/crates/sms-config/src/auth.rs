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
    /// `private_key_jwt` assertion. Not restored yet (VSA-3).
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
            Self::Keypair | Self::HmacWebhooks => true,
            Self::MachineTokens | Self::HumanLogin => false,
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
    /// (rules 1 to 5 in the module docs).
    pub fn resolve(&self) -> Result<AuthMethods, ConfigError> {
        let Some(names) = &self.methods else {
            return Ok(AuthMethods {
                permitted: AuthMethod::ALL.into_iter().collect(),
                explicit: false,
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

    /// The permitted methods this release actually serves: what
    /// `GET /.well-known/vsms-auth` tells a client.
    ///
    /// **Permitted minus the methods this release has no code for**
    /// (`machine_tokens` until VSA-3, `human_login` until VSA-4), and nothing
    /// else. The absent default permits all four and serves two. **This is the
    /// one place that changes when either is restored.**
    pub fn served(&self) -> impl Iterator<Item = AuthMethod> + '_ {
        self.permitted().filter(|method| method.is_restored())
    }

    /// Whether something permitted can authenticate a caller of the API: a
    /// signed request, a token or a signed-in person. `hmac_webhooks` cannot; it
    /// authenticates what vsms sends, never who is asking.
    fn can_authenticate_a_caller(&self) -> bool {
        [
            AuthMethod::Keypair,
            AuthMethod::MachineTokens,
            AuthMethod::HumanLogin,
        ]
        .into_iter()
        .any(|method| self.permits(method))
    }
}
