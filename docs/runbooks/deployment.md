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
# a real, random SMS_HASH_PEPPER (minimum 32 bytes) — #134/#140
openssl rand -base64 48
```

`SMS_OIDC_ISSUER` must equal `https://` + `SMS_GATEWAY_DOMAIN` exactly —
every token this deployment issues carries it as `iss`, and a mismatch
here is silent until the first token validation fails.

`SMS_HASH_PEPPER` backs `Message.msisdnHash`/`bodyHash` (HMAC-SHA256) —
`sms-gateway serve` validates it (minimum 32 bytes) before doing anything
else, so a missing or too-short value fails the container at boot, not at
the first `sendMessage` call. Generate it once and keep it: rotating later
does not retroactively rehash rows already written under the old value —
see `crates/sms-api/src/pepper.rs`'s own module doc.

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

## 4. Seed the `orange_cm` Provider row — before sms-gateway's first start

`sms-gateway serve` resolves an `active` `Provider` row keyed `orange_cm`
at startup — `resolve_provider_row_id` in `app/sms-gateway/src/main.rs` —
*before* binding its listener, the same ordering trap step 3 above already
documents for the OP signing key. Nothing seeded this row until
[#148](https://github.com/vymalo/vsms/issues/148): a fresh database made
the container exit immediately and crash-loop under
`restart: unless-stopped`, exactly like a missing signing key does, and
this runbook used to work around it with a raw `psql` `INSERT` run
*after* bringing the containers up — which meant watching `sms-gateway`
crash-loop, then fixing it by hand. `sms-gateway seed-provider` closes
that gap: a real CLI subcommand, going through the CrateStack delegate
under a hand-built `owner` context (never raw SQL — R1), idempotent by
construction (`create` + catching the `23505` on `Provider.key`'s
`@unique` index, so re-running it is a clean no-op). Same `run --rm`
reasoning as step 3 — this needs a fresh one-off container, not the
not-yet-running `sms-gateway` service:

```bash
docker compose --env-file .env run --rm sms-gateway seed-provider
```

Its own defaults already match the row this runbook used to hand-write —
`--key orange_cm`, `--kind orange_cm_http`, `--credential-ref
env:ORANGE_CM_CLIENT_SECRET`, `--max-tps 10`, `--max-daily-submissions
100000`, `--cost-per-segment-xaf 0` — so no flags are required here.
`config`/`credential-ref` stay placeholders regardless of what's passed:
confirmed against `send_test_message.rs`'s own doc comment, neither
binary reads this row's `config`/`credentialRef` to construct the real
`OrangeCmProvider` — both build it from their own CLI flags/env instead
(§2.4). This row's job is only to exist, carry `key = 'orange_cm'`, and
end up `state = 'active'`, which the subcommand also guarantees (a fresh
row is created `disabled` — `Provider.state`'s own `@default` — and
activated in a second write).

## 5. Bring up sms-gateway and sms-worker

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
`sms-gateway` should now report `(healthy)` on the first attempt — both
the signing key from step 3 and the `Provider` row from step 4 already
exist, so neither startup dependency is missing this time.

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

**As of #156, `deploy/Caddyfile` uses `rate_limit`, a third-party module
the stock `caddy:2-alpine` image does not carry** — `deploy/docker-compose.yml`'s
`caddy` service still points `image:` at that stock image (that edit was
deliberately left out of #156 — see `deploy/caddy.Dockerfile`'s own
header for why and for the exact one-line change it needs). Until
`docker-compose.yml` is updated to `build:` from `deploy/caddy.Dockerfile`
instead, `docker compose up -d --build admin caddy` brings up a `caddy`
that fails to start (`Caddyfile:NN - Error during parsing: unrecognized
directive: rate_limit`, since the stock binary has no `http.handlers.rate_limit`
module) rather than silently running unlimited — a fail-loud gap, not a
silent one, but real until that compose edit lands.

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

## Rate limiting (#156) — what was actually proven, against a real stack

`deploy/Caddyfile`'s `rate_limit` block was verified against a genuinely
separate compose project (`-p vsms156`, non-default host port `18080`,
built from `deploy/caddy.Dockerfile` and the same `sms-gateway`/`migrate`
images this tree produces elsewhere), not asserted from reading the
config. Real `/token` and `/dlr/{providerKey}` requests, garbage
credentials and all — the point was to prove the edge throttles before
`authkestra-op`'s RS256 check and Postgres ever see the excess requests,
not to complete a real token exchange.

**`/token`, per-source-IP zone** (tested at a scaled-down `events=5,
window=20s` for a fast burst — the shipped default is `20/1m`, same
mechanism): the first 5 requests from one source got real application
responses (`401 invalid_client` — the garbage assertion was rejected by
`sms-auth`, proving the request reached it), the 6th through 10th all got
`429 Too Many Requests` with `Retry-After` set, straight from Caddy
(`Server: Caddy`, no application body). Waiting out the window recovered
it — a request sent ~20s later got a fresh `401`, not a `429`.

**Keying, verified both directions, not just claimed:**

- A second, genuinely different source IP (a separate container on the
  compose network, confirmed via Caddy's own `log_key`-enabled logs to
  be a distinct `remote_ip`) sending the *same* `client_id` got its own,
  independent budget — unaffected by the first source's bucket already
  reading `429`. This is the literal acceptance criterion: one abusive
  caller cannot exhaust a bucket a different, legitimate caller shares a
  client_id with, because there is no such shared bucket.
- That second source's own budget throttled identically once *it*
  sent enough requests (5 through before `429`), proving the limit isn't
  one-sided.
- Sending `X-Forwarded-For: 1.2.3.4` and, on a separate request,
  `X-Forwarded-For: 9.9.9.9` from that same already-throttled source
  did **not** create new buckets and did **not** lift the throttle —
  both still got `429`. Confirms the key is genuinely
  `{http.request.remote.host}` (the real TCP peer Caddy accepted the
  connection from), not anything a client-supplied header can influence
  — the exact bypass this task named as a risk to check for, not assume
  away.

**`/dlr/{providerKey}`, per-source-IP zone** (tested at a scaled-down
`events=4, window=20s`; shipped default `100/10s`): the first 4 requests
to `/dlr/orange_cm` got a real `400` from `sms-gateway` (malformed DLR
payload, on purpose — proves the request reached the app), the 5th
onward got `429`.

That burst was originally run against a composite `(IP, path)` key, and
the run also showed a different `providerKey` from the same source
getting its own bucket. **That composite key has since been removed**, so
the per-path isolation it demonstrated no longer applies and the sentence
claiming it has been struck rather than left to mislead. The reason is in
`deploy/Caddyfile`'s own comment on the zone: a composite key silently
disables `ipv6_prefix`, because the module only masks a key that parses
as a bare IP. Keeping IPv6 masking was judged the more valuable of the
two, since only one provider is wired up today. The `429`-after-4 result
above is unaffected — it was never a property of the path component.

**Not verified by this burst test, and worth being explicit about:**
the two aggregate/global zones (`token_global`, `dlr_global`) were
confirmed to parse and load (`caddy validate`, and `log_key` showed them
evaluating on every request) but were not driven past their own
(120/min, 200/10s) thresholds by volume — doing so meaningfully would
mean sending on the order of a hundred-plus requests in this runbook's
own verification pass, which wasn't judged worth the added noise given
they share the exact same sliding-window mechanism already proven above,
just with a static instead of a dynamic key (i.e. strictly simpler:
"one bucket for everyone" rather than "one bucket per key value").

**Composite `client_id` + source-IP keying on `/token`, as
`docs/architecture.md` §4.2 asks for, is not what this actually
implements** — it keys on source IP only, for a real reason checked
during this work, not an oversight: `client_id` on `/token` arrives only
in the URL-encoded POST body, and every way this edge could read it out
(Caddy's own `{http.request.body}` placeholder, explicitly documented
"inefficient; use only for debugging"; the one third-party module that
parses form bodies into placeholders, a single-contributor, zero-star,
recently-created repository) was rejected as unfit for a TLS-terminating
production edge parsing OAuth request bodies. See `deploy/Caddyfile`'s
own comment on the `token_per_ip`/`token_global` zones for the full
reasoning and the two real fixes that remain open (a first-party Caddy
module, or a second limiter inside `sms-auth`'s own `/token` handler,
which already has the parsed body) — filed as
[#168](https://github.com/vymalo/vsms/issues/168), not silently accepted.

## Ordering and failure modes, summarized

| Component       | Waits on                                   | What "not ready" looks like |
|------------------|---------------------------------------------|------------------------------|
| `migrate`        | `postgres` healthy                          | exits non-zero; nothing downstream should start (compose enforces this via `service_completed_successfully`) |
| `sms-gateway`     | `postgres` healthy, `migrate` completed      | **will not even start** without step 3's signing key or step 4's `orange_cm` `Provider` row — both are resolved before the listener ever binds, so either one missing crash-loops the container the same way (#148) |
| `sms-worker`      | `postgres` healthy, `migrate` completed      | starts fine with no signing key (only `sms-gateway` loads one); `dispatch` role specifically needs step 4's `Provider` row to route anything, other roles don't, and none of them crash-loop on its absence the way `sms-gateway` does |
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

`.github/workflows/release.yml` builds and pushes four images
(`sms-gateway`, `sms-worker`, `admin`, and `migrate` — #145 added the
fourth, for the Helm chart's migrate Job below) to GHCR, plus the
`deploy/charts/vsms` Helm chart as an OCI artifact, on two triggers:

- **Every push to `main`** — `:main` (mutable, moves with each push) and
  `:sha-<12 hex>` (immutable, one commit). The chart publishes as
  `0.0.0-main.<sha>`.
- **Any `v*.*.*` tag** — `:$TAG` and `:latest`, unchanged from what this
  workflow did before #145. The chart publishes as `$TAG` with its
  leading `v` stripped (Chart.yaml requires strict semver).

A third trigger, `workflow_dispatch`, exists only to smoke-test the
pipeline itself from a branch that isn't `main` and isn't tagged — it
tags images `:branch-<slug>` / `:sha-<12 hex>` and the chart
`0.0.0-dev.<sha>`, deliberately never touching `:main`/`:latest`. #145's
own PR used this to prove the pipeline works before this section existed
to document it — see that PR for the exact GHCR references it produced.

On the VM (compose path):

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

## Kubernetes (Helm)

`deploy/charts/vsms` — the same three long-running processes as the
compose path (`sms-gateway`, `sms-worker`, `admin`), plus the three
one-shot steps compose's own runbook above walks through by hand (apply
migrations, seed the `orange_cm` `Provider` row, mint the first OP signing
key), as Helm-hook Jobs that run automatically in the right order. Built
directly on the bjw-s **common** library chart v4 (`^4.6.2`), per #145.

**One correction to #145's own text, found while building this, not
assumed:** the issue named `oci://ghcr.io/bjw-s-labs/helm/common` as the
dependency reference. That artifact does not exist — confirmed against
GHCR's own API (`gh api orgs/bjw-s-labs/packages?package_type=container`
lists 21 packages; `helm/app-template` and `helm/multus` are there,
`helm/common` is not) and against bjw-s-labs/helm-charts' own
`.github/workflows/charts-release.yaml`, whose `release-library-charts`
job runs with `publishToOciRegistry: false` — library charts (`common` is
one) publish to the classic index.yaml repo
(`https://bjw-s-labs.github.io/helm-charts`) only, never to GHCR as OCI.
`deploy/charts/vsms/Chart.yaml` depends on `common` through that classic
repository URL instead — byte-identical content, same source repo, same
release pipeline, just a different distribution channel — pinned to
4.6.2, the newest v4 release as of this PR. Only `deploy/charts/vsms`
itself publishes to GHCR as OCI; that part of #145's ask stands as
written.

### What the chart does not do

- **Does not run Postgres.** `existingSecrets.database.name` names a
  Secret containing a `DATABASE_URL` key the operator's own database (or
  their own operator/subscription in front of one) supplies — the same
  "bring your own database" split the compose path already documents for
  why it doesn't use `sops`.
- **Does not create any Secret.** Every credential (`DATABASE_URL`,
  `SMS_HASH_PEPPER`, `ORANGE_CM_CLIENT_SECRET`, `DASHBOARD_BASIC_USERS`,
  the admin console's private key PEM) is referenced by name from an
  `existingSecrets.*` value; nothing under `deploy/charts/vsms/templates`
  ever writes one into a ConfigMap or a rendered manifest. See
  `values.yaml`'s own `existingSecrets` block for the exact key each
  Secret must contain.

### Ordering — why a Helm hook, not an init container

`sms-gateway serve` calls `sms_auth::op::load_signing_keys(...)?` and then
`resolve_provider_row_id(...)?` before binding its listener (this file's
own steps 3 and 4, above) — a fresh database makes the container exit
immediately on either one missing, not fail its first `/token` request or
DLR callback ([#148](https://github.com/vymalo/vsms/issues/148): the
`orange_cm` `Provider` row used to have no seeding path under Helm at
all — no window exists between the pre-install hooks completing and the
gateway `Deployment` being created for an operator to seed anything by
hand, unlike compose, where a human can at least `docker compose exec`
into Postgres while the container crash-loops). The chart's
`rotateSigningKey` and `seedProvider` controllers are both `pre-install`
Helm hook Jobs for exactly this reason: Helm does not create *any* other
release resource — including the `sms-gateway` Deployment — until every
pre-install hook Job has succeeded. A post-install step or a readiness
check on the gateway would both deadlock permanently, since the gateway
can never become ready without these steps having already run.

`migrate` (weight `-20`) and `seedProvider` (weight `-15`) are both
`pre-install,pre-upgrade` hooks — safe on every deploy because both are
genuinely idempotent: `migrate` via `deploy/migrate.sql`'s own
advisory-lock-guarded `schema_migrations` tracking table, `seedProvider`
via `sms-gateway seed-provider`'s own `create` + catch-`23505` dedupe (see
step 4 above). `rotateSigningKey` (weight `-10`, last in the chain) is
deliberately **not** hooked to `pre-upgrade`: unlike the other two, it is
not idempotent — every run mints a brand-new signing key with an overlap
window, so hooking it to every upgrade would rotate the key on every
`helm upgrade`, silently. Deliberate routine rotation later is a real
operator action: `kubectl create job --from=job/<release>-rotate-signing-key
<name> -n <namespace>` re-runs the same Job spec on demand (the
`before-hook-creation` delete policy leaves the most recent run's Job
object around after success for exactly this). `seedProvider` and
`migrate` sitting at distinct weights either side of it (`-15` vs. `-20`
and `-10`) is not a hard dependency — a `Provider` row and an OP signing
key are unrelated domains — but it keeps all three hooks a single,
strictly-ordered chain rather than two of them racing at a shared weight;
see `values.yaml`'s own comment on `seedProvider` for the full reasoning.

### Installing

```bash
helm repo add bjw-s-labs https://bjw-s-labs.github.io/helm-charts   # or use --repository-config
helm dependency build deploy/charts/vsms

# Pre-create every Secret existingSecrets.* names — see values.yaml's own
# comments for the exact key each one must contain. Example for two of
# them:
kubectl create secret generic vsms-hash-pepper \
  --from-literal=SMS_HASH_PEPPER="$(openssl rand -base64 48)"
kubectl create secret generic vsms-console-key \
  --from-file=console-private-key.pem=./console-private-key.pem

helm install vsms deploy/charts/vsms \
  --set image.tag=<a real tag published by release.yml — see above> \
  --set oidcIssuer=https://api.example.com \
  --set orange.clientId=... \
  --set orange.senderNumber=... \
  --set admin.consoleClientId=... \
  --set existingSecrets.database.name=vsms-database \
  --set existingSecrets.hashPepper.name=vsms-hash-pepper \
  --set existingSecrets.orange.name=vsms-orange \
  --set existingSecrets.dashboardBasicUsers.name=vsms-dashboard-users \
  --set existingSecrets.consolePrivateKey.name=vsms-console-key
```

Every one of `image.tag`, `oidcIssuer`, `orange.clientId`,
`orange.senderNumber`, `admin.consoleClientId`, and all five
`existingSecrets.*.name` fields is enforced with Helm's `required`
function — an install with any of them missing fails immediately with a
named field, not a cryptic render error or a silently-broken Pod.

### Verification actually performed for this PR

`helm lint --strict` and `helm template` (with a full set of test values —
no chart default was left to guess at) both ran clean, and the rendered
output was read, not assumed clean: hook annotations and weights land in
the order described above, no `existingSecrets` value's *contents* ever
appear in a rendered manifest (only Secret *names*/*keys* do), and the
probe paths match `/healthz` (gateway), `/api/health` (admin), and the
worker's heartbeat-file exec check exactly. See the PR description for
whether this also reached a real `kind`/`k3d` cluster and, separately,
whether the GHCR artifacts this section references were confirmed to
exist (`docker buildx imagetools inspect` / `helm pull --version`), not
just a green workflow run — this repo has been bitten by exactly that
gap before (`#87`, `AGENTS.md`).
