#![doc = include_str!("config.md")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Format, Yaml};
use figment::value::Value;
use serde::Deserialize;

use crate::auth::{AuthConfig, AuthMethods};
use crate::cli::DEFAULT_PROFILE;
use crate::error::ConfigError;
use crate::human_login::{
    CLIENT_ID_LENGTH, HumanLogin, JWKS_MIN_REFRESH_SECS, REFUSED_JIT_ROLES, Sso,
};
use crate::model::{Api, Bucket, Database, Deployment, Security};
use crate::op::{ACCESS_TOKEN_TTL_SECS, Op};
use crate::provider::{Providers, RawProvider, validate_providers};
use crate::retired;

/// Which process is loading the document.
///
/// They read the same file, but not the same part of it: only the gateway
/// computes a hash, so only it resolves and requires `security.*`. Handing the
/// pepper to a process that never uses it would be a least-privilege regression
/// the loader can simply decline to cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binary {
    /// `sms-gateway serve`: the whole document.
    Gateway,
    /// `sms-worker`: everything but `security`.
    Worker,
    /// A one-shot `sms-gateway` command that only calls a provider
    /// (`mtn-subscribe-dlr`): everything but `security`, as for the worker. It
    /// is a variant of its own because it is not the worker, and a command
    /// that loads as `Worker` says something false about itself.
    ProviderTool,
    /// A one-shot `sms-gateway` command that writes to the database and needs to
    /// know which ways of authenticating the deployment serves
    /// (`provision-client`, `rotate-signing-key`): everything but `security`. It
    /// never hashes a message, so it is not handed the pepper.
    Operator,
}

impl Binary {
    /// Whether this process reads `security:`. Only the gateway does.
    const fn reads_security(self) -> bool {
        matches!(self, Self::Gateway)
    }

    /// Whether this process reads `human_login.sso`, and with it the identity
    /// provider client secret. Only the gateway does: the worker never signs a
    /// person in.
    const fn reads_sso(self) -> bool {
        matches!(self, Self::Gateway)
    }
}

/// The loaded, resolved and validated configuration of one process.
///
/// `Debug` is safe to log: every secret is a [`crate::Secret`].
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// `deployment:`.
    pub deployment: Deployment,
    /// `auth.methods`, resolved: the ways of authenticating this deployment
    /// permits (every one when the key is absent).
    pub auth_methods: AuthMethods,
    /// `security:` - `Some` for [`Binary::Gateway`], `None` for
    /// [`Binary::Worker`], [`Binary::ProviderTool`] and [`Binary::Operator`],
    /// which do not read or resolve the block. Use
    /// [`Config::security`] to ask for it.
    pub security: Option<Security>,
    /// `api:`.
    pub api: Api,
    /// `database:`.
    pub database: Database,
    /// `providers:`, validated.
    pub providers: Providers,
    /// `op:`, the `OpenID` Provider (`machine_tokens`' block). `Some` only on a
    /// deployment that wrote it, and then `auth.methods` permits the method
    /// (rule 7 of `auth.md`); ask [`AuthMethods::machine_tokens_served`] rather
    /// than testing this.
    pub op: Option<Op>,
    /// `human_login:`, the console sign-in's block. `Some` only on a deployment
    /// that wrote it, and then `auth.methods` permits the method and `op:` is
    /// written (rules 6 to 9 of `auth.md`); ask
    /// [`AuthMethods::human_login_served`] rather than testing this.
    pub human_login: Option<HumanLogin>,
}

/// What serde reads, before the rules that need more than one field.
///
/// Every block refuses unknown keys, this one included, so a misspelt key is an
/// error naming it rather than a setting that is silently never applied. That is
/// also what makes `auth.methods` safe to spell wrong: `auth_methods:` or an
/// `auth:` under `api:` is an unknown key, not a narrowing that was dropped and
/// left every method permitted (vpay needs two rules of its own for that, because
/// its document ignores unknown top-level keys).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct Raw {
    deployment: Deployment,
    #[serde(default)]
    auth: AuthConfig,
    // Absent for the worker (the loader removes the block before it resolves
    // placeholders), required for the gateway (`validate` says so).
    #[serde(default)]
    security: Option<Security>,
    api: Api,
    database: Database,
    providers: Vec<RawProvider>,
    // A key that is present is a block: `op:` with every item commented out is
    // a null, and reading that as "no provider" would leave a deployment that
    // meant to serve tokens serving none.
    #[serde(default, deserialize_with = "present_op")]
    op: Option<Op>,
    // The same for `human_login:`: a null is a shape error, never "not served".
    #[serde(default, deserialize_with = "present_human_login")]
    human_login: Option<HumanLogin>,
}

