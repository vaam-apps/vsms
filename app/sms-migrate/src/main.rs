//! The one-shot migration runner — §9.2's "which component applies
//! migrations, when, and what happens if two instances start at once."
//!
//! Replaces `deploy/migrate.Dockerfile`'s old `postgres:16-alpine` + `psql`
//! shell script (`deploy/migrate.sql`) with a small Rust binary that embeds
//! the same SQL at *compile* time via [`include_str!`] rather than `COPY`ing
//! `.sql` files into the image and shelling out to `psql` at *run* time.
//! `cratestack-sqlx` already vendors `sqlx-core`/`sqlx-postgres` under
//! `cratestack::sqlx` (`crates/sms-api`'s own `main.rs` already uses
//! `cratestack::sqlx::postgres::PgPoolOptions`) — this binary is the first
//! to reach past that facade for [`cratestack::sqlx::raw_sql`], the one
//! piece deploy/migrate.sql's own logic actually needs and `CrateStack`'s
//! own generated delegates have no reason to expose.
//!
//! This is **not** an adoption of `sqlx`'s own migration framework (the
//! `sqlx::migrate!` macro, its `_sqlx_migrations` bookkeeping table, its
//! `<version>_<description>.sql` naming convention). That convention
//! doesn't fit this repo's committed layout — `schema/migrations/postgres/
//! <name>/{up,down}.sql`, generated wholesale by `cratestack migrate diff`
//! (see `AGENTS.md`'s "Regenerating migrations") — and adopting it would
//! mean either reshaping that generated output or running two divergent
//! bookkeeping tables side by side. Instead this ports
//! `deploy/migrate.sql`'s own `schema_migrations` bookkeeping logic
//! statement-for-statement into Rust: same advisory lock, same
//! per-migration "already applied?" check, same three names
//! (`0001_init`, `0002_bootstrap`, `cratestack_idempotency_table`). The
//! `.sql` this runs is unchanged, byte-for-byte, from what `psql \i` used
//! to run — only the runner changed.
//!
//! # R1
//!
//! Raw `sqlx` calls, not a `CrateStack` delegate — `ci/assert-no-raw-sqlx.sh`
//! allowlists this file by path. This is not a new exception: "migrations"
//! was already one of R1's four named exceptions
//! (`CONTRIBUTING.md`) before this binary existed; `deploy/migrate.sql`
//! was simply the exception's previous form.
//!
//! # Why a dedicated `PgConnection`, not a `Pool`
//!
//! `pg_advisory_lock` is session-scoped: the lock lives on whichever
//! Postgres backend process holds the connection, and is released the
//! moment that session ends. `crates/sms-worker/src/lease.rs` already
//! documents the trap a `Pool`'s `PoolConnection` sets here — it returns to
//! the pool rather than closing on drop, so a lock taken on a pooled
//! connection can silently outlive the code that took it, on a session a
//! later, unrelated query then reuses. A one-shot process like this one
//! exits immediately after either success or a propagated `Err`, so a bare
//! `PgConnection` closing on drop is sufficient — Postgres releases the
//! advisory lock the instant that socket closes, no explicit unlock
//! required on the error path. The happy path still unlocks explicitly
//! (matching `deploy/migrate.sql`'s own final statement), simply because
//! it costs nothing and keeps the two paths symmetric to read.

use anyhow::{Context, Result};
use cratestack::sqlx::{Connection, PgConnection};
use tracing::info;

/// A single named migration to check for and, if missing, apply — mirrors
/// one `\if`/`\else` block of `deploy/migrate.sql` exactly.
struct Migration {
    name: &'static str,
    sql: &'static str,
}

/// Applied in this order, matching `deploy/migrate.sql`'s own sequence.
/// Paths are relative to this file, three levels up to the repository
/// root (`src/` → `sms-migrate/` → `app/` → root) — `include_str!`
/// resolves at compile time, so the committed `.sql` text becomes part of
/// this binary rather than something the runtime image has to carry
/// separately.
const MIGRATIONS: &[Migration] = &[
    Migration {
        name: "0001_init",
        sql: include_str!("../../../schema/migrations/postgres/0001_init/up.sql"),
    },
    Migration {
        name: "0002_bootstrap",
        sql: include_str!("../../../schema/migrations/postgres/0002_bootstrap/up.sql"),
    },
    // `cratestack_idempotency`'s DDL — see `ci/idempotency-table.sql`'s own
    // header for why this is a third, separately-tracked "migration"
    // rather than folded into `0002_bootstrap`: it's the library's own
    // bookkeeping table, not part of `schema/schema.cstack`, and never
    // touched by `cratestack migrate diff`.
    Migration {
        name: "cratestack_idempotency_table",
        sql: include_str!("../../../ci/idempotency-table.sql"),
    },
];

