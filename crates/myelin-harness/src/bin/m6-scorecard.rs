use myelin_harness::scorecard::{m6_required_rows, today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::M6SelfTenant);

    println!(
        "== M6 self-hosting exit-gate scorecard ({date}) - re-running every switch-test / self-hosting-CI / self_tenant / truth-up drill =="
    );
    println!("   (the switch tests are driven over the real surface with measured contrast + latency; the self-hosting CI graph is green on the platform's own commits. Bring the live docker-compose stack up first.)\n");
    for row in m6_required_rows() {
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
        println!("\nGATE: GREEN - every M6 self_tenant drill proven-and-dated (the switch tests + self-hosting-CI + the self_tenant drills + STOR-D37 restore-verify on Myelin's own commits + the truth-up pass); the platform is self_tenant-complete, M7 may start.");
        println!("       (M6 green is SELF_TENANT-COMPLETE, NOT production-ready - the M7 production floors, incl. sandbox prod-exec, are named dated deferrals filled by P-522..P-546, fail-closed at P-546.)");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED - M7 is BLOCKED (the M6→M7 self-hosting go/no-go is red).");
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
             (several M6 rows reach the backends; the live docker-compose stack must be up)",
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
    root.join("testing")
        .join("scorecards")
        .join("m6-self_tenant.md")
}
