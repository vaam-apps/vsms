#![doc = include_str!("model.md")]

use serde::Deserialize;

use crate::secret::Secret;

/// `deployment:` - who this deployment is and whether it is live.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Deployment {
    /// A label for logs and the boot line. Gates nothing.
    pub name: String,
    /// Whether this deployment sends real traffic. It is the lever for the
    /// `https`-only, no-stub-host and no-literal-secret rules; it selects no
    /// code path.
    pub livemode: bool,
    /// The origin this deployment is reached at. Validated and logged at boot.
    /// It is **not** where `op.issuer` comes from: an issuer is written, never
    /// derived (`op.md`).
    pub public_base_url: String,
}

/// `security:` - the server-held secrets that are not a provider's.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Security {
    /// The pepper behind `Message.msisdnHash` / `bodyHash`. Written as
    /// `${SMS_HASH_PEPPER}`; its length is checked by `sms_api::HashPepper`.
    pub hash_pepper: Secret,
}

/// One token bucket: how many requests it holds and how fast it refills.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Bucket {
    /// Capacity - the burst a caller can spend before throttling starts.
    pub burst: u32,
    /// Tokens added per second.
    pub refill_per_second: f64,
}

/// `api:` - the gateway's request-level policy.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Api {
    /// How long a cached `Idempotency-Key` response stays replayable.
    pub idempotency_ttl_secs: u64,
    /// The per-principal bucket.
    pub rate_limit: Bucket,
    /// The coarser, per-peer bucket outside the envelope layer.
    pub source_rate_limit: Bucket,
}

/// `database:` - pool sizes. The URL is process plumbing and stays
/// `DATABASE_URL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Database {
    /// `sms-gateway serve`'s pool.
    pub gateway_max_connections: u32,
    /// `sms-worker`'s pool.
    pub worker_max_connections: u32,
}
