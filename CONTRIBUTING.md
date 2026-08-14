# Contributing to vsms

Five rules constrain everything else. Each is a default with named exceptions, and the exceptions are the interesting part. The full reasoning is in [docs/architecture.md](docs/architecture.md); this file is the version you should have in your head during review.

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
| Live-Postgres test harness | `crates/sms-test-support/src/lib.rs` | The shared harness (`AGENTS.md`'s "The live suites run in CI now" section) takes its own advisory lock (a distinct namespace from `lease.rs`'s own `NS`) to serialise the ensure-template-migrated / drop-and-recreate-this-binary's-database sequence, and issues `CREATE DATABASE ... TEMPLATE` / `DROP DATABASE ... WITH (FORCE)` to give each test binary its own scratch database. Both are the same two categories the rows immediately above and below already cover — advisory locks, and DDL/database-catalog operations — just never previously attached to this file, a gap found while porting `ci/assert-no-raw-sqlx.sh` to `cargo xtask no-raw-sqlx`: the script's own allowlist had carried this path since the harness landed, `CONTRIBUTING.md` never had a matching row. Not a schema model, so nothing for row-level policy/audit/`@@emit`/`@version` to apply to — this manages the database a test *runs against*, not a table inside it. |
| `LISTEN` / `NOTIFY` | `crates/sms-worker/src/notify.rs`, `crates/sms-api/src/cache.rs` | Cache-invalidation fan-out. No delegate expression exists. |
| Readiness probe | `app/sms-gateway/src/health.rs` | `GET /readyz` (#157) round-trips a bare `SELECT 1` on the pool to prove the database is reachable. There is no model this check is *about* — it reads no application row — so there is nothing for row-level policy, audit, outbox, or `@version` to apply to. A delegate call would also fail #157's other named trap: any real table read on an unauthenticated route is a query-amplification DoS surface, which a policy-free connectivity ping is not. |
| Outbox age telemetry | `crates/sms-worker/src/drain.rs` | The `drain` role (#39) alerts on oldest-undelivered age, not just on errors — a stalled outbox with zero errors is exactly as silent as one full of retries. `cratestack_event_outbox` is the framework's own internal bookkeeping table (created lazily by `ensure_event_outbox_table`), not one of `schema.cstack`'s models, so no delegate exists to read it. Same reasoning as the readiness probe: no row-level policy to bypass (the table isn't ours), no audit trail to skip (a `SELECT` isn't a mutation), no `@version`/soft-delete concern (it isn't a model). |
| Outbox reap and poison-row alert | `crates/sms-worker/src/jobs/reap_outbox.rs` | The `reap_outbox` job (#42) deletes delivered `cratestack_event_outbox` rows past a 24h retention and alerts on still-undelivered rows past `attempts > 5` (§7.5). Same table as the row above, same reasoning: no model, no delegate, nothing for row-level policy/audit/`@version` to apply to. Poison rows are never deleted, only logged — see that file's own module doc for why "reap" means "delete delivered rows," not "delete poison rows." |
| Worker lock visibility | `crates/sms-api/src/worker_locks.rs` | The Workers screen (#57) answers "which node holds which singleton lock" from `pg_locks` joined against `pg_stat_activity` — Postgres's own lock catalog, not one of `schema.cstack`'s models, so no delegate exists to read it. No row-level policy applies to a system catalog, a `SELECT` writes no audit row regardless, and neither `@version` nor soft-delete concerns a catalog view. See that file's own module doc for what was verified live (not assumed) about what `pg_locks` actually reports for a session advisory lock, and why two workers holding the same role's lock can never show up as two granted rows there. |
| Reserved-role-key test fixture | `app/sms-gateway/tests/login_flow_live_postgres.rs` | #194's own guard-failure proof needs a `Role` row keyed `"system"` — a row `roles_key_not_reserved_check` (§2.10) makes unreachable through any real path, on purpose. `seed_reserved_role_login_account` briefly `ALTER TABLE ... DROP CONSTRAINT`s it, seeds the row through the real `db.role().create()` delegate, then restores the constraint before returning — the constraint drop is the only raw SQL; every data write still goes through a real delegate. Not production data access, and not a fourth-guarantee bypass: nothing about a test-only, immediately-restored schema constraint has an audit trail, row-level policy, or `@version` concern to begin with. |
| Audit-row hashing and reading | `crates/sms-api/src/audit_log.rs` | Folds every `cratestack_audit` row in a period into a hash chain (`anchor_audit`, #68) so tampering with the audit log is detectable, and (#58) reads a filtered/paged window of the same table for the console's own audit-log screen. Same reasoning as the two outbox rows above: `cratestack_audit` is bootstrapped lazily by `ensure_audit_table` (`cratestack-sqlx`'s own internal table, not one of `schema.cstack`'s models), so no delegate exists to read it — no row-level policy to bypass, no audit trail to skip (a `SELECT` isn't a mutation, and auditing the audit table is circular besides), no `@version`/soft-delete concern. The anchors themselves (`AuditAnchor`) are a real schema model with a real delegate, read and written the normal way — only the raw audit rows being anchored or browsed need this exception. Moved here from `crates/sms-worker/src/jobs/anchor_audit.rs` (#58): the console's own audit-chain-status procedure needed the identical hashing/verification logic `anchor_audit` already had, and `sms-api` cannot depend on `sms-worker` (the dependency runs the other way), so the read/hash logic moved down to where both callers can reach it rather than being duplicated. `anchor_audit.rs` itself now imports it and contains no raw SQL of its own. |
| Audit-row tamper test | `crates/sms-worker/tests/anchor_audit_live_postgres.rs` | The house standard of proving a guard can actually fail (`AGENTS.md`) means this suite has to simulate the exact attack `anchor_audit` defends against: an actor with direct Postgres access editing a `cratestack_audit` row after it was anchored. There is no delegate for that table at all — not even a read one outside the exception above — so a test proving tamper-evidence has no non-raw way to tamper. This is a test-only exception: it exercises the same table the row above already names, for the same underlying reason (no delegate exists), just from the attacker's side of the same table rather than the defender's. |

That is the complete list, and `cargo xtask no-raw-sqlx` enforces it (the deleted `ci/assert-no-raw-sqlx.sh`'s Rust successor — see `AGENTS.md`'s xtask section for why every CI check in this repo now lives there instead of in a shell/Python script). Adding a twelfth entry should feel like a design decision, because it is one — put the reasoning in the PR.

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

## R4 — The admin console is optional. The backend must run without it.

**Some deployments will ship the backends only, with no admin surface at all.** That is a supported configuration, not a degraded one, and it is a hard rule rather than a preference: a client integrating vsms as a delivery backend behind their own product has no use for this console, and may have a policy reason not to expose one.

Concretely, for any change:

- **No server-side code may depend on the console existing.** `crates/` and `app/` reference `admin/` only in comments today, and that is the invariant — a Rust file that needs a value only the console produces is a violation. `sms-gateway serve` must start, serve, and pass its health checks with no console deployed anywhere.
- **Every operator action must be reachable without a browser.** This is what makes the rule survivable rather than aspirational. `provision-client`, `provision-user`, `seed-console-client`, `seed-dispatch`, `rotate-signing-key` and `record-route-validation` exist as `sms-gateway` subcommands for exactly this reason. If you add a console screen that performs an action no CLI subcommand can perform, you have made the console load-bearing — add the subcommand in the same change.
- **Deployment must be able to omit it.** The Helm chart and the compose stack must both bring up a working gateway + worker + migrate with the console switched off, and nothing console-specific may be a hard-`required` value in that configuration. `sms-console`'s `OauthClient`, `ADMIN_BASE_URL`, `SMS_CONSOLE_*` and the OIDC session secret are all console-only concerns; a backend-only install must not need any of them.
- **A backend-only deployment must still be observable and operable.** Metrics, alerting, the DLR endpoint, webhooks and the audit trail are backend concerns and must not degrade. The console is a *view* onto this system, never a component of it.

The test to apply when reviewing: *if the console were deleted from this repository entirely, would this change still work?* If the answer is no, the change is wrong, not the rule.

**Closed 2026-08-14 (#233):** the deployment layer now satisfies this rule too. `deploy/charts/vsms/values.yaml` gates the whole `admin` controller (and its Service) behind `admin.enabled` (default `true`, so an existing install is unaffected) — `helm template --set admin.enabled=false` renders with no console-only value set, and `admin.enabled=true` still hard-`required`s them; see `deploy/charts/vsms/templates/common.yaml`'s own comment for the mechanism (values.yaml itself is never templated as a whole, so the toggle has to mutate the merged values before `bjw-s.common.loader.generate` runs). `deploy/docker-compose.yml`'s `admin` and `caddy` both carry `profiles: [console]`, opt-in via `--profile console`; a bare `docker compose up` brings up postgres, migrate, sms-gateway, sms-worker, backup and prometheus only. See `docs/runbooks/deployment.md`'s own "Backend-only deployment" section for a real, driven-end-to-end proof, not just a rendered template.

---

## R5 — Helm charts are built on `bjw-s` common ≥ v4, as one umbrella chart.

Kubernetes packaging goes through the [`bjw-s` common library chart](https://bjw-s-labs.github.io/helm-charts), version **4 or newer**, and vsms ships as **one umbrella chart** containing several controllers — not a chart per service.

Both halves are load-bearing:

- **`bjw-s` common ≥ v4, not hand-written manifests.** A `Deployment`, `Service`, `Ingress`, `ServiceAccount` and probe set written by hand is a few hundred lines of YAML per service that nobody reviews carefully and every service copies from the last one. The library chart makes a controller a values entry. Version 4 is the floor because its `controllers`/`route` schema is what this chart is written against; v3 and earlier use an incompatible shape, so "upgrade the dependency" is not a mechanical bump.
- **One umbrella chart, not one chart per service.** `sms-gateway`, `sms-worker`, `admin` and the migrate/seed jobs are a single deployable unit with one shared database, one migration ordering, and one set of secrets. Splitting them into separate charts means a released version number that cannot express "these four things go together", and an operator installing four releases that must agree on `DATABASE_URL`, the hash pepper and the OP issuer. One release, several controllers — the same shape the compose stack already has.

Do not add a second chart. Do not hand-roll a manifest that the library chart can express. If the library cannot express something, say so in the PR with the specific limitation, rather than quietly forking into raw YAML.

`deploy/charts/vsms/Chart.yaml` currently pins `common` at `4.6.2`. Note it is a **classic HTTP repository dependency, not an OCI one** — no OCI reference for the library chart exists, verified against the GHCR API and bjw-s-labs' own release workflow rather than assumed. That is recorded in the chart's own comments; don't "modernise" it to `oci://` without checking that again.

R4 applies here too: whatever this chart grows, installing it with the console switched off must remain possible.

---

## Changing the schema

`schema/schema.cstack` is the source of truth for the model layer. `0002_bootstrap` is generated from §2.10 of the design doc, so the document and the SQL cannot drift.

```bash
# 1. edit schema/schema.cstack

# 2. regenerate the framework migration
cratestack migrate diff --schema schema/schema.cstack \
  --out-dir schema/migrations/postgres --backend postgres --name <change_name>

# 3. if you touched §2.10 of the design doc, regenerate the bootstrap SQL
cargo xtask bootstrap-sql schema/migrations/postgres/0002_bootstrap/up.sql

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
