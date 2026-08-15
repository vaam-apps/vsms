Reading `cratestack_audit` — the audit hash-chain machinery #68 built to
write it, and (#58) the read-only console view over it.

# Moved here from `backends/crates/sms-worker/src/jobs/anchor_audit.rs`, not
# duplicated

Everything below `# The chain itself` used to live entirely inside
`sms-worker`'s `anchor_audit` job, which writes a new `AuditAnchor` every
run. #58 needed the *same* hashing and verification logic for a second,
independent reason: the console's own audit-log screen wants to say "this
period's chain verifies," not just print a table of anchor rows, and
computing that from scratch a second time — even a careful, tested
second time — is exactly the kind of algorithm duplication AGENTS.md's
own `#134` section warns against (a hand-rolled second copy of a hash
computation that silently drifts the moment the real one changes). Two
honest options existed:

1. Duplicate the hashing functions into `sms-api`, accepting the drift
   risk the way `sms-provider-mtn`'s `classify_transport_error` accepts
   it against `sms-provider-orange-cm`'s copy (a genuinely provider-
   agnostic algorithm, "two instances isn't yet a rule"). Rejected here:
   a hash chain that silently diverges between the function that *wrote*
   an anchor and the function that *verifies* it would fail in exactly
   the misleading way this feature exists to prevent — a real edit could
   go undetected because the verifier's own recomputation was already
   wrong for an unrelated reason, or worse, a false "chain broken" alarm
   could fire on unmodified data purely from the two copies disagreeing.
2. Move the pure hashing/verification logic to the one crate already
   sitting below both callers. `sms-api` cannot depend on `sms-worker`
   (the dependency runs the other way — `sms-worker.workspace = true` is
   a *dev*-only dependency of `sms-api`, confirmed by that crate's own
   `Cargo.toml` comment), so the move has to go this direction: out of
   `sms-worker`, into `sms-api`, with `sms-worker`'s own `anchor_audit.rs`
   importing it back. **Taken.**

`sms-worker`'s `anchor_audit.rs` keeps only what is genuinely its own:
`ANCHOR_LAG` (a scheduling-policy constant — how far behind `now` the job
deliberately stays, see that module's own "the race this design accepts"
section) and the `AnchorAudit`/`JobHandler` orchestration that decides
*when* to write a new anchor. Everything about what a hash chain over
`cratestack_audit` *means*, and how to verify one, lives here, once.

# The chain itself

See `backends/crates/sms-worker/src/jobs/anchor_audit.rs`'s own module doc (now
the *design* doc for this mechanism, even though the code moved) for the
full reasoning: why a hash chain rather than a bare snapshot or external
anchoring, exactly what it proves and does not (in particular: it cannot
catch deletion of the single newest anchor before anything references
it — an attacker with sustained database write access can rebuild a
self-consistent chain from that point forward), and the race the
`ANCHOR_LAG` safety margin accepts.

# R1 exception, moved from the seventh to here

`cratestack_audit` is the framework's own internal bookkeeping table
(created lazily by `ensure_audit_table`), not one of `schema.cstack`'s
models — no delegate exists to read it, so there is no row-level policy
to bypass, no audit trail to skip (a `SELECT` isn't a mutation, and this
*is* the audit table besides), no `@version`/soft-delete concern.
`cargo xtask no-raw-sqlx` and `CONTRIBUTING.md`'s own exceptions table
both name this file now, in place of `anchor_audit.rs`.

# #58: the console's own read surface

[`list_audit_entries`] and [`chain_status`] back the `auditLog`/
`auditChainStatus` procedures (`backends/crates/sms-api/src/procedures.rs`) — a
filtered, paged view over the raw rows, and a snapshot of whether the
anchor chain currently re-verifies, respectively. Both are read-only:
neither this module nor either procedure ever writes a `cratestack_audit`
row or an `AuditAnchor` row (only `anchor_audit`'s own scheduled job
does, via a real delegate `create()` call — see that module). There is
no mechanism anywhere in this codebase for a console screen, or any
human role including `owner`, to edit or delete an audit row or an
anchor: `AuditAnchor`'s own `schema.cstack` model declares no
`@@allow("update", ...)` / `@@allow("delete", ...)` clause at all.

**What that actually guarantees, checked live rather than assumed —**
`cratestack-macros` still generates an `UpdateAuditAnchorInput` type and
a `.update(id).set(input).run(ctx)` method on the delegate (confirmed:
`db.audit_anchor().update(id).set(UpdateAuditAnchorInput { rowCount:
Some(999), ..Default::default() }).run(ctx)` type-checks and compiles
fine) — the first attempt at this doc comment claimed the opposite (a
compile-time absence), which turned out to be wrong the moment it was
actually tried, the same "verify against live execution" trap AGENTS.md
is full of examples of. **The real guard is deny-by-default at
runtime, for every caller including `system`.** Run for real against a
live, migrated Postgres, with a `system`-role `CoolContext` — the most
privileged context this codebase ever constructs — the call above
returns `Err(CoolError::Forbidden("update policy denied this
operation"))`, not an error naming a missing row (there was one — the
id was fictional — so a row-not-found error would have meant the
*policy* check never ran at all, which would be the actual hole). No
role, human or synthetic, can write this row through any path this
codebase exposes. `audit_chain_status_and_audit_log_live_postgres.rs`'s
own `no_role_including_system_can_write_an_audit_anchor` pins this down
as a permanent regression assertion rather than a one-off finding.
