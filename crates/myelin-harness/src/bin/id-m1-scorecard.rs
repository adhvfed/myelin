//! The **Identity M1 → M2 exit-gate scorecard runner** (P-079 / P-ID-21) — the CI band-boundary
//! go/no-go binary.
//!
//! Runs EVERY required Id-M1 gate row's proof command (the per-feature M1 Id drill that emits its
//! dated green artifact to its named contract-1.8 survival signal), records PASS /
//! claimed-not-proven into a [`Scorecard`], writes the dated artifact to
//! `testing/scorecards/id-m1.md`, prints each row LOUD to stdout, and **exits non-zero if the gate
//! is RED** (a missing row OR any claimed-not-proven row blocks M2 — the gate invariant,
//! master-sequencing §2 / EI-01 §2). There is deliberately NO `|| true` / swallow path: a red row
//! fails the job.
//!
//! This binary WIRES the existing M1 Id drills (it does not re-implement them, P-ID-21
//! DELIVERABLE): each row's proof command is a `cargo test -p myelin-identity-service --test
//! drill_id_d*` invocation that lives with its feature prompt (P-068..P-078). The ninth row
//! re-affirms the 4.1–4.11 CDC pairs via the contract-coverage scanner. The runner shells out to
//! `cargo` directly (argv, no shell), so a non-zero exit from any proof command is recorded as a
//! claimed-not-proven row, never softened.
//!
//! The consolidated M1 → M2 Id gate (the prompt GATE): ID-D3 (cross-tenant 0), ID-D2 (fail-static),
//! ID-D1 (disabled-in-5-min), ID-D4 (leak-free pre-filter incl. the S8 JOIN), ID-D7 (watermark),
//! ID-D5 (delegation intersection), ID-D6 (token-crash), ID-D8 (restore) each emit a dated green
//! artifact to its named signal. 8/8 green-and-dated is the go; any red row is a dated
//! "claimed, not proven" scorecard entry, NEVER a softened threshold (EI-01 §3 / roadmap §5).
//!
//! Floor named (M5 hardening): ID-D9 (the 30× surge) + the multi-cell floor drills are M5
//! (P-ID-31 / P-ID-35). Identity is *correct* at M1, *hardened* at M5 — those drills are not part
//! of this M1→M2 go/no-go and are recorded as a named, visible deferral in the rendered artifact.
//!
//! Usage: `cargo run -p myelin-harness --bin id-m1-scorecard`. CI runs it as the `id-m1-scorecard`
//! job.

use myelin_harness::scorecard::{id_m1_required_rows, today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::M1Identity);

    println!("== Identity M1→M2 exit-gate scorecard ({date}) — re-running every M1 Id drill ==");
    for row in id_m1_required_rows() {
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
        println!("\nGATE: GREEN — 8/8 M1 Id drills proven-and-dated; M2 may start.");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED — M2 is BLOCKED (the M1→M2 Id go/no-go is red).");
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
            "`cargo {}` exited non-zero ({status}) — the drill read RED",
            args.join(" ")
        ))
    }
}

/// The committed scorecard path: `<workspace-root>/testing/scorecards/id-m1.md`. The workspace
/// root is the parent of this crate's manifest dir (`crates/myelin-harness` → up two).
fn scorecard_path() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&crate_dir)
        .to_path_buf();
    root.join("testing").join("scorecards").join("id-m1.md")
}
