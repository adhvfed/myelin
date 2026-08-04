use myelin_harness::self_hosting_ci::{run_graph, run_job_via_cargo, self_hosting_jobs};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let jobs = self_hosting_jobs();
    println!(
        "== Myelin self-hosting CI graph (the dogfood loop, SUB-M6) - running the substrate \
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
        println!("\nGATE: GREEN - the self-hosting CI graph is green on Myelin's own commit (SUB-M6 dogfood loop).");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "\nGATE: RED - the dogfood ratchet rejected this commit; red jobs: {}.",
            run.red_jobs().join(", ")
        );
        ExitCode::FAILURE
    }
}

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
