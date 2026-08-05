use myelin_lints::lints::all_twelve;
use myelin_lints::policy::{drop_test_span_violations, is_excluded, is_excluded_for_lint};
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest dir")
        .to_path_buf()
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
            for v in drop_test_span_violations(&src, lint.run(&src)) {
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
