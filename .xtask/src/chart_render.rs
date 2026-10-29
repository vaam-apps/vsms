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
//! 3. An operator's own `configMaps` entry survives next to the overlay's and is
//!    mounted by the workload that names it (the chart used to replace the whole
//!    map), and an edit to the overlay changes `checksum/configMaps` on the
//!    gateway and the worker, which is what rolls their Pods.
//! 4. Each chart guard fires when it should, with its own name in the message:
//!    the retired `orange`, `mtn` and `awsSms` values, an empty or blank overlay,
//!    an overlay holding a template action, a profile that is not a safe file
//!    name, the removed `config.createOverlayConfigMap`, and an operator
//!    `configMaps.overlay`.
//!
//! Helm runs with its own config, cache and data directories under the target
//! directory, so adding the dependency repository never touches the developer's
//! real `~/.config/helm`.
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

const REFUSALS: [Refusal; 12] = [
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
    Refusal {
        what: "an operator ConfigMap under the chart's own `overlay` key",
        values: "configMaps:\n  overlay:\n    enabled: true\n    data:\n      x: y\n",
        guard: "config-maps-overlay",
    },
];

/// An operator's own `ConfigMap` and the mount of it: `configMaps` and
/// `persistence` entries a release adds beside the overlay's. Mounted by
/// `identifier`, which is the form that failed when the chart replaced the map.
const OPERATOR_CONFIGMAP: &str = "\
configMaps:
  ca:
    enabled: true
    suffix: ca
    data:
      ca.pem: render-check-ca-bundle
persistence:
  ca:
    type: configMap
    identifier: ca
    advancedMounts:
      gateway:
        main:
          - path: /etc/ssl/render-check/ca.pem
            subPath: ca.pem
            readOnly: true
";

pub fn run(root: &Path) -> Result<(), String> {
    let chart = root.join(CHART);
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| root.join("target"), |dir| root.join(dir));
    let scratch = target.join(format!("chart-render-{}", std::process::id()));
    fs::create_dir_all(&scratch)
        .map_err(|error| format!("creating {}: {error}", scratch.display()))?;
    let outcome =
        Helm::isolated(&scratch).and_then(|helm| render_everything(&helm, &chart, &scratch));
    fs::remove_dir_all(&scratch).ok();
    outcome
}

