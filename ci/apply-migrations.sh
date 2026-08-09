#!/usr/bin/env bash
# Apply every migration in order to $DATABASE_URL. Fails on the first error.
set -euo pipefail
: "${DATABASE_URL:?set DATABASE_URL, e.g. postgres://postgres@localhost:5432/vsms}"

for dir in schema/migrations/postgres/*/; do
  name=$(basename "$dir")
  [ -f "$dir/up.sql" ] || continue
  echo "==> $name"
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -f "$dir/up.sql"
done

# #153: cratestack's own idempotency bookkeeping table — not a
# schema/migrations/postgres entry (see that file's own header for why),
# but every scratch database this script sets up needs it too, since
# IdempotencyLayer is now mounted unconditionally on every generated route.
# Both statements in this file are IF NOT EXISTS, so reapplying it on every
# run of this already-unconditional loop is safe.
echo "==> idempotency-table"
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -f ci/idempotency-table.sql

echo "all migrations applied"
