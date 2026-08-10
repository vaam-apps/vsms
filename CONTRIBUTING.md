# Contributing to vsms

Three rules constrain everything else. Each is a default with named exceptions, and the exceptions are the interesting part. The full reasoning is in [docs/architecture.md](docs/architecture.md); this file is the version you should have in your head during review.

---

## R1 — All data access goes through CrateStack delegates. Never raw `sqlx`.

The generated `Cratestack` runtime is the only way application code touches the database.

```rust
db.message().find_many().where_expr(...).order_by(...).limit(n).run(&ctx).await?
db.message().update(id).set(input).if_match(version).run(&ctx).await?
db.job().create(input).run_in_tx(&mut tx, &ctx).await?
```

No `sqlx::query!`, no `query_as`, no `query_scalar`, no `raw_sql`.

This is not stylistic. Going through the delegates is what keeps four guarantees switched on, and raw SQL bypasses all four silently:

- **Row-level policy.** `@@allow` compiles into SQL predicates appended to your `WHERE`. A raw query is unscoped — it sees every app's messages and every soft-deleted tombstone.
- **Audit.** `@@audit` rows are written inside the mutation's transaction. A raw `UPDATE` produces no audit entry, so the trail develops holes exactly where someone worked around the framework.
- **Events.** `@@emit` writes the outbox row in the same transaction. A raw `UPDATE` moving a message to `delivered` fires no `message.delivered` webhook — the customer is never told.
- **Optimistic locking and soft delete.** `@version` bumping and `deleted_at IS NULL` scoping happen in the delegate layer, not in the database.

A raw query against `messages` is therefore not the same query written by hand. It is a different query with four safety properties removed.

### The named exceptions

