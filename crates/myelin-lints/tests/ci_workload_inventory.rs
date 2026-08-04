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
//! plus a structured `migration_step`/`migration_state` pair recording real progress.
//!
//! 2026-07-25 revision (adversarial review by gpt-5.6-sol): the original schema had a single
//! free-text `owner` field ("CT-007 step 2 (...) — not started") that the test never actually
//! examined — any string, including an empty or meaningless one, would have passed. That
//! mechanically validates nothing; it is prose, not a gate. `owner` is replaced by two structured,
//! closed-vocabulary fields the test DOES examine: `migration_step` (an integer 1-4, matching the
//! ledger's four cutover-floor gates) and `migration_state` (a closed enum recording real,
//! currently-true progress, never a plan). This also fixes a genuine drift the old free-text field
//! let slip through silently: `build-test-clippy`'s row said "in progress" in prose while the file's
//! own header comment still said "none of steps 2-4 started" — a contradiction no test caught.
//!
//! The gate, in multiple directions:
//!   - every job id discovered by scanning `.github/workflows/*.yml`'s `jobs:` block MUST have
//!     exactly one manifest row (else a job silently drops out of the inventory — the exact
//!     defect the ledger's gate exists to catch);
//!   - every manifest row MUST name a job that still actually exists in the workflow files (else
//!     the inventory drifts into a stale, untrue claim);
//!   - `migration_step` must be 1-4 and `migration_state` must be one of the closed enum values;
//!   - `status = "github-only"` rows must carry a migration_state that has NOT yet reached
//!     "graph-passing"/"cutover-repeated" (those imply the row should already be `myelin-native`)
//!     and must carry an empty `myelin_job`;
//!   - `status = "myelin-native"` rows must name a non-empty `myelin_job` that is mechanically
//!     verified to actually exist as a `[[jobs]] name = "..."` entry in the real `.myelin/ci.toml`
//!     — not merely a plausible-looking string.
//!
//! `check_inventory` carries the cross-referencing logic and is unit-tested directly with synthetic
//! RED/GREEN data below (matching how `crates/myelin-lints/src/erosion.rs` unit-tests its scan
//! function in-process) — the real-workspace test below cannot easily inject a fake missing/stale
//! job without a temp-file fixture, so the shared checking function itself is proven to fire instead.

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

/// States that imply real Myelin-native execution has been reached — a `github-only` row must
/// never claim one of these (it would mean the row should already be `myelin-native`). Added
/// `job-passing` 2026-07-25 (gpt-5.6-sol, second adversarial round): the original two-state split
/// (`graph-passing`/`cutover-repeated` only) could not honestly represent "this ONE job's real
/// vertical slice passes through the real dispatch path" while the OTHER eleven jobs are still
/// GitHub-only — `graph-passing` means the COMPLETE mapped graph (ledger step 3), which a single
/// migrated job does not yet satisfy. `job-passing` fills that gap.
const MYELIN_NATIVE_STATES: &[&str] = &["job-passing", "graph-passing", "cutover-repeated"];

