//! The Helm chart renders, and the overlay `ConfigMap` is the one the Pods mount.
//!
//! # Why this exists
//!
//! VSC-1 added a `ConfigMap` to `deploy/charts/vsms` (the YAML overlay) and a
//! volume that mounts it into the gateway and the worker. The first version
//! rendered the `ConfigMap` from `templates/configmap-overlay.yaml` and the volume
//! from `templates/common.yaml`, each deriving the name with the same helper.
//! Helm renders templates in reverse path order, so the `ConfigMap`'s name was
//! computed before `common.yaml` set `global.nameOverride` and the volume's after.
//! For a release whose name contains "vsms" the two agree by accident; for any
//! other, `helm template foo` yielded `ConfigMap` `foo-vsms-config` and a volume
//! naming `foo-config`, and the Pods hung in `ContainerCreating`. Every file-level
//! test was green, and so was `helm lint --strict`: the defect is in what the
//! templates *render to*, which only rendering them shows.
//!
//! So this renders them. It needs `helm` on the `PATH` (CI installs it with
//! `azure/setup-helm`, pinned in `.github/workflows/ci.yml`'s `rules` job) and
//! network access to the chart repository `Chart.yaml` names, for
//! `helm dependency build`.
//!
//! # What is checked
//!
//! 1. `helm lint --strict` over a minimal, valid set of values.
//! 2. `helm template` under two release names, one **without** "vsms" in it (the
//!    case that was broken) and one with. In each, every `ConfigMap` a workload
//!    mounts must be a `ConfigMap` the chart renders; the overlay `ConfigMap` must
//!    exist, carry `application-<profile>.yml`, be named after the release, and be
//!    mounted by both the gateway and the worker.
//! 3. Each chart guard fires when it should, with its own name in the message:
//!    the retired `orange`, `mtn` and `awsSms` values, an empty or blank overlay,
//!    an overlay holding a template action, a profile that is not a safe file
//!    name, and the removed `config.createOverlayConfigMap`.
//!
//! Output is read as text: this crate has no YAML parser, and a rendered
//! manifest has a regular enough shape that two patterns answer the question.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use regex::Regex;

/// The chart under test.
const CHART: &str = "deploy/charts/vsms";

/// Release names the render runs under. The first has no "vsms" in it, which is
/// what the original defect needed to show itself.
const RELEASES: [&str; 2] = ["config-render-check", "vsms-render-check"];

/// The smallest set of values the chart accepts: every `required` one, and an
/// overlay. Nothing here is a real key or a real secret; they are Secret *names*.
const VALUES: &str = "\
image:
  tag: v0.0.0-render-check
admin:
  serverKeys:
    current: render-check-current
    next: render-check-next
existingSecrets:
  database:
    name: render-check-database
  hashPepper:
    name: render-check-pepper
  envelopeKeys:
    name: render-check-envelope
config:
  profile: prod
  overlay: |
    deployment:
      name: render-check
      livemode: true
      public_base_url: https://sms.example.com
    providers: []
";

/// A values overlay that must make the chart refuse, and the guard that says so.
struct Refusal {
    what: &'static str,
    values: &'static str,
    guard: &'static str,
}

const REFUSALS: [Refusal; 11] = [
    Refusal {
        what: "the retired `orange` values",
        values: "orange:\n  clientId: x\n",
        guard: "retired-values",
    },
    Refusal {
        what: "`orange` set to null",
        values: "orange: null\n",
        guard: "retired-values",
    },
    Refusal {
        what: "the retired `mtn` values, empty",
        values: "mtn: {}\n",
        guard: "retired-values",
    },
    Refusal {
        what: "the retired `awsSms` values",
        values: "awsSms:\n  region: eu-west-1\n",
        guard: "retired-values",
    },
    Refusal {
        what: "the removed `config.createOverlayConfigMap`",
        values: "config:\n  createOverlayConfigMap: true\n",
        guard: "retired-values",
    },
    Refusal {
        what: "an empty overlay",
        values: "config:\n  overlay: \"\"\n",
        guard: "config-overlay",
    },
    Refusal {
        what: "a whitespace-only overlay",
        values: "config:\n  overlay: \"  \\n \\n\"\n",
        guard: "config-overlay",
    },
    Refusal {
        what: "an overlay holding a template action",
        values: "config:\n  overlay: \"name: '{{ .Release.Name }}'\\n\"\n",
        guard: "config-overlay-template",
    },
    Refusal {
        what: "a profile with a capital letter",
        values: "config:\n  profile: Prod\n",
        guard: "config-profile",
    },
    Refusal {
        what: "a profile that climbs out of the directory",
        values: "config:\n  profile: ../x\n",
        guard: "config-profile",
    },
    Refusal {
        what: "a profile with a slash",
        values: "config:\n  profile: a/b\n",
        guard: "config-profile",
    },
];