/// Reads `op` when the key is present, as a block and never as a null.
fn present_op<'de, D>(deserializer: D) -> Result<Option<Op>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Op::deserialize(deserializer).map(Some)
}

/// Reads `human_login` when the key is present, as a block and never as a null.
fn present_human_login<'de, D>(deserializer: D) -> Result<Option<HumanLogin>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    HumanLogin::deserialize(deserializer).map(Some)
}

/// Hosts that mark a URL as a stub, searched in the host only: a path may say
/// `mockups`.
const STUB_MARKERS: [&str; 5] = ["wiremock", "stub", "mock", "localhost", "fake"];

/// The credential keys that hold a secret, in a provider's `credentials`.
/// Client and access-key ids are identifiers, not secrets, and may be literals.
const SECRET_CREDENTIAL_KEYS: [&str; 2] = ["client_secret", "secret_access_key"];

impl Config {
    /// Loads `path`, overlays `application-<profile>.yml` beside it, refuses
    /// retired variables, resolves `${VAR}` placeholders against the process
    /// environment and validates the result.
    ///
    /// A profile with no overlay file is an error naming the file it expected
    /// ([`ConfigError::UnknownProfile`]), except [`DEFAULT_PROFILE`], which may
    /// run on the base file alone.
    ///
    /// # Errors
    ///
    /// See [`ConfigError`]. A failure here means the caller must exit without
    /// serving: this never returns a half-valid configuration.
    pub fn load(path: Option<&Path>, profile: &str, binary: Binary) -> Result<Self, ConfigError> {
        Self::load_with_env(path, profile, binary, &|name: &str| {
            std::env::var(name).ok()
        })
    }

    /// As [`Config::load`], with the environment injected. `std::env::set_var` is
    /// `unsafe` in edition 2024 and this workspace forbids `unsafe`, so the
    /// tests pass the variables in; a `&dyn Fn` also makes the dependency
    /// visible at the call site.
    ///
    /// # Errors
    ///
    /// See [`ConfigError`].
    pub fn load_with_env(
        path: Option<&Path>,
        profile: &str,
        binary: Binary,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let base_path = path.ok_or(ConfigError::MissingPath)?;
        if !base_path.is_file() {
            return Err(ConfigError::FileNotFound(base_path.display().to_string()));
        }

        let mut figment = Figment::new().merge(Yaml::file(base_path));
        if let Some(overlay) = profile_overlay_path(base_path, profile) {
            if overlay.is_file() {
                figment = figment.merge(Yaml::file(overlay));
            } else if profile != DEFAULT_PROFILE {
                return Err(ConfigError::UnknownProfile {
                    profile: profile.to_owned(),
                    expected: overlay.display().to_string(),
                });
            }
        }
        let mut document: Value = figment.extract().map_err(|error| ConfigError::Load {
            path: base_path.display().to_string(),
            reason: error.to_string(),
        })?;

        // Everything that needs the text as written runs before step 2 below
        // resolves it: afterwards a literal and a resolved placeholder are the
        // same string, and a placeholder name is gone.
        let mut referenced = BTreeSet::new();
        collect_placeholders(&document, &mut referenced);
        retired::check_env(&referenced, env)?;
        let literal_secrets = literal_secret_keys(&document);
        let secret_keys: BTreeSet<String> = secret_entries(&document)
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        require_text_secrets(&document)?;

        // Only the gateway reads `security`. Dropping the block here, after the
        // retired-variable check has seen its placeholders and before they
        // resolve, is what lets the others start without the pepper in their
        // environment.
        if !binary.reads_security()
            && let Value::Dict(_, dict) = &mut document
        {
            dict.remove("security");
        }

        // The same for the identity provider client secret: only the gateway
        // signs a person in, so only it is handed the secret. The block is
        // dropped whole, so a worker neither resolves its placeholder nor checks
        // its shape; the gateway does both.
        if !binary.reads_sso()
            && let Some(Value::Dict(_, human_login)) = dict_entry(&mut document, "human_login")
        {
            human_login.remove("sso");
        }

        resolve_placeholders(&mut document, env, &secret_keys)?;

        // figment's `Value` and serde_yaml_ng's are different crates' types with
        // no conversion between them; `Value` is `Serialize`, so the merged
        // document goes through YAML text instead of a hand-written Deserializer.
        let text = serde_yaml_ng::to_string(&document)
            .map_err(|error| ConfigError::Shape(error.to_string()))?;
        let raw: Raw = serde_yaml_ng::from_str(&text)
            .map_err(|error| ConfigError::Shape(error.to_string()))?;

        Self::validate(raw, binary, &literal_secrets)
    }

