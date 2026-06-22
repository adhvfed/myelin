//! The **M3 producer-subsystems exit-gate scorecard runner** (M3 → M4) — the band-boundary
//! go/no-go binary.
//!
//! Runs EVERY required M3 gate row's proof command (the per-feature Git-hosting / Knowledge drill
//! that emits its dated green artifact), records PASS / claimed-not-proven into a [`Scorecard`],
//! writes the dated artifact to `testing/scorecards/m3-producers.md`, prints each row LOUD to
//! stdout, and **exits non-zero if the gate is RED** (a missing row OR any claimed-not-proven row
//! blocks M4 — the gate invariant, master-sequencing §2 / EI-01 §2). There is deliberately NO
//! `|| true` / swallow path: a red row fails the job.
//!
//! This binary WIRES the existing M3 drills (it does not re-implement them): each row's proof
//! command is a `cargo test`/`cargo run` invocation that lives with its feature prompt
//! (P-246..P-318). The two families: Git hosting (GIT-D1/D2/D3/D7/D8/D9 + the receive-pack seam +
//! GIT-D10/D11 incl. the two integration legs) and Knowledge (KN-D1/D3/D4/D5/D6/D7/D9/D10/D11/D12/
//! D13), plus the contract-coverage re-affirm.
//!
//! ## The integration rows need the LIVE stack (RED-until-proven)
//! Six rows — GIT-D10, GIT-D11-int, KN-D5, KN-D7, KN-D9, KN-D10 — run `--features integration`
//! against the live docker-compose stack (Postgres / RustFS / Valkey / NATS JetStream). On a host
//! with the stack DOWN those proof commands FAIL, so the gate cannot read green from a DB-free run
//! (like the infra scorecard, these rows are RED-until-proven against the real backends). Bring the
//! stack up first (`scripts/integration-test.sh` or `docker compose up --wait`).
//!
//! ## Named floors (printed in the rendered artifact, NOT rows that red this gate)
//! - **KN-D3** the per-block CAS-merge NAMED FLOOR: the M3 deliverable proved the soft-lock +
//!   offline-reconcile floor; the full real-time CRDT/OT convergence is the named later follow-on.
//! - **The world-scale 30× LOAD surge** (real fleet hardware) is deferred to M5.
//!
//! Usage: `cargo run -p myelin-harness --bin m3-scorecard` (with the live docker-compose stack up).

use myelin_harness::scorecard::{m3_required_rows, today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::M3Producers);

    println!(
        "== M3 producer-subsystems exit-gate scorecard ({date}) — re-running every Git+Knowledge drill =="
    );
    println!("   (the GIT-D10/D11-int + KN-D5/D7/D9/D10 rows run --features integration against the live docker-compose stack)\n");
    for row in m3_required_rows() {
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
        println!("\nGATE: GREEN — every M3 producer drill proven-and-dated (Git hosting + Knowledge, incl. the live-stack integration rows); M4 may start.");
        println!("       (KN-D3 is a PROVEN floor — full CRDT/OT convergence is a named follow-on; the world-scale 30× LOAD surge is deferred to M5 by design.)");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED — M4 is BLOCKED (the M3→M4 producer-subsystems go/no-go is red).");
        for missing in card.missing_required() {
            eprintln!("  MISSING required row: {missing}");
        }
        for red in card.not_proven() {
            eprintln!("  claimed-not-proven: {}", red.id);
        }
        ExitCode::FAILURE
    }
}

/// Run one proof command via `cargo <args>`. `Ok(())` iff cargo exited 0; otherwise an `Err`
/// naming the non-zero exit (the claimed-not-proven reason). LOUD: the child's output is inherited
/// so the failing drill's own red artifact prints. No `|| true` / swallow path — a non-zero exit is
/// a RED row.
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
             (if this is an integration row, the live docker-compose stack must be up)",
            args.join(" ")
        ))
    }
}

/// The committed scorecard path: `<workspace-root>/testing/scorecards/m3-producers.md`. The
/// workspace root is the parent of this crate's manifest dir (`crates/myelin-harness` → up two).
fn scorecard_path() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&crate_dir)
        .to_path_buf();
    root.join("testing")
        .join("scorecards")
        .join("m3-producers.md")
}
