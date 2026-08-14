//! Repository automation for vsms. Run via `cargo xtask <cmd>` or `just`.
//!
//! This crate exists because the maintainer's standing direction is: no bash
//! (or Python) script survives in this repo — `xtask` plus Docker, for full
//! portability. Every subcommand here is a straight port of a script that
//! used to live under `ci/` (or `sdks/rust/vsms-sdk-rust/`); the deleted
//! original is named in each module's own doc comment, along with anything
//! non-obvious found while porting it.
//!
//! Structure: one module per check, so a future subcommand only ever
//! conflicts with the `match` arm in [`main`], never with another check's
//! internals. Deliberately depends on nothing that expands
//! `include_server_schema!` (no `sms-api`, no `cratestack`) — that macro is
//! memory-hungry (see the root `AGENTS.md`'s "Build cost" section), and
//! `cargo xtask` should stay fast regardless of what else is building.

// This is a CLI; stdout/stderr are its output medium, not stray debugging.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod bootstrap_sql;
mod cratestack_pin;
mod diff;
mod migrations_current;
mod parity;
mod raw_sqlx;
mod sdk_schema;
mod workflow_paths;

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "help".to_owned());
    let root = repo_root();

    let result = match cmd.as_str() {
        "no-raw-sqlx" => raw_sqlx::run(&root),
        "parity" => parity::run(&root),
        "workflow-paths" => workflow_paths::run(&root),
        "bootstrap-sql" => {
            let Some(out) = args.next() else {
                eprintln!("usage: cargo xtask bootstrap-sql <output-path>");
                return ExitCode::FAILURE;
            };
            bootstrap_sql::generate(&root, Path::new(&out)).map(|stats| {
                println!(
                    "{} lines, {} timestamped tables",
                    stats.line_count, stats.table_count
                );
            })
        }
        "bootstrap-sql-check" => bootstrap_sql::check(&root),
        "sdk-schema-check" => sdk_schema::check(&root),
        "sdk-schema-vendor" => sdk_schema::vendor(&root),
        "cratestack-pin" => cratestack_pin::read_pin(&root).map(|v| println!("{v}")),
        "migrations-current" => migrations_current::run(&root),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(format!("unknown command: {other}\n\n{}", help_text())),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("xtask: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!("{}", help_text());
}

fn help_text() -> &'static str {
    "usage: cargo xtask <command>\n\n\
     commands:\n  \
     no-raw-sqlx          R1 — no raw sqlx outside the named exceptions\n  \
     parity                R2 — state diagrams and transition tables agree\n  \
     bootstrap-sql <out>   regenerate 0002_bootstrap/up.sql from docs/architecture.md §2.10\n  \
     bootstrap-sql-check   fail if 0002_bootstrap/up.sql has drifted from the design doc\n  \
     sdk-schema-check      fail if the vendored SDK schema has drifted\n  \
     sdk-schema-vendor     refresh the vendored SDK schema copy\n  \
     cratestack-pin         print the pinned cratestack version from Cargo.toml\n  \
     migrations-current    fail if 0001_init has drifted from `cratestack migrate diff`\n  \
     workflow-paths        fail if a workflow names a path that does not exist"
}

/// The directory containing this crate's own `Cargo.toml`'s parent — i.e.
/// the workspace root, since `.xtask` lives directly under it.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf()
}
