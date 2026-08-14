//! R6 — UI architecture: pages compose, smart components decide, dumb
//! components style.
//!
//! R6 landed in #269 as prose. This module is the guard, and it exists for
//! one reason the repo has already paid for fifteen times: a rule with no
//! mechanical check gets violated repeatedly and silently. `AGENTS.md`'s own
//! "Invariants that fail the build rather than production" section makes the
//! point directly — the `hasRole('system')` policy gap was found *seven*
//! separate times before #155 finally wrote a golden test for it, and the
//! section's closing line ("until someone writes it, expect an eighth") was
//! correct: instances nine through fifteen followed. R6 has exactly the same
//! shape — invisible when broken, obvious only to whoever happens to reread
//! the file — so it gets a guard on the way in rather than after the seventh
//! repeat.
//!
//! # What is checked, and what deliberately is not
//!
//! Three checks here are **exact**: they have no heuristic component and no
//! plausible false positive, so they hard-fail.
//!
//! 1. A view file contains no `className=` and no `cn(` call.
//! 2. A view file contains no string literal carrying a Tailwind responsive
//!    variant (`lg:table-cell`) — this is what catches a hoisted
//!    `const COL_ID = "hidden lg:table-cell"`, which check 1 alone would
//!    miss if the const were exported rather than applied locally.
//! 3. A smart component contains no raw HTML markup (`<div`, `<table`, …).
//!
//! `useState` is **counted and printed, never failed on**. R6's own text
//! says "avoid", not "never", and grants an explicit escape hatch — genuinely
//! ephemeral single-value presentational state is allowed, and anything else
//! "needs a sentence in the PR saying which of the above was considered and
//! why it did not fit". A hard failure would contradict the rule it claims to
//! enforce. Reporting the count keeps it visible without pretending a
//! judgement call is a lint.
//!
//! Likewise **not** checked: whether a dumb component fetches its own data
//! (a real R6 violation — `opt-outs-screen.tsx`'s `SearchPanel` calling tRPC
//! internally is the known instance), and whether a hoisted mapping object or
//! date helper should have moved to a pure module. Both need to know what a
//! symbol *means*, not what it looks like. A regex that guessed at either
//! would produce exactly the false positives that get a guard deleted six
//! months later. They stay prose, reviewed by humans.
//!
//! # The gap this guard has, stated rather than papered over
//!
//! A file is classified as a view **by its name** — `page.tsx`,
//! `layout.tsx`, `*-screen.tsx`. So the guard enforces R6 *given* the
//! naming convention, and a smart component named something else is
//! invisible to it. That is not hypothetical: `console-shell.tsx` carries
//! 14 `className` occurrences today and is not scanned, because by R6's own
//! layer table it is a component, not a view — which is the right answer
//! here, but the same mechanism would let a genuine smart component escape
//! by being renamed `jobs-view.tsx`.
//!
//! Closing that would mean inferring "is this a smart component" from
//! whether it calls `useQuery`/`useMutation`, which is a real signal but
//! also fires on a page that legitimately composes one. Left as a known
//! limitation rather than guessed at: the convention is enforced in review,
//! and this guard covers the case that actually recurs.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

/// Only the console is in scope. `frontends/packages/ui/**` is the dumb-
/// component library — classes are precisely what belongs there — and
/// `frontends/packages/{api,gateway,env}` render nothing at all.
const ROOT: &str = "frontends/apps/admin/app";

/// Directory name marking route-local dumb components. R6's layer table
/// puts them at `frontends/apps/admin/app/<route>/components/**`, so
/// anything below such a directory is exempt from every check here — that
/// is the layer classes are supposed to end up in.
const DUMB_DIR: &str = "components";

/// Files under `app/api/**` are route handlers, not React views: they
/// export `GET`/`POST` and never render. Scanning them would be noise.
const API_DIR: &str = "api";

/// What makes a file a *view* — the two layers R6 forbids classes in.
/// A page is `page.tsx`/`layout.tsx`; a smart component is
/// `<name>-screen.tsx`. Anything else under `app/` (a `*.ts` pure module,
/// a route-local dumb component) is not a view.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Page,
    Smart,
}

impl Layer {
    fn classify(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        match name {
            "page.tsx" | "layout.tsx" => Some(Self::Page),
            n if n.ends_with("-screen.tsx") => Some(Self::Smart),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Smart => "smart component",
        }
    }
}

/// `className=` (JSX attribute) or a `cn(` call. Both are the same
/// violation — `cn(...)` is only ever used to build a class string.
///
/// `cn` is matched with a leading non-word boundary so it cannot fire on an
/// identifier that merely ends in those letters (`fn(`, `dyn(`, a variable
/// named `wcn`). Written as an explicit alternation rather than `\b`
/// because `\b` before `c` would still match inside `fn(`… no: it would
/// not, but the explicit form documents the intent and survives someone
/// widening the alternation later.
fn class_pattern() -> Regex {
    Regex::new(r"(className\s*=|(?:^|[^A-Za-z0-9_$])cn\s*\()")
        .expect("pattern is a fixed, valid regex")
}

