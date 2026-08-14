//! Regenerate `backends/migrations/postgres/0002_bootstrap/up.sql` from
//! §2.10 of `docs/architecture.md`.
//!
//! Port of the deleted `ci/gen-bootstrap-sql.py`. This is the highest-risk
//! script in the whole `xtask` port: its output IS a committed migration,
//! applied to real databases, and a subtly-wrong port would not fail loudly
//! — it would silently generate a migration that differs from what a human
//! reviewing the diff expects. The porting discipline was: read the Python
//! source as a literal byte-manipulation spec, not as "generate roughly the
//! same SQL", and verify the two produce byte-identical output before
//! trusting this file at all (see the PR description for the empty `diff`).
//!
//! # What the Python did, translated line for line
//!
//! 1. Slice `docs/architecture.md` between the first `### 2.10 Hand-written
//!    SQL` and the first `## 3. The send path` (Python's `str.index`, i.e.
//!    the first occurrence of each — replicated with [`str::find`]).
//! 2. Extract every ```` ```sql\n...``` ```` fenced block in that slice, in
//!    document order.
//! 3. Right-trim each block, append one `\n`, and join the blocks with a
//!    single `\n` separator — which, because each already ends in `\n`,
//!    produces a **blank line between blocks**, not a bare newline. This
//!    is the one step where "equivalent" and "identical" diverge easily —
//!    a naive re-join without replicating that has the same table of
//!    contents but a different byte stream.
//! 4. Replace two literal, multi-line "`-- ... repeat for every table using
//!    @use(Timestamps)`" placeholders with one generated block per
//!    timestamped table — the search text keeps the doc's own (17- and
//!    4-space) indentation, the *generated replacement* uses its own (12-
//!    and 4-space) indentation, and those are deliberately different
//!    numbers copied from what the doc and the committed migration each
//!    already used, not a stylistic choice made here.
//! 5. Prepend a fixed header naming this file as the source of truth.
//!
//! # The one deliberate byte the verification diff is not empty against
//!
//! Verification proved this port byte-for-byte against the deleted
//! Python's real output using the Python's own header text (`... by
//! ci/gen-bootstrap-sql.py.`) — a genuinely empty `diff`, confirmed by
//! SHA-256, not just visual inspection. [`HEADER`] below then has that one
//! line updated to name `cargo xtask bootstrap-sql` instead, because
//! `ci/gen-bootstrap-sql.py` no longer exists once this PR deletes it — a
//! migration whose own comment names a deleted script is exactly the
//! "documentation asserts something the code does not do" pattern
//! `AGENTS.md` spends a great many words on elsewhere. So the one-line
//! diff between the pre-PR committed `0002_bootstrap/up.sql` and what this
//! module now generates is intentional and reviewed, not a port bug — see
//! the PR description for both diffs side by side.
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const DOC: &str = "docs/architecture.md";
const SECTION_START: &str = "### 2.10 Hand-written SQL";
const SECTION_END: &str = "## 3. The send path";
const OUTPUT: &str = "backends/migrations/postgres/0002_bootstrap/up.sql";

/// `ts` in the Python script, verbatim and in the same order — the models
/// that `@use(Timestamps)` and therefore need a `created_at`/`updated_at`
/// default plus a `touch_updated_at` trigger. Models with their own one-off
/// timestamp default (`DeliveryReceipt`, `AuditAnchor`, `RouteValidation`)
/// are deliberately absent — they are hand-written directly in the fenced
/// SQL block, not generated from this list.
const TIMESTAMPED_MODELS: [&str; 19] = [
    "App",
    "AppClient",
    "OauthClient",
    "OauthSigningKey",
    "ClientAssertion",
    "SenderId",
    "SenderIdRegistration",
    "Provider",
    "Route",
    "OperatorPrefixRule",
    "Message",
    "MessagePart",
    "Job",
    "OptOut",
    "ConsentRecord",
    "WebhookEndpoint",
    "User",
    "Role",
    "UserCredential",
];

/// Byte-for-byte the Python `hdr` triple-quoted string.
const HEADER: &str = "-- 0002_bootstrap / up.sql\n\
--\n\
-- Everything cratestack-migrate does not emit: identifier and timestamp\n\
-- defaults, the updated_at trigger, the two state machines, non-unique and\n\
-- partial indexes, and foreign keys.\n\
--\n\
-- Generated from docs/architecture.md section 2.10 by cargo xtask bootstrap-sql.\n\
-- Do not hand-edit: edit the document, regenerate, and commit both.\n\
\n";

pub struct Stats {
    pub line_count: usize,
    pub table_count: usize,
}

/// `tbl()` in the Python: insert `_` before every uppercase letter that
/// isn't the first character, lowercase the result, then pluralise with
/// the same naive `ends_with('s') ? +"es" : +"s"` rule the emitter itself
/// uses elsewhere (`AGENTS.md`'s own "`pluralize()` is naive" note).
fn table_name(model: &str) -> String {
    let mut snake = String::with_capacity(model.len() + 4);
    for (i, c) in model.chars().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            snake.push('_');
        }
        snake.push(c.to_ascii_lowercase());
    }
    if snake.ends_with('s') {
        snake.push_str("es");
    } else {
        snake.push('s');
    }
    snake
}

/// Every ```` ```sql\n...``` ```` fenced block in `text`, in document order,
/// with the fence markers stripped — `re.findall(r"```sql\n(.*?)```", ...,
/// re.S)`.
fn sql_fences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    let needle = "```sql\n";
    while let Some(start) = rest.find(needle) {
        let after_open = &rest[start + needle.len()..];
        let Some(end) = after_open.find("```") else {
            break;
        };
        out.push(&after_open[..end]);
        rest = &after_open[end + 3..];
    }
    out
}

