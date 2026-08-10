#!/usr/bin/env bash
# Apply every migration in order to $DATABASE_URL. Fails on the first error.
set -euo pipefail
: "${DATABASE_URL:?set DATABASE_URL, e.g. postgres://postgres@localhost:5432/vsms}"

# #153's cratestack idempotency bookkeeping table used to be a separate
# step here (ci/idempotency-table.sql, applied after this loop) because it
# isn't a cratestack-generated migration. It now lives at
# schema/migrations/postgres/0003_idempotency_table/up.sql instead — same
# directory, same shape as 0001_init/0002_bootstrap — so this one loop
# already applies it, in order, with nothing special-cased. Every
# statement in every file here is IF NOT EXISTS/idempotent by construction,
# so reapplying all of them on every run of this already-unconditional
# loop is safe.
for dir in schema/migrations/postgres/*/; do
  name=$(basename "$dir")
  [ -f "$dir/up.sql" ] || continue
  echo "==> $name"
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -f "$dir/up.sql"
done

echo "all migrations applied"
