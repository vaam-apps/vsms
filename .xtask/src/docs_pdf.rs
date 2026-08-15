//! `cargo xtask docs-pdf` — merges this repository's documentation into one
//! PDF book, via `pandoc` + `Typst`, both running inside a single pinned
//! container image. Never installed on the host — the maintainer's
//! standing "containerize-tooling" direction (`AGENTS.md`) applies here
//! exactly as it does to `sms-migrate`, `deploy/backup-tool`, and every
//! `just demo` service: the tool a developer needs is a `docker`
//! invocation, not a package they install locally.
//!
//! # Why Typst, and why this image
//!
//! Typst was the brief. Two engine choices were live options and both were
//! rejected before writing any code: `asciidoctor-pdf` needs a Ruby/JVM
//! toolchain and produces a noticeably different visual system than the
//! rest of this evaluation asked for; a LaTeX engine (`tectonic`,
//! `texlive`) is the heaviest possible container image for the smallest
//! marginal benefit over Typst's own native PDF backend, which needs no
//! second document-processing system at all.
//!
//! Typst itself has no official Docker image — checked directly against
//! Docker Hub's search API and the `typst/typst` name (unclaimed), not
//! assumed. What does exist, and is what this module pins, is
//! `pandoc/typst` — a real, first-party image from
//! <https://github.com/pandoc/dockerfiles> (the same organisation that
//! publishes `pandoc/core`/`pandoc/minimal`, maintained by pandoc's own
//! lead maintainer, per that image's own `org.opencontainers.image.*`
//! labels) that bundles **both** `pandoc` and `typst` in one Alpine-based
//! image. That is a better fit than hand-rolling a Dockerfile that layers
//! a separately-sourced Typst release binary onto `pandoc/minimal`: one
//! upstream-maintained image, one digest to pin, no second supply chain
//! this repository would otherwise have to verify by hand.
//!
//! [`IMAGE`] pins that image **by digest**, not by the mutable `latest` or
//! `3.10.0.0-alpine` tag — a tag can be repointed at a different digest by
//! its publisher at any time; a digest cannot. Confirmed live at the time
//! of writing: `docker run --rm --entrypoint sh <digest> -c 'pandoc
//! --version; typst --version'` reports `pandoc 3.10` / `typst 0.14.2`.
//! Bumping this pin is a one-line change plus a rerun of this module's own
//! verification (regenerate the PDF, confirm the page count and a
//! known-distinctive phrase from `docs/architecture.md` are still present)
//! — the same discipline `AGENTS.md`'s own "cratestack bumped" sections
//! apply to every other pinned dependency in this repository.
//!
//! # Markdown and `AsciiDoc`, in the same book
//!
//! `docs/runbooks/*.md` is mid-migration to `.adoc` (a separate, concurrent
//! effort) at the time this module was written — so [`discover_inputs`]
//! accepts either extension for every file it looks for, and a directory
//! scan (`docs/design/`, `docs/legal/`, `docs/runbooks/`) picks up whatever
//! mix of `.md`/`.adoc` happens to be on disk without needing an edit here.
//! `pandoc` reads both natively (`-f markdown` / `-f asciidoc`) and writes
//! both to the same Typst output dialect, so the merge needs no format
//! normalisation pass of its own.
//!
//! One real asymmetry between the two was found live, not assumed, and
//! [`extract_title`]/[`write_fragment`] exist because of it: `AsciiDoc`'s
//! leading `= Title` line is the document's **title** (metadata pandoc
//! lifts out of the body entirely, the same way a Markdown file's YAML
//! front matter would be), not a body heading — confirmed by converting a
//! synthetic `.adoc` fixture and observing the title line simply absent
//! from `pandoc -t typst`'s body-only output. Markdown has no equivalent
//! title/body split (no YAML front matter is used anywhere in this
//! repository's docs), so a leading `# Title` line stays exactly where it
//! is, as the first heading of the body — confirmed the same way, against
//! `docs/architecture.md`. Rather than lean on pandoc's own metadata
//! export (which would mean parsing pandoc's JSON AST for one field), this
//! module reads each source file's own first non-blank line itself and,
//! **only** for an `AsciiDoc` source, re-emits it as a Typst heading ahead of
//! the converted body — restoring parity with what a Markdown source gets
//! for free, without asking pandoc to round-trip through a second format.
//!
//! # Body-only output needs its own preamble, once
//!
//! `pandoc -t typst` without `--standalone` — deliberate, so every chapter
//! can share one page/heading/outline sequence via Typst's own `#include`
//! rather than each carrying a competing one — omits the handful of
//! `#let`/`#show`/`#set` bindings pandoc's *own* default template normally
//! splices in per file. `docs/pdf/templates/helpers.typ` is that preamble,
//! extracted verbatim from `pandoc -D typst`'s static portion and imported
//! by every generated fragment; see that file's own header comment for the
//! full reasoning and the exact failure it fixes.
//!
//! # What this does not (yet) do
//!
//! Rust API docs via `cargo doc --output-format json` are explicitly out of
//! scope for this change, not silently dropped: `--output-format json` is
//! itself an unstable, nightly-only rustdoc flag as of this writing, and
//! turning its output into readable Typst content is a real conversion
//! problem (the JSON is an internal, semver-unstable representation of the
//! whole rustdoc IR, not prose) — enough of one that bolting it on here
//! would be the kind of drive-by this repository's own conventions warn
//! against elsewhere. If it lands, it belongs as a further named input
//! producing its own chapter(s) in [`discover_inputs`], appended after the
//! hand-written docs, with its own conversion step next to
//! [`write_fragment`] — not a rewrite of this module's structure.
//!
//! `AGENTS.md`/`CLAUDE.md` are deliberately never discovered here at all —
//! not filtered out, simply never named. They are institutional memory for
//! agents working *on* this repository, not the operator/integrator-facing
//! documentation this book exists to hand someone.
//!
//! # Cross-document links, found live on the first real run
//!
//! Every source doc here links to others — `xref:getting-started.adoc[...]`
//! between sibling runbooks, `[...](docs/architecture.md)` from
//! `CONTRIBUTING.md`, and so on. Pandoc has no idea this book merges many
//! files into one: from its own perspective it converts exactly one file at
//! a time, so a link target with no recognised URL scheme (`http`,
//! `mailto`, ...) is emitted as a same-document Typst label reference —
//! `#link(<getting-started.adoc>)[...]` — on the assumption that the target
//! names an anchor *within this file*. Merged into one book, that label
//! never exists (each fragment is its own Typst module scope), and Typst
//! hard-errors the whole compile on an unresolved reference. This was found
//! by actually running the merge against the real, fully-migrated
//! `docs/runbooks/*.adoc` — not anticipated in advance.
//!
//! [`resolve_cross_document_links`] fixes this in two steps: every
//! fragment gets an invisible, book-unique anchor
//! (`#metadata(none) <chapter-NN>`) at its very top ([`chapter_label`]);
//! and [`register_reference_keys`] populates a lookup table from every
//! plausible way another document in this book might reference a given
//! one — its bare filename, its repo-relative path, that path with the
//! `docs/` prefix stripped (for a sibling under `docs/` linking
//! `runbooks/x.md`), and *both* `.md`/`.adoc` spellings of each of those
//! (since a stale link can still say `.md` for a file the runbooks
//! migration already renamed to `.adoc`, and vice versa). A link whose
//! target resolves gets rewritten to point at that chapter's own anchor —
//! a real, working intra-book jump, not merely a non-crashing one. A link
//! that doesn't resolve (the two genuinely out-of-book targets in this
//! corpus, `../examples/README.md` and `../sdks/rust/vsms-sdk-rust/README.md`,
//! plus any link carrying a `#sub-heading` fragment this module doesn't
//! attempt to resolve to that precise position) has its `#link(...)`
//! wrapper stripped, keeping the link's own text as plain content rather
//! than losing it — degraded, not dropped, and never a reason to fail the
//! build. A label with no recognisable document extension (`<12-milestones>`
//! is left completely untouched: pandoc's own auto-generated heading labels
//! are real, valid, same-fragment anchors and must not be touched.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `pandoc/typst`, pinned by digest — see the module doc for why this
/// image and why a digest rather than a tag. Tag recorded in the trailing
/// comment purely for a human cross-reference on Docker Hub; only the
/// digest is ever what's pulled or run.
const IMAGE: &str =
    "pandoc/typst@sha256:9eecb00186f3f108b8d3bda5171a3b4ba5dd991d80a78e489e73512ce3b3096e"; // 3.10.0.0-alpine (pandoc 3.10, typst 0.14.2)

