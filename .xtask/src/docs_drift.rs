//! Every documentation path this repository names must exist.
//!
//! # Why this exists
//!
//! `AGENTS.md` records "documentation asserts something the code does not
//! do" as this repository's single most-repeated defect, at least eight
//! separate times: the M1 `/token`-fails-at-startup claim, `rust-version`
//! declaring 1.85 against a lockfile needing 1.88, the `/token`
//! rate-limiting table naming a crate `sms-auth` never linked, `msisdnHash`
//! documented as HMAC while shipping bare SHA-256 (#134 — a security
//! consequence), `seed-provider` runbooks sending an operator to a command
//! that never seeded a `Route`, the SDK's `private_key_jwt` doc comment
//! claiming `base_url` backs the token endpoint, and `compose.dev.yaml`'s
//! post-rename paths — that last one found twice, independently, on the
//! same day, because nothing short of running `just demo` surfaced it.
//!
//! Every one of those was caught by a human noticing, or by production.
//! [`crate::workflow_paths`] closed the build-file half of the problem
//! after `release.yml` shipped five broken merges. This is the docs half:
//! the part of a documentation claim that a machine *can* check is whether
//! the things it points at are still there.
//!
//! # What is checked
//!
//! Two rules, both chosen because a failure is unambiguous rather than a
//! matter of taste:
//!
//! 1. **Hyperlinks in `.md` and `.adoc` resolve.** Markdown `[text](path)`,
//!    `AsciiDoc` `xref:path[]` and `link:path[]`. Someone wrote a link
//!    intending it to be followed; if it 404s that is a bug with no second
//!    reading. Targets are resolved relative to the linking file.
//!
//! 2. **Bare `docs/…` path mentions resolve, anywhere in the repository.**
//!    Not just in docs — in `deploy/prometheus/alerts.yml`'s alert
//!    annotations (which an on-call engineer clicks during an incident), in
//!    `deploy/.env.example` comments, in Helm values, and in Rust strings
//!    printed to an operator mid-restore (`deploy/backup-tool/src/restore.rs`
//!    embeds one in a real warning, not a comment). A stale path there
//!    misdirects a human at the worst possible moment.
//!
//! # What is deliberately not checked
//!
//! **Hyperlinks inside the `include_str!` sidecars** — the `.md` files that
//! now carry each module's prose, identified by living under a `src/`
//! directory. Their links are rustdoc intra-doc references into the Rust
//! item namespace (`crate::FakeOrange`, `sms_api::schema::RouteValidation`,
//! `../sms_api/index.html`), not filesystem paths, and the first version of
//! this check duly reported all seven of them as broken. `rustdoc` already
//! warns on genuinely broken intra-doc links; a second, worse checker over
//! the same ground is precisely how a guard earns the false positives that
//! get it deleted. Rule 2 still applies to these files, which is what
//! catches a stale `docs/…` pointer in a module doc.
//!
//! **Bare path mentions other than `docs/…`** — `backends/…`, `deploy/…`,
//! `app/…`. Prose legitimately names paths that no longer exist, because
//! `AGENTS.md` is a historical record: it discusses `app/sms-migrate` in the
//! past tense precisely because that path was renamed. A guard that cannot
//! tell a live pointer from a narrative one produces false positives, and a
//! guard with false positives gets deleted. `docs/…` is exempt from that
//! ambiguity in practice — this repository's documentation tree has been
//! reorganised, not rewritten, so a `docs/…` mention is a live pointer.
//!
//! Also skipped, for the same reason [`crate::workflow_paths`] skips them:
//! URLs, anything containing `${{`, glob patterns, and `<placeholder>`
//! spellings — none is a literal path this check could evaluate.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

/// A single unresolvable reference, reported with enough context to fix it
/// without opening anything else.
struct Finding {
    file: String,
    line: usize,
    target: String,
    kind: &'static str,
}

