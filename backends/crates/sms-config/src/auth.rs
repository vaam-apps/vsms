#![doc = include_str!("auth.md")]

use std::collections::BTreeSet;
use std::fmt;

use serde::Deserialize;

use crate::error::ConfigError;

/// One way of authenticating, as `auth.methods` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AuthMethod {
    /// A request signed by a registered Ed25519 key, and the sealed answer
    /// (`COSE_Sign1`).
    Keypair,
    /// A bearer token the deployment's own `OpenID` Provider minted from a
    /// `private_key_jwt` assertion. The provider is back (VSA-3a) and the API
    /// accepts the token beside the signed envelope (VSA-3b), so a list naming
    /// this method without `keypair` is a legal one.
    MachineTokens,
    /// A webhook endpoint delivered with `X-Sms-Signature`, a signature keyed by
    /// a secret both sides hold (`WebhookEndpoint.deliveryMode = hmac`).
    HmacWebhooks,
    /// A person's password sign-in to the console, through the deployment's own
    /// `OpenID` Provider (`authorization_code` with PKCE). Restored by VSA-4a;
    /// the API accepts the access token it mints (VSA-4b), never as an `owner` or
    /// an `admin`.
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
    /// [`ConfigError::DuplicateAuthMethod`] and [`ConfigError::NoWayIn`]
    /// (rules 1 to 4 in the module docs). The set it returns knows nothing of the
    /// blocks, so [`AuthMethods::machine_tokens_served`] and
    /// [`AuthMethods::human_login_served`] are `false`: the rules that need `op:`
    /// and `human_login:` run once the whole document is read.
    pub fn resolve(&self) -> Result<AuthMethods, ConfigError> {
        let Some(names) = &self.methods else {
            return Ok(AuthMethods {
                permitted: AuthMethod::ALL.into_iter().collect(),
                explicit: false,
                op_block: false,
                human_login_block: false,
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
        let methods = AuthMethods {
            permitted,
            explicit: true,
            op_block: false,
            human_login_block: false,
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
    /// Whether the document has an `op:` block. Set by the one place that has
    /// seen the document; [`AuthConfig::resolve`] leaves it `false`.
    op_block: bool,
    /// Whether the document has a `human_login:` block, set the same way.
    human_login_block: bool,
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

    /// Whether `human_login` is **served**: permitted, the `human_login:` block
    /// present **and** `op:` present (the sign-in rides the `OpenID` Provider, so
    /// it cannot run without it). The one question `POST /login`, the grant gate,
    /// the discovery document, the boot check for the console client and the
    /// `seed-console-client`, `provision-user --password` and
    /// `bootstrap --owner-password-out` commands all ask, so they cannot disagree
    /// about it.
    #[must_use]
    pub fn human_login_served(&self) -> bool {
        self.permits(AuthMethod::HumanLogin) && self.human_login_block && self.op_block
    }

    /// Whether the `OpenID` Provider is mounted: the `op:` block is present and
    /// at least one method that rides it is served. `/token`, `/jwks.json` and the
    /// discovery document exist exactly then.
    #[must_use]
    pub fn op_served(&self) -> bool {
        self.machine_tokens_served() || self.human_login_served()
    }

    /// The permitted methods this deployment actually serves: what
    /// `GET /.well-known/vsms-auth` tells a client.
    ///
    /// Permitted, and for a method with a block, whose block is present
    /// ([`Self::machine_tokens_served`], [`Self::human_login_served`]). The
    /// absent default permits all four and serves two on a deployment that wrote
    /// neither `op:` nor `human_login:`.
    pub fn served(&self) -> impl Iterator<Item = AuthMethod> + '_ {
        self.permitted().filter(|method| match method {
            AuthMethod::MachineTokens => self.machine_tokens_served(),
            AuthMethod::HumanLogin => self.human_login_served(),
            AuthMethod::Keypair | AuthMethod::HmacWebhooks => true,
        })
    }

    /// The rules that need the document, given whether it has an `op:` block and
    /// a `human_login:` block, and the set that remembers the answers.
    ///
    /// # Errors
    ///
    /// [`ConfigError::AuthMethodDisabled`]: `human_login:` is present and
    /// `human_login` is not permitted, or `op:` is present and neither
    /// `machine_tokens` nor `human_login` is. [`ConfigError::AuthMethodNotConfigured`]:
    /// `machine_tokens` is named explicitly and `op:` is missing, `human_login` is
    /// named explicitly and `human_login:` is missing, or `human_login:` is
    /// present without `op:` (the sign-in rides the provider; this one holds for
    /// the absent default too). An absent `auth.methods` raises none of the
    /// "named" ones: it permits the methods and serves each only where its block
    /// is.
    pub(crate) fn with_blocks(
        mut self,
        op_present: bool,
        human_login_present: bool,
    ) -> Result<Self, ConfigError> {
        let tokens = self.permits(AuthMethod::MachineTokens);
        let human = self.permits(AuthMethod::HumanLogin);
        if !human && human_login_present {
            return Err(ConfigError::AuthMethodDisabled {
                methods: vec![AuthMethod::HumanLogin],
                key: "human_login".to_owned(),
            });
        }
        if !tokens && !human && op_present {
            return Err(ConfigError::AuthMethodDisabled {
                methods: vec![AuthMethod::MachineTokens, AuthMethod::HumanLogin],
                key: "op".to_owned(),
            });
        }
        if tokens && self.explicit && !op_present {
            return Err(ConfigError::AuthMethodNotConfigured {
                method: AuthMethod::MachineTokens,
                key: "op".to_owned(),
            });
        }
        if human && self.explicit && !human_login_present {
            return Err(ConfigError::AuthMethodNotConfigured {
                method: AuthMethod::HumanLogin,
                key: "human_login".to_owned(),
            });
        }
        if human_login_present && !op_present {
            return Err(ConfigError::AuthMethodNotConfigured {
                method: AuthMethod::HumanLogin,
                key: "op".to_owned(),
            });
        }
        self.op_block = op_present;
        self.human_login_block = human_login_present;
        Ok(self)
    }

    /// Whether something permitted can authenticate a caller of the API's `/v1`
    /// surface: vpay's `NoWayIn` is `!keypair && !machine_tokens`, and R5 holds
    /// the two repositories to that rule. Since VSA-3b the API accepts a machine
    /// token beside the signed envelope, so `machine_tokens` counts and
    /// `[machine_tokens]` alone is a legal list (it boots with its `op:` block;
    /// without one it is `AuthMethodNotConfigured`, the next rule). `human_login`
    /// does not count: it signs a person in to the console, it does not
    /// authenticate an API caller. `hmac_webhooks` cannot either; it
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
            human_login_block: false,
        }
    }

    /// vpay's rule (R5): `human_login` signs a person in to the console and says
    /// nothing about who an API caller is. `resolve` refuses a list with no way
    /// in before it can be observed through a loaded document, so this builds the
    /// sets directly. Since VSA-4b the API accepts a person's token, and the rule
    /// still holds: a person's token is not a way in by itself. A list must still
    /// permit the keypair or `machine_tokens`, and the person's token is one more
    /// credential beside them, never what makes a deployment reachable.
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
    fn a_keypair_is_a_way_in() {
        assert!(permitting(&[AuthMethod::Keypair]).can_authenticate_a_caller());
        assert!(
            permitting(&[AuthMethod::Keypair, AuthMethod::MachineTokens])
                .can_authenticate_a_caller()
        );
    }

    /// Since VSA-3b the API accepts a bearer, so a machine token is a way in on
    /// its own. (`resolve` still needs its `op:` block for it afterwards, which
    /// is a different rule.)
    #[test]
    fn machine_tokens_alone_is_a_way_in() {
        assert!(permitting(&[AuthMethod::MachineTokens]).can_authenticate_a_caller());
        assert!(
            permitting(&[AuthMethod::MachineTokens, AuthMethod::HmacWebhooks])
                .can_authenticate_a_caller()
        );
    }
}
