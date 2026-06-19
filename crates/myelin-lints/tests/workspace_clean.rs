//! The LIVE gate: run the four load-bearing lints over Myelin's OWN `crates/*/src` tree and
//! fail the build on ANY violation. This is what makes the lints a committed gate, not just a
//! fixture exercise — "an uncommitted gate is no gate" (EI-01 §5). The whole point of P-S10 is
//! that the four bug-classes are impossible to MERGE, so the lints must run on real code.
//!
//! Documented, LOUD exclusions (never silent skips — EI-01 §4):
//! - `myelin-events/src/relay.rs` — the relay is the ONE legitimate broker-publish component
//!   (it drains the outbox to the broker; everything else emits via OutboxTx). Excluding it from
//!   `no-raw-publish` is correct BY DESIGN; the exclusion is named here, not hidden.
//! - `myelin-lints/**` — this crate's own fixtures/test-helpers and the lint scanners themselves
//!   contain the forbidden tokens as DATA (the strings the scanner looks for). Scanning the lint
//!   crate would flag its own pattern lists. Excluded and named.
//! - `**/tests/**` and `**/fixtures/**` — test fixtures deliberately contain red samples.

use myelin_lints::engine::run;
use myelin_lints::lints::load_bearing_four;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/myelin-lints; the workspace root is two levels up.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest dir")
        .to_path_buf()
}

/// Documented exclusions (path substrings). A file whose path contains any of these is skipped
/// for a NAMED, by-design reason (see the module docs). Loud: the list is in source, reviewed.
const EXCLUDED_SUBSTRINGS: &[&str] = &[
    "myelin-events/src/relay.rs", // the one legitimate broker-publish component (by design).
    "myelin-lints/",              // this crate: scanners + fixtures carry the tokens as data.
    "/tests/",                    // test fixtures deliberately contain red samples.
    "/fixtures/",
];

fn is_excluded(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    EXCLUDED_SUBSTRINGS.iter().any(|ex| s.contains(ex))
}

/// Recursively collect every `*.rs` file under `crates/*/src`.
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
fn the_four_lints_are_clean_over_the_workspace_source() {
    let root = workspace_root();
    let lints = load_bearing_four();
    let mut all_violations = Vec::new();
    let mut scanned = 0usize;

    for file in rust_source_files(&root) {
        if is_excluded(&file) {
            continue;
        }
        let src = std::fs::read_to_string(&file).expect("readable source file");
        if let Err(violations) = run(&lints, &src) {
            for v in violations {
                all_violations.push(format!("{}: {v}", file.display()));
            }
        }
        scanned += 1;
    }

    // Sanity: we actually scanned the tree (a 0-file run would be a vacuous green — the
    // un-wired-gate failure mode EI-01 §5 warns about).
    assert!(
        scanned >= 8,
        "expected to scan the workspace src tree (>= 8 files), scanned {scanned}"
    );

    assert!(
        all_violations.is_empty(),
        "the four load-bearing architecture lints found violations in workspace source \
         (loud, never swallowed — fix the code, do not weaken the lint):\n{}",
        all_violations.join("\n")
    );
}
