use myelin_lints::dependency_direction::scan_dependency_directions;
use myelin_lints::erosion::{parse_budget, scan_workspace};
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest dir")
        .to_path_buf()
}

#[test]
fn erosion_budget_is_green_over_the_real_workspace() {
    let root = workspace_root();
    let source = std::fs::read_to_string(root.join("erosion-budget.toml"))
        .expect("erosion-budget.toml must stay checked in at the workspace root");
    let budget = parse_budget(&source).expect("erosion budget parses");
    let errors = scan_workspace(&root, &budget);
    assert!(
        errors.is_empty(),
        "erosion budget violations (shrink-only; lower a stale allowance, never raise one):\n{}",
        errors.join("\n")
    );
}

#[test]
fn dependency_direction_is_green_over_the_real_workspace() {
    let root = workspace_root();
    let errors = scan_dependency_directions(&root);
    assert!(
        errors.is_empty(),
        "core-crate dependency direction violations:\n{}",
        errors.join("\n")
    );
}