/// Output directory, repo-root-relative — an ordinary build artifact
/// directory under `/target`, already covered by the root `.gitignore`
/// entry the same way every other workspace member's build output is.
const OUT_DIR: &str = "target/docs-pdf";

/// Final merged PDF's filename, inside [`OUT_DIR`].
const PDF_NAME: &str = "vsms-docs.pdf";

/// The static Typst preamble every generated fragment imports — see its
/// own header comment for why it has to exist at all.
const HELPERS_SRC: &str = "docs/pdf/templates/helpers.typ";

/// A source document format pandoc can read natively, mapped straight onto
/// its `-f` flag value. Deliberately not a richer enum — this module only
/// ever needs to know "which pandoc reader" and "does this format strip
/// its own leading title line into metadata" ([`Format::extracts_title`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Markdown,
    AsciiDoc,
}

impl Format {
    fn from_extension(path: &Path) -> Result<Self, String> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("md") => Ok(Self::Markdown),
            Some("adoc" | "asciidoc") => Ok(Self::AsciiDoc),
            other => Err(format!(
                "{}: unsupported extension {other:?} (expected .md or .adoc)",
                path.display()
            )),
        }
    }

    /// The pandoc `-f`/`-t` reader name for this format.
    fn pandoc_reader(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::AsciiDoc => "asciidoc",
        }
    }

    /// The line prefix that marks a leading document title in this format
    /// — see the module doc's "Markdown and `AsciiDoc`" section. `None`
    /// means this format keeps its own title in the body already, so no
    /// re-injection is needed.
    fn title_marker(self) -> Option<&'static str> {
        match self {
            Self::Markdown => None,
            Self::AsciiDoc => Some("= "),
        }
    }
}

