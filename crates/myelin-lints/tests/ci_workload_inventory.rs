use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct WorkflowJob {
    workflow: String,
    job: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct ManifestJob {
    workflow: String,
    job: String,
    status: String,
    migration_step: u32,
    migration_state: String,
    #[serde(default)]
    myelin_job: String,
    #[allow(dead_code)]
    note: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    job: Vec<ManifestJob>,
}

const VALID_MIGRATION_STATES: &[&str] = &[
    "not-started",
    "capability-smoke",
    "capability-proven",
    "job-passing",
    "graph-passing",
    "cutover-repeated",
];

const MYELIN_NATIVE_STATES: &[&str] = &["job-passing", "graph-passing", "cutover-repeated"];

fn state_matches_step(state: &str, step: u32) -> bool {
    match state {
        "not-started" => matches!(step, 2..=4),
        "capability-smoke" | "capability-proven" => step == 2,
        "job-passing" | "graph-passing" => step == 3,
        "cutover-repeated" => step == 4,
        _ => false,
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("myelin-lints lives under <workspace>/crates")
        .to_path_buf()
}

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
                continue;
            }
            break;
        }
        let Some(rest) = line.strip_prefix("  ") else {
            continue;
        };
        if rest.starts_with(' ') || rest.starts_with('#') {
            continue;
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

fn discover_github_jobs(workflows_dir: &Path) -> Vec<WorkflowJob> {
    if !workflows_dir.exists() {
        return Vec::new();
    }
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

#[derive(Debug, Deserialize)]
struct MyelinJob {
    name: String,
}

#[derive(Debug, Deserialize)]
struct MyelinManifest {
    #[serde(default)]
    jobs: Vec<MyelinJob>,
}

fn parse_myelin_job_names(source: &str) -> BTreeSet<String> {
    let manifest: MyelinManifest =
        toml::from_str(source).expect("parse .myelin/ci.toml-shaped source");
    manifest.jobs.into_iter().map(|job| job.name).collect()
}

fn check_inventory(
    discovered: &[WorkflowJob],
    manifest: &[ManifestJob],
    myelin_jobs: &BTreeSet<String>,
) -> Vec<String> {
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
                "GitHub job `{}` (workflow `{}`) has NO ci-workload-inventory.toml entry - a job \
                 silently dropped from the inventory. Add a [[job]] row naming its status and \
                 migration_step/migration_state.",
                job.job, job.workflow
            ));
        }
    }

    for entry in manifest {
        let key = (entry.workflow.as_str(), entry.job.as_str());
        if !discovered_set.contains(&key) {
            errors.push(format!(
                "ci-workload-inventory.toml claims job `{}` in workflow `{}`, but no such job \
                 exists in .github/workflows/{} anymore - a stale inventory entry.",
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
                 `{}` - exactly one entry per job is required.",
                entry.job, entry.workflow
            ));
        }
    }

    for entry in manifest {
        let label = format!("`{}`/`{}`", entry.workflow, entry.job);

        if !(1..=4).contains(&entry.migration_step) {
            errors.push(format!(
                "{label}: migration_step {} is outside the valid 1-4 range (the ledger's four \
                 cutover-floor gates)",
                entry.migration_step
            ));
        }

        if !VALID_MIGRATION_STATES.contains(&entry.migration_state.as_str()) {
            errors.push(format!(
                "{label}: migration_state `{}` is not one of the closed vocabulary {:?}",
                entry.migration_state, VALID_MIGRATION_STATES
            ));
        } else if (1..=4).contains(&entry.migration_step)
            && !state_matches_step(&entry.migration_state, entry.migration_step)
        {
            errors.push(format!(
                "{label}: migration_state `{}` does not belong to migration_step {} - \
                 capability-smoke/capability-proven pair with step 2, job-passing/graph-passing \
                 with step 3, cutover-repeated with step 4",
                entry.migration_state, entry.migration_step
            ));
        }

        match entry.status.as_str() {
            "github-only" => {
                if MYELIN_NATIVE_STATES.contains(&entry.migration_state.as_str()) {
                    errors.push(format!(
                        "{label}: status is `github-only` but migration_state `{}` implies real \
                         Myelin-native execution - either the state is premature or status should \
                         already be `myelin-native`",
                        entry.migration_state
                    ));
                }
                if !entry.myelin_job.is_empty() {
                    errors.push(format!(
                        "{label}: status is `github-only` but names a myelin_job `{}` - either \
                         the status is stale or the myelin_job claim is premature",
                        entry.myelin_job
                    ));
                }
            }
            "myelin-native" => {
                if entry.myelin_job.is_empty() {
                    errors.push(format!(
                        "{label}: status is `myelin-native` but myelin_job is empty - must name \
                         the real .myelin/ci.toml job that carries this workload"
                    ));
                } else if !myelin_jobs.contains(&entry.myelin_job) {
                    errors.push(format!(
                        "{label}: myelin_job `{}` does not exist in .myelin/ci.toml - a \
                         fabricated or stale Myelin-native claim",
                        entry.myelin_job
                    ));
                }
                if !MYELIN_NATIVE_STATES.contains(&entry.migration_state.as_str()) {
                    errors.push(format!(
                        "{label}: status is `myelin-native` but migration_state `{}` does not \
                         imply real execution yet",
                        entry.migration_state
                    ));
                }
            }
            other => errors.push(format!(
                "{label}: status `{other}` is not one of the recognized values \
                 (\"github-only\", \"myelin-native\")"
            )),
        }
    }

    errors
}

