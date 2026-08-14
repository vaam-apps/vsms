# better-auth evaluation — should it replace the admin console's session layer?

**Status: evaluation only. No code under `frontends/apps/admin/`, `frontends/packages/`, or `crates/` changed by
this document or its PR.** A console redesign is in flight across several agents and
already owns `frontends/apps/admin/`; this document changes nothing there.

**Question asked:** should [`better-auth`](https://www.better-auth.com/) replace the
admin console's hand-rolled session layer — the OIDC relying-party mechanics in
`frontends/apps/admin/middleware.ts`, `frontends/apps/admin/lib/oidc.ts`, `frontends/apps/admin/lib/session.ts`, and
`frontends/apps/admin/app/api/auth/**`?

**Scoped deliberately, and the boundary is fixed, not re-litigated here.** Identity and
token issuance stay in Rust and Postgres — `User`, `UserCredential`, `Role`, Argon2id
hashing in `backends/crates/sms-auth/src/login.rs`, and `authkestra-op` (`backends/crates/sms-auth/src/op.rs`)
minting the tokens `backends/crates/sms-api/src/auth.rs`'s `GatewayAuth` validates. **R4 is the
reason** (`CONTRIBUTING.md`): a deployment with no console at all must still authenticate
humans and validate tokens, so identity cannot live in a TypeScript library that only
ships with the console. This document does not propose moving it, and does not propose a
design where better-auth owns a user table. What's in scope is narrower: the console's
role as an **OIDC relying party** — the PKCE/state/nonce transaction, the code exchange,
`id_token` validation, the session cookie, refresh, and logout.

**Method.** Read every file under evaluation (`frontends/apps/admin/middleware.ts`,
`frontends/apps/admin/lib/oidc.ts`, `frontends/apps/admin/lib/session.ts`, `frontends/apps/admin/app/api/auth/**`) and their Rust
counterparts (`backends/crates/sms-auth/src/login.rs`, `src/op.rs`, `backends/crates/sms-api/src/auth.rs`),
not sampled. Checked better-auth's actual behaviour **from its source** on GitHub
(`better-auth/better-auth`, `main` branch, which as of this check tracks the same commit
published to npm as `1.6.28` on 2026-08-13 — one day before this check), not from its
docs alone, because the docs turned out to describe some things at a level that hid the
decisive finding (see below). Every version/release-date claim is checked against the
live npm registry using `time[<version>]`, not `time.modified` — the mistake flagged in
this task's own brief, and one this repository's `AGENTS.md` already records happening
once this week for a different check (napi-rs release tooling).

---

## What better-auth actually offers here

better-auth is a full authentication **framework**, not a thin OIDC-client library: its
core (`betterAuth({...})`) always provisions `user`, `session`, `account`, and
`verification` tables through a database adapter (Kysely, Drizzle, Prisma, or MongoDB —
checked against `frontends/packages/core/src/db`; there is no adapter-less mode for anything that
touches OAuth account linking). The plugin that acts as an OIDC/OAuth2 **relying party**
against an arbitrary third-party provider is `genericOAuth`
(`frontends/packages/better-auth/src/plugins/generic-oauth`), part of the core `better-auth`
package. A second, separate package, [`@better-auth/sso`](https://www.npmjs.com/package/@better-auth/sso)
(MIT, same release cadence — `1.6.28`, 2026-08-13), adds an OIDC-specific connector
(`frontends/packages/sso/src/routes/sso.ts`) aimed at enterprise multi-tenant SSO (domain
verification, per-organization IdP configs, a self-service onboarding dashboard) that
happens to also work as a single fixed-provider RP.

Both were evaluated, since the task asks which plugin does this and neither is the
obvious sole candidate:

| | `genericOAuth` (core) | `@better-auth/sso` |
|---|---|---|
| PKCE | Opt-in, `pkce?: boolean`, **default `false`** (`generic-oauth/types.ts:95`) | Opt-in, same shape |
| `id_token` handling | `decodeJwt()` from `jose` — **decodes the payload, does not verify the signature** (`generic-oauth/routes.ts:11,795`) | Real verification: `jwtVerify()` against a `createRemoteJWKSet()` built from `config.jwksEndpoint`, checking `audience`/`issuer` (`frontends/packages/core/src/oauth2/validate-authorization-code.ts:170-189`, called from `sso.ts:1673`) |
| `nonce` claim | Not in the config type at all — grepped the whole plugin, zero occurrences of `nonce` | **Also absent.** `validateToken`'s own options type is `{ audience?, issuer? }` — no `nonce` parameter exists anywhere in `@better-auth/sso`'s source (grepped `frontends/packages/sso/src/**`, zero occurrences) |
| Database of its own | Yes — links into better-auth's `user`/`account` tables via `handleOAuthUserInfo`, same as every other social provider | Yes — identical `handleOAuthUserInfo` call site (`sso.ts:1735`) |
| Fit for "one fixed first-party OIDC provider, no hosted third-party login page" | Assumes a classic redirect-to-provider dance; nothing hosts a login form on our side of the exchange | Same — plus organization/domain-verification machinery this deployment has no use for (one issuer, one client, no multi-tenant IdP registry) |

**The decisive finding is in the source, not the docs.** better-auth's own docs for
`genericOAuth` don't mention nonce handling one way or the other, and describe issuer
checking in a way that reads as more thorough than it is. Reading
`frontends/packages/better-auth/src/plugins/generic-oauth/routes.ts` directly settles it:

```ts
// routes.ts:11
import { decodeJwt } from "jose";
...
// routes.ts:794-795
if (tokens.idToken) {
    const decoded = decodeJwt(tokens.idToken) as { sub: string; ... };
```

`decodeJwt` is `jose`'s **unverified** decode — it does not check the signature, `aud`,
`iss`, or `exp`. The only issuer check anywhere in this plugin runs off an `iss` **query
parameter** on the callback URL, gated behind `providerConfig.requireIssuerValidation`
(off unless set) — not a claim inside a verified token. There is no code path that
verifies a `genericOAuth` `id_token` at all.

`@better-auth/sso` is genuinely better here — `validateToken` does real
`createRemoteJWKSet` + `jwtVerify` with `audience`/`issuer` checked (and `exp`, which
`jwtVerify` checks unconditionally) — but its own `validateToken` signature has no
`nonce` parameter, and nothing in `frontends/packages/sso/src/**` reads a `nonce` claim at all.
Confirmed by grep across the whole package, not inferred from the absence of docs. The
Node.js pattern this evaluation had originally hoped for (drop in a library, keep the
same guarantees) does not exist in either candidate: even the stronger of the two
candidates has a real, silent gap on exactly one of the four checks this system's own
`frontends/apps/admin/lib/oidc.ts` / `frontends/apps/admin/app/api/auth/callback/route.ts` currently enforce.

---

## The five must-survive properties

| # | Property | better-auth (`genericOAuth`) | better-auth (`@better-auth/sso`) | Verdict |
|---|---|---|---|---|
| 1 | Pure relying party — no own identity provider, no own user/account/session tables, no database of its own | **Fails.** Core `betterAuth({...})` always provisions `user`/`session`/`account`/`verification` via a DB adapter; OAuth sign-in always links into `account` through `handleOAuthUserInfo`, regardless of "stateless session" mode (which only removes the *session-validation* read, not account provisioning) | **Fails**, identically — same `handleOAuthUserInfo` call site | **Reject** |
| 2 | Runs on the Edge runtime (`frontends/apps/admin/middleware.ts`'s hard constraint) | **Fails**, per better-auth's own docs (verbatim below) | Same session-handling code path — same failure | **Reject** |
| 3 | Full OIDC validation: S256 PKCE, constant-time `state`, `nonce` against the transaction, `id_token` signature via JWKS + `iss`/`aud`/`exp` | **Fails** on three of four — PKCE off by default, no signature verification, no `nonce` | **Fails** on one of four — no `nonce`; PKCE still opt-in, but signature/`iss`/`aud`/`exp` are real | **Reject** (genericOAuth outright; sso partial, see below) |
| 4 | Yields the signed-in human's real access token per request, forwardable one hop downstream via `x-vsms-access-token` (`frontends/packages/gateway`'s `AsyncLocalStorage`) | Not a built-in offering either way — `frontends/apps/admin/middleware.ts` would still need to read whatever better-auth stores server-side and set the same header itself | Same | **No net gain** either way — see below |
| 5 | R4 — doesn't make the console load-bearing for the backend; no new required env var or migration for a backend-only deployment | R4 is about the *backend* not depending on the console, so this specific rule is not directly violated — but adopting either plugin gives the console **its first database dependency ever** (`frontends/apps/admin/` has none today — checked; no `DATABASE_URL`, no adapter package, nothing) | Same | **New, unforced architectural dependency** |

**On property 3, being precise rather than just failing both wholesale:** `genericOAuth`
fails PKCE (off by default — would need `pkce: true` set explicitly, which is at least
achievable), fails signature verification entirely, and fails nonce entirely. `@better-
auth/sso` gets PKCE (opt-in, achievable) and signature/`iss`/`aud`/`exp` for free — a
real improvement over `genericOAuth` — but still fails nonce, silently, with no
configuration knob to add it. Since nonce defends against `id_token`
replay/substitution across concurrent login attempts on the same browser — precisely the
threat `frontends/apps/admin/lib/oidc.ts::verifyNonce` exists for — adopting `@better-auth/sso` would
mean **removing an existing, working, tested defence** (`frontends/apps/admin/lib/oidc.test.ts`'s own
`a_mismatched_state_is_rejected` guard-failure proof, and the equivalent nonce logic) and
not getting an equivalent back. The only way to close that gap is to hand-write nonce
verification on top of the library anyway — which reintroduces the OIDC-specific
crypto code this evaluation was asked to consider removing.

### Property 2, in the library's own words

Checked directly against `better-auth`'s Next.js integration docs (not assumed, and not
inferred from generic Edge-runtime knowledge — pulled the actual page text):

> "In older Next.js versions, middleware runs on the Edge Runtime and cannot make
> database calls."
>
> "Since Next.js middleware doesn't support running Node.js APIs directly, you must make
> an HTTP request."
>
> "From Next.js 15.2.0, you can use the Node.js runtime in middleware for full session
> validation with database checks" — but: "Node.js runtime in middleware is experimental
> in Next.js versions before 16."
>
> On the one function that *does* run on Edge, `getSessionCookie()`: "The
> `getSessionCookie` function only checks for the existence of a session cookie; it does
> **not** validate it" — and the docs' own security warning: **"THIS IS NOT SECURE!"**

`frontends/apps/admin/package.json` pins `next@15.5.23` — below the Next 16 line where Node middleware
stops being experimental. So the real choices for a better-auth-backed session gate on
this stack are: (a) run middleware in the Node runtime under an experimental flag for a
security-critical gate, (b) make an HTTP round trip from Edge middleware to a Node route
handler on every navigation (new latency this system doesn't have today), or (c) fall
back to `getSessionCookie()`'s cookie-existence-only check, which the library's own docs
say is not secure and only fit for optimistic UI redirects.

**Worth being precise about why, because the honest framing matters more than the
verdict:** this is not "the Edge runtime can't do session crypto" — this codebase's own
`frontends/apps/admin/lib/oidc.ts` disproves that framing every request, using `jose`'s WebCrypto
backend for AES-GCM decrypt, SHA-256, and PKCE challenge computation, entirely on Edge,
with no database call at all (the session *is* the encrypted cookie; refresh is a plain
`fetch` to `/token`). The actual disqualifier is narrower and structural: better-auth's
session model is **database-adapter-shaped by design** (a `session` row looked up by id,
optionally cookie-cached), so "validate a session on Edge" for better-auth specifically
means either a DB round trip Edge can't make, or falling back to the documented-insecure
cookie-existence check. `admin`'s own design sidesteps the whole problem by making the
session self-contained (a JWE cookie) and treating "refresh" as a stateless token
exchange with the issuer — a shape better-auth's core architecture doesn't offer, since
it doesn't treat OAuth-issued tokens as the source of truth for its own session.

---

## Line counts — what would actually be deleted

| File | Lines | What it does |
|---|---|---|
| `frontends/apps/admin/middleware.ts` | 256 | Session decrypt, refresh-ahead-of-expiry, txn-cookie minting on `GET /login`, header injection/stripping (`x-vsms-actor`/`x-vsms-role`/`x-vsms-access-token`) |
| `frontends/apps/admin/lib/oidc.ts` | 210 | PKCE pair generation, state/nonce generation, constant-time `verifyState`/`verifyNonce`, JWE encrypt/decrypt for both cookies |
| `frontends/apps/admin/lib/session.ts` | 77 | Node-runtime cookie read/write wrappers around `oidc.ts` for the route handlers |
| `frontends/apps/admin/app/api/auth/login/route.ts` | 80 | Reads the txn cookie, calls `sms-gateway`'s own `POST /login` (password + PKCE/state/nonce in one call), redirects |
| `frontends/apps/admin/app/api/auth/callback/route.ts` | 140 | State check, `/token` exchange, `id_token` verify (JWKS/iss/aud/exp), nonce check, session cookie write |
| `frontends/apps/admin/app/api/auth/logout/route.ts` | 20 | Clears the session cookie |
| **Total** | **783** | |

**What better-auth would add back, not remove for free:**

- A database adapter and its own migration (`user`/`session`/`account`/`verification`) —
  a genuinely new piece of infrastructure for `frontends/apps/admin/`, which owns none today. Realistic
  size for a minimal Postgres-via-Kysely setup: on the order of 50–100 lines of adapter
  wiring plus a schema this repository's own migration discipline (`AGENTS.md`'s
  "Regenerating migrations" section) has no hook for, since it's a second, independent
  schema in the same database or a second database entirely.
- The `nonce` check `@better-auth/sso` doesn't provide — hand-written on top of the
  library, the same ~15 lines `frontends/apps/admin/lib/oidc.ts::verifyNonce` already is.
- The `x-vsms-actor`/`x-vsms-role`/`x-vsms-access-token` header bridge into
  `frontends/packages/gateway`'s `AsyncLocalStorage` scope (property 4) — not a better-auth
  concept at all, so this stays exactly as hand-written as it is today, roughly the
  30-line tail of `middleware.ts`'s own `middleware()` function.
- **A structural mismatch neither plugin's docs surface, found by reading what `#194`
  actually built:** `genericOAuth`/`@better-auth/sso` both assume the classic
  redirect-the-browser-to-the-provider's-own-hosted-login-page OAuth dance — the browser
  is sent to an `/authorize`-shaped URL that renders *someone else's* login form.
  `backends/crates/sms-auth`'s OP deliberately does not expose one: `AGENTS.md`'s own #194 section
  is explicit that `GET /authorize` is "never mounted... a spec-compliant
  redirect-to-a-hosted-login-page dance buys nothing a first-party BFF needs." `POST
  /login` (`backends/apps/sms-gateway/src/login.rs`) collapses "authenticate the human" and "run
  `handle_authorize`" into one server-to-server call from `admin`'s own `POST
  /api/auth/login`, specifically so the console can render its *own* login form rather
  than redirecting to a separate hosted page. Neither better-auth plugin has a mode for
  this — adopting either as designed would mean building a real, browser-facing `GET
  /authorize` login page in Rust, reversing a deliberate #194 decision, not just wiring a
  client library.

Net honest estimate: **roughly 500–600 of the 783 lines could go**, offset by
**~150–300 lines of new adapter/config/nonce glue in TypeScript**, plus **a new
database schema this deployment has never needed**, plus (if the redirect-based flow
is kept as-is rather than reversed) a genuine architecture conflict that isn't a
line-count question at all. That is the shape this task's brief warned about
directly: *"a library that replaces 300 lines with 250 lines of adapter is not a win"* —
except here the trade is worse than break-even, because two of the deleted lines'
guarantees (nonce, Edge-native validation) don't come back.

---

## Weighing the risk direction

The two most recent real incidents in this exact area — `#211` (every upstream call
authenticating as the machine credential instead of the signed-in human) and `#243`
(every console write returning `403` in a containerised deployment because
`req.url`'s computed origin is the container's own bind address, not the browser's
`Origin`) — were both **deployment-topology bugs**: a header-forwarding gap and a
same-origin check comparing against the wrong source of truth. Neither is a defect in
hand-rolled cryptography, and neither would have been caught or prevented by using
better-auth instead of the current code:

