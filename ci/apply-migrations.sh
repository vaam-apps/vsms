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
echo "all migrations applied"
