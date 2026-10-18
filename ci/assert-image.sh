#!/usr/bin/env bash
# Assert that ONE built image really is the platform it claims to be, and that
# it can run under `runAsNonRoot: true`.
#
#   ci/assert-image.sh <image-ref> <os/arch> [--elf <path>]... [--scan <dir>]...
#
#   <os/arch>   linux/amd64 or linux/arm64 — the platform the image was
#               built FOR (the native runner it was built on).
#   --elf PATH  a file inside the image that must be an ELF of that
#               architecture (the service binary, or `node`).
#   --scan DIR  a directory inside the image; every `*.node` / `*.so*` file in
#               it must be an ELF of that architecture too (Next.js's
#               standalone trace and the demo app's node_modules carry native
#               addons, which is where a wrong-arch binary would hide).
#
# Three checks, each a way an image has been wrong before:
#
#   1. The image config says the platform we asked for. A buildx
#      misconfiguration (QEMU, a leaked `--platform`) can produce an image
#      whose config and whose bytes disagree; the config alone proves nothing,
#      hence check 2.
#   2. The binaries ARE that architecture, read from the ELF header
#      (`e_machine`), not from the tag, the runner label or the image config.
#   3. `Config.User` is numeric and not root. A kubelet enforcing
#      `runAsNonRoot: true` cannot verify a NAMED user and refuses the pod with
#      "image has non-numeric user (nonroot), cannot verify user is non-root".
#      Distroless's `nonroot:nonroot` and a `useradd`ed `node` both hit this.
#
# bash, not zsh: this runs on a GitHub-hosted ubuntu runner. It uses only
# `docker`, `od`, `find`, `mktemp` — no `readelf`/`file`, so it also runs on a
# developer's Mac.
set -euo pipefail

die() { echo "::error::assert-image: $*" >&2; exit 1; }

[[ $# -ge 2 ]] || die "usage: assert-image.sh <image-ref> <os/arch> [--elf PATH]... [--scan DIR]..."
IMAGE="$1"; PLATFORM="$2"; shift 2
OS="${PLATFORM%%/*}"; ARCH="${PLATFORM#*/}"

# ELF e_machine, little-endian: 0x3e x86-64, 0xb7 AArch64.
case "$ARCH" in
  amd64) WANT_MACHINE="3e 00"; WANT_NAME="x86-64" ;;
  arm64) WANT_MACHINE="b7 00"; WANT_NAME="aarch64" ;;
  *) die "unsupported architecture '$ARCH' (expected amd64 or arm64)" ;;
esac
[[ "$OS" == "linux" ]] || die "unsupported os '$OS'"

elf_machine() { # prints "<magic> <e_machine>" as hex bytes, or fails
  local bytes
  bytes="$(od -An -v -tx1 -N20 "$1" | tr -s ' \n' ' ' | sed 's/^ //; s/ $//')"
  echo "${bytes}"
}

check_elf() { # $1 = host file, $2 = label to print
  local bytes magic machine
  bytes="$(elf_machine "$1")"
  magic="${bytes:0:11}"                       # 7f 45 4c 46
  machine="${bytes:54:5}"                     # bytes 18-19
  [[ "$magic" == "7f 45 4c 46" ]] || die "$2 is not an ELF file (magic: ${magic})"
  [[ "$machine" == "$WANT_MACHINE" ]] \
    || die "$2 has ELF e_machine '${machine}', expected '${WANT_MACHINE}' (${WANT_NAME}) for ${PLATFORM}"
  echo "  ok  ${2}: ELF ${WANT_NAME}"
}

echo "assert-image: ${IMAGE} as ${PLATFORM}"

# 1. image config platform
GOT_PLATFORM="$(docker image inspect --format '{{.Os}}/{{.Architecture}}' "$IMAGE")"
[[ "$GOT_PLATFORM" == "$PLATFORM" ]] || die "image config says ${GOT_PLATFORM}, expected ${PLATFORM}"
echo "  ok  image config platform: ${GOT_PLATFORM}"

# 3. numeric, non-root user
USER_SPEC="$(docker image inspect --format '{{.Config.User}}' "$IMAGE")"
[[ "$USER_SPEC" =~ ^([0-9]+)(:[0-9]+)?$ ]] \
  || die "USER is '${USER_SPEC}': empty or not numeric. runAsNonRoot: true cannot verify a named user ('cannot verify user is non-root'). Use a numeric USER in the Dockerfile."
[[ "${BASH_REMATCH[1]}" -ne 0 ]] || die "USER is '${USER_SPEC}': uid 0 is root"
echo "  ok  USER ${USER_SPEC} (numeric, non-root)"

# 2. ELF architecture of every named binary and every scanned native addon
WORK="$(mktemp -d)"
CID="$(docker create "$IMAGE")"
trap 'docker rm -f "$CID" >/dev/null 2>&1 || true; rm -rf "$WORK"' EXIT

CHECKED=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --elf)
      [[ $# -ge 2 ]] || die "--elf needs a path"
      docker cp "${CID}:$2" "$WORK/elf-$CHECKED" >/dev/null || die "cannot copy $2 out of ${IMAGE}"
      check_elf "$WORK/elf-$CHECKED" "$2"
      CHECKED=$((CHECKED + 1)); shift 2 ;;
    --scan)
      [[ $# -ge 2 ]] || die "--scan needs a directory"
      DEST="$WORK/scan-$CHECKED"; mkdir -p "$DEST"
      docker cp "${CID}:$2/." "$DEST" >/dev/null || die "cannot copy $2 out of ${IMAGE}"
      N=0
      while IFS= read -r -d '' f; do
        [[ -f "$f" && ! -L "$f" ]] || continue
        # A `.so*` may legitimately be a text linker script; a `.node` never is.
        if [[ "$f" != *.node ]] && [[ "$(od -An -tx1 -N4 "$f" | tr -d ' \n')" != "7f454c46" ]]; then continue; fi
        check_elf "$f" "${2}/${f#"$DEST"/}"
        N=$((N + 1))
      done < <(find "$DEST" \( -name '*.node' -o -name '*.so' -o -name '*.so.*' \) -print0)
      echo "  ok  scanned ${2}: ${N} native object(s), all ${WANT_NAME}"
      CHECKED=$((CHECKED + 1)); shift 2 ;;
    *) die "unknown argument '$1'" ;;
  esac
done
[[ "$CHECKED" -gt 0 ]] || die "nothing to check: pass at least one --elf or --scan (an assertion with no subject proves nothing)"
echo "assert-image: ${IMAGE} OK"