/// Steps 1-5 of this module's own doc, producing the full file contents
/// plus the raw (pre-header) line count used for the printed stat.
fn render(doc: &str) -> Result<(String, usize), String> {
    let start = doc
        .find(SECTION_START)
        .ok_or_else(|| format!("{SECTION_START:?} not found in {DOC}"))?;
    let end = doc
        .find(SECTION_END)
        .ok_or_else(|| format!("{SECTION_END:?} not found in {DOC}"))?;
    if end < start {
        return Err(format!(
            "{SECTION_END:?} appears before {SECTION_START:?} in {DOC}"
        ));
    }
    let section = &doc[start..end];

    let blocks = sql_fences(section);
    let joined: String = blocks
        .iter()
        .map(|b| format!("{}\n", b.trim_end()))
        .collect::<Vec<_>>()
        .join("\n");

    let tables: Vec<String> = TIMESTAMPED_MODELS.iter().map(|m| table_name(m)).collect();

    let timestamps_search = format!(
        "ALTER TABLE apps ALTER COLUMN created_at SET DEFAULT now(),\n\
         {}ALTER COLUMN updated_at SET DEFAULT now();\n\
         -- ... repeat for every table using @use(Timestamps)",
        " ".repeat(17)
    );
    let timestamps_replacement = tables
        .iter()
        .map(|t| {
            format!(
                "ALTER TABLE {t} ALTER COLUMN created_at SET DEFAULT now(),\n\
                 {}ALTER COLUMN updated_at SET DEFAULT now();",
                " ".repeat(12)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let trigger_search = format!(
        "CREATE TRIGGER apps_touch BEFORE UPDATE ON apps\n\
         {}FOR EACH ROW EXECUTE FUNCTION touch_updated_at();\n\
         -- ... repeat for every table using @use(Timestamps)",
        " ".repeat(4)
    );
    let trigger_replacement = tables
        .iter()
        .map(|t| {
            format!(
                "CREATE TRIGGER {t}_touch BEFORE UPDATE ON {t}\n\
                 {}FOR EACH ROW EXECUTE FUNCTION touch_updated_at();",
                " ".repeat(4)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let raw = joined
        .replace(&timestamps_search, &timestamps_replacement)
        .replace(&trigger_search, &trigger_replacement);

    if raw.contains("repeat for every table") {
        return Err(
            "bootstrap-sql: a 'repeat for every table' placeholder survived both \
             replacements — the search text no longer matches docs/architecture.md \
             verbatim (indentation or wording drifted)"
                .to_owned(),
        );
    }

    // The stat this module prints counts `raw` only, not the header — same
    // as the Python original, whose own `print(f"{len(raw.splitlines())}
    // ...")` runs before `hdr` is ever prepended.
    let raw_line_count = raw.lines().count();

    let mut out = String::with_capacity(HEADER.len() + raw.len());
    out.push_str(HEADER);
    out.push_str(&raw);
    Ok((out, raw_line_count))
}

pub fn generate(root: &Path, output: &Path) -> Result<Stats, String> {
    let doc = fs::read_to_string(root.join(DOC)).map_err(|e| format!("{DOC}: {e}"))?;
    let (rendered, raw_line_count) = render(&doc)?;
    fs::write(output, &rendered).map_err(|e| format!("{}: {e}", output.display()))?;
    Ok(Stats {
        line_count: raw_line_count,
        table_count: TIMESTAMPED_MODELS.len(),
    })
}

/// `just bootstrap-sql-check` / the `migrations` CI job: regenerate into a
/// scratch file and diff against the committed migration.
pub fn check(root: &Path) -> Result<(), String> {
    let doc = fs::read_to_string(root.join(DOC)).map_err(|e| format!("{DOC}: {e}"))?;
    let (rendered, _) = render(&doc)?;
    let committed_path = root.join(OUTPUT);
    let committed = fs::read_to_string(&committed_path)
        .map_err(|e| format!("{}: {e}", committed_path.display()))?;

    if rendered == committed {
        println!("bootstrap-sql-check: OK — {OUTPUT} matches docs/architecture.md §2.10");
        return Ok(());
    }

    let mut msg = String::new();
    let _ = writeln!(
        msg,
        "{OUTPUT} is stale — regenerate it with `cargo xtask bootstrap-sql {OUTPUT}`\n"
    );
    for diff_line in crate::diff::line_diff(&committed, &rendered) {
        msg.push_str(&diff_line);
        msg.push('\n');
    }
    Err(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_name_matches_the_naive_pluraliser() {
        assert_eq!(table_name("App"), "apps");
        assert_eq!(table_name("AppClient"), "app_clients");
        assert_eq!(table_name("OauthSigningKey"), "oauth_signing_keys");
        assert_eq!(
            table_name("SenderIdRegistration"),
            "sender_id_registrations"
        );
        assert_eq!(table_name("OptOut"), "opt_outs");
        assert_eq!(table_name("UserCredential"), "user_credentials");
    }

    #[test]
    fn sql_fences_extracts_every_block_in_order() {
        let text = "prose\n```sql\nSELECT 1;\n```\nmore prose\n```sql\nSELECT 2;\n```\n";
        let blocks = sql_fences(text);
        assert_eq!(blocks, vec!["SELECT 1;\n", "SELECT 2;\n"]);
    }

    #[test]
    fn render_fails_loudly_if_the_section_markers_move() {
        let doc = "no markers here";
        assert!(render(doc).is_err());
    }
}
