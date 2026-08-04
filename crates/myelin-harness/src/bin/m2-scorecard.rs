use myelin_harness::scorecard::{m2_required_rows, today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::M2Reactive);

    println!(
        "== M2 reactive-shared-layer exit-gate scorecard ({date}) - re-running every M2 drill =="
    );
    println!("   (AG-D4 runs --features integration with MYELIN_REQUIRE_KVM=1: a real microVM MUST boot - no vacuous green)\n");
    for row in m2_required_rows() {
        let require_kvm = row.id == "AG-D4";
        let tag = if require_kvm {
            " [AG-D4 keystone - MYELIN_REQUIRE_KVM=1: a real microVM must boot]"
        } else {
            ""
        };
        print!("  {} … ", row.id);
        match run_proof(row.proof_command, require_kvm) {
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
        println!("\nGATE: GREEN - every M2 reactive-layer drill proven-and-dated (incl. AG-D4 on a real microVM); M3 may start.");
        println!("       (the ONE true remaining floor - the world-scale 30× LOAD drill on real fleet hardware - is deferred to M5 by design.)");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED - M3 is BLOCKED (the M2→M3 reactive-layer go/no-go is red).");
        for missing in card.missing_required() {
            eprintln!("  MISSING required row: {missing}");
        }
        for red in card.not_proven() {
            eprintln!("  claimed-not-proven: {}", red.id);
        }
        ExitCode::FAILURE
    }
}

fn run_proof(args: &[&str], require_kvm: bool) -> Result<(), String> {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(args);
    if require_kvm {
        cmd.env("MYELIN_REQUIRE_KVM", "1");
        cmd.arg("--").arg("--nocapture");
    }
    let status = cmd
        .status()
        .map_err(|e| format!("could not spawn `cargo {}`: {e}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else if require_kvm {
        Err(format!(
            "`cargo {}` (MYELIN_REQUIRE_KVM=1) exited non-zero ({status}) - the AG-D4 real-kernel \
             escape drill did NOT boot a microVM / did NOT attest 0 escapes (a vacuous green is refused)",
            args.join(" ")
        ))
    } else {
        Err(format!(
            "`cargo {}` exited non-zero ({status}) - the drill read RED",
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
        .join("m2-reactive.md")
}
