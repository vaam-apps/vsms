#![doc = include_str!("db.md")]

use anyhow::{Context, Result};
use postgres::{Client, NoTls};

pub fn connect(database_url: &str) -> Result<Client> {
    Client::connect(database_url, NoTls)
        .with_context(|| format!("connecting to postgres at {}", redact(database_url)))
}

/// Redacts any credentials embedded in a `postgres://user:pass@host/db`
/// URL before it ever reaches a log line — mirrors the old `backup.sh`'s
/// own `postgres://***@${DATABASE_URL#*@}` trick, done properly instead
/// of by string-slicing on the first `@`.
pub fn redact(database_url: &str) -> String {
    match database_url.split_once("://").and_then(|(scheme, rest)| {
        rest.split_once('@')
            .map(|(_, host_and_db)| format!("{scheme}://***@{host_and_db}"))
    }) {
        Some(redacted) => redacted,
        None => "***".to_owned(),
    }
}

/// `app/sms-migrate`'s own bookkeeping table (not part of the committed
/// `schema/migrations` tree). A database that predates `sms-migrate`, or
/// was migrated by hand, simply won't have it — that's an empty string
/// here, matching the old script's `2>/dev/null || echo ""` fallback,
/// never a hard failure.
pub fn applied_migrations(client: &mut Client) -> Result<String> {
    let row = client.query_opt(
        "SELECT coalesce(string_agg(name, ',' ORDER BY name), '') \
         FROM public.schema_migrations",
        &[],
    );
    match row {
        Ok(Some(row)) => Ok(row.get::<_, String>(0)),
        Ok(None) => Ok(String::new()),
        // `relation "public.schema_migrations" does not exist` (42P01) is
        // the expected shape of "this database predates sms-migrate" —
        // anything else propagates.
        Err(error) if error.code().map(|c| c.code()) == Some("42P01") => Ok(String::new()),
        Err(error) => Err(error).context("querying public.schema_migrations"),
    }
}

/// One exact `count(*)` per table in `public`, table name -> row count.
/// A `BTreeMap` (not a `HashMap`) so two snapshots compare and print in
/// stable, sorted order — the direct Rust equivalent of the old drill's
/// own `t=c,t=c,...` string, without the string-building or the risk of
/// two runs producing the same map in a different order and looking
/// "different" when they aren't.
pub fn row_counts(client: &mut Client) -> Result<std::collections::BTreeMap<String, i64>> {
    let tables = client
        .query(
            "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename",
            &[],
        )
        .context("listing public tables")?;

    let mut counts = std::collections::BTreeMap::new();
    for row in tables {
        let table: String = row.get(0);
        // Table names here come from `pg_tables` itself, never caller
        // input, so a quoted-identifier interpolation is safe — there is
        // no parameterised way to bind a table name in SQL.
        let count_row = client
            .query_one(&format!("SELECT count(*) FROM \"{table}\""), &[])
            .with_context(|| format!("counting rows in {table}"))?;
        let count: i64 = count_row.get(0);
        counts.insert(table, count);
    }
    Ok(counts)
}

/// Seeds `restore-drill`'s own marker table + one recognisable row.
/// `IF NOT EXISTS` because a drill run against a database that already
/// has this table from a previous drill must not fail on the second run.
pub fn seed_marker(client: &mut Client, marker_id: &str) -> Result<()> {
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS public.backup_drill_marker (\
                id text PRIMARY KEY, \
                note text NOT NULL, \
                created_at timestamptz NOT NULL DEFAULT now()\
            )",
        )
        .context("creating public.backup_drill_marker")?;
    client
        .execute(
            "INSERT INTO public.backup_drill_marker (id, note) VALUES ($1, $2)",
            &[&marker_id, &"restore-drill-proof"],
        )
        .context("seeding the marker row")?;
    Ok(())
}

/// `None` if the row is missing entirely after a restore — a real, distinct
/// failure from "present but wrong", both handled by the caller.
pub fn read_marker_note(client: &mut Client, marker_id: &str) -> Result<Option<String>> {
    let row = client
        .query_opt(
            "SELECT note FROM public.backup_drill_marker WHERE id = $1",
            &[&marker_id],
        )
        .context("reading the marker row back")?;
    Ok(row.map(|row| row.get::<_, String>(0)))
}

/// The drill's own destructive step — `DROP SCHEMA public CASCADE;
/// CREATE SCHEMA public;`, not a real `dropdb`/`createdb`. Needs only the
/// one connection this tool already has (no separate admin connection to
/// a different database), and works against a managed Postgres where the
/// application's own role can never hold `DROP DATABASE` — an honest
/// stand-in for "destroy the database," documented at length in
/// `docs/runbooks/backup-restore.md`.
pub fn destroy_public_schema(client: &mut Client) -> Result<()> {
    client
        .batch_execute("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .context("dropping and recreating the public schema")
}

pub fn count_public_tables(client: &mut Client) -> Result<i64> {
    let row = client
        .query_one(
            "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'",
            &[],
        )
        .context("counting tables in public")?;
    Ok(row.get(0))
}
