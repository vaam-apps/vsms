#!/usr/bin/env bash
# Restores a backup.sh dump into $DATABASE_URL. This is the reusable
# restore primitive both an incident response and restore-drill.sh build
# on — see docs/runbooks/backup-restore.md for the actual incident
# procedure (this script restores INTO an existing, reachable database;
# it never creates one).
#
# pg_restore --clean here drops and recreates every object *inside* the
# target database before restoring, but never the database itself — so
# this is safe to point at an empty, freshly-migrated-or-not database, or
# one that already has (unwanted) data in it.
#
# Usage:
#   restore.sh <dump-name>       # pulls <dump-name> (+ its .manifest.json,
#                                 # if present) from $BACKUP_RCLONE_REMOTE
#   restore.sh --latest          # picks the newest vsms-*.dump in the remote
#   restore.sh --local <path>    # restores a dump already on disk — no
#                                 # rclone involved; used by
#                                 # restore-drill.sh, which already has the
#                                 # dump from its own backup step
set -euo pipefail

: "${DATABASE_URL:?set DATABASE_URL — the database to restore INTO}"

mode="${1:?usage: restore.sh <dump-name> | --latest | --local <path>}"

workdir=$(mktemp -d)
trap 'rm -rf "${workdir}"' EXIT

if [ "${mode}" = "--local" ]; then
  dump_path="${2:?--local needs a path to an existing .dump file}"
  manifest_path="${dump_path%.dump}.manifest.json"
else
  : "${BACKUP_RCLONE_REMOTE:?set BACKUP_RCLONE_REMOTE to pull a named or latest backup}"
  if [ "${mode}" = "--latest" ]; then
    dump_name=$(rclone lsf "${BACKUP_RCLONE_REMOTE}" --include "vsms-*.dump" | sort | tail -n1)
    [ -n "${dump_name}" ] || {
      echo "restore.sh: no backups found in ${BACKUP_RCLONE_REMOTE}" >&2
      exit 1
    }
  else
    dump_name="${mode}"
  fi
  manifest_name="${dump_name%.dump}.manifest.json"
  dump_path="${workdir}/${dump_name}"
  manifest_path="${workdir}/${manifest_name}"
  echo "restore.sh: pulling ${dump_name} from ${BACKUP_RCLONE_REMOTE}"
  rclone copyto "${BACKUP_RCLONE_REMOTE}/${dump_name}" "${dump_path}"
  rclone copyto "${BACKUP_RCLONE_REMOTE}/${manifest_name}" "${manifest_path}" 2>/dev/null ||
    echo "restore.sh: no manifest found for ${dump_name} — skipping the pepper fingerprint check" >&2
fi

# The pepper is part of the recoverable state, not just the database
# (docs/runbooks/backup-restore.md). msisdnHash/bodyHash are HMAC-SHA256
# keyed by SMS_HASH_PEPPER (crates/sms-api/src/pepper.rs); a dump restored
# under a DIFFERENT pepper than the one active when it was taken matches
# nothing — opt-out and dedupe lookups against the restored rows silently
# stop working, with no error anywhere. This only WARNS, never blocks:
# restoring under a deliberately different pepper is a legitimate DR
# choice in some scenarios, but it must never be a silent one.
if [ -f "${manifest_path}" ]; then
  stored_fp=$(grep -o '"pepper_fingerprint_sha256": *"[^"]*"' "${manifest_path}" | cut -d'"' -f4)
  if [ -n "${SMS_HASH_PEPPER:-}" ]; then
    current_fp=$(printf '%s' "${SMS_HASH_PEPPER}" | sha256sum | cut -d' ' -f1)
    if [ "${stored_fp}" != "${current_fp}" ]; then
      echo "restore.sh: WARNING — SMS_HASH_PEPPER does not match the pepper this backup was taken under." >&2
      echo "restore.sh: msisdnHash/bodyHash in the restored rows will not match anything hashed under the current pepper — opt-out and dedupe checks silently stop matching old rows. See crates/sms-api/src/pepper.rs and docs/runbooks/backup-restore.md before proceeding." >&2
    else
      echo "restore.sh: pepper fingerprint matches — restored hashes stay comparable under the current SMS_HASH_PEPPER."
    fi
  else
    echo "restore.sh: SMS_HASH_PEPPER not set locally — skipping the pepper fingerprint check (this backup's stored fingerprint: ${stored_fp})." >&2
  fi
else
  echo "restore.sh: no manifest available — cannot check whether this backup's pepper matches the current SMS_HASH_PEPPER. Proceeding blind; see docs/runbooks/backup-restore.md." >&2
fi

# `pg_restore --clean --if-exists` below drops and recreates every object in
# whatever DATABASE_URL points at. That is correct for a disaster recovery,
# and catastrophic for a fat-fingered DATABASE_URL during a rehearsal — the
# script cannot tell the two apart, so the operator has to. `restore-drill.sh`
# already guards its own destroy step this way; this is the same discipline
# applied to the primitive it calls.
#
# Deliberately an env var rather than a positional flag: `restore.sh` already
# takes `--latest` / a backup name, and during a real outage the last thing
# anyone needs is an argument-order mistake between a name and a confirmation.
if [ "${RESTORE_CONFIRM_OVERWRITE:-}" != "yes" ]; then
  echo "restore.sh: refusing to overwrite ${DATABASE_URL#*@} without confirmation." >&2
  echo "restore.sh: this drops and recreates every object in the target database." >&2
  echo "restore.sh: re-run with RESTORE_CONFIRM_OVERWRITE=yes once you have checked DATABASE_URL." >&2
  exit 1
fi

echo "restore.sh: restoring into postgres://***@${DATABASE_URL#*@}"
pg_restore --dbname="${DATABASE_URL}" --clean --if-exists --no-owner --no-privileges "${dump_path}"
echo "restore.sh: done"