/// `helm`, with its config, cache and data directories inside the scratch
/// directory. `helm repo add` writes `repositories.yaml` and a cache, and a
/// check that ran on a developer's machine must not edit their real helm.
struct Helm {
    homes: [(&'static str, PathBuf); 3],
}

impl Helm {
    fn isolated(scratch: &Path) -> Result<Self, String> {
        let homes = [
            ("HELM_CONFIG_HOME", scratch.join("helm-config")),
            ("HELM_CACHE_HOME", scratch.join("helm-cache")),
            ("HELM_DATA_HOME", scratch.join("helm-data")),
        ];
        for (_, dir) in &homes {
            fs::create_dir_all(dir)
                .map_err(|error| format!("creating {}: {error}", dir.display()))?;
        }
        Ok(Self { homes })
    }

    fn run(&self, args: &[&str]) -> Result<Output, String> {
        let mut command = Command::new("helm");
        command.args(args);
        for (name, dir) in &self.homes {
            command.env(name, dir);
        }
        command.output().map_err(|error| {
            format!(
                "running `helm {}`: {error}. chart-render needs helm on the PATH (CI installs it with azure/setup-helm)",
                args.first().copied().unwrap_or("")
            )
        })
    }
}

fn render_everything(helm: &Helm, chart: &Path, scratch: &Path) -> Result<(), String> {
    let values = write(scratch, "values.yaml", VALUES)?;

    check_isolated(helm, scratch)?;
    build_dependencies(helm, chart)?;

    let lint = helm.run(&["lint", "--strict", path(chart)?, "-f", path(&values)?])?;
    require_success("helm lint --strict", &lint)?;

    for release in RELEASES {
        let rendered = helm.run(&["template", release, path(chart)?, "-f", path(&values)?])?;
        require_success(&format!("helm template {release}"), &rendered)?;
        check_overlay(release, &String::from_utf8_lossy(&rendered.stdout))?;
    }

    check_operator_configmap(helm, chart, scratch, &values)?;
    check_overlay_edit_rolls_pods(helm, chart, scratch, &values)?;

    for (index, refusal) in REFUSALS.iter().enumerate() {
        let extra = write(scratch, &format!("refusal-{index}.yaml"), refusal.values)?;
        let rendered = helm.run(&[
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
        "chart-render OK (helm lint --strict; {} releases rendered, the overlay ConfigMap is the one mounted; an operator ConfigMap survives beside it; an overlay edit changes checksum/configMaps; {} refusals fire)",
        RELEASES.len(),
        REFUSALS.len()
    );
    Ok(())
}

/// Helm is using the directories `Helm::isolated` gave it, not the developer's.
fn check_isolated(helm: &Helm, scratch: &Path) -> Result<(), String> {
    for (name, _) in &helm.homes {
        let printed = helm.run(&["env", name])?;
        require_success(&format!("helm env {name}"), &printed)?;
        let dir = String::from_utf8_lossy(&printed.stdout).trim().to_owned();
        if !Path::new(&dir).starts_with(scratch) {
            return Err(format!(
                "chart-render: helm's {name} is {dir:?}, outside {}; `helm repo add` would edit the developer's real helm configuration",
                scratch.display()
            ));
        }
    }
    Ok(())
}

/// An operator's own `ConfigMap` is rendered and mounted beside the overlay's.
fn check_operator_configmap(
    helm: &Helm,
    chart: &Path,
    scratch: &Path,
    values: &Path,
) -> Result<(), String> {
    let extra = write(scratch, "operator-configmap.yaml", OPERATOR_CONFIGMAP)?;
    for release in RELEASES {
        let rendered = helm.run(&[
            "template",
            release,
            path(chart)?,
            "-f",
            path(values)?,
            "-f",
            path(&extra)?,
        ])?;
        require_success(
            &format!("helm template {release} (operator ConfigMap)"),
            &rendered,
        )?;
        let manifest = String::from_utf8_lossy(&rendered.stdout);
        // The overlay is still the one mounted, by both workloads...
        check_overlay(release, &manifest)?;
        // ...and the operator's ConfigMap is rendered and mounted by the gateway.
        check_operator_mount(release, &manifest)?;
    }
    Ok(())
}

/// The `ConfigMap` carrying `ca.pem` is rendered, and exactly one workload
/// mounts it.
fn check_operator_mount(release: &str, manifest: &str) -> Result<(), String> {
    let documents: Vec<&str> = manifest.split("\n---\n").collect();
    let name_of = Regex::new(r"(?m)^  name: (\S+)\s*$").map_err(|error| error.to_string())?;
    let mounted_of = Regex::new(r"(?m)^[\s-]+configMap:\n\s+name: (\S+)\s*$")
        .map_err(|error| error.to_string())?;

    let operator = documents
        .iter()
        .find(|document| {
            document.lines().any(|line| line == "kind: ConfigMap")
                && document.contains("ca.pem:")
        })
        .and_then(|document| name_of.captures(document).map(|c| c[1].to_owned()))
        .ok_or_else(|| {
            format!(
                "chart-render ({release}): the operator's `configMaps.ca` was not rendered; the chart replaced the operator's configMaps with its own"
            )
        })?;
    let mounting = documents
        .iter()
        .filter(|document| mounted_of.captures_iter(document).any(|c| c[1] == operator))
        .count();
    if mounting != 1 {
        return Err(format!(
            "chart-render ({release}): the operator's ConfigMap {operator:?} is mounted by {mounting} workloads, expected the gateway alone"
        ));
    }
    Ok(())
}

/// Editing the overlay changes the gateway's and the worker's pod templates, so
/// their Pods restart and read it: the property the chart's own checksum
/// annotation used to carry, now common's `checksum/configMaps`.
fn check_overlay_edit_rolls_pods(
    helm: &Helm,
    chart: &Path,
    scratch: &Path,
    values: &Path,
) -> Result<(), String> {
    let edited_text = VALUES.replace("name: render-check\n", "name: render-check-edited\n");
    if edited_text == VALUES {
        return Err(
            "chart-render: the edited overlay is the original; fix the replacement".to_owned(),
        );
    }
    let edited = write(scratch, "values-edited.yaml", &edited_text)?;
    let mut sums = Vec::new();
    for file in [values, edited.as_path()] {
        let rendered = helm.run(&["template", RELEASES[0], path(chart)?, "-f", path(file)?])?;
        require_success("helm template (overlay edit)", &rendered)?;
        sums.push(pod_checksums(&String::from_utf8_lossy(&rendered.stdout))?);
    }
    for workload in ["gateway", "worker"] {
        let before = sums[0].get(workload);
        let after = sums[1].get(workload);
        match (before, after) {
            (Some(before), Some(after)) if before != after => {}
            (Some(_), Some(_)) => {
                return Err(format!(
                    "chart-render: editing the overlay left the {workload} pod template unchanged, so its Pods would keep running on the old configuration"
                ));
            }
            _ => {
                return Err(format!(
                    "chart-render: the {workload} Deployment carries no checksum/configMaps annotation"
                ));
            }
        }
    }
    Ok(())
}

/// `checksum/configMaps` of each Deployment, by the workload's last name part.
fn pod_checksums(manifest: &str) -> Result<std::collections::BTreeMap<String, String>, String> {
    let name_of = Regex::new(r"(?m)^  name: \S+-(\S+)\s*$").map_err(|error| error.to_string())?;
    let sum_of =
        Regex::new(r"(?m)^\s+checksum/configMaps: (\S+)\s*$").map_err(|error| error.to_string())?;
    let mut found = std::collections::BTreeMap::new();
    for document in manifest.split("\n---\n") {
        if !document.lines().any(|line| line == "kind: Deployment") {
            continue;
        }
        let (Some(name), Some(sum)) = (name_of.captures(document), sum_of.captures(document))
        else {
            continue;
        };
        found.insert(name[1].to_owned(), sum[1].to_owned());
    }
    Ok(found)
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
fn build_dependencies(helm: &Helm, chart: &Path) -> Result<(), String> {
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
        let added = helm.run(&[
            "repo",
            "add",
            "--force-update",
            &format!("vsms-chart-render-{index}"),
            url,
        ])?;
        require_success("helm repo add", &added)?;
    }
    let built = helm.run(&["dependency", "build", path(chart)?])?;
    require_success("helm dependency build", &built)
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

    const OPERATOR: &str = "\
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: config-render-check-ca
data:
  ca.pem: |
    x
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
            name: config-render-check-ca
          name: ca
";

    #[test]
    fn helm_gets_three_directories_inside_the_scratch_directory() {
        let scratch = std::env::temp_dir().join(format!("vsms-xtask-helm-{}", std::process::id()));
        let helm = Helm::isolated(&scratch).unwrap();
        let names: Vec<&str> = helm.homes.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            ["HELM_CONFIG_HOME", "HELM_CACHE_HOME", "HELM_DATA_HOME"]
        );
        for (_, dir) in &helm.homes {
            assert!(
                dir.starts_with(&scratch) && dir.is_dir(),
                "{}",
                dir.display()
            );
        }
        fs::remove_dir_all(&scratch).ok();
    }

    #[test]
    fn an_operator_configmap_that_is_rendered_and_mounted_passes() {
        check_operator_mount("config-render-check", OPERATOR).unwrap();
    }

    /// The defect, as the render showed it: the chart's own `configMaps` replaced
    /// the operator's, so the `ConfigMap` is not there.
    #[test]
    fn an_operator_configmap_the_chart_dropped_is_refused() {
        let dropped = OPERATOR.replace("ca.pem:", "other.pem:");
        let error = check_operator_mount("config-render-check", &dropped).unwrap_err();
        assert!(error.contains("was not rendered"), "{error}");
    }

    #[test]
    fn an_operator_configmap_nobody_mounts_is_refused() {
        let unmounted = OPERATOR.replace(
            "name: config-render-check-ca\n          name",
            "name: x\n          name",
        );
        let error = check_operator_mount("config-render-check", &unmounted).unwrap_err();
        assert!(error.contains("mounted by 0 workloads"), "{error}");
    }

    #[test]
    fn pod_checksums_are_read_per_workload() {
        let manifest = "\
---
kind: Deployment
metadata:
  name: config-render-check-gateway
spec:
  template:
    metadata:
      annotations:
        checksum/configMaps: aaa
---
kind: Deployment
metadata:
  name: config-render-check-worker
spec:
  template:
    metadata:
      annotations:
        checksum/configMaps: bbb
";
        let sums = pod_checksums(manifest).unwrap();
        assert_eq!(sums.get("gateway").map(String::as_str), Some("aaa"));
        assert_eq!(sums.get("worker").map(String::as_str), Some("bbb"));
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
