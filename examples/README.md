# vsms integration examples

Runnable, copy-pasteable examples of the thing nothing else in this repo
demonstrates: **a third-party backend calling vsms over real HTTP.**
`backends/crates/sms-api/examples/send_test_message.rs` calls
`Procedures::send_message` in-process - useful for seeding fixtures, but it
never proves the signed transport or the REST surface an actual integrator
uses. These do. See [#149](https://github.com/vymalo/vsms/issues/149).

Both examples do the identical, real thing, end to end:

1. Read the Ed25519 key (PKCS#8 PEM) and the gateway's two pinned public keys
   (`sms-gateway server-keys`' output).
2. Seal `POST {base-url}/$procs/sendMessage` in a COSE_Sign1 envelope signed
   with the key, bound to the route, the audience `vsms` and a fresh nonce.
3. Open the gateway's answer against the pinned keys. An answer that is not
   sealed, or does not verify, is an error whatever its status.
4. Seal and open `GET {base-url}/messages/{id}` to read the message back and
   print its state.

There is no token exchange and nothing to refresh; the only credential is the
key, and the gateway stores only its public half.

**Both examples do all of that by depending on official SDKs**
(`vsms-sdk-rust` under `sdks/rust/vsms-sdk-rust`, [#171](https://github.com/vymalo/vsms/issues/171);
`@vaam-apps/vsms-node` under `sdks/node/vsms-sdk-node`, [#242](https://github.com/vymalo/vsms/issues/242))
instead of hand-rolling it - see each SDK's own docs for what it owns (the
sealing and the opening) and this file's own "Design decisions" section
below. `examples/node/demo-app` depends on the same SDK by path rather than
from npm, and its image builds the SDK from this checkout.

## Layout

```text
examples/
├── README.md              this file — the only doc either example needs
├── rust/                  a separate Cargo workspace (own [workspace], root
│   │                      Cargo.toml excludes it — never a member of the
│   │                      product workspace)
│   └── sms-send/          cargo run -p vsms-example-send
├── pnpm-workspace.yaml     a separate pnpm workspace (packages: "node/*"),
├── package.json            never a member of the root pnpm workspace
└── node/
    ├── sms-send-example/   node src/index.mjs
    ├── webhook-receiver/   a reference inbound receiver (§4.4 signature
    │                       verification), against a local fabricated emitter
    └── demo-app/           the demo showcase's own evaluator — sends AND
                             receives, against the real compose.dev.yaml/
                             compose.demo.yaml stack, not a local fabrication.
                             See that package's own README.
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
  `pnpm-workspace.yaml` (`admin`, `frontends/packages/*`). `pnpm install` run from
  `examples/` produces its own `pnpm-lock.yaml` here and never touches
  `frontends/apps/admin/`'s or `frontends/packages/*`'s.

## Running either example

Both need a live gateway and a registered key. The fastest path is `just
demo` from the repo root (see `docs/runbooks/getting-started.adoc`), which
brings up a scratch Postgres, `sms-gateway`, `sms-worker`, `sms-fake-orange`
(an impersonation of Orange Cameroon - no real SMS is ever sent). `sms-gateway
routes` needs no database and is useful for sanity-checking the route surface
these examples call.

Register your own key against any running gateway:

```bash
openssl genpkey -algorithm ed25519 -out vsms-signing-key.pem
./target/debug/sms-gateway register-client \
  --app <an existing App.slug> \
  --public-key <the public key the example's --print-public-key printed> \
  --scope sms:send --scope sms:read
./target/debug/sms-gateway server-keys \
  --envelope-signing-key-file current.pem \
  --envelope-next-signing-key-file next.pem > server-keys.txt
```

`register-client` stores the public half only; the private key never leaves
you and there is nothing to return once.

### Rust

```bash
cd examples/rust
cargo run -p vsms-example-send -- --print-public-key --signing-key-path vsms-signing-key.pem
cargo run -p vsms-example-send -- \
  --base-url http://127.0.0.1:8080 \
  --signing-key-path vsms-signing-key.pem \
  --server-keys-path server-keys.txt \
  --to +237677123456 \
  --sender-id VYMALO \
  --body "Hello from the vsms Rust example" \
  --client-ref rust-example-1
```

Every flag also reads from an env var (`VSMS_BASE_URL`,
`VSMS_SIGNING_KEY_PATH`, `VSMS_SERVER_KEYS_PATH`).

### Node.js

The SDK's signing is Rust compiled to WebAssembly, so the SDK has to be built
before the example can load it (`just wasm-tools` once, then):

```bash
pnpm --filter @vaam-apps/vsms-node run build     # from the repo root
cd examples/node/sms-send-example
pnpm install    # or: cd examples && pnpm install (installs the whole workspace)
node src/index.mjs --print-public-key --signing-key-path vsms-signing-key.pem
node src/index.mjs \
  --base-url http://127.0.0.1:8080 \
  --signing-key-path vsms-signing-key.pem \
  --server-keys-path server-keys.txt \
  --to +237677123456 \
  --sender-id VYMALO \
  --body "Hello from the vsms Node example" \
  --client-ref node-example-1
```

Same env vars as the Rust example (`VSMS_BASE_URL`, `VSMS_SIGNING_KEY_PATH`,
`VSMS_SERVER_KEYS_PATH`).

## Verified against a live stack

Both examples were run against a real `sms-gateway` + `sms-worker` +
`sms-fake-orange` stack (a scratch Postgres and dedicated ports — `just
demo`'s own default ports/container were deliberately avoided, in case a
concurrent session was using them; see "Ports" below), not just compiled.
Both messages were sent, then independently confirmed via `psql` to have
reached `delivered` (`sms-worker`'s `dispatch` role picked them up and
`sms-fake-orange` delivered them, exactly like `just demo`'s own
composer-to-`/messages` path):

| Example | messageId                 | state after send | state after dispatch | providerMessageRef            |
| ------- | ------------------------- | ---------------- | -------------------- | ----------------------------- |
| Rust    | `c689dc4d33ed19311d7a1aa` | `accepted`       | `delivered`          | `res-c689dc4d33ed19311d7a1aa` |
| Node.js | `c989d795a7c0509a07bc140` | `accepted`       | `delivered`          | `res-c989d795a7c0509a07bc140` |

The `--client-ref` dedupe path was also exercised on both: resending the
identical `clientRef` (`rust-example-verify-1` / `node-example-verify-1`)
returned `409 Conflict` — `"duplicate key value violates unique
constraint \"messages_app_idem_key\""` — rather than sending a second
message. See "Idempotency" below for exactly what this does and doesn't
prove.

## Design decisions

### Rust: `vsms-sdk-rust`, not a hand-rolled envelope

`examples/rust/sms-send/src/main.rs` once hand-rolled RFC 7523 assertion
signing, the `/token` exchange and token caching itself (~230 lines) - the
exact duplication [#171](https://github.com/vymalo/vsms/issues/171) was filed
to stop. Since the signed transport (VS-3, VS-6) there is no token at all, and
the example depends on `sdks/rust/vsms-sdk-rust` (a path dependency here - a
real integrator outside this monorepo would depend on the published crates.io
version instead) for everything between building a `SendMessageInput` and
reading the result. See that crate's own module doc for what's generated
(`cratestack::include_client_schema!` - the model/input/procedure surface)
versus hand-written (the key, the pinned server keys, and the sealing of
`send_message`).

### Node: `@vaam-apps/vsms-node`, not a hand-rolled envelope

`examples/node/sms-send-example/src/index.mjs` once hand-rolled the same token
dance (~230 lines). It depends on `@vaam-apps/vsms-node`
(`sdks/node/vsms-sdk-node`, [#242](https://github.com/vymalo/vsms/issues/242))
and does nothing but parse flags, build a client and call `sendMessage` /
`getMessage`. Unlike the old SDK, which was TypeScript over `jose`, this one
does not reimplement the envelope: it calls `sdks/rust/vsms-sdk-wasm`, the Rust
SDK's own sealing path compiled to WebAssembly (VS-7), so a Node and a Rust
caller bind the same bytes.

*(Note: In this monorepo, `examples/node/sms-send-example` references `@vaam-apps/vsms-node`
via a local `file:../../../sdks/node/vsms-sdk-node` dependency for local integration
testing prior to registry publication; a third-party integrator should install
`@vaam-apps/vsms-node` directly from npm.)*

### Idempotency: two independent mechanisms, both live as of #153

`docs/architecture.md` §4.5 documents an HTTP-level `IdempotencyLayer`
(`cratestack::idempotency::IdempotencyLayer`, keyed on an `Idempotency-Key`
request header). **An earlier revision of this section said it wasn't
wired into this deployment — that was true when this section was written,
and is no longer true.** [#153](https://github.com/vymalo/vsms/issues/153)
mounted it in `backends/crates/sms-api/src/router.rs`; sending an `Idempotency-Key`
header now does exactly what §4.5 always said it would.

Both examples demonstrate both mechanisms, and they protect against
different failures:

- **`--client-ref`** (`sendMessage`'s own `clientRef` field,
  `backends/crates/sms-api/src/procedures.rs`) doubles as `idempotencyKey` at the
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

### The audience is `vsms`, never the host

A request is bound to an audience, and the gateway refuses one sealed for any
other. It is a *configured name* of the service, `vsms`
(`sms_api::envelope::AUDIENCE`), not the `Host` header or the base URL a caller
happens to reach it by (a proxy rewrites that, and the sender does not control
it), so both SDKs carry it as a constant and a caller sets nothing. A
mismatch is the same coarse unsigned `401` as every other refused signature;
`cargo xtask sdk-parity-check` holds the server's and both SDKs' copies equal.

## demo-app: a third package, a different job

`examples/node/demo-app` is not a third variant of "send one message and
print the result" — it sends AND receives, against the real
`compose.dev.yaml`/`compose.demo.yaml` stack `just demo` brings up, and its
exit code is the actual pass/fail signal those compose files' own `demo-app`
service reports. See that package's own README for what it proves and how
to run it standalone; its typecheck and its copy of the cross-language
signature-vector test (shared with `webhook-receiver`, see
`verbatim-copy.test.ts`) run in `js` CI — unlike the two examples below,
which have no CI gate at all (next section).

## CI coverage — read this before assuming these are checked automatically

**Neither `sms-send-example` (Rust or Node) is exercised by CI today**, and
this is a deliberate, stated gap rather than an oversight to discover later:

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
