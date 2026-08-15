Thin wrappers over the real `rclone` binary — the object-storage layer
stays exactly what `docs/runbooks/backup-restore.adoc` chose it for
(provider-agnostic: S3, B2, GCS, Azure Blob, MinIO, or a bare local
path all work behind the same three calls below), unreimplemented,
unreplaced. This module is process orchestration, not a client library
— the same shape `.xtask/src/migrations_current.rs` already uses to
shell out to the real `cratestack` CLI in the main workspace.