/// A Tailwind responsive/state variant inside a string literal — `sm:`,
/// `md:`, `lg:`, `xl:`, `2xl:`, `hover:`, `focus:`, `dark:` followed by a
/// utility-shaped token.
///
/// This is the check that catches an exported class const, which
/// [`class_pattern`] cannot see. The `:` immediately followed by a
/// lowercase utility token is what makes it safe: an English sentence in a
/// string ("Delete this route: are you sure?") has a space after the colon,
/// and a URL (`https://`) has a slash. Verified against the real console in
/// the test below.
fn class_literal_pattern() -> Regex {
    Regex::new(r"(?:sm|md|lg|xl|2xl|hover|focus|active|dark|group-hover):[a-z][a-z0-9-]*")
        .expect("pattern is a fixed, valid regex")
}

/// Raw HTML markup — forbidden in a smart component, which per R6's layer
/// table may render dumb components and nothing else.
///
/// Deliberately does **not** include every HTML tag: this is the set that
/// actually appears in these files today plus the obvious structural
/// neighbours. A smart component reaching for `<article>` is a violation
/// too, but a partial list that never false-fires is worth more than an
/// exhaustive one that has to guess whether `<Foo>` is a component.
/// Capitalised tags are components, which are allowed — the lowercase-only
/// character class is what distinguishes them.
fn markup_pattern() -> Regex {
    Regex::new(
        r"<(div|span|table|thead|tbody|tr|td|th|ul|ol|li|p|section|header|footer|nav|form|label|input|button|h1|h2|h3|h4|h5|h6)[\s/>]",
    )
    .expect("pattern is a fixed, valid regex")
}

/// Informational only — never a failure. See the module doc.
fn use_state_pattern() -> Regex {
    Regex::new(r"\buseState\s*[(<]").expect("pattern is a fixed, valid regex")
}

struct Violation {
    file: String,
    line: usize,
    text: String,
    rule: &'static str,
}