/// One document to include in the book, already resolved to a concrete
/// file on disk.
struct DocInput {
    /// Repo-root-relative, forward-slash-separated — both pandoc's input
    /// argument (resolved inside the container against the read-only
    /// `/repo` mount) and this module's own log/error output.
    rel_path: String,
    abs_path: PathBuf,
    format: Format,
}

pub fn run(root: &Path) -> Result<(), String> {
    ensure_docker_available()?;

    let inputs = discover_inputs(root)?;
    if inputs.is_empty() {
        return Err("docs-pdf: no input documents discovered — nothing to build".to_owned());
    }

    let out_dir = root.join(OUT_DIR);
    let fragments_dir = out_dir.join("fragments");
    // A clean slate every run: a source doc renamed or removed since the
    // last build must not leave a stale fragment behind for main.typ to
    // silently stop referencing (harmless) or a leftover file to shadow a
    // real one (not harmless). Rebuilding is cheap enough that caching
    // isn't worth the staleness risk.
    if out_dir.is_dir() {
        fs::remove_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    }
    fs::create_dir_all(&fragments_dir).map_err(|e| format!("{}: {e}", fragments_dir.display()))?;

    let helpers_src = root.join(HELPERS_SRC);
    let helpers_text =
        fs::read_to_string(&helpers_src).map_err(|e| format!("{}: {e}", helpers_src.display()))?;
    fs::write(out_dir.join("helpers.typ"), helpers_text)
        .map_err(|e| format!("{}: {e}", out_dir.join("helpers.typ").display()))?;

    // First pass: every document gets a stable, book-unique anchor label
    // before any conversion happens, so the second pass can resolve a link
    // from document A to document B regardless of which order they convert
    // in. See the module doc's "Cross-document links" section.
    let mut resolve: HashMap<String, String> = HashMap::new();
    for (i, doc) in inputs.iter().enumerate() {
        register_reference_keys(&mut resolve, &doc.rel_path, &chapter_label(i));
    }

    println!(
        "docs-pdf: converting {} documents via {IMAGE}",
        inputs.len()
    );
    let mut includes: Vec<PathBuf> = Vec::with_capacity(inputs.len());
    for (i, doc) in inputs.iter().enumerate() {
        println!("  [{:>2}/{}] {}", i + 1, inputs.len(), doc.rel_path);
        includes.push(write_fragment(root, &out_dir, doc, i, &resolve)?);
    }

    write_main_typ(&out_dir, &includes)?;
    let pdf_path = compile_pdf(&out_dir)?;

    let size = fs::metadata(&pdf_path)
        .map_err(|e| format!("{}: {e}", pdf_path.display()))?
        .len();
    println!("docs-pdf: wrote {} ({size} bytes)", pdf_path.display());
    Ok(())
}

