use crate::engine::{LintId, Violation};
use crate::erosion::test_line_ranges;
use crate::lints::{NO_HOST_EXEC, NO_RAW_CI_VERDICT};
use std::path::Path;

// The single scan-exclusion policy for lint-gate and its workspace tests.
pub const EXCLUDED_SUBSTRINGS: &[&str] = &[
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
    "myelin-ci-sandbox/src/firecracker.rs",
    "myelin-agent-model/src/",
    "myelin-lints/",
    "/tests/",
    "/fixtures/",
];

pub const PER_LINT_EXCLUSIONS: &[(LintId, &[&str])] = &[
    (
        NO_HOST_EXEC,
        &[
            "myelin-ci-sandbox/src/launch_gate.rs",
            "myelin-ci-sandbox/src/workspace_storage.rs",
            "myelin-ci-sandbox/src/rootfs_overlay.rs",
            "myelin-ci-sandbox/src/gvisor/cgroup.rs",
            "myelin-ci-sandbox/src/gvisor/explicit_userns.rs",
            "myelin-ci-sandbox/src/gvisor/checkout_preparation.rs",
            "myelin-ci-sandbox/src/gvisor/preflight.rs",
            "myelin-ci-sandbox/src/gvisor/teardown.rs",
            "myelin-ci-sandbox/src/gvisor/output_capture.rs",
        ],
    ),
    (
        NO_RAW_CI_VERDICT,
        &[
            "myelin-flow/src/executor.rs",
            "myelin-flow/src/pg_executor.rs",
            "myelin-ci-controlplane/src/ci_pipeline_driver.rs",
        ],
    ),
];

pub fn is_excluded(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    EXCLUDED_SUBSTRINGS.iter().any(|ex| s.contains(ex))
}

pub fn is_excluded_for_lint(lint: LintId, path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    PER_LINT_EXCLUSIONS
        .iter()
        .any(|(id, subs)| *id == lint && subs.iter().any(|ex| s.contains(ex)))
}

// Lints cover production code; `/tests/` is excluded above and inline
// `#[cfg(test)]` items are excluded here, span-accurate via syn. A file that
// fails to parse keeps every violation (fail loud, not silently clean).
pub fn drop_test_span_violations(src: &str, violations: Vec<Violation>) -> Vec<Violation> {
    if violations.is_empty() {
        return violations;
    }
    let Ok(ranges) = test_line_ranges(src) else {
        return violations;
    };
    violations
        .into_iter()
        .filter(|v| !ranges.iter().any(|(start, end)| v.line >= *start && v.line <= *end))
        .collect()
}