- `#211` is about *which token* gets forwarded downstream to `frontends/packages/gateway`, a
  concern entirely outside any session library's scope — better-auth doesn't know this
  system's `AsyncLocalStorage` credential-scoping convention exists.
- `#243` is `frontends/packages/api/src/context.ts`'s CSRF check comparing `Origin` against
  `new URL(req.url).origin` inside a Next.js standalone container — a Next.js
  request-object quirk, unrelated to how the session cookie is produced or validated.

If the real risk in this subsystem is deployment topology, not OIDC crypto, then
replacing the crypto layer trades a small, already-tested, already-guard-proven risk
(this codebase's own convention: every check here has a recorded "broken the guard,
watched it fail, restored it" proof — `frontends/apps/admin/lib/oidc.test.ts`'s
`a_mismatched_state_is_rejected`, the four guard-failure proofs in `AGENTS.md`'s #194
section) for a new, less-tested one (an authentication framework's database adapter,
migration, and a plugin with a confirmed silent gap on one of four required checks).

---

## Verdict: **Reject**

Three of the five must-survive properties fail outright (pure-RP/no-DB, Edge runtime,
full OIDC validation), a fourth is a wash (token forwarding — not something either side
offers for free), and the fifth introduces a new, currently-nonexistent architectural
dependency (a database for `frontends/apps/admin/`) that this evaluation's own brief didn't ask for and
R4's spirit argues against adding without a forcing reason. The line-count math doesn't
rescue it: net code removed is real but modest once the adapter, the missing nonce
check, and (if kept) the redirect-based-authorize mismatch are priced in, and the biggest
single property this evaluation was built to protect — full OIDC validation, precisely
because the current code proves out every guard failing before trusting it — is the one
property no available better-auth configuration fully provides.

