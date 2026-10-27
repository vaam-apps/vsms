#!/usr/bin/env bash
# Builds the sealing module the Node SDK loads: `sdks/rust/vsms-sdk-wasm`,
# compiled to WebAssembly and wrapped for Node by `wasm-bindgen --target nodejs`.
#
#   scripts/build-wasm.sh              -> sdks/node/vsms-sdk-node/wasm       (ships in the npm package)
#   scripts/build-wasm.sh --responder  -> sdks/node/vsms-sdk-node/wasm-test  (the server half, for the tests only)
#   scripts/build-wasm.sh --web [DIR]  -> frontends/apps/admin/public/sealer (the operator console's browser build)
#   scripts/build-wasm.sh --web --responder DIR
#                                      -> DIR (the browser build plus the server half, for the console's tests)
#
# All are build outputs and gitignored. `--web` is the same crate and the same
# pinned `wasm-bindgen`, run with `--target web` (an ES module the console
# imports by URL, and a `.wasm` file its own origin serves); it stops there:
# the Node glue's rewrite below is for a bundler that would inline it, and the
# console loads this one at run time, unbundled.
#
# `wasm-bindgen`'s command-line tool must be exactly the version of the `wasm-bindgen` crate in sdks/rust/Cargo.lock:
# the two agree on a private interface, and a mismatch fails at load time with
# an error that names neither (`just wasm-tools` installs the right one; this
# script installs nothing and says which to install when it disagrees).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
package="$(cd "$here/.." && pwd)"
rust="$(cd "$package/../../rust" && pwd)"

features=()
out=""
bindgen_target=nodejs
web_default="$(cd "$package/../../../frontends/apps/admin" && pwd)/public/sealer"
for arg in "$@"; do
	case "$arg" in
	--responder) features=(--features responder) ;;
	--web) bindgen_target=web ;;
	*) out="$arg" ;;
	esac
done
if [ -z "$out" ]; then
	if [ "$bindgen_target" = "web" ]; then
		out="$web_default"
	elif [ "${#features[@]}" -gt 0 ]; then
		out="$package/wasm-test"
	else
		out="$package/wasm"
	fi
fi

wanted="$(awk '/^name = "wasm-bindgen"$/ {getline; gsub(/version = |"/, ""); print; exit}' "$rust/Cargo.lock")"
if ! command -v wasm-bindgen >/dev/null 2>&1; then
	echo "build-wasm: FAIL — wasm-bindgen is not on PATH." >&2
	echo "build-wasm: run \`just wasm-tools\`, or: cargo install wasm-bindgen-cli --locked --version $wanted" >&2
	exit 1
fi
found="$(wasm-bindgen --version | awk '{print $2}')"
if [ "$found" != "$wanted" ]; then
	echo "build-wasm: FAIL — wasm-bindgen $found is installed, sdks/rust/Cargo.lock pins the crate at $wanted." >&2
	echo "build-wasm: run \`just wasm-tools\`, or: cargo install wasm-bindgen-cli --locked --version $wanted --force" >&2
	exit 1
fi
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

# Size over speed: this is a signing call, not a hot loop, and the module is
# embedded in the glue and parsed on every `require`.
(
	cd "$rust"
	CARGO_PROFILE_RELEASE_OPT_LEVEL=s \
		cargo build --release --locked --target wasm32-unknown-unknown -p vsms-sdk-wasm "${features[@]}"
)

rm -rf "$out"
mkdir -p "$out"
wasm-bindgen --target "$bindgen_target" --out-dir "$out" --out-name vsms_sdk_wasm \
	"${CARGO_TARGET_DIR:-$rust/target}/wasm32-unknown-unknown/release/vsms_sdk_wasm.wasm"

if [ "$bindgen_target" = "web" ]; then
	echo "build-wasm: wrote $out"
	exit 0
fi

# wasm-bindgen's Node glue reads its `.wasm` from `${__dirname}` at load time.
# A bundler that inlines the glue (Next's Turbopack does, for a workspace
# package) rewrites `__dirname` to a build-time path that does not exist where
# the bundle runs, and every call to the gateway then fails with ENOENT. So the
# module carries its own bytes: the read becomes a base64 literal, and the glue
# needs no file, no path and no `fs` wherever it ends up. Each of the two lines
# must be found exactly once, so a wasm-bindgen that words them differently
# fails this build rather than shipping a module that still reads from disk.
glue="$out/vsms_sdk_wasm.js"
b64="$out/.wasm.b64"
base64 <"$out/vsms_sdk_wasm_bg.wasm" | tr -d '\n' >"$b64"
awk -v b64file="$b64" '
  $0 == "const wasmPath = `${__dirname}/vsms_sdk_wasm_bg.wasm`;" { path++; next }
  $0 == "const wasmBytes = require('"'"'fs'"'"').readFileSync(wasmPath);" {
    bytes++
    getline literal < b64file
    print "const wasmBytes = Buffer.from(\"" literal "\", \"base64\");"
    next
  }
  { print }
  END { if (path != 1 || bytes != 1) exit 1 }
' "$glue" >"$glue.embedded" || {
	echo "build-wasm: FAIL — wasm-bindgen's glue no longer reads its module the way this script rewrites." >&2
	exit 1
}
mv "$glue.embedded" "$glue"
rm -f "$b64" "$out/vsms_sdk_wasm_bg.wasm"
# The SDK is ESM ("type": "module"); wasm-bindgen emits CommonJS. A nested
# package.json is what tells Node which one this directory is.
printf '{ "type": "commonjs" }\n' >"$out/package.json"
echo "build-wasm: wrote $out"
