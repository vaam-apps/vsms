#!/usr/bin/env bash
# #69's actual gate: "backups that have never been restored are not
# backups." This seeds a recognisable marker row, records an exact
# row-count for every table, takes a backup (via backup.sh — the same
# script the scheduled `backup` service runs), destroys every object in
# the target database, restores (via restore.sh), and diffs both the row
# counts and the marker row's exact content before vs after. A non-zero
# exit means the drill failed — treat that as the backup mechanism being
# broken, not this script.
#
# The destructive step is `DROP SCHEMA public CASCADE; CREATE SCHEMA
# public;`, not a real `dropdb`/`createdb`. Deliberate: it needs no
# separate admin connection (a single $DATABASE_URL is enough) and works
# against any Postgres, including a managed one where the application's
# own role can never hold DROP DATABASE — see
# docs/runbooks/backup-restore.md for why that's an honest stand-in for
# "destroy the database" rather than a shortcut.
#
# NEVER point this at a database with data you care about. Step 4 below
# is real, total, and depends on step 5 succeeding to get anything back.
# Requires the confirmation flag for exactly that reason — there is no
# default target and no way to skip it.
#
# Usage:
#   DATABASE_URL=postgres://user:pass@host:5432/db \
#     restore-drill.sh --yes-i-understand-this-destroys-the-target-database
#
# Optional: set BACKUP_RCLONE_REMOTE to exercise a real rclone remote
# (S3, B2, ...) instead of the drill's own throwaway local directory —
# proves the upload/download path too, not just pg_dump/pg_restore.
set -euo pipefail

: "${DATABASE_URL:?set DATABASE_URL — the SCRATCH database to drill against}"

confirm="${1:-}"
if [ "${confirm}" != "--yes-i-understand-this-destroys-the-target-database" ]; then
  echo "restore-drill.sh: refusing to run without the confirmation flag." >&2
  echo "This destroys every object in \$DATABASE_URL and restores from a fresh backup. Point it at a scratch database only." >&2
  echo "usage: DATABASE_URL=... restore-drill.sh --yes-i-understand-this-destroys-the-target-database" >&2
  exit 1
fi

: "${SMS_HASH_PEPPER:=$(openssl rand -base64 48)}"
export SMS_HASH_PEPPER

drill_dir=$(mktemp -d)
trap 'rm -rf "${drill_dir}"' EXIT

using_own_remote=0
if [ -z "${BACKUP_RCLONE_REMOTE:-}" ]; then
  BACKUP_RCLONE_REMOTE="${drill_dir}/remote"
  using_own_remote=1
fi
export BACKUP_RCLONE_REMOTE
mkdir -p "${BACKUP_RCLONE_REMOTE}"
if [ "${using_own_remote}" = 1 ]; then
  echo "restore-drill.sh: BACKUP_RCLONE_REMOTE not set — using a throwaway local directory (${BACKUP_RCLONE_REMOTE}). Set BACKUP_RCLONE_REMOTE to also exercise a real object-storage remote."
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

row_counts() {
  local url="$1" tables t c out=""
  tables=$(psql "${url}" -Atc "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename;")
  while IFS= read -r t; do
    [ -z "${t}" ] && continue
    c=$(psql "${url}" -Atc "SELECT count(*) FROM \"${t}\";")
    out="${out}${t}=${c},"
  done <<<"${tables}"
  echo "${out%,}"
}

echo "== 1/6: seeding a recognisable marker row =="
marker_id="drill-$(date -u +%s)"
psql "${DATABASE_URL}" -v ON_ERROR_STOP=1 -q -c \
  "CREATE TABLE IF NOT EXISTS public.backup_drill_marker (id text PRIMARY KEY, note text NOT NULL, created_at timestamptz NOT NULL DEFAULT now());"
psql "${DATABASE_URL}" -v ON_ERROR_STOP=1 -q -c \
  "INSERT INTO public.backup_drill_marker (id, note) VALUES ('${marker_id}', 'restore-drill-proof');"
echo "marker: ${marker_id}"

echo "== 2/6: recording exact row counts before backup =="
before=$(row_counts "${DATABASE_URL}")
echo "before: ${before}"

echo "== 3/6: taking a backup (backup.sh — the same script the scheduled service runs) =="
"${script_dir}/backup.sh"
# rclone lsf, not a raw `find` — BACKUP_RCLONE_REMOTE is an rclone remote
# spec (e.g. "s3:bucket/prefix" or "drilllocal:/backups"), which only
# looks like a filesystem path when it happens to be a bare local one.
# The first version of this script used `find` here and only worked by
# accident for that one case — found live, restoring from a real named
# remote, not by inspection.
dump_name=$(rclone lsf "${BACKUP_RCLONE_REMOTE}" --include "vsms-*.dump" | sort | tail -n1)
[ -n "${dump_name}" ] || {
  echo "restore-drill.sh: backup.sh did not produce a .dump file in ${BACKUP_RCLONE_REMOTE}" >&2
  exit 1
}
echo "backup: ${dump_name}"

echo "== 4/6: destroying every object in the target database =="
psql "${DATABASE_URL}" -v ON_ERROR_STOP=1 -q -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"
remaining=$(psql "${DATABASE_URL}" -Atc "SELECT count(*) FROM pg_tables WHERE schemaname = 'public';")
echo "tables remaining in public after DROP: ${remaining}"
if [ "${remaining}" != "0" ]; then
  echo "restore-drill.sh: expected 0 tables after DROP SCHEMA, got ${remaining}" >&2
  exit 1
fi

echo "== 5/6: restoring from the backup (restore.sh) =="
# Named-remote mode, not --local: BACKUP_RCLONE_REMOTE is already
# exported above, and restore.sh pulls ${dump_name} through rclone itself
# — the same path a real restore takes, not a filesystem shortcut only
# available when the remote happens to be local.
RESTORE_CONFIRM_OVERWRITE=yes "${script_dir}/restore.sh" "${dump_name}"

echo "== 6/6: verifying =="
after=$(row_counts "${DATABASE_URL}")
echo "after:  ${after}"
marker_after=$(psql "${DATABASE_URL}" -Atc "SELECT note FROM public.backup_drill_marker WHERE id = '${marker_id}';" || echo "")

failed=0
if [ "${before}" != "${after}" ]; then
  echo "RESTORE DRILL FAILED: row counts differ before vs after." >&2
  echo "  before: ${before}" >&2
  echo "  after:  ${after}" >&2
  failed=1
fi
if [ "${marker_after}" != "restore-drill-proof" ]; then
  echo "RESTORE DRILL FAILED: marker row '${marker_id}' missing or wrong after restore (got '${marker_after}')." >&2
  failed=1
fi

if [ "${failed}" = 1 ]; then
  exit 1
fi

echo "RESTORE DRILL PASSED: exact row counts match, and marker row '${marker_id}' survived backup + destroy + restore intact."
