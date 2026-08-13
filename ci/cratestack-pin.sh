#!/usr/bin/env bash
# The single place that reads the pinned cratestack version out of root
# Cargo.toml. Echoes the version (e.g. "0.7.10") to stdout and nothing
# else — exit non-zero with a message on stderr if it can't be parsed.
#
# #204's own review found the first cut of this had *stopped the value*
# from drifting (three sites all reading Cargo.toml instead of one of them
# hardcoding a literal) but left the *extraction* triplicated: the same
# sed expression duplicated across ci/assert-migrations-current.sh and two
# ci.yml steps. That is a milder instance of the exact
# duplicated-hardcoded-list smell AGENTS.md's release-engineering notes
# warn about — if Cargo.toml's line shape ever changes (reordered keys, an
# appended comment, a move into [workspace.dependencies] with different
# spacing), three regexes would need updating in lockstep instead of one.
# This script is that one place; every other site calls it rather than
# re-deriving the pin itself.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

version=$(sed -n 's/^cratestack = { package = "cratestack-pg", version = "=\([0-9.]*\)" }.*/\1/p' "$root/Cargo.toml")

if [ -z "$version" ]; then
  echo "cratestack-pin: could not read the cratestack version pin from $root/Cargo.toml" >&2
  echo "cratestack-pin: expected a line shaped like: cratestack = { package = \"cratestack-pg\", version = \"=X.Y.Z\" }" >&2
  exit 1
fi

printf '%s' "$version"
