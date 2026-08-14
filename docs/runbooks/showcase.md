# Showcase: run vsms from published images, no source build

One command, no Rust toolchain, no Node toolchain, no clone-and-build wait —
`compose.demo.yaml` pulls every vsms image from GHCR and brings up a working
console you can sign into, with a message reaching a terminal state. This is
**not** [Local development](local-development.md) (`just demo`, which builds
from source and is what you want if you're changing this codebase) and it is
**not** [Deployment](deployment.md) (`deploy/docker-compose.yml`, the
production-shaped stack with a real Caddy TLS edge, backups, and Prometheus).
Read `compose.demo.yaml`'s own header comment for the full three-way
comparison.

## Prerequisites

Docker and the Compose plugin (`docker compose version`). Nothing else —
no `git clone` of anything beyond this repo, no `cargo`, no `pnpm`. If your
machine's architecture isn't `linux/amd64` (Apple Silicon, for example),
Docker Desktop's own emulation handles it transparently; expect the first
`docker compose ... up` to take a few minutes longer while it pulls
`--platform linux/amd64` layers.

## Bring it up

```bash
docker compose -f compose.demo.yaml --profile console up -d
```

This runs, in order: Postgres, `migrate` (applies both migrations), then in
parallel `seed-signing-key` (an active OP signing key — `sms-gateway`
refuses to bind its listener without one) and `seed-dispatch` (a `Provider`
+ `Route`, same reason), then `seed-demo-app` (an `App` + approved
`SenderId` — see that command's own doc for why this step exists only for
this showcase), then `provision-client` (a real, HTTP-usable machine
credential) and, because `--profile console` is present, `seed-console-client`
and `provision-user` (the account you'll sign in with) — before finally
starting `sms-gateway`, `sms-worker`, `sms-fake-orange`, and `admin`.

Watch it settle:

```bash
docker compose -f compose.demo.yaml --profile console ps
```

Everything with a `restart: unless-stopped` service should show `Up
(healthy)` for `sms-gateway` within about 30 seconds; the one-shot seed/
provision containers show `Exited (0)` — that's success, not a crash.

## Sign in

Find the generated password (never stored, printed once, to this
container's own log — the exact same design `sms-gateway provision-user`
uses everywhere else):

```bash
docker compose -f compose.demo.yaml logs provision-user
```

Then open <http://localhost:3200/> (override with `VSMS_DEMO_CONSOLE_PORT`
if that port collides with something else on your machine) and sign in with
`demo@vsms.local` and the password from the log line above.

## Send a message and watch it reach a terminal state

From the console's composer, send a message to any Cameroon-shaped number
(e.g. `+237677123456`) using sender `VSMS` — the values `seed-demo-app`
registered. Within a few seconds it should move `accepted → queued →
routed → submitted → delivered` on the Messages screen, the same pipeline
[Local development](local-development.md) proves, just against
`sms-fake-orange` instead of a real Orange sandbox — no real SMS is ever
sent (`app/sms-fake-orange/src/main.rs`'s own module doc).

## Backend-only (no console)

```bash
docker compose -f compose.demo.yaml up -d
```

Omitting `--profile console` skips `admin`, `seed-console-client`,
`provision-user`, and the permission-fix step between them entirely — not
started-then-idle, not present-but-misconfigured. `sms-gateway` and
`sms-worker` still come up healthy, and `provision-client` still runs (a
working machine credential is exactly what a backend-only deployment should
be able to prove it has), so you can drive the gateway directly:

```bash
curl http://127.0.0.1:8280/healthz
```

(override with `VSMS_DEMO_GATEWAY_PORT`). This is `compose.demo.yaml`'s own
proof of [CONTRIBUTING.md's R4](../../CONTRIBUTING.md) — "the admin console
is optional, the backend must run without it."

## Reset and re-run

```bash
docker compose -f compose.demo.yaml --profile console down -v
docker compose -f compose.demo.yaml --profile console up -d
```

`-v` matters: `provision-client` refuses to overwrite an existing private
key (a real safety property, not a demo limitation), so a second `up`
against the *same* named volumes fails loudly on that step rather than
silently reusing stale credentials. `-v` drops this file's own named
volumes (`vsms_demo_pgdata`, `vsms_demo_secrets` — scoped to the
`vsms-demo` Compose project name, never touching anything from
`compose.yml`, `deploy/docker-compose.yml`, or an unrelated project on the
same machine) so every `up` after a `down -v` starts genuinely fresh.

## Bumping the showcased version

```bash
VSMS_IMAGE_TAG=v0.2.0 docker compose -f compose.demo.yaml --profile console up -d
```

`sms-gateway`/`sms-worker`/`admin`/`migrate` are published together on
every `v*.*.*` tag; `VSMS_IMAGE_TAG` is the one thing this file expects you
to change. `sms-fake-orange` (`VSMS_FAKE_ORANGE_IMAGE_TAG`) is not yet
published on the same cadence — see `compose.demo.yaml`'s own comment on
that service for the current state and why.

## No Postgres port, on purpose

```bash
docker compose -f compose.demo.yaml ps postgres
docker port $(docker compose -f compose.demo.yaml ps -q postgres)
```

The second command prints nothing — Postgres has no published port at
all. Every other service reaches it over the Compose network's own
internal DNS (`postgres:5432`). If you need a `psql` prompt for a real
look, go through the container rather than opening the port to the host:

```bash
docker compose -f compose.demo.yaml exec postgres psql -U vsms
```
