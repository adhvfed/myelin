//! CT-007 pre-registered cutover-floor GATE 1/4 (workload inventory) —
//! `planning/system-reviews/2026-06-26/12-ci-track-ledger.md` (~line 302, "Pre-registered CT-007
//! cutover floor"): "A committed Myelin-native workflow maps every still-required GitHub job to an
//! executable Myelin job or names a mechanically gated, non-CI owner; an inventory test fails on
//! silent job loss."
//!
//! This IS that inventory test. It is INVENTORY ONLY — CT-007 has not been opened; no GitHub job
//! is migrated, disabled, or removed here. The committed manifest `ci-workload-inventory.toml`
//! (workspace root) names, for every job GitHub Actions currently runs, an honest `status` (today:
//! uniformly `github-only` — see the manifest's own header for why NONE are `myelin-native` yet)
//! and an `owner` naming the CT-007 cutover-plan step accountable for eventually migrating it.
//!
//! The gate, in both directions:
//!   - every job id discovered by scanning `.github/workflows/*.yml`'s `jobs:` block MUST have
//!     exactly one manifest row (else a job silently drops out of the inventory — the exact
//!     defect the ledger's gate exists to catch);
//!   - every manifest row MUST name a job that still actually exists in the workflow files (else
//!     the inventory drifts into a stale, untrue claim).
//!
//! `check_inventory` carries that logic and is unit-tested directly with synthetic RED/GREEN data
//! below (matching how `crates/myelin-lints/src/erosion.rs` unit-tests its scan function
//! in-process) — the real-workspace test below cannot easily inject a fake missing/stale job
//! without a temp-file fixture, so the shared checking function itself is proven to fire instead.

use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// One job id discovered directly from a workflow file's `jobs:` block.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct WorkflowJob {
    workflow: String,
    job: String,
}

/// One row of the committed `ci-workload-inventory.toml`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct ManifestJob {
    workflow: String,
    job: String,
    status: String,
    #[allow(dead_code)] // read for completeness/documentation; not asserted on directly
    owner: String,
    #[serde(default)]
    myelin_job: String,
    #[allow(dead_code)]
    note: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    job: Vec<ManifestJob>,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("myelin-lints lives under <workspace>/crates")
        .to_path_buf()
}

/// Parse job identifiers out of the top-level (0-indent) `jobs:` section of a GitHub Actions
/// workflow file: exactly-two-space-indented `name:` keys, stopping at the next 0-indent key (or
/// EOF). Deliberately dumb line scanning, matching the style already used in this crate
/// (`tests/github_actions_pinned.rs`, `tests/container_images_pinned.rs`) rather than pulling in a
/// YAML dependency.
fn parse_workflow_job_names(source: &str) -> Vec<String> {
    let mut jobs = Vec::new();
    let mut in_jobs = false;
    for line in source.lines() {
        if !in_jobs {
            if line == "jobs:" {
                in_jobs = true;
            }
            continue;
        }
        if !line.is_empty() && !line.starts_with(' ') {
            if line.starts_with('#') {
                continue; // a trailing top-level comment does not end the jobs: block
            }
            break; // a new top-level key ends the jobs: block
        }
        let Some(rest) = line.strip_prefix("  ") else {
            continue;
        };
        if rest.starts_with(' ') || rest.starts_with('#') {
            continue; // deeper-nested key (job body) or a comment, not a job name
        }
        let Some(name) = rest.strip_suffix(':') else {
            continue;
        };
        if !name.is_empty()
            && name
                .chars()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == '-' || byte == '_')
        {
            jobs.push(name.to_string());
        }
    }
    jobs
}

/// Discover every job in every `.yml`/`.yaml` workflow file under `workflows_dir`.
fn discover_github_jobs(workflows_dir: &Path) -> Vec<WorkflowJob> {
    let mut entries: Vec<PathBuf> = fs::read_dir(workflows_dir)
        .expect("read .github/workflows")
        .map(|entry| entry.expect("workflow directory entry").path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("yml" | "yaml")
            )
        })
        .collect();
    entries.sort();

    let mut discovered = Vec::new();
    for path in entries {
        let workflow = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("workflow file has a UTF-8 name")
            .to_string();
        let source = fs::read_to_string(&path).expect("read workflow");
        for job in parse_workflow_job_names(&source) {
            discovered.push(WorkflowJob {
                workflow: workflow.clone(),
                job,
            });
        }
    }
    discovered
}

/// The shared gate logic: every discovered GitHub job must have exactly one manifest row, every
/// manifest row must name a job that still actually exists among the discovered jobs, and no
/// (workflow, job) pair may have more than one manifest row. Returns one error string per
/// violation (empty ⇒ green).
fn check_inventory(discovered: &[WorkflowJob], manifest: &[ManifestJob]) -> Vec<String> {
    let mut errors = Vec::new();

    let discovered_set: BTreeSet<(&str, &str)> = discovered
        .iter()
        .map(|job| (job.workflow.as_str(), job.job.as_str()))
        .collect();
    let manifest_set: BTreeSet<(&str, &str)> = manifest
        .iter()
        .map(|entry| (entry.workflow.as_str(), entry.job.as_str()))
        .collect();

    for job in discovered {
        let key = (job.workflow.as_str(), job.job.as_str());
        if !manifest_set.contains(&key) {
            errors.push(format!(
                "GitHub job `{}` (workflow `{}`) has NO ci-workload-inventory.toml entry — a job \
                 silently dropped from the inventory. Add a [[job]] row naming its status and owner.",
                job.job, job.workflow
            ));
        }
    }

    for entry in manifest {
        let key = (entry.workflow.as_str(), entry.job.as_str());
        if !discovered_set.contains(&key) {
            errors.push(format!(
                "ci-workload-inventory.toml claims job `{}` in workflow `{}`, but no such job \
                 exists in .github/workflows/{} anymore — a stale inventory entry.",
                entry.job, entry.workflow, entry.workflow
            ));
        }
    }

    let mut seen = BTreeSet::new();
    for entry in manifest {
        let key = (entry.workflow.as_str(), entry.job.as_str());
        if !seen.insert(key) {
            errors.push(format!(
                "ci-workload-inventory.toml has more than one [[job]] entry for `{}` in workflow \
                 `{}` — exactly one entry per job is required.",
                entry.job, entry.workflow
            ));
        }
    }

    errors
}