    /// The `security:` block, for the process that loaded it.
    ///
    /// # Errors
    ///
    /// [`ConfigError::SecurityNotLoaded`] when this configuration was loaded for
    /// a [`Binary`] other than the gateway, which never reads the block.
    pub fn security(&self) -> Result<&Security, ConfigError> {
        self.security.as_ref().ok_or(ConfigError::SecurityNotLoaded)
    }

    /// The `human_login.sso:` block, for the process that loaded it.
    ///
    /// `Some` only on the gateway ([`Binary::Gateway`] is the one process that
    /// reads the block) and only when `human_login` is served; a worker, a
    /// provider tool or an operator command is never handed the secret, so it
    /// gets `None` whatever the document says.
    #[must_use]
    pub fn sso(&self) -> Option<&Sso> {
        self.human_login
            .as_ref()
            .and_then(|human_login| human_login.sso.as_ref())
            .filter(|_| self.auth_methods.human_login_served())
    }

    fn validate(raw: Raw, binary: Binary, literal_secrets: &[String]) -> Result<Self, ConfigError> {
        let Raw {
            deployment,
            auth,
            security,
            api,
            database,
            providers,
            op,
            human_login,
        } = raw;
        let livemode = deployment.livemode;

        if binary.reads_security() && security.is_none() {
            return Err(ConfigError::Shape("missing field `security`".to_owned()));
        }
        // First of the rules about the document's content: which ways of
        // authenticating are permitted is a property of the whole deployment, and
        // a list that permits none is better named than whichever rule would have
        // tripped on a later key.
        let auth_methods = auth
            .resolve()?
            .with_blocks(op.is_some(), human_login.is_some())?;
        require_text("deployment.name", &deployment.name)?;
        check_url(
            "deployment.public_base_url",
            &deployment.public_base_url,
            livemode,
        )?;

        if livemode && let Some(key) = literal_secrets.first() {
            return Err(ConfigError::LiteralSecret { key: key.clone() });
        }

        validate_bucket("api.rate_limit", api.rate_limit)?;
        validate_bucket("api.source_rate_limit", api.source_rate_limit)?;
        if api.idempotency_ttl_secs == 0 {
            return Err(invalid("api.idempotency_ttl_secs", "must be at least 1"));
        }
        for (key, connections) in [
            (
                "database.gateway_max_connections",
                database.gateway_max_connections,
            ),
            (
                "database.worker_max_connections",
                database.worker_max_connections,
            ),
        ] {
            if connections == 0 {
                return Err(invalid(key, "a pool of zero connections can run nothing"));
            }
        }

        let providers = validate_providers(providers)?;
        validate_provider_hosts(&providers, livemode)?;
        if let Some(op) = &op {
            validate_op(op, livemode)?;
        }
        if let Some(human_login) = &human_login {
            validate_human_login(human_login, livemode)?;
            if binary.reads_sso()
                && let Some(sso) = &human_login.sso
            {
                validate_sso(sso, livemode)?;
            }
        }

        Ok(Self {
            deployment,
            auth_methods,
            security,
            api,
            database,
            providers,
            op,
            human_login,
        })
    }
}