**What we would lose by adopting it, stated plainly rather than folded into the verdict
above:**

- **The `nonce` check.** Confirmed absent from both `genericOAuth` and `@better-auth/sso`
  source, not merely undocumented. Would have to be hand-written back on top of the
  library regardless of which plugin is chosen — the exact code this evaluation would
  otherwise be deleting.
- **`id_token` signature/`iss`/`aud`/`exp` verification**, if `genericOAuth` were chosen
  over `@better-auth/sso` — a straightforward downgrade, not a simplification, exactly
  the failure mode this task's brief warned against accepting quietly.
- **Edge-native, no-round-trip session validation.** Every request today decrypts and
  (when near expiry) refreshes the session inside `frontends/apps/admin/middleware.ts`, on Edge, with
  no network hop for the common case. A better-auth-backed session gate on Next 15.5
  either adds a per-request HTTP round trip from Edge to Node, or runs middleware
  under an experimental Next.js flag for a security-critical check, or falls back to a
  check the library's own docs call insecure.
- **A zero-database console.** `frontends/apps/admin/` needs no database of its own today — every
  session concern round-trips through `sms-gateway`. Adopting either better-auth plugin
  changes that permanently.

---

## What would change this verdict

Not a permanent rejection — three concrete things would flip it, in rough order of how
likely each is to happen:

