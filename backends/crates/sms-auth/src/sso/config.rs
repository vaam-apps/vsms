//! SSO configuration: what `human_login.sso` and `op.issuer` say, as the rest
//! of the broker uses it.
//!
//! **There is nothing to validate here.** The operator's input is
//! `sms_config::Sso`, which the loader has already refused to hand over unless
//! every rule holds (`sms-config`'s `human_login.md`, "Rules of `sso`"): a
//! complete block, a multi-factor rule, a secure issuer, a JIT role that is not
//! reserved. This module only derives what that block does not state - the
//! callback URL and the space-joined scope string - and is the type the
//! provider, the policy and the store take, so none of them depends on how the
//! operator wrote it.

use std::time::Duration;

use sms_config::{Secret, Sso};
use thiserror::Error;
use url::{Host, Url};

/// Where the gateway mounts the browser's entry point.
pub const START_PATH: &str = "/login/sso/start";
/// Where the gateway mounts the upstream `IdP`'s return leg. The redirect URI
/// registered at the identity provider is `op.issuer` followed by exactly this.
pub const CALLBACK_PATH: &str = "/login/sso/callback";

/// A validated SSO configuration. One issuer per deployment.
#[derive(Debug, Clone)]
pub struct SsoConfig {
    /// The upstream issuer, **verbatim** - the discovery document's `issuer`
    /// and every `id_token`'s `iss` are compared to this byte for byte, so it
    /// is never normalised (no trailing-slash trimming, no case folding).
    pub issuer: String,
    /// This gateway's client id at the upstream `IdP`.
    pub client_id: String,
    /// This gateway's client secret at the upstream `IdP`. A
    /// [`Secret`]: `Debug` prints a placeholder, so a stray `{:?}` of the config
    /// in a log line cannot leak it.
    pub client_secret: Secret,
    /// The gateway's public callback: `op.issuer` + [`CALLBACK_PATH`].
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
    /// Whether the deployment is `livemode`: every endpoint the identity
    /// provider's discovery document names must then be `https`, not only the
    /// issuer.
    pub livemode: bool,
}

/// Why a configuration the loader accepted cannot be used. Reachable only for
/// a value a hand-built [`Sso`] (a test) carries, never from a loaded document:
/// the loader's rules are a superset of these.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    /// A URL setting did not parse, or carries a part it must not.
    #[error("{field} is not acceptable: {reason}")]
    BadUrl {
        /// Which key.
        field: &'static str,
        /// What is wrong with it.
        reason: &'static str,
    },
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
    /// The broker's configuration from the loaded block and the issuer the
    /// callback is derived from (`op.issuer`).
    ///
    /// # Errors
    ///
    /// [`ConfigError::BadUrl`] if `op_issuer` does not parse: it cannot, for a
    /// loaded document.
    pub fn new(sso: &Sso, op_issuer: &str, livemode: bool) -> Result<Self, ConfigError> {
        let base = require_secure_url("op.issuer", op_issuer)?;
        Ok(Self {
            issuer: sso.issuer.clone(),
            client_id: sso.client_id.clone(),
            client_secret: sso.client_secret.clone(),
            redirect_uri: format!("{}{CALLBACK_PATH}", base.as_str().trim_end_matches('/')),
            scopes: sso.scopes.join(" "),
            required_group: sso.required_group.clone(),
            groups_claim: sso.groups_claim.clone(),
            accepted_acr: sso.accepted_acr.clone(),
            accepted_amr: sso.accepted_amr.clone(),
            jit_role_key: sso.jit_role.clone(),
            jwks_min_refresh: Duration::from_secs(sso.jwks_min_refresh_secs),
            livemode,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A block as `sms-config` would hand it over.
    /// A client secret drawn at run time (a literal that reaches a secret sink is
    /// what `CodeQL` flags, even in a test).
    pub(crate) fn secret() -> String {
        sms_core::password::generate_password(24)
    }

    pub(crate) fn sso(overrides: &serde_json::Value) -> Sso {
        let mut block = serde_json::json!({
            "issuer": "https://auth.example.test/realms/staff",
            "client_id": "vsms",
            "client_secret": secret(),
            "scopes": ["openid", "email", "profile"],
            "required_group": "vsms-staff",
            "groups_claim": "groups",
            "accepted_amr": ["otp"],
            "accepted_acr": [],
            "jit_role": "support",
            "jwks_min_refresh_secs": 30,
            "rate_limit": { "burst": 30, "refill_per_second": 1.0 },
        });
        if let (Some(base), Some(extra)) = (block.as_object_mut(), overrides.as_object()) {
            base.extend(extra.clone());
        }
        serde_json::from_value(block).expect("a complete block")
    }

    #[test]
    fn the_callback_is_the_op_issuer_plus_the_callback_route() {
        let config = SsoConfig::new(
            &sso(&serde_json::json!({})),
            "https://vsms.example.test",
            false,
        )
        .unwrap();
        assert_eq!(
            config.redirect_uri,
            "https://vsms.example.test/login/sso/callback"
        );
        // A path on the issuer is kept, and a trailing slash never doubles.
        let config = SsoConfig::new(
            &sso(&serde_json::json!({})),
            "https://vsms.example.test/gw/",
            false,
        )
        .unwrap();
        assert_eq!(
            config.redirect_uri,
            "https://vsms.example.test/gw/login/sso/callback"
        );
    }

    #[test]
    fn the_scopes_are_joined_and_the_issuer_is_kept_verbatim() {
        let config = SsoConfig::new(
            &sso(&serde_json::json!({ "issuer": "https://auth.example.test/realms/staff/" })),
            "https://vsms.example.test",
            false,
        )
        .unwrap();
        assert_eq!(config.scopes, "openid email profile");
        assert_eq!(config.issuer, "https://auth.example.test/realms/staff/");
        assert_eq!(config.jwks_min_refresh, Duration::from_secs(30));
    }

    #[test]
    fn the_client_secret_never_prints() {
        let secret = secret();
        let config = SsoConfig::new(
            &sso(&serde_json::json!({ "client_secret": secret })),
            "https://vsms.example.test",
            false,
        )
        .unwrap();
        assert_eq!(
            config.client_secret.expose(),
            secret,
            "the secret is carried"
        );
        assert!(!format!("{config:?}").contains(&secret));
    }

    #[test]
    fn a_cleartext_remote_url_is_refused_but_loopback_is_allowed() {
        assert!(matches!(
            require_secure_url("x", "http://auth.example.test/realms/staff"),
            Err(ConfigError::BadUrl { .. })
        ));
        assert!(require_secure_url("x", "http://localhost:8080/realms/staff").is_ok());
        assert!(require_secure_url("x", "http://127.0.0.1:8080/realms/staff").is_ok());
    }

    #[test]
    fn a_url_with_a_fragment_or_credentials_is_refused() {
        for bad in [
            "https://a.example.test/x#frag",
            "https://user:pw@a.example.test/x",
        ] {
            assert!(require_secure_url("x", bad).is_err(), "{bad}");
        }
    }
}