pub fn run(root: &Path) -> Result<(), String> {
    let files = tracked_files(root)?;

    let md_link = Regex::new(r"\[[^\]]*\]\(([^)\s]+)").map_err(|e| e.to_string())?;
    let adoc_link = Regex::new(r"(?:xref|link):([^\[\s]+)\[").map_err(|e| e.to_string())?;
    let docs_path =
        Regex::new(r"docs/[A-Za-z0-9._/-]*\.[A-Za-z0-9]+").map_err(|e| e.to_string())?;

    let mut findings = Vec::new();

    for rel in &files {
        let abs = root.join(rel);
        let Ok(text) = fs::read_to_string(&abs) else {
            continue; // binary or unreadable: nothing to check
        };
        let ext = Path::new(rel)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        // A `.md` under a `src/` directory is an `include_str!` sidecar:
        // rustdoc content, whose links are intra-doc references into the Rust
        // item namespace (`crate::Foo`, `../sms_api/index.html`), not
        // filesystem paths. rustdoc itself already warns on broken ones; a
        // second, worse checker over the same ground is how a guard earns the
        // false positives that get it deleted.
        let is_rustdoc_sidecar = rel.contains("/src/");
        let is_doc = matches!(ext, "md" | "adoc") && !is_rustdoc_sidecar;

        for (idx, line) in text.lines().enumerate() {
            let lineno = idx + 1;

            if is_doc {
                for caps in md_link.captures_iter(line) {
                    check(&caps[1], rel, lineno, "link", root, &mut findings);
                }
                for caps in adoc_link.captures_iter(line) {
                    check(&caps[1], rel, lineno, "link", root, &mut findings);
                }
            }

            // Rule 2 applies everywhere, including non-doc files. Skip the
            // historical record: see the module doc.
            if rel != "AGENTS.md" && rel != "CLAUDE.md" {
                for m in docs_path.find_iter(line) {
                    check(m.as_str(), rel, lineno, "docs path", root, &mut findings);
                }
            }
        }
    }

    // A reference can match both rules (a markdown link whose target also
    // starts with `docs/`). Report each distinct one once.
    findings.sort_by(|a, b| (&a.file, a.line, &a.target).cmp(&(&b.file, b.line, &b.target)));
    findings.dedup_by(|a, b| a.file == b.file && a.line == b.line && a.target == b.target);

    if findings.is_empty() {
        println!("docs-drift: every documentation path resolves");
        return Ok(());
    }

    let mut out = format!(
        "{} unresolvable documentation reference(s):\n",
        findings.len()
    );
    for f in &findings {
        let _ = writeln!(
            out,
            "  {}:{}: {} -> {} (no such file)",
            f.file, f.line, f.kind, f.target
        );
    }
    out.push_str("\nA renamed or deleted doc leaves these pointing at nothing. Fix the\nreference, or restore the target.");
    Err(out)
}

/// Decide whether `raw` is a checkable repo path, and if so whether it exists.
fn check(
    raw: &str,
    from: &str,
    line: usize,
    kind: &'static str,
    root: &Path,
    findings: &mut Vec<Finding>,
) {
    let target = raw.trim_end_matches(['.', ',', ':', ';', ')', '"', '\'', '`']);
    // Strip an anchor: the file must exist; which heading it lands on is not
    // statically checkable across two markup languages.
    let target = target.split('#').next().unwrap_or(target);

    if target.is_empty()
        || target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.contains("${{")
        || target.contains('*')
        || target.contains('<')
        || target.contains('>')
        || target.contains("://")
    {
        return;
    }

    // Resolve: a link is relative to its own file; a bare `docs/…` mention is
    // relative to the repository root.
    let candidate: PathBuf = if target.starts_with("docs/") {
        root.join(target)
    } else {
        let dir = Path::new(from).parent().unwrap_or(Path::new(""));
        root.join(normalise(&dir.join(target)))
    };

    if !candidate.exists() {
        findings.push(Finding {
            file: from.to_owned(),
            line,
            target: target.to_owned(),
            kind,
        });
    }
}

/// Resolve `..` segments textually. `Path::canonicalize` is unusable here —
/// it requires the path to exist, which is the very thing being tested.
fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in p.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Every tracked file, plus untracked-but-present ones, so a doc added in the
/// working tree is checked before it is ever committed.
fn tracked_files(root: &Path) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    for args in [
        vec!["ls-files"],
        vec!["ls-files", "--others", "--exclude-standard"],
    ] {
        let listing = std::process::Command::new("git")
            .args(&args)
            .current_dir(root)
            .output()
            .map_err(|e| format!("running git {args:?}: {e}"))?;
        if !listing.status.success() {
            return Err(format!("git {args:?} failed"));
        }
        for line in String::from_utf8_lossy(&listing.stdout).lines() {
            if line.is_empty() || line.contains("node_modules") || line.contains("/target/") {
                continue;
            }
            out.insert(line.to_owned());
        }
    }
    Ok(out)
}
