//! #153: `ci/idempotency-table.sql` hand-copies `cratestack`'s own
//! `IDEMPOTENCY_TABLE_DDL` constant — `crates/sms-api/src/router.rs`'s
//! `router()` deliberately never calls `SqlxIdempotencyStore::ensure_schema()`
//! itself (see that function's own doc for why the deploy migrate job and
//! `ci/apply-migrations.sh` own this table instead, both by applying that
//! one file). A hand copy drifts silently the moment the pinned
//! `cratestack` family changes that constant's shape — this test is the
//! guard: it asserts the exact text in `ci/idempotency-table.sql` is still
//! present, byte for byte, in the library's own constant.
//!
//! No live database needed — this only compares two strings.

/// `ci/idempotency-table.sql` is two directories up from this crate
/// (`crates/sms-api/`), same relative path shape `include_server_schema!`
/// already uses in `src/lib.rs` for `../../schema/schema.cstack`.
const IDEMPOTENCY_TABLE_SQL: &str = include_str!("../../../ci/idempotency-table.sql");

#[test]
fn ci_idempotency_table_sql_carries_cratestacks_own_ddl_verbatim() {
    let upstream = cratestack::idempotency::IDEMPOTENCY_TABLE_DDL.trim();
    assert!(
        IDEMPOTENCY_TABLE_SQL.contains(upstream),
        "ci/idempotency-table.sql no longer matches \
         cratestack::idempotency::IDEMPOTENCY_TABLE_DDL — the pinned cratestack version \
         changed the table shape. Re-copy the constant's exact text into \
         ci/idempotency-table.sql, and if a real deployment may already have the table in \
         the old shape, give deploy/migrate.sql's tracking a *new* schema_migrations name \
         rather than reusing 'cratestack_idempotency_table' — a table already created under \
         the old shape needs a real ALTER, not a second CREATE TABLE IF NOT EXISTS that \
         silently no-ops.\n\nupstream constant:\n{upstream}"
    );
}
