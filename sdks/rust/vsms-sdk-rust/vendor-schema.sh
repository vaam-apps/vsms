#!/usr/bin/env bash
# Refreshes this crate's vendored copy of schema/schema.cstack.
#
# Why a copy exists at all: `include_client_schema!` (cratestack-macros)
# resolves its path against `CARGO_MANIFEST_DIR` of the crate that invokes
# it — see cratestack-macros' `include::parse::parse_schema_literal`, which
# does `PathBuf::from(CARGO_MANIFEST_DIR).join(schema_relative)` and then
# bakes an absolute `include_str!(...)` of that resolved path into the
# expansion. That path is real at *this* repo's own build time (a relative
# `../../../schema/schema.cstack` would resolve fine), but this crate is
# meant to be published to crates.io and built from an integrator's cargo
# registry cache, where nothing above this crate's own directory exists.
# So the schema this crate expands against has to live *inside* the
# published package, not be reached by climbing back into the monorepo.
#
# This is a plain, verifiable copy — not a fork. Run this after any change
# to schema/schema.cstack that this SDK's typed surface should track, and
# commit the result in the same change. There is no drift-detection CI gate
# for it yet (unlike packages/sms-client's `client-check`); `diff` against
# schema/schema.cstack by hand until one exists.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cp "$ROOT/schema/schema.cstack" "$(dirname "${BASH_SOURCE[0]}")/schema.cstack"
echo "vendored $(dirname "${BASH_SOURCE[0]}")/schema.cstack from $ROOT/schema/schema.cstack"
