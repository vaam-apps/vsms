#![doc = include_str!("op.md")]

use serde::Deserialize;

use crate::model::Bucket;

/// How long an access token the `OpenID` Provider issues is valid, in seconds.
/// Fifteen minutes. It is a constant and not a YAML key: `client_credentials`
/// has no refresh, so a caller that wants a longer session asks again, and
/// [`SigningKey::rotation_overlap_secs`] is held above it (plus the refresh
/// interval).
pub const ACCESS_TOKEN_TTL_SECS: u64 = 900;

/// `op:` - the deployment's own `OpenID` Provider, the `machine_tokens` method's
/// block.
///
/// Present means the provider is mounted (when `auth.methods` permits it); absent
/// means it is not. Every field is required: the base file states the values.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Op {
    /// The issuer URL: the `iss` of every token this deployment signs and the
    /// prefix of the endpoints its discovery document names. The origin clients
    /// reach the gateway at, with no trailing slash (the discovery document
    /// appends `/token` and `/jwks.json` to it).
    pub issuer: String,
    /// The per-`client_id` bucket in front of `/token`. The edge cannot key on a
    /// form field, so this is the one limit that can.
    pub token_rate_limit: Bucket,
    /// How the OP's signing keys rotate.
    pub signing_key: SigningKey,
}

/// `op.signing_key:` - the timing of the RS256 keys the provider signs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SigningKey {
    /// How often a running `serve` reloads the keys from the database, so a
    /// `rotate-signing-key` run against it takes effect without a restart.
    pub refresh_interval_secs: u64,
    /// How long a rotated-out key keeps publishing in the JWKS after it stops
    /// signing. It must exceed [`SigningKey::refresh_interval_secs`] plus
    /// [`ACCESS_TOKEN_TTL_SECS`]: a running `serve` signs with the old key until
    /// its next refresh, so the last old-key token is issued that late and lives
    /// that much longer, and must still find its key in the JWKS.
    pub rotation_overlap_secs: u64,
}
