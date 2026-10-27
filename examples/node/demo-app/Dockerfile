# syntax=docker/dockerfile:1
#
# The demo showcase's own evaluator — see `src/index.ts`'s own module doc
# for what this proves. Build context is the REPOSITORY ROOT, matching
# every other Dockerfile in this workspace's own convention (see e.g.
# `frontends/apps/admin/Dockerfile`'s and `backends/apps/vsms-demo-seed/Dockerfile`'s
# own header comments) — and required by `.github/workflows/release.yml`,
# whose `build-and-push` job hardcodes `context: .` for every matrix
# entry rather than a per-image context. The package keeps its own
# `package.json`/`pnpm-lock.yaml` and installs with `--ignore-workspace`, but
# it now depends on the Node SDK by path (`file:`), so the repo-root context
# is also what lets the build stage below build that SDK from this checkout.
# Build with:
#
#   docker build -f examples/node/demo-app/Dockerfile .

# The Node SDK this evaluator signs with is built from this checkout, not
# pulled from npm: it depends on `@vaam-apps/vsms-node` by path, and the SDK
# seals through `sdks/rust/vsms-sdk-wasm` compiled to WebAssembly, so the build
# stage below needs Rust with the wasm32 target. The toolchain block is the same
# one `frontends/apps/admin/Dockerfile` carries, from the same
# `rust:1.98-bookworm` the CI runner image is (`ci/runner/Dockerfile`).
FROM rust:1.98-bookworm AS rust

FROM node:26-bookworm-slim AS sdk

# DL3008: no apt version pin, deliberately - the packages come from the
# floating Debian bookworm suite the base image tracks, and a patch pin would
# fail the build the day the mirror drops that exact revision (the same call
# as ci/runner/Dockerfile's).
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust /usr/local/rustup /usr/local/rustup
COPY --from=rust /usr/local/cargo /usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN rustup target add wasm32-unknown-unknown

RUN npm install -g corepack@0.35.0 && corepack enable && corepack prepare pnpm@11.18.0 --activate

WORKDIR /repo
COPY . .

RUN pnpm install --frozen-lockfile --filter @vaam-apps/vsms-node...

# `wasm-bindgen`'s command-line tool at exactly the version `sdks/rust/Cargo.lock`
# pins for the crate: the two agree on a private interface, and a mismatch
# fails at load time naming neither.
RUN wanted="$(awk '/^name = "wasm-bindgen"$/ {getline; gsub(/version = |"/, ""); print; exit}' sdks/rust/Cargo.lock)" \
    && cargo install wasm-bindgen-cli --locked --version "$wanted"

RUN pnpm --filter @vaam-apps/vsms-node run build

FROM node:26-bookworm-slim AS runtime

# Node 26 no longer bundles corepack (same finding
# frontends/apps/admin/Dockerfile's own comment records) — install it
# explicitly, pinned, then use it to honour this package's own
# packageManager pin exactly.
RUN npm install -g corepack@0.35.0 && corepack enable

RUN useradd --system --uid 10002 --home-dir /nonexistent --shell /usr/sbin/nologin demoapp

# The SDK sits where `package.json`'s `file:../../../sdks/node/vsms-sdk-node`
# resolves from `/app` (three levels up from `/app` is `/`): pnpm copies a
# directory dependency at install time, so only what the package publishes
# (`files`: `dist`, `wasm`) is needed. The app stays at `/app`, the path
# images.yml's assert scans.
COPY --from=sdk /repo/sdks/node/vsms-sdk-node/package.json /sdks/node/vsms-sdk-node/package.json
COPY --from=sdk /repo/sdks/node/vsms-sdk-node/dist /sdks/node/vsms-sdk-node/dist
COPY --from=sdk /repo/sdks/node/vsms-sdk-node/wasm /sdks/node/vsms-sdk-node/wasm

WORKDIR /app
COPY examples/node/demo-app/package.json examples/node/demo-app/pnpm-lock.yaml ./
COPY examples/node/demo-app/src ./src
COPY examples/node/demo-app/tsconfig.json ./

# `--ignore-workspace`: this directory has its own lockfile and is not a
# member of either the repo-root pnpm workspace or `examples/`'s own
# — same flag, same reason, `examples/node/webhook-receiver`'s own CI
# installation step uses (AGENTS.md's "hooks_node_receiver_live.rs"
# section). `--prod`: no build step exists to need devDependencies at
# runtime (Node's native TypeScript support runs `src/index.ts`
# directly) — typecheck/lint/test only ever run in CI, against a full
# (non `--prod`) install. `--trust-lockfile`: pnpm 11's minimumReleaseAge
# supply-chain check re-verifies every lockfile entry's publish date on
# every install, and `--ignore-workspace` (needed above, for the reason
# just given) makes it ignore this directory's own `pnpm-workspace.yaml`
# too, so there is no scoped `minimumReleaseAgeExclude` this file could
# read even if one existed here — confirmed live, not assumed: a local
# `minimumReleaseAgeExclude`/`minimumReleaseAge: 0` in this directory's
# own `pnpm-workspace.yaml` has no effect at all under `--ignore-workspace`,
# only without it. `--trust-lockfile` is pnpm's own documented answer for
# exactly this shape ("CI runs against an already-verified lockfile") —
# this Dockerfile installs from a lockfile already committed and reviewed
# in this repo's own PR process, never one an attacker could substitute
# at build time, so re-verifying each entry's age against the registry on
# every image build buys nothing a newly-bumped dependency wouldn't
# already trip regardless of how recently it was reviewed here.
RUN corepack prepare pnpm@11.18.0 --activate \
    && pnpm install --ignore-workspace --frozen-lockfile --trust-lockfile --prod

RUN chown -R demoapp:demoapp /app
# Numeric uid, not `USER demoapp` — a kubelet cannot verify a named user
# is non-root under `runAsNonRoot: true` (see frontends/apps/admin/Dockerfile).
# 10002 is the uid created above.
USER 10002

ENV NODE_ENV=production
EXPOSE 9000

HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=5 \
  CMD node -e "require('node:http').get('http://127.0.0.1:9000/healthz',r=>process.exit(r.statusCode===200?0:1)).on('error',()=>process.exit(1))"

ENTRYPOINT ["node", "src/index.ts"]
