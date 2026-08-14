# Frontend package audit — hand-rolled machinery vs. well-maintained libraries

**Status: investigation only. No code under `packages/` or `admin/` changed by this
document or its PR.** A console redesign (`docs/design/console-redesign.md`) is
in flight across several agents and already owns `admin/` and `packages/ui/`;
this audit defers to it wherever the two overlap and says so explicitly.

**Question asked (maintainer, 2026-08-14):** *"Your `packages/*` for frontend
apps do have a lot of manual work. Investigate if that cannot be reduced by
using well maintained pre-existing libraries."*

**Method.** Every file under `packages/gateway`, `packages/api`, `packages/ui`,
`packages/hooks`, `packages/env` was read, not sampled. `packages/sms-client`
was actually generated (`cratestack generate-typescript`, into a scratch
directory outside the repo — nothing under `packages/sms-client` was touched)
and its output read line by line, because the question "why is it unused" only
has a real answer once you've seen what it produces today, not what the
committed `GENERATING.md` said as of its last edit. Every library named below
was checked against the npm registry on 2026-08-14 for its actual latest
version, release date, and peer-dependency range — not assumed from
familiarity. `admin/` screens were read to find where gateway/UI packages are
actually consumed, since a package's own file doesn't show whether its shape
is exercised or dead weight.

---

## Summary table

| Package | Hand-rolled machinery | Verdict | One-line reasoning |
|---|---|---|---|
| `packages/gateway` | ~20 duplicated fetch-with-bearer-token-and-401-retry shells across 17 files | **Reject** the 3 named libraries (ky/ofetch/openapi-fetch) | None of them do ETag/If-Match, none remove the credential/error-mapping logic; the win is a same-file dedup, not a library |
| `packages/gateway` | ETag capture / `If-Match` replay (`rest.ts`) | **Keep as-is** | Confirmed no candidate library exposes response headers the way this needs — see sms-client below |
| `packages/gateway` | `AsyncLocalStorage` credential scoping (`request-credential.ts`) | **Keep as-is** | Already idiomatic Node for this; a library here is a downgrade |
| `packages/gateway` | mTLS `Agent` + `private_key_jwt` minting (`dispatcher.ts`, `token.ts`) | **Keep as-is** | Already built on `jose` and `undici` (both well-maintained); the glue around them is inherently deployment-specific |
| `packages/gateway` | `MessageStreamHub` poll-with-backoff singleton (`message-stream.ts`) | **Keep as-is** | Explicitly named as must-survive; no library replaces a shared-poll-loop-feeding-a-server-long-poll |
| `packages/gateway` | `null`→`undefined` JSON normalization (`json.ts`) | **Keep as-is** | Already a single, tested, schema-aware function — this *is* the fix, not a gap |
| `packages/sms-client` | Generated, unused CrateStack TS client | **Reject** as a runtime dependency | Structurally cannot support `ETag`/`If-Match` (discards the `Response` object); browser-facing react-query design, wrong architecture for this server-only mTLS BFF; `Decimal` fields are now class instances, not strings |
| `packages/api` | tRPC routers | **Keep as-is** | Already thin wrappers over tRPC + zod; nothing to replace |
| `packages/env` | Env validation + cross-field rules | **Keep as-is** | Already built on `@t3-oss/env-nextjs`; the extra logic is cross-field validation t3-env doesn't support declaratively |
| `packages/hooks` | tRPC/react-query provider (3 files, ~50 lines) | **Keep as-is** | Nothing to replace; not where the redesign's hook question lives |
| `packages/ui` | Radix primitives, hand toast store | **Defer** to `console-redesign.md` | Primitives are already mid-migration to Headless UI/DaisyUI elsewhere; toast store is small, correct, and encodes a real product rule |
| `admin/*-screen.tsx` (not owned by this audit, referenced) | Hand `useState` create/edit forms, 7 screens / ~10 forms | **Adopt** `react-hook-form` + `zod` | Already dependencies, already used correctly in the composer; every other write screen reinvents the same state/validation glue |
| `admin/audit-log-screen.tsx` | Hand `useState` filters + offset pagination | **Adopt** `nuqs` | Already the pattern for messages/jobs/webhooks; this screen was simply missed |
| `admin/*-screen.tsx` tables | Static JSX tables, no client-side sort/filter | **Reject** `@tanstack/react-table` (for now) | Confirmed zero click-to-sort or client-side row filtering anywhere in `admin/`; nothing today for it to replace |
| `packages/hooks` (redesign-owned) | `useDebouncedValue`/scroll-listener/media-query hooks scattered in `admin/` | **Not this audit's call** — already `console-redesign.md` D12/D13 | Independently reaches the same conclusion this audit would; one staleness risk flagged below |

Rough payoff estimate (see each section for the basis):

