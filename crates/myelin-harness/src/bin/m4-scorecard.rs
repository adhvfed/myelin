//! The **M4 consumer-subsystems exit-gate scorecard runner** (M4 → M5) — the band-boundary
//! go/no-go binary.
//!
//! Runs EVERY required M4 gate row's proof command (the per-feature CI / Issues / Chat drill that
//! emits its dated green artifact), records PASS / claimed-not-proven into a [`Scorecard`], writes
//! the dated artifact to `testing/scorecards/m4-consumers.md`, prints each row LOUD to stdout, and
//! **exits non-zero if the gate is RED** (a missing row OR any claimed-not-proven row blocks M5 —
//! the gate invariant, master-sequencing §2 / EI-01 §2). There is deliberately NO `|| true` /
//! swallow path: a red row fails the job.
//!
//! This binary WIRES the existing M4 drills (it does not re-implement them): each row's proof
//! command is a `cargo test`/`cargo run` invocation that lives with its feature prompt
//! (P-319..P-419). The three families: CI (CI-D9/D1/D5/D8/D11/D6/D4/D7 + the two permanent
//! integration gates), Issues (ISS-P06/D2/D3/D4/D5/D6/D7/D8/D9/D11/D13), and Chat
//! (CHAT-D5/D6/D7/D18/D8/D9/D10/D11/D12/D15/D16/D17), plus the contract-coverage re-affirm.
//!
//! ## AG-D4/CI-T1 — PROVEN-ON-REAL-HARDWARE, non-vacuous
//! For the AG-D4/CI-T1 prod-image re-confirm row (and only that row) this runner sets
//! `MYELIN_REQUIRE_KVM=1` in the child env. With that env, the escape drill HARD-FAILS (panics) if
//! /dev/kvm or firecracker or the staged guest assets are absent — it does NOT skip gracefully. So
//! the row reads green ONLY when the COMMITTED prod CI runner image actually boots a real
//! Firecracker microVM, runs the adversarial corpus, and attests 0 escapes. A vacuous green is
//! refused.
//!
//! ## The two PERMANENT integration rows need the LIVE stack (RED-until-proven)
//! AG-D4/CI-T1 and STOR-D1/D2 run `--features integration` against the live docker-compose stack
//! (Postgres / RustFS / Valkey / NATS JetStream). On a host with the stack DOWN (or, for AG-D4, no
//! /dev/kvm) those proof commands FAIL, so the gate cannot read green from a DB-free / non-KVM run.
//! Bring the stack up first (`scripts/integration-test.sh` or `docker compose up --wait`).
//!
//! ## Named floors (printed in the rendered artifact, NOT rows that red this gate)
//! - **The world-scale 30× LOAD / surge drills** (FLOW-D8 / AG-D6 / the CHAT+Issues surge) need real
//!   fleet hardware and are deferred to M5.
//! - **gVisor (`runsc`) as a second escape-drill backend (CI-P28)** is a run-when-available residual;
//!   Firecracker (the production default) is the exercised gate backend.
//!
//! Usage: `cargo run -p myelin-harness --bin m4-scorecard` (on a KVM-capable host with the live
//! docker-compose stack up so AG-D4/CI-T1 boots a real microVM and STOR-D1/D2 reaches the backends).

use myelin_harness::scorecard::{m4_required_rows, today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::M4Consumers);

    println!(
        "== M4 consumer-subsystems exit-gate scorecard ({date}) — re-running every CI+Issues+Chat drill =="
    );
    println!("   (AG-D4/CI-T1 runs --features integration with MYELIN_REQUIRE_KVM=1: a real microVM MUST boot — no vacuous green; STOR-D1/D2 runs --features integration against the live stack)\n");
    for row in m4_required_rows() {
        let require_kvm = row.id == "AG-D4/CI-T1";
        let tag = if require_kvm {
            " [AG-D4/CI-T1 prod-image re-confirm — MYELIN_REQUIRE_KVM=1: a real microVM must boot]"
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
        println!("\nGATE: GREEN — every M4 consumer drill proven-and-dated (CI + Issues + Chat, incl. the AG-D4/CI-T1 prod-image re-confirm on a real microVM + the STOR-D1/D2 CI-store restore-verify); M5 may start.");
        println!("       (the ONE true remaining floor — the world-scale 30× LOAD / surge drills on real fleet hardware — is deferred to M5 by design; gVisor CI-P28 is a named run-when-available residual.)");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED — M5 is BLOCKED (the M4→M5 consumer-subsystems go/no-go is red).");
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
/// so the failing drill's own red artifact (and, for AG-D4/CI-T1, the real guest-console /
/// attestation line) prints. When `require_kvm` is set, `MYELIN_REQUIRE_KVM=1` and `--nocapture`
/// are passed so the prod-image re-confirm drill hard-fails on a non-KVM host (no vacuous green)
/// and its real-boot proof is visible. No `|| true` / swallow path — a non-zero exit is a RED row.
fn run_proof(args: &[&str], require_kvm: bool) -> Result<(), String> {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(args);
    if require_kvm {
        cmd.env("MYELIN_REQUIRE_KVM", "1");
        // Surface the real guest console + the dated green attestation line under the test harness.
        cmd.arg("--").arg("--nocapture");
    }
    let status = cmd
        .status()
        .map_err(|e| format!("could not spawn `cargo {}`: {e}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else if require_kvm {
        Err(format!(
            "`cargo {}` (MYELIN_REQUIRE_KVM=1) exited non-zero ({status}) — the AG-D4/CI-T1 \
             prod-image re-confirm did NOT boot a microVM / did NOT attest 0 escapes (a vacuous green is refused)",
            args.join(" ")
        ))
    } else {
        Err(format!(
            "`cargo {}` exited non-zero ({status}) — the drill read RED \
             (if this is an integration row, the live docker-compose stack must be up)",
            args.join(" ")
        ))
    }
}

/// The committed scorecard path: `<workspace-root>/testing/scorecards/m4-consumers.md`. The
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
        .join("m4-consumers.md")
}
