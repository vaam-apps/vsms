//! R2 — the state diagram and the transition table must agree.
//!
//! Port of the deleted `ci/assert-state-machine-parity.py`. Legal edges live
//! in `message_state_transitions` / `job_state_transitions` /
//! `attempt_state_transitions`; triggers reject everything else with
//! SQLSTATE `SM001`. The diagrams in the design doc are what people
//! actually read. When the two drift, the first symptom is a production
//! `SM001` on a transition that looks perfectly legal in the diagram — a
//! failure that is confusing precisely because the documentation is wrong.
//!
//! This compares them in both directions and fails on any asymmetry, plus
//! checks that "terminal state" (no outgoing edges) agrees between the two
//! representations.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use regex::{Regex, RegexBuilder};

const DOC: &str = "docs/architecture.md";
const SQL: &str = "schema/migrations/postgres/0002_bootstrap/up.sql";

/// mermaid's start/end pseudo-state. Entry into the initial state and exit
/// from a terminal one are not rows in the table — terminality is expressed
/// as "no outgoing rows" — so both are dropped before comparing.
const PSEUDO: &str = "[*]";

type Edge = (String, String);

struct Diagram {
    states: BTreeSet<String>,
    edges: BTreeSet<Edge>,
}

/// Every ```mermaid stateDiagram-v2``` block in `markdown`, as (states, edges).
fn mermaid_state_diagrams(markdown: &str) -> Vec<Diagram> {
    let block_re = RegexBuilder::new(r"```mermaid\n(.*?)```")
        .dot_matches_new_line(true)
        .build()
        .expect("fixed pattern");
    let edge_re = Regex::new(r"^\s*(\S+)\s*-->\s*([^:\n]+?)\s*(?::.*)?$").expect("fixed pattern");

    let mut out = Vec::new();
    for cap in block_re.captures_iter(markdown) {
        let block = &cap[1];
        let Some(first_line) = block.lines().next() else {
            continue;
        };
        if !first_line.contains("stateDiagram") {
            continue;
        }

        let mut edges: BTreeSet<Edge> = BTreeSet::new();
        let mut states: BTreeSet<String> = BTreeSet::new();
        for line in block.lines() {
            let Some(m) = edge_re.captures(line) else {
                continue;
            };
            let src = m[1].to_owned();
            let dst = m[2].to_owned();
            if src != PSEUDO {
                states.insert(src.clone());
            }
            if dst != PSEUDO {
                states.insert(dst.clone());
            }
            if src == PSEUDO || dst == PSEUDO {
                continue;
            }
            edges.insert((src, dst));
        }
        if !edges.is_empty() {
            out.push(Diagram { states, edges });
        }
    }
    out
}

/// Rows of the `VALUES` list in `INSERT INTO <table> (...) VALUES (...);`.
fn sql_transitions(sql: &str, table: &str) -> Result<BTreeSet<Edge>, String> {
    let insert_re = RegexBuilder::new(&format!(
        r"INSERT\s+INTO\s+{}\s*\([^)]*\)\s*VALUES(.*?);",
        regex::escape(table)
    ))
    .case_insensitive(true)
    .dot_matches_new_line(true)
    .build()
    .expect("fixed pattern shape, escaped table name");

    let Some(cap) = insert_re.captures(sql) else {
        return Err(format!("could not find an INSERT INTO {table} in {SQL}"));
    };
    let values = &cap[1];

    let row_re = Regex::new(r"\(\s*'(\w+)'\s*,\s*'(\w+)'\s*\)").expect("fixed pattern");
    Ok(row_re
        .captures_iter(values)
        .map(|m| (m[1].to_owned(), m[2].to_owned()))
        .collect())
}

/// Print any asymmetry. Returns `true` when the two agree.
fn report(name: &str, diagram: &BTreeSet<Edge>, table: &BTreeSet<Edge>) -> bool {
    let only_diagram: Vec<&Edge> = diagram.difference(table).collect();
    let only_table: Vec<&Edge> = table.difference(diagram).collect();

    if only_diagram.is_empty() && only_table.is_empty() {
        println!("  {name}: {} edges, diagram and table agree", table.len());
        return true;
    }

    eprintln!("  {name}: MISMATCH");
    for (src, dst) in &only_diagram {
        eprintln!(
            "    {src} -> {dst}: in the diagram, missing from the table. \
             Rust would propose it and Postgres would raise SM001."
        );
    }
    for (src, dst) in &only_table {
        eprintln!(
            "    {src} -> {dst}: in the table, missing from the diagram. Legal but undocumented."
        );
    }
    false
}

