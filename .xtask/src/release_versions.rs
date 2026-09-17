//! Every version release-please is responsible for must already agree, and
//! every line it must rewrite must still carry its annotation.
//!
//! # Why this exists
//!
//! `release.yml`'s own `version` job already compares the git tag against
//! the three manifest versions — but only `if:
//! startsWith(github.ref, 'refs/tags/')`. So it first runs *after* the
//! release pull request has merged and the tag exists, which is the same
//! shape [`crate::workflow_paths`]'s module doc is about: a check that
//! cannot run before merge is not a check. This one runs on every pull
//! request.
//!
//! # The failure it is actually built for
//!
//! release-please rewrites a line only if that line carries an
//! `x-release-please-version` comment (`src/updaters/generic.ts`: the
//! annotation must be on the same line as the version, and only the first
//! semver-shaped substring on it is replaced). Twenty lines carry one
//! today, eighteen of them `image:` defaults in the two compose files.
//!
//! Adding a nineteenth service — or reformatting an existing line so the
//! comment moves off it — does not break anything visibly. The release PR
//! is still opened, still green, still merges. The compose default for
//! that one service simply stays at the previous release forever, and the
//! first symptom is `docker compose pull` fetching a stale image in
//! somebody's deployment. Nothing in `cargo check`, `cargo test` or
//! `ci.yml` can see it.
//!
//! So this check asserts three things:
//!
//! 1. every version release-please owns is the same version;
//! 2. every file `release-please-config.json` points at still contains an
//!    annotation to act on;
//! 3. every versioned vsms image default in the compose files is
//!    annotated — the direction that catches a newly added service.
//!
//! # Deliberately not checked
//!
//! An annotated line in a file the config does *not* list. Catching that
//! needs a whole-tree walk, and the failure needs someone to write an
//! annotation while never touching the config — far less likely than
//! (3), which happens by simply adding a service. Named rather than left
//! to look like an oversight.
//!
//! The lockfiles are not checked here either: `ci.yml` already runs
//! `cargo metadata --locked` (root) and `cargo check --locked` (each
//! excluded manifest), which is a stronger check than reading a version
//! out of them, and `release-please.yml` refreshes them on the release
//! branch.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use regex::Regex;

const CONFIG: &str = "release-please-config.json";
const MANIFEST: &str = ".release-please-manifest.json";
const ANNOTATION: &str = "x-release-please-version";

/// Where a version was found, and what it said.
type Findings = BTreeMap<String, String>;

pub fn run(root: &Path) -> Result<(), String> {
    let config = read(root, CONFIG)?;
    let (generic_paths, json_path) = extra_files(&config)?;

    let mut found: Findings = BTreeMap::new();
    let mut problems: Vec<String> = Vec::new();

    // (1) release-please's own record of the current version.
    match json_string_field(&read(root, MANIFEST)?, ".") {
        Some(v) => {
            found.insert(format!("{MANIFEST} (\".\")"), v);
        }
        None => problems.push(format!("  {MANIFEST}: no \".\" entry")),
    }

    // (2) the one extra file updated by jsonpath rather than by annotation.
    match json_string_field(&read(root, &json_path)?, "version") {
        Some(v) => {
            found.insert(format!("{json_path} ($.version)"), v);
        }
        None => problems.push(format!("  {json_path}: no \"version\" field")),
    }

    // (3) every annotated line in every generic extra file.
    let version_on_line = Regex::new(r"\d+\.\d+\.\d+(-[\w.]+)?").expect("static pattern");
    for rel in &generic_paths {
        let text = read(root, rel)?;
        let mut annotated = 0;
        for (n, line) in text.lines().enumerate() {
            if !line.contains(ANNOTATION) {
                continue;
            }
            annotated += 1;
            match version_on_line.find(line) {
                Some(m) => {
                    found.insert(format!("{rel}:{}", n + 1), m.as_str().to_owned());
                }
                // An annotation with nothing to replace is a silent no-op
                // inside release-please; here it is an error.
                None => problems.push(format!(
                    "  {rel}:{}: carries {ANNOTATION} but no version to replace",
                    n + 1
                )),
            }
        }
        if annotated == 0 {
            problems.push(format!(
                "  {rel}: listed in {CONFIG} extra-files but carries no {ANNOTATION} line, \
                 so release-please will never change it"
            ));
        }
    }

    // (4) the other direction, for compose: a versioned vsms image default
    // that nobody annotated. This is the one that happens by accident.
    let image = Regex::new(
        r"^\s*image:\s*ghcr\.io/\$\{VSMS_IMAGE_OWNER[^}]*\}/\S+:\$\{VSMS_\w*TAG:-v\d+\.\d+\.\d+\}",
    )
    .expect("static pattern");
    for rel in generic_paths.iter().filter(|p| p.contains("compose")) {
        for (n, line) in read(root, rel)?.lines().enumerate() {
            if image.is_match(line) && !line.contains(ANNOTATION) {
                problems.push(format!(
                    "  {rel}:{}: a versioned vsms image default with no {ANNOTATION} comment — \
                     release-please will leave this service pinned to the previous release",
                    n + 1
                ));
            }
        }
    }

    if problems.is_empty() {
        let mut versions: Vec<&String> = found.values().collect();
        versions.sort_unstable();
        versions.dedup();
        match versions.as_slice() {
            [one] => {
                println!("OK — {} version references all say {one}", found.len());
                return Ok(());
            }
            _ => {
                for (place, v) in &found {
                    problems.push(format!("  {place}: {v}"));
                }
            }
        }
    }

    Err(format!(
        "release version references disagree, or a release-please annotation is missing:\n{}\n\
         Every line above is one release-please owns. They must all hold the same version, \
         and every file listed in {CONFIG}'s extra-files must keep at least one \
         `{ANNOTATION}` comment for it to act on — see .xtask/src/release_versions.rs.",
        problems.join("\n")
    ))
}