| Exception | Where | Why the delegates can't do it |
|---|---|---|
| DDL and migrations | `schema/migrations/**`, `app/sms-migrate/src/main.rs` | Triggers, partial indexes, foreign keys, column defaults. Not data access; the emitter produces none of it. `sms-migrate` (replacing the old `psql`-based `deploy/migrate.Dockerfile`) is the runner that applies this DDL via `cratestack::sqlx::raw_sql` — same exception, same reasoning, just executed from Rust instead of a shell script now. |
| Advisory locks | `crates/sms-worker/src/lease.rs` | `pg_try_advisory_lock` / `pg_advisory_unlock`. Not a table. |
| `LISTEN` / `NOTIFY` | `crates/sms-worker/src/notify.rs`, `crates/sms-api/src/cache.rs` | Cache-invalidation fan-out. No delegate expression exists. |
| Readiness probe | `app/sms-gateway/src/health.rs` | `GET /readyz` (#157) round-trips a bare `SELECT 1` on the pool to prove the database is reachable. There is no model this check is *about* — it reads no application row — so there is nothing for row-level policy, audit, outbox, or `@version` to apply to. A delegate call would also fail #157's other named trap: any real table read on an unauthenticated route is a query-amplification DoS surface, which a policy-free connectivity ping is not. |

That is the complete list, and `ci/assert-no-raw-sqlx.sh` enforces it. Adding a fifth entry should feel like a design decision, because it is one — put the reasoning in the PR.

Two things people reach for that are **not** exceptions:

- **Transactions.** Every delegate builder has `.run_in_tx(&mut tx, &ctx)`. Delegates inside a caller-managed transaction still write their audit and outbox rows into that transaction. `cratestack::run_in_isolated_tx(pool, isolation, closure)` handles `SERIALIZABLE` with 40001 retries.
- **Row locking.** `.for_update()` exists on `find_many` and `find_unique` and appends a real `FOR UPDATE`.

The one genuine gap is **`SKIP LOCKED`**, which the framework cannot express — `skip_locked()`, `nowait()` and `lock_mode()` are all compile errors. The claim loops use optimistic compare-and-swap on `@version` instead, treating `CoolError::PreconditionFailed` as "another worker won". That is better here regardless: no lock is held across the provider HTTP call.

One trap in that pattern. `CoolError::Forbidden` is **ambiguous** — the framework returns it both when the update policy denies and when the row is gone, because both produce zero rows. Do not fold it into the "lost the race" branch; log it. Swallowing it hides a policy regression as unexplained throughput loss.

---

## R2 — State transitions are proposed by Rust and decided by Postgres.

Legal edges live in `message_state_transitions` and `job_state_transitions`. `BEFORE UPDATE` triggers reject everything else with SQLSTATE `SM001`. Application code never assumes a transition is valid because it checked first.

Three reasons this lives in the database, in ascending order of how much they will matter to you:

1. **Every writer is covered** — a migration script, a psql session at 2am, an admin route someone forgot to lock down, a service written by someone who never read this file. Rules that live only in application code are enforced only for callers who use that code.
2. **It closes races `@version` leaves open.** Optimistic locking tells you the row changed; it does not tell you the *new* state is one you may move from. A cancel and a submit racing on the same message can both pass their version check in different transactions. Only one passes the trigger.
3. **The machine is legible.** `SELECT * FROM message_state_transitions` is the authoritative answer to "can a message go from X to Y", and adding an edge is a reviewed migration rather than a code change buried in a match arm.

Terminal states are simply rows with no outgoing edges, so terminality is data rather than a branch someone can forget.

**When you touch a state machine:** update the transition table in a migration, update the Mermaid diagram in the design doc, and make sure `state_machine_parity` still passes. Map `sqlstate = 'SM001'` to `CoolError::Conflict` so it surfaces as `409`, not `500`.

**Alert on any non-zero `SM001` rate in production.** In a correct system it is flat zero — the trigger is a backstop, not a control path. A non-zero rate means the code and the transition table disagree, and it will tell you before a customer does.

---

## R3 — Nothing that must be written can be `@server_only`.

`@server_only` excludes a field from **both** create and update inputs, so under R1 such a field can never be populated at all. It is for columns the database owns, not for secrets you write.

Field secrecy comes from model-level `@@allow`, or from keeping the secret out of this database entirely. `Provider.credentialRef` is a *pointer* (`vault://...`), not a credential, which is why it is safe as a plain column. When the framework won't let you hide a field, arrange for the field not to be worth hiding.

Related, and easy to get wrong: **`@pii` and `@sensitive` redact audit snapshots only.** They add no serde attribute, so a `@sensitive` field is still returned by `GET /messages/{id}`. They are not confidentiality controls.

---

## Changing the schema

`schema/schema.cstack` is the source of truth for the model layer. `0002_bootstrap` is generated from §2.10 of the design doc, so the document and the SQL cannot drift.

```bash
# 1. edit schema/schema.cstack

# 2. regenerate the framework migration
cratestack migrate diff --schema schema/schema.cstack \
  --out-dir schema/migrations/postgres --backend postgres --name <change_name>

# 3. if you touched §2.10 of the design doc, regenerate the bootstrap SQL
python3 ci/gen-bootstrap-sql.py schema/migrations/postgres/0002_bootstrap/up.sql

# 4. prove it applies to an empty database, and that the machines still hold
createdb vsms_check
DATABASE_URL=postgres://localhost/vsms_check ./ci/apply-migrations.sh
psql postgres://localhost/vsms_check -v ON_ERROR_STOP=1 -f ci/test-state-machine.sql
dropdb vsms_check

# 5. once crates exist, prove it still EXPANDS — not just parses
cargo check -p sms-api
```

Step 5 is not redundant with step 4, and the distinction has bitten this project already. `cratestack-parser` and `cratestack-migrate` will both happily accept a schema that `include_server_schema!` refuses to compile. A schema can be valid to two of the three tools and still not build.

## Schema constraints that are not obvious

Every one of these was found by running the toolchain, not by reading its documentation. [§2.0](docs/architecture.md#20-grammar-and-emitter-constraints) has the full table with the exact error each produces.

- **Exactly one whitespace character** between a field name and its type. No column alignment — `id  Cuid` fails with `invalid type reference:`.
- **The parser is line-based.** An `@@allow` cannot wrap onto a second line.
- **`@use(Mixin)` is single-`@`.** `@@use` parses, is silently retained as an unknown attribute, and does not expand the mixin. There is no error.
- **A misspelled `@@allow` action is silently dropped**, and deny-by-default then makes that operation unreachable.
- **No model may declare a list field.** `String[]` and `Int[]` panic `include_server_schema!`. Multi-values are space-delimited strings with sentinel separators — `" a b "` not `"a b"`, so `.contains(" a ")` cannot false-match `ab`. `sms-core` owns `pack()` / `unpack()`.
- **Any `@default(...)` excludes the field from the create input**, literals included.
- **Ids must be lowercase alphanumeric, no separator.** `Cuid` is format-guarded as `[a-z0-9]{2,32}` on REST query filters, so a `msg_` prefix makes `GET /messages?id=…` return 400. `cs_cuid()` emits 23 chars.
- **No `@@index`, no foreign keys, no triggers** come out of the emitter. All hand-written in `0002_bootstrap`.
- **`pluralize()` is naive** (`ends_with('s') ? +"es" : +"s"`) and there is no `@@map`. Model names here were chosen to pluralise cleanly — `WebhookDelivery` would have become `webhook_deliverys`, which is why the model is `WebhookAttempt`.

## Pull requests

- CI must be green: migrations apply to an empty database, the state-machine test passes, the R1 lint passes.
- A schema change includes its migration and a note in the design doc if it changes a decision.
- New raw-SQL exceptions need the reasoning in the PR description, not just the allowlist edit.
- If you hit a new framework constraint, add it to §2.0 with the exact error text. That table is the most valuable thing in the repository for whoever comes next.