fn load_real_manifest() -> Manifest {
    let source = fs::read_to_string(workspace_root().join("ci-workload-inventory.toml"))
        .expect("read ci-workload-inventory.toml");
    toml::from_str(&source).expect("parse ci-workload-inventory.toml")
}

#[test]
fn every_discovered_github_job_has_exactly_one_honest_inventory_entry() {
    let root = workspace_root();
    let discovered = discover_github_jobs(&root.join(".github/workflows"));
    assert!(
        !discovered.is_empty(),
        "must discover at least one GitHub job from .github/workflows — parser regression?"
    );

    let manifest = load_real_manifest();
    let errors = check_inventory(&discovered, &manifest.job);
    assert!(
        errors.is_empty(),
        "ci-workload-inventory.toml drifted out of truth with .github/workflows:\n{}",
        errors.join("\n")
    );
}

#[test]
fn no_inventory_entry_claims_myelin_native_without_naming_its_myelin_job() {
    // Honesty invariant: a row may claim progress beyond `github-only` ONLY once it names the
    // concrete `.myelin/ci.toml` job that now owns the workload — the manifest must never
    // "silently mean done". Today every row MUST be `github-only` with an empty `myelin_job`;
    // CT-007 has not migrated anything yet. This also guards against a future accidental
    // premature flip (status changed without a real myelin_job following it).
    let manifest = load_real_manifest();
    for entry in &manifest.job {
        assert_eq!(
            entry.status, "github-only",
            "`{}`/`{}`: CT-007 has not migrated anything yet, every row must currently be \
             `github-only`, found `{}`",
            entry.workflow, entry.job, entry.status
        );
        assert!(
            entry.myelin_job.is_empty(),
            "`{}`/`{}` is `github-only` but names a myelin_job `{}` — either the status is stale \
             or the myelin_job claim is premature",
            entry.workflow,
            entry.job,
            entry.myelin_job
        );
    }
}

#[test]
fn check_inventory_is_green_when_the_manifest_matches_discovered_jobs_exactly() {
    let discovered = vec![WorkflowJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
    }];
    let manifest = vec![ManifestJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
        status: "github-only".into(),
        owner: "test".into(),
        myelin_job: String::new(),
        note: "test".into(),
    }];
    assert!(check_inventory(&discovered, &manifest).is_empty());
}

#[test]
fn check_inventory_fails_loudly_on_a_silently_dropped_github_job() {
    // The RED path this gate exists for: a GitHub job appears with no corresponding manifest row.
    let discovered = vec![
        WorkflowJob {
            workflow: "ci.yml".into(),
            job: "build-test-clippy".into(),
        },
        WorkflowJob {
            workflow: "ci.yml".into(),
            job: "a-new-job-nobody-inventoried".into(),
        },
    ];
    let manifest = vec![ManifestJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
        status: "github-only".into(),
        owner: "test".into(),
        myelin_job: String::new(),
        note: "test".into(),
    }];
    let errors = check_inventory(&discovered, &manifest);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("a-new-job-nobody-inventoried")),
        "must name the silently-dropped job, got: {errors:?}"
    );
}

#[test]
fn check_inventory_fails_loudly_on_a_stale_manifest_entry() {
    // The reverse RED path: a manifest row names a job that no longer exists.
    let discovered = vec![WorkflowJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
    }];
    let manifest = vec![
        ManifestJob {
            workflow: "ci.yml".into(),
            job: "build-test-clippy".into(),
            status: "github-only".into(),
            owner: "test".into(),
            myelin_job: String::new(),
            note: "test".into(),
        },
        ManifestJob {
            workflow: "ci.yml".into(),
            job: "a-removed-job".into(),
            status: "github-only".into(),
            owner: "test".into(),
            myelin_job: String::new(),
            note: "test".into(),
        },
    ];
    let errors = check_inventory(&discovered, &manifest);
    assert!(
        errors.iter().any(|error| error.contains("a-removed-job")),
        "must name the stale entry, got: {errors:?}"
    );
}

#[test]
fn check_inventory_fails_loudly_on_a_duplicate_manifest_entry() {
    let discovered = vec![WorkflowJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
    }];
    let duplicate = ManifestJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
        status: "github-only".into(),
        owner: "test".into(),
        myelin_job: String::new(),
        note: "test".into(),
    };
    let manifest = vec![duplicate.clone(), duplicate];
    let errors = check_inventory(&discovered, &manifest);
    assert!(
        errors.iter().any(|error| error.contains("more than one")),
        "must flag the duplicate row, got: {errors:?}"
    );
}

#[test]
fn parse_workflow_job_names_finds_top_level_jobs_only() {
    let source = "on:\n  push:\n    branches: [main]\njobs:\n  frontend:\n    runs-on: ubuntu-latest\n    services:\n      valkey:\n        image: x\n    steps:\n      - run: x\n  build:\n    runs-on: ubuntu-latest\n";
    assert_eq!(
        parse_workflow_job_names(source),
        vec!["frontend".to_string(), "build".to_string()]
    );
}
