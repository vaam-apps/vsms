# Getting started

First-time setup, in order. Each step proves something works before the next one builds on it — if a step fails, stop there rather than pushing through.

## Prerequisites

- **Rust**, stable, edition 2021 toolchain (`rustup default stable`).
- **PostgreSQL 16** reachable somehow — a local install, or Docker (`docker run --rm -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:16`). Nothing here needs a specific host; every command below assumes `psql`/`createdb` are on `PATH` and `DATABASE_URL` points at a reachable server.
- **[`just`](https://github.com/casey/just)** — the task runner this repo's own commands go through (`cargo install just`, or your package manager).
- **Python 3** — only for `ci/gen-bootstrap-sql.py` and the R2 parity check; nothing exotic.

## 1. Clone and build

```bash
git clone https://github.com/vymalo/vsms.git
cd vsms
just check
```

`just check` is `cargo check --workspace --all-targets` with the build-concurrency cap this repo's own `justfile` sets. First build compiles the whole dependency tree including `include_server_schema!`'s expansion of `sms-api` — expect a few minutes the first time, seconds after.

## 2. Apply migrations to a scratch database

```bash
createdb vsms_check
export DATABASE_URL=postgres://localhost/vsms_check
./ci/apply-migrations.sh
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f ci/test-state-machine.sql
```

The last command asserts, against a real trigger, that legal state transitions succeed, illegal ones raise `SM001`, terminal states have no exits, and generated ids satisfy CrateStack's format guard. It runs inside a transaction and rolls back — nothing it does is left behind. `ALL ASSERTIONS PASSED` is the expected last line.

## 3. Run the full check suite

```bash
just all-checks
```

fmt, clippy with warnings as errors, the full offline test suite (no `DATABASE_URL` needed — this stays green without one on purpose), the R1 raw-`sqlx` lint, and the R2 state-machine parity check. This is everything CI runs, in CI's order.

## 4. See what's actually here

```bash
just routes
```

Needs no database — prints the full generated REST route table (100+ routes as of this writing) straight from the expanded schema. A fast way to get a feel for the surface area before reading code.

```bash
DATABASE_URL=postgres://localhost/vsms_check \
    cargo test --workspace -- --ignored
```

Runs the live-Postgres suites against the scratch database from step 2 — these are the tests that prove behavior against a real trigger and a real claim loop, not just a type-checker. Safe to run repeatedly against the same database; every suite seeds its own uniquely-suffixed fixtures.

## 5. Run the gateway and worker for real

Two one-time steps before either binary will start. First, an OP signing key — `sms-gateway serve` refuses to start without one:

```bash
DATABASE_URL=postgres://localhost/vsms_check \
    cargo run -p sms-gateway -- rotate-signing-key
```

Second, `sms-gateway serve` also refuses to start until an `active` `Provider` row keyed `orange_cm` exists, **and**, since [#62](https://github.com/vymalo/vsms/issues/62)'s routing rules engine, `sms-worker`'s `dispatch` role needs at least one enabled `Route` pointing at it or it refuses every message with no matching route (a deliberate cutover from routing's old "any active provider" placeholder — `sms-gateway serve` itself still only checks for the `Provider` row at startup, so a deployment missing only the `Route` half starts and reports healthy, then rejects everything it's asked to send). No admin console exists yet to create either by hand, so run something that seeds both *before* starting either binary, not after. Two options: `send_test_message` below also creates both as a side effect (along with the `App`/`AppClient`/`SenderId` fixtures this walkthrough's message needs anyway), or, if you only want the `Provider`/`Route` pair and nothing else, `sms-gateway seed-dispatch` (#148, renamed from `seed-provider` and extended to also seed the `Route` in #62) does exactly that — idempotent on both halves, no message/fixtures attached:

```bash
DATABASE_URL=postgres://localhost/vsms_check \
    cargo run -p sms-gateway -- seed-dispatch
```

This walkthrough uses `send_test_message` below instead, since it needs the message/App/AppClient fixtures regardless and `send_test_message` already creates the `Provider`/`Route` pair along with them — no need to run both.

It also needs `SMS_HASH_PEPPER` — the server-held key behind `msisdnHash`/`bodyHash`
(#134, `sms_api::pepper`). Required, minimum 32 bytes, and it **must be the same value
for every process in this walkthrough**: a hash computed under one pepper never equals one
computed under another, and nothing detects the mismatch — opt-out matching and dedupe just
silently stop working. So export it once, here, rather than generating one per command:

```bash
export SMS_HASH_PEPPER="$(openssl rand -base64 48)"

DATABASE_URL=postgres://localhost/vsms_check \
    cargo run -p sms-api --example send_test_message -- \
    --to +237677123456 --sender-id VYMALO --body "Hello from vsms"
```

Idempotent — safe to run again later to send a second message; it reuses the `App`/`Provider`/`SenderId` fixtures it created the first time rather than accumulating new ones.

Now, in separate terminals:

```bash
# terminal 1 — the API server
DATABASE_URL=postgres://localhost/vsms_check \
SMS_OIDC_ISSUER=http://127.0.0.1:8080 \
SMS_HASH_PEPPER="$SMS_HASH_PEPPER" \
ORANGE_CM_CLIENT_ID=placeholder \
ORANGE_CM_CLIENT_SECRET=placeholder \
ORANGE_CM_SENDER_NUMBER=+2370000 \
    cargo run -p sms-gateway -- serve

# terminal 2 — dispatch, scheduler, and jobs
DATABASE_URL=postgres://localhost/vsms_check \
SMS_WORKER_ROLES=dispatch,scheduler,jobs \
ORANGE_CM_CLIENT_ID=placeholder \
ORANGE_CM_CLIENT_SECRET=placeholder \
ORANGE_CM_SENDER_NUMBER=+2370000 \
    cargo run --bin sms-worker
```

`cargo run --bin sms-worker`, not `-p sms-worker` — the package is named `sms-worker-bin` (the *library* crate `crates/sms-worker` already owns the plain name), but the `[[bin]]` it produces is still called `sms-worker`, and `--bin` resolves by binary name across the whole workspace regardless of which package declares it.

Placeholder Orange credentials are enough to prove the whole pipeline moves the message you already sent through `accepted → queued → routed`: routing only needs an `active` `Provider` row and a matching, enabled `Route` (since [#62](https://github.com/vymalo/vsms/issues/62); `send_test_message` seeds both), not a real Orange account. `routed` is where dispatch actually attempts a submission — with placeholder credentials that attempt fails outright (a real `401` from Orange's own OAuth endpoint, since `ORANGE_CM_BASE_URL` defaults to the real `https://api.orange.com`), landing the message in `failed` with the rejection reason attached, never reaching `submitted`. That's expected, and exactly the boundary [docs/runbooks/36-handset-gate.md](36-handset-gate.md) exists to cross with real credentials. Watch it with the `psql` command `send_test_message` printed:

```text
 id  | state  | provider_message_ref |                     state_reason
-----+--------+-----------------------+-------------------------------------------------------
 ... | failed |                       | oauth_401: token endpoint rejected credentials: ...
```

## 6. See it reach `delivered` without a real Orange account

Everything above proves routing; it can't prove delivery, because `routed` is exactly
where a placeholder credential dies. `app/sms-fake-orange` closes that gap for a demo or
local run — it's a standalone process that impersonates Orange's token and submit
endpoints and independently POSTs a `delivered` DLR back, so `sms-worker`/`sms-gateway`
need no code change to talk to it, only `ORANGE_CM_BASE_URL` pointed elsewhere. **It is a
development/demo tool, not a real provider — never point a production deployment at it**
(see the binary's own module doc, `app/sms-fake-orange/src/main.rs`).

```bash
# terminal 3 — the fake, bound on its own port
cargo run -p sms-fake-orange-bin -- \
    --bind-addr 127.0.0.1:8090 \
    --dlr-endpoint http://127.0.0.1:8080/dlr/orange_cm \
    --sender-number +2370000
```

Then start terminals 1 and 2 exactly as in step 5, but with
`ORANGE_CM_BASE_URL=http://127.0.0.1:8090` added to both (still with placeholder
`ORANGE_CM_CLIENT_ID`/`ORANGE_CM_CLIENT_SECRET` — the fake accepts any credentials by
default) and send a fresh message. Watch the same `psql` query this time land on
`delivered`, and `app/sms-fake-orange`'s own log show the submit it received and the DLR
it posted back. `--fault-mode seeded --seed <n>` drives the same weighted failure mix
`crates/sms-worker`'s chaos suite uses, for demoing the interesting paths instead of the
happy one.

This still doesn't close [`36-handset-gate.md`](36-handset-gate.md) — nothing here proves
Orange's real DLR payload shape, or that a handset actually buzzes.

## 7. Provision a client the console (or any HTTP caller) can actually use

Steps 1–5 prove the pipeline moves a message end to end, but nothing in them produces a credential anything *outside* this process could authenticate with: `sendMessage` there runs through `send_test_message`'s own in-process `Procedures` call, not over HTTP. `provisionAppClient`'s own policy (`hasRole('owner') || hasRole('admin')`) also means no token `sms-gateway serve` can actually issue is ever allowed to call it — see `AGENTS.md`'s notes on `GatewayAuth` always minting `role: "app"`. `sms-gateway provision-client` closes that gap: it calls the real `provisionAppClient` procedure directly, under an operator-supplied `owner`/`admin` context, and hands back a private key file a real `/token` exchange can use.

```bash
# `SMS_HASH_PEPPER` is already exported from step 5 — provision-client requires it
# too, and it must be the same value every other process here uses.
DATABASE_URL=postgres://localhost/vsms_check \
    cargo run -p sms-gateway -- provision-client \
    --app-id <an App.id, e.g. from `psql ... -c 'select id from app'`> \
    --label "admin console" \
    --scope sms:send --scope sms:read \
    --scope job:read --scope job:enqueue --scope worker:read \
    --scope provider:read --scope route:read --scope dashboard:read \
    --key-out ./console-client-key.pem
```

`--key-out` is created with `0600` permissions and the command refuses to run if that path already exists — it will not overwrite a key still in use. The private key is written there once and printed nowhere, including on failure; only the client id and the key path are echoed:

```text
provisioned client: appc_...
private key written to: ./console-client-key.pem

paste into the console (or any other machine caller)'s environment:
  SMS_CONSOLE_CLIENT_ID=appc_...
  SMS_CONSOLE_PRIVATE_KEY_PATH=./console-client-key.pem
```

Those two lines are exactly `.env.example`'s `SMS_CONSOLE_CLIENT_ID` / `SMS_CONSOLE_PRIVATE_KEY_PATH` — paste them (plus `SMS_AUTH_ISSUER` matching whatever `--issuer` `serve` was started with, and a `SMS_CONSOLE_SCOPE` drawn from `--scope` above) straight into the admin console's own environment. `app/sms-gateway/tests/provision_client_cli_live_postgres.rs` is this command's own live acceptance test: it runs the real binary against a real Postgres, then spawns a genuinely separate `sms-gateway serve` process and proves the key file it wrote completes a real `private_key_jwt` exchange at `/token` and an authenticated `sendMessage` call — the exact thing `crates/sms-api/examples/send_test_message.rs` does *not* prove, since the `AppClient` row it writes directly has no `OauthClient.jwks` and could never complete that exchange.

## 8. Run the admin console against it

Everything above proves the gateway/worker chain over HTTP; nothing in steps 1–7 actually
starts the Next.js console. This step was never written down before — the first end-to-end
dry run of this whole chain found the gap.

Prerequisites: **Node >= 22** and **pnpm 11** (`packageManager` in the root `package.json`
pins the exact version; `corepack enable` will resolve it automatically).

```bash
# from the repo root — this is a pnpm workspace, not just admin/
pnpm install
```

Then create `admin/.env.local` (gitignored — `.env.example` at the repo root is the
template, but the console's own required keys are the ones listed in
`packages/env/src/index.ts`) with the values `provision-client` printed above:

```text
SMS_API_URL=http://127.0.0.1:8080

SMS_AUTH_ISSUER=http://127.0.0.1:8080
SMS_CONSOLE_CLIENT_ID=appc_...                        # from provision-client's output
SMS_CONSOLE_PRIVATE_KEY_PATH=/absolute/path/to/console-client-key.pem
SMS_CONSOLE_SCOPE=sms:send sms:read job:read job:enqueue worker:read provider:read route:read dashboard:read

# The human login flow (#194). DASHBOARD_AUTH is gone — it was replaced by
# real sessions, not supplemented, so there is no bypass mode any more and
# these three are required. ADMIN_BASE_URL must be the literal origin you
# open in a browser: the redirect_uri is matched exactly, not by prefix.
ADMIN_BASE_URL=http://127.0.0.1:3100
SMS_CONSOLE_OIDC_CLIENT_ID=sms-console
SMS_CONSOLE_SESSION_SECRET=at-least-32-characters-of-real-entropy-here

MESSAGE_STREAM_POLL_MS=2000

NEXT_PUBLIC_APP_NAME=vsms Admin Console
NODE_ENV=development
```

`SMS_CONSOLE_PRIVATE_KEY_PATH` must be an absolute path (or resolve correctly relative to
`admin/`, wherever Next actually runs from) — `provision-client --key-out` above accepts a
relative path, but the console reads the file at request time from its own working
directory, not the shell's.

Then register the console's OIDC client and create an account to sign in with —
without both, the console starts and immediately redirects every page back to
`/login` with nothing able to satisfy it:

```bash
DATABASE_URL=postgres://localhost/vsms_dev ./target/debug/sms-gateway seed-console-client \
  --client-id sms-console \
  --redirect-uri http://127.0.0.1:3100/api/auth/callback

DATABASE_URL=postgres://localhost/vsms_dev ./target/debug/sms-gateway provision-user \
  --email you@example.com --display-name "Your Name" --role-key owner
```

The `--redirect-uri` must equal `${ADMIN_BASE_URL}/api/auth/callback` character for
character — RFC 6749 §3.1.2 exact matching, so a trailing slash breaks it. `--role-key`
accepts any of §5.2's six built-in roles, all seeded by `0002_bootstrap`.
`provision-user` prints a generated password once and never stores it.

```bash
pnpm --filter admin exec next dev -p 3100
```

`next dev` does not read a `PORT` value out of `.env.local` when choosing which port to
bind — that file is only loaded once the server process is already up. Pass `-p` explicitly
(or export `PORT` in the shell that launches it) rather than relying on the env file.

Open `http://localhost:3100/`, sign in with the account you just provisioned, send a message from the composer, and watch it move to
`delivered` on `http://localhost:3100/messages` (polling, ~2s per `MESSAGE_STREAM_POLL_MS`
above) — confirmed working end to end, composer through `sms-fake-orange`'s DLR, in the
session that added this section.

## Where to go next

- [`docs/architecture.md`](../architecture.md) — the full design: data model, provider abstraction, worker topology, security, compliance, and every framework constraint that shaped a decision.
- [`CONTRIBUTING.md`](../../CONTRIBUTING.md) — the three engineering rules (R1/R2/R3) the codebase is written against, and the schema-change workflow.
- [`AGENTS.md`](../../AGENTS.md) — current project status: what's built, what's still open, and every non-obvious thing found by actually running the toolchain rather than reading its docs.
- [`docs/runbooks/`](README.md) — other step-by-step procedures, including the real-handset acceptance gate.

## Cleaning up

```bash
dropdb vsms_check
```

If you started Postgres in Docker for this, stop that container too — nothing here needs it to stay running between sessions.