| Adopt item | Estimated lines removed/simplified | Risk | Owner |
|---|---|---|---|
| `react-hook-form` + `zod` for 7 admin write screens | ~250–350 lines of hand-rolled form state/validation glue | Low — library already proven in this codebase (composer) | `admin/` (redesign-adjacent, sequence after it) |
| `nuqs` for `audit-log-screen.tsx` | ~15–20 lines, mostly URL-sync boilerplate removed | Very low — direct copy of an existing pattern | `admin/` |
| **Not filed as an issue** — internal (no new dependency) dedup of the gateway's ~20 duplicated fetch shells | ~150–250 lines within `packages/gateway` | Low, if scoped to exclude `rest.ts`/`token.ts`/`message-stream.ts` | mentioned for completeness, see below |

The single biggest reduction available is the **admin write-screen forms**
(`react-hook-form`/`zod`) — real, hand-transcribed state machines duplicated
across seven screens, replacing a pattern this codebase already trusts
elsewhere. The single biggest *rejected* temptation is **`packages/sms-client`**
— it looks like the obvious fix for "hand-transcribed types" and is
structurally wrong for this app's transport.

---

## `packages/gateway`

This is where the "a lot of manual work" observation is most visible, and
where it's most heavily — and, on inspection, mostly correctly — justified in
its own doc comments. Every file opens with a `server-only` import and a
module doc explaining *why* it's hand-written, which is unusual rigor; the
question worth asking isn't "is this manual" (yes, all of it) but "is the
manual-ness paying for something a library can't give for free."

### The duplicated fetch-with-retry shape

`grep -l "const attempt = async ()" packages/gateway/src/*.ts` (excluding
tests) matches **17 files** — `app-clients.ts`, `apps.ts` (×2), `audit-log.ts`,
`client.ts`, `dashboard.ts`, `jobs.ts` (×2), `messages.ts` (×3),
`opt-outs.ts`, `providers.ts`, `rest.ts` (×4), `roles.ts`, `route-simulator.ts`,
`routes.ts`, `senders.ts`, `token.ts`(via `requestToken`), `users.ts`,
`webhooks.ts`, `workers.ts` — roughly **20 independent copies** of:

```ts
const attempt = async () => {
  const token = await resolveUpstreamAccessToken(); // or getMachineAccessToken()
  return undiciFetch(url, { method, headers: {...}, dispatcher: gatewayAgent() });
};
let response = await attempt();
if (response.status === 401) {
  invalidateUpstreamAccessToken();
  response = await attempt();
}
const parsed = await parseGatewayJson(response);
if (!response.ok) throw mapGatewayError(response.status, parsed, routeLabel);
```

Every one of these files' own doc comments already names this as deliberate,
not accidental — e.g. `messages.ts`: *"same shape as `client.ts`'s
`callProcedure`, duplicated rather than shared: this module and `client.ts`
are two temporary, independently-replaceable halves of the same seam... each
is small enough that sharing a helper isn't worth coupling their futures
together before T3 replaces both anyway."* T3 is `packages/sms-client`. Since
T3 turns out not to be viable (next section), that reasoning's premise is
gone, which makes this duplication worth actually fixing — but not
necessarily with a *library*.

**Candidates evaluated against the real requirement**, which is: attach a
Bearer token resolved from `AsyncLocalStorage` (or a cached machine token),
route every request through the mTLS `undici.Agent`, retry exactly once on an
unexpected 401 using a possibly-different resolver per call site, and surface
the raw response so `rest.ts` can read `ETag`/send `If-Match`.