1. **`@better-auth/sso` (or `genericOAuth`) adds `nonce` verification against a
   caller-supplied expected value.** This is the single most fixable gap of the three —
   it's a missing parameter on an existing, already-correct `validateToken` call, not a
   structural redesign. Track `better-auth/better-auth` for a `nonce` option landing in
   either plugin's OIDC callback path.
2. **better-auth ships a genuinely adapter-less / stateless RP mode** that doesn't
   provision `user`/`account` rows for a pure sign-in-and-forward-the-token use case —
   i.e., the "stateless session management" mode extended to cover OAuth account linking,
   not just session *validation*. As of this check, "stateless" only removes the
   session-lookup database read; account linking through `handleOAuthUserInfo` still
   needs the adapter regardless.
3. **This deployment moves past Next 16**, where Node.js middleware stops being
   experimental — at that point property 2's objection weakens to "a network hop per
   request" rather than "an experimental flag for a security gate," which is a much
   smaller cost to weigh against the properties still in play (1, 3, 5).

None of the three is close today. Re-check when any one of them lands, rather than
re-running this full investigation from scratch — the source citations above (specific
files, specific line ranges, as of the commit tracking npm's `1.6.28`) are the fast path
to confirming whether a given later version actually changed the finding.

---

## `docs/roadmap.md`

Checked per `AGENTS.md`'s mandatory-check rule. No edit needed: this document completes
no milestone, resolves no blocker or decision from the roadmap's own tables, changes no
dependency between milestones, and lands no infrastructure ahead of its milestone — a
reject verdict on a library evaluation has no sequencing consequence to record.
