`anchor_audit` — #68. §7.5's own table: "Merkle root of the day's audit
rows → append-only store." The issue itself: "the audit log is only
worth having if it can be shown not to have been edited. Periodic hash
anchoring so tampering is detectable... `@@audit` produces the rows;
this is about making them defensible."

# #58: the hashing/verification machinery moved to `sms_api::audit_log`

Everything about what the hash chain over `cratestack_audit` *means* —
the row hash, the fold, the chain-hash computation, and the two
verification functions ([`sms_api::audit_log::verify_chain_linkage`],
[`sms_api::audit_log::verify_period_content`]) — used to live in this
file. It moved to `sms_api::audit_log` because #58's own console
audit-log screen needed the identical logic to answer "does this
period's chain verify," and `sms-api` cannot depend on `sms-worker` (the
dependency runs the other way), so duplicating a hash-chain algorithm
between "the thing that writes it" and "the thing that verifies it" was
rejected outright — see that module's own doc for the full reasoning.
This file keeps only what's genuinely its own: [`ANCHOR_LAG`] (a
scheduling-policy constant) and the orchestration below that decides
*when* to write a new anchor.

# What `@@audit` actually writes — read from the vendored source, not
# assumed

`cratestack-sqlx-0.7.10/src/audit.rs` + `src/audit/schema.rs`, read
directly before designing anything here:

- The table is `cratestack_audit`: `event_id UUID PRIMARY KEY,
  schema_name TEXT, model TEXT, operation TEXT, primary_key JSONB,
  actor JSONB, tenant TEXT, before JSONB, after JSONB, request_id TEXT,
  occurred_at TIMESTAMPTZ NOT NULL, delivered_at TIMESTAMPTZ, attempts
  BIGINT DEFAULT 0, last_error TEXT`.
