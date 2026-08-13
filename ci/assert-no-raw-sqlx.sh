#!/usr/bin/env bash
# R1 — all data access goes through CrateStack delegates.
# The allowlist lives here, in one place, so adding an exception is a review.
set -euo pipefail

# Both source roots: crates/ is libraries, app/ is binaries. Scanning only one
# of them would leave the other free to reach past the delegates.
roots=()
for dir in crates app; do
  [ -d "$dir" ] && roots+=("$dir")
done

if [ ${#roots[@]} -eq 0 ]; then
  echo "no crates/ or app/ yet — R1 lint vacuously passes"
  exit 0
fi

hits=$(grep -rn --include='*.rs' -E 'sqlx::(query|query_as|query_scalar|raw_sql)\b' "${roots[@]}" \
       | grep -vE 'sms-worker/src/(lease|notify|drain|jobs/reap_outbox)\.rs|sms-worker/tests/anchor_audit_live_postgres\.rs|sms-api/src/(cache|worker_locks|audit_log)\.rs|sms-test-support/src/lib\.rs|sms-gateway/src/health\.rs|sms-migrate/src/main\.rs|sms-gateway/tests/login_flow_live_postgres\.rs' || true)

if [ -n "$hits" ]; then
  echo "R1 violation — raw sqlx outside the named exceptions:" >&2
  echo "$hits" >&2
  echo >&2
  echo "See CONTRIBUTING.md. Raw SQL bypasses row-level policy, audit rows," >&2
  echo "@@emit outbox rows and version bumping — all four, silently." >&2
  exit 1
fi
echo "R1 OK (scanned: ${roots[*]})"
