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

Second, `sms-gateway serve` also refuses to start until an `active` `Provider` row keyed `orange_cm` exists — nothing seeds it automatically, and no admin console exists yet to create one by hand. The seed/send tool built for this milestone's own acceptance testing does it as a side effect, so run it *before* starting either binary, not after:

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

Placeholder Orange credentials are enough to prove the whole pipeline moves the message you already sent through `accepted → queued → routed`: routing only needs an `active` `Provider` row to exist, not a real Orange account. `routed` is where dispatch actually attempts a submission — with placeholder credentials that attempt fails outright (a real `401` from Orange's own OAuth endpoint, since `ORANGE_CM_BASE_URL` defaults to the real `https://api.orange.com`), landing the message in `failed` with the rejection reason attached, never reaching `submitted`. That's expected, and exactly the boundary [docs/runbooks/36-handset-gate.md](36-handset-gate.md) exists to cross with real credentials. Watch it with the `psql` command `send_test_message` printed:

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
