#!/usr/bin/env bash
# R1 — all data access goes through CrateStack delegates.
# The allowlist lives here, in one place, so adding an exception is a review.
set -euo pipefail

[ -d crates ] || [ -d app ] || { echo "no crates/ or app/ yet — R1 lint vacuously passes"; exit 0; }

hits=$(grep -rn --include='*.rs' -E 'sqlx::(query|query_as|query_scalar|raw_sql)\b' crates/ app/ 2>/dev/null \
       | grep -vE 'sms-worker/src/(lease|notify)\.rs|sms-api/src/cache\.rs' || true)

if [ -n "$hits" ]; then
  echo "R1 violation — raw sqlx outside the named exceptions:" >&2
  echo "$hits" >&2
  echo >&2
  echo "See CONTRIBUTING.md. Raw SQL bypasses row-level policy, audit rows," >&2
  echo "@@emit outbox rows and version bumping — all four, silently." >&2
  exit 1
fi
echo "R1 OK"
