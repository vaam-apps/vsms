#!/usr/bin/env bash
# #204: nothing checked that schema/migrations/postgres/0001_init/{up,down}.sql
# is what `cratestack migrate diff` actually produces from the current
# schema/schema.cstack. Migrations here are regenerated *wholesale* — there
# is no committed schema.snapshot.json baseline (AGENTS.md's "Regenerating
# migrations" section) — so a stale 0001_init applies fine to an empty
# database (the `migrations` job's own gate) and compiles fine against
# `include_server_schema!` (the `rust` job's own gate) while silently
# missing whatever a schema edit added, until some live suite happens to
# touch the affected column and fails with a confusing runtime error
# instead of "your migrations are stale." Two concurrent PRs each
# regenerating 0001_init from a different base is the concrete way this
# happens: Git merges the result as ordinary text, and a non-overlapping
# hunk merges silently wrong.
#
# Assumes a `cratestack` binary is already on PATH (the calling workflow
# step installs it, version-locked to the same pin this script re-derives
# below) — same division of labour as ci/apply-migrations.sh assuming
# `psql` is already installed by a preceding step.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# Read from ci/cratestack-pin.sh — the one place that parses Cargo.toml for
# this — rather than re-deriving the pin here too. AGENTS.md's own
# release-engineering notes warn specifically about a duplicated hardcoded
# *value* drifting when only one copy gets updated; the milder version of
# that same smell is a duplicated *extraction* of a single value, which is
# what having this script and both ci.yml steps each carry their own copy
# of the same sed expression against Cargo.toml would have been. This also
# means the check fails loudly on a CLI/library mismatch instead of
# silently trusting whatever the caller happened to install: a newer CLI
# than the pin has, twice, emitted DDL the pinned library never produces
# (AGENTS.md).
pinned=$(./ci/cratestack-pin.sh)

installed=$(cratestack --version 2>/dev/null | awk '{print $2}' || true)
if [ "$installed" != "$pinned" ]; then
  echo "assert-migrations-current: installed cratestack CLI ('$installed') does not match the Cargo.toml pin (=$pinned)." >&2
  echo "A mismatched CLI emits DDL the compiled library never produces — see AGENTS.md's 'Regenerating migrations' section." >&2
  exit 1
fi

out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT

# --out-dir is the *parent* of the backend directory `migrate diff` writes
# into: it writes "$out/postgres/<timestamp>_init/{up,down}.sql", never
# "$out/<timestamp>_init/...". Verified by actually running the command
# both ways, not assumed from the docs — AGENTS.md's own command example in
# its "Regenerating migrations" section passes `--out-dir schema/migrations`
# (correct), while CONTRIBUTING.md's example passes
# `--out-dir schema/migrations/postgres` (would nest a second `postgres/`
# one level too deep). Using a scratch directory outside the repo here
# means `migrate diff`'s own schema.snapshot.json side effect — which this
# repo deliberately never commits — never touches the working tree at all,
# so there's nothing to clean up and nothing to confuse a later diff.
cratestack migrate diff --schema schema/schema.cstack --out-dir "$out" --backend postgres --name init >&2

regenerated=$(find "$out/postgres" -maxdepth 1 -type d -name '*_init')
if [ -z "$regenerated" ]; then
  echo "assert-migrations-current: cratestack migrate diff produced no *_init directory under $out/postgres" >&2
  exit 1
fi

fail=0
for f in up down; do
  if ! diff -u "schema/migrations/postgres/0001_init/$f.sql" "$regenerated/$f.sql"; then
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo >&2
  echo "assert-migrations-current: schema/migrations/postgres/0001_init/{up,down}.sql has drifted from what \`cratestack migrate diff\` produces from the current schema/schema.cstack (diff above)." >&2
  echo "Regenerate it (see AGENTS.md's 'Regenerating migrations' section):" >&2
  echo "  cratestack migrate diff --schema schema/schema.cstack --out-dir schema/migrations --backend postgres --name init" >&2
  echo "  # then copy the output over schema/migrations/postgres/0001_init/{up,down}.sql" >&2
  echo "  rm -f schema/migrations/postgres/schema.snapshot.json   # this repo does not commit it" >&2
  exit 1
fi

echo "assert-migrations-current: OK — 0001_init matches \`cratestack migrate diff\` for the current schema.cstack"
