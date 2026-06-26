//! The **M6 dogfooding exit-gate scorecard runner** (M6 → M7) — the FINAL band-boundary go/no-go
//! binary, the platform done-bar reached by DOGFOODING.
//!
//! Runs EVERY required M6 gate row's proof command (the per-feature dogfood / switch-test drill that
//! emits its dated green artifact), records PASS / claimed-not-proven into a [`Scorecard`], writes
//! the dated artifact to `testing/scorecards/m6-dogfood.md`, prints each row LOUD to stdout, and
//! **exits non-zero if the gate is RED** (a missing row OR any claimed-not-proven row blocks M7 — the
//! gate invariant, master-sequencing §2 / EI-01 §2). There is deliberately NO `|| true` / swallow
//! path: a red row fails the job.
//!
//! This binary WIRES the existing M6 drills (it does not re-implement them): each row's proof command
//! is a `cargo test`/`cargo run` invocation that lives with its feature prompt (P-445..P-521). The
//! four families: the switch tests (browser-driven over the real surface, measured contrast +
//! latency), the self-hosting CI graph (the dogfood loop is live), the dogfood drills (the platform
//! runs on its own work), and the truth-up pass (every PROVEN row rests on a dated green artifact),
//! plus the permanent STOR-D37 restore gate on Myelin's own commits.
//!
//! ## The rows just need the LIVE stack up (no /dev/kvm, no `--features integration`)
//! Several rows reach the backends; bring the live docker-compose stack up first
//! (`scripts/integration-test.sh` or `docker compose -f docker-compose.dev.yml up -d --wait`). No M6
//! row needs /dev/kvm (that was AG-D4-specific) and none passes `--features integration` (the dogfood
//! loop's LOGIC runs in-process over the platform's own work; the switch tests drive the real surface
//! directly).
//!
//! ## The honesty framing (printed in the rendered artifact, NOT rows that red this gate)
//! - **M6 green is dogfood-complete, NOT production-ready.** M7 (P-522..P-546, production readiness &
//!   security hardening) is the next band and is NOT yet implemented; M0..M6 deliberately shipped
//!   several production mechanisms as documented EI-01 §1 structural FLOORS (auth-token crypto,
//!   HSM-class KMS, durable Identity stores, real backup/restore, sandbox PRODUCTION exec on both
//!   backends), each filled by M7 + a separate verification prompt, gated fail-closed (P-546).
//! - **STOR-D37 dogfood restore-verify on Myelin's own commits** is permanent (a backup never
//!   restored is not a backup, EI-01 §3) — re-run-forever.
//!
//! Usage: `cargo run -p myelin-harness --bin m6-scorecard` (with the live docker-compose stack up so
//! the rows that reach the backends pass).

use myelin_harness::scorecard::{m6_required_rows, today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::M6Dogfood);

    println!(
        "== M6 dogfooding exit-gate scorecard ({date}) — re-running every switch-test / self-hosting-CI / dogfood / truth-up drill =="
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
        println!("\nGATE: GREEN — every M6 dogfood drill proven-and-dated (the switch tests + self-hosting-CI + the dogfood drills + STOR-D37 restore-verify on Myelin's own commits + the truth-up pass); the platform is dogfood-complete, M7 may start.");
        println!("       (M6 green is DOGFOOD-COMPLETE, NOT production-ready — the M7 production floors, incl. sandbox prod-exec, are named dated deferrals filled by P-522..P-546, fail-closed at P-546.)");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED — M7 is BLOCKED (the M6→M7 dogfooding go/no-go is red).");
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
             (several M6 rows reach the backends; the live docker-compose stack must be up)",
            args.join(" ")
        ))
    }
}

/// The committed scorecard path: `<workspace-root>/testing/scorecards/m6-dogfood.md`. The workspace
/// root is the parent of this crate's manifest dir (`crates/myelin-harness` → up two).
fn scorecard_path() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&crate_dir)
        .to_path_buf();
    root.join("testing")
        .join("scorecards")
        .join("m6-dogfood.md")
}