pub fn run(root: &Path) -> Result<(), String> {
    let chart = root.join(CHART);
    let scratch = std::env::temp_dir().join(format!("vsms-chart-render-{}", std::process::id()));
    fs::create_dir_all(&scratch)
        .map_err(|error| format!("creating {}: {error}", scratch.display()))?;
    let outcome = render_everything(&chart, &scratch);
    fs::remove_dir_all(&scratch).ok();
    outcome
}

fn render_everything(chart: &Path, scratch: &Path) -> Result<(), String> {
    let values = write(scratch, "values.yaml", VALUES)?;

    build_dependencies(chart)?;

    let lint = helm(&["lint", "--strict", path(chart)?, "-f", path(&values)?])?;
    require_success("helm lint --strict", &lint)?;

    for release in RELEASES {
        let rendered = helm(&["template", release, path(chart)?, "-f", path(&values)?])?;
        require_success(&format!("helm template {release}"), &rendered)?;
        check_overlay(release, &String::from_utf8_lossy(&rendered.stdout))?;
    }

    for (index, refusal) in REFUSALS.iter().enumerate() {
        let extra = write(scratch, &format!("refusal-{index}.yaml"), refusal.values)?;
        let rendered = helm(&[
            "template",
            RELEASES[0],
            path(chart)?,
            "-f",
            path(&values)?,
            "-f",
            path(&extra)?,
        ])?;
        let stderr = String::from_utf8_lossy(&rendered.stderr);
        let marker = format!("vsms chart guard \"{}\"", refusal.guard);
        if rendered.status.success() {
            return Err(format!(
                "chart-render: {} rendered, and the `{}` guard exists to refuse it",
                refusal.what, refusal.guard
            ));
        }
        if !stderr.contains(&marker) {
            return Err(format!(
                "chart-render: {} was refused, but not by the `{}` guard. helm said:\n{stderr}",
                refusal.what, refusal.guard
            ));
        }
    }

    println!(
        "chart-render OK (helm lint --strict; {} releases rendered, the overlay ConfigMap is the one mounted; {} refusals fire)",
        RELEASES.len(),
        REFUSALS.len()
    );
    Ok(())
}

/// The overlay `ConfigMap` exists under the name the Pods mount.
fn check_overlay(release: &str, manifest: &str) -> Result<(), String> {
    let documents: Vec<&str> = manifest.split("\n---\n").collect();

    let name_of = Regex::new(r"(?m)^  name: (\S+)\s*$").map_err(|error| error.to_string())?;
    let mounted_of = Regex::new(r"(?m)^[\s-]+configMap:\n\s+name: (\S+)\s*$")
        .map_err(|error| error.to_string())?;

    let mut rendered = Vec::new();
    let mut overlay_name = None;
    for document in &documents {
        if !document.lines().any(|line| line == "kind: ConfigMap") {
            continue;
        }
        let Some(name) = name_of.captures(document).map(|c| c[1].to_owned()) else {
            return Err(format!(
                "chart-render ({release}): a rendered ConfigMap has no metadata.name"
            ));
        };
        if document.contains("application-prod.yml:") {
            overlay_name = Some(name.clone());
        }
        rendered.push(name);
    }
    let Some(overlay_name) = overlay_name else {
        return Err(format!(
            "chart-render ({release}): no rendered ConfigMap carries application-prod.yml; rendered: {rendered:?}"
        ));
    };
    if !overlay_name.starts_with(release) {
        return Err(format!(
            "chart-render ({release}): the overlay ConfigMap is named {overlay_name:?}, not after the release"
        ));
    }

    let mut mounting_workloads = 0usize;
    for document in &documents {
        let mounted: Vec<String> = mounted_of
            .captures_iter(document)
            .map(|c| c[1].to_owned())
            .collect();
        if mounted.is_empty() {
            continue;
        }
        for name in &mounted {
            if !rendered.contains(name) {
                return Err(format!(
                    "chart-render ({release}): a workload mounts ConfigMap {name:?}, which the chart does not render \
                     (it renders {rendered:?}); the Pods would hang in ContainerCreating"
                ));
            }
        }
        if mounted.contains(&overlay_name) {
            mounting_workloads += 1;
        }
    }
    if mounting_workloads != 2 {
        return Err(format!(
            "chart-render ({release}): the overlay ConfigMap {overlay_name:?} is mounted by {mounting_workloads} workloads, \
             expected the gateway and the worker"
        ));
    }
    Ok(())
}