fn load_real_manifest() -> Manifest {
    let source = fs::read_to_string(workspace_root().join("ci-workload-inventory.toml"))
        .expect("read ci-workload-inventory.toml");
    toml::from_str(&source).expect("parse ci-workload-inventory.toml")
}

fn load_real_myelin_jobs() -> BTreeSet<String> {
    let source =
        fs::read_to_string(workspace_root().join(".myelin/ci.toml")).expect("read .myelin/ci.toml");
    parse_myelin_job_names(&source)
}

#[test]
fn every_discovered_github_job_has_exactly_one_honest_inventory_entry() {
    let root = workspace_root();
    let discovered = discover_github_jobs(&root.join(".github/workflows"));

    if discovered.is_empty() {
        return;
    }

    let manifest = load_real_manifest();
    let myelin_jobs = load_real_myelin_jobs();
    let errors = check_inventory(&discovered, &manifest.job, &myelin_jobs);
    assert!(
        errors.is_empty(),
        "ci-workload-inventory.toml drifted out of truth:\n{}",
        errors.join("\n")
    );
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
        migration_step: 2,
        migration_state: "not-started".into(),
        myelin_job: String::new(),
        note: "test".into(),
    }];
    assert!(check_inventory(&discovered, &manifest, &BTreeSet::new()).is_empty());
}

#[test]
fn check_inventory_fails_loudly_on_a_silently_dropped_github_job() {
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
        migration_step: 2,
        migration_state: "not-started".into(),
        myelin_job: String::new(),
        note: "test".into(),
    }];
    let errors = check_inventory(&discovered, &manifest, &BTreeSet::new());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("a-new-job-nobody-inventoried")),
        "must name the silently-dropped job, got: {errors:?}"
    );
}

#[test]
fn check_inventory_fails_loudly_on_a_stale_manifest_entry() {
    let discovered = vec![WorkflowJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
    }];
    let manifest = vec![
        ManifestJob {
            workflow: "ci.yml".into(),
            job: "build-test-clippy".into(),
            status: "github-only".into(),
            migration_step: 2,
            migration_state: "not-started".into(),
            myelin_job: String::new(),
            note: "test".into(),
        },
        ManifestJob {
            workflow: "ci.yml".into(),
            job: "a-removed-job".into(),
            status: "github-only".into(),
            migration_step: 2,
            migration_state: "not-started".into(),
            myelin_job: String::new(),
            note: "test".into(),
        },
    ];
    let errors = check_inventory(&discovered, &manifest, &BTreeSet::new());
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
        migration_step: 2,
        migration_state: "not-started".into(),
        myelin_job: String::new(),
        note: "test".into(),
    };
    let manifest = vec![duplicate.clone(), duplicate];
    let errors = check_inventory(&discovered, &manifest, &BTreeSet::new());
    assert!(
        errors.iter().any(|error| error.contains("more than one")),
        "must flag the duplicate row, got: {errors:?}"
    );
}

