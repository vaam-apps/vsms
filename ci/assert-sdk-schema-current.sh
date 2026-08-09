#!/usr/bin/env bash
# The Rust SDK vendors its own copy of schema/schema.cstack, and it has to:
# `include_client_schema!` resolves its path against the invoking crate's
# CARGO_MANIFEST_DIR, so a published crate built from an integrator's
# registry cache cannot climb back into this monorepo to find the canonical
# file. See sdks/rust/vsms-sdk-rust/vendor-schema.sh for the full reasoning.
#
# A copy that nothing checks is a copy that drifts, and this one drifting is
# worse than most: the SDK's generated types would silently stop matching
# the server's wire contract, which is the one thing an SDK exists to get
# right. AGENTS.md already records the same shape of bug biting this repo
# when a "which files move together" list was duplicated and only one copy
# updated.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
canonical="$root/schema/schema.cstack"
vendored="$root/sdks/rust/vsms-sdk-rust/schema.cstack"

if [ ! -f "$vendored" ]; then
  echo "assert-sdk-schema-current: $vendored is missing" >&2
  exit 1
fi

if ! diff -u "$canonical" "$vendored" > /dev/null; then
  echo "assert-sdk-schema-current: the SDK's vendored schema has drifted from schema/schema.cstack." >&2
  echo >&2
  diff -u "$canonical" "$vendored" >&2 || true
  echo >&2
  echo "Refresh it with: ./sdks/rust/vsms-sdk-rust/vendor-schema.sh" >&2
  echo "and commit the result in the same change as the schema edit." >&2
  exit 1
fi

echo "assert-sdk-schema-current: OK — the SDK's vendored schema matches schema/schema.cstack"
