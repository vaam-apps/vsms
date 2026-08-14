# Runbook: #160 — the joined integration story

Two halves of the integration story were proven separately and never joined:
[#144](https://github.com/vymalo/vsms/issues/144) proved a message sent from the admin
console's own composer reaches `delivered`, and
[#149](https://github.com/vymalo/vsms/issues/149) proved a third-party Rust backend can
complete a `private_key_jwt` exchange over real HTTP and send. Neither proved the thing a
customer actually cares about: **an external client sends, and an operator sees that exact
message in the dashboard reaching `delivered`.** This runbook is that observation.

```bash
just e2e-integration
```

**Rewritten as part of the `containerize-tooling` PR** — `scripts/e2e-integration.sh` (bash,
`openssl dgst -sign`, `curl`, `jq`) is gone, replaced by [`ci/e2e-integration`](../../ci/e2e-integration),
a small Rust binary reusing `vsms-sdk-rust`'s own `private_key_jwt` token exchange rather
than hand-signing RFC 7523 assertions in shell a second time. `just e2e-integration` is
self-contained and safely rerunnable — it brings up `compose.dev.yaml` fresh (`just demo`,
which itself wipes and recreates every named volume) before every run, so running it twice
in a row with no manual cleanup in between is the normal way to use it, not a special
"cold reset" mode. It exits non-zero at the first broken link, naming the step.

## What it does

1. **Brings the stack up** via `just demo` — every service as a container built from this
   checkout's own source (`compose.dev.yaml`), including a scratch Postgres,
   `sms-gateway`, `sms-worker` (`dispatch,scheduler,jobs`), `sms-fake-orange`, and the
   admin console, plus one `App` (`vsms-demo`) and a "demo console" `AppClient`.
2. **Provisions a SECOND, independent `AppClient`** — "external integrator" — against
   that same `App`, via `docker compose run --rm sms-gateway provision-client`. Two real
   credentials: different `clientId`, different RSA keypair, each provisioned separately.
   See "Why the same `App`" below for why this is the right design, not a shortcut.
3. **Sends as the integrator, over real HTTP**, via `ci/e2e-integration` — its own
   `private_key_jwt` exchange at `POST /token` (through `vsms-sdk-rust`'s
   `PrivateKeyJwtTokenStore`), then `POST /$procs/sendMessage` with the resulting Bearer
   token. A `--client-ref` unique to the run is attached so the later assertion matches
   *the* message, not *a* message (#160's own acceptance criterion 5). Runs *inside*
   `compose.dev.yaml`'s own Compose network, reaching `sms-gateway` at its internal DNS
   name — see that tool's own module doc for why it can't run as a bare host process
   against this specific stack (a real, live-verified limitation, not an assumption:
   `sms-gateway`'s configured OIDC issuer has to be the internal Compose DNS name for the
   admin console's own login flow to work at all, and a `private_key_jwt` client assertion's
   audience has to match that exact issuer — a host process can only ever *connect* to the
   host-published port, and `vsms-sdk-rust`'s `base_url`/`issuer` split turned out not to
   decouple those two the way its own doc comment claims; see the tool's module doc for the
   live 401 that proved it).
4. **Builds a second, independent client** — the console's own credential, extracted from
   the same Compose stack via `docker compose cp` (its long-lived `provision-client`
   container, still present after `up`, unlike the integrator's `run --rm` one) — the exact
   identity the admin console's Next.js server holds.
5. **Polls `GET /messages/{id}` as the console**, once a second, until that exact id
   reaches `delivered` (or a terminal non-delivered state, or a 60s timeout — either
   fails the tool loudly). This is the same route
   `packages/gateway/src/messages.ts`'s `getMessageById` calls — not a database query.
   Every poll also asserts the returned `appId` matches the App both clients share.

The tool prints the exact message id, the App id, both client ids, and the observed state
progression.

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

## Evidence — the `containerize-tooling` rewrite

**The bash-era evidence this section used to report is superseded, not repeated** — the
mechanism changed (a Rust tool running in-network instead of a shell script hand-signing
JWTs), so it needed re-proving, not just re-describing. Two full runs, both clean, both via
`just e2e-integration` (which itself tears down and rebuilds `compose.dev.yaml` before each):

**Run 1** — message id `c49cd97618d8af7447270ef`, `clientRef=e2e-1786719471`:

```
    [14:57:51] state=accepted
    [14:57:52] state=queued
    [14:57:53] state=submitted
    [14:57:55] state=delivered
```

**Run 2** (immediately after Run 1, `just e2e-integration`'s own `just demo` prerequisite
did the teardown-and-rebuild, no manual cleanup) — message id `cfae30185525385c9e40056`,
`clientRef=e2e-1786719791`:

```
    [15:03:11] state=accepted
    [15:03:12] state=queued
    [15:03:13] state=submitted
    [15:03:15] state=delivered
```

Both runs: `appId` on every poll response matched the App both the console and integrator
clients were provisioned against, and neither run ever saw a `404` from
`GET /messages/{id}` under the console's credential — i.e., same-tenant, cross-client
visibility held in both passes, not just once. Both ended `PASSED`, printing the full
`accepted -> queued -> submitted -> delivered` progression.

## What was not verified

- Real Orange delivery, a real handset, or a real `kill -9` against a live Orange account
  — [`36-handset-gate.md`](36-handset-gate.md)'s own scope, unaffected by this work.
- Cross-`App` (cross-tenant) visibility under a genuine human-login role — that role does
  not exist in this deployment yet (see "Why the same `App`" above).
- Concurrent runs of `just e2e-integration` on one machine — like `just demo` itself, the
  Compose project name and ports are fixed; don't run two copies of this scenario against
  each other on the same host.
- A real browser confirmation via the console UI (the bash-era version of this doc reported
  one) — not re-run this time, since the underlying claim (`GET /messages/{id}` returning
  the right row under the console's credential) is exactly what `ci/e2e-integration`'s own
  poll loop already asserts programmatically on every run; the console UI reads the same
  route.
