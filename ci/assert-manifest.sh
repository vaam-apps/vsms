#!/usr/bin/env bash
# Assert that every given image reference is a manifest list containing
# EXACTLY linux/amd64 and linux/arm64 — no fewer, no more, no duplicates.
#
#   ci/assert-manifest.sh ghcr.io/vaam-apps/vsms-sms-gateway:v0.6.0 ...
#
# Reads the registry, not the build: it inspects the tag as a consumer would
# pull it, which is the only place "the tag is multi-arch" is actually true.
#
# BuildKit attaches provenance as extra manifests whose platform is
# `unknown/unknown`. Those are not images and are ignored; every entry with a
# real platform is counted.
set -euo pipefail

die() {
	echo "::error::assert-manifest: $*" >&2
	exit 1
}
[[ $# -ge 1 ]] || die "usage: assert-manifest.sh <image-ref>..."

WANT='["linux/amd64","linux/arm64"]'
for ref in "$@"; do
	raw="$(docker buildx imagetools inspect --raw "$ref")" || die "cannot inspect ${ref}"
	jq -e '(.manifests | type) == "array"' >/dev/null <<<"$raw" ||
		die "${ref} is a single-platform image, not a manifest list"
	got="$(jq -c '[.manifests[]
                 | select((.platform.os // "unknown") != "unknown"
                          and (.platform.architecture // "unknown") != "unknown")
                 | "\(.platform.os)/\(.platform.architecture)"]
                | sort' <<<"$raw")"
	[[ "$got" == "$WANT" ]] ||
		die "${ref} contains ${got}, expected exactly ${WANT}"
	echo "  ok  ${ref}: ${got}"
done
