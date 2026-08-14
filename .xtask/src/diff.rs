//! A minimal, dependency-free line diff.
//!
//! Shared by [`crate::bootstrap_sql`] and [`crate::migrations_current`],
//! both of which exist to show a reviewer exactly what drifted between a
//! committed file and a freshly regenerated one. Not a real LCS-based
//! diff — a positional line-by-line comparison — which is enough for this
//! use: both callers compare a generator's own output against its own
//! prior output, so a genuine drift is almost always a contiguous block of
//! changed/added/removed lines, not an interleaved rearrangement a proper
//! diff algorithm would render more cleverly. Deliberately not shelling out
//! to the system `diff` binary: this keeps the highest-risk check in this
//! crate (`bootstrap_sql`) free of any dependency on what happens to be
//! installed on the runner.
pub fn line_diff(old: &str, new: &str) -> Vec<String> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let max = old_lines.len().max(new_lines.len());
    let mut out = Vec::new();
    for i in 0..max {
        match (old_lines.get(i), new_lines.get(i)) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => {
                out.push(format!("- {a}"));
                out.push(format!("+ {b}"));
            }
            (Some(a), None) => out.push(format!("- {a}")),
            (None, Some(b)) => out.push(format!("+ {b}")),
            (None, None) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_has_no_diff() {
        assert!(line_diff("a\nb\n", "a\nb\n").is_empty());
    }

    #[test]
    fn reports_changed_added_and_removed_lines() {
        let d = line_diff("a\nb\nc\n", "a\nx\nc\nd\n");
        assert_eq!(d, vec!["- b", "+ x", "+ d"]);
    }
}
