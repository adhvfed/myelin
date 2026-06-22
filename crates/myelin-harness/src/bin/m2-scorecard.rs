//! The **M2 reactive-shared-layer exit-gate scorecard runner** (M2 → M3) — the band-boundary
//! go/no-go binary.
//!
//! Runs EVERY required M2 gate row's proof command (the per-feature M2 drill that emits its dated
//! green artifact), records PASS / claimed-not-proven into a [`Scorecard`], writes the dated
//! artifact to `testing/scorecards/m2-reactive.md`, prints each row LOUD to stdout, and **exits
//! non-zero if the gate is RED** (a missing row OR any claimed-not-proven row blocks M3 — the gate
//! invariant, master-sequencing §2 / EI-01 §2). There is deliberately NO `|| true` / swallow path:
//! a red row fails the job.
//!
//! This binary WIRES the existing M2 drills (it does not re-implement them): each row's proof
//! command is a `cargo test`/`cargo run` invocation that lives with its feature prompt
//! (P-126..P-245). The six families: the bus/reactive dispatch engine (BUS-D1/D3/D6/D5/D8), the
//! Reference Graph (REF-CDC), Search (SRCH-D1/D2/D3/D4/D7, the zero-leak keystone), Notifications
//! (NOTIF-D1..D11 + snooze), the Agent Fabric M2-B deterministic-correctness family
//! (AG-D1/2/3/5/7/8/11), and the Durable Workflow engine (FLOW-D1/D3/D4/D5/D6/D7 + merge-queue),
//! plus the contract-coverage re-affirm.
//!
//! ## AG-D4 — PROVEN-ON-REAL-HARDWARE, non-vacuous
//! For the AG-D4 row (and only that row) this runner sets `MYELIN_REQUIRE_KVM=1` in the child
//! env. With that env, the escape drill HARD-FAILS (panics) if /dev/kvm or firecracker or the
//! staged guest assets are absent — it does NOT skip gracefully. So the AG-D4 row reads green ONLY
//! when a real Firecracker microVM actually boots, runs the 11-attack adversarial corpus, and
//! attests 0 escapes (a dated `target/ag-d4-attestation/<date>.json`). A vacuous green is refused.
//!
//! The one genuine remaining floor — the world-scale 30× LOAD drill (real fleet hardware) — is a
//! named, dated M5 deferral in the rendered artifact, NOT a row that reds this gate.
//!
//! Usage: `cargo run -p myelin-harness --bin m2-scorecard` (on a KVM-capable host so AG-D4 boots).

use myelin_harness::scorecard::{m2_required_rows, today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::M2Reactive);

    println!(
        "== M2 reactive-shared-layer exit-gate scorecard ({date}) — re-running every M2 drill =="
    );
    println!("   (AG-D4 runs --features integration with MYELIN_REQUIRE_KVM=1: a real microVM MUST boot — no vacuous green)\n");
    for row in m2_required_rows() {
        let require_kvm = row.id == "AG-D4";
        let tag = if require_kvm {
            " [AG-D4 keystone — MYELIN_REQUIRE_KVM=1: a real microVM must boot]"
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
        println!("\nGATE: GREEN — every M2 reactive-layer drill proven-and-dated (incl. AG-D4 on a real microVM); M3 may start.");
        println!("       (the ONE true remaining floor — the world-scale 30× LOAD drill on real fleet hardware — is deferred to M5 by design.)");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED — M3 is BLOCKED (the M2→M3 reactive-layer go/no-go is red).");
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
/// so the failing drill's own red artifact (and, for AG-D4, the real guest-console / attestation
/// line) prints. When `require_kvm` is set, `MYELIN_REQUIRE_KVM=1` and `--nocapture` are passed so
/// the AG-D4 drill hard-fails on a non-KVM host (no vacuous green) and its real-boot proof is
/// visible.
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
            "`cargo {}` (MYELIN_REQUIRE_KVM=1) exited non-zero ({status}) — the AG-D4 real-kernel \
             escape drill did NOT boot a microVM / did NOT attest 0 escapes (a vacuous green is refused)",
            args.join(" ")
        ))
    } else {
        Err(format!(
            "`cargo {}` exited non-zero ({status}) — the drill read RED",
            args.join(" ")
        ))
    }
}

/// The committed scorecard path: `<workspace-root>/testing/scorecards/m2-reactive.md`. The
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
        .join("m2-reactive.md")
}
