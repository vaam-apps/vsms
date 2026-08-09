# syntax=docker/dockerfile:1
#
# #156: `deploy/Caddyfile`'s `rate_limit` directive needs
# github.com/mholt/caddy-ratelimit, a third-party Caddy module not
# compiled into the stock `caddy:2-alpine` image
# `deploy/docker-compose.yml` currently references (confirmed, not
# assumed: `docker run --rm caddy:2-alpine caddy list-modules` has no
# `http.handlers.rate_limit` in its output). This builds one that does,
# via xcaddy, off the same Alpine base the runtime stage already used —
# nothing about this deployment's libc story changes.
#
# Build context is the repository root, matching every other Dockerfile
# under this tree — `docker build -f deploy/caddy.Dockerfile .` — even
# though this one doesn't COPY anything from the repo; consistency with
# `app/sms-gateway/Dockerfile` et al. matters more than the one-line
# savings from scoping the context to `deploy/`.
#
# **This file is not wired into `deploy/docker-compose.yml` — that edit
# is intentionally left out of this PR.** #156 scopes this agent to
# `deploy/Caddyfile`, `deploy/.env.example`, and
# `docs/runbooks/deployment.md` only; `docker-compose.yml` is owned by
# other concurrent work. The change it needs, verified locally against a
# throwaway compose project (see this PR's description for the exact
# command and observed output) before being left for a human to land:
#
#   caddy:
#     build:
#       context: ..
#       dockerfile: deploy/caddy.Dockerfile
#     # image: caddy:2-alpine   <- delete this line
#
# Everything else in that service block (ports, volumes, depends_on,
# environment) is unaffected — this produces a drop-in replacement binary
# under the same `/usr/bin/caddy` entrypoint the base image already used.
#
# **Module pin.** github.com/mholt/caddy-ratelimit has no tagged release
# past `v0.1.0` (August 2024 — predates the `ipv6_prefix` option
# `deploy/Caddyfile` uses, confirmed by grepping that tag's own README),
# despite 479 stars, 34 forks, and commits through June 2026 (checked via
# the GitHub API, not assumed) — written by mholt, Caddy's own author.
# Pinned to a specific commit SHA rather than a branch or an unpinned
# `@latest`, for the same reason the root CLAUDE.md's global release-
# engineering notes ban floating dependencies in any release pipeline:
# reproducible today, and a deliberate, reviewable one-line bump later —
# not a moving target this build silently drifts onto.
FROM caddy:2.11.4-builder-alpine AS builder

RUN xcaddy build \
	--with github.com/mholt/caddy-ratelimit@5625512f24f6f59d6f64fb3aafe5eecff0b286db

FROM caddy:2.11.4-alpine

COPY --from=builder /usr/bin/caddy /usr/bin/caddy
