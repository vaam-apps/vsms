# Open questions

Things this system does not yet know the answer to, gathered in one place so
they stop being rediscovered.

**What belongs here:** a question whose answer changes what gets built, and
that reading more of this codebase will not settle. Some need a human
decision, some need evidence nobody has gathered yet, some are accepted
limitations that a future reader should not mistake for oversights.

**What does not:** ordinary unbuilt work. A story that is simply not written
yet lives in GitHub Issues, not here. If the only thing standing between a
question and its answer is someone doing the work, it is not an open
question — it is a ticket.

**Status is a dated snapshot** *(2026-08-13)*. GitHub is the tracker and is
always more current; `docs/roadmap.md` owns sequencing; `docs/architecture.md`
§12 owns what each milestone means. This file owns only the *unknowns*.

---

## 1. Needs a human decision

Nobody can settle these by reading code. Each blocks or reshapes real work.

### 1.1 Does vsms need its own ART title? ([#4](https://github.com/vymalo/vsms/issues/4))

**Blocks:** whether Milestone 7 (SMPP, direct interconnect) exists at all.

Orange's VAS interconnection catalogue requires an ART title — a licence or a
*récépissé de déclaration préalable* — plus a short-code allocation document,
before it will interconnect. ART has enforced this: in 2018 it announced it
would dismantle unlicensed VAS providers' networks, with penalties of
100–500 million FCFA.

**Unverified:** whether a pure API consumer buying capacity from a *licensed
aggregator* needs its own title. The safe reading is that direct MNO
interconnection or a short code unambiguously requires one.

