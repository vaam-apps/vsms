# Deployment — from a bare VM to a running stack

A from-scratch deploy of §9.2's shape: Docker Compose on one VM, Caddy
terminating TLS, Postgres as the only coordination mechanism. Each step
proves something works before the next builds on it, the same convention
[`getting-started.md`](getting-started.md) uses — if a step fails, stop
there rather than pushing through.

This is the automatable half. It does not decide *where* that VM is —
[#3](https://github.com/vymalo/vsms/issues/3) (cross-border data transfer
under Law No. 2024/017) blocks that decision, not this runbook.

## Known seams — read this before you start

Two things this deploy tree deliberately does not build, because they were
landing in parallel elsewhere at the time this runbook was written:

- **No `provision-client` CLI subcommand exists yet.** Nothing today can
  mint an OAuth client + `private_key_jwt` keypair for the admin console
  to authenticate with. Step 6 below works around that by hand, the same
  way `crates/sms-api/examples/send_test_message.rs` already works around
  the equivalent gap for a test `App`/`AppClient`.
- **No `fake-orange` demo profile exists yet.** Every step below assumes
  real Orange Cameroon `client_credentials` and a real, contracted sender
  ID. There is no way to run this stack against a fake provider today.

Both are expected to land as their own PRs; re-check this file's git log
before repeating either workaround if you're reading this later.

## Prerequisites

- A VM with Docker Engine and the Compose plugin (`docker compose version`
  should print v2.20+ — `service_completed_successfully` depends-on
  conditions, used below, need it).
- Two DNS records (A/AAAA) pointed at the VM — one for the gateway's
  public origin, one for the admin console. A real domain, not just an
  IP: Caddy's automatic HTTPS and the OIDC issuer both need one. (Testing
  without public DNS: see `deploy/Caddyfile`'s own comment for the
  `tls internal` swap — everything else below still applies.)
- Real Orange Cameroon `client_credentials` and a contracted sender ID.
  Placeholder credentials get you through migrations and a running stack,
  but every submit attempt fails at Orange's own OAuth endpoint — the
  same boundary [`36-handset-gate.md`](36-handset-gate.md) documents.
- `openssl` on your workstation, for generating a Postgres password and
  (until `provision-client` lands) an RSA keypair by hand.

## 1. Clone and configure

```bash
git clone https://github.com/vymalo/vsms.git
cd vsms/deploy
cp .env.example .env
chmod 600 .env
```

Fill in `.env` — every var traces to a real `#[arg(long, env = ...)]` in
`app/sms-gateway/src/main.rs` / `app/sms-worker/src/main.rs`, or to a
`docker-compose.yml` service block; see that file's own comments for
which. At minimum:

```bash
# a real, random Postgres password
openssl rand -base64 24
```

`SMS_OIDC_ISSUER` must equal `https://` + `SMS_GATEWAY_DOMAIN` exactly —
every token this deployment issues carries it as `iss`, and a mismatch
here is silent until the first token validation fails.

## 2. Build and bring up Postgres + migrations first

```bash
docker compose --env-file .env up -d --build postgres migrate
docker compose logs migrate
```

