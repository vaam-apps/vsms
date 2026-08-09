#!/usr/bin/env bash
# The one thing this deployment's data-loss story hangs on (#69). Takes a
# single pg_dump of $DATABASE_URL in custom format (pg_restore's own
# format — built-in compression, selective/parallel restore; a plain SQL
# dump gets none of that), writes a small unencrypted manifest next to it,
# and pushes both to $BACKUP_RCLONE_REMOTE via `rclone`.
#
# Why pg_dump, not WAL archiving/PITR: see docs/runbooks/backup-restore.md
# ("Decision: pg_dump, not WAL archiving"). Short version — this is a
# single-VM, pre-production deployment with no live traffic yet (AGENTS.md
# records that as a standing fact, not a guess). WAL archiving buys
# point-in-time recovery at the cost of a second always-on pipeline
# (archive_command, a WAL-shipping agent, a restore procedure that replays
# from a base backup forward) protecting nothing that exists yet. A
# once-daily pg_dump plus a *proven* restore path (restore-drill.sh) is
# the honest tool for this deployment's actual size; the upgrade path to
# WAL/PITR is documented, not silently foreclosed.
#
# Why the manifest: a bare pg_dump says nothing about which pepper
# (crates/sms-api/src/pepper.rs) hashed the msisdnHash/bodyHash columns
# inside it. Storing the pepper itself next to the dump would turn a
# backup leak into a full de-anonymisation of every row it covers, so
# this stores only a SHA-256 *fingerprint* of the pepper — safe to keep
# unencrypted because HashPepper's own minimum is 32 bytes of
# `openssl rand` output, so the fingerprint can't be brute-forced the way
# a raw MSISDN hash can (AGENTS.md's #134 section: ~10^7 candidates for
# an MSISDN, nothing comparable for 256 real bits of entropy). A restore
# operator hashes their own candidate pepper the same way and compares —
# see restore.sh.
set -euo pipefail

: "${DATABASE_URL:?set DATABASE_URL, e.g. postgres://vsms:...@postgres:5432/vsms}"
: "${SMS_HASH_PEPPER:?set SMS_HASH_PEPPER to the same value sms-gateway is running with right now — see the header comment above}"
: "${BACKUP_RCLONE_REMOTE:?set BACKUP_RCLONE_REMOTE, e.g. s3:my-bucket/vsms-backups, or a bare local path for a drill}"
: "${BACKUP_RETENTION_DAYS:=30}"

ts=$(date -u +%Y%m%dT%H%M%SZ)
workdir=$(mktemp -d)
trap 'rm -rf "${workdir}"' EXIT

dump_name="vsms-${ts}.dump"
manifest_name="vsms-${ts}.manifest.json"
dump_path="${workdir}/${dump_name}"
manifest_path="${workdir}/${manifest_name}"

echo "vsms-backup: dumping postgres://***@${DATABASE_URL#*@} -> ${dump_name}"
pg_dump "${DATABASE_URL}" --format=custom --file="${dump_path}"

pepper_fingerprint=$(printf '%s' "${SMS_HASH_PEPPER}" | sha256sum | cut -d' ' -f1)

# schema_migrations is deploy/migrate.sql's own bookkeeping table (not
# part of the committed schema/migrations tree — see that file's header).
# Recording which migrations this dump was taken under lets a restore
# operator sanity-check they're restoring into a database that has run at
# least the same migrations, not fewer. A database that predates
# deploy/migrate.sql (or was migrated by hand) won't have this table;
# that's an empty string here, not a failure.
applied_migrations=$(psql "${DATABASE_URL}" -Atc \
  "SELECT coalesce(string_agg(name, ',' ORDER BY name), '') FROM public.schema_migrations" \
  2>/dev/null || echo "")

cat >"${manifest_path}" <<EOF
{
  "taken_at": "${ts}",
  "pg_dump_format": "custom",
  "postgres_version": "$(pg_dump --version | awk '{print $NF}')",
  "pepper_fingerprint_sha256": "${pepper_fingerprint}",
  "schema_migrations_applied": "${applied_migrations}"
}
EOF

echo "vsms-backup: uploading to ${BACKUP_RCLONE_REMOTE}"
rclone copyto "${dump_path}" "${BACKUP_RCLONE_REMOTE}/${dump_name}"
rclone copyto "${manifest_path}" "${BACKUP_RCLONE_REMOTE}/${manifest_name}"

echo "vsms-backup: pruning backups older than ${BACKUP_RETENTION_DAYS}d in ${BACKUP_RCLONE_REMOTE}"
rclone delete --min-age "${BACKUP_RETENTION_DAYS}d" --include "vsms-*.dump" "${BACKUP_RCLONE_REMOTE}" || true
rclone delete --min-age "${BACKUP_RETENTION_DAYS}d" --include "vsms-*.manifest.json" "${BACKUP_RCLONE_REMOTE}" || true

echo "vsms-backup: done — ${dump_name}"
