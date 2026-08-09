# syntax=docker/dockerfile:1
#
# #69's backup mechanism: postgres:16-alpine gives the exact `pg_dump` /
# `pg_restore` for this stack's own server major version — same reasoning
# deploy/migrate.Dockerfile already uses for `psql`, so the tool taking
# the dump and the tool that will one day restore it are never a version
# guess. `rclone` is added for the object-storage upload and is
# deliberately provider-agnostic: AWS S3, Backblaze B2, GCS, Azure Blob,
# MinIO, or a bare local path all work behind the same `rclone
# copyto`/`rclone delete` calls in backup.sh — "the bucket is the
# operator's choice" (docs/runbooks/backup-restore.md) shouldn't mean
# picking a cloud vendor for them.
#
# Scheduling uses Alpine's own busybox `crond`, already in the base image
# — no extra scheduler daemon (supercronic, etc.) to install, trust, or
# keep patched for a job this simple.
#
# Build context is the repository root, same as every other Dockerfile
# under app/ — build with `docker build -f deploy/backup.Dockerfile .`.
FROM postgres:16-alpine

# openssl is needed only by restore-drill.sh's own fallback
# (`SMS_HASH_PEPPER:=$(openssl rand ...)`) for an ad-hoc drill run with no
# pepper supplied — the scheduled `backup` service always gets a real one
# from $SMS_HASH_PEPPER (required, no default) and never exercises this
# path.
RUN apk add --no-cache rclone bash openssl

COPY deploy/backup.sh /backup.sh
COPY deploy/restore.sh /restore.sh
COPY deploy/restore-drill.sh /restore-drill.sh
COPY deploy/backup-entrypoint.sh /entrypoint.sh
RUN chmod +x /backup.sh /restore.sh /restore-drill.sh /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
