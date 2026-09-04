#![doc = include_str!("manifest.md")]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `rename_all = "snake_case"` is a no-op here — every field is already a
/// snake_case Rust identifier, so serde would emit the identical JSON keys
/// without it. Stated explicitly anyway, matching this workspace's
/// convention for shapes it owns: it documents the wire format as a
/// decision rather than an accident, and it means a future field can't
/// silently drift into `camelCase` (serde's other common default) without
/// the attribute visibly contradicting it. Never remove or change this
/// without checking `manifest.md`'s own note first — a real deployment may
/// have backups already sitting in a bucket that were written under the
/// bash-era `backup.sh` field names this struct still has to read back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[cfg(test)]
mod tests {
    use super::Manifest;

    /// Pins the exact wire shape `manifest.md` promises: field names
    /// unchanged from the bash-era manifest, so an existing backup bucket's
    /// `manifest.json` still deserializes. `rename_all = "snake_case"` is a
    /// no-op today (every field is already snake_case) — this test is what
    /// would catch a *future* field ever silently drifting from that.
    #[test]
    fn the_manifest_serializes_with_unchanged_snake_case_keys() {
        let manifest = Manifest {
            taken_at: "2026-01-01T00:00:00Z".to_owned(),
            pg_dump_format: "custom".to_owned(),
            postgres_version: "16.4".to_owned(),
            pepper_fingerprint_sha256: "deadbeef".to_owned(),
            schema_migrations_applied: "0001_init,0002_bootstrap".to_owned(),
        };
        let json: serde_json::Value = serde_json::to_value(&manifest).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "taken_at": "2026-01-01T00:00:00Z",
                "pg_dump_format": "custom",
                "postgres_version": "16.4",
                "pepper_fingerprint_sha256": "deadbeef",
                "schema_migrations_applied": "0001_init,0002_bootstrap",
            })
        );
    }
}
