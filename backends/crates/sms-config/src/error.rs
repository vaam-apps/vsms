//! Every reason a configuration is refused, one variant per rule.

use crate::auth::AuthMethod;

/// Why a configuration, a retired environment variable or a retired flag was
/// refused. Each variant is one rule with one fixture (`tests/fixtures`), and
/// each message names the YAML key the operator has to edit.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    /// No `--config` / `SMS_CONFIG` was given. There is no implicit default
    /// path: a deployment names its file, and guessing one would be exactly the
    /// silent behaviour this repository forbids.
    #[error("no config path given (pass --config or set SMS_CONFIG)")]
    MissingPath,

    /// The base file does not exist. figment alone yields an empty document for
    /// a missing file, which would turn one typo'd character into a pile of
    /// errors naming keys the operator never wrote.
    #[error("config file not found: {0}")]
    FileNotFound(String),

    /// `--profile` / `SMS_PROFILE` names a profile with no overlay file. Loading
    /// the base alone instead would boot a process on whatever the base happens
    /// to configure, and a typo (`prodd`) would look exactly like a choice. The
    /// default profile is exempt: it may run on the base file alone.
    #[error(
        "profile {profile:?} names no file: expected {expected}. Set SMS_PROFILE (or pass \
         --profile) to a profile that has an application-<profile>.yml beside the base file"
    )]
    UnknownProfile {
        /// The profile as given.
        profile: String,
        /// The overlay file that was looked for.
        expected: String,
    },

    /// [`crate::Config::security`] on a configuration loaded for a process other
    /// than the gateway (the worker, a provider-only command), which does not
    /// read the `security` block.
    #[error(
        "`security` was not loaded: this configuration was loaded by a process that does not read it"
    )]
    SecurityNotLoaded,

    /// The base file or the profile overlay exists but would not parse.
    #[error("failed to load {path}: {reason}")]
    Load {
        /// The file that failed.
        path: String,
        /// The parser's reason.
        reason: String,
    },

    /// A `${` with no closing `}`, or a name that is not `[A-Za-z0-9_]+`, in a
    /// value that is not a secret. Carries the key and the offending fragment
    /// only. A secret's own failures are [`ConfigError::SecretNotAPlaceholder`],
    /// which carries no fragment at all.
    #[error("malformed ${{VAR}} placeholder in `{key}`: {fragment}")]
    MalformedPlaceholder {
        /// The YAML key the string was written under.
        key: String,
        /// The offending `${...}` text, capped, without the string around it.
        fragment: String,
    },

    /// A `${VAR}` names a variable the environment does not set. Never an empty
    /// string: a secret that quietly resolved to nothing is the failure this
    /// exists to prevent.
    #[error("the configuration references ${{{0}}}, which the environment does not set")]
    UnresolvedPlaceholder(String),

    /// A secret that is not exactly one `${VAR}` naming an environment variable
    /// the environment sets. It holds a `${` that never closes, a name that is
    /// not `[A-Za-z0-9_]+`, a placeholder with text around it that fails to
    /// resolve (a name the environment does not set, as in `${SET}${UNSET}`), or
    /// a `${NAME}` whose `NAME` is not shaped like an environment variable
    /// (`^[A-Z_][A-Z0-9_]*$`, as in a secret pasted over the variable's name).
    /// Carries the key and nothing else: a value there is a literal secret, or
    /// has one around its placeholder, or is one written as a variable name, and
    /// an error message is a log line. Only a whole, well-shaped `${VAR}` that
    /// is unset is [`ConfigError::UnresolvedPlaceholder`], which names the
    /// variable because that name is not the secret.
    #[error(
        "`{key}` is a secret and must be exactly one `${{VAR}}` placeholder, VAR being a variable \
         name in capitals, digits and underscores; it is not, it names a variable the \
         environment does not set, or it holds a `${{` that does not close. Write the secret as \
         exactly `${{VAR}}` and put its text in the environment"
    )]
    SecretNotAPlaceholder {
        /// The YAML key the secret was written under.
        key: String,
    },

    /// The merged document is not the shape `Config` reads: an unknown key (every
    /// block refuses them, so a misspelt key cannot be dropped in silence), a
    /// missing one or a value of the wrong type.
    #[error("the configuration has the wrong shape: {0}")]
    Shape(String),

    /// A required string is blank.
    #[error("`{key}` must not be blank")]
    EmptyField {
        /// The YAML key.
        key: String,
    },

    /// A value the process cannot run with: a pool of zero connections, a
    /// non-positive rate, a URL that does not parse, a message type AWS does
    /// not know.
    #[error("`{key}` is invalid: {reason}")]
    InvalidValue {
        /// The YAML key.
        key: String,
        /// What is wrong with it.
        reason: String,
    },

    /// `deployment.livemode: true` with a URL that is not `https`.
    #[error("livemode requires https, but `{key}` is {url}")]
    InsecureHost {
        /// The YAML key.
        key: String,
        /// The URL as written.
        url: String,
    },

    /// `deployment.livemode: true` with a host that is a stub: `localhost`, a
    /// `wiremock`, one of the `sms-fake-*` containers.
    #[error("livemode must not reference a stub host, but `{key}` is {url}")]
    StubHostInLivemode {
        /// The YAML key.
        key: String,
        /// The URL as written.
        url: String,
    },

    /// `deployment.livemode: true` with a secret written as a literal. Checked
    /// against the text as written, before `${VAR}` resolves: afterwards a
    /// literal and a resolved placeholder are the same string.
    #[error(
        "livemode secrets must come from ${{VAR}} placeholders, not literals: `{key}` is written \
         inline"
    )]
    LiteralSecret {
        /// The YAML key.
        key: String,
    },

    /// `providers` names one `code` twice. A second entry could only shadow the
    /// first or be shadowed by it.
    #[error("provider {0:?} is configured twice")]
    DuplicateProvider(String),

    /// A configured provider lacks a setting its adapter cannot run without. An
    /// entry's presence is what configures the provider, so a partial one is
    /// refused rather than read as "not configured".
    #[error("provider {provider:?} is configured but `{key}` is missing or blank")]
    MissingProviderSetting {
        /// The provider `code`.
        provider: String,
        /// The key, relative to the entry.
        key: String,
    },

    /// An `aws_sms` entry with neither half of the provider: no sending
    /// credentials and no events topic. It would configure nothing.
    #[error(
        "provider \"aws_sms\" is configured but sends nothing and receives nothing: set the \
         sending keys (region, configuration_set, tps_ceiling, cost_per_segment_xaf, \
         credentials) and/or `settings.events_topic_arn`"
    )]
    ProviderConfiguresNothing,

    /// `settings.fake_sns_cert_base` under `deployment.livemode: true`. It makes
    /// the gateway trust SNS signing certificates fetched from an arbitrary
    /// host.
    #[error(
        "`providers[code=aws_sms].settings.fake_sns_cert_base` makes the gateway trust a fake \
         SNS certificate authority and is refused under livemode"
    )]
    FakeSnsCertBaseInLivemode,

    /// `auth.methods` is present and an empty list or a null (VSA-1). A
    /// deployment that permits no way of authenticating is a misconfiguration;
    /// absent is legal and permits every method. A null is how `methods:` reads
    /// once every item under it is commented out, and is refused rather than
    /// read as absent: the operator was narrowing the list, and all four is the
    /// opposite of what they meant.
    #[error(
        "auth.methods is present and empty (or null: `methods:` with every item commented \
         out); a deployment that permits no way of authenticating is a misconfiguration. Name \
         one or more of: keypair, machine_tokens, hmac_webhooks (or delete the key to permit every method)"
    )]
    NoAuthMethods,

    /// `auth.methods` names something that is not a method.
    #[error(
        "auth.methods contains an unknown value {0:?}; the legal set is: keypair, \
         machine_tokens, hmac_webhooks, human_login"
    )]
    UnknownAuthMethod(String),

    /// `auth.methods` names one method twice. Refused rather than merged: a list
    /// that repeats itself was edited by someone who meant to change it.
    #[error("auth.methods names {0} more than once; each method is listed at most once")]
    DuplicateAuthMethod(AuthMethod),

    /// `auth.methods` names a method this release has no code for
    /// (`human_login`, restored by VSA-4). Listing it would promise a door that
    /// is not there. Delete it from the list.
    #[error(
        "auth.methods names {0}, which is not available in this release; delete it from the \
         list (an absent auth.methods permits it without serving it)"
    )]
    AuthMethodUnavailable(AuthMethod),

    /// `auth.methods` names a method explicitly and the block that makes it run
    /// is missing: `machine_tokens` without `op:`. An absent `auth.methods` never
    /// raises this: it permits the method and serves it only where its block is,
    /// so a deployment that never wrote the block keeps booting.
    #[error(
        "auth.methods names {method} but {key} is not configured; add it, or delete {method} \
         from auth.methods"
    )]
    AuthMethodNotConfigured {
        /// The method named.
        method: AuthMethod,
        /// The configuration it needs.
        key: String,
    },

    /// A block is present for a method `auth.methods` does not permit: `op:` on
    /// a list without `machine_tokens`. Dormant configuration is refused so that
    /// a door an operator believes is shut cannot be one that is only unwatched.
    #[error(
        "{key} is configured but auth.methods does not permit {method}; delete {key}, or add \
         {method} to auth.methods"
    )]
    AuthMethodDisabled {
        /// The method that is not permitted.
        method: AuthMethod,
        /// The configuration that belongs to it.
        key: String,
    },

    /// `auth.methods` permits nothing that can authenticate a caller of the API,
    /// so the gateway would answer every request with the unsigned 401. That is a
    /// list with neither `keypair` nor `machine_tokens` (vpay's rule); the API
    /// accepts a machine token since VSA-3b.
    #[error(
        "auth.methods permits nothing that can authenticate a caller: hmac_webhooks signs what \
         vsms sends and human_login signs a person in to the console; neither says who an API \
         caller is. Add keypair or machine_tokens"
    )]
    NoWayIn,

    /// An environment variable that used to configure the process is set and
    /// the YAML does not read it through a `${VAR}` placeholder. clap ignores an
    /// environment variable it no longer declares, so without this refusal a
    /// deployment that still sets `ORANGE_CM_CLIENT_ID` would boot with Orange
    /// silently missing.
    #[error(
        "the environment sets {var}, which no longer configures anything. Write the value in \
         the YAML as `{key}` (reading it as ${{{var}}} is fine for a secret), or unset {var}"
    )]
    RetiredEnv {
        /// The variable.
        var: String,
        /// The YAML key that replaced it.
        key: String,
    },

    /// A command-line flag that used to configure the process. clap would say
    /// "unexpected argument"; this says where the value went.
    #[error(
        "{flag} no longer exists. Write the value in the YAML configuration as `{key}` \
         (docs/architecture.md, section 9.4)"
    )]
    RetiredFlag {
        /// The flag as given.
        flag: String,
        /// The YAML key that replaced it.
        key: String,
    },
}