/// The rules of `human_login:` (`human_login.md`).
fn validate_human_login(human_login: &HumanLogin, livemode: bool) -> Result<(), ConfigError> {
    let client = &human_login.console_client;
    require_text("human_login.console_client.client_id", &client.client_id)?;
    if !CLIENT_ID_LENGTH.contains(&client.client_id.chars().count()) {
        return Err(invalid(
            "human_login.console_client.client_id",
            &format!(
                "must be {} to {} characters: it is the console's OauthClient.clientId",
                CLIENT_ID_LENGTH.start(),
                CLIENT_ID_LENGTH.end()
            ),
        ));
    }
    if client.client_id.chars().any(char::is_whitespace) {
        return Err(invalid(
            "human_login.console_client.client_id",
            "must not contain whitespace",
        ));
    }
    if client.redirect_uris.is_empty() {
        return Err(invalid(
            "human_login.console_client.redirect_uris",
            "at least one redirect URI is required: the provider redirects a code nowhere else",
        ));
    }
    for (index, uri) in client.redirect_uris.iter().enumerate() {
        let key = format!("human_login.console_client.redirect_uris[{index}]");
        check_url(&key, uri, livemode)?;
        let parsed = url::Url::parse(uri).map_err(|error| ConfigError::InvalidValue {
            key: key.clone(),
            reason: format!("{uri:?} is not a URL: {error}"),
        })?;
        if parsed.fragment().is_some() {
            return Err(invalid(
                &key,
                "must not carry a fragment: RFC 6749 section 3.1.2 forbids one in a redirect URI",
            ));
        }
        if uri.chars().any(char::is_whitespace) {
            return Err(invalid(
                &key,
                "must not contain whitespace: the registration stores the list space-delimited",
            ));
        }
        if client.redirect_uris[..index].contains(uri) {
            return Err(invalid(&key, "is listed twice"));
        }
    }
    let limits = &human_login.login_rate_limit;
    validate_bucket("human_login.login_rate_limit.per_email", limits.per_email)?;
    validate_bucket("human_login.login_rate_limit.aggregate", limits.aggregate)
}

/// The rules of `human_login.sso:` (`human_login.md`, "Rules of `sso`").
fn validate_sso(sso: &Sso, livemode: bool) -> Result<(), ConfigError> {
    let key = |name: &str| format!("human_login.sso.{name}");
    validate_sso_issuer(&sso.issuer, livemode)?;
    require_text(&key("client_id"), &sso.client_id)?;
    if sso.client_id.chars().any(char::is_whitespace) {
        return Err(invalid(&key("client_id"), "must not contain whitespace"));
    }
    if sso.client_secret.is_blank() {
        return Err(ConfigError::EmptyField {
            key: key("client_secret"),
        });
    }
    validate_sso_scopes(&sso.scopes)?;
    require_text(&key("required_group"), &sso.required_group)?;
    require_text(&key("groups_claim"), &sso.groups_claim)?;
    validate_sso_values(&key("accepted_amr"), &sso.accepted_amr)?;
    validate_sso_values(&key("accepted_acr"), &sso.accepted_acr)?;
    if sso.accepted_amr.is_empty() && sso.accepted_acr.is_empty() {
        return Err(invalid(
            &key("accepted_amr"),
            "the multi-factor rule is empty: write at least one amr value or one acr value, \
             because an empty rule would read as no rule",
        ));
    }
    validate_jit_role(&sso.jit_role)?;
    if !JWKS_MIN_REFRESH_SECS.contains(&sso.jwks_min_refresh_secs) {
        return Err(invalid(
            &key("jwks_min_refresh_secs"),
            &format!(
                "must be {} to {} seconds",
                JWKS_MIN_REFRESH_SECS.start(),
                JWKS_MIN_REFRESH_SECS.end()
            ),
        ));
    }
    validate_bucket("human_login.sso.rate_limit", sso.rate_limit)
}

/// The issuer is compared byte for byte, so it is checked as written: a URL with
/// a host and nothing else on it, `https` in every mode except plain `http` to a
/// loopback host (a developer's Keycloak, the tests' fake), and under `livemode`
/// the same real-host rule as every other URL.
/// `FederatedLink.issuer`'s `@length(max: ...)` in `schemas/vsms.cstack`.
const ISSUER_MAX_CHARS: usize = 512;

