use myelin_harness::scorecard::{infra_required_rows, today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::Infra);

    println!("== Infra integration exit-gate scorecard ({date}) - running every --features integration drill ==");
    println!("   (red-until-proven: the live docker-compose stack must be up; run via scripts/integration-test.sh)\n");
    for row in infra_required_rows() {
        let tag = if row.floor.is_some() {
            " [floor smoke - the named floor stays open]"
        } else {
            ""
        };
        print!("  {} … ", row.id);
        match run_proof(row.proof_command) {
            Ok(()) => {
                let proof = format!("[{date}] PASS  `cargo {}`", row.proof_command.join(" "));
                println!("PASS{tag}");
                card.record(RowResult::pass(row.id, proof, &date));
            }
            Err(reason) => {
                println!("RED{tag} - {reason}");
                card.record(RowResult::claimed_not_proven(row.id, reason, &date));
            }
        }
    }

    let artifact = card.render_markdown(&date);
    let path = scorecard_path();
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
    println!("\nscorecard written to {}", path.display());

    if card.is_green() {
        println!("\nGATE: GREEN - every infra integration drill proven against the live stack.");
        println!("       (the two named floors - real-kernel SANDBOX-ESCAPE + WORLD-SCALE 30× - stay open by design.)");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED - the infra integration gate is red.");
        eprintln!("       (if every row failed to connect, the live stack is not up: run scripts/integration-test.sh)");
        for missing in card.missing_required() {
            eprintln!("  MISSING required row: {missing}");
        }
        for red in card.not_proven() {
            eprintln!("  claimed-not-proven: {}", red.id);
        }
        ExitCode::FAILURE
    }
}

fn run_proof(args: &[&str]) -> Result<(), String> {
    let status = Command::new(env!("CARGO"))
        .args(args)
        .status()
        .map_err(|e| format!("could not spawn `cargo {}`: {e}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "`cargo {}` exited non-zero ({status}) - the drill read RED (stack down? run scripts/integration-test.sh)",
            args.join(" ")
        ))
    }
}

fn scorecard_path() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&crate_dir)
        .to_path_buf();
    root.join("testing").join("scorecards").join("infra.md")
}