/// Same key `deploy/migrate.sql` used (`hashtext('vsms_migrate')`) — stable
/// across runs, readable in `pg_locks` next to a role name, and confirmed
/// non-colliding with `crates/sms-worker/src/lease.rs`'s own advisory-lock
/// namespace (that one uses the two-argument `(classid, objid)` form,
/// which Postgres hashes through an entirely different code path than the
/// single-bigint form used here).
const ADVISORY_LOCK_SQL: &str = "SELECT pg_advisory_lock(hashtext('vsms_migrate'))";
const ADVISORY_UNLOCK_SQL: &str = "SELECT pg_advisory_unlock(hashtext('vsms_migrate'))";

const ENSURE_BOOKKEEPING_TABLE_SQL: &str = "\
    CREATE TABLE IF NOT EXISTS public.schema_migrations (\n\
    \x20 name        text PRIMARY KEY,\n\
    \x20 applied_at  timestamptz NOT NULL DEFAULT now()\n\
    )";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sms_migrate=info".into()),
        )
        .init();

    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?;

    // A dedicated, unpooled connection — see this module's own doc for why.
    let mut conn = PgConnection::connect(&database_url)
        .await
        .context("connecting to Postgres")?;

    cratestack::sqlx::raw_sql(ADVISORY_LOCK_SQL)
        .execute(&mut conn)
        .await
        .context("acquiring the migration advisory lock")?;

    // Everything from here on runs under the lock. Any `?` below returns
    // early and drops `conn` without an explicit unlock — safe, per this
    // module's own doc, because a dropped `PgConnection` closes its
    // session immediately and Postgres releases the lock with it.
    run_migrations(&mut conn).await?;

    cratestack::sqlx::raw_sql(ADVISORY_UNLOCK_SQL)
        .execute(&mut conn)
        .await
        .context("releasing the migration advisory lock")?;

    info!("migrations up to date");
    Ok(())
}

/// Applies [`MIGRATIONS`] in order, skipping any already recorded in
/// `public.schema_migrations` — the exact behaviour of
/// `deploy/migrate.sql`'s own `\if :applied` guard per migration.
async fn run_migrations(conn: &mut PgConnection) -> Result<()> {
    cratestack::sqlx::raw_sql(ENSURE_BOOKKEEPING_TABLE_SQL)
        .execute(&mut *conn)
        .await
        .context("ensuring public.schema_migrations exists")?;

    for migration in MIGRATIONS {
        let already_applied: bool = cratestack::sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM public.schema_migrations WHERE name = $1)",
        )
        .bind(migration.name)
        .fetch_one(&mut *conn)
        .await
        .with_context(|| format!("checking whether {} was already applied", migration.name))?;

        if already_applied {
            info!(migration = migration.name, "already applied — skipping");
            continue;
        }

        info!(migration = migration.name, "applying");
        // Wrapped in an implicit transaction by `raw_sql` itself when the
        // text contains more than one statement (sqlx's own documented
        // behaviour) — stricter than the old `psql \i` path, which
        // auto-committed each statement individually unless the script
        // opened its own `BEGIN`. None of the three files here use
        // anything transaction-incompatible (no `CREATE INDEX
        // CONCURRENTLY` — checked directly, not assumed), so this is a
        // strict improvement: a failure partway through a migration now
        // rolls back cleanly instead of leaving that migration half-applied.
        cratestack::sqlx::raw_sql(migration.sql)
            .execute(&mut *conn)
            .await
            .with_context(|| format!("applying migration {}", migration.name))?;

        cratestack::sqlx::query("INSERT INTO public.schema_migrations (name) VALUES ($1)")
            .bind(migration.name)
            .execute(&mut *conn)
            .await
            .with_context(|| format!("recording {} as applied", migration.name))?;
    }

    Ok(())
}