fn validate_sso_issuer(issuer: &str, livemode: bool) -> Result<(), ConfigError> {
    const KEY: &str = "human_login.sso.issuer";
    // `FederatedLink.issuer` is `@length(max: 512)`: a longer issuer would pass every
    // other rule and then fail the first account creation as a validation error.
    if issuer.chars().count() > ISSUER_MAX_CHARS {
        return Err(invalid(
            KEY,
            &format!(
                "must be at most {ISSUER_MAX_CHARS} characters: it is stored in FederatedLink.issuer"
            ),
        ));
    }
    let parsed = url::Url::parse(issuer).map_err(|error| ConfigError::InvalidValue {
        key: KEY.to_owned(),
        reason: format!("{issuer:?} is not a URL: {error}"),
    })?;
    let loopback = match parsed.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    match parsed.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => {
            return Err(invalid(
                KEY,
                "must be https (plain http only to a loopback host): the client secret and the \
                 authorization code travel over it",
            ));
        }
    }
    if parsed.host().is_none() {
        return Err(invalid(KEY, "has no host"));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(invalid(KEY, "must not carry a query or a fragment"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid(KEY, "must not carry credentials"));
    }
    if livemode {
        check_url(KEY, issuer, true)?;
    }
    Ok(())
}

fn validate_sso_scopes(scopes: &[String]) -> Result<(), ConfigError> {
    const KEY: &str = "human_login.sso.scopes";
    if scopes.is_empty() {
        return Err(invalid(KEY, "at least one scope is required"));
    }
    for (index, scope) in scopes.iter().enumerate() {
        if scope.trim().is_empty() || scope.chars().any(char::is_whitespace) {
            return Err(invalid(
                &format!("{KEY}[{index}]"),
                "must be one scope with no whitespace in it: the request joins the list with spaces",
            ));
        }
        if scopes[..index].contains(scope) {
            return Err(invalid(&format!("{KEY}[{index}]"), "is listed twice"));
        }
    }
    if !scopes.iter().any(|scope| scope == "openid") {
        return Err(invalid(
            KEY,
            "must include `openid`: without it there is no id_token",
        ));
    }
    Ok(())
}

/// An `amr` or `acr` list: each entry non-blank, trimmed, listed once.
fn validate_sso_values(key: &str, values: &[String]) -> Result<(), ConfigError> {
    for (index, value) in values.iter().enumerate() {
        if value.trim().is_empty() || value.trim() != value {
            return Err(invalid(
                &format!("{key}[{index}]"),
                "must be non-blank with no leading or trailing whitespace: it is compared exactly",
            ));
        }
        if values[..index].contains(value) {
            return Err(invalid(&format!("{key}[{index}]"), "is listed twice"));
        }
    }
    Ok(())
}

/// `Role.key`'s own shape (`^[a-z][a-z0-9_]{2,31}$`) and not a refused role.
fn validate_jit_role(role: &str) -> Result<(), ConfigError> {
    const KEY: &str = "human_login.sso.jit_role";
    let mut chars = role.chars();
    let shaped = (3..=32).contains(&role.chars().count())
        && chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !shaped {
        return Err(invalid(
            KEY,
            "must be a Role.key: 3 to 32 characters of a-z, 0-9 and _, starting with a letter",
        ));
    }
    if REFUSED_JIT_ROLES.contains(&role) {
        return Err(invalid(
            KEY,
            &format!(
                "{role:?} can never be a just-in-time role: an account created by an \
                 unsupervised sign-in is never given {}",
                REFUSED_JIT_ROLES.join(", ")
            ),
        ));
    }
    Ok(())
}

/// The rules of `op:` (`op.md`).
fn validate_op(op: &Op, livemode: bool) -> Result<(), ConfigError> {
    check_url("op.issuer", &op.issuer, livemode)?;
    if op.issuer.ends_with('/') {
        return Err(invalid(
            "op.issuer",
            "must not end with a slash: the discovery document appends /token and /jwks.json to it",
        ));
    }
    validate_bucket("op.token_rate_limit", op.token_rate_limit)?;
    if op.signing_key.refresh_interval_secs == 0 {
        return Err(invalid(
            "op.signing_key.refresh_interval_secs",
            "must be at least 1",
        ));
    }
    // A running `serve` signs with the old key until its next refresh, so the
    // last token under it is issued as late as T + refresh_interval_secs and
    // lives until T + refresh_interval_secs + ACCESS_TOKEN_TTL_SECS. The old key
    // stops being published at T + rotation_overlap_secs.
    let latest_old_key_token_expiry = op
        .signing_key
        .refresh_interval_secs
        .saturating_add(ACCESS_TOKEN_TTL_SECS);
    if op.signing_key.rotation_overlap_secs <= latest_old_key_token_expiry {
        return Err(invalid(
            "op.signing_key.rotation_overlap_secs",
            &format!(
                "must exceed refresh_interval_secs ({}) plus the {ACCESS_TOKEN_TTL_SECS}-second                  access token lifetime ({latest_old_key_token_expiry}): a running gateway keeps                  signing with the old key until its next refresh, so the last token under it                  would outlive the key's place in /jwks.json",
                op.signing_key.refresh_interval_secs
            ),
        ));
    }
    Ok(())
}