pub fn run(root: &Path) -> Result<(), String> {
    let scan_root = root.join(ROOT);
    if !scan_root.is_dir() {
        println!("no {ROOT} yet — R6 lint vacuously passes");
        return Ok(());
    }

    let class_re = class_pattern();
    let literal_re = class_literal_pattern();
    let markup_re = markup_pattern();
    let use_state_re = use_state_pattern();

    let mut violations: Vec<Violation> = Vec::new();
    let mut views = 0usize;
    let mut use_state_files: Vec<String> = Vec::new();

    for file in view_sources(&scan_root) {
        let Some(layer) = Layer::classify(&file) else {
            continue;
        };
        let rel = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        views += 1;

        let mut saw_use_state = false;
        for (index, line) in text.lines().enumerate() {
            // A `//`-prefixed line is documentation. This repo's convention
            // is long explanatory comments in exactly these files (the
            // drawer bug writeup in `gallery/page.tsx` runs ~95 lines and
            // quotes class names), and failing on prose would make the
            // guard hostile to the thing that makes this codebase legible.
            if line.trim_start().starts_with("//") || line.trim_start().starts_with('*') {
                continue;
            }
            let lineno = index + 1;
            if class_re.is_match(line) {
                violations.push(Violation {
                    file: rel.clone(),
                    line: lineno,
                    text: line.trim().to_owned(),
                    rule: "CSS classes in a view file",
                });
            }
            if literal_re.is_match(line) {
                violations.push(Violation {
                    file: rel.clone(),
                    line: lineno,
                    text: line.trim().to_owned(),
                    rule: "class string literal in a view file",
                });
            }
            if layer == Layer::Smart && markup_re.is_match(line) {
                violations.push(Violation {
                    file: rel.clone(),
                    line: lineno,
                    text: line.trim().to_owned(),
                    rule: "raw HTML markup in a smart component",
                });
            }
            if use_state_re.is_match(line) {
                saw_use_state = true;
            }
        }
        if saw_use_state {
            use_state_files.push(format!("{rel} ({})", layer.label()));
        }
    }

    if !use_state_files.is_empty() {
        println!(
            "R6 note — useState in {} view file(s):",
            use_state_files.len()
        );
        for file in &use_state_files {
            println!("  {file}");
        }
        println!(
            "  Not a failure. R6 permits ephemeral single-value presentational state;\n  \
             anything else needs a sentence in the PR saying why nuqs / react-query /\n  \
             react-hook-form / useRef / useReducer did not fit."
        );
        println!();
    }

    if violations.is_empty() {
        println!("R6 OK ({views} view files scanned under {ROOT})");
        return Ok(());
    }

    // Two numbers, because one alone misleads. A single line can break more
    // than one rule (`<div className="…">` is both raw markup in a smart
    // component and a class in a view file), so the finding count runs well
    // ahead of the number of lines a person actually has to touch. Report
    // both rather than letting the bigger one stand in for the work.
    let mut lines: Vec<(&str, usize)> = violations
        .iter()
        .map(|v| (v.file.as_str(), v.line))
        .collect();
    lines.sort_unstable();
    lines.dedup();
    let mut files: Vec<&str> = violations.iter().map(|v| v.file.as_str()).collect();
    files.sort_unstable();
    files.dedup();

    eprintln!(
        "R6 violation — {} finding(s) on {} line(s) across {} of {views} view file(s):",
        violations.len(),
        lines.len(),
        files.len()
    );
    eprintln!();
    for v in &violations {
        eprintln!("{}:{}: {}", v.file, v.line, v.rule);
        eprintln!("    {}", truncate(&v.text, 100));
    }
    eprintln!();
    eprintln!("R6: pages compose, smart components decide, dumb components style.");
    eprintln!("Classes belong in dumb components — frontends/packages/ui/src/components/**");
    eprintln!("(shared) or frontends/apps/admin/app/<route>/components/** (route-local).");
    eprintln!("See AGENTS.md's R6 section for the layer table.");
    Err(format!("R6 violation ({} findings)", violations.len()))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

/// Every `*.tsx` under `dir`, recursively, skipping dumb-component
/// directories and API route handlers. `node_modules`/`.next` are excluded
/// by name — unlike the Rust side, a JS app really does carry nested build
/// output, so a plain walk is not sufficient here.
fn view_sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == DUMB_DIR || name == API_DIR || name == "node_modules" || name == ".next" {
                continue;
            }
            out.extend(view_sources(&path));
        } else if path.extension().is_some_and(|e| e == "tsx") {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_pattern_catches_the_real_shapes() {
        let re = class_pattern();
        assert!(re.is_match(r#"<div className="flex gap-2">"#));
        assert!(re.is_match(r#"className={cn("a", b)}"#));
        assert!(re.is_match(r"  className={styles.row}"));
        assert!(re.is_match(r#"const c = cn("a", "b");"#));
    }

    #[test]
    fn class_pattern_does_not_fire_on_lookalikes() {
        let re = class_pattern();
        // An identifier merely ending in `cn`.
        assert!(!re.is_match("const wcn = compute(1);"));
        // A function call that is not `cn`.
        assert!(!re.is_match("return fn(value);"));
        // The word in prose, with no `=` and no `(`.
        assert!(!re.is_match("// the className goes on the dumb component"));
    }

    /// The precise case R6's rule text quotes verbatim from
    /// `jobs-screen.tsx` as "corrective, not hypothetical".
    #[test]
    fn class_literal_pattern_catches_a_hoisted_column_const() {
        let re = class_literal_pattern();
        assert!(re.is_match(r#"const COL_ID = "hidden lg:table-cell";"#));
        assert!(re.is_match(r#"const COL_ATTEMPTS = "hidden sm:table-cell";"#));
        assert!(re.is_match(r#"  active: "bg-success/10 hover:bg-success/20","#));
    }

    /// The false positives that would get this guard deleted. A colon
    /// followed by a space (prose) or a slash (URL) must never match.
    #[test]
    fn class_literal_pattern_does_not_fire_on_prose_or_urls() {
        let re = class_literal_pattern();
        assert!(!re.is_match(r#"title="Delete this route: are you sure?""#));
        assert!(!re.is_match(r#"const DOCS = "https://cratestack.dev/";"#));
        assert!(!re.is_match(r"// Note: the md file explains this"));
        assert!(!re.is_match(r#"description="Rotate: the old secret stays valid""#));
        // A time literal, which has digits after the colon.
        assert!(!re.is_match(r#"const AT = "runs at 15:04 daily";"#));
    }

    #[test]
    fn markup_pattern_separates_html_from_components() {
        let re = markup_pattern();
        assert!(re.is_match("  <div>"));
        assert!(re.is_match(r#"<table className="table">"#));
        assert!(re.is_match("<span />"));
        // Components are allowed in a smart component — that is the whole
        // point of the layer.
        assert!(!re.is_match("<DataTable rows={rows} />"));
        assert!(!re.is_match("<StatusPill state={state} />"));
        // A comparison operator, not a tag.
        assert!(!re.is_match("if (a < divisor) {"));
    }

    #[test]
    fn layer_classification_matches_r6s_own_table() {
        assert!(matches!(
            Layer::classify(Path::new("app/jobs/page.tsx")),
            Some(Layer::Page)
        ));
        assert!(matches!(
            Layer::classify(Path::new("app/layout.tsx")),
            Some(Layer::Page)
        ));
        assert!(matches!(
            Layer::classify(Path::new("app/jobs/jobs-screen.tsx")),
            Some(Layer::Smart)
        ));
        // A route-local dumb component is not a view.
        assert!(Layer::classify(Path::new("app/jobs/components/job-table.tsx")).is_none());
        // A pure module is not a view.
        assert!(Layer::classify(Path::new("app/messages/[id]/timeline.ts")).is_none());
    }
}
