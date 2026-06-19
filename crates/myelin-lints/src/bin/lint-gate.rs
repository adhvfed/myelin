//! `lint-gate` — the committed CI entrypoint for the twelve architecture lints (EB-07 → P-019).
//!
//! This binary IS the loud, never-swallowed gate the CI workflow runs (`.github/workflows/ci.yml`,
//! the `architecture-lints` job). It scans every `*.rs` file under one or more roots with all
//! twelve lints ([`myelin_lints::all_twelve`]) and **exits NON-ZERO on any violation** — there is
//! no `... || true` swallow path possible, because the gate is the process exit code itself
//! (doctrine EI-01 §5: "an uncommitted gate is no gate; make violations loud").
//!
//! Usage:
//!   `lint-gate [ROOT ...]`  — scan each ROOT's `*.rs` files. With no ROOT, scans the workspace's
//!   own `crates/*/src` tree (the live workspace gate). Prints every violation to stderr and exits
//!   1 if any are found, 0 if the tree is clean.
//!
//! Why a binary (the EB-07 "wired into CI, loud, never swallowed" obligation): the substrate
//! prompts P-017/P-018 shipped the lints + the `cargo test` matrix/workspace-scan gate, but EB-07
//! requires the lint be wired into CI such that **the workflow fails with a non-zero exit on a red
//! fixture, with no `|| true` swallow**. A process whose exit code IS the gate cannot be silently
//! swallowed by a shell `||`; the `ci_gate_fails_loudly` test (`tests/ci_gate.rs`) proves the
//! red-fixture run exits non-zero and the clean tree exits zero.

use myelin_lints::engine::run;
use myelin_lints::lints::all_twelve;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Documented, LOUD exclusions — mirrors `tests/workspace_clean.rs` (named, never silent skips,
/// EI-01 §4). The relay is the one legitimate broker-publish site; the lint crate carries the
/// forbidden tokens as scanner data; test/fixture trees deliberately hold red samples.
///
/// The `myelin-harness/src/bin/*-scorecard.rs` band-boundary exit-gate RUNNERS (the SUB-M0 runner
/// `sub-m0-scorecard.rs`, P-S24 → P-039; the Identity M1→M2 runner `id-m1-scorecard.rs`, P-ID-21 →
/// P-079; the infra integration runner `infra-scorecard.rs`, Stage 4) are CI/test-support
/// ORCHESTRATION tooling (the leaf test-support crate `myelin-harness`,
/// NOT a node in the production DAG, architecture §2.9) whose whole job is to spawn `cargo
/// test`/`cargo run` for each per-feature drill and aggregate the result. Their
/// `Command::new(env!("CARGO"))` is the one legitimate host-exec site, exactly analogous to the
/// relay's one legitimate broker-publish site: it is developer/CI tooling, never reachable on a
/// user/agent request path, so the `no-host-exec` sandbox-escape rule (which guards PLATFORM code)
/// does not apply. These are NAMED, LOUD exclusions of single tool files — the lint stays fully
/// live on every production crate; it is NOT weakened. (The production execution seam
/// `ToolHands::exec` lands in M2/CI.)
const EXCLUDED_SUBSTRINGS: &[&str] = &[
    "myelin-events/src/relay.rs",
    // The OLTP-co-located RELAY (Stage 2 / infra): `PgRelay::relay_once` drains the co-located
    // outbox table and is the ONE legitimate broker-publish site for the OLTP service — exactly
    // the role relay.rs plays for the in-process floor (BUS-2: the relay is the only
    // broker-publish component). Its `bus.put(...)` forwards an ALREADY-committed outbox row
    // (emit-iff-committed), not a fire-and-forget bypass; its outbox queries are relay-INTERNAL
    // (the outbox is keyed by (aggregate, seq) and drained across aggregates), NOT tenant-store
    // queries — the same posture as relay.rs. NAMED, LOUD exclusion (see the crate note in
    // pgrelay.rs), never a silent skip; the tenant-store code in pg.rs stays fully linted.
    "myelin-storage/src/pgrelay.rs",
    "myelin-harness/src/bin/sub-m0-scorecard.rs",
    "myelin-harness/src/bin/id-m1-scorecard.rs",
    // The infra integration exit-gate runner (Stage 4): same posture as the two runners above —
    // its `Command::new(env!("CARGO"))` spawns `cargo test --features integration` per drill, the
    // one legitimate host-exec site for a CI/test-support orchestration binary. NAMED, LOUD
    // exclusion of a single tool file; the lint stays fully live on every production crate.
    "myelin-harness/src/bin/infra-scorecard.rs",
    "myelin-lints/",
    "/tests/",
    "/fixtures/",
];

fn is_excluded(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    EXCLUDED_SUBSTRINGS.iter().any(|ex| s.contains(ex))
}

/// The workspace's own `crates/*/src` tree (the default scan root when no arg is given).
fn default_roots() -> Vec<PathBuf> {
    // CARGO_MANIFEST_DIR = crates/myelin-lints; the workspace root is two levels up.
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
        // An EXPLICITLY-passed file is scanned regardless of extension (the fixtures the CI-gate
        // self-test points at are `*.rs.txt`). A DIRECTORY walk still only picks up `*.rs`.
        out.push(root.to_path_buf());
    } else {
        collect_rs(root, &mut out);
    }
    out
}

fn main() -> ExitCode {
    // Args after argv[0] are scan roots; default to the workspace crates tree. A `--no-exclude`
    // flag (used by the CI-gate self-test over a red fixture) disables the by-design exclusions so
    // a fixture under a `/fixtures/` path is actually scanned.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let no_exclude = args.iter().any(|a| a == "--no-exclude");
    let roots: Vec<PathBuf> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .collect();
    let roots = if roots.is_empty() { default_roots() } else { roots };

    let lints = all_twelve();
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
            if let Err(found) = run(&lints, &src) {
                for v in found {
                    violations.push(format!("{}: {v}", file.display()));
                }
            }
            scanned += 1;
        }
    }

    if violations.is_empty() {
        eprintln!("lint-gate: OK — {scanned} file(s) scanned, 0 violations.");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "lint-gate: FAIL — {} violation(s) in {scanned} file(s) (loud, never swallowed — fix \
             the code, do not weaken the lint):",
            violations.len()
        );
        for v in &violations {
            eprintln!("  {v}");
        }
        ExitCode::FAILURE
    }
}