fn ensure_docker_available() -> Result<(), String> {
    let status = Command::new("docker")
        .arg("--version")
        .status()
        .map_err(|e| {
            format!(
                "docs-pdf needs `docker` — pandoc and typst both run inside a pinned container, \
             never installed on the host (see this module's own doc comment). Could not run \
             `docker --version`: {e}"
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`docker --version` exited with {status}"))
    }
}

/// The book's chapter order. Explicit, not a single recursive walk of
/// `docs/`, on purpose: `AGENTS.md`/`CLAUDE.md` must never appear (see the
/// module doc), and a handful of standalone files (`README.md`,
/// `docs/architecture.md`, ...) belong in specific, named positions rather
/// than wherever a directory listing would put them. `docs/design/`,
/// `docs/legal/`, and `docs/runbooks/` are still discovered by directory
/// scan — a new file dropped into one of those needs no edit here to be
/// picked up, and the runbooks scan accepts both `.md` and `.adoc` so the
/// in-flight migration named in the module doc needs no edit here either.
fn discover_inputs(root: &Path) -> Result<Vec<DocInput>, String> {
    let mut out = Vec::new();

    out.push(find_named(root, ".", "README")?);
    out.push(find_named(root, "docs", "architecture")?);
    out.extend(scan_dir(root, "docs/design", &[])?);
    out.extend(scan_dir(root, "docs/legal", &[])?);
    out.push(find_named(root, "docs", "integrating")?);
    out.push(find_named(root, "docs", "roadmap")?);

    // Runbooks: the directory's own README first (it is the reader's map
    // of the rest), then everything else in the directory, alphabetically,
    // whichever of `.md`/`.adoc` each one happens to be on disk.
    let runbooks_readme = find_named(root, "docs/runbooks", "README")?;
    let readme_rel = runbooks_readme.rel_path.clone();
    out.push(runbooks_readme);
    out.extend(scan_dir(root, "docs/runbooks", &[readme_rel])?);

    out.push(find_named(root, ".", "CONTRIBUTING")?);
    out.push(find_named(root, ".", "OPEN_QUESTIONS")?);

    Ok(out)
}

/// Resolves `<dir>/<stem>.md` or `<dir>/<stem>.adoc`, whichever exists —
/// `dir == "."` for a repo-root file. Errors loudly if neither does,
/// rather than silently omitting a named chapter: a missing input here
/// means either this module's own file list drifted from the repository,
/// or a rename landed without updating it — the same "a missing thing
/// should fail loudly, not vanish" standard the rest of this repository's
/// own `xtask` checks already hold to.
fn find_named(root: &Path, dir: &str, stem: &str) -> Result<DocInput, String> {
    for ext in ["md", "adoc"] {
        let candidate = root.join(dir).join(format!("{stem}.{ext}"));
        if candidate.is_file() {
            let format = Format::from_extension(&candidate)?;
            return Ok(DocInput {
                rel_path: rel(root, &candidate),
                abs_path: candidate,
                format,
            });
        }
    }
    Err(format!(
        "docs-pdf: neither {dir}/{stem}.md nor {dir}/{stem}.adoc exists"
    ))
}

/// Every `.md`/`.adoc` file directly inside `<root>/<dir>`, sorted by file
/// name, excluding any repo-root-relative path in `exclude`.
fn scan_dir(root: &Path, dir: &str, exclude: &[String]) -> Result<Vec<DocInput>, String> {
    let abs_dir = root.join(dir);
    let entries = fs::read_dir(&abs_dir).map_err(|e| format!("{}: {e}", abs_dir.display()))?;

    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", abs_dir.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if Format::from_extension(&path).is_err() {
            continue; // not a .md/.adoc file — e.g. a stray non-doc file
        }
        paths.push(path);
    }
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let format = Format::from_extension(&path)?;
            Ok(DocInput {
                rel_path: rel(root, &path),
                abs_path: path,
                format,
            })
        })
        .filter(|doc: &Result<DocInput, String>| match doc {
            Ok(d) => !exclude.contains(&d.rel_path),
            Err(_) => true,
        })
        .collect()
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The first non-blank line of `text`, stripped of `format`'s title
/// marker — `None` if the format has no such marker, or the first
/// non-blank line doesn't carry one.
fn extract_title(text: &str, format: Format) -> Option<String> {
    let marker = format.title_marker()?;
    let first = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    first.strip_prefix(marker).map(str::trim).map(str::to_owned)
}