/// Which `migration_step` values a `migration_state` may legitimately pair with — a row cannot
/// claim, say, `migration_step = 4` while `migration_state = "capability-smoke"` (that combination
/// previously passed unchecked). Returns `true` if the pairing is consistent.
fn state_matches_step(state: &str, step: u32) -> bool {
    match state {
        "not-started" => matches!(step, 2..=4),
        "capability-smoke" | "capability-proven" => step == 2,
        "job-passing" | "graph-passing" => step == 3,
        "cutover-repeated" => step == 4,
        _ => false, // an already-unrecognized state is reported separately; don't double-report
    }
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
///
/// An ABSENT `.github/workflows/` is the CT-007 end state: once the self-hosted cutover completes
/// and GitHub Actions is disabled, the directory is removed, so there are zero GitHub jobs to
/// reconcile — that is a valid completed migration, not a read error.
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

/// One `[[jobs]]` entry in a Myelin-native `ci.toml`-shaped source — only the field this test
/// cares about is modeled; `#[serde(deny_unknown_fields)]` is deliberately NOT used here since this
/// test only needs to read `name`, not validate the full `.myelin/ci.toml` schema (that belongs to
/// `crates/myelin-ci-dispatch`'s own config parser).
#[derive(Debug, Deserialize)]
struct MyelinJob {
    name: String,
}

#[derive(Debug, Deserialize)]
struct MyelinManifest {
    #[serde(default)]
    jobs: Vec<MyelinJob>,
}

/// Parse every `[[jobs]] name = "..."` entry out of a Myelin-native `ci.toml`-shaped source using
/// real TOML parsing (not line-scanning) — 2026-07-25 fix (gpt-5.6-sol, second adversarial round):
/// the original line-scanning version accepted ANY `name = "..."` line ANYWHERE in the file, not
/// just inside `[[jobs]]` tables, so a `name` field under a different section (or added later
/// elsewhere in `.myelin/ci.toml`) would have been silently admitted as if it were a real job name.
fn parse_myelin_job_names(source: &str) -> BTreeSet<String> {
    let manifest: MyelinManifest =
        toml::from_str(source).expect("parse .myelin/ci.toml-shaped source");
    manifest.jobs.into_iter().map(|job| job.name).collect()
}

/// The shared gate logic: every discovered GitHub job must have exactly one manifest row, every
/// manifest row must name a job that still actually exists among the discovered jobs, no
/// (workflow, job) pair may have more than one manifest row, every row's structured fields must be
/// internally consistent, and a `myelin-native` row's `myelin_job` must actually exist among
/// `myelin_jobs`. Returns one error string per violation (empty ⇒ green).
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
                "GitHub job `{}` (workflow `{}`) has NO ci-workload-inventory.toml entry — a job \
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
            // Guarded on migration_step already being in-range so an out-of-range step reports
            // only its own dedicated error above, not a confusing second one here.
            errors.push(format!(
                "{label}: migration_state `{}` does not belong to migration_step {} — \
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
                         Myelin-native execution — either the state is premature or status should \
                         already be `myelin-native`",
                        entry.migration_state
                    ));
                }
                if !entry.myelin_job.is_empty() {
                    errors.push(format!(
                        "{label}: status is `github-only` but names a myelin_job `{}` — either \
                         the status is stale or the myelin_job claim is premature",
                        entry.myelin_job
                    ));
                }
            }
            "myelin-native" => {
                if entry.myelin_job.is_empty() {
                    errors.push(format!(
                        "{label}: status is `myelin-native` but myelin_job is empty — must name \
                         the real .myelin/ci.toml job that carries this workload"
                    ));
                } else if !myelin_jobs.contains(&entry.myelin_job) {
                    errors.push(format!(
                        "{label}: myelin_job `{}` does not exist in .myelin/ci.toml — a \
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

    // CT-007 END STATE: GitHub Actions is disabled and `.github/workflows/` removed, so there are
    // zero GitHub jobs to reconcile — the migration this inventory tracked is COMPLETE. The
    // drift-detection logic itself stays fully covered by the `check_inventory_*` unit tests below
    // (a silently-dropped job still fails loudly there). While `.github/workflows/` still exists
    // (mid-migration), every discovered job must still map to exactly one honest inventory entry.
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
    // A row cannot claim "graph-passing" progress while still being honestly `github-only` — that
    // combination means either the state is premature or the status is stale.
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
    // A `myelin-native` row naming a job that does NOT exist in .myelin/ci.toml is a fabricated
    // claim — this is the mechanical check that a plausible-looking string is not enough.
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
    // Uses load_real_myelin_jobs() against the ACTUAL .myelin/ci.toml (2026-07-25 fix, gpt-5.6-sol:
    // the earlier version only checked a synthetic BTreeSet, so this test never actually exercised
    // real membership positively — none of today's manifest rows is myelin-native, so the
    // integrated workspace test above never does either). `.myelin/ci.toml` really does define a
    // `[[jobs]] name = "build"` job today; if that ever changes, this test's failure is itself the
    // signal to update it.
    let real_myelin_jobs = load_real_myelin_jobs();
    assert!(
        real_myelin_jobs.contains("build"),
        "expected .myelin/ci.toml to define a `build` job today, found {real_myelin_jobs:?} — \
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
    // migration_step and migration_state must pair consistently — step 4 ("a second ordinary
    // commit repeats the graph") paired with "capability-smoke" (step 2's toolchain-execution
    // proof) is internally contradictory and previously passed unchecked.
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
    // 2026-07-25 regression test (gpt-5.6-sol, second adversarial round): the original
    // line-scanning parser accepted ANY `name = "..."` line anywhere in the file. Real TOML parsing
    // must NOT admit a decoy `name` field living outside the `[[jobs]]` array — e.g. under a
    // top-level `[execution]` table, as this synthetic fixture has.
    let source = "schema_version = 2\non = \"push\"\n\n[execution]\nname = \"not-a-real-job\"\nprofile = \"linux-small-v1\"\n\n[[jobs]]\nname = \"build\"\nimage = \"x\"\n";
    let jobs = parse_myelin_job_names(source);
    assert_eq!(
        jobs,
        ["build".to_string()].into_iter().collect(),
        "the decoy `[execution]` name must NOT be admitted as a job name"
    );
}