fn invalid(key: &str, reason: &str) -> ConfigError {
    ConfigError::InvalidValue {
        key: key.to_owned(),
        reason: reason.to_owned(),
    }
}

fn require_text(key: &str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        return Err(ConfigError::EmptyField {
            key: key.to_owned(),
        });
    }
    Ok(())
}

fn validate_bucket(key: &str, bucket: Bucket) -> Result<(), ConfigError> {
    if bucket.burst == 0 {
        return Err(invalid(&format!("{key}.burst"), "must be at least 1"));
    }
    if !bucket.refill_per_second.is_finite() || bucket.refill_per_second <= 0.0 {
        return Err(invalid(
            &format!("{key}.refill_per_second"),
            "must be a positive number",
        ));
    }
    Ok(())
}

/// A URL must parse, and under livemode must be `https` with a real host.
fn check_url(key: &str, raw: &str, livemode: bool) -> Result<(), ConfigError> {
    let parsed = url::Url::parse(raw).map_err(|error| ConfigError::InvalidValue {
        key: key.to_owned(),
        reason: format!("{raw:?} is not a URL: {error}"),
    })?;
    if !livemode {
        return Ok(());
    }
    if parsed.scheme() != "https" {
        return Err(ConfigError::InsecureHost {
            key: key.to_owned(),
            url: raw.to_owned(),
        });
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if STUB_MARKERS.iter().any(|marker| host.contains(marker)) {
        return Err(ConfigError::StubHostInLivemode {
            key: key.to_owned(),
            url: raw.to_owned(),
        });
    }
    Ok(())
}

fn validate_provider_hosts(providers: &Providers, livemode: bool) -> Result<(), ConfigError> {
    if let Some(orange) = &providers.orange_cm {
        check_url(
            "providers[code=orange_cm].host.url",
            &orange.base_url,
            livemode,
        )?;
    }
    if let Some(mtn) = &providers.mtn_cm {
        check_url("providers[code=mtn_cm].host.url", &mtn.base_url, livemode)?;
    }
    if let Some(aws) = &providers.aws_sms {
        for (key, url) in aws.hosts() {
            check_url(&key, url, livemode)?;
        }
        if livemode && aws.fake_sns_cert_base.is_some() {
            return Err(ConfigError::FakeSnsCertBaseInLivemode);
        }
    }
    Ok(())
}

/// `<stem>-<profile>.<ext>` beside the base file: `application.yml` and
/// `dev` give `application-dev.yml`.
fn profile_overlay_path(base_path: &Path, profile: &str) -> Option<PathBuf> {
    let parent = base_path.parent().unwrap_or_else(|| Path::new(""));
    let stem = base_path.file_stem()?.to_str()?;
    let name = match base_path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => format!("{stem}-{profile}.{ext}"),
        None => format!("{stem}-{profile}"),
    };
    Some(parent.join(name))
}

// ---------------------------------------------------------------------------
// Placeholders
// ---------------------------------------------------------------------------

/// Every `${NAME}` the document writes, in any string leaf.
fn collect_placeholders(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::String(_, text) => scan_placeholders(text, out),
        Value::Array(_, items) => items
            .iter()
            .for_each(|item| collect_placeholders(item, out)),
        Value::Dict(_, dict) => dict
            .values()
            .for_each(|child| collect_placeholders(child, out)),
        Value::Char(..) | Value::Bool(..) | Value::Num(..) | Value::Empty(..) => {}
    }
}

fn scan_placeholders(text: &str, out: &mut BTreeSet<String>) {
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return;
        };
        out.insert(after[..end].to_owned());
        rest = &after[end + 1..];
    }
}

