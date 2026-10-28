-- 0011_webhook_endpoint_due_idx / up.sql
--
-- VS-8 follow-up (M1): the due list of ONE endpoint. When any endpoint is one
-- a worker cannot claim (a `sealed` one for a worker without the envelope
-- signing key, an inactive one, one whose circuit is open), the claim reads the
-- claimable endpoints first and then asks for each one's due attempts, oldest
-- first (`backends/crates/sms-worker/src/claim.md`). The only
-- index that could serve that is `webhook_due_idx (next_attempt_at)`, which is
-- ordered by time across every endpoint: reading it for one endpoint walks the
-- whole due backlog, whatever the other endpoints' modes are, because the
-- endpoint is a filter there and not a key. Measured on PostgreSQL 16 with
-- 200 000 due sealed attempts and five `hmac` ones, the old claim read 13 883
-- buffers; this index brings it to a handful, whatever the sealed backlog.
--
-- It is partial on the same states `webhook_due_idx` covers (`pending`, and
-- `failed` resting out a backoff), so a `delivering`, `succeeded` or `dead` row
-- is not in it and a delivery that finishes writes one index entry less than a
-- full index would. `webhook_due_idx` stays: when every endpoint is claimable,
-- either kind of worker claims in one global oldest-first list and reads it.
--
-- There is no generator output to copy: `cratestack migrate diff` emits no
-- partial index (`@@index` has no predicate), which is also why `webhook_due_idx`
-- and `webhook_attempts_lease_reclaim_idx` are hand-written in `0002_bootstrap`.
-- `cargo xtask migrations-current` compares `0001_init` only and needs no entry
-- for this file. The index is proved by
-- `webhook_endpoint_due_idx_live_postgres.rs` (the migration) and by
-- `hooks_live_postgres.rs` (a keyless claim never reads the sealed backlog's
-- entries). Without it every claim still answers, but reads the backlog once per
-- endpoint it asks for, on every poll.
--
-- Plain `CREATE INDEX`, not `CONCURRENTLY`: `sms-migrate` applies a migration in
-- a transaction, where `CONCURRENTLY` is refused. A plain build takes a SHARE
-- lock on `webhook_attempts`, which blocks every write to it for the whole build,
-- and on an upgrade those writers are running: the migrate Job is a
-- `pre-install,pre-upgrade` Helm hook (`deploy/charts/vsms/values.yaml`), so the
-- previous release's gateway and workers are still up, enqueueing and delivering
-- attempts, and they stall until the build ends. Nothing purges the table yet:
-- `WebhookAttempt` declares `@@retain(days: 30)`, but `purge_retention` deletes
-- messages and delivery receipts only, so the build takes as long as the table's
-- whole history. For a table of any size, build the index by hand with
-- `CREATE INDEX CONCURRENTLY IF NOT EXISTS` before the deploy;
-- `docs/runbooks/deployment.adoc` ("Before deploying: the `webhook_attempts`
-- index of migration 0011") has the size query and the statement; the
-- alternative is a maintenance window.
--
-- The block below is what makes that safe, and `CREATE INDEX IF NOT EXISTS` is
-- not, for two reasons. It takes the SHARE lock *before* it looks whether the
-- index exists, so over an index built by hand it still waits for every open
-- write to the table and then blocks the next ones (seen: a lock timeout with an
-- uncommitted UPDATE held, over an index that was there). And it steps over an
-- *invalid* index of the same name, the leftover of a `CONCURRENTLY` build that
-- was interrupted or failed, reports "already exists, skipping" and lets the
-- migration succeed with an index no query can use (seen: `indisvalid` stayed
-- false). So the index is looked up first, by name, with no lock on the table:
--
--   - absent: built, plainly, as described above;
--   - present, valid and the definition below: nothing runs, no `CREATE INDEX`,
--     so no lock on `webhook_attempts` is taken at all;
--   - present and invalid, or valid with another definition: the migration
--     fails, naming the runbook, and `sms-migrate` rolls it back. A migration
--     that succeeds over an index the claim cannot use, or that does not serve
--     it, would leave the slow claim in place and say nothing.
--
-- `to_regclass` finds the name on the migration's `search_path`, the one the
-- tables above were created on.

DO $migration$
DECLARE
    existing regclass := to_regclass('webhook_endpoint_due_idx');
    is_valid boolean;
    is_unique boolean;
    definition text;
BEGIN
    IF existing IS NULL THEN
        CREATE INDEX webhook_endpoint_due_idx ON webhook_attempts (endpoint_id, next_attempt_at)
            WHERE state IN ('pending','failed');
        RETURN;
    END IF;

    SELECT i.indisvalid, i.indisunique, pg_get_indexdef(i.indexrelid)
      INTO is_valid, is_unique, definition
      FROM pg_index i
     WHERE i.indexrelid = existing
       AND i.indrelid = 'webhook_attempts'::regclass;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'webhook_endpoint_due_idx exists but is not an index of webhook_attempts; drop or rename it, see docs/runbooks/deployment.adoc, "Before deploying: the webhook_attempts index of migration 0011"';
    END IF;
    IF NOT is_valid THEN
        RAISE EXCEPTION 'webhook_endpoint_due_idx exists but is INVALID (an interrupted CREATE INDEX CONCURRENTLY); run DROP INDEX CONCURRENTLY webhook_endpoint_due_idx and build it again, see docs/runbooks/deployment.adoc, "Before deploying: the webhook_attempts index of migration 0011"';
    END IF;
    IF is_unique OR definition NOT LIKE '%USING btree (endpoint_id, next_attempt_at) WHERE (state = ANY (ARRAY[''pending''::text, ''failed''::text]))' THEN
        RAISE EXCEPTION 'webhook_endpoint_due_idx exists with another definition (%); drop it and let this migration build it, see docs/runbooks/deployment.adoc, "Before deploying: the webhook_attempts index of migration 0011"', definition;
    END IF;
END
$migration$;
