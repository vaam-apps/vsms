#![doc = include_str!("human_login.md")]

use serde::Deserialize;

use crate::model::Bucket;
use crate::secret::Secret;

/// The `Role.key`s `human_login.sso.jit_role` may never name: the two reserved
/// keys no human holds (`system`, `app`) and the two roles that act only from a
/// paired console key (`owner`, `admin`). A just-in-time account is created by a
/// sign-in nobody supervised, so it is never given one of them.
///
/// `sms-config` cannot import `sms_api`'s `RESERVED_ROLE_KEYS` and
/// `DEVICE_ONLY_ROLES` (it depends on neither), so this is a copy that a test in
/// `sms-auth` pins to their union.
pub const REFUSED_JIT_ROLES: [&str; 4] = ["system", "app", "owner", "admin"];

/// The shortest and longest `client_id` `OauthClient.clientId` admits
/// (`@length(min: 8, max: 64)` in `schemas/vsms.cstack`).
pub const CLIENT_ID_LENGTH: std::ops::RangeInclusive<usize> = 8..=64;

/// `human_login:` - the `human_login` method's block: the one console client a
/// person's sign-in is issued to, and the limit in front of the sign-in.
///
/// Present means the sign-in is served (when `auth.methods` permits it and `op:`
/// is written, because the sign-in rides the `OpenID` Provider); absent means it
/// is not. Every field is required: the base file states the values.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct HumanLogin {
    /// The console, the one `OpenID` client a person signs in through.
    pub console_client: ConsoleClient,
    /// The two buckets in front of `POST /login`, both taken before any password
    /// is hashed.
    pub login_rate_limit: LoginRateLimit,
    /// Single sign-on through an `OpenID` Connect identity provider (VSA-4c).
    /// Absent means the password is the only way in; present means the SSO
    /// routes are served beside it. A `null` is a shape error, never "absent".
    #[serde(default, deserialize_with = "present_sso")]
    pub sso: Option<Sso>,
}

/// Reads `sso` when the key is present, as a block and never as a null.
fn present_sso<'de, D>(deserializer: D) -> Result<Option<Sso>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Sso::deserialize(deserializer).map(Some)
}

/// `human_login.sso:` - the upstream identity provider a person may sign in
/// through instead of typing a password, and the conditions of letting them in.
///
/// Every key is required (the base file states the values). The redirect URI is
/// not a key: it is derived from `op.issuer` (`human_login.md`, "The callback is
/// derived").
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Sso {
    /// The upstream issuer URL, compared **byte for byte** with the discovery
    /// document's `issuer` and every `id_token`'s `iss`: never trimmed or
    /// case-folded.
    pub issuer: String,
    /// This gateway's confidential client id at the identity provider.
    pub client_id: String,
    /// This gateway's client secret at the identity provider
    /// (`client_secret_basic`). A secret: a `${VAR}` placeholder under
    /// `livemode`, and never printed.
    pub client_secret: Secret,
    /// The scopes requested upstream. Contains `openid`.
    pub scopes: Vec<String>,
    /// The group a person must belong to, an exact match against
    /// [`Sso::groups_claim`]. A gate, never a role.
    pub required_group: String,
    /// The `id_token` claim the group list is read from.
    pub groups_claim: String,
    /// The `amr` values that satisfy the multi-factor rule (any one suffices).
    pub accepted_amr: Vec<String>,
    /// The `acr` values that satisfy the multi-factor rule (any one suffices).
    pub accepted_acr: Vec<String>,
    /// The `Role.key` a just-in-time account is created with. Never one of
    /// [`REFUSED_JIT_ROLES`]; no token claim ever sets a role.
    pub jit_role: String,
    /// The floor, in seconds, on the gap between two fetches of the identity
    /// provider's signing keys, 1 to [`JWKS_MIN_REFRESH_SECS`]`.end()`.
    pub jwks_min_refresh_secs: u64,
    /// The bucket in front of both SSO routes, one for each.
    pub rate_limit: Bucket,
}

/// The range `human_login.sso.jwks_min_refresh_secs` may take.
pub const JWKS_MIN_REFRESH_SECS: std::ops::RangeInclusive<u64> = 1..=3600;

/// `human_login.login_rate_limit:` - the buckets in front of the password check.
///
/// There is no per-source bucket, on purpose (`human_login.md`, "No bucket per
/// source address"): behind the edge proxy and the console's own server the
/// gateway sees one peer for everybody.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct LoginRateLimit {
    /// One bucket per email address (trimmed, lowercased): what one account can
    /// be tried with.
    pub per_email: Bucket,
    /// One bucket for every sign-in together: the most Argon2 work the process
    /// can be asked for, whoever asks.
    pub aggregate: Bucket,
}

/// `human_login.console_client:` - the public client the console is registered
/// as.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ConsoleClient {
    /// The `client_id` of the console's `OauthClient` row, 8 to 64 characters
    /// ([`CLIENT_ID_LENGTH`]). `POST /login` accepts this value and no other.
    pub client_id: String,
    /// The redirect URIs the console may be sent back to, each matched exactly
    /// by the provider. At least one. `seed-console-client` registers exactly
    /// this list, and `serve` refuses to start when the row differs.
    pub redirect_uris: Vec<String>,
}