#[test]
fn check_inventory_fails_loudly_on_an_out_of_range_migration_step() {
    let discovered = vec![WorkflowJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
    }];
    let manifest = vec![ManifestJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
        status: "github-only".into(),
        migration_step: 5,
        migration_state: "not-started".into(),
        myelin_job: String::new(),
        note: "test".into(),
    }];
    let errors = check_inventory(&discovered, &manifest, &BTreeSet::new());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("outside the valid 1-4 range")),
        "must flag the out-of-range step, got: {errors:?}"
    );
}

#[test]
fn check_inventory_fails_loudly_on_an_unrecognized_migration_state() {
    let discovered = vec![WorkflowJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
    }];
    let manifest = vec![ManifestJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
        status: "github-only".into(),
        migration_step: 2,
        migration_state: "vibes-based".into(),
        myelin_job: String::new(),
        note: "test".into(),
    }];
    let errors = check_inventory(&discovered, &manifest, &BTreeSet::new());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("not one of the closed vocabulary")),
        "must flag the bogus state, got: {errors:?}"
    );
}

#[test]
fn check_inventory_fails_loudly_when_github_only_claims_a_native_state() {
    let discovered = vec![WorkflowJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
    }];
    let manifest = vec![ManifestJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
        status: "github-only".into(),
        migration_step: 3,
        migration_state: "graph-passing".into(),
        myelin_job: String::new(),
        note: "test".into(),
    }];
    let errors = check_inventory(&discovered, &manifest, &BTreeSet::new());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("implies real Myelin-native execution")),
        "must flag the premature native-state claim, got: {errors:?}"
    );
}

#[test]
fn check_inventory_fails_loudly_on_a_fabricated_myelin_job() {
    let discovered = vec![WorkflowJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
    }];
    let manifest = vec![ManifestJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
        status: "myelin-native".into(),
        migration_step: 3,
        migration_state: "graph-passing".into(),
        myelin_job: "a-job-that-does-not-exist".into(),
        note: "test".into(),
    }];
    let myelin_jobs: BTreeSet<String> = ["build".to_string()].into_iter().collect();
    let errors = check_inventory(&discovered, &manifest, &myelin_jobs);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("does not exist in .myelin/ci.toml")),
        "must flag the fabricated myelin_job, got: {errors:?}"
    );
}

#[test]
fn check_inventory_is_green_for_a_real_myelin_native_row() {
    let real_myelin_jobs = load_real_myelin_jobs();
    assert!(
        real_myelin_jobs.contains("build"),
        "expected .myelin/ci.toml to define a `build` job today, found {real_myelin_jobs:?} - \
         update this test to match reality if that changed"
    );

    let discovered = vec![WorkflowJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
    }];
    let manifest = vec![ManifestJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
        status: "myelin-native".into(),
        migration_step: 3,
        migration_state: "graph-passing".into(),
        myelin_job: "build".into(),
        note: "test".into(),
    }];
    assert!(check_inventory(&discovered, &manifest, &real_myelin_jobs).is_empty());
}

#[test]
fn check_inventory_fails_loudly_on_a_step_state_mismatch() {
    let discovered = vec![WorkflowJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
    }];
    let manifest = vec![ManifestJob {
        workflow: "ci.yml".into(),
        job: "build-test-clippy".into(),
        status: "github-only".into(),
        migration_step: 4,
        migration_state: "capability-smoke".into(),
        myelin_job: String::new(),
        note: "test".into(),
    }];
    let errors = check_inventory(&discovered, &manifest, &BTreeSet::new());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("does not belong to migration_step")),
        "must flag the step/state mismatch, got: {errors:?}"
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

#[test]
fn parse_myelin_job_names_finds_job_names() {
    let source = "schema_version = 2\non = \"push\"\n\n[[jobs]]\nname = \"build\"\nimage = \"x\"\n";
    let jobs = parse_myelin_job_names(source);
    assert_eq!(jobs, ["build".to_string()].into_iter().collect());
}

#[test]
fn parse_myelin_job_names_does_not_admit_a_name_field_outside_jobs() {
    let source = "schema_version = 2\non = \"push\"\n\n[execution]\nname = \"not-a-real-job\"\nprofile = \"linux-small-v1\"\n\n[[jobs]]\nname = \"build\"\nimage = \"x\"\n";
    let jobs = parse_myelin_job_names(source);
    assert_eq!(
        jobs,
        ["build".to_string()].into_iter().collect(),
        "the decoy `[execution]` name must NOT be admitted as a job name"
    );
}
