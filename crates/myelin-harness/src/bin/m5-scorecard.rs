//! The **M5 world-scale-hardening exit-gate scorecard runner** (M5 → M6) — the band-boundary
//! go/no-go binary that declares world-scale readiness.
//!
//! Runs EVERY required M5 gate row's proof command (the per-feature surge / world-scale / DSR / E2E
//! drill that emits its dated green artifact), records PASS / claimed-not-proven into a
//! [`Scorecard`], writes the dated artifact to `testing/scorecards/m5-world.md`, prints each row
//! LOUD to stdout, and **exits non-zero if the gate is RED** (a missing row OR any claimed-not-proven
//! row blocks M6 — the gate invariant, master-sequencing §2 / EI-01 §2). There is deliberately NO
//! `|| true` / swallow path: a red row fails the job.
//!
//! This binary WIRES the existing M5 drills (it does not re-implement them): each row's proof command
//! is a `cargo test`/`cargo run` invocation that lives with its feature prompt (P-420..P-444). The
//! five families: the F6 30× surge family (all owners), Git world-scale (GIT-D4/D5), Knowledge
//! (KN-D1-re-green/KN-D8), multi-cell/DSR (GA-D1/GA-D8/CP-D7/CP-D8), and the four whole-system E2E
//! scenarios (E2E-1..E2E-4), plus the permanent STOR-D2 cell-scale restore gate and the
//! contract-coverage re-affirm.
//!
//! ## The STOR-D2 cell-scale restore gate needs the LIVE stack
//! STOR-D2-cell re-confirms the permanent restore gate at cell scale under REAL world-scale generated
//! load; bring the live docker-compose stack up first (`scripts/integration-test.sh` or
//! `docker compose -f docker-compose.dev.yml up -d --wait`). It does NOT need /dev/kvm (that was
//! AG-D4-specific) and does NOT pass `--features integration` (the cell-scale drill drives the
//! harness gates directly).
//!
//! ## The honesty framing (printed in the rendered artifact, NOT rows that red this gate)
//! - **The world-scale 30× surge family is a SINGLE-BOX SCALED drill** — the shed-order /
//!   lane-priority / cross-tenant-isolation LOGIC is exercised and green; the **true multi-node FLEET
//!   proof** (30× fan-out across a real multi-box cluster) is the ONE genuine named floor, needing
//!   real fleet hardware this dev host lacks. The drill proves the mechanism; the fleet residual is
//!   NAMED, never faked green (EI-01 §1).
//! - **STOR-D2 at cell scale** is the permanent restore gate (a backup never restored is not a
//!   backup, EI-01 §3) — re-run-forever.
//! - **Carried-forward floor (M7):** a real `JobSpec.command` does not yet flow through the
//!   production sandbox `launch()` on either backend — filled by M7 P-544/P-545, named here.
//! - **Measured-trigger-gated floors (M4-C1 / M4-C2 / OQ-L):** each ships its seam + named follow-on,
//!   promoted only on its measured trigger; not a row that reds this gate.
//!
//! Usage: `cargo run -p myelin-harness --bin m5-scorecard` (with the live docker-compose stack up so
//! the STOR-D2 cell-scale restore gate reaches the backends).

use myelin_harness::scorecard::{m5_required_rows, today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::M5World);

    println!(
        "== M5 world-scale-hardening exit-gate scorecard ({date}) — re-running every surge / world-scale / DSR / E2E drill =="
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
                println!("RED — {reason}");
                card.record(RowResult::claimed_not_proven(row.id, reason, &date));
            }
        }
    }

    // Write the dated artifact (the committed scorecard body). A write failure is a loud error,
    // never a silent skip.
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
        println!("\nGATE: GREEN — every M5 world-scale drill proven-and-dated (the F6 surge family + GIT-D4/D5 + KN-D1-re-green/KN-D8 + GA-D1/GA-D8/CP-D7/CP-D8 + E2E-1..E2E-4 + the STOR-D2 cell-scale restore gate); world-scale readiness declared, M6 may start.");
        println!("       (the ONE true remaining floor — the true multi-node FLEET proof of the 30× surge on real fleet hardware — is a named, dated deferral by design; the single-box SCALED drill proves the mechanism.)");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED — M6 is BLOCKED (the M5→M6 world-scale-hardening go/no-go is red).");
        for missing in card.missing_required() {
            eprintln!("  MISSING required row: {missing}");
        }
        for red in card.not_proven() {
            eprintln!("  claimed-not-proven: {}", red.id);
        }
        ExitCode::FAILURE
    }
}

/// Run one proof command via `cargo <args>`. `Ok(())` iff cargo exited 0; otherwise an `Err` naming
/// the non-zero exit (the claimed-not-proven reason). LOUD: the child's output is inherited so the
/// failing drill's own red artifact prints. No `|| true` / swallow path — a non-zero exit is a RED
/// row.
fn run_proof(args: &[&str]) -> Result<(), String> {
    let status = Command::new(env!("CARGO"))
        .args(args)
        .status()
        .map_err(|e| format!("could not spawn `cargo {}`: {e}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "`cargo {}` exited non-zero ({status}) — the drill read RED \
             (if this is the STOR-D2 cell-scale row, the live docker-compose stack must be up)",
            args.join(" ")
        ))
    }
}

/// The committed scorecard path: `<workspace-root>/testing/scorecards/m5-world.md`. The workspace
/// root is the parent of this crate's manifest dir (`crates/myelin-harness` → up two).
fn scorecard_path() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&crate_dir)
        .to_path_buf();
    root.join("testing").join("scorecards").join("m5-world.md")
}