/// `helm dependency build`, after adding the repository `Chart.yaml` names (a
/// fresh runner has none configured, and the command refuses an unknown one).
fn build_dependencies(chart: &Path) -> Result<(), String> {
    let manifest = fs::read_to_string(chart.join("Chart.yaml"))
        .map_err(|error| format!("reading Chart.yaml: {error}"))?;
    let repository = Regex::new(r"(?m)^\s+repository:\s*(https://\S+)\s*$")
        .map_err(|error| error.to_string())?;
    let urls: Vec<&str> = repository
        .captures_iter(&manifest)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
        .collect();
    if urls.is_empty() {
        return Err("chart-render: Chart.yaml names no https dependency repository".to_owned());
    }
    for (index, url) in urls.iter().enumerate() {
        let added = helm(&[
            "repo",
            "add",
            "--force-update",
            &format!("vsms-chart-render-{index}"),
            url,
        ])?;
        require_success("helm repo add", &added)?;
    }
    let built = helm(&["dependency", "build", path(chart)?])?;
    require_success("helm dependency build", &built)
}

fn helm(args: &[&str]) -> Result<Output, String> {
    Command::new("helm").args(args).output().map_err(|error| {
        format!(
            "running `helm {}`: {error}. chart-render needs helm on the PATH (CI installs it with azure/setup-helm)",
            args.first().copied().unwrap_or("")
        )
    })
}

fn require_success(what: &str, output: &Output) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "chart-render: {what} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn write(dir: &Path, name: &str, text: &str) -> Result<PathBuf, String> {
    let file = dir.join(name);
    fs::write(&file, text).map_err(|error| format!("writing {}: {error}", file.display()))?;
    Ok(file)
}

fn path(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("{} is not UTF-8", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "\
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: config-render-check-config
  labels:
    a: b
data:
  application-prod.yml: |
    x: y
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: config-render-check-gateway
spec:
  template:
    spec:
      volumes:
        - configMap:
            name: config-render-check-config
          name: config-overlay
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: config-render-check-worker
spec:
  template:
    spec:
      volumes:
        - configMap:
            name: config-render-check-config
          name: config-overlay
";

    #[test]
    fn a_render_whose_names_agree_passes() {
        check_overlay("config-render-check", GOOD).unwrap();
    }

    /// The original defect, as the rendered text showed it.
    #[test]
    fn a_configmap_named_for_the_release_while_the_pods_mount_another_is_refused() {
        let broken = GOOD.replacen(
            "name: config-render-check-config\n  labels",
            "name: config-render-check-vsms-config\n  labels",
            1,
        );
        let error = check_overlay("config-render-check", &broken).unwrap_err();
        assert!(error.contains("which the chart does not render"), "{error}");
    }

    #[test]
    fn an_overlay_nobody_mounts_is_refused() {
        let unmounted = GOOD.replace(
            "config-render-check-config\n          name",
            "other\n          name",
        );
        assert!(check_overlay("config-render-check", &unmounted).is_err());
    }

    #[test]
    fn a_render_with_no_overlay_configmap_is_refused() {
        let none = GOOD.replace("application-prod.yml:", "other.yml:");
        let error = check_overlay("config-render-check", &none).unwrap_err();
        assert!(error.contains("no rendered ConfigMap carries"), "{error}");
    }

    #[test]
    fn every_refusal_names_a_guard_the_chart_defines() {
        let template = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect(".xtask always has a parent")
                .join(CHART)
                .join("templates/common.yaml"),
        )
        .expect("the chart's common.yaml is committed");
        for refusal in &REFUSALS {
            assert!(
                template.contains(&format!("chart guard \\\"{}\\\"", refusal.guard)),
                "no guard named {} in templates/common.yaml",
                refusal.guard
            );
        }
    }
}
