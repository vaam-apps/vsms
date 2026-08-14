# vsms integration examples

Runnable, copy-pasteable examples of the thing nothing else in this repo
demonstrates: **a third-party backend calling vsms over real HTTP.**
`crates/sms-api/examples/send_test_message.rs` calls
`Procedures::send_message` in-process — useful for seeding fixtures, but it
never proves the `private_key_jwt` token exchange or the REST surface an
actual integrator uses. These do. See
[#149](https://github.com/vymalo/vsms/issues/149).

Both examples do the identical, real thing, end to end:

1. Read the PEM `sms-gateway provision-client` wrote.
2. Sign an RFC 7523 §3 `private_key_jwt` client assertion.
3. Exchange it at `POST {issuer}/token` for a `client_credentials` access
   token.
4. Call `POST {issuer}/$procs/sendMessage` with that Bearer token.
5. Read the message back with `GET {issuer}/messages/{id}` and print its
   state.

Both mirror `packages/gateway/src/token.ts` — the admin console's own
token acquisition — for steps 1-3, rather than inventing a second
interpretation of the same exchange.

**Both examples now do steps 1-3 by depending on official SDKs**
(`vsms-sdk-rust` under `sdks/rust/vsms-sdk-rust`, [#171](https://github.com/vymalo/vsms/issues/171);
`@vsms/sdk` under `sdks/node/vsms-sdk-node`, [#242](https://github.com/vymalo/vsms/issues/242))
instead of hand-rolling them — see each SDK's own docs for what it
owns (the `private_key_jwt` credential lifecycle) and this file's own
"Design decisions" section below for what that did to each example's size.

## Layout

```
examples/
├── README.md              this file — the only doc either example needs
├── rust/                  a separate Cargo workspace (own [workspace], root
│   │                      Cargo.toml excludes it — never a member of the
│   │                      product workspace)
│   └── sms-send/          cargo run -p vsms-example-send
├── pnpm-workspace.yaml     a separate pnpm workspace (packages: "node/*"),
├── package.json            never a member of the root pnpm workspace
└── node/
    └── sms-send-example/  node src/index.mjs
```

`examples/rust` and `examples/pnpm-workspace.yaml`/`examples/package.json`
are the shared scaffolding this PR owns. `examples/node/*` is a glob —
[#150](https://github.com/vymalo/vsms/issues/150) is concurrently adding
`examples/node/webhook-receiver` as a sibling package under it, and does
not need to touch the workspace file to do so.

Both are genuinely separate from the product's own workspaces:

- **Rust:** `examples/rust/Cargo.toml` declares its own `[workspace]`.
  The root `Cargo.toml` also lists `exclude = ["examples/rust"]` so the
  boundary holds even if `members` above ever grows a glob. `cargo check
  --workspace`/`cargo test --workspace` run from the repo root never see
  it; `examples/rust/target/` has its own `.gitignore` entry, since the
  root's `/target` pattern is anchored to the repo root only.
- **Node:** `examples/pnpm-workspace.yaml` (packages: `node/*`) is a
  workspace root of its own, separate from the repo root's
  `pnpm-workspace.yaml` (`admin`, `packages/*`). `pnpm install` run from
  `examples/` produces its own `pnpm-lock.yaml` here and never touches
  `admin/`'s or `packages/*`'s.

## Running either example

Both need a live gateway and a provisioned credential. The fastest path is
`just demo` from the repo root (see `docs/runbooks/getting-started.md`),
which brings up a scratch Postgres, `sms-gateway`, `sms-worker`,
`sms-fake-orange` (an impersonation of Orange Cameroon — no real SMS is
ever sent), and prints/writes a provisioned client at
`.demo/console-client-key.pem`. `sms-gateway routes` needs no database and
is useful for sanity-checking the route surface these examples call.

If you'd rather provision your own client by hand against any running
gateway:

```bash
./target/debug/sms-gateway provision-client \
  --app-id <an existing, active App.id> \
  --label "integration example" \
  --scope sms:send --scope sms:read \
  --key-out /path/to/client-key.pem
```

prints `provisioned client: <clientId>` and writes the private key once,
`0600`, to `--key-out`.

### Rust

```bash
cd examples/rust
cargo run -p vsms-example-send -- \
  --issuer http://127.0.0.1:8080 \
  --client-id <clientId provision-client printed> \
  --private-key-path /path/to/client-key.pem \
  --to +237677123456 \
  --sender-id VYMALO \
  --body "Hello from the vsms Rust example" \
  --client-ref rust-example-1
```

Every flag also reads from an env var (`VSMS_ISSUER`, `VSMS_CLIENT_ID`,
`VSMS_PRIVATE_KEY_PATH`, `VSMS_SCOPE`).

### Node.js

```bash
cd examples/node/sms-send-example
pnpm install    # or: cd examples && pnpm install (installs the whole workspace)
node src/index.mjs \
  --issuer http://127.0.0.1:8080 \
  --client-id <clientId provision-client printed> \
  --private-key-path /path/to/client-key.pem \
  --to +237677123456 \
  --sender-id VYMALO \
  --body "Hello from the vsms Node example" \
  --client-ref node-example-1
```

Same env vars as the Rust example (`VSMS_ISSUER`, `VSMS_CLIENT_ID`,
`VSMS_PRIVATE_KEY_PATH`, `VSMS_SCOPE`).

## Verified against a live stack

Both examples were run against a real `sms-gateway` + `sms-worker` +
`sms-fake-orange` stack (a scratch Postgres and dedicated ports — `just
demo`'s own default ports/container were deliberately avoided, in case a
concurrent session was using them; see "Ports" below), not just compiled.
Both messages were sent, then independently confirmed via `psql` to have
reached `delivered` (`sms-worker`'s `dispatch` role picked them up and
`sms-fake-orange` delivered them, exactly like `just demo`'s own
composer-to-`/messages` path):

| Example | messageId | state after send | state after dispatch | providerMessageRef |
|---|---|---|---|---|
| Rust | `c689dc4d33ed19311d7a1aa` | `accepted` | `delivered` | `res-c689dc4d33ed19311d7a1aa` |
| Node.js | `c989d795a7c0509a07bc140` | `accepted` | `delivered` | `res-c989d795a7c0509a07bc140` |

The `--client-ref` dedupe path was also exercised on both: resending the
identical `clientRef` (`rust-example-verify-1` / `node-example-verify-1`)
returned `409 Conflict` — `"duplicate key value violates unique
constraint \"messages_app_idem_key\""` — rather than sending a second
message. See "Idempotency" below for exactly what this does and doesn't
prove.

## Design decisions

### Rust: `vsms-sdk-rust`, not a hand-rolled token dance

`examples/rust/sms-send/src/main.rs` used to hand-roll RFC 7523 assertion
signing, the `/token` exchange, and token caching itself (~230 lines of
it) — the exact duplication [#171](https://github.com/vymalo/vsms/issues/171)
was filed to stop, since `packages/gateway/src/token.ts` and
`app/sms-gateway/tests/provision_client_cli_live_postgres.rs` each
implemented the identical dance separately. It now depends on
`sdks/rust/vsms-sdk-rust` (a path dependency here — a real integrator
outside this monorepo would depend on the published crates.io version
instead) and shrank from 377 to 152 lines; `Cargo.toml` dropped
`jsonwebtoken`, `reqwest`, `serde`, `serde_json`, and `uuid` as direct
dependencies (they're still there transitively, via the SDK). See that
crate's own module doc for what's generated (`cratestack::
include_client_schema!` — the model/input/procedure surface) versus
hand-written (the auth layer, which is the part that used to live here).

### Node: `@vsms/sdk`, not a hand-rolled token dance

`examples/node/sms-send-example/src/index.mjs` used to hand-roll RFC 7523
assertion signing, the `/token` exchange, and token caching itself (~230
lines of boilerplate). It now depends on `@vsms/sdk` (`sdks/node/vsms-sdk-node`,
[#242](https://github.com/vymalo/vsms/issues/242)) and shrank from 298 to 110 lines;
the example no longer directly depends on `jose` or hand-rolls token caching,
relying on `@vsms/sdk`'s built-in credential lifecycle, automatic bounded 401
refresh, and typed procedures.

### Idempotency: two independent mechanisms, both live as of #153

`docs/architecture.md` §4.5 documents an HTTP-level `IdempotencyLayer`
(`cratestack::idempotency::IdempotencyLayer`, keyed on an `Idempotency-Key`
request header). **An earlier revision of this section said it wasn't
wired into this deployment — that was true when this section was written,
and is no longer true.** [#153](https://github.com/vymalo/vsms/issues/153)
mounted it in `crates/sms-api/src/router.rs`; sending an `Idempotency-Key`
header now does exactly what §4.5 always said it would.

Both examples demonstrate both mechanisms, and they protect against
different failures:

- **`--client-ref`** (`sendMessage`'s own `clientRef` field,
  `crates/sms-api/src/procedures.rs`) doubles as `idempotencyKey` at the
  database layer, backed by the real `messages_app_idem_key` unique index,
  scoped per `App`. Two `sendMessage` calls under the same `App` with the
  same `clientRef` result in exactly one `Message` row — the second
  attempt fails with `409 Conflict` (verified below, not asserted from
  reading the source). This runs regardless of which HTTP header a caller
  sends, or doesn't.
- **`--idempotency-key`** sets the `Idempotency-Key` HTTP header, handled
  entirely outside procedure code by `IdempotencyLayer`, scoped per
  caller's own `Authorization` header (not per `App`). A repeat within the
  TTL window (24h by default) never re-executes `sendMessage` at all — it
  replays the exact first response (`Idempotency-Replayed: true`). This is
  the one that protects a caller who *doesn't know* whether their first
  attempt landed (a timeout, a dropped connection) — `clientRef` alone
  still protects that case today (the DB-level index doesn't care why a
  second call arrived), but a caller relying only on `clientRef` pays for
  a second full `sendMessage` execution — sender-id resolution, quota
  checks, encoding analysis — before hitting the `409`, where
  `Idempotency-Key` short-circuits before any of that runs.

Reusing an `Idempotency-Key` with a *different* request body/path/method
returns `422 idempotency_key_conflict` rather than either sending or
replaying — both examples print an explanation, not a bare error, when
that fires.

### `aud` on the client assertion

Both examples set `aud` to the token endpoint URL (`{issuer}/token`),
matching `packages/gateway/src/token.ts` exactly. `authkestra` 0.3.2+ also
accepts the bare issuer — the live test suites in `app/sms-gateway/tests/`
use that form — but matching the canonical reference removes one axis of
divergence to debug if the exchange ever starts failing.

## CI coverage — read this before assuming these are checked automatically

**Neither example is exercised by CI today**, and this is a deliberate,
stated gap rather than an oversight to discover later:

- The root `cargo check --workspace` / `cargo clippy --workspace
  --all-targets` / `cargo test --workspace` (`.github/workflows/ci.yml`'s
  `rust` job) only ever see the root workspace's `members` list.
  `examples/rust` is excluded by name and was never added to CI.
- The root `pnpm biome ci .` (`js` job) **does** traverse
  `examples/node/**` — Biome walks the filesystem independent of pnpm
  workspace boundaries — so these files must stay Biome-clean (verified:
  `pnpm exec biome check examples` passes from the repo root). But
  `pnpm turbo run typecheck build test` only sees the root pnpm
  workspace's own packages, so nothing here is built, typechecked, or
  tested by that step.
- No live-gateway run of either example happens in CI. The table above is
  a manually-run, one-time verification, not a regression gate — if a
  future schema or route change breaks either example, CI will not catch
  it.

Wiring a cheap gate (`cargo check` on `examples/rust`; `pnpm install &&
node --check` or a syntax-only pass on `examples/node`) was considered
in scope for a follow-up, not this change — it would need its own
decision about whether to spin up a live gateway (expensive, matches
`live` job's own Postgres-in-CI pattern) or stay compile/lint-only
(cheap, but doesn't catch a route or schema drift the way the manual run
above would). Left as an explicit open question rather than silently
adding an untested workflow step.

## Ports used for the live verification above

`vsms-examples-verify-postgres` (removed after verification), gateway on
`127.0.0.1:8199`, `sms-fake-orange` on `127.0.0.1:8198`, Postgres on
`15499` — chosen to avoid `just demo`'s own defaults (`8080`/`8090`/`3100`,
Postgres `15433`) in case another session was using them concurrently.
None of this is left running; rerun the "Running either example" steps
above (or `just demo`) to reproduce.
