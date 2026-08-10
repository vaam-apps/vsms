-- #153, relocated here from `ci/idempotency-table.sql` while landing the
-- rustls/musl/distroless PR: the table cratestack's own
-- `SqlxIdempotencyStore`/`IdempotencyLayer` read and write — library
-- bookkeeping, not part of `schema/schema.cstack` and never touched by
-- `cratestack migrate diff`, but shaped exactly like the other two entries
-- in this directory (`0001_init`, generated DDL; `0002_bootstrap`,
-- hand-authored from the design doc) for the same reason: something that
-- must be applied once, in order, tracked. Copied verbatim (same
-- whitespace) from `cratestack-sql`'s `IDEMPOTENCY_TABLE_DDL` constant
-- (pinned cratestack =0.7.8) rather than invoked at runtime, because none
-- of this file's callers — `ci/apply-migrations.sh` (the CI/scratch-
-- database path), `crates/sms-test-support` (the live-suite harness), and
-- `app/sms-migrate` (the real deploy path) — has a Rust runtime available
-- at the point it applies migrations to call
-- `SqlxIdempotencyStore::ensure_schema()` from. Both statements are
-- `IF NOT EXISTS`, so this file is safe to apply unconditionally and
-- repeatedly — every one of those three callers now discovers it exactly
-- like `0001_init`/`0002_bootstrap`, a directory walk over
-- `schema/migrations/postgres/*/up.sql`, with no special-cased extra step
-- for this file any more.
--
-- `crates/sms-api/tests/idempotency_table_ddl_matches_cratestack.rs`
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
