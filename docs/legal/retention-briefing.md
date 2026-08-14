# Data retention briefing for counsel

**Purpose.** This document exists to make legal review of [#5](https://github.com/vymalo/vsms/issues/5) fast and cheap. It is a factual summary written by the engineering team (with AI assistance — see the PR that introduced this file) so that counsel can answer without reading the codebase. **It is not legal advice, contains no legal conclusions, and nothing in it should be read as a compliance determination.** Where a claim comes from the system's own design document rather than something independently verified, that is stated explicitly.

Everything below reflects the system as designed and partially built as of this writing. No live production traffic exists yet (see "What is blocked," below) — this is a pre-launch review, not a remediation of an existing violation.

---

## 1. What the system is and what data it holds

VSMS is an application-to-person (A2P) SMS gateway for Cameroon, sending one-time-password (OTP) and notification text messages through MTN and Orange as the underlying carriers. A client application (the "App") calls an API to send a message to a phone number; the gateway routes it to a carrier, tracks delivery status, and exposes the result. There is no consumer-facing product — the direct actors are the businesses whose Apps send messages, and the ultimate data subjects are the phone-number holders who receive them.

The concrete data categories, and where each lives, drawn from the system's schema definition (`schemas/vsms.cstack`):

| Data | Field(s) | Model | Personal data? | Notes |
|---|---|---|---|---|
| Recipient phone number (MSISDN), plaintext | `msisdn` | `Message`, `OptOut` | Yes — marked `@pii` in the schema | E.164 format, e.g. `+237...` |
| Recipient phone number, hashed | `msisdnHash` | `Message`, `OptOut` | Pseudonymised — see §1a below | Used for dedupe, opt-out matching, analytics |
| Message text | `body` | `Message` | Yes | **Stored in plaintext for all classes, including OTP** — see below |
| Message text, hashed | `bodyHash` | `Message` | Pseudonymised — same scheme as `msisdnHash` | |
| Delivery status / carrier response | `state`, provider refs, raw carrier payload | `Message`, `DeliveryReceipt` | Metadata; `DeliveryReceipt.rawPayload` may echo carrier-side data | |
| Audit trail (before/after snapshots of every write) | — | `cratestack_audit` (framework table, not a schema model) | Redacted at write time — see §1a | Captures actor, operation, timestamps |
| Outbound event/webhook payloads | `to` (recipient), `clientRef`, state, etc. | `cratestack_event_outbox` → delivered to app-configured webhook endpoints | Yes, unless masked | `to` is masked by default (`WebhookEndpoint.maskRecipient`, default-on per the design doc), but an app can turn masking off for its own webhook |

**A known defect, disclosed here because it changes the facts counsel is reasoning about.** The design document states that for `class = otp` the send procedure sets `body = null`, storing only the hash and length, on the reasoning that "an OTP gateway that stores OTP plaintext for 90 days is a credential database." **The implementation does not do this.** `backends/crates/sms-api/src/procedures.rs:624` writes `body: Some(body)` unconditionally, with no message-class check, so one-time-password text is currently retained in plaintext for the full 90-day window and is readable over the API (the `@sensitive` marker redacts audit snapshots only — it is not a confidentiality control). Tracked as issue #165. **It has since been decided not to redact OTP bodies**, so counsel should treat OTP content as **stored, and remaining stored**, not as a defect about to disappear.

The reasoning, so counsel can weigh it rather than take it on trust. Redaction at send time is not implementable: the sending worker is a separate process that reads the body back off the stored row, so a null body fails the send outright. Redaction later, once the message reaches a final state, *was* built and then rejected — the code has a 15-minute validity window, so it is worthless as a credential long before the 90-day retention period is relevant, and the row already discloses the sender identity and destination number regardless, so removing the text conceals little while costing the ability to investigate delivery complaints. The engineering judgement was therefore that retention of spent OTP text is a **data-minimisation** question — whether it is lawful to keep content that no longer serves a purpose — rather than a security one. That judgement is precisely what we are asking counsel to confirm or overturn.

### 1a. The hash is keyed, and that distinction matters for minimisation

`msisdnHash` and `bodyHash` are **not** general-purpose one-way hashes of the kind that can be produced by anyone. As of change [#134](https://github.com/vymalo/vsms/issues/134) (merged), they are HMAC-SHA256, computed under a secret key ("pepper") held only by the server (`backends/crates/sms-api/src/pepper.rs`), stored as `hmac-sha256-v1:<hex>`.

This matters because **before #134, the same columns were plain, unkeyed SHA-256** — a hash with no secret key, computable by anyone. Cameroon's mobile numbering space is small enough (~10⁷ candidate numbers, per the codebase's own MSISDN-parsing rules) that an unkeyed hash of a phone number can be reversed by brute force in seconds: an attacker (or anyone with database access) could recompute SHA-256 over every valid Cameroonian mobile number and match it against the stored hash. That means a "purge" that deleted the plaintext `msisdn` column but kept the old `msisdnHash` had **not actually de-identified the row** — the phone number was recoverable from the hash alone.

The keyed (HMAC) version is different in kind: without the server-held pepper, an attacker cannot precompute the hash space, because the pepper is an unknown input to the hash function. This is the technical fact question 1 in §4 below is asking counsel to evaluate — engineering believes the keyed hash is a materially stronger de-identification technique than the unkeyed one that preceded it, but whether it meets the legal bar for "de-identified" or "anonymised" under Law No. 2024/017 is a legal judgment, not an engineering one.

One more fact relevant to any minimisation design: rotating the pepper (changing the secret key) does not retroactively rehash already-stored rows, and a row whose plaintext `msisdn` has already been purged can **never** be rehashed — there is no plaintext left to rehash from. This is documented in the pepper module's own code comments and is discussed further in §5.

### 1b. Retention periods currently declared in the schema (not yet enforced by a purge job)

The schema declares intent via a `@@retain(days: N)` annotation, but **no purge job exists yet** — see §5. Current declared values:

| Model | Declared retention | Contains |
|---|---|---|
| `Message` | 90 days | plaintext `msisdn`, optional `body`, hashes, delivery metadata |
| `DeliveryReceipt` | 90 days | carrier delivery status, raw carrier payload |
| `Job` (internal work queue) | 14 days | operational, not personal data |
| `WebhookAttempt` (outbound webhook delivery log) | 30 days | may echo message metadata sent to an app's own webhook |
| `OptOut` (STOP/suppression list) | **no declared limit** | plaintext `msisdn` + hash, indefinite by design — this is the record that prevents re-sending to someone who opted out, and dropping it would defeat its own purpose |

These are the schema's declared intentions, not verified against any legal requirement — they are exactly what's in question in this briefing.

---

## 2. The two obligations, stated as the conflict they are

Two Cameroonian statutes point in opposite directions for the same data:

**Law No. 2010/012 of 21 December 2010 (cybersecurity/cybercriminality), Article 25(1).** Per the design document's reading, this article requires operators and electronic communications service providers to retain **connection and traffic data for ten years**. VSMS's own legal-classification question — whether it counts as an "electronic communications service provider" for this purpose, and whether "traffic data" reaches message content or only metadata (timestamp, parties, routing, status) — is exactly the kind of question this briefing is not equipped to answer and is asking counsel to resolve (see §4, question 2).

**Law No. 2024/017 of 23 December 2024 (data protection).** Per the design document's reading: consent must be opt-in and explicit — "legitimate interest" is not a recognised lawful basis, which the design doc characterises as stricter than GDPR — and data minimisation principles push toward not retaining personal data (including a phone number) longer than necessary for the purpose it was collected for. The schema's 90-day retention windows above were sized against this reading. The design doc also states this law requires prior registration with a data protection authority before processing personal data, and prior authorisation for any cross-border transfer of personal data — relevant to the adjacent question in §6.

**The conflict as stated in issue #5:** a straightforward reading of "keep traffic data 10 years" and "minimise personal data, don't over-retain" cannot both be satisfied by keeping the same row, in the same shape, for both periods. Something has to give — either the retained data changes shape at some point, or one obligation is read narrower than its plain text suggests (e.g., "traffic data" excludes the *recipient identity* while covering everything else), or there is a data-protection-law carve-out for retention that is itself a statutory or regulatory obligation. None of that has been resolved; it is exactly what's being asked in §4.

---

## 3. §10's proposed split ledger — a proposal, not a decision

Section 10 of the system's design document (`docs/architecture.md`) proposes one way to reconcile the two obligations, explicitly flagged there as needing a lawyer's sign-off, not as a decision already taken:

- **Keep, for the full statutory period (currently read as ten years):** a minimal traffic-metadata record — timestamp, the **hashed** MSISDN (not plaintext), operator/carrier, segment count, and final delivery state.
- **Purge at 90 days:** message content (`body`) and the **plaintext** MSISDN (`msisdn`). The hash stays; the reversible plaintext goes.

The reasoning offered in the design doc: this shape is "most likely to satisfy both" obligations, because it keeps something that could plausibly count as "traffic data" for the full statutory window while removing the personal, directly-identifying, and content elements early. The doc is explicit that this is a proposal aimed at counsel, not an implemented or legally validated design.

Two things worth flagging honestly:

- The proposal has not been tested against the actual text or any interpretive guidance for either statute — it's an engineering best guess at a shape that might work, informed by the general pattern of "pseudonymise long-retained records."
- Whether a **hashed** MSISDN, even keyed as described in §1a, is legally equivalent to "no personal data" for the purposes of either statute is precisely open — see §4 question 1. If it is not, the split-ledger proposal may not resolve the tension at all, because the retained record would still be personal data under Law No. 2024/017 for the full ten years.

---

## 4. Questions for counsel

These are framed to be answerable, and each answer changes what gets built:

1. **Does the keyed HMAC-SHA256 pseudonymisation described in §1a count as "de-identified" or "anonymised" for the purposes of Law No. 2024/017's minimisation obligation** — such that a record containing only `msisdnHash` (not plaintext `msisdn`) falls outside, or under a relaxed version of, the minimisation duty? Or does a keyed hash that is still capable of being matched against a *known* candidate number (i.e., it is reversible if you already suspect a specific number, even if not brute-forceable at scale) still count as personal data for legal purposes?
2. **Does the Article 25(1) ten-year retention duty attach to "traffic data" in a way that covers message content, or is it limited to connection/routing metadata** (parties, timestamps, routing path, delivery status) **and does it require the recipient's identity to be retained in identifiable (plaintext) form, or would a pseudonymised/hashed identifier satisfy it?**
3. **What is the lawful retention period for each of the following, specifically:** (a) plaintext recipient MSISDN, (b) message content/body, (c) hashed MSISDN + delivery metadata, (d) the opt-out/suppression list (which the system currently proposes to retain indefinitely, since its purpose — never messaging someone who opted out — has no natural expiry)?
4. **Is VSMS (an A2P messaging gateway routing through licensed carriers, not itself a network operator) within the class of entities Article 25(1) applies to** ("operators and electronic communications service providers"), or is that duty scoped to the carriers (MTN/Orange) rather than to a service built on top of them?
5. **If the split-ledger approach in §3 is broadly the right shape, what specific fields may the long-retained record contain** — is a hashed MSISDN acceptable, or does the ten-year record need to be structured differently (e.g., a separate, more restricted-access table; additional consent language; a different pseudonymisation technique)?
6. **Does the answer differ by message class** (OTP vs. notification/marketing)? Note the correction in §1: contrary to an earlier revision of the design document, OTP bodies are stored in plaintext for 90 days (issue #165), alongside notification bodies, and — following the reasoning set out in §1 — the current decision is to keep storing them. **If your answer to any question above turns on whether one-time-password content is retained, please say so explicitly.** That decision was made on engineering grounds (a spent 15-minute code is not a credential); if minimisation law reaches a different conclusion, it is the deciding input and the decision will be revisited. Please also say whether your answer would differ if OTP bodies were purged earlier than 90 days but not immediately, since a shorter uniform retention for message content is the cheaper remedy than an OTP-only carve-out.

---

## 5. What is blocked pending the answer, and the cost of delay

**[#67](https://github.com/vymalo/vsms/issues/67)** — implementing the retention purge job — is blocked directly on this decision. Its own issue text states the reason plainly: "the split determines which columns survive, so writing this first means writing it twice." No purge job exists in the codebase today; the `@@retain(days: N)` annotations in the schema (§1b) currently declare intent only and are not enforced by any running process.

**Why late is more expensive than early**, based on facts already documented in the codebase:

- **Rows accumulate from the first production message onward.** There is no live database with real traffic yet, but the moment there is, every day of delay is a day of rows added under the *current*, legally-unvalidated retention shape — rows that may need to be reshaped, re-hashed, or purged retroactively once counsel answers.
- **A purged row cannot be un-purged.** Per `backends/crates/sms-api/src/pepper.rs`'s own documentation, once a row's plaintext `msisdn` is deleted, that row's hash can never be recomputed under a new hashing scheme (e.g., if the pepper rotates, or if counsel requires a different pseudonymisation approach) — there is no plaintext left to rehash from. If the eventual answer requires a different retention shape than what's implemented first, rows already purged under the old shape are stuck; only rows that still have plaintext at the time of the change can be migrated forward. Every day of production traffic under the wrong shape is data that a later correction cannot reach.
- **This makes the ordinary "we can fix it later" software assumption false here.** For most engineering decisions, shipping something imperfect and correcting it later is cheap. For this one, it specifically is not, because purging is a one-way door per row.

---

## 6. Adjacent question worth asking the same counsel together

**[#3](https://github.com/vymalo/vsms/issues/3) — hosting location and cross-border transfer**, tracked separately, is not the subject of this briefing but shares the same statute (Law No. 2024/017) and likely the same counsel. Per the design document's reading, the law requires prior authorisation for *all* cross-border personal-data transfers, and does not recognise "legitimate interest" as a lawful basis for processing. Hosting the gateway and its database outside Cameroon — or routing through an offshore aggregator — would itself be a cross-border transfer under that reading. This bears on retention because the *location* of the long-retained ten-year record (§3) may itself require the cross-border question to be settled first, or the two may need a single combined answer. It is flagged here only to save counsel a second engagement; no analysis of #3 is attempted in this document.

---

## Sources

- [#5](https://github.com/vymalo/vsms/issues/5) — the issue this briefing exists to support.
- [#67](https://github.com/vymalo/vsms/issues/67) — the blocked purge implementation.
- [#3](https://github.com/vymalo/vsms/issues/3) — the adjacent hosting/cross-border question.
- [#134](https://github.com/vymalo/vsms/issues/134) — the fix from unkeyed to keyed hashing.
- `docs/architecture.md` §10 (Compliance), §2.5 (the `Message` model), §13 (Risks and open questions) — the design document's own analysis, quoted/paraphrased and attributed throughout above.
- `schemas/vsms.cstack` — the actual schema definitions for `Message`, `DeliveryReceipt`, `Job`, `WebhookAttempt`, `OptOut`.
- `backends/crates/sms-api/src/pepper.rs` — the hashing implementation and its own documentation of the rotation/purge consequence.