/// The `NAME` of a `text` that is exactly one `${NAME}` and nothing else, `NAME`
/// being any run of `[A-Za-z0-9_]`.
fn whole_placeholder_name(text: &str) -> Option<&str> {
    text.strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
        .filter(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Whether `name` has the shape of an environment variable that a deployment
/// would set: `^[A-Z_][A-Z0-9_]*$`.
///
/// A secret's `${NAME}` must have it. `${s3cr3tAlnumValue9}` is a valid
/// placeholder to the resolver, but nobody names a variable that way: it is a
/// secret pasted over the variable name, and an error that said "references
/// `${s3cr3tAlnumValue9}`" would print it.
fn is_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_uppercase() || first == '_')
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Whether `text` is exactly one `${NAME}` with an environment-variable `NAME`:
/// the only form in which a secret is not a literal.
fn is_whole_secret_placeholder(text: &str) -> bool {
    whole_placeholder_name(text).is_some_and(is_env_var_name)
}

/// Where a key path stands against the secret keys.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SecretPlace {
    /// Not a secret and not inside one.
    Outside,
    /// The secret's own key: a string leaf, the only shape a secret may take.
    Exact,
    /// Under a secret that is an array or a map (`...client_secret[0]`,
    /// `...client_secret.x`). The secret is not a string there, so nothing in it
    /// may be repeated either.
    Inside,
}

fn secret_place(key: &str, secret_keys: &BTreeSet<String>) -> SecretPlace {
    if secret_keys.contains(key) {
        return SecretPlace::Exact;
    }
    let inside = secret_keys.iter().any(|secret| {
        key.strip_prefix(secret.as_str())
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
    });
    if inside {
        SecretPlace::Inside
    } else {
        SecretPlace::Outside
    }
}

/// How a `providers:` entry is named in an error: `providers[code=orange_cm]`,
/// or `providers[code=?]` for an entry with no `code`. Every key path the loader
/// reports for a provider is built by this one function, so the loader's errors
/// cannot name the same place two ways.
fn provider_label(entry: &figment::value::Dict) -> String {
    let code = entry.get("code").and_then(Value::as_str).unwrap_or("?");
    format!("providers[code={code}]")
}

/// Every secret the document writes, with the key it is written under:
/// `security.hash_pepper`, and each provider's `credentials.client_secret` /
/// `credentials.secret_access_key`.
fn secret_entries(document: &Value) -> Vec<(String, &Value)> {
    let mut found = Vec::new();
    if let Some(pepper) = document.find_ref("security.hash_pepper") {
        found.push(("security.hash_pepper".to_owned(), pepper));
    }
    if let Some(secret) = document.find_ref("human_login.sso.client_secret") {
        found.push(("human_login.sso.client_secret".to_owned(), secret));
    }
    let providers = document
        .find_ref("providers")
        .and_then(Value::as_array)
        .unwrap_or_default();
    for provider in providers {
        let Some(entry) = provider.as_dict() else {
            continue;
        };
        let label = provider_label(entry);
        let Some(credentials) = entry.get("credentials").and_then(Value::as_dict) else {
            continue;
        };
        for key in SECRET_CREDENTIAL_KEYS {
            if let Some(value) = credentials.get(key) {
                found.push((format!("{label}.credentials.{key}"), value));
            }
        }
    }
    found
}

/// The keys of secrets written as a literal.
fn literal_secret_keys(document: &Value) -> Vec<String> {
    secret_entries(document)
        .into_iter()
        .filter(|(_, value)| {
            value
                .as_str()
                .is_none_or(|text| !is_whole_secret_placeholder(text))
        })
        .map(|(key, _)| key)
        .collect()
}

/// A secret that YAML reads as a number or a boolean (`client_secret: 123456`)
/// is refused here, naming the key and nothing else.
///
/// Left to serde, the same value would come back as `invalid type: integer
/// `123456`, expected a string`, which prints the secret. Converting it to text
/// instead would be worse: figment has already normalised the number (`0123`
/// is no longer what the operator wrote), so the process would run on a secret
/// nobody typed.
fn require_text_secrets(document: &Value) -> Result<(), ConfigError> {
    for (key, value) in secret_entries(document) {
        if matches!(value, Value::Num(..) | Value::Bool(..)) {
            return Err(invalid(
                &key,
                "a secret must be a quoted YAML string (YAML read it as a number or a boolean)",
            ));
        }
    }
    Ok(())
}

/// The child `name` of a document that is a dict, mutably.
fn dict_entry<'a>(document: &'a mut Value, name: &str) -> Option<&'a mut Value> {
    match document {
        Value::Dict(_, dict) => dict.get_mut(name),
        _ => None,
    }
}

