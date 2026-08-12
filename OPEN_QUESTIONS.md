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

**Status is a dated snapshot** *(2026-08-12)*. GitHub is the tracker and is
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

### 1.2 Should the console act as the logged-in human? ([#211](https://github.com/vymalo/vsms/issues/211))

**Blocks:** [#52](https://github.com/vymalo/vsms/issues/52) (apps and service
accounts), [#58](https://github.com/vymalo/vsms/issues/58) (users and roles),
cross-app visibility in [#50](https://github.com/vymalo/vsms/issues/50), and
the *allow* half of every Layer-2 permission check.

#194 shipped a real authorization_code + PKCE login: a person signs in, a
session is issued, and `GatewayAuth` resolves a genuine `User` → `Role` →
`perms` principal. But `packages/gateway` still authenticates **upstream**
with its own machine credential, so the human's role never reaches the
gateway. Verified by signing in as a freshly provisioned `owner` and watching
a `Provider` edit fail with `missing required permission "provider:update"`.

The open part is not *whether* to fix it but *how*, and the two shapes differ
in security surface:

- **Forward the human's `id_token`** and let `GatewayAuth` validate it
  directly. #194 already validates human tokens, so this may be nearly free —
  but audience and expiry handling is exactly where that shortcut goes wrong.
- **Exchange the session for a per-request token.** More moving parts, but
  keeps the browser's cookie and the gateway's credential separate.

Either way, a second decision follows: calls that are legitimately *not*
user-initiated — the message-stream poller, health checks — may still want
the machine credential, which argues for both paths existing rather than a
straight swap.

**Consequence while unanswered:** `@@audit` attributes every console write to
one client id, so the audit log cannot say *who* did anything. That is
precisely what #68's anchoring exists to make defensible.

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

`crates/sms-provider-mtn` targets a **placeholder** request/response contract
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

`docs/runbooks/36-handset-gate.md` is the acceptance gate for
[#36](https://github.com/vymalo/vsms/issues/36) and still requires a human
with a real Orange account and a real phone: a message reaching `delivered`
within 15 seconds, and a human-timed `kill -9` against a real (not mocked)
provider.

Everything automated around it — the chaos suite, the fake-Orange fault
injection, the kill-9 reclaim gate — proves the *system's* behaviour under
faults. None of it proves Orange behaves the way this code assumes.

### 2.3 A crash between submit and persistence sends the message twice

Known, tested, and accepted rather than fixed. `app/sms-worker/tests/kill9_reclaim_live.rs`
pins this down as a permanent regression assertion, not a one-off finding: a
`SIGKILL` in the window between an outbound submit and persisting
`providerMessageRef` produces two real submissions on recovery.

`providerMessageRef` has no database-level uniqueness constraint, and nothing
today gives Orange an idempotency key. **Closing this needs a provider-side
dedup key that no adapter currently sends.**

### 2.4 Grey-route detection has no ground truth ([#64](https://github.com/vymalo/vsms/issues/64))

Grey routes silently replace the sender ID, which breaks the Article 48
identity requirement and looks fine in every metric except delivery quality.
The issue proposes monthly handset validation per route plus an alert on
delivery-rate divergence between routes that should behave identically.

Both require something this system does not have: **a trusted observation of
what actually arrived on a handset.** Until §2.2's gate runs with real
hardware, there is no baseline to diverge *from*.

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
See `crates/sms-api/src/pepper.rs`'s own module doc.

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

---

## 4. Framework questions with a filed answer pending

Two upstream bugs and two evaluations, all of which change how this code
should be written once resolved.

| Question | Where | State |
|---|---|---|
| Does `cratestack studio`'s direct-DB mode intend to bypass `@version` and `@@emit`? | [cratestack#507](https://github.com/cratestack/cratestack/issues/507) | Filed 2026-08-12. A Studio write leaves a stale version that a later `if_match` still accepts, and fires no outbox events. |
| Should `@length` on a nullable field compile? | [cratestack#537](https://github.com/cratestack/cratestack/issues/537) | Filed 2026-08-12. Breaks the generated `Update{Model}Input::validate()`; worked around here with a non-null sentinel. |
| Should `auth().isSystem()` replace the `hasRole('system')` convention? | [#176](https://github.com/vymalo/vsms/issues/176) | Not evaluated. Would touch the gap this codebase has hit **eleven times**. |
| Should `.upsert().do_nothing()` replace create-then-catch-`23505`? | [#177](https://github.com/vymalo/vsms/issues/177) | Not evaluated. Affects `ClientAssertion`, seed-dispatch, and scheduler dedupe. |

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
- **Is `packages/sms-client` meant to be committed?** It is generated, partly
  tracked, and not imported at runtime by anything. `packages/gateway`'s
  hand-rolled seam exists to make swapping to it a one-package change, but
  nobody has decided when that swap happens.
