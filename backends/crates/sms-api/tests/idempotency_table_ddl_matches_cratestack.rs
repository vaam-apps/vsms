//! #153: `backends/migrations/postgres/0003_idempotency_table/up.sql`
//! hand-copies `cratestack`'s own `IDEMPOTENCY_TABLE_DDL` constant —
//! `backends/crates/sms-api/src/router.rs`'s `router()` deliberately never calls
//! `SqlxIdempotencyStore::ensure_schema()` itself (see that function's own
//! doc for why `backends/apps/sms-migrate`/`ci/apply-migrations.sh` own this table
//! instead, both by applying that one file). A hand copy drifts silently
//! the moment the pinned `cratestack` family changes that constant's
//! shape — this test is the guard: it asserts the exact text in that file
//! is still present, byte for byte, in the library's own constant.
//!
//! No live database needed — this only compares two strings.

/// Same relative-path shape `include_server_schema!` already uses in
/// `src/lib.rs` for `../../schemas/vsms.cstack` — three directories up
/// from this crate (`backends/crates/sms-api/`) to the repository root, then into
/// `backends/migrations/postgres/0003_idempotency_table/up.sql`.
const IDEMPOTENCY_TABLE_SQL: &str =
    include_str!("../../../../backends/migrations/postgres/0003_idempotency_table/up.sql");

#[test]
fn idempotency_table_ddl_carries_cratestacks_own_ddl_verbatim() {
    let upstream = cratestack::idempotency::IDEMPOTENCY_TABLE_DDL.trim();
    assert!(
        IDEMPOTENCY_TABLE_SQL.contains(upstream),
        "backends/migrations/postgres/0003_idempotency_table/up.sql no longer matches \
         cratestack::idempotency::IDEMPOTENCY_TABLE_DDL — the pinned cratestack version \
         changed the table shape. Re-copy the constant's exact text into that file — but if a \
         real deployment may already have the table in the old shape, a second \
         `CREATE TABLE IF NOT EXISTS` under the same file (and the same `schema_migrations` \
         name, `0003_idempotency_table`) silently no-ops against it: that deployment needs a \
         real `ALTER TABLE`, which means either a new, separately-tracked migration alongside \
         this one, or hand-editing this file to an ALTER rather than a bare CREATE — a decision \
         to make deliberately, not something this test can pick for \
         you.\n\nupstream constant:\n{upstream}"
    );
}
