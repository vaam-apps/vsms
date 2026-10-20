//! SSO configuration: what the operator hands the gateway, validated into
//! something the rest of the broker can trust.
//!
//! **Fail closed.** [`SsoConfig::from_raw`] refuses a half-filled block, a
//! block with no MFA rule, a plaintext non-loopback URL, and a JIT role that
//! is one of the two reserved keys. The gateway turns any of those into a
//! startup failure: SSO that is configured but cannot enforce its own
//! conditions must not run as though it could.

use std::time::Duration;

use thiserror::Error;
use url::{Host, Url};

use crate::sso::RESERVED_ROLE_KEYS;

/// Scopes requested upstream when none are configured.
pub const DEFAULT_SCOPES: &str = "openid email profile";
/// The `id_token` claim the group list is read from when none is configured.
pub const DEFAULT_GROUPS_CLAIM: &str = "groups";
/// The role a just-in-time provisioned user gets when none is configured.
/// `support` has the fewest permissions of the six built-in roles - see
/// `docs/architecture.md` section 4.3 for the comparison.
pub const DEFAULT_JIT_ROLE: &str = "support";
/// Minimum gap between two upstream JWKS fetches (an unknown `kid` triggers
/// one refetch, but never more often than this).
pub const DEFAULT_JWKS_MIN_REFRESH_SECS: u64 = 30;
/// Where the gateway mounts the browser's entry point.
pub const START_PATH: &str = "/login/sso/start";
/// Where the gateway mounts the upstream `IdP`'s return leg. The configured
/// `redirect_uri` must end in exactly this path.
pub const CALLBACK_PATH: &str = "/login/sso/callback";

/// A secret that never prints: `Debug` shows a placeholder, so a stray
/// `{:?}` of the config in a log line cannot leak it.
#[derive(Clone)]
pub struct ClientSecret(String);

impl ClientSecret {
    /// The secret itself, for the one place that must send it (the code
    /// exchange's `Authorization` header).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ClientSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClientSecret([redacted])")
    }
}

/// The operator's input, exactly as it arrives from flags and environment:
/// every scalar optional, blank meaning unset (an env var that is present
/// but empty is a value to `clap`, not an absence - AGENTS.md's MTN section
/// measured it - so a blank is normalised away here rather than trusted to
/// every deployment tool).
#[derive(Debug, Clone, Default)]
pub struct RawSsoConfig {
    /// `--sso-issuer`.
    pub issuer: Option<String>,
    /// `--sso-client-id`.
    pub client_id: Option<String>,
    /// `--sso-client-secret`.
    pub client_secret: Option<String>,
    /// `--sso-redirect-uri`.
    pub redirect_uri: Option<String>,
    /// `--sso-scopes`.
    pub scopes: Option<String>,
    /// `--sso-required-group`.
    pub required_group: Option<String>,
    /// `--sso-groups-claim`.
    pub groups_claim: Option<String>,
    /// `--sso-accepted-acr`.
    pub accepted_acr: Vec<String>,
    /// `--sso-accepted-amr`.
    pub accepted_amr: Vec<String>,
    /// `--sso-jit-role`.
    pub jit_role: Option<String>,
    /// `--sso-jwks-min-refresh-secs`.
    pub jwks_min_refresh_secs: Option<u64>,
}

/// A validated SSO configuration. One issuer per deployment.
#[derive(Debug, Clone)]
pub struct SsoConfig {
    /// The upstream issuer, **verbatim** - the discovery document's `issuer`
    /// and every `id_token`'s `iss` are compared to this byte for byte, so it
    /// is never normalised (no trailing-slash trimming, no case folding).
    pub issuer: String,
    /// This gateway's client id at the upstream `IdP`.
    pub client_id: String,
    /// This gateway's client secret at the upstream `IdP`.
    pub client_secret: ClientSecret,
    /// The gateway's public callback, `.../login/sso/callback`.
    pub redirect_uri: String,
    /// Space-delimited scopes requested upstream; always contains `openid`.
    pub scopes: String,
    /// The `IdP` group a user must be in to sign in at all.
    pub required_group: String,
    /// The `id_token` claim the group list lives in.
    pub groups_claim: String,
    /// `acr` values that satisfy the MFA rule. May be empty if
    /// `accepted_amr` is not.
    pub accepted_acr: Vec<String>,
    /// `amr` values that satisfy the MFA rule (any one suffices). May be
    /// empty if `accepted_acr` is not.
    pub accepted_amr: Vec<String>,
    /// The `Role.key` a just-in-time provisioned user is created with.
    pub jit_role_key: String,
    /// Floor on the gap between two JWKS fetches.
    pub jwks_min_refresh: Duration,
}