fn terminal_states(edges: &BTreeSet<Edge>, states: &BTreeSet<String>) -> BTreeSet<String> {
    states
        .iter()
        .filter(|s| !edges.iter().any(|(src, _)| src == *s))
        .cloned()
        .collect()
}

pub fn run(root: &Path) -> Result<(), String> {
    let doc_path = root.join(DOC);
    let sql_path = root.join(SQL);
    if !doc_path.exists() || !sql_path.exists() {
        return Err(format!(
            "run from the repository root; expected {DOC} and {SQL}"
        ));
    }

    let markdown = fs::read_to_string(&doc_path).map_err(|e| format!("{DOC}: {e}"))?;
    let sql = fs::read_to_string(&sql_path).map_err(|e| format!("{SQL}: {e}"))?;

    let diagrams = mermaid_state_diagrams(&markdown);
    if diagrams.len() < 3 {
        return Err(format!(
            "expected at least three stateDiagram-v2 blocks, found {}",
            diagrams.len()
        ));
    }

    let machines: [(&str, &str); 3] = [
        ("message", "message_state_transitions"),
        ("job", "job_state_transitions"),
        ("attempt", "attempt_state_transitions"),
    ];

    println!("state machine parity:");
    let mut ok = true;
    for (name, table_name) in machines {
        let table = sql_transitions(&sql, table_name)?;
        let table_states: BTreeSet<String> = table
            .iter()
            .flat_map(|(a, b)| [a.clone(), b.clone()])
            .collect();

        // Match each diagram to a machine by state overlap rather than by
        // position, so reordering the document does not silently compare
        // the wrong pair. Ties keep the first (document-order) match, same
        // as Python's `max()`.
        let mut best_idx = None;
        let mut best_overlap = 0usize;
        for (idx, d) in diagrams.iter().enumerate() {
            let overlap = d.states.intersection(&table_states).count();
            if best_idx.is_none() || overlap > best_overlap {
                best_idx = Some(idx);
                best_overlap = overlap;
            }
        }
        let best = &diagrams[best_idx.expect("diagrams is non-empty, checked above")];

        if best_overlap == 0 {
            return Err(format!("no diagram matches the {name} transition table"));
        }

        ok &= report(name, &best.edges, &table);

        let sql_terminal = terminal_states(&table, &table_states);
        let diagram_terminal = terminal_states(&best.edges, &best.states);
        if sql_terminal != diagram_terminal {
            eprintln!(
                "    terminal states disagree: table {:?} vs diagram {:?}",
                Vec::from_iter(&sql_terminal),
                Vec::from_iter(&diagram_terminal)
            );
            ok = false;
        }
    }

    if !ok {
        eprintln!();
        eprintln!(
            "R2: legal edges are the transition table. Fix whichever side is \
             wrong — the doc if the table is right, the migration if it is not."
        );
        return Err("state machine parity mismatch".to_owned());
    }
    println!("state machine parity OK");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_simple_state_diagram() {
        let md =
            "```mermaid\nstateDiagram-v2\n    [*] --> a\n    a --> b: reason\n    b --> [*]\n```\n";
        let diagrams = mermaid_state_diagrams(md);
        assert_eq!(diagrams.len(), 1);
        assert_eq!(diagrams[0].edges.len(), 1);
        assert!(diagrams[0]
            .edges
            .contains(&("a".to_owned(), "b".to_owned())));
        assert!(diagrams[0].states.contains("a"));
        assert!(diagrams[0].states.contains("b"));
        assert!(!diagrams[0].states.contains(PSEUDO));
    }

    #[test]
    fn ignores_non_state_diagrams() {
        let md = "```mermaid\nflowchart TD\n    a --> b\n```\n";
        assert!(mermaid_state_diagrams(md).is_empty());
    }

    #[test]
    fn extracts_sql_insert_rows() {
        let sql = "INSERT INTO message_state_transitions (from_state, to_state) VALUES\n    ('accepted','queued'), ('accepted','rejected');\n";
        let rows = sql_transitions(sql, "message_state_transitions").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.contains(&("accepted".to_owned(), "queued".to_owned())));
    }

    #[test]
    fn terminal_states_have_no_outgoing_edge() {
        let edges: BTreeSet<Edge> = [("a".to_owned(), "b".to_owned())].into_iter().collect();
        let states: BTreeSet<String> = ["a".to_owned(), "b".to_owned()].into_iter().collect();
        let terminal = terminal_states(&edges, &states);
        assert_eq!(terminal, BTreeSet::from(["b".to_owned()]));
    }
}
