# Runbook: #160 — the joined integration story

Two halves of the integration story were proven separately and never joined:
[#144](https://github.com/vymalo/vsms/issues/144) proved a message sent from the admin
console's own composer reaches `delivered`, and
[#149](https://github.com/vymalo/vsms/issues/149) proved a third-party Rust backend can
complete a `private_key_jwt` exchange over real HTTP and send. Neither proved the thing a
customer actually cares about: **an external client sends, and an operator sees that exact
message in the dashboard reaching `delivered`.** This runbook is that observation, and
`scripts/e2e-integration.sh` is the one command that reproduces it.

```bash
just e2e-integration
# or directly:
./scripts/e2e-integration.sh
```

It is self-contained and safely rerunnable — it tears down and rebuilds the demo stack
itself (`scripts/demo.sh down` then `up`) before every run, so running it twice in a row
with no manual cleanup in between is the normal way to use it, not a special "cold reset"
mode. It exits non-zero at the first broken link, naming the step.

## What it does

1. **Brings the stack up** via `scripts/demo.sh` (reused, not reimplemented) — a scratch
   Postgres, `sms-gateway`, `sms-worker` (`dispatch,scheduler,jobs`), `sms-fake-orange`,
   and the admin console, plus one `App` and a "demo console" `AppClient`.
2. **Provisions a SECOND, independent `AppClient`** — "external integrator" — against
   that same `App`, via `sms-gateway provision-client`. Two real credentials: different
   `clientId`, different RSA keypair, each provisioned separately. See "Why the same
   `App`" below for why this is the right design, not a shortcut.
3. **Sends as the integrator, over real HTTP**, via `examples/rust/sms-send` — its own
   `private_key_jwt` exchange at `POST /token`, then `POST /$procs/sendMessage` with the
   resulting Bearer token. A `--client-ref` unique to the run is attached so the later
   assertion matches *the* message, not *a* message (#160's own acceptance criterion 5).
4. **Mints a third, independent access token** — the console's own credential, read
   straight from `admin/.env.local` (`SMS_CONSOLE_CLIENT_ID` /
   `SMS_CONSOLE_PRIVATE_KEY_PATH`), the exact identity the admin console's Next.js server
   holds. The script hand-signs the RFC 7523 assertion with `openssl dgst -sign` rather
   than pulling in a JWT library — this repo's shell scripts are bash-only by convention,
   and the scheme is a direct, field-for-field mirror of
   `packages/gateway/src/token.ts`'s own `mintAssertion` (same claims, same 60s TTL).
5. **Polls `GET /messages/{id}` as the console**, once a second, until that exact id
   reaches `delivered` (or a terminal non-delivered state, or a 60s timeout — either
   fails the script loudly). This is the same route
   `packages/gateway/src/messages.ts`'s `getMessageById` calls — not a database query.
   Every poll also asserts the returned `appId` matches the App both clients share.

The script prints the exact message id, the App id, both client ids, the observed state
progression, and a direct browser link
(`http://127.0.0.1:3100/messages?clientRef=<the run's clientRef>`) for manual/visual
confirmation.

## Why the same `App` for both clients, not two

`Message`'s own row policy in `schema/schema.cstack` is:

```
@@allow("list", auth().kind == "user" || appId == auth().appId || hasRole('system'))
@@allow("detail", auth().kind == "user" || appId == auth().appId || hasRole('system'))
```

No `auth().kind == "user"` token exists anywhere in this deployment — `GatewayAuth` only
ever mints `role: "app"`/`"system"` (`AGENTS.md`'s M1 section: no human-login flow exists
yet). So the console's own credential is, today, just another `App`-scoped principal —
not a cross-tenant "operator" one. `admin/app/messages/messages-screen.tsx` already says
this on screen, verbatim: *"Scoped to this app only — the console's own service-account
token can only read the one app it belongs to, so there is nothing to switch to. This is
not a filter and not a bug."*

Given that, provisioning the integrator under a **different** `App` would not test
anything this deployment claims to support — it would just rediscover the documented
scope cut above (a `GET /messages/{id}` for another `App`'s message returns `404`,
per `messages.ts`'s own module doc, point 9 — confirmed live during #94/#96/#121 already
and unchanged here). It would not be a "harder" or "more honest" test; it would be testing
a claim nobody is making.

Provisioning both under the **same** `App` is what actually matches the product story:
a tenant's own console access and a tenant's own backend integration are two separate
credentials — different `clientId`, different private key, independently revocable,
independently scoped (`sms:send`/`sms:read` per client) — that both legitimately act for
that one tenant. That is a genuine, previously-unproven claim: before this runbook,
nothing had shown that a **different** `AppClient` than the one that sent a message can
read it back through `GET /messages`, over real HTTP, under the same `App`. A
implementation that (incorrectly) scoped `Message` visibility by `clientId` instead of
`appId` would have failed this exact scenario while still passing every existing
single-client live suite — this closes that gap.

**What this does not prove**, and is out of scope for #160: cross-tenant "vendor
operator sees every customer's traffic" visibility. That needs a real human-login role
(`auth().kind == "user"`), which does not exist yet — tracked separately, not by this
issue. If that role ever lands, the interesting version of this scenario becomes
"provision the two clients under *different* `App`s and confirm an operator token *can*
see both" — worth writing then, not now.

## Orange is faked, on purpose, and that boundary is unchanged

`sms-fake-orange` impersonates Orange Cameroon's submit/token endpoints and its own DLR
delivery — no real SMS is sent to any real handset. This proves **integration
readiness** (the credential, transport, and visibility chain all work), not **carrier
readiness**. [`36-handset-gate.md`](36-handset-gate.md) — a real Orange account and a
real phone, run by a human — remains the actual carrier gate and is untouched by this
runbook.

## Evidence — two full runs, both clean, both from `scripts/e2e-integration.sh`'s own
teardown-then-rebuild (no manual cleanup between them)

**Run 1** — message id `c0a36ef1418ecda823639f5`, `clientRef=e2e-1786259725-aa11c2a6`:

```
    [09:15:26] state=accepted
    [09:15:27] state=queued
    [09:15:28] state=submitted
    [09:15:30] state=delivered
```

Confirmed in a real browser (Chrome, via the console at
`http://127.0.0.1:3100/messages?clientRef=e2e-1786259725-aa11c2a6`) — the rendered table
showed exactly one row: status **Delivered**, recipient `+237 6 77 00 02 22` (MTN), client
ref `e2e-1786259725-aa11c2a6`, sender `VYMALO`, id `c0a36ef` (the `IdDisplay` component's
first-7-chars table view — matches the full id's own prefix). Screenshot captured during
this session.

**Run 2** (immediately after Run 1, no manual `demo.sh down` run by hand — the script's
own `down` did it) — message id `c0aefd45f57b147545eb4d8`,
`clientRef=e2e-1786259847-95a08151`:

```
    [09:17:28] state=queued
    [09:17:29] state=submitted
    [09:17:31] state=delivered
```

(`accepted` was already past by the first poll this run — the claim/dispatch loop had
already advanced it in the ~1s between the read-back inside `sms-send` and this script's
first poll; the state machine invariant this script actually asserts, "no terminal
non-`delivered` state, no timeout," held regardless.) Also confirmed in a real browser at
`http://127.0.0.1:3100/messages?clientRef=e2e-1786259847-95a08151` — one row, status
**Delivered**, id `c0aefd4`, matching the full id's prefix.

Both runs: `appId` on every poll response matched the App both the console and integrator
clients were provisioned against, and neither run ever saw a `404` from
`GET /messages/{id}` under the console's credential — i.e., same-tenant, cross-client
visibility held in both passes, not just once.

## What was not verified

- Real Orange delivery, a real handset, or a real `kill -9` against a live Orange account
  — [`36-handset-gate.md`](36-handset-gate.md)'s own scope, unaffected by this work.
- Cross-`App` (cross-tenant) visibility under a genuine human-login role — that role does
  not exist in this deployment yet (see "Why the same `App`" above).
- Concurrent runs of this script on one machine — like `scripts/demo.sh` itself, the
  Postgres container name and ports are fixed and global; don't run two copies of this
  scenario against each other on the same host.
