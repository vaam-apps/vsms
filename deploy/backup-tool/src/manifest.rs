#![doc = include_str!("manifest.md")]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub taken_at: String,
    pub pg_dump_format: String,
    pub postgres_version: String,
    pub pepper_fingerprint_sha256: String,
    pub schema_migrations_applied: String,
}

/// `sha256(pepper)`, hex-encoded — never the pepper itself. Safe to leave
/// unencrypted: `HashPepper`'s own minimum (`crates/sms-api/src/pepper.rs`
/// in the main vsms repo) is 32 bytes of real entropy, nowhere near
/// brute-forceable the way a raw MSISDN hash is over Cameroon's ~10^7
/// numbering space.
pub fn pepper_fingerprint(pepper: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(pepper.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
