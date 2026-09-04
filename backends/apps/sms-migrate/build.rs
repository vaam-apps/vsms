//! Generates `$OUT_DIR/migrations.rs`: a `&[Migration]` literal built from
//! every subdirectory of `backends/migrations/postgres` that contains an
//! `up.sql`, sorted lexically (`0001_init` before `0002_bootstrap` before
//! `0003_idempotency_table`, and so on for whatever gets added later).
//!
//! This exists so `src/main.rs` doesn't hand-list each migration by name —
//! sqlx's own directory-scanning `Migrator`/`MigrationSource` (re-exported
//! at `cratestack::sqlx::migrate`, no new dependency needed) was
//! considered and rejected for this: its resolver
//! (`sqlx-core-0.8.6/src/migrate/source.rs`) only reads flat files
//! directly inside one directory, named `<VERSION>_<DESCRIPTION>.sql`, and
//! silently skips anything that isn't a file — so it cannot see this
//! repo's `<name>/up.sql` layout at all (confirmed by reading that
//! resolver's source, not assumed). That layout is fixed by
//! `cratestack migrate diff --out-dir backends/migrations --backend postgres
//! --name <name>`'s own output shape and by `AGENTS.md`'s "never hand-edit"
//! rule, so the fix is a small build script that reads the same directory
//! shape our own tooling already produces, not a reshape to fit sqlx's
//! convention.
//!
//! `up.pre.sql`, when a directory has one, is embedded too, as a second,
//! optional `include_str!` on the same `Migration` literal — cratestack
//! migrate diff (>=0.11.0) scaffolds one whenever it detects a blocking
//! operation (`cratestack-migrate-0.11.0/src/emit/postgres/up_pre.rs`'s
//! own doc: "The runner executes this file immediately before `up.sql`,
//! inside the SAME transaction... both halves land or neither does").
//! `main.rs` is the runner referred to there — see its own doc for how it
//! honours that.
//!
//! Each `include_str!` call embeds the file's content into the binary at
//! *compile* time, same as the hand-written version this replaced — the
//! point of a build script here is only to generate the *list* of
//! `include_str!` calls from what's on disk, not to change when or how
//! the SQL gets embedded. `backends/apps/sms-migrate/Dockerfile`'s runtime image
//! still needs no `COPY` of `schema/` — the SQL is inside the binary
//! either way.

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo");
    let migrations_root = Path::new(&manifest_dir).join("../../../backends/migrations/postgres");

    println!("cargo:rerun-if-changed={}", migrations_root.display());

    let mut names: Vec<String> = std::fs::read_dir(&migrations_root)
        .unwrap_or_else(|e| panic!("reading {}: {e}", migrations_root.display()))
        .map(|entry| entry.unwrap_or_else(|e| panic!("reading a directory entry: {e}")))
        .filter(|entry| entry.path().is_dir())
        .filter(|entry| entry.path().join("up.sql").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    // Lexical order is apply order — `0001_init` before `0002_bootstrap`
    // before `0003_idempotency_table` — matching every other reader of
    // this directory (`ci/apply-migrations.sh`'s glob loop,
    // `backends/crates/sms-test-support`'s own `migration_dirs()`).
    names.sort();

    assert!(
        !names.is_empty(),
        "no migrations found under {} — has the workspace layout changed?",
        migrations_root.display()
    );

    let mut generated = String::from("&[\n");
    for name in &names {
        let up_sql_path = migrations_root
            .join(name)
            .join("up.sql")
            .canonicalize()
            .unwrap_or_else(|e| panic!("resolving the path to {name}/up.sql: {e}"));
        println!("cargo:rerun-if-changed={}", up_sql_path.display());

        // `up.pre.sql` is conditional — most migrations don't have one.
        // `pre_sql: None` when the file is absent; `rerun-if-changed` is
        // already registered on the *directory* itself (above, before
        // this loop), so adding one later still triggers a rebuild even
        // though this specific file didn't exist to watch at that point.
        let up_pre_sql_path = migrations_root.join(name).join("up.pre.sql");
        let pre_sql_literal = if up_pre_sql_path.is_file() {
            let up_pre_sql_path = up_pre_sql_path
                .canonicalize()
                .unwrap_or_else(|e| panic!("resolving the path to {name}/up.pre.sql: {e}"));
            println!("cargo:rerun-if-changed={}", up_pre_sql_path.display());
            // A statement, not a bare tail expression, so the attribute
            // below actually attaches to something — rustc ignores an
            // outer attribute on a macro invocation used as a tail
            // expression (confirmed live: it built with a warning until
            // this was changed).
            #[allow(clippy::unnecessary_debug_formatting)]
            let literal = format!("Some(include_str!({up_pre_sql_path:?}))");
            literal
        } else {
            "None".to_owned()
        };

        // `include_str!` resolves a relative path against the file it
        // appears in — which, once this is written to `$OUT_DIR`, is not
        // this crate's own `src/` — so the embedded path must be absolute.
        // `{up_sql_path:?}` is deliberate, not clippy's suggested
        // `.display()`: this has to produce a valid, escaped Rust string
        // *literal* for the generated source below, which is exactly what
        // `Debug` on a `Path` gives and `Display` does not (no quotes, no
        // escaping).
        #[allow(clippy::unnecessary_debug_formatting)]
        writeln!(
            generated,
            "    crate::Migration {{ name: {name:?}, pre_sql: {pre_sql_literal}, sql: include_str!({up_sql_path:?}) }},",
        )
        .expect("writing to an in-memory String");
    }
    generated.push(']');

    let dest = Path::new(&std::env::var("OUT_DIR").expect("set by cargo")).join("migrations.rs");
    std::fs::write(&dest, generated).unwrap_or_else(|e| panic!("writing {}: {e}", dest.display()));
}
