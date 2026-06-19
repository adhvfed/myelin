//! The **infra integration exit-gate scorecard runner** (Stage 4) — the band-boundary
//! integration gate binary over the REAL backends (Postgres / RustFS / Valkey / NATS JetStream).
//!
//! Runs EVERY required Infra gate row's proof command (the retrofitted `--features integration`
//! drill that emits its dated green artifact against the LIVE docker-compose stack), records
//! PASS / claimed-not-proven into a [`Scorecard`], writes the dated artifact to
//! `testing/scorecards/infra.md`, prints each row LOUD to stdout, and **exits non-zero if the
//! gate is RED** (a missing row OR any claimed-not-proven row — the gate invariant). There is
//! deliberately NO `|| true` / swallow path: a red row fails the job.
//!
//! ## Red-until-proven (the Stage 4 testing-policy ratchet)
//! Every integration row's proof command is a `cargo test --features integration` that FAILS
//! without the live stack. Run THIS binary only after the stack is up — `scripts/integration-
//! test.sh` brings the stack up `--wait`, then runs `cargo test --features integration`, and the
//! integration CI workflow runs this binary as its band-boundary gate. A DB-free run reds the
//! gate (the proof commands fail to connect), which is the correct, honest verdict.
//!
//! ## The two genuine floors stay RED (their containerized smokes are not the full gate)
//! Two rows (SANDBOX-SMOKE, LOAD-10X-SMOKE) carry containerized smokes that run green under
//! Docker, but the rendered artifact STILL prints their named floors (the real-kernel
//! SANDBOX-ESCAPE gate on gVisor / microVM, and the WORLD-SCALE 30× LOAD drill on real hardware)
//! as open deferrals — a proven smoke never silently claims its floor closed (EI-01 §1).
//!
//! Usage (with the stack up): `cargo run -p myelin-harness --bin infra-scorecard`.

use myelin_harness::scorecard::{infra_required_rows, today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::Infra);

    println!("== Infra integration exit-gate scorecard ({date}) — running every --features integration drill ==");
    println!("   (red-until-proven: the live docker-compose stack must be up; run via scripts/integration-test.sh)\n");
    for row in infra_required_rows() {
        let tag = if row.floor.is_some() {
            " [floor smoke — the named floor stays open]"
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
                println!("RED{tag} — {reason}");
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
        println!("\nGATE: GREEN — every infra integration drill proven against the live stack.");
        println!("       (the two named floors — real-kernel SANDBOX-ESCAPE + WORLD-SCALE 30× — stay open by design.)");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED — the infra integration gate is red.");
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

/// Run one proof command via `cargo <args>`. `Ok(())` iff cargo exited 0; otherwise an `Err`
/// naming the non-zero exit (the claimed-not-proven reason). LOUD: the child's output is inherited
/// so the failing drill's own red artifact prints.
fn run_proof(args: &[&str]) -> Result<(), String> {
    let status = Command::new(env!("CARGO"))
        .args(args)
        .status()
        .map_err(|e| format!("could not spawn `cargo {}`: {e}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "`cargo {}` exited non-zero ({status}) — the drill read RED (stack down? run scripts/integration-test.sh)",
            args.join(" ")
        ))
    }
}

/// The committed scorecard path: `<workspace-root>/testing/scorecards/infra.md`. The workspace
/// root is the parent of this crate's manifest dir (`crates/myelin-harness` → up two).
fn scorecard_path() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&crate_dir)
        .to_path_buf();
    root.join("testing").join("scorecards").join("infra.md")
}
