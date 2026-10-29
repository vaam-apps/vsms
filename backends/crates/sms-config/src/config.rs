#![doc = include_str!("config.md")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Format, Yaml};
use figment::value::Value;
use serde::Deserialize;

use crate::cli::DEFAULT_PROFILE;
use crate::error::ConfigError;
use crate::model::{Api, Bucket, Database, Deployment, Security};
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
}

impl Binary {
    /// Whether this process reads `security:`. Only the gateway does.
    const fn reads_security(self) -> bool {
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
    /// `security:` - `Some` for [`Binary::Gateway`], `None` for
    /// [`Binary::Worker`] and [`Binary::ProviderTool`], which do not read or
    /// resolve the block. Use
    /// [`Config::security`] to ask for it.
    pub security: Option<Security>,
    /// `api:`.
    pub api: Api,
    /// `database:`.
    pub database: Database,
    /// `providers:`, validated.
    pub providers: Providers,
}

/// What serde reads, before the rules that need more than one field.
///
/// Every block refuses unknown keys, this one included, so a misspelt key is an
/// error naming it rather than a setting that is silently never applied. The
/// auth methods (VSA-1) will be one more field here, between `deployment` and
/// `security`; until it exists an `auth:` key is refused by this very rule.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct Raw {
    deployment: Deployment,
    // Absent for the worker (the loader removes the block before it resolves
    // placeholders), required for the gateway (`validate` says so).
    #[serde(default)]
    security: Option<Security>,
    api: Api,
    database: Database,
    providers: Vec<RawProvider>,
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

    fn validate(raw: Raw, binary: Binary, literal_secrets: &[String]) -> Result<Self, ConfigError> {
        let Raw {
            deployment,
            security,
            api,
            database,
            providers,
        } = raw;
        let livemode = deployment.livemode;

        if binary.reads_security() && security.is_none() {
            return Err(ConfigError::Shape("missing field `security`".to_owned()));
        }
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

        Ok(Self {
            deployment,
            security,
            api,
            database,
            providers,
        })
    }
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

/// Whether `text` is exactly one `${NAME}` and nothing else.
fn is_whole_placeholder(text: &str) -> bool {
    text.strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
        .is_some_and(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
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
                .is_none_or(|text| !is_whole_placeholder(text))
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
/// fragment nor the variable it asked for. A whole `${VAR}` that is unset names
/// `VAR`, which is a variable name and not the secret. Outside the secret keys a
/// malformed placeholder is reported by key and a capped fragment.
fn resolve_string(
    key: &str,
    text: &str,
    env: &dyn Fn(&str) -> Option<String>,
    secret_keys: &BTreeSet<String>,
) -> Result<String, ConfigError> {
    let hidden = secret_keys.contains(key) && !is_whole_placeholder(text);
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
