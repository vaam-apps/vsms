//! #252 — the hand-written Node SDK's enum unions must agree with
//! `schemas/vsms.cstack`.
//!
//! `sdks/node/vsms-sdk-node/src/types.ts` is deliberately hand-written, not
//! generated (its own module doc says so — the SDK curates a small public
//! surface out of a much larger schema). Hand-written means nothing catches
//! it drifting the moment a schema enum gains, loses, or renames a variant:
//! `cargo check`/`cargo test` never touch this package (it's TypeScript),
//! and `pnpm turbo run typecheck` only checks that the *type* is internally
//! consistent, never that it still matches the enum it was copied from.
//! That is the same "documentation/generated-code-by-hand asserts something
//! the schema does not" shape AGENTS.md's own doc-drift section records
//! repeatedly — just on a hand-curated SDK type instead of prose.
//!
//! Four enums are checked, the ones this SDK actually re-exposes:
//! `Encoding`, `OperatorCode`, `MessageClass`, `MessageState`. Both sides
//! are parsed with the same line-based approach `parity.rs` already uses
//! for the state-machine diagrams — this schema format is simple enough
//! (one bare identifier per line inside `enum Name { ... }`) that a real
//! parser would be more code for no more correctness.
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use regex::Regex;

const SCHEMA: &str = "schemas/vsms.cstack";
const TYPES_TS: &str = "sdks/node/vsms-sdk-node/src/types.ts";

/// The enums this SDK hand-curates a TypeScript union for. Adding a fifth
/// one to `types.ts` needs a matching entry here — there is no way to
/// derive "which enums does the SDK expose" from the schema itself, the
/// whole point of the SDK being a curated subset.
const CHECKED_ENUMS: &[&str] = &["Encoding", "OperatorCode", "MessageClass", "MessageState"];

/// Every bare variant name inside `enum <name> { ... }` in `schema.cstack`.
fn schema_enum_variants(schema: &str, name: &str) -> Option<BTreeSet<String>> {
    let start_re = Regex::new(&format!(r"(?m)^enum {name} \{{")).expect("fixed pattern");
    let start = start_re.find(schema)?.end();
    let end = start + schema[start..].find('}')?;
    let body = &schema[start..end];

    let mut variants = BTreeSet::new();
    for line in body.lines() {
        let ident = line.trim();
        if ident.is_empty() || ident.starts_with('@') || ident.starts_with("//") {
            continue;
        }
        // A bare identifier line — variants in this schema carry no
        // per-variant attributes today, so the whole trimmed line is the
        // variant name. If that ever changes, this line is the one to
        // widen (e.g. split on whitespace and take the first token).
        variants.insert(ident.to_owned());
    }
    Some(variants)
}

/// Every string literal inside `export type <name> = "a" | "b" | ...;` in
/// `types.ts` — the union may be written on one line or wrapped with a
/// leading `|` per line (as `MessageState` is), so this matches every
/// quoted literal between the `=` and the terminating `;`, not a
/// single-line shape.
fn ts_type_variants(source: &str, name: &str) -> Option<BTreeSet<String>> {
    let start_re = Regex::new(&format!(r"export type {name} =")).expect("fixed pattern");
    let start = start_re.find(source)?.end();
    let end = start + source[start..].find(';')?;
    let body = &source[start..end];

    let literal_re = Regex::new(r#""([^"]+)""#).expect("fixed pattern");
    Some(
        literal_re
            .captures_iter(body)
            .map(|c| c[1].to_owned())
            .collect(),
    )
}

pub fn run(root: &Path) -> Result<(), String> {
    let schema_path = root.join(SCHEMA);
    let types_path = root.join(TYPES_TS);

    let schema =
        fs::read_to_string(&schema_path).map_err(|e| format!("{}: {e}", schema_path.display()))?;
    let types_ts =
        fs::read_to_string(&types_path).map_err(|e| format!("{}: {e}", types_path.display()))?;

    let mut problems = Vec::new();

    for &name in CHECKED_ENUMS {
        let Some(schema_variants) = schema_enum_variants(&schema, name) else {
            problems.push(format!(
                "{name}: no `enum {name} {{ ... }}` found in {SCHEMA} — has it been renamed or removed?"
            ));
            continue;
        };
        let Some(ts_variants) = ts_type_variants(&types_ts, name) else {
            problems.push(format!(
                "{name}: no `export type {name} = ...;` found in {TYPES_TS} — has it been renamed or removed?"
            ));
            continue;
        };

        let missing_from_ts: Vec<_> = schema_variants.difference(&ts_variants).cloned().collect();
        let extra_in_ts: Vec<_> = ts_variants.difference(&schema_variants).cloned().collect();

        if !missing_from_ts.is_empty() || !extra_in_ts.is_empty() {
            let mut msg = format!("{name}: {TYPES_TS} has drifted from {SCHEMA}'s `enum {name}`.");
            if !missing_from_ts.is_empty() {
                let _ = write!(
                    msg,
                    "\n  in the schema but missing from types.ts: {}",
                    missing_from_ts.join(", ")
                );
            }
            if !extra_in_ts.is_empty() {
                let _ = write!(
                    msg,
                    "\n  in types.ts but not in the schema: {}",
                    extra_in_ts.join(", ")
                );
            }
            problems.push(msg);
        }
    }

    if problems.is_empty() {
        println!(
            "node-sdk-types-check: OK — {} matches {SCHEMA} for {}",
            TYPES_TS,
            CHECKED_ENUMS.join(", ")
        );
        return Ok(());
    }

    Err(format!(
        "node-sdk-types-check:\n\n{}\n\nUpdate {TYPES_TS} by hand to match — it is deliberately \
         hand-written, not generated (see its own module doc).",
        problems.join("\n\n")
    ))
}
