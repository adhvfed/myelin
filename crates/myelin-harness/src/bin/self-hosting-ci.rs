//! The **Myelin self-hosting CI graph runner** (P-507 / P-S37 → M6) — the dogfood loop binary.
//!
//! Runs the substrate ratchet AS Myelin CI jobs on Myelin's OWN commit (HEAD): the twelve
//! architecture lints + the contract-coverage scanner + the mandatory-core cargo-mutants mutation
//! gate, then drives the substrate's surge/restore/migration drills (SUB-D3/D6/D10). Records each
//! job PASS / RED into a dated artifact (`testing/scorecards/self-hosting-ci.md`), prints each row
//! LOUD, and **exits non-zero if ANY job is red** — the gate IS the process exit code, so there is
//! no `... || true` swallow path. A deliberately-violating commit (a lint red, a surviving mutant,
//! a red drill) reds the graph: the ratchet rejects on Myelin's own work (the SUB-M6 exit gate).
//!
//! This binary WIRES the existing lints/scanner/drills (it does not re-implement them) and shells
//! out to `cargo` / `cargo mutants` exactly the way the band-boundary scorecard runners do — the
//! one legitimate host-exec site for CI orchestration tooling (named, loud; never on a user/agent
//! request path).
//!
//! Usage: `cargo run -p myelin-harness --bin self-hosting-ci`. CI runs it as the
//! `self-hosting-ci` job (the dogfood loop on every Myelin commit).

use myelin_harness::self_hosting_ci::{run_graph, run_job_via_cargo, self_hosting_jobs};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let jobs = self_hosting_jobs();
    println!(
        "== Myelin self-hosting CI graph (the dogfood loop, SUB-M6) — running the substrate \
         ratchet on Myelin's own commit =="
    );
    for job in &jobs {
        println!(
            "  scheduled: {:<24} [{}] {}",
            job.id,
            job.kind.label(),
            job.title
        );
    }
    println!();

    let run = run_graph(&jobs, &run_job_via_cargo);

    for r in &run.results {
        println!("{}", r.artifact_row(&run.date));
    }

    let artifact = run.render_markdown();
    let path = artifact_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("FATAL: could not create {}: {e}", parent.display());
            return ExitCode::FAILURE;
        }
    }
    if let Err(e) = std::fs::write(&path, &artifact) {
        eprintln!("FATAL: could not write {}: {e}", path.display());
        return ExitCode::FAILURE;
    }
    println!("\nself-hosting CI artifact written to {}", path.display());

    if run.is_green() {
        println!("\nGATE: GREEN — the self-hosting CI graph is green on Myelin's own commit (SUB-M6 dogfood loop).");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "\nGATE: RED — the dogfood ratchet rejected this commit; red jobs: {}.",
            run.red_jobs().join(", ")
        );
        ExitCode::FAILURE
    }
}

/// The committed artifact path: `<workspace-root>/testing/scorecards/self-hosting-ci.md`.
fn artifact_path() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&crate_dir)
        .to_path_buf();
    root.join("testing")
        .join("scorecards")
        .join("self-hosting-ci.md")
}