- **It is insert-only.** `create.rs`/`update.rs`/`delete.rs` in
  `cratestack-sqlx` all route through the same `enqueue_audit_event`,
  which is a bare `INSERT`. Grepping the whole crate for any `UPDATE`
  or `DELETE` against `cratestack_audit` finds none — the `delivered_at`/
  `attempts`/`last_error` columns exist in the DDL (apparently
  provisioned for a future `AuditSink` drain mechanism, mirroring
  `cratestack_event_outbox`'s own shape) but nothing in this framework
  version ever reads or writes them. Vestigial, same shape as
  `crypto-aws-lc-rs`'s empty `install_fips_crypto_provider()` AGENTS.md
  already records finding once before — not acted on here, just noted
  so the next person doesn't assume a drain mechanism exists.
- **No ordering guarantee beyond `occurred_at`.** The primary key is a
  random `UUID` (`uuid::Uuid::new_v4()` in `build_audit_event`), not a
  sequence — there is no column this job could use as a monotonic
  cursor. `occurred_at` (`chrono::Utc::now()`, stamped once per event at
  build time, before the row's own transaction necessarily commits) is
  the only ordering signal, and it is wall-clock, not commit-order. See
  "the race this design accepts" below for what that costs.

# The design decision: a hash chain, not a bare Merkle snapshot, and
# not external anchoring

The issue names no implementation; three real options exist, and the
difference is not cosmetic:

- **A hash chain** (chosen): each anchor folds every audit row in its
  own period into one digest, and includes the *previous anchor's own
  digest* as an input to its own. Editing or deleting any row already
  covered by an anchor changes that anchor's digest on recomputation;
  deleting the anchor row itself breaks the link the *next* anchor
  holds to it. Detects tampering with interior rows and with earlier
  anchors, not just "does the current snapshot look different."
- **A bare periodic Merkle/digest snapshot** (rejected): "the set as of
  time T hashes to X" on its own only proves membership at the moment
  it ran. Without chaining to what came before, deleting an *entire*
  past period's worth of rows — and its own anchor — leaves nothing
  contradicting the remaining anchors; each one is still internally
  self-consistent. A chain turns that from "silently possible" into
  "leaves a dangling `prevChainHash` the next-newer anchor's own
  verification can find," at the cost of some ordering bookkeeping —
  cheap, given this table is written to by a handful of models, not a
  flood.
- **External anchoring** (rejected, and not just for this PR): the
  strongest of the three — publishing a digest somewhere an operator
  with database access alone cannot rewrite (a notary service, a
  separate WORM store, even a signed commit to a repo the deployment
  itself has no write access to). This repo has no such service
  anywhere in `deploy/`, and standing one up is real infrastructure, not
  a job-kind PR — matching #68's own scope, which is "make `@@audit`'s
  rows defensible," not "stand up a compliance notary."

**A keyed (HMAC) chain under a secret pepper was also considered and
rejected, for a reason specific to this codebase.** `backends/crates/sms-api/src/
pepper.rs` already has exactly the machinery — a `HashPepper` loaded
outside the database — and keying the chain under it would raise the
bar from "an attacker with only `psql` access can recompute a valid-
looking chain" to "...cannot, without the pepper too." That is a real
improvement in isolation. It was rejected here because `pepper.rs`'s
own module doc is explicit that a pepper is designed to *rotate*, and
rotation does not retroactively rehash anything — so a keyed chain
would need every historical pepper value kept around indefinitely, just
to keep re-verifying anchors written under it, or every rotation would
make every past anchor look like a verification failure indistinguishable
from real tampering. That is a second, separate secret-management
design problem this ticket does not ask for and should not absorb as a
drive-by. A plain, unkeyed SHA-256 chain has no such coupling — it
proves everything the design doc's own §4.7 scoped ("if you need
tamper-evidence... hashes the day's audit rows... converts 'trust my
database' into 'verify my chain'"), no more.

# Be honest about what this proves, and what it does not

**An anchor stored in the same database an attacker already controls
proves less than "tamper-evident" sounds like it promises.** Concretely,
for an attacker who can write to this Postgres instance with no other
constraint:

- Editing or deleting an audit row *without* touching the anchor chain
  is caught: the next `anchor_audit` run's own re-verification of the
  most recent period (see below), or a manual re-verification against
  an older anchor, recomputes a different `rangeHash` than what is
  stored.
- Editing or deleting an *entire past anchor row* is caught too, as
  long as a *later* anchor still exists to hold the deleted one's
  `chainHash` as its own `prevChainHash` —
  [`sms_api::audit_log::verify_chain_linkage`] checks exactly that,
  every run, over the whole table.
- **Deleting the single most-recent anchor, before the next one is
  written, is not caught by anything in this database.** Nothing yet
  references its `chainHash` as a `prevChainHash`. An attacker who can
  also recompute a fresh, internally-consistent chain from that point
  forward — i.e. who has the same write access this job itself has —
  leaves no contradiction anywhere in the table.

So this genuinely raises the bar from "trust the database" to "trust
the database, or independently verify the chain against a copy taken
before an intrusion" — real, and worth having — but it does **not**
defend against an attacker with sustained, undetected write access to
this same database indefinitely. Closing that gap needs one of the two
rejected options above: ship anchors to somewhere this deployment
cannot itself rewrite (offsite replication of just this table, an
external notary, WORM storage), or accept that this control's job is to
make a *smash-and-grab* edit detectable, not to defend against a
standing, patient adversary. Both are real, named follow-up work, not
silently implied by "tamper-evident" in the issue title.

# The race this design accepts, and why

Each anchor covers `(periodStart, periodEnd]` — exclusive lower bound,
inclusive upper. `periodStart` is `None` (meaning "everything up to
`periodEnd`") only for the very first anchor; every anchor after that
inherits the previous anchor's own `periodEnd` as its `periodStart`, so
the covered ranges are contiguous with no gap and no overlap by
construction — *if* every row that will ever exist with `occurred_at`
in a given window is actually visible by the time that window's anchor
runs.

It might not be. `occurred_at` is stamped inside the mutation's
transaction, before that transaction necessarily commits — a slow
transaction can stamp an `occurred_at` earlier than a fast one that
commits first. If a transaction is still open when this job draws its
`periodEnd` boundary and only commits afterward, its audit row's
`occurred_at` can land inside a window this job has already anchored —
and because windows only ever move forward, that row would never be
covered by *any* future anchor either, since the row it belongs "in"
has already passed.

[`ANCHOR_LAG`] is the accepted mitigation, not a fix: `periodEnd` is
drawn as `now - ANCHOR_LAG`, not `now`, so only a transaction that stays
open longer than the lag can still slip through. Five minutes is a
large multiple of every write path in this codebase — audit-carrying
mutations commit within the same request, typically well under a
second — so the realistic risk is close to zero, not eliminated. A
genuinely stuck transaction that outlives the lag would leave its own
audit row permanently unanchored (still fully present and readable in
`cratestack_audit` — nothing is lost — just never folded into any
anchor's hash), which would look identical, from a verifier's chair, to
a row someone deleted from an anchored period and is hoping goes
unnoticed. This job does not currently detect that specific case (a row
that exists but was never covered, as opposed to a row that was covered
and no longer matches) — a documented, accepted gap, not a silent one.

# What this job actually does, every run

1. Reads the most recent anchor, if any
   ([`sms_api::audit_log::latest_anchor`]).
2. **Re-verifies the whole anchor chain's own internal linkage**
   ([`sms_api::audit_log::verify_chain_linkage`]) — cheap, `O(number of
   anchors)` (at most one row per scheduled run, ever), and does not
   touch `cratestack_audit` at all: every anchor's `prevChainHash` must
   equal the actual previous anchor's `chainHash`, and every anchor's
   own `chainHash` must still equal what recomputing it from that
   anchor's own stored fields produces. A mismatch is logged loudly
   (`error!`), every run, forever, until fixed — matching `reap_outbox`'s
   own "make a broken row loud, never silently swallow it" convention.
3. **Re-verifies the most recent anchor's own row content**
   ([`sms_api::audit_log::verify_period_content`]) — re-reads
   `cratestack_audit` for exactly that anchor's `(periodStart,
   periodEnd]` and recomputes `rangeHash`, comparing against what is
   stored. Bounded to one period's worth of rows, not the whole history
   — see "explicitly out of scope" below for why a full-history
   re-verification is not this job's own hot path.
4. Computes the next period — `periodStart` = the latest anchor's
   `periodEnd` (or `None` on the very first run), `periodEnd` = `now -
   ANCHOR_LAG` — and, if there is anything new to cover, folds every
   covered `cratestack_audit` row into a `rangeHash`, chains it onto the
   previous anchor's `chainHash` (or the fixed genesis sentinel), and
   writes the new `AuditAnchor` row. An anchor is written even when
   `rowCount` is zero — an unbroken chain, including "nothing happened"
   periods, is itself the thing that makes a *gap* in the chain (a
   period silently never anchored) visible as a gap, rather than
   indistinguishable from "quiet day."

# Explicitly out of scope, named rather than silently dropped

- **A full re-verification of every historical audit row, every run.**
  Cost grows with total audit history, not with one period — the
  opposite of this job's own "roughly cheap, runs daily forever" shape.
  An operator who needs that assurance can run the same
  [`sms_api::audit_log::verify_period_content`]/
  [`sms_api::audit_log::verify_chain_linkage`] functions this job and
  #58's own `auditChainStatus` procedure already call, walking every
  anchor, as a one-off — the functions exist and are tested; wiring a
  CLI subcommand around them is real, separate follow-up work, not
  built here.
- **External anchoring** — see "be honest about what this proves" above.
- **The row that exists but was never covered by any window** — see
  "the race this design accepts" above.

# R1 exception

The raw `cratestack_audit` reads themselves moved to
`backends/crates/sms-api/src/audit_log.rs` (#58) — see that module's own doc for
the up-to-date exception reasoning and `cargo xtask no-raw-sqlx`/
`CONTRIBUTING.md`'s own exceptions table, which both name that file now.
This file itself contains no raw SQL query call any more.
`backends/crates/sms-worker/tests/anchor_audit_live_postgres.rs` keeps its own,
separate, test-only exception: proving tamper-evidence means the test
itself has to tamper with a raw `cratestack_audit`/`audit_anchors` row
the same way a real attacker would, and there is no delegate for either
to do that through even on the defender's side, let alone the
attacker's.

`AuditAnchor` itself is a real, new schema model (§68's own DDL change)
with a real delegate — reading/writing *it* never needs raw SQL, only
the underlying `cratestack_audit` rows being anchored do.