/// Replaces every `${VAR}` in a string leaf with what `env` returns.
///
/// `secret_keys` are the key paths of the secrets, known from the text as
/// written. A value under one of them that fails to resolve is reported by key
/// alone ([`ConfigError::SecretNotAPlaceholder`]): the value is either a literal
/// secret or one with a placeholder inside it, and an error is a log line.
fn resolve_placeholders(
    value: &mut Value,
    env: &dyn Fn(&str) -> Option<String>,
    secret_keys: &BTreeSet<String>,
) -> Result<(), ConfigError> {
    resolve_at(value, &mut String::new(), env, secret_keys)
}

/// How much of a malformed placeholder's name an error may repeat. Only a value
/// outside the secret keys reaches this, so the cap is a courtesy to the log
/// line and not the protection: for a secret the fragment is not shown at all.
const FRAGMENT_LIMIT: usize = 32;

fn resolve_at(
    value: &mut Value,
    path: &mut String,
    env: &dyn Fn(&str) -> Option<String>,
    secret_keys: &BTreeSet<String>,
) -> Result<(), ConfigError> {
    let outer = path.len();
    match value {
        Value::String(_, text) => *text = resolve_string(path, text, env, secret_keys)?,
        Value::Array(_, items) => {
            for (index, item) in items.iter_mut().enumerate() {
                match item.as_dict() {
                    Some(entry) if path == "providers" => path.push_str(
                        provider_label(entry)
                            .strip_prefix("providers")
                            .unwrap_or_default(),
                    ),
                    _ => path.push_str(&["[", &index.to_string(), "]"].concat()),
                }
                resolve_at(item, path, env, secret_keys)?;
                path.truncate(outer);
            }
        }
        Value::Dict(_, dict) => {
            for (name, child) in dict.iter_mut() {
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(name);
                resolve_at(child, path, env, secret_keys)?;
                path.truncate(outer);
            }
        }
        Value::Char(..) | Value::Bool(..) | Value::Num(..) | Value::Empty(..) => {}
    }
    Ok(())
}

/// Resolves one string leaf, `key` being where it was written.
///
/// Under a secret key, anything that is not exactly one `${VAR}` is a literal
/// (a sandbox's) or has text around its placeholder, and either way the string
/// is the secret: a failure there names the key and nothing else, neither the
/// fragment nor the variable it asked for. So is a `${NAME}` whose `NAME` is not
/// shaped like an environment variable (`^[A-Z_][A-Z0-9_]*$`), whole or not: a
/// secret pasted over the variable's name. A whole `${VAR}` of that shape that
/// is unset names `VAR`, which is a variable name and not the secret. A secret
/// written as an array or a map is hidden at every depth. Outside the secret
/// keys a malformed placeholder is reported by key and a capped fragment.
fn resolve_string(
    key: &str,
    text: &str,
    env: &dyn Fn(&str) -> Option<String>,
    secret_keys: &BTreeSet<String>,
) -> Result<String, ConfigError> {
    let hidden = match secret_place(key, secret_keys) {
        SecretPlace::Outside => false,
        SecretPlace::Exact => !is_whole_secret_placeholder(text),
        SecretPlace::Inside => true,
    };
    let secret_not_a_placeholder = || ConfigError::SecretNotAPlaceholder {
        key: key.to_owned(),
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            if hidden {
                return Err(secret_not_a_placeholder());
            }
            return Err(ConfigError::MalformedPlaceholder {
                key: key.to_owned(),
                fragment: "${ with no closing }".to_owned(),
            });
        };
        let name = &after[..end];
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            if hidden {
                return Err(secret_not_a_placeholder());
            }
            let shown: String = name.chars().take(FRAGMENT_LIMIT).collect();
            let ellipsis = if name.chars().count() > FRAGMENT_LIMIT {
                "..."
            } else {
                ""
            };
            return Err(ConfigError::MalformedPlaceholder {
                key: key.to_owned(),
                fragment: format!("${{{shown}{ellipsis}}}"),
            });
        }
        if hidden && !is_env_var_name(name) {
            // Whatever the environment holds under this name, the name is not
            // one: it is the secret, written where a variable's name goes.
            return Err(secret_not_a_placeholder());
        }
        let resolved = env(name).ok_or_else(|| {
            if hidden {
                secret_not_a_placeholder()
            } else {
                ConfigError::UnresolvedPlaceholder(name.to_owned())
            }
        })?;
        out.push_str(&resolved);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}