/// Escapes a plain string for interpolation as a Typst **string literal**
/// (`"..."`) — not markup escaping. A string value referenced from content
/// via `#ident` is rendered verbatim, with none of its characters
/// reparsed as markup (confirmed live: a title containing markup-special
/// characters and a literal quote round-tripped unchanged through
/// `= #chapter-title`), so the only characters that need escaping here are
/// the two the *string literal syntax itself* uses: the backslash and the
/// closing quote.
fn typst_string_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A filesystem-safe, human-readable fragment name derived from a
/// repo-relative path — `docs/runbooks/36-handset-gate.adoc` becomes
/// `docs-runbooks-36-handset-gate`. Cosmetic only (main.typ's own
/// generation order is what actually decides chapter order, not this
/// name), kept for a human skimming `target/docs-pdf/fragments/`.
fn slugify(rel_path: &str) -> String {
    let stem = rel_path.rsplit_once('.').map_or(rel_path, |(s, _)| s);
    stem.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Converts one source document to a Typst fragment (via `pandoc`, in the
/// pinned container) and writes it into `<out_dir>/fragments/`, prefixed
/// with this chapter's own anchor label, the shared-helpers import, and —
/// for `AsciiDoc` sources only — a re-injected title heading (see the
/// module doc). Every cross-document link in the converted body is
/// rewritten or stripped per [`resolve_cross_document_links`]. Returns the
/// fragment's path relative to `out_dir`, for `main.typ`'s own `#include`.
fn write_fragment(
    root: &Path,
    out_dir: &Path,
    doc: &DocInput,
    index: usize,
    resolve: &HashMap<String, String>,
) -> Result<PathBuf, String> {
    let source = fs::read_to_string(&doc.abs_path)
        .map_err(|e| format!("{}: {e}", doc.abs_path.display()))?;

    let frag_name = format!("{index:02}-{}.typ", slugify(&doc.rel_path));
    let frag_rel = PathBuf::from("fragments").join(&frag_name);
    let frag_abs = out_dir.join(&frag_rel);

    run_pandoc(root, doc, &frag_abs)?;

    let body = fs::read_to_string(&frag_abs).map_err(|e| format!("{}: {e}", frag_abs.display()))?;
    let body = resolve_cross_document_links(&body, resolve);

    let mut assembled = String::with_capacity(body.len() + 256);
    let _ = writeln!(assembled, "#metadata(none) <{}>", chapter_label(index));
    assembled.push_str("#import \"../helpers.typ\": *\n\n");
    if let Some(title) = extract_title(&source, doc.format) {
        let _ = writeln!(
            assembled,
            "#let chapter-title = \"{}\"\n= #chapter-title\n",
            typst_string_escape(&title)
        );
    }
    assembled.push_str(&body);

    fs::write(&frag_abs, assembled).map_err(|e| format!("{}: {e}", frag_abs.display()))?;
    Ok(frag_rel)
}

/// This chapter's own book-unique, synthetic Typst label — see the module
/// doc's "Cross-document links" section. Not derived from the document's
/// title or slug: a plain index is trivially unique and collision-free,
/// where a title-derived slug would have to somehow also avoid colliding
/// with pandoc's own auto-generated heading labels within the same
/// fragment.
fn chapter_label(index: usize) -> String {
    format!("chapter-{index:02}")
}

/// Registers every plausible way another document in this book might spell
/// a link to `rel_path`, all pointing at the same `label`. See the module
/// doc for the exact set and why each member is needed; `HashMap::entry`
/// means the first document to claim a given spelling wins, which cannot
/// happen in practice here (every registered key is derived from a real,
/// unique repo path) but is a safe, cheap guard against it regardless.
fn register_reference_keys(map: &mut HashMap<String, String>, rel_path: &str, label: &str) {
    let mut bases = vec![rel_path.to_owned()];
    if let Some(stripped) = rel_path.strip_prefix("docs/") {
        bases.push(stripped.to_owned());
    }
    if let Some(basename) = rel_path.rsplit('/').next() {
        bases.push(basename.to_owned());
    }

    for base in bases {
        if let Some(swapped) = swap_doc_extension(&base) {
            map.entry(swapped).or_insert_with(|| label.to_owned());
        }
        map.entry(base).or_insert_with(|| label.to_owned());
    }
}

/// `foo.md` <-> `foo.adoc` (and `foo.asciidoc` -> `foo.md`, for symmetry) —
/// `None` for anything else. Lets a link that still says `.md` resolve
/// against a document the runbooks migration has already renamed to
/// `.adoc`, and vice versa.
fn swap_doc_extension(s: &str) -> Option<String> {
    if let Some(stem) = s.strip_suffix(".md") {
        return Some(format!("{stem}.adoc"));
    }
    let stem = s
        .strip_suffix(".adoc")
        .or_else(|| s.strip_suffix(".asciidoc"))?;
    Some(format!("{stem}.md"))
}

/// A label looks like a cross-document reference — as opposed to one of
/// pandoc's own auto-generated same-fragment heading labels — if it names
/// a document extension, optionally followed by a `#sub-heading` fragment.
/// `<12-milestones>` (no extension) fails this and is left completely
/// untouched; `<architecture.md#12-milestones>` passes it.
fn is_cross_document_label(label: &str) -> bool {
    let file_part = label.split('#').next().unwrap_or(label);
    [".md", ".adoc", ".asciidoc"]
        .iter()
        .any(|ext| file_part.ends_with(ext))
}

/// Rewrites every `#link(<TARGET>)[...]` in `body` whose `TARGET` looks
/// like a cross-document reference (per [`is_cross_document_label`]):
/// resolved against `resolve` (ignoring any `#sub-heading` fragment —
/// this module jumps to the target chapter, not the precise position
/// within it) if possible, otherwise the `#link(...)` wrapper is stripped
/// and only its own display text kept. Every other `#link(...)` — a real
/// URL, or a same-fragment heading reference — passes through byte for
/// byte. A hand-written scanner rather than a regex: the display-text
/// argument can itself contain nested `#emph[...]`/`#strong[...]` calls
/// with their own brackets, which needs balanced-bracket tracking a
/// regular expression cannot express.
fn resolve_cross_document_links(body: &str, resolve: &HashMap<String, String>) -> String {
    const PREFIX: &str = "#link(<";

    let mut out = String::with_capacity(body.len());
    let mut rest = body;

    while let Some(pos) = rest.find(PREFIX) {
        out.push_str(&rest[..pos]);
        let after_prefix = &rest[pos + PREFIX.len()..];

        let Some(gt) = after_prefix.find('>') else {
            // Not the shape we know how to parse — preserve the rest of
            // the file verbatim rather than risk corrupting it.
            out.push_str(&rest[pos..]);
            return out;
        };
        let label = &after_prefix[..gt];
        let after_label = &after_prefix[gt + 1..];

        let Some(after_open_bracket) = after_label.strip_prefix(")[") else {
            // `#link(<label>)` with no following `[...]` — not a shape
            // pandoc's own link output ever produces; pass through
            // unmodified and keep scanning past it.
            out.push_str(PREFIX);
            out.push_str(label);
            out.push('>');
            rest = after_label;
            continue;
        };

        let Some(close) = find_matching_bracket(after_open_bracket) else {
            out.push_str(&rest[pos..]);
            return out;
        };
        let inner = &after_open_bracket[..close];
        rest = &after_open_bracket[close + 1..];

        if !is_cross_document_label(label) {
            let _ = write!(out, "{PREFIX}{label}>)[{inner}]");
            continue;
        }

        let file_part = label.split('#').next().unwrap_or(label);
        match resolve.get(file_part) {
            Some(target_label) => {
                let _ = write!(out, "{PREFIX}{target_label}>)[{inner}]");
            }
            None => out.push_str(inner),
        }
    }

    out.push_str(rest);
    out
}

/// Given the text immediately after an opening `[`, finds the byte offset
/// of its matching `]` — tracking nested `[`/`]` depth so a nested
/// `#emph[...]`/`#strong[...]` call's own brackets don't terminate the
/// scan early.
fn find_matching_bracket(s: &str) -> Option<usize> {
    let mut depth = 1i32;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Runs `pandoc <doc> -f <reader> -t typst -o <out>` inside [`IMAGE`]. The
/// repo root is mounted read-only at `/repo` (pandoc never needs to write
/// there — it only reads the one named input file); the fragments
/// directory is mounted read-write at `/out`. `--wrap=preserve` keeps
/// pandoc from re-wrapping prose to a fixed column width, so a diff
/// between two regenerations reflects a real content change rather than
/// wrapping-width noise.
fn run_pandoc(root: &Path, doc: &DocInput, frag_abs: &Path) -> Result<(), String> {
    let out_dir = frag_abs
        .parent()
        .ok_or_else(|| format!("{}: has no parent directory", frag_abs.display()))?;
    let frag_file = frag_abs
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{}: not a valid file name", frag_abs.display()))?;

    let status = Command::new("docker")
        .args(["run", "--rm"])
        .arg("-v")
        .arg(format!("{}:/repo:ro", root.display()))
        .arg("-v")
        .arg(format!("{}:/out", out_dir.display()))
        .args(["-w", "/repo", IMAGE])
        .arg(&doc.rel_path)
        .args(["-f", doc.format.pandoc_reader(), "-t", "typst", "-o"])
        .arg(format!("/out/{frag_file}"))
        .arg("--wrap=preserve")
        .status()
        .map_err(|e| format!("failed to run docker/pandoc for {}: {e}", doc.rel_path))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "pandoc failed converting {} (docker exit {status})",
            doc.rel_path
        ))
    }
}

