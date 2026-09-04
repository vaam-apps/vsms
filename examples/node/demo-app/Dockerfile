# syntax=docker/dockerfile:1
#
# The demo showcase's own evaluator — see `src/index.ts`'s own module doc
# for what this proves. Build context is the REPOSITORY ROOT, matching
# every other Dockerfile in this workspace's own convention (see e.g.
# `frontends/apps/admin/Dockerfile`'s and `backends/apps/vsms-demo-seed/Dockerfile`'s
# own header comments) — and required by `.github/workflows/release.yml`,
# whose `build-and-push` job hardcodes `context: .` for every matrix
# entry rather than a per-image context. This package is still a fully
# standalone one otherwise — own `package.json`/`pnpm-lock.yaml`, no path
# dependency into this checkout, `--ignore-workspace` install below — the
# repo-root context here is purely a COPY-path convenience, not a
# dependency on anything else in the tree. Build with:
#
#   docker build -f examples/node/demo-app/Dockerfile .

FROM node:26-bookworm-slim AS runtime

# Node 26 no longer bundles corepack (same finding
# frontends/apps/admin/Dockerfile's own comment records) — install it
# explicitly, pinned, then use it to honour this package's own
# packageManager pin exactly.
RUN npm install -g corepack@0.35.0 && corepack enable

RUN useradd --system --uid 10002 --home-dir /nonexistent --shell /usr/sbin/nologin demoapp

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
USER demoapp

ENV NODE_ENV=production
EXPOSE 9000

HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=5 \
  CMD node -e "require('node:http').get('http://127.0.0.1:9000/healthz',r=>process.exit(r.statusCode===200?0:1)).on('error',()=>process.exit(1))"

ENTRYPOINT ["node", "src/index.ts"]
