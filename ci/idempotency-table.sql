-- #153: the table cratestack's own `SqlxIdempotencyStore`/`IdempotencyLayer`
-- read and write — library bookkeeping, not part of `schema/schema.cstack`
-- or the committed `schema/migrations/` tree, and never touched by
-- `cratestack migrate diff`. Copied verbatim (same whitespace) from
-- `cratestack-sql`'s `IDEMPOTENCY_TABLE_DDL` constant (pinned cratestack
-- =0.7.8) rather than invoked at runtime, because neither of this file's
-- two callers — `ci/apply-migrations.sh` (the CI/scratch-database path)
-- and `deploy/migrate.sql` (the real deploy path, via `\i` in
-- `deploy/migrate.Dockerfile`) — has a Rust runtime to call
-- `SqlxIdempotencyStore::ensure_schema()` from. Both statements are
-- `IF NOT EXISTS`, so this file is safe to apply unconditionally and
-- repeatedly, matching `ci/apply-migrations.sh`'s own "dumber,
-- unconditional loop" style (it starts from an empty database every time
-- and has no "was this already applied" question to answer) — the deploy
-- path wraps this same file in its own `schema_migrations`-tracked
-- `\if`/`\else` for the different reason documented in `migrate.sql`
-- itself (skip re-running DDL against an already-migrated production
-- database on every redeploy).
--
-- `crates/sms-api/tests/idempotency_table_ddl_matches_ci_sql.rs`
-- guards this file from drifting apart from the pinned library's own
-- constant on a future cratestack version bump.
CREATE TABLE IF NOT EXISTS cratestack_idempotency (
    principal_fingerprint TEXT NOT NULL,
    key TEXT NOT NULL,
    request_hash BYTEA NOT NULL,
    reservation_id UUID NOT NULL,
    response_status INT,
    response_headers BYTEA,
    response_body BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (principal_fingerprint, key)
);

CREATE INDEX IF NOT EXISTS cratestack_idempotency_expires_idx
    ON cratestack_idempotency (expires_at);