/// Writes `main.typ`: a title page, a table of contents (Typst's
/// `#outline()`, built automatically from every chapter's own headings —
/// no manual TOC bookkeeping), then every fragment in order, each starting
/// on its own page. Heading numbering is deliberately left to each
/// document's own convention (`docs/architecture.md`'s headings already
/// read "2.5", "3.3", ...) rather than a Typst `#set heading(numbering:
/// ...)` on top of it, which would double-number every chapter.
fn write_main_typ(out_dir: &Path, includes: &[PathBuf]) -> Result<(), String> {
    let mut s = String::new();
    s.push_str("#import \"helpers.typ\": *\n");
    s.push_str("#set document(title: \"vsms — Documentation\")\n");
    s.push_str("#set page(numbering: \"1\", number-align: center)\n");
    s.push_str("#set text(size: 10pt)\n\n");
    s.push_str("#align(center)[\n");
    s.push_str("  #v(4cm)\n");
    s.push_str("  #text(size: 24pt, weight: \"bold\")[vsms]\n");
    s.push_str("  #v(0.5cm)\n");
    s.push_str("  #text(size: 14pt)[Documentation]\n");
    s.push_str("  #v(0.3cm)\n");
    s.push_str(
        "  #text(size: 10pt, fill: gray)[Generated by `cargo xtask docs-pdf` — see AGENTS.md's \"containerize-tooling\" section]\n",
    );
    s.push_str("]\n");
    s.push_str("#pagebreak()\n\n");
    s.push_str("#outline(title: auto, depth: 3)\n");
    s.push_str("#pagebreak()\n\n");

    for include in includes {
        // Forward slashes always — this path is a Typst string literal
        // read by the container's own Linux typst binary, never a native
        // path on whatever host built it.
        let include_str = include.to_string_lossy().replace('\\', "/");
        let _ = writeln!(s, "#include \"{include_str}\"\n#pagebreak(weak: true)\n");
    }

    let main_path = out_dir.join("main.typ");
    fs::write(&main_path, s).map_err(|e| format!("{}: {e}", main_path.display()))
}

