Seeds the one `App` + approved `SenderId` a GHCR-only showcase compose
stack (`compose.demo.yaml`) needs before `sms-gateway provision-client`
can run at all.

**This used to be `sms-gateway seed-demo-app` (`Command::SeedDemoApp`,
`app/sms-gateway/src/main.rs`).** Moved out into its own binary, its own
crate, and its own image (`ghcr.io/vaam-store/vsms/demo`) — the maintainer's
own words on the image-hygiene PR that did this: "Images should be tiny
and have ONLY the core business logic. Demo stuffs, never! Unless it's
rust -> binary, it should never be in production images." Roughly 200
lines of demo-only fixture-seeding logic (this file) were compiled into
every production `sms-gateway` image before this — every real deployment
shipped code it could never legitimately run (`App`/`SenderId` create
policy admits only `hasRole('owner') || hasRole('admin')`, and no
production bootstrap path ever calls this), purely because it happened
to live in the same `main.rs`.

# Why this is a Rust binary and not a shell script

The obvious-looking alternative — a plain shell script in a tiny Alpine
image — was considered first and rejected, because of what this command
actually has to do, not because Rust is the default. `App`/`SenderId`/
`SenderIdRegistration` creation has to go through `CrateStack`'s generated
delegates (R1: "all data access goes through `CrateStack` delegates. Never
raw `sqlx`" — `CONTRIBUTING.md`) to get the real things this repo's own
§2.0 grammar table promises: `@@allow` policy enforcement (this command
runs under a hand-built `owner`-role [`sms_api::Principal`], the same
"a CLI acting on behalf of a human role" shape `sms-gateway
provision-user`/`record-route-validation` already use — not a rubber
stamp), a real `@@audit` row per write, and `cs_cuid()`-generated ids.
Two ways to get that from outside the `sms-gateway` binary itself:

- **Over the HTTP API**, the way an external integrator would. Rejected:
  `App.create`/`SenderId.create`'s own `@@allow` admit only
  `hasRole('owner') || hasRole('admin')`, and `GatewayAuth::authenticate`
  never mints either role for a machine (`client_credentials`) token —
  only a real human login (#194) can, and `compose.demo.yaml`'s own
  dependency chain runs this seeding step *before* `provision-client`,
  let alone before any human account exists (`provision-user`, gated
  behind the `console` profile and not even started in a backend-only
  run). There is no bearer token this step could present that would ever
  be let through.
- **Raw `psql`**, bypassing R1 entirely. Rejected: it would skip the
  `@@audit` row every other write in this system gets, skip whatever
  `@db_enforce`-backed `CHECK` constraints exist (and silently skip the
  ones that don't — `@regex` is a documented no-op at the DB level,
  AGENTS.md's own "Framework constraints" table), and duplicate
  `App`/`SenderId`/`SenderIdRegistration`'s insert shape as hand-written
  SQL that would drift the moment the schema does. A demo tool getting
  this wrong fails quietly by producing fixtures the rest of the stack
  then rejects for reasons that look unrelated — not worth the shortcut
  for a handful of rows.

So this stays exactly what it was inside `sms-gateway`: real
`Cratestack` delegate calls, through `sms-api`'s generated schema types,
under a hand-built context — just compiled into its own tiny binary and
its own image instead of the production one. This is the "unless it's
rust -> binary" carve-out the maintainer's own instruction names
explicitly: what's forbidden is demo logic *inside the production
image*, not Rust itself.

# Demo-only — deliberately not part of any production bootstrap sequence

A real deployment's first `App` is a business decision (quota, IP
allowlist, a `SenderId` actually approved by a real provider account)
this command cannot make on an operator's behalf; every production
runbook creates it by hand or through the console, once one exists
(#52/#58's App CRUD screen). The fixed defaults below — an
auto-approved `SenderIdRegistration` with no real provider approval
behind it — exist only to unblock the showcase. This is the GHCR-only
equivalent of `backends/crates/sms-api/examples/send_test_message.rs`'s own
fixture-seeding half; that binary is a `cargo run --example`, never
published as a GHCR image, so a `build:`-free compose stack has no way
to invoke it either.

Idempotent, the same look-up-by-unique-key-then-reuse shape
`sms-gateway seed-dispatch`'s own `create_or_find_provider` already
uses: safe to run on every `docker compose up`. Requires the `Provider`
row named by `--provider-key` to already exist (i.e. `sms-gateway
seed-dispatch` has already run) — `SenderIdRegistration.providerId` is a
real foreign key.