fn read(root: &Path, rel: &str) -> Result<String, String> {
    let path = root.join(rel);
    fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The `extra-files` entries: every bare-string path, plus the single
/// object-form `"path"`.
///
/// Parsed line by line rather than with a JSON library, matching this
/// crate's own "plain std plus regex" discipline. A reformat that puts the
/// array on one line finds nothing and is reported as an error rather than
/// passing vacuously.
fn extra_files(config: &str) -> Result<(Vec<String>, String), String> {
    let mut generic = Vec::new();
    let mut json_path = None;
    let bare = Regex::new(r#"^\s*"([^"]+)",?\s*$"#).expect("static pattern");
    let keyed = Regex::new(r#"^\s*"path":\s*"([^"]+)",?\s*$"#).expect("static pattern");

    let mut inside = false;
    for line in config.lines() {
        if line.contains("\"extra-files\"") {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line.trim_start().starts_with(']') {
            break;
        }
        if let Some(c) = keyed.captures(line) {
            json_path = Some(c[1].to_owned());
        } else if let Some(c) = bare.captures(line) {
            generic.push(c[1].to_owned());
        }
    }

    if generic.is_empty() {
        return Err(format!(
            "{CONFIG}: found no bare-string extra-files entries. This parser is line-based \
             (see .xtask/src/release_versions.rs); if the config was reformatted, reformat it \
             back or teach the parser the new shape — do not leave the check passing vacuously."
        ));
    }
    let json_path = json_path.ok_or_else(|| {
        format!("{CONFIG}: found no object-form extra-files entry with a \"path\"")
    })?;
    Ok((generic, json_path))
}

/// The string value of a top-level `"<key>": "<value>"` pair.
fn json_string_field(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let after = text.lines().find_map(|l| l.trim().strip_prefix(&needle))?;
    let after = after.trim_start().strip_prefix('"')?;
    let end = after.find('"')?;
    Some(after[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_top_level_json_string() {
        assert_eq!(
            json_string_field("{\n  \".\": \"0.3.1\"\n}\n", ".").as_deref(),
            Some("0.3.1")
        );
        assert_eq!(
            json_string_field("{\n  \"version\": \"1.2.3\",\n}\n", "version").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(json_string_field("{}\n", "version"), None);
    }

    #[test]
    fn splits_extra_files_into_generic_paths_and_the_json_one() {
        let config = r#"{
  "packages": {
    ".": {
      "extra-files": [
        "Cargo.toml",
        "compose.demo.yaml",
        {
          "type": "json",
          "path": "sdks/node/vsms-sdk-node/package.json",
          "jsonpath": "$.version"
        }
      ]
    }
  }
}"#;
        let (generic, json_path) = extra_files(config).expect("parses");
        assert_eq!(generic, vec!["Cargo.toml", "compose.demo.yaml"]);
        assert_eq!(json_path, "sdks/node/vsms-sdk-node/package.json");
    }

    /// A reformat that collapses the array must fail loudly. A check that
    /// silently finds nothing to check is worse than no check.
    #[test]
    fn refuses_a_config_it_cannot_read() {
        let collapsed = r#"{"extra-files": ["Cargo.toml"]}"#;
        assert!(extra_files(collapsed).is_err());
    }

    /// The realistic regression: a compose service added without the
    /// annotation. Prove the pattern that finds it actually matches a real
    /// line, and that the annotated form is the one it lets through.
    #[test]
    fn spots_an_unannotated_vsms_image_default() {
        let image = Regex::new(
            r"^\s*image:\s*ghcr\.io/\$\{VSMS_IMAGE_OWNER[^}]*\}/\S+:\$\{VSMS_\w*TAG:-v\d+\.\d+\.\d+\}",
        )
        .expect("static pattern");

        let bare = "    image: ghcr.io/${VSMS_IMAGE_OWNER:-vaam-apps}/vsms-admin:${VSMS_IMAGE_TAG:-v0.3.1}";
        assert!(image.is_match(bare));
        assert!(!bare.contains(ANNOTATION));

        let ok = format!("{bare} # {ANNOTATION}");
        assert!(image.is_match(&ok));
        assert!(ok.contains(ANNOTATION));

        // A nested repo path (vsms/demo-app) and a second knob name must
        // both still match — they are two of the eighteen real lines.
        let nested = "    image: ghcr.io/${VSMS_IMAGE_OWNER:-vaam-apps}/vsms/demo-app:${VSMS_DEMO_APP_IMAGE_TAG:-v0.3.1}";
        assert!(image.is_match(nested));

        // Postgres is not a vsms image and must never be flagged.
        let postgres = "    image: postgres:16.4-alpine";
        assert!(!image.is_match(postgres));
    }
}
