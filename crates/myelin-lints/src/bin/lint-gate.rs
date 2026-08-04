use myelin_lints::lints::{all_twelve, no_raw_ci_verdict, NO_HOST_EXEC, NO_RAW_CI_VERDICT};
use myelin_lints::LintId;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const EXCLUDED_SUBSTRINGS: &[&str] = &[
    "myelin-events/src/relay.rs",
    "myelin-storage/src/pgrelay.rs",
    "myelin-storage/src/events_durable.rs",
    "myelin-storage/src/pg_migrator.rs",
    "myelin-storage/src/placement_durable.rs",
    "myelin-storage/src/kms_durable.rs",
    "myelin-storage/src/cell_root_durable.rs",
    "myelin-events/src/firehose.rs",
    "myelin-knowledge/src/transport.rs",
    "myelin-ci-controlplane/src/log_pipeline.rs",
    "myelin-ci-controlplane/src/job_queue_region.rs",
    "myelin-ci-controlplane/src/ci_run_region.rs",
    "myelin-ci-controlplane/src/ci_scheduler_db.rs",
    "myelin-chat-gateway/src/delivery.rs",
    "myelin-harness/src/bin/sub-m0-scorecard.rs",
    "myelin-harness/src/bin/id-m1-scorecard.rs",
    "myelin-harness/src/bin/infra-scorecard.rs",
    "myelin-harness/src/bin/m2-scorecard.rs",
    "myelin-harness/src/bin/m3-scorecard.rs",
    "myelin-harness/src/bin/m4-scorecard.rs",
    "myelin-harness/src/bin/m5-scorecard.rs",
    "myelin-harness/src/bin/m6-scorecard.rs",
    "myelin-harness/src/bin/make-it-real-scorecard.rs",
    "myelin-harness/src/bin/self-hosting-ci.rs",
    "myelin-harness/src/self_hosting_ci.rs",
    "myelin-ci-sandbox/src/firecracker.rs",
    "myelin-ci-sandbox/src/gvisor.rs",
    "myelin-agent-model/src/",
    "myelin-lints/",
    "/tests/",
    "/fixtures/",
];

const PER_LINT_EXCLUSIONS: &[(LintId, &[&str])] = &[
    (NO_HOST_EXEC, &["myelin-ci-sandbox/src/launch_gate.rs"]),
    (NO_HOST_EXEC, &["myelin-ci-sandbox/src/workspace_storage.rs"]),
    (
        NO_HOST_EXEC,
        &[
            "myelin-ci-sandbox/src/gvisor/cgroup.rs",
            "myelin-ci-sandbox/src/gvisor/explicit_userns.rs",
        ],
    ),
    (NO_HOST_EXEC, &["myelin-ci-sandbox/src/rootfs_overlay.rs"]),
    (
        NO_RAW_CI_VERDICT,
        &[
            "myelin-flow/src/executor.rs",
            "myelin-flow/src/pg_executor.rs",
            "myelin-ci-controlplane/src/ci_pipeline_driver.rs",
        ],
    ),
];

fn is_excluded_for_lint(lint: LintId, path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    PER_LINT_EXCLUSIONS
        .iter()
        .any(|(id, subs)| *id == lint && subs.iter().any(|ex| s.contains(ex)))
}

fn is_excluded(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    EXCLUDED_SUBSTRINGS.iter().any(|ex| s.contains(ex))
}

fn default_roots() -> Vec<PathBuf> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    vec![workspace.join("crates")]
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if root.is_file() {
        out.push(root.to_path_buf());
    } else {
        collect_rs(root, &mut out);
    }
    out
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let no_exclude = args.iter().any(|a| a == "--no-exclude");
    let roots: Vec<PathBuf> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .collect();
    let roots = if roots.is_empty() {
        default_roots()
    } else {
        roots
    };

    let mut lints = all_twelve();
    lints.push(no_raw_ci_verdict());
    let mut violations = Vec::new();
    let mut scanned = 0usize;

    for root in &roots {
        for file in rust_files(root) {
            if !no_exclude && is_excluded(&file) {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&file) else {
                continue;
            };
            for lint in &lints {
                if is_excluded_for_lint(lint.id, &file) {
                    continue;
                }
                for v in lint.run(&src) {
                    violations.push(format!("{}: {v}", file.display()));
                }
            }
            scanned += 1;
        }
    }

    if violations.is_empty() {
        eprintln!("lint-gate: OK - {scanned} file(s) scanned, 0 violations.");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "lint-gate: FAIL - {} violation(s) in {scanned} file(s) (loud, never swallowed - fix \
             the code, do not weaken the lint):",
            violations.len()
        );
        for v in &violations {
            eprintln!("  {v}");
        }
        ExitCode::FAILURE
    }
}
