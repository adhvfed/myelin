use myelin_harness::scorecard::{m5_required_rows, today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::M5World);

    println!(
        "== M5 world-scale-hardening exit-gate scorecard ({date}) - re-running every surge / world-scale / DSR / E2E drill =="
    );
    println!("   (the F6 30× surge family runs as a SINGLE-BOX SCALED drill; the true multi-node FLEET proof is the ONE named floor. STOR-D2 at cell scale needs the live docker-compose stack up.)\n");
    for row in m5_required_rows() {
        print!("  {} … ", row.id);
        match run_proof(row.proof_command) {
            Ok(()) => {
                let proof = format!("[{date}] PASS  `cargo {}`", row.proof_command.join(" "));
                println!("PASS");
                card.record(RowResult::pass(row.id, proof, &date));
            }
            Err(reason) => {
                println!("RED - {reason}");
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
        println!("\nGATE: GREEN - every M5 world-scale drill proven-and-dated (the F6 surge family + GIT-D4/D5 + KN-D1-re-green/KN-D8 + GA-D1/GA-D8/CP-D7/CP-D8 + E2E-1..E2E-4 + the STOR-D2 cell-scale restore gate); world-scale readiness declared, M6 may start.");
        println!("       (the ONE true remaining floor - the true multi-node FLEET proof of the 30× surge on real fleet hardware - is a named, dated deferral by design; the single-box SCALED drill proves the mechanism.)");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED - M6 is BLOCKED (the M5→M6 world-scale-hardening go/no-go is red).");
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
            "`cargo {}` exited non-zero ({status}) - the drill read RED \
             (if this is the STOR-D2 cell-scale row, the live docker-compose stack must be up)",
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
    root.join("testing").join("scorecards").join("m5-world.md")
}