Expect `migrations up to date` as the last line. `migrate` is a one-shot
container (see `deploy/migrate.sql`'s own header) — it applies
`schema/migrations/postgres/{0001_init,0002_bootstrap}` exactly once each,
tracked in a `schema_migrations` table this deploy path owns, under a
Postgres advisory lock. It never regenerates anything; the two `up.sql`
files it runs are copied verbatim from what's committed. If it exits
non-zero, nothing downstream is safe to start — fix this before continuing.

## 3. Create the first OP signing key — before sms-gateway's first start

`AGENTS.md`'s M1 notes describe `sms-gateway serve` as failing "loudly on
the first `/token` request... not at process start" if no active OP
signing key exists. **That's stale relative to the code as of this PR** —
found live, not by reading the doc: `Command::Serve` in
`app/sms-gateway/src/main.rs` calls `sms_auth::op::load_signing_keys`
*before* binding the listener, so a fresh database makes the container
exit immediately and crash-loop under `restart: unless-stopped`. Worth
fixing upstream (either the doc or the eager check), not done here —
out of scope for a deploy PR, and the practical consequence is what
matters for this runbook: **you cannot `docker compose exec` into a
container that never stays up.** Use `run --rm` instead, which starts a
fresh one-off container from the same image and env rather than reusing
the (not-yet-running) `sms-gateway` service container:

```bash
docker compose --env-file .env build sms-gateway
docker compose --env-file .env run --rm sms-gateway rotate-signing-key
```

(Not `sms-gateway rotate-signing-key` as the second argument — the image's
`ENTRYPOINT` is already the binary; only the subcommand goes after the
service name.) Run this once, on a fresh database, before ever bringing
`sms-gateway` up as a long-running service. Re-running it later rotates
the key with an overlap window (`sms_auth::op::ROTATION_OVERLAP`) — safe
to do as routine key hygiene, not just a bootstrap step, but only once
`sms-gateway` is already up (`docker compose exec` works fine against an
already-running container).

## 4. Bring up sms-gateway and sms-worker

```bash
docker compose --env-file .env up -d sms-gateway sms-worker
docker compose ps
```

Both wait on `migrate` finishing successfully (`service_completed_successfully`,
not just "started"). `sms-gateway`'s `HEALTHCHECK` hits its own
`GET /healthz` (added by this same PR — there was no health endpoint for
either binary before it); `sms-worker` has no HTTP surface, so its
`HEALTHCHECK` instead checks the mtime of a heartbeat file its `main.rs`
touches every 15s (see `app/sms-worker/Dockerfile`'s own comment for why
that's a more meaningful signal than a bare "is the process running"
check). Give both `--start-period` a few seconds before checking status.
`sms-gateway` should now report `(healthy)` on the first attempt — the
signing key from step 3 already exists.

## 5. Seed the `Provider` row

`sms-gateway serve` and `sms-worker` (with `dispatch` in `--roles`) both
resolve an `active` `Provider` row keyed `orange_cm` at startup —
`resolve_provider_row_id` in `app/sms-gateway/src/main.rs` — and there is
**no admin console or CLI path to create one yet** (M4 territory; the
existing dev-only `cargo run -p sms-api --example send_test_message` also
creates test `App`/`AppClient` fixtures you don't want in production, so
it isn't a fit here either). Seed it directly:

```bash
docker compose exec postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "
INSERT INTO providers (
  created_at, updated_at, id, key, display_name, kind, state, config,
  credential_ref, max_tps, max_daily_submissions, supports_dlr,
  supports_alpha_sender, supports_ucs2, supports_concat,
  cost_per_segment_xaf, healthy
) VALUES (
  now(), now(), cs_cuid(), 'orange_cm', 'Orange Cameroon SMS API',
  'orange_cm_http', 'active', '{}', 'env:ORANGE_CM_CLIENT_SECRET',
  10, 100000, true, true, true, true, 0, true
)
ON CONFLICT (key) DO NOTHING;
"
```

`config` and `credential_ref` are placeholders — confirmed against
`send_test_message.rs`'s own doc comment: neither binary reads this row's
`config`/`credentialRef` to construct the real `OrangeCmProvider`, both
build it from their own CLI flags/env instead (§2.4). This row's job is
only to exist, be `key = 'orange_cm'`, and be `state = 'active'`.

Restart both so the now-successful lookup takes effect:

```bash
docker compose restart sms-gateway sms-worker
```

## 6. Provision the admin console's OAuth client (manual, until #139's seam lands)

No `provisionAppClient` CLI or admin-console flow exists yet. Generate a
keypair by hand and register it the same way `provisionAppClient` would —
directly against the schema, under a `system` context, matching the
pattern `send_test_message.rs` already uses for its own fixtures:

```bash
mkdir -p secrets && chmod 700 secrets
openssl genrsa -out secrets/console-private-key.pem 2048
openssl rsa -in secrets/console-private-key.pem -pubout -out /tmp/console-public-key.pem
chmod 600 secrets/console-private-key.pem
```

Registering the resulting public key as an `OauthClient` with
`tokenEndpointAuthMethod = 'private_key_jwt'` needs a JWK, not a PEM —
building that by hand is exactly the kind of step `provisionAppClient`
(#111) exists to automate on the machine-caller side, and the admin
console's own provisioning flow is M4. Until either lands, this is a real
manual gap: track it, don't paper over it with a shortcut that skips
`private_key_jwt` (there is no shared-secret fallback in this schema on
purpose — see `AGENTS.md` on `tokenEndpointAuthMethod` having no
`@default`).

Set `SMS_CONSOLE_CLIENT_ID` in `.env` once the client is registered.

## 7. Bring up admin and Caddy

```bash
docker compose --env-file .env up -d --build admin caddy
docker compose ps
```

`admin`'s `HEALTHCHECK` hits its existing `GET /api/health` —
unauthenticated, on purpose: found live wiring this up (#139), the same
route was previously gated by `middleware.ts`'s Basic Auth check like
every other route, which meant an unauthenticated liveness probe got a
permanent `401` under `DASHBOARD_AUTH=basic` (the only mode
`NODE_ENV=production` accepts). `admin/middleware.ts`'s matcher now
excludes `api/health` explicitly — see that file's own comment. `caddy`
depends on both `sms-gateway` and `admin` being healthy before it starts
routing — see `deploy/Caddyfile`'s own comment for why the gateway and
admin domains have to stay two separate origins rather than one
path-routed domain (the OIDC discovery document's URLs are only
well-defined at an origin root).

## 8. Verify

```bash
curl -f https://$SMS_GATEWAY_DOMAIN/healthz
curl -f https://$SMS_GATEWAY_DOMAIN/.well-known/openid-configuration
curl -f https://$SMS_ADMIN_DOMAIN/api/health
```

All three should return `200`. If you're on the `tls internal` local/demo
path, add `-k` or trust Caddy's local CA first. This exact sequence — all
five containers, real health checks, a real Caddy TLS hop in front of
both origins — was run end to end against a throwaway config while
building this PR, using `tls internal` in place of real DNS; see the PR
description for what specifically was and wasn't verified.

## Ordering and failure modes, summarized

| Component       | Waits on                                   | What "not ready" looks like |
|------------------|---------------------------------------------|------------------------------|
| `migrate`        | `postgres` healthy                          | exits non-zero; nothing downstream should start (compose enforces this via `service_completed_successfully`) |
| `sms-gateway`     | `postgres` healthy, `migrate` completed      | **will not even start** without step 3's signing key — see that step for why this is stricter than `AGENTS.md`'s own prose currently describes |
| `sms-worker`      | `postgres` healthy, `migrate` completed      | starts fine with no signing key (only `sms-gateway` loads one); `dispatch` role specifically needs step 5's `Provider` row, other roles don't |
| `admin`           | `sms-gateway` healthy                        | starts; token exchange fails until step 6's client is provisioned |
| `caddy`           | `sms-gateway` and `admin` healthy            | won't route until both are up |

Two instances of `migrate` starting at once (a redeploy racing a still-up
previous stack) serialise on a Postgres advisory lock
(`deploy/migrate.sql`) rather than double-applying — see that file's own
header for why a `schema_migrations` tracking table exists only in this
deploy path, not in the committed `schema/migrations/` tree.

## Secrets — the decision, and what it doesn't protect against

**No `sops` here**, despite §9.2 mentioning it. This deployment's secrets
are: `POSTGRES_PASSWORD`, `ORANGE_CM_CLIENT_SECRET`, and the admin
console's `console-private-key.pem`. All three live in `deploy/.env`
(`chmod 600`, never committed — `.gitignore`/`.dockerignore` both exclude
it) and a `deploy/secrets/` directory (also never committed) mounted
read-only into the one container that needs it.

That is genuinely less than §9.2's own aspiration, and the honest
threat model is: this protects against the secrets ending up in git
history, in a Docker image layer, or in `docker inspect` output visible to
anyone without host access. It does **not** protect against anyone with
read access to the VM's filesystem, and it does **not** give you audited,
rotatable secret access the way `sops` + age/KMS would. For a single VM
with a small operator set, that trade is reasonable; it stops being
reasonable the moment more than a couple of people need host access, or
this deploys to more than one VM. Revisit with real `sops` (age-encrypted,
keyed to each operator, decrypted only at container start) before either
of those becomes true — not preemptively, since a half-wired `sops` setup
that still leaves plaintext secrets in `.env` for the vars this file
doesn't cover would be worse than being honest about the current gap.

## Releasing a new version

`.github/workflows/release.yml` builds and pushes all three images to GHCR
on any `v*.*.*` tag, tagged both `:$TAG` and `:latest`. On the VM:

```bash
cd deploy
docker compose --env-file .env pull
docker compose --env-file .env up -d
```

No image tag pinning in `docker-compose.yml` today — it always builds
locally from source (`build:`, not `image:`). Pulling pre-built GHCR images
instead of building on the VM is a reasonable follow-up once release
cadence picks up; not done here since it wasn't needed to get a first
deploy working.