| Candidate | Latest / released (checked 2026-08-14) | Handles ETag round trip? | Handles the 401-retry-with-refreshed-token idiom? | Verdict |
|---|---|---|---|---|
| [`ky`](https://www.npmjs.com/package/ky) | 2.0.2 / 2026-04-21, `engines.node >=22` (repo requires `>=22` — fine) | No — a caller still reads `response.headers.get('etag')` itself; ky doesn't special-case it | Yes, via `hooks.beforeRetry`/`afterResponse` — but the existing code already does this in ~10 lines per file, so the "idiom" isn't actually hard to hand-write | **Reject** |
| [`ofetch`](https://www.npmjs.com/package/ofetch) | 1.5.1 / 2025-11-01, no `engines` restriction | Same as ky — no built-in ETag handling | Same — hooks exist, but again this isn't the hard part | **Reject** |
| [`openapi-fetch`](https://www.npmjs.com/package/openapi-fetch) | 0.17.0 / 2026-02-11 | N/A | N/A | **Reject** — needs a real OpenAPI/Swagger document to generate types against; `sms-gateway` publishes none (`docs/architecture.md` and the whole repo were grepped: no `openapi.json`, no `swagger` reference anywhere) |

None of the three touches the actual expensive part: `mapGatewayError`'s
422→`fieldErrors`/409→`CONFLICT`/412→`CONFLICT`-with-`isStaleWriteError`/
401,403→`FORBIDDEN`-with-server-log mapping (`errors.ts`, 203 lines), the
credential decision (`request-credential.ts`), or the ETag capture/replay
(`rest.ts`). Adopting any of them still leaves ~150 of those 200 lines
untouched and only removes the ~8–12 line `attempt`/retry shell per file —
and a shared **internal** helper (no new dependency) removes exactly the same
lines with strictly less risk: no new peer-dependency surface to track
against React 19/Next 15, no library-internal retry semantics that could
interact unexpectedly with the mTLS dispatcher.

**Verdict: reject all three named libraries.** The duplication is real and
worth fixing, but the fix is an in-package refactor (extract one
`fetchWithAuth(method, path, { resolveToken, onUnauthorized, body? })`
helper that every non-ETag caller uses, leaving `rest.ts` on its own — it
already returns `{ data, etag }` and needs the raw `Response`, which the
shared helper can still hand back). Because this needs no new dependency, it
isn't filed as a GitHub issue per this audit's own "file an issue per adopt
verdict" rule — it's mentioned here so it isn't lost, and because leaving the
duplication until "T3 lands" (which now isn't going to happen — see below) is
no longer the right reason to defer it.

### Everything else in `packages/gateway`

- **`rest.ts` (322 lines) — ETag capture / `If-Match` replay.** Confirmed by
  generating the real client (below): no evaluated library exposes the
  underlying `Response.headers` the way this needs. **Keep as-is.**
- **`request-credential.ts` (153 lines) — `AsyncLocalStorage` credential
  scoping.** This is the property named explicitly in the brief as
  must-survive: `resolveUpstreamAccessToken()` throws rather than falling
  back to the machine credential when no scope was entered. This is already
  the idiomatic Node answer to "which identity is this request running as" —
  no library (React context doesn't reach server-only code the way this
  needs to cross `await`s in a Next.js Route Handler; a DI container would be
  more code, not less) improves on 50 lines of `AsyncLocalStorage` plus a
  throw. **Keep as-is**, and any future refactor of the fetch-shell
  duplication above must preserve this function's exact throw-not-fallback
  contract — `packages/gateway/src/request-credential.test.ts` already
  proves it fails loudly; that test must keep passing unmodified.
- **`token.ts` (187 lines) / `dispatcher.ts` (91 lines) — `private_key_jwt`
  minting and the mTLS `undici.Agent`.** Already built on `jose` (6.2.8) —
  itself a well-maintained, standard choice for JOSE/JWT in Node, nothing to
  swap it for. The caching (`globalThis`-scoped to survive Next's dev-mode
  HMR), the `jti`-per-attempt discipline, and the cert-path/scheme
  cross-checking are all deployment-specific glue no generic OAuth client
  library encodes, because no generic library knows this deployment's own
  "the scheme of `SMS_API_URL` selects mTLS or not" convention. **Keep
  as-is.**
- **`message-stream.ts` (292 lines) — the poll-with-backoff singleton
  hub.** Named explicitly in the brief as must-survive: this is **not**
  `useQuery({ refetchInterval })`, on purpose — `packages/hooks/src/
  provider.tsx`'s own module doc and `packages/api/src/routers/messages.ts`'s
  own module doc both record that combination stalling after one or two
  calls when it was tried live. What this hub does that no data-fetching
  library offers as a primitive: **one** upstream poll shared across every
  open browser tab regardless of subscriber count (`subscribers.size === 1`
  gates `start()`/`stop()`), a bounded `(id, version)` dedupe set, exponential
  backoff with a one-time `degraded`/`recovered` frame pair (not per-tick),
  and a `nextAllowedPollAt` throttle that stays correct even under
  subscribe/unsubscribe churn from `messages.ts`'s bounded server-side
  long-poll (`onStateChange` opens and closes a hub subscription on *every*
  browser request). SWR/TanStack Query's polling is per-hook-instance, not
  process-wide-singleton; neither has a "buffer N frames, timeout, and
  degrade with a recovery signal" primitive. **Keep as-is.**
- **`json.ts` (206 lines) — the `null`→`undefined` normalization.** This
  *is* the fix for a real, three-times-independently-discovered bug
  (`#221`'s own history, recorded in `AGENTS.md`): fourteen ad hoc
  `normalize*` functions collapsed into one schema-aware walk, with a named
  `VERBATIM_STRING_FIELDS` guard for the columns that store pre-serialised
  JSON as a `String` scalar (`WebhookAttempt.payload`, the audit snapshot
  columns, `Provider.config`) and a prototype guard against silently
  emptying a `Date`/`Map`/`Set`. No generic "deep null-strip" library on npm
  knows about `sms-api`'s specific schema-level distinction between "this
  column is a nullable scalar" and "this column is pre-serialised JSON
  stored as a string" — that distinction is the whole point of the function,
  and it's the reason a blind recursive strip would be actively dangerous
  here. **Keep as-is** — this is the single strongest "already did the
  library's job in-house, correctly" example in the whole audit.
- **`errors.ts` (203 lines).** Small, sms-api-specific status→tRPC-code
  mapping (`422`→`BAD_REQUEST` with `fieldErrors`, `409`/`412`→`CONFLICT`,
  `401`/`403`→`FORBIDDEN` with a server log, everything else→
  `INTERNAL_SERVER_ERROR`) plus `isStaleWriteError` — the one thing an edit
  screen needs to branch on for #59's optimistic-concurrency UX. No generic
  error-mapping library encodes this app's specific vocabulary. **Keep
  as-is.**

---

## `packages/sms-client` — why it's unused, and whether that reason still holds

`packages/sms-client/GENERATING.md` (the one committed file; everything else
is gitignored and regenerated) gives the standing reason: cratestack's
`Decimal` TypeScript emission was broken until `cratestack#456` (fixed in
0.7.8), and `packages/gateway/src/client.ts`'s own module doc says this
package "will replace" the hand-transcribed types "once an upstream `Decimal`
fix ships." **That fix has shipped** — `cratestack --version` in this
environment reports `0.7.12`, well past the `0.7.10` pin this repo already
uses, and `AGENTS.md`'s own cratestack-bump section confirms `Decimal` fields
became real `decimal.js` instances in 0.7.10. So the *documented* reason the
client is unused is stale. That does not mean the client is now adoptable —
it means the real reason had to be found by actually generating it and
reading the output, which this audit did (`cratestack generate-typescript
--schema schema/schema.cstack --out <scratch dir>`, never touching the
tracked `packages/sms-client/`).

**What the generated client actually is, read from `src/runtime.ts` and
`src/client.ts` directly:**

```ts
async request<T>(method: string, path: string, options: CratestackRequestOptions = {}): Promise<T> {
  ...
  const response = await this.fetchFn(this.url(path, options.query), { method, headers, body, signal });
  const payload = await readResponsePayload(response);
  if (!response.ok) throw new CratestackHttpError(response, payload);
  return payload as T;   // <-- the Response object is discarded here
}
```

Every generated method (`AppApi.get`, `ProviderApi.update`, …) chains
`.then((value) => reviveDecimalFields(value, 'Provider'))` off that call and
returns the parsed body — **there is no path back to `response.headers`**.
This is disqualifying, not inconvenient: `rest.ts`'s entire job is reading
`ETag` off a `GET`/detail response and sending it back as `If-Match` on the
following `PATCH`, and the generated runtime structurally cannot hand that
header back without patching `runtime.ts` by hand — which doesn't survive
`packages/sms-client` being gitignored and regenerated on every
`just client-gen`. #59's whole optimistic-concurrency story (ten `@version`d
models, `PATCH /providers/{id}` etc.) depends on this exact round trip; the
generated client cannot carry it.

Three further findings, in order of how much they matter:

1. **Wrong architecture, not just a missing feature.** `src/react-query.ts`
   (1,887 lines) generates `useQuery`/`useMutation` hooks with a
   `@tanstack/react-query` **peer dependency** — this package is built to be
   called directly from a browser component. Every consumer in this repo
   (`@vsms/gateway`) is `server-only`: mTLS client certs read from disk,
   `AsyncLocalStorage`-scoped credential resolution, a process-wide
   `undici.Agent`. Wiring the generated client into that shape means
   ignoring `react-query.ts` entirely and hand-supplying `options.fetch` (a
   custom fetch closure that reattaches the mTLS `dispatcher` — workable,
   `CratestackRuntime` accepts an injected `fetch`) and `options.headers`
   (a sync-or-async function — also workable, and it *would* correctly pick
   up `resolveUpstreamAccessToken()`'s `AsyncLocalStorage` context, since
   that context follows the async causality chain, not the literal call
   stack). Both are real, buildable wrappers — but building them reproduces
   most of `dispatcher.ts`/`request-credential.ts` again, just underneath a
   different fetch signature, for zero net reduction.
2. **No retry-on-401 at all.** `CratestackHttpError` carries `status`,
   `response`, `payload` — nothing retries a stale token. Same wrapper cost
   as above.
3. **Money fields become `decimal.js` instances, not strings.**
   `models.ts`'s own `reviveDecimalFields`/`revivePagedDecimalFields` is
   good engineering (per-shape field revival, not a flat cross-type
   name-set — the comment explains a real bug that approach had) — but it
   means `Provider.costPerSegmentXaf`/`Message.costXaf`/
   `SendMessageResult.estimatedCostXaf` arrive as `Decimal` class instances.
   This repo's money-safety convention is "never floating point for minor
   units," currently satisfied by keeping these fields as plain strings
   end-to-end (`client.ts`'s own doc: *"kept as a string, never parsed to
   `number`"*). Adopting the generated client for money-bearing responses
   means auditing every consumer for correct `.toString()` use — a real,
   non-trivial migration for models this repo already handles safely.

**One thing worth borrowing conceptually, not wholesale:** `queries.ts`'s
`toSearchQuery` builds exactly the `fields=/sort=/where=/or=/limit/offset`
grammar `messages.ts`'s own module doc spent nine numbered findings probing
live against a real gateway. It's a good, already-correct implementation of
that grammar. It is not, on its own, worth taking a dependency on the whole
client for.

**Verdict: reject `packages/sms-client` as a runtime dependency.** The
disqualifying reason is structural (no ETag path), not the Decimal issue the
committed doc currently blames — which is itself now stale. **Recommended,
not filed as an issue** (a documentation-only fix, not a library adoption):
correct `packages/sms-client/GENERATING.md` and `OPEN_QUESTIONS.md` §5
("Is `packages/sms-client` meant to be committed?") to record the real
reason (no `ETag`/`If-Match` support, wrong browser-facing architecture)
rather than the now-outdated "waiting on a Decimal fix" — the current text
will otherwise mislead the next person who checks whether the blocker
cleared, exactly the "documentation asserts something the code does not do"
pattern `AGENTS.md` already tracks five instances of.

---

## `packages/api`

Seventeen router files, ~1,124 lines total, every one a thin
`publicProcedure.input(zodSchema).query/mutation(async ({ ctx, input }) =>
{ try { return await ctx.gateway.xxx(input); } catch (e) {
rethrowGatewayError(e); } })`. `trpc.ts` (42 lines) wires tRPC + `superjson`
+ an `errorFormatter` that threads `GatewayError.fieldErrors` through to the
browser for `react-hook-form`'s `setError` — already built and already
correctly wired, waiting for `sms-api` to actually populate `details` on a
`422` (pinned `cratestack-pg` doesn't yet). This package already *is* "use a
well-maintained library" — tRPC and zod are the libraries, and the routers
are the minimum glue tRPC's own design requires. **Keep as-is.** Nothing
found here worth replacing.

---

## `packages/env`

Already built on [`@t3-oss/env-nextjs`](https://www.npmjs.com/package/@t3-oss/env-nextjs)
(latest `0.13.11`, released 2026-03-22 — actively maintained) — this
directly answers the brief's own suspicion ("is this reimplementing
t3-env/znv?"): no, it's already using t3-env for exactly what it's for
(per-field zod schemas, split server/client, `runtimeEnv` wiring). The
~40 lines of hand-written logic *after* `createEnv(...)` are cross-field
rules t3-env has no declarative mechanism for: "if `SMS_API_URL` is
`https:`, all three cert paths must be set, and vice versa" and "in
`NODE_ENV=production` (but not `next build`'s own `NEXT_PHASE=phase-
production-build` compile pass), `ADMIN_BASE_URL` must be `https:`."
`createEnv`'s schema is per-key; it cannot express "field A implies field B."
[`znv`](https://www.npmjs.com/package/znv) (latest `0.5.0`, released
2025-03-24 — noticeably less active than t3-env) has the identical
limitation. A single top-level `z.object({...}).superRefine(...)` wrapping
the whole env would technically express this, but t3-env's `createEnv`
doesn't accept one in place of the per-key `server`/`client` shape, so this
would mean hand-rolling the *entire* env schema outside `createEnv` to get
one cross-field check — a strictly worse trade than the current ~40 lines.
**Keep as-is.**

---

## `packages/hooks`

Three files, ~50 lines: a `TrpcProvider` (`QueryClientProvider` +
`trpc.Provider`, one `httpBatchStreamLink`) and a type-only `trpc` client.
There is no hand-rolled generic-hook machinery in this package today —
`useDebouncedValue`, a scroll-position listener, and a media-query check all
currently live *inside* `admin/` screens (`admin/app/page.tsx`,
`admin/app/apps/apps-screen.tsx`, `bespoke/live-row.tsx`), not here.

This is exactly the ground `docs/design/console-redesign.md` already covers,
independently, and reaches the same place this audit would:

> **D12** — `@uidotdev/usehooks` replaces genuinely generic hand-rolled
> hooks: the `prefers-reduced-motion` check in `LiveRow` (`useMediaQuery`),
> the scroll-position listener in `messages-screen.tsx`
> (`useWindowScroll`)...
>
> **D13** — **Not** replaced by `usehooks`, and must not be:
> `TimestampDisplay`'s shared 30-second-tick external store... and
> `messages-screen.tsx`'s self-scheduling long-poll loop... both encode
> product-specific correctness properties, not generic hook patterns.

That is the identical distinction this audit's own gateway section draws
between `json.ts`/`request-credential.ts` (keep — encodes a real property)
and the duplicated fetch shells (fix — encodes nothing). Since this decision
is already made and owned elsewhere, **this audit makes no independent
recommendation for `packages/hooks`** — deferring avoids exactly the
collision the task brief warned about.

**One flag worth handing back, not a verdict:** `@uidotdev/usehooks`'s own
npm listing shows its **latest version, `2.4.1`, was published
2023-10-23** — essentially three years stale as of this check (2026-08-14),
versus `usehooks-ts` (latest `3.1.1`, published 2025-02-05, peer dep
explicitly `react ^19`) which covers the same three primitives
(`useMediaQuery`/window-scroll-equivalent/debounce) and has shipped more
recently. `@uidotdev/usehooks`'s own peer range (`react >=18.0.0`, open-
ended) makes no explicit claim about React 19 either way. This doesn't
override constraint 9 in `console-redesign.md` ("`@uidotdev/usehooks` for
hook needs" — given, non-negotiable) — it's a due-diligence note for
whichever agent executes D12/D13: verify the specific hooks used
(`useMediaQuery`, the window-scroll hook) actually behave correctly under
React 19 strict mode before relying on them, since the package's own release
history predates React 19 by roughly a year and a half.

**Second flag, a genuine gap in D12's own enumeration:** `admin/app/page.tsx`
hand-rolls a fourth generic hook, `useDebouncedValue` (used for the composer's
debounced preview inputs), that D12's list of three doesn't mention. It's the
same shape `usehooks`' `useDebounce` already covers — worth folding into
whichever PR executes D12/D13 rather than left as a still-hand-rolled fourth
instance after the other three move.

---

## `packages/ui`

Thirty-one components, already itemised component-by-component in
`console-redesign.md` §5 ("Component inventory") with its own Port/Refresh/
Rebuild/Delete verdicts — Radix primitives (`dialog`, `dropdown-menu`,
`label`, `popover`, `select`, `tabs`) are already scheduled to become
Headless UI; `separator`/`tooltip`/`slot` are scheduled for removal; `table`
is scheduled for a DaisyUI-class pass under its existing hand-rolled
row/cell logic. **This audit defers to that document entirely for every
primitive it already covers** rather than layering a second, possibly
conflicting set of verdicts on top.

Two things outside that inventory's direct scope, checked independently:

- **`primitives/toast.tsx` (105 lines)** — a hand-rolled store
  (`useSyncExternalStore` + a module-level array + `setTimeout` dismissal),
  deliberately not Radix. `console-redesign.md` marks it "Refresh (mechanism
  kept)." Checked against [`sonner`](https://www.npmjs.com/package/sonner)
  as an obvious alternative: sonner is well-maintained and would delete
  these ~100 lines, but this file's own module doc encodes a real,
  deliberate product rule this codebase enforces elsewhere too — "anything
  an operator must act on is inline, never a toast" (matching
  `providers-screen.tsx`'s pattern of showing `updateMutation.isError` as an
  inline banner, never a toast, for exactly this reason). A generic toast
  library doesn't know that rule and would need the same call-site
  discipline either way — the 100 lines saved buy nothing this app couldn't
  already build correctly, and `useSyncExternalStore` is the React-
  recommended pattern for exactly this shape of external store. **Reject** —
  correctly small already, not worth the swap.
- **`primitives/table.tsx` (81 lines)** is a presentation-only wrapper
  (`<table>`/`<thead>`/`<tbody>`/`<tr>`/`<th>`/`<td>` with the design
  system's spacing/border/hover classes) — it does not own sorting,
  filtering, or pagination; those live per-screen in `admin/`. See the next
  section for why that matters to the `@tanstack/react-table` question.

---

## `admin/*-screen.tsx` — referenced, not owned by this audit

`admin/` is explicitly off-limits to edit for this task (the console
redesign owns it), but the maintainer's question can't be answered honestly
without reading what actually consumes `packages/gateway`/`packages/ui`.
Two real, independent findings came out of that reading, both outside
`console-redesign.md`'s own scope (that document covers visual/structural
redesign of `admin/` and `packages/ui/`; it does not cover form-state
management or data-fetching), and both are filed as GitHub issues.

### Tables: `@tanstack/react-table` — reject, not adopt-later

The suspicion in the brief was that Messages/Jobs/Providers hand-roll
sorting, filtering, and pagination. Checked directly:

```
grep -rln "onClick.*[Ss]ort\|toggleSort\|sortBy\|\.sort((a, ?b)" admin --include="*.tsx"
→ no matches anywhere in admin/
```

**There is no client-side sorting or row filtering anywhere in this admin
console.** What actually happens:

- `messages-screen.tsx`: fixed `sort=-createdAt` server-side; filters
  (`state`, `clientRef`, date range) are `nuqs`-backed URL state fed straight
  into the tRPC query input, which `@vsms/gateway`'s own `listMessages`
  applies server-side within its documented bounded-window limits.
- `jobs-screen.tsx`: identical shape — `nuqs`-backed `state`/`kind` filters
  feed the query input directly; no client-side array filtering.
- `routes-screen.tsx`: a small, fixed list, server-sorted by priority; no
  interactive sort at all.
- `audit-log-screen.tsx`: hand `useState` offset pagination (see next
  section) with a plain prev/next pair — not something a table library's
  pagination plugin (built for client-side row models or server row-count
  callbacks) would meaningfully simplify over two `onClick` handlers.

`@tanstack/react-table` (checked: latest `9.1.2`, released **2026-08-09** —
five days before this audit, extremely actively maintained; peer dep
`react >=18`, compatible with React 19.2) is a genuinely good, headless
library and nothing here is a knock against it. The honest finding is that
**nothing in this codebase today exercises the problem it solves** — every
screen's "table" is a static column layout over a server-shaped array, and
introducing a row-model/column-def abstraction over that would replace
simple, readable JSX with more code for zero behavior change. It would also
collide directly with `console-redesign.md`'s own D16 (`table.tsx` gets a
DaisyUI-class rebuild "underneath existing bespoke row/cell logic") — adding
a data-table engine at the same time as that rebuild is in flight is exactly
the kind of double-move the task brief asked to avoid.

**Verdict: reject for now.** Revisit only if a screen grows a real
requirement this shape doesn't already satisfy — user-driven column
sorting/reordering/resizing, or client-side filtering over an
already-fetched large row set. None of the current nine list screens have
that need.

### Forms: `react-hook-form` + `zod` — adopt

Both are already dependencies (`react-hook-form@7.85.0`, `zod@4.4.3`, both
current — `react-hook-form`'s latest release was six days before this audit
and its peer range is explicitly `^16.8.0 || ^17 || ^18 || ^19`) and are
already used correctly in the composer (`admin/app/page.tsx`). Checked which
other screens use them:

```
grep -rl "react-hook-form" admin --include="*.tsx"  →  admin/app/page.tsx  (only)
```

Every other screen with a create/edit form hand-rolls its own state. Read in
full for a representative case, `providers-screen.tsx`:

```ts
interface EditFormState {
  displayName: string; state: ProviderState; maxTps: string;
  maxDailySubmissions: string; costPerSegmentXaf: string;
}
const [form, setForm] = useState<EditFormState | null>(null);
useEffect(() => { /* populate `form` from the fetched detail row */ }, [detailQuery.data]);
// per field: onChange={(e) => setForm({ ...form, maxTps: e.target.value })}
function save() {
  updateMutation.mutate({ ..., maxTps: Number(form.maxTps), maxDailySubmissions: Number(form.maxDailySubmissions) });
}
```

No client-side validation before the request; number fields are carried as
strings and coerced at submit time; errors surface only from the mutation's
own `isError`/`error.message`. The same pattern repeats, with more fields
and more dialogs each, in:

- `routes-screen.tsx` — create/edit route (priority/weight/match predicates)
- `webhooks-screen.tsx` — create endpoint, rotate-secret confirmation form
- `sender-ids-screen.tsx` — create sender ID, edit registration (two
  separate hand-rolled forms)
- `users-screen.tsx` — provision user, edit role assignment (two forms)
- `apps-screen.tsx` — provision app, edit app (two forms)
- `opt-outs-screen.tsx` — search-by-MSISDN form, record-opt-out form (two
  forms)

That's seven screens, roughly ten independent hand-rolled form state
machines, each reinventing what `react-hook-form` already does once,
correctly, for the composer.

**What must survive the migration, named explicitly because getting any of
these wrong is a real regression, not a style nit:**

- **412 (`isStaleWriteError`) handling stays exactly as-is.** This is
  independent of form *state* management — it's a response-shape check in
  `packages/gateway/src/errors.ts`, already decoupled from how a screen
  manages its input fields. A form library must not swallow or reinterpret
  it; the "someone else changed this, reload" UX it drives is a hard
  requirement `#59` was built for.
- **Layer-2 `403` denials must keep surfacing as real, visible errors**,
  not be absorbed into `react-hook-form`'s own field-level error UI as if
  they were a validation failure. `providers-screen.tsx`'s existing inline
  banner (`updateMutation.isError && <div>Save failed: {message}</div>`) is
  the right shape and should be kept as a form-level error, not remapped
  onto a specific field.
- **`trpc.ts`'s `errorFormatter` already threads `GatewayError.fieldErrors`
  through** (`packages/api/src/trpc.ts`, `fieldErrors` on `error.data`) —
  this was built *for* `react-hook-form`'s `setError`, per its own module
  doc, and is currently unused by every hand-rolled screen. Adopting RHF is
  what actually turns this dormant plumbing live, not new work.
- **Number-typed fields (`maxTps`, `maxDailySubmissions`, weight,
  priority) need an explicit `valueAsNumber`/zod `.coerce.number()` at the
  boundary** — the current hand-rolled `Number(form.maxTps)` coercion is
  exactly the kind of silent-empty-string-becomes-`NaN` risk zod's coercion
  closes properly; this is a real correctness improvement, not just less
  code.
- **`costPerSegmentXaf` and any other `Decimal`-on-the-wire field stay
  plain strings, never `z.number()`.** Feeding a money field through
  `valueAsNumber` would violate this repo's own floating-point-money
  convention — the zod schema for that one field must be `z.string()` with
  a regex/format check, not a numeric coercion.

**Estimate:** each hand-rolled form (state interface + `useEffect`
population + per-field `onChange` + manual `save()` coercion) runs roughly
30–50 lines; ten forms across seven screens is a realistic 250–350 lines
removed, replaced by `useForm`/`zodResolver`/`register` calls that are
already a proven pattern one file over. **Filed as an issue** (see below).

### `nuqs` for `audit-log-screen.tsx` — adopt

`nuqs` (checked: latest `2.9.5`, released 2026-08-05 — nine days before this
audit; peer range explicitly includes `react ^19.0.0-0` and `next >=14.2.0`)
is already a dependency and already used correctly for URL-persisted,
bookmarkable filter state in `messages-screen.tsx`, `jobs-screen.tsx`, and
`webhooks-screen.tsx`. `audit-log-screen.tsx` was the one screen with
filter-and-pagination state that never got the same treatment:

```ts
const [model, setModel] = useState("");
const [actorId, setActorId] = useState("");
const [offset, setOffset] = useState(0);
```

This is a small, mechanical, zero-risk port of an already-proven pattern —
not a new library, not a new decision, just closing a gap the other three
screens already closed. **Filed as an issue** (see below), scoped narrowly:
convert these three fields to `useQueryStates` the same way
`jobs-screen.tsx` already does, nothing else in the screen changes.

---

## Keep as-is — the hand-rolled code that should stay

Collected here so this investigation doesn't get redone in three months:

1. **`packages/gateway/src/rest.ts`** — ETag capture / `If-Match` replay.
   Confirmed no library (generated or third-party) exposes the response
   headers this needs without hand-patching generated code.
2. **`packages/gateway/src/request-credential.ts`** — `AsyncLocalStorage`
   credential scoping, fail-loud on no scope. Already idiomatic; the
   fail-loud property is a named hard requirement.
3. **`packages/gateway/src/token.ts` / `dispatcher.ts`** — `private_key_jwt`
   minting (on top of `jose`, already a well-maintained library) and the
   mTLS `undici.Agent`. Deployment-specific glue, not reinvention.
4. **`packages/gateway/src/message-stream.ts`** — the poll-with-backoff
   singleton hub. Named must-survive; no library offers "one poll shared
   across N subscribers, feeding a bounded server-side long-poll" as a
   primitive.
5. **`packages/gateway/src/json.ts`** — the `null`→`undefined`
   normalization. Already the fix for a real, three-times-found bug; a
   generic deep-null-strip library would be actively unsafe against this
   schema's pre-serialised-JSON-as-`String` columns.
6. **`packages/gateway/src/errors.ts`** — sms-api-specific error→tRPC-code
   mapping. No generic library encodes this app's vocabulary.
7. **`packages/api`** — thin tRPC + zod routers. Already minimal.
8. **`packages/env`** — already `@t3-oss/env-nextjs`; the extra logic is
   legitimate cross-field validation with no declarative equivalent in
   t3-env or znv.
9. **`packages/ui/src/components/primitives/toast.tsx`** — small, correct,
   `useSyncExternalStore`-based, and encodes a real "no toast for
   actionable content" product rule a generic toast library doesn't know.
10. **`admin/app/messages/messages-screen.tsx`'s poll loop** — explicitly
    not `useQuery({ refetchInterval })` for a documented, previously-live
    reason (stalls after one or two calls). Any future data-fetching
    library adopted elsewhere in this codebase must not be applied here
    without re-proving that failure mode is actually fixed, live, first.
11. **`packages/hooks`** — already minimal; the redesign's own D12/D13 in
    `console-redesign.md` already correctly separates what should move to
    `usehooks` from what must stay custom (`TimestampDisplay`'s shared
    tick, the messages poll loop).
12. **`admin/*-screen.tsx` tables** (`Table`/`TableRow`/etc. usage) — no
    client-side sort/filter exists anywhere to replace; `@tanstack/
    react-table` is a good library with nothing in this codebase to solve
    yet.

---

## What was rejected, and the concrete reason each time

| Rejected | Concrete, evidence-based reason |
|---|---|
| `ky` / `ofetch` for the gateway's fetch layer | Neither handles `ETag`/`If-Match`; the actually-expensive logic (credential resolution, error mapping) stays custom regardless; the one thing they'd centralize (retry-on-401) is cheaper as an internal helper with no new dependency |
| `openapi-fetch` | No OpenAPI/Swagger document exists anywhere in this repo for it to generate against |
| `packages/sms-client` as a runtime dependency | Generated `CratestackRuntime.request()` discards the `Response` object — structurally cannot support the `ETag` round trip `#59` depends on; designed for direct browser calls (`@tanstack/react-query` peer dep) against a server-only mTLS BFF; `Decimal` fields are now class instances requiring a real migration |
| `@tanstack/react-table` | Zero client-side sorting/filtering exists anywhere in `admin/` today for it to replace; would collide with `console-redesign.md`'s own in-flight `table.tsx` rebuild |
| `sonner` (toast) | Existing 105-line store is already correct, minimal, and encodes a product rule (no toast for actionable content) the library doesn't know about |
| `znv` (env validation) | Already using the more actively maintained `@t3-oss/env-nextjs`; both share the identical cross-field-validation gap this repo's hand-written rules fill |

---

## Filed issues

Two GitHub issues, both scoped to one person, both stating the must-survive
properties explicitly in the issue body:

- **Adopt `react-hook-form` + `zod` for the admin console's hand-rolled
  create/edit forms** (providers, routes, webhooks, sender IDs, users, apps,
  opt-outs — ~10 forms across 7 screens).
- **Adopt `nuqs` for `audit-log-screen.tsx`'s filter/pagination state**,
  matching the pattern already in messages/jobs/webhooks screens.

No issue filed for the internal gateway fetch-shell dedup (not a library
adoption — see the gateway section) or for the `GENERATING.md`/
`OPEN_QUESTIONS.md` §5 documentation correction (a doc fix, not a library
adoption) — both are called out above so they aren't lost, but neither fits
this document's "file an issue per adopt verdict" rule.

## `docs/roadmap.md`

Checked per `AGENTS.md`'s mandatory-check rule. No edit needed: this PR
completes no milestone, resolves no blocker or decision from the roadmap's
own tables, changes no dependency between milestones, and lands no
infrastructure ahead of its milestone — it's a documentation-only
investigation with no sequencing consequence.
