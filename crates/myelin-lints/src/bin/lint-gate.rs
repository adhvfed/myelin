use myelin_lints::lints::{all_twelve, no_raw_ci_verdict};
use myelin_lints::policy::{drop_test_span_violations, is_excluded, is_excluded_for_lint};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
                for v in drop_test_span_violations(&src, lint.run(&src)) {
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