Consequences are binary: no title needed → M7 is cancelled and the system
stays on aggregator HTTP indefinitely; title needed and obtained → M7
proceeds, and inbound STOP handling via a real short code
([#76](https://github.com/vymalo/vsms/issues/76)) becomes possible at all.

**Note this is not blocking anything today.** M5's aggregator path
deliberately avoids the question, which is why the MTN adapter was built that
way. It becomes urgent only when someone wants a direct interconnect.

### 1.2 Should the console act as the logged-in human? (settled 2026-08-13, [#211](https://github.com/vymalo/vsms/issues/211))

**Answered: forward the human's own `accessToken` verbatim, at the single
tRPC route-handler boundary, via `AsyncLocalStorage` — not a per-request
token exchange.**

#194 shipped a real authorization_code + PKCE login, but `frontends/packages/gateway`
kept authenticating **upstream** with its own machine credential regardless
of who was signed in — verified by signing in as a freshly provisioned
`owner` and watching a `Provider` edit fail with `missing required
permission "provider:update"`. Two shapes were on the table:

- **Forward the human's own `accessToken`** and let `GatewayAuth` validate
  it directly. **Taken.** Checked, not assumed: `GatewayAuth::
  authenticate_human` already validates a human token fully — signature via
  JWKS, issuer, and audience against `sms-console`
  (`GatewayAuth::human_client_id`) — and `frontends/apps/admin/middleware.ts` already
  refreshes the session's `accessToken` ~60s ahead of expiry on *every*
  request, redirecting to `/login` outright if refresh fails. So by the time
  a request reaches the tRPC route handler, the token is already fresh for
  that request's lifetime — no separate exchange step, no expiry logic
  needed in `frontends/packages/gateway` at all.
- **Exchange the session for a per-request token.** Not taken — strictly
  more moving parts for no benefit once the above was confirmed live.

**How it's plumbed, and why that shape specifically:** `frontends/packages/gateway`'s
~13 upstream-calling functions across 9 files are called via `ctx.gateway.
xxx()` — a module reference, not a per-request-bound instance — so an
explicit credential parameter would have touched every one of those call
sites *and* every router that calls them. Instead, `frontends/packages/gateway/src/
request-credential.ts`'s `AsyncLocalStorage` is set once, at the one true
per-request boundary (`frontends/apps/admin/app/api/trpc/[trpc]/route.ts`'s `handler`,
reading the `x-vsms-access-token` header `frontends/apps/admin/middleware.ts` forwards),
and every ordinary call site resolves it implicitly via
`resolveUpstreamAccessToken()`. The failure mode the ticket named — a new
screen silently getting the machine credential when it meant the human's —
is closed structurally: `resolveUpstreamAccessToken()` throws if no
credential scope was ever entered, rather than defaulting to the machine
credential. Reaching for the machine credential requires importing
`getMachineAccessToken` from `./token` directly, by name, which the module's
own doc reserves for two documented exceptions:
`client.ts`'s `previewMessage`/`sendMessage` (`backends/crates/sms-api/src/
procedures.rs::caller_client_id` structurally rejects a human caller — "no
design yet" for deriving an `App` from one) and `messages.ts`'s
`listMessagesForStream` (the `MessageStreamHub` singleton polls once, shared
across every open tab — no single human to attribute it to, and
`AsyncLocalStorage` would otherwise leak whichever operator's tab happened
to trigger the first poll into every other tab's stream).

**A real, pre-existing bug found and fixed in the same PR, because the fix
is what finally exercised it:** the seeded role `permissions`
(`backends/migrations/postgres/0002_bootstrap`) used `message:read`/
`message:send` where `backends/crates/sms-api/src/rbac.rs::require_permission`
actually checks `sms:read`/`sms:send` (the same literals the machine
scope table already used correctly), and no role carried `dashboard:read`
at all. Both were silent until a human token was ever forwarded to hit
`listMessageReceipts`/`dashboardSummary` — which #211 is the first PR to
do. Fixed by renaming the seed literals and adding `dashboard:read` to
`owner`/`admin`/`operator`/`auditor`; data-only, no schema/DDL change.

**A real, deliberate consequence, not a bug:** `Message`/`DeliveryReceipt`'s
own `@@allow` already admitted `auth().kind == "user"` unconditionally —
unscoped by `appId` — before this PR; nothing had ever exercised it because
no human token reached `sms-api`. Forwarding one means the Messages list and
Dashboard now show *every app* in this deployment to *any* signed-in human,
not just the console's own machine credential's one app — genuinely wider
visibility than before, and exactly the "cross-app visibility" #211's own
issue named as something it unblocks for #50. The messages list's own
live-update poll stays scoped to the machine credential's one app
(deliberately, see above), so a row belonging to another app can appear in
the initial list but won't receive a live update until the next refetch —
documented in `messages-screen.tsx`'s own module doc, not silently shipped.

**Proven live**, not just reasoned about: signed in as a freshly provisioned
`owner`, a `Provider` edit that previously 403'd now succeeds, and
`cratestack_audit`'s own `actor` column shows the real signed-in `User.id`
and `role: "owner"` — the audit trail can now say *who*. A second user
provisioned with `auditor` (which lacks `provider:update`) gets a real,
live `Forbidden` on the identical edit.

### 1.3 Where is this hosted, and does the 90-day decision hold? (settled, recorded for context)

Both parts are **answered** and listed here only so a future reader does not
reopen them by accident:

- **Hosting** ([#3](https://github.com/vymalo/vsms/issues/3)) — settled.
  Law No. 2024/017 requires prior authorisation for *all* cross-border
  personal-data transfers, and "legitimate interest" is not a lawful basis.
- **Retention** ([#5](https://github.com/vymalo/vsms/issues/5)) — settled
  2026-08-11: **90-day minimisation, no split ledger.** vsms purges content
  *and* plaintext MSISDN at 90 days and carries no ten-year traffic ledger.
  Long-horizon retention, if ever required, is infrastructure handled outside
  this application.

The residual risk in the second is worth stating once rather than leaving
implicit: Law 2010/012 art. 25 is the reason the question was open, and
delegating long-horizon retention to infrastructure is only true if somebody
actually configures it. **Nothing in this repository fails if nobody does.**

---

## 2. Claims nobody has verified against reality

These are not decisions. They are places where the system asserts something
that has never been tested against the real world, and where being wrong is
silent.

### 2.1 The MTN aggregator API shape is invented

`backends/crates/sms-provider-mtn` targets a **placeholder** request/response contract
— `POST {base_url}/v1/messages`, Bearer API-key auth, a `201` with a
`messageId`, and a DLR echoing it back. That shape was chosen to match the
common pattern across the aggregators §6.2 names as candidates; it was **not**
transcribed from any real vendor's documentation, because no aggregator
contract exists yet.

What *is* trustworthy in that crate: the `SmsProvider` impl and the
connect-vs-read `ProviderError` classification, which follow from what
`reqwest` itself guarantees rather than from any vendor's behaviour.

**When a real contract lands, replace the request/response structs — not the
trait impl or the error classification around them.** And revisit
`provider_ref_alt`, which is always `None` here purely because the *assumed*
DLR shape echoes the submit id back; that is a property of the guess, not a
proven simplification.

### 2.2 No message has ever reached a real handset

`docs/runbooks/36-handset-gate.adoc` is the acceptance gate for
[#36](https://github.com/vymalo/vsms/issues/36) and still requires a human
with a real Orange account and a real phone: a message reaching `delivered`
within 15 seconds, and a human-timed `kill -9` against a real (not mocked)
provider.

Everything automated around it — the chaos suite, the fake-Orange fault
injection, the kill-9 reclaim gate — proves the *system's* behaviour under
faults. None of it proves Orange behaves the way this code assumes.

### 2.3 A crash between submit and persistence sends the message twice

Known, tested, and accepted rather than fixed. `backends/apps/sms-worker/tests/kill9_reclaim_live.rs`
pins this down as a permanent regression assertion, not a one-off finding: a
`SIGKILL` in the window between an outbound submit and persisting
`providerMessageRef` produces two real submissions on recovery.

`providerMessageRef` has no database-level uniqueness constraint, and nothing
today gives Orange an idempotency key. **Closing this needs a provider-side
dedup key that no adapter currently sends.**

### 2.4 Grey-route detection has no ground truth ([#64](https://github.com/vymalo/vsms/issues/64)) — landed, gap narrowed not closed

Grey routes silently replace the sender ID, which breaks the Article 48
identity requirement and looks fine in every metric except delivery quality.
The issue proposed monthly handset validation per route plus an alert on
delivery-rate divergence between routes that should behave identically —
both landed (`backends/crates/sms-worker/src/jobs/grey_route_watch.rs`,
`RouteValidation`, `docs/runbooks/grey-route-validation.adoc`,
`sms-gateway record-route-validation`).

**What changed: the divergence half is now a real, gated statistical
proxy, and the validation half is now a real, queryable record of staleness
— not a placeholder for either.** "Routes that should behave identically"
is `Message.operator`/`Message.class` grouping, taken from the issue's own
text; a finding requires a 30-message sample floor, a two-proportion z-test
past a conservative threshold, and a 15-point practical-significance floor,
all three, specifically so a small sample never pages anyone. A `Route`
with no `RouteValidation` row in the last 30 days is now visible
(`sms_route_validation_overdue`) rather than invisible.

**What did not change, and cannot change without §2.2's own gate:** neither
half is, or claims to be, a trusted observation of what a handset actually
displays. The divergence check is built entirely from `DeliveryReceipt`-
adjacent outcome counts — a grey route's whole effect is invisible to every
one of them, per this entry's own opening paragraph, so it is evidence
worth investigating, never confirmation. The validation record is exactly
as trustworthy as the human who filed it and exactly as current as its own
`performedAt` — a route that turns grey the day after a passing check
reports "fine" until the next scheduled run or a divergence finding
happens to cross the statistical bar first. **There is still no baseline
to diverge *from* in the sense of "known-good," only "better than its own
peers right now"** — until §2.2's gate runs with real hardware, both halves
remain proxies, documented as such in `grey_route_watch.rs`'s own module
doc and the runbook's closing section.

---

## 3. Accepted limitations a reader should not mistake for bugs

Each of these is a deliberate trade with a stated reason. They are open in the
sense that a future requirement could reverse them.

### 3.1 Rotating the hash pepper permanently breaks matching against purged rows

`msisdnHash` survives the 90-day purge and is what post-purge correlation
(opt-out matching, dedupe) runs on. Rotating `SMS_HASH_PEPPER` does **not**
rehash stored rows. A row that still holds a plaintext `msisdn` could in
principle be rehashed by a job that does not exist; a row whose `msisdn` has
already been purged **never can be**.

So after `purge_retention` runs, a pepper rotation silently stops opt-out
matching from working against those rows, and nothing detects the mismatch.
See `backends/crates/sms-api/src/pepper.rs`'s own module doc.

### 3.2 An `Indeterminate` submit trades a possibly-lost message for never sending a duplicate

When a submit times out *after* connecting, the outcome is genuinely unknown,
and the message goes to `uncertain` rather than being retried or failed over.
It leaves the claim candidate set permanently.

**This is a product decision, not a technical one.** For OTP traffic it is
right: a duplicate OTP is worse than one the user re-requests. **If
notification traffic ever wants the opposite, the mapping must differ per
message class** — it currently does not.

### 3.3 Audit anchoring cannot detect deletion of the most recent anchor

The hash chain catches editing or deleting any covered audit row, and any
past anchor that a later anchor references. It cannot catch deletion of the
single newest anchor before anything references it — an anchor stored in the
same database an attacker already controls proves less than the phrase
"tamper-evident" suggests.

Closing this needs real external anchoring (an offsite copy, a notary, or WORM
storage). No such service exists in `deploy/`, and adding one is an
infrastructure dependency rather than a code change.

### 3.4 `occurredAt` on a webhook is the delivery time, not the event time

`WebhookAttempt` carries no creation timestamp, and the framework's own
`ModelEvent::occurred_at` is read and discarded before the row exists. The
`hooks` role stamps delivery time instead — accurate for a first attempt,
wrong by up to the full backoff span (24h) for one that only succeeds on its
last try.

Fixing it needs either a stored event timestamp on `WebhookAttempt` or
threading `occurred_at` through #38's subscriber.

### 3.5 Two `dispatch` workers cannot be detected by the Workers screen

Postgres's own exclusivity guarantee means `pg_locks` can never show two
granted rows for one role. If two workers were genuinely both `dispatch` — the
failure that screen exists to catch, since it means a blocked provider account
— it would be a bug bypassing the lease check entirely, and invisible there.
The screen states this rather than implying otherwise.

### 3.6 Neither a console-account password nor a service-account client key
### has a rotation/reset path — only provision-a-replacement

`provisionUser` (#52/#58) and `provisionAppClient` (#23) both mint a secret
exactly once — a one-time password, a private key — with no companion
procedure to issue a *new* one against an *existing* row. A user locked out
of their account, or a client that needs its key rotated without downtime,
has no self-service or admin-console recovery today:

- **A user's password** can only ever be set once, at `provisionUser` time.
  There is no write path to `UserCredential` other than that one `create`
  call — `backends/crates/sms-api/src/procedures.rs` has no `resetPassword`-shaped
  procedure, and `UserCredential` itself is `hasRole('system')`-only on
  every action, so nothing short of a new procedure could add one. A locked-
  out account's only recovery is an `owner`/`admin` provisioning a
  *replacement* user under a different email and deactivating the old one —
  the account itself, and its own audit history under that identity, cannot
  be recovered in place.
- **A service-account client's key** has the coarser, but real, fallback
  `AppClient.active`/`retiredAt` already documents (#23, restated in #52's
  own admin screen): retire the old client, provision a new one, migrate the
  integration. That is a real answer, just not a zero-downtime one — there
  is no overlap window (see `@vsms/gateway/app-clients.ts`'s own module
  doc).

Both are the *coarse fallback* this codebase's own convention prefers to
state plainly rather than silently accept: #52/#58's own admin screens do
not hide either gap behind a UI that implies a reset button exists.
Building either a real reset flow needs a decision this file's own opening
paragraph asks for before writing code: how does a caller *prove* they are
the account holder before a new secret is issued to them — email-verified
token, a break-glass CLI command run by someone with database access, or
something else? Nobody has decided.

---

## 4. Framework questions with a filed answer pending

Two upstream bugs and two evaluations, all of which change how this code
should be written once resolved. **Updated 2026-08-14 (the cratestack 0.7.16
bump): all three filed upstream bugs closed between 0.7.11 and 0.7.15 — see
each row.** None of the fixes were adopted (removing a workaround is a real
schema/code change with its own risk, out of scope for a pure dependency
bump), but the blocking reason for each is gone.

**Re-checked 2026-08-18 (the cratestack 0.8.3 bump): every row below is
unchanged.** Nothing in `v0.7.16...v0.8.3` touches `auth().isSystem()` or
`.upsert().do_nothing()`, and the three closed bugs stay closed with their
workarounds still in place here — 0.8.x's own breaking changes (the
`Cool*` → `Cratestack*` rename, additive decimal backends, the `--tanstack`
gate) are unrelated to all five. Each remains a deliberate follow-up rather
than something a dependency bump should spend its budget on.

| Question | Where | State |
|---|---|---|
| Does `cratestack studio`'s direct-DB mode intend to bypass `@version` and `@@emit`? | [cratestack#507](https://github.com/cratestack/cratestack/issues/507) | **Closed 2026-08-13, cratestack 0.7.13** (PR [cratestack#553](https://github.com/cratestack/cratestack/pull/553): `[target.db]` writes now route through the same descriptor path every other write does, rather than being refused outright; PR [cratestack#557](https://github.com/cratestack/cratestack/pull/557) fixed a related no-payload SQL-preview duplication in the same window). Not verified live against `cratestack studio` by this PR — `docs/roadmap.md`'s own #46 section already disqualified Studio from any deployed vsms surface for unrelated reasons (no procedure surface, bypasses `@@allow`), so nothing here currently depends on the fix. |
| Should `@length` on a nullable field compile? | [cratestack#537](https://github.com/cratestack/cratestack/issues/537) | **Closed 2026-08-13, cratestack 0.7.13** (PR [cratestack#546](https://github.com/cratestack/cratestack/pull/546)). The two workarounds this repo carries — `AuditAnchor.prevChainHash`'s non-null sentinel, `RouteValidation.notes`'s dropped `@length` bound (#64) — are now removable in principle, but reverting either is a real `schema.cstack` edit with its own migration-regeneration and re-verification cost, not attempted in this dependency-bump PR. Left for a follow-up that wants to spend that budget deliberately. |
| Should `auth().isSystem()` replace the `hasRole('system')` convention? | [#176](https://github.com/vymalo/vsms/issues/176) | Not evaluated. Would touch the gap this codebase has hit **eleven times**. **Unchanged by 0.7.16** — no commit in `v0.7.10...v0.7.16` touches `isSystem` (it landed in cratestack 0.7.10 itself, per cratestack#500; nothing in the six releases since built on it further). |
| Should `.upsert().do_nothing()` replace create-then-catch-`23505`? | [#177](https://github.com/vymalo/vsms/issues/177) | Not evaluated. Affects `ClientAssertion`, seed-dispatch, and scheduler dedupe. **Unchanged by 0.7.16** — same as above: landed in cratestack 0.7.10 (cratestack#501), no further work on it in `v0.7.10...v0.7.16`. |
| Can a generated `PATCH` route clear a nullable field at all? | [cratestack#567](https://github.com/cratestack/cratestack/issues/567) | **Closed 2026-08-13, cratestack 0.7.15** (PR [cratestack#574](https://github.com/cratestack/cratestack/pull/574), "distinguish PATCH null-clear from omitted field on nullable columns" — marked breaking upstream). `@vsms/gateway/senders.ts`'s `foldClearedSentinel` empty-string workaround for `SenderId.notes`/`SenderIdRegistration.reference`/`rejectionReason` is now removable in principle, but the fixed wire shape has not been verified live against this exact schema, and the workaround lives in `frontends/packages/gateway` (adjacent to `frontends/apps/admin/`, which a concurrent console redesign is actively touching) — reverting it is left as a deliberate follow-up, not attempted here. |

---

## 5. Questions this session raised and did not settle

Recorded because they came out of doing the work, and would otherwise be lost.

- **Do the six seeded role permission sets match §5.2's intent?** §5.2's table
  is prose ("everything", "all except role editing and owner-level deletes").
  Seeding it required expanding that into explicit lists for the first time —
  a judgement call that deserves a second reader.
- **Should built-in roles live in `0002_bootstrap` or a `seed-roles` command?**
  `Role.builtin` implies they are part of the schema's definition, which is why
  they went into the migration. `seed-dispatch` is precedent for the other
  shape.
- **Should CI verify migrations match the schema?**
  ([#204](https://github.com/vymalo/vsms/issues/204)) Nothing does today. The
  check was performed by hand on every schema-touching PR in this session,
  which is exactly the kind of discipline that stops happening.
- **Is `frontends/packages/sms-client` meant to be committed?** It is generated, partly
  tracked, and not imported at runtime by anything. `frontends/packages/gateway`'s
  hand-rolled seam exists to make swapping to it a one-package change, but
  nobody has decided when that swap happens.
