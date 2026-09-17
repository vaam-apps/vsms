# ADR: GDPR engineering readiness

**Status:** accepted for implementation

**Date:** 2026-09-13

**Issues:** [#371](https://github.com/vaam-apps/vsms/issues/371),
[#372](https://github.com/vaam-apps/vsms/issues/372),
[#373](https://github.com/vaam-apps/vsms/issues/373),
[#374](https://github.com/vaam-apps/vsms/issues/374),
[#375](https://github.com/vaam-apps/vsms/issues/375),
[#376](https://github.com/vaam-apps/vsms/issues/376), and
epic [#377](https://github.com/vaam-apps/vsms/issues/377)

**Implementation RFC:**
[`gdpr-engineering-readiness-rfc.md`](gdpr-engineering-readiness-rfc.md)

## Context

vsms processes recipient MSISDNs, message content, delivery metadata, provider responses,
webhook payloads, authentication data, audit records, and backups. Existing controls include
HMAC-based pseudonymisation, a 90-day message/receipt purge, audited writes, audit anchoring,
app-scoped model policies, operator RBAC, and a restore drill. They are useful foundations, but
they do not yet form a complete, testable data lifecycle:

- message content and delivery metadata share one retention lifecycle;
- an expired `accepted`, `queued`, or `routed` message can fall out of the claim set without being
  transitioned to a terminal state, leaving it outside the terminal-only retention purge;
- provider-controlled response bodies can become durable `Message.stateReason` values;
- `Job` and `WebhookAttempt` declare retention periods that no scheduled enforcement applies;
- restore can reintroduce data whose retention deadline passed after the backup was taken;
- no explicit, scoped legal-hold mechanism exists;
- no operator workflow can produce a tenant-bounded subject or incident export; and
- generic model audit records do not provide the authentication-event history an incident
  investigation needs.

The epic covers technical controls and evidence. It does not decide controller or processor roles,
lawful bases, consent validity, disclosure approval, breach notification, contracts, transfer
mechanisms, privacy notices, or Article 42 certification.

## Decision

### Maintain a code-grounded inventory

Every schema field and every known material non-schema data surface will be classified in a
version-controlled inventory. A build check will require a human classification when a schema field
is added or removed. Non-schema coverage combines a maintained registry with static discovery of
known datastore, telemetry, and external-I/O patterns; it is a review aid, not a proof that arbitrary
new code cannot create an unregistered surface. The inventory may record `decision-required`; it
must never invent a legal basis or organisational role.

### Separate content from metadata

Message content and delivery metadata will have independent, explicit deadlines stored on each
message. The applicable content class is derived server-side from the message class. A message may
not be scheduled or remain retryable beyond the point at which delivery still requires its body.

Submission cutoff and physical deletion are separate instants. Dispatch checks the cutoff before
claim and immediately before external I/O. Deletion waits for any valid dispatch lease and a bounded
provider-call safety interval, so retention cannot erase a row while a request already in flight is
accepted externally. The policy declares and monitors the maximum deletion lag; it does not call a
row deleted merely because its eligibility deadline passed.

The previous decision to retain OTP bodies for 90 days is reopened. This ADR does not choose a new
duration. Authentication-content, ordinary-content, and metadata durations are maintainer/legal
inputs that must be recorded before the corresponding implementation is merged.

### Prevent telemetry from becoming a second datastore

Logs, traces, metrics, alerts, provider errors, and webhook diagnostics will use approved structured
fields and identifiers. Provider-controlled bodies and arbitrary error text will be filtered before
they cross the diagnostic boundary. Redacting an already-formatted arbitrary string is not an
acceptable primary control.

### Enforce retention through one policy

Retention declarations must map to executable, idempotent rules. Scheduled enforcement and
post-restore reconciliation will use the same policy and clock semantics. Enforcement runs to drain
a bounded backlog, reports the oldest overdue row, and alerts when the policy's maximum deletion lag
is exceeded. A build check will prevent a model from declaring retention without a registered
enforcement rule.

State transitions remain subject to R2: Rust proposes a transition and Postgres decides whether it
is legal. Data access remains subject to R1: enforcement uses CrateStack delegates except for an
explicitly reviewed, named exception where no model exists.

### Make holds explicit

A retention hold is an audited record, never an environment flag or silent purge bypass. It has a
mandatory app boundary, a constrained scope, a reason, an actor, and lifecycle timestamps. A hold
contains no plaintext recipient identifier. Holds expire by default; whether any indefinite hold is
permitted remains a policy decision rather than a nullable field chosen in advance. Releasing or
expiring a hold makes already-overdue data eligible on the next enforcement pass.

No destructive retention rule is enabled until the hold policy says whether that data category can
be held and, if it can, the hold lookup exists. This includes short-lived message content.

The independent retention-control journal is authoritative; the database hold table is its queryable
projection. A hold mutation acquires the same target-scoped advisory lock as destructive enforcement,
appends an idempotent event to the journal, then advances the database projection and its journal
watermark. If projection fails after append, the event remains authoritative and deletion fails
closed until replay catches up. Destructive enforcement acquires the lock and refuses to act unless
the projection watermark matches the authoritative journal. This avoids pretending two stores can
participate in one database transaction.

### Reconcile before restored data becomes live

Restore is not complete when `pg_restore` exits. The restore workflow must first replay the current
retention-control journal, then apply current retention rules before services are allowed to consume
the restored database, and must fail closed when either step fails. The append-only control journal
is stored independently from the database snapshot and records hold creation, release, and any later
out-of-cycle erasure decision. This prevents an old snapshot from reviving a released hold or omitting
a hold created after the backup. Backup objects remain personal-data stores until their own retention
expires; this decision does not claim otherwise.

Time-based expiry can be recomputed from a restored row. Immediate, subject-requested erasure outside
that lifecycle is not part of this epic; if it is added later, its tombstone uses the same independent
control journal before the feature can claim restore safety.

### Version recipient lookup keys

Recipient pseudonyms used for suppression, holds, or export are derived from a dedicated versioned
subject-lookup keyring. Every persisted subject hash records its key version. Lookup computes a
candidate under every retained version, and a key cannot be retired while any live enforcement or
export record depends on it. Body hashes follow the content lifecycle and are not reused as subject
lookup keys. Restore validates availability of every referenced key version before reconciliation.

The current `SMS_HASH_PEPPER` becomes subject-key version `v1` at migration. All gateway, worker, and
restore processes receive the same keyring plus one active version and fail at startup when a
referenced version is absent. Rotation is an explicit operator command, not an uncoordinated secret
replacement. Journal compaction may discard released control events only after every restorable
backup predating them has expired and a verified checkpoint preserves current state.

### Retire audit and external records deliberately

Audit rows, audit anchors, backup objects, and retention-control events each have explicit policy and
enforcement ownership. Audit rows retire only in contiguous, already-anchored ranges after a verified
retention checkpoint is written outside the database; the next retained range links to that
checkpoint rather than silently breaking the chain. Backup deletion or journal compaction failure is
an unhealthy enforcement outcome, not a successful run with a warning. A subject-key version cannot
be retired while any live row, journal checkpoint, or restorable backup still references it.

### Export allowlisted projections

Subject and incident exports will use purpose-built projections. They will not serialize generated
schema records wholesale and will not expose unfiltered `cratestack_audit` snapshots. Every export
requires a dedicated permission, a mandatory app boundary, bounded selectors, and its own audited
invocation record. Export bodies and plaintext search criteria are not persisted in that record.

The authorization model stays global for human privacy operators, matching current RBAC. App
isolation is enforced by a mandatory query boundary and tested with the same subject in two apps.
This ADR does not add user-to-app assignments.

### Record safe security events

Authentication and privacy-relevant outcomes needed for incident investigation will be recorded in
an append-only event model with an explicit, enforced retention period. It stores fixed-cardinality
identifiers and reason codes, not tokens, assertions, passwords, private keys, OTPs, message content,
or raw provider payloads. Unauthenticated refusal recording is admitted only behind endpoint rate
limits and a bounded failure policy, so an attacker cannot turn malformed requests into unlimited
durable writes.

### Keep country and operator concerns separate

Canonical E.164 is the portable subject identity. Explicit country data is used once multi-country
stage 2 (#356) lands, but this program does not depend on that separate schema change. An export,
retention rule, or hold must not infer jurisdiction or subject identity from a Cameroonian operator
prefix. Cameroon remains the zero-configuration default, while foreign traffic may legitimately
carry an unknown operator.

## Consequences

- The schema gains explicit deadlines, lifecycle markers, hold records, export-audit records, and
  security events.
- Content can disappear while routing and delivery history remains intelligible.
- Retry eligibility and content availability become one enforced invariant.
- Restore takes longer and depends on the retention-control journal and referenced subject-key
  versions because replay and reconciliation are part of successful recovery.
- Privacy exports are reviewable artifacts rather than direct database dumps.
- A global privacy operator can select an app, but cannot omit the app boundary.
- Existing HMAC values are pseudonymous, not anonymous. Their retention and key lifecycle require an
  explicit policy decision.
- Audit retention cannot be changed casually: rewriting or deleting covered rows affects audit-anchor
  verification and requires a separately designed chain transition.
- The console remains optional under R4. Every operator-only workflow also has a backend command.

## Rejected alternatives

### Keep one 90-day lifecycle for everything

This retains high-risk content merely because lower-risk delivery metadata remains useful, and does
not satisfy the independent-content-retention requirement.

### Sanitize each log call independently

This makes safety dependent on every future caller remembering the same rules and lets
provider-controlled strings bypass local assumptions.

### Treat keyed hashes as anonymous data

They remain linkable by a holder of the pepper and a candidate identifier. Calling them anonymous
would overstate the control and lead to unjustified indefinite retention.

### Use raw model or audit JSON as exports

Generated records and audit snapshots contain fields unrelated to the request and can include
secrets or provider-controlled payloads. An allowlist is easier to review and safer to evolve.

### Make a hold an environment flag

An environment flag has no subject, app, reason, actor, expiry, or release history. It creates an
unbounded and unauditable bypass.

### Document post-restore cleanup without enforcing it

That leaves a window in which expired plaintext can be read or acted on after restoration. Recovery
must fail closed until reconciliation succeeds.

### Add per-app human membership in this epic

Current human RBAC is deployment-global. A membership model would be a broader authorization redesign
than the subject and incident export requirements need. Dedicated permissions plus mandatory app
selection provide the required export boundary without that expansion.

## Unresolved policy inputs

Implementation must stop rather than invent answers for:

- authentication-content and ordinary-content durations;
- metadata retention duration and the event from which it is measured;
- consent, opt-out, security-event, audit, webhook, and backup retention;
- whether recipient lookup hashes survive metadata expiry; body hashes always follow content;
- hold-authorizing roles, maximum duration, review cadence, and whether a hold covers content;
- maximum enforcement/deletion lag and backlog alert threshold;
- whether still-retained plaintext content may appear in a subject export; and
- which source-network attributes, if any, may be kept in authentication evidence.

## Scope

This ADR approves the architecture and implementation sequence. It does not close any linked story,
change the schema, select retention durations, or claim GDPR compliance or Article 42 certification.
