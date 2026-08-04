use myelin_lints::lints::{all_twelve, NO_HOST_EXEC};
use myelin_lints::LintId;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest dir")
        .to_path_buf()
}

const EXCLUDED_SUBSTRINGS: &[&str] = &[
    "myelin-events/src/relay.rs",
    "myelin-storage/src/pgrelay.rs",
    "myelin-storage/src/events_durable.rs",
    "myelin-storage/src/placement_durable.rs",
    "myelin-storage/src/kms_durable.rs",
    "myelin-storage/src/cell_root_durable.rs",
    "myelin-storage/src/pg_migrator.rs",
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

fn is_excluded(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    EXCLUDED_SUBSTRINGS.iter().any(|ex| s.contains(ex))
}

const PER_LINT_EXCLUSIONS: &[(LintId, &[&str])] = &[(
    NO_HOST_EXEC,
    &[
        "myelin-ci-sandbox/src/launch_gate.rs",
        "myelin-ci-sandbox/src/workspace_storage.rs",
    ],
)];

fn is_excluded_for_lint(lint: LintId, path: &Path) -> bool {
    let path = path.to_string_lossy().replace('\\', "/");
    PER_LINT_EXCLUSIONS
        .iter()
        .any(|(id, excluded)| *id == lint && excluded.iter().any(|item| path.contains(item)))
}

fn rust_source_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let crates_dir = root.join("crates");
    let entries = std::fs::read_dir(&crates_dir).expect("crates/ must exist");
    for crate_entry in entries.flatten() {
        let src = crate_entry.path().join("src");
        if src.is_dir() {
            collect_rs(&src, &mut out);
        }
    }
    out
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

#[test]
fn the_twelve_lints_are_clean_over_the_workspace_source() {
    let root = workspace_root();
    let lints = all_twelve();
    let mut all_violations = Vec::new();
    let mut scanned = 0usize;

    for file in rust_source_files(&root) {
        if is_excluded(&file) {
            continue;
        }
        let src = std::fs::read_to_string(&file).expect("readable source file");
        for lint in &lints {
            if is_excluded_for_lint(lint.id, &file) {
                continue;
            }
            for v in lint.run(&src) {
                all_violations.push(format!("{}: {v}", file.display()));
            }
        }
        scanned += 1;
    }

    assert!(
        scanned >= 8,
        "expected to scan the workspace src tree (>= 8 files), scanned {scanned}"
    );

    assert!(
        all_violations.is_empty(),
        "the twelve architecture lints found violations in workspace source \
         (loud, never swallowed - fix the code, do not weaken the lint):\n{}",
        all_violations.join("\n")
    );
}