/// Why a [`RawSsoConfig`] was refused. Every variant is a startup failure.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    /// Some of the required SSO settings are present and some are not.
    #[error(
        "SSO is partly configured: {} must be set together with the rest, or none of the SSO \
         settings at all",
        .missing.join(", ")
    )]
    Incomplete {
        /// The flags that are missing.
        missing: Vec<&'static str>,
    },
    /// Both the accepted `acr` list and the accepted `amr` list are empty,
    /// so no `id_token` could ever satisfy the MFA rule - or, worse, a future
    /// edit could read "empty" as "no rule".
    #[error(
        "SSO needs a multi-factor rule: set --sso-accepted-amr and/or --sso-accepted-acr \
         (both are empty)"
    )]
    NoMfaRule,
    /// A URL setting did not parse, or carries a part it must not.
    #[error("{field} is not acceptable: {reason}")]
    BadUrl {
        /// Which flag.
        field: &'static str,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The requested scopes do not include `openid`.
    #[error("--sso-scopes must include `openid`")]
    MissingOpenidScope,
    /// The JIT role is one of the keys no human may ever hold.
    #[error("--sso-jit-role {0:?} is reserved and can never be given to a human")]
    ReservedJitRole(String),
}

fn non_blank(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

fn clean_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .collect()
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Parses `value` and enforces what every URL this broker talks to or is
/// told about must satisfy: `https`, except plain `http` to a loopback host
/// (a developer's Keycloak, the test suite's fake). The client secret and
/// authorization code travel over these, so a cleartext remote URL is
/// refused outright.
///
/// # Errors
///
/// [`ConfigError::BadUrl`] for anything that does not parse or is cleartext
/// to a non-loopback host.
pub fn require_secure_url(field: &'static str, value: &str) -> Result<Url, ConfigError> {
    let url = Url::parse(value).map_err(|_| ConfigError::BadUrl {
        field,
        reason: "not an absolute URL",
    })?;
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(&url) => {}
        _ => {
            return Err(ConfigError::BadUrl {
                field,
                reason: "must be https (plain http only to localhost)",
            });
        }
    }
    if url.host().is_none() {
        return Err(ConfigError::BadUrl {
            field,
            reason: "has no host",
        });
    }
    if url.fragment().is_some() {
        return Err(ConfigError::BadUrl {
            field,
            reason: "must not carry a fragment",
        });
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConfigError::BadUrl {
            field,
            reason: "must not carry credentials",
        });
    }
    Ok(url)
}