/// Runs `typst compile main.typ <PDF_NAME>` inside [`IMAGE`], with
/// `out_dir` mounted read-write at `/data` — `main.typ`, `helpers.typ` and
/// every fragment all live under it already, so this is the one step that
/// needs no `/repo` mount at all.
fn compile_pdf(out_dir: &Path) -> Result<PathBuf, String> {
    let status = Command::new("docker")
        .args(["run", "--rm"])
        .arg("-v")
        .arg(format!("{}:/data", out_dir.display()))
        .args([
            "-w",
            "/data",
            "--entrypoint",
            "typst",
            IMAGE,
            "compile",
            "main.typ",
            PDF_NAME,
        ])
        .status()
        .map_err(|e| format!("failed to run docker/typst: {e}"))?;

    if status.success() {
        Ok(out_dir.join(PDF_NAME))
    } else {
        Err(format!("typst compile failed (docker exit {status})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_has_no_title_marker() {
        assert_eq!(Format::Markdown.title_marker(), None);
    }

    #[test]
    fn asciidoc_title_is_extracted_and_stripped() {
        let text = "\n= My Runbook\nSome Author\n\nBody text.\n";
        assert_eq!(
            extract_title(text, Format::AsciiDoc).as_deref(),
            Some("My Runbook")
        );
    }

    #[test]
    fn markdown_h1_is_never_extracted_as_a_title() {
        // Markdown keeps its H1 in the pandoc body output already (see the
        // module doc) — extracting and re-injecting it here would
        // duplicate it.
        let text = "# My Doc\n\nBody text.\n";
        assert_eq!(extract_title(text, Format::Markdown), None);
    }

    #[test]
    fn asciidoc_with_no_leading_title_extracts_nothing() {
        let text = "Just a paragraph, no doctitle.\n";
        assert_eq!(extract_title(text, Format::AsciiDoc), None);
    }

    #[test]
    fn string_escape_covers_backslash_and_quote_only() {
        // Markup-special characters (`*_#$@[]` etc.) must survive
        // untouched — they are never reparsed as markup when a *string*
        // value is interpolated into content (verified live; see the
        // module doc). Only the string-literal syntax's own two special
        // characters need escaping here.
        assert_eq!(
            typst_string_escape(r#"a * b _ c # d "quoted" e\f"#),
            r#"a * b _ c # d \"quoted\" e\\f"#
        );
    }

    #[test]
    fn slugify_lowercases_and_replaces_separators() {
        assert_eq!(
            slugify("docs/runbooks/36-handset-gate.adoc"),
            "docs-runbooks-36-handset-gate"
        );
    }

    #[test]
    fn format_from_extension_accepts_both_adoc_spellings() {
        assert_eq!(
            Format::from_extension(Path::new("x.adoc")).unwrap(),
            Format::AsciiDoc
        );
        assert_eq!(
            Format::from_extension(Path::new("x.asciidoc")).unwrap(),
            Format::AsciiDoc
        );
        assert_eq!(
            Format::from_extension(Path::new("x.md")).unwrap(),
            Format::Markdown
        );
        assert!(Format::from_extension(Path::new("x.txt")).is_err());
    }

    #[test]
    fn register_reference_keys_covers_bare_stripped_and_swapped_extension_forms() {
        let mut map = HashMap::new();
        register_reference_keys(&mut map, "docs/runbooks/getting-started.adoc", "chapter-07");

        for key in [
            "docs/runbooks/getting-started.adoc",
            "docs/runbooks/getting-started.adoc",
            "runbooks/getting-started.adoc",
            "runbooks/getting-started.adoc",
            "getting-started.adoc",
            "getting-started.md",
        ] {
            assert_eq!(
                map.get(key).map(String::as_str),
                Some("chapter-07"),
                "{key}"
            );
        }
    }

    #[test]
    fn cross_document_label_detection_ignores_same_fragment_heading_anchors() {
        assert!(is_cross_document_label("architecture.md"));
        assert!(is_cross_document_label(
            "docs/architecture.md#12-milestones"
        ));
        assert!(is_cross_document_label("getting-started.adoc"));
        assert!(!is_cross_document_label("12-milestones"));
        assert!(!is_cross_document_label("sms-gateway-architecture-design"));
    }

    #[test]
    fn resolvable_cross_document_link_is_rewritten_to_the_target_chapter() {
        let mut resolve = HashMap::new();
        resolve.insert("getting-started.adoc".to_owned(), "chapter-07".to_owned());

        let body = "See #link(<getting-started.adoc>)[Getting started] for setup.";
        let out = resolve_cross_document_links(body, &resolve);
        assert_eq!(out, "See #link(<chapter-07>)[Getting started] for setup.");
    }

    #[test]
    fn unresolvable_cross_document_link_is_stripped_but_text_survives() {
        let resolve = HashMap::new(); // nothing registered — genuinely out of book
        let body = "See #link(<../examples/README.md>)[the examples] for more.";
        let out = resolve_cross_document_links(body, &resolve);
        assert_eq!(out, "See the examples for more.");
    }

    #[test]
    fn same_fragment_heading_link_passes_through_untouched() {
        let resolve = HashMap::new();
        let body = "See #link(<12-milestones>)[the milestones section] above.";
        let out = resolve_cross_document_links(body, &resolve);
        assert_eq!(out, body);
    }

    #[test]
    fn link_text_with_nested_strong_is_not_truncated_at_the_inner_bracket() {
        let mut resolve = HashMap::new();
        resolve.insert("architecture.md".to_owned(), "chapter-01".to_owned());
        let body = "#link(<architecture.md>)[#strong[architecture.md]] is the spec.";
        let out = resolve_cross_document_links(body, &resolve);
        assert_eq!(
            out,
            "#link(<chapter-01>)[#strong[architecture.md]] is the spec."
        );
    }

    #[test]
    fn a_real_url_link_is_never_touched() {
        let resolve = HashMap::new();
        let body = "#link(\"https://example.com\")[Example] and text.";
        assert_eq!(resolve_cross_document_links(body, &resolve), body);
    }
}