impl SsoConfig {
    /// Validates `raw`. `Ok(None)` means SSO is not configured at all (every
    /// required setting is absent) - not a failure, the deployment simply has
    /// no SSO. SSO is active exactly when its configuration is present; there
    /// is no other switch.
    ///
    /// # Errors
    ///
    /// See [`ConfigError`]; every variant must stop the process starting.
    pub fn from_raw(raw: RawSsoConfig) -> Result<Option<Self>, ConfigError> {
        let issuer = non_blank(raw.issuer);
        let client_id = non_blank(raw.client_id);
        let client_secret = non_blank(raw.client_secret);
        let redirect_uri = non_blank(raw.redirect_uri);
        let required_group = non_blank(raw.required_group);

        if issuer.is_none()
            && client_id.is_none()
            && client_secret.is_none()
            && redirect_uri.is_none()
            && required_group.is_none()
        {
            return Ok(None);
        }

        let mut missing = Vec::new();
        for (name, present) in [
            ("--sso-issuer", issuer.is_some()),
            ("--sso-client-id", client_id.is_some()),
            ("--sso-client-secret", client_secret.is_some()),
            ("--sso-redirect-uri", redirect_uri.is_some()),
            ("--sso-required-group", required_group.is_some()),
        ] {
            if !present {
                missing.push(name);
            }
        }
        let (
            Some(issuer),
            Some(client_id),
            Some(client_secret),
            Some(redirect_uri),
            Some(required_group),
        ) = (
            issuer,
            client_id,
            client_secret,
            redirect_uri,
            required_group,
        )
        else {
            return Err(ConfigError::Incomplete { missing });
        };

        let classes = clean_list(raw.accepted_acr);
        let methods = clean_list(raw.accepted_amr);
        if classes.is_empty() && methods.is_empty() {
            return Err(ConfigError::NoMfaRule);
        }

        let issuer_url = require_secure_url("--sso-issuer", &issuer)?;
        if issuer_url.query().is_some() {
            return Err(ConfigError::BadUrl {
                field: "--sso-issuer",
                reason: "must not carry a query",
            });
        }
        let callback_url = require_secure_url("--sso-redirect-uri", &redirect_uri)?;
        if callback_url.path() != CALLBACK_PATH || callback_url.query().is_some() {
            return Err(ConfigError::BadUrl {
                field: "--sso-redirect-uri",
                reason: "must end in exactly /login/sso/callback, with no query",
            });
        }

        let scopes = non_blank(raw.scopes).unwrap_or_else(|| DEFAULT_SCOPES.to_owned());
        let scopes = scopes.split_whitespace().collect::<Vec<_>>().join(" ");
        if !scopes.split(' ').any(|s| s == "openid") {
            return Err(ConfigError::MissingOpenidScope);
        }

        let jit_role_key = non_blank(raw.jit_role).unwrap_or_else(|| DEFAULT_JIT_ROLE.to_owned());
        if RESERVED_ROLE_KEYS.contains(&jit_role_key.as_str()) {
            return Err(ConfigError::ReservedJitRole(jit_role_key));
        }

        Ok(Some(Self {
            issuer,
            client_id,
            client_secret: ClientSecret(client_secret),
            redirect_uri,
            scopes,
            required_group,
            groups_claim: non_blank(raw.groups_claim)
                .unwrap_or_else(|| DEFAULT_GROUPS_CLAIM.to_owned()),
            accepted_acr: classes,
            accepted_amr: methods,
            jit_role_key,
            jwks_min_refresh: Duration::from_secs(
                raw.jwks_min_refresh_secs
                    .unwrap_or(DEFAULT_JWKS_MIN_REFRESH_SECS),
            ),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> RawSsoConfig {
        RawSsoConfig {
            issuer: Some("https://auth.example.test/realms/staff".to_owned()),
            client_id: Some("vsms".to_owned()),
            client_secret: Some("s3cret".to_owned()),
            redirect_uri: Some("https://vsms.example.test/login/sso/callback".to_owned()),
            required_group: Some("vsms-staff".to_owned()),
            accepted_amr: vec!["otp".to_owned()],
            ..RawSsoConfig::default()
        }
    }

    #[test]
    fn nothing_set_means_sso_is_simply_not_configured() {
        assert!(
            SsoConfig::from_raw(RawSsoConfig::default())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn blank_values_count_as_unset() {
        let raw = RawSsoConfig {
            issuer: Some("   ".to_owned()),
            client_secret: Some(String::new()),
            ..RawSsoConfig::default()
        };
        assert!(SsoConfig::from_raw(raw).unwrap().is_none());
    }

    #[test]
    fn a_complete_block_yields_the_documented_defaults() {
        let config = SsoConfig::from_raw(full()).unwrap().unwrap();
        assert_eq!(config.scopes, DEFAULT_SCOPES);
        assert_eq!(config.groups_claim, "groups");
        assert_eq!(config.jit_role_key, "support");
        assert_eq!(config.accepted_acr, Vec::<String>::new());
        assert_eq!(config.accepted_amr, vec!["otp".to_owned()]);
    }

    #[test]
    fn a_partial_block_names_what_is_missing() {
        let mut raw = full();
        raw.client_secret = None;
        raw.required_group = None;
        assert_eq!(
            SsoConfig::from_raw(raw).unwrap_err(),
            ConfigError::Incomplete {
                missing: vec!["--sso-client-secret", "--sso-required-group"]
            }
        );
    }

    #[test]
    fn both_mfa_lists_empty_refuses_to_start() {
        let mut raw = full();
        raw.accepted_amr = vec![" ".to_owned()];
        raw.accepted_acr = vec![];
        assert_eq!(
            SsoConfig::from_raw(raw).unwrap_err(),
            ConfigError::NoMfaRule
        );
    }

    #[test]
    fn an_empty_acr_list_is_fine_when_amr_is_set_and_vice_versa() {
        assert!(SsoConfig::from_raw(full()).unwrap().is_some());
        let mut raw = full();
        raw.accepted_amr = vec![];
        raw.accepted_acr = vec!["urn:mfa".to_owned()];
        assert!(SsoConfig::from_raw(raw).unwrap().is_some());
    }

    #[test]
    fn a_cleartext_remote_issuer_is_refused_but_loopback_is_allowed() {
        let mut raw = full();
        raw.issuer = Some("http://auth.example.test/realms/staff".to_owned());
        assert!(matches!(
            SsoConfig::from_raw(raw).unwrap_err(),
            ConfigError::BadUrl {
                field: "--sso-issuer",
                ..
            }
        ));
        let mut raw = full();
        raw.issuer = Some("http://localhost:8080/realms/staff".to_owned());
        assert!(SsoConfig::from_raw(raw).unwrap().is_some());
    }

    #[test]
    fn the_redirect_uri_must_be_the_callback_route() {
        let mut raw = full();
        raw.redirect_uri = Some("https://vsms.example.test/somewhere/else".to_owned());
        assert!(matches!(
            SsoConfig::from_raw(raw).unwrap_err(),
            ConfigError::BadUrl {
                field: "--sso-redirect-uri",
                ..
            }
        ));
    }

    #[test]
    fn scopes_must_include_openid() {
        let mut raw = full();
        raw.scopes = Some("email profile".to_owned());
        assert_eq!(
            SsoConfig::from_raw(raw).unwrap_err(),
            ConfigError::MissingOpenidScope
        );
    }

    #[test]
    fn the_jit_role_can_never_be_a_reserved_key() {
        for key in ["system", "app"] {
            let mut raw = full();
            raw.jit_role = Some(key.to_owned());
            assert_eq!(
                SsoConfig::from_raw(raw).unwrap_err(),
                ConfigError::ReservedJitRole(key.to_owned())
            );
        }
    }

    #[test]
    fn the_client_secret_never_prints() {
        let config = SsoConfig::from_raw(full()).unwrap().unwrap();
        assert!(!format!("{config:?}").contains("s3cret"));
    }
}
