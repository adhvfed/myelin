//! The **M0 exit-gate scorecard runner** (P-S24 → P-039) — the CI band-boundary gate binary.
//!
//! Runs EVERY required SUB-M0 gate row's proof command (the per-feature drill that emits its
//! dated green artifact), records PASS / claimed-not-proven into a [`Scorecard`], writes the
//! dated artifact to `testing/scorecards/sub-m0.md`, prints each row LOUD to stdout, and
//! **exits non-zero if the gate is RED** (a missing row OR any claimed-not-proven row blocks
//! M1 — the gate invariant, master-sequencing §2 / EI-01 §2). There is deliberately NO
//! `|| true` / swallow path: a red row fails the job.
//!
//! This binary WIRES the existing drills (it does not re-implement them, P-S24 DELIVERABLE):
//! each row's proof command is a `cargo test` / `cargo run` invocation that lives with its
//! feature prompt. The runner shells out to `cargo` directly (argv, no shell), so a non-zero
//! exit from any proof command is recorded as a claimed-not-proven row, never softened.
//!
//! Usage: `cargo run -p myelin-harness --bin sub-m0-scorecard`. CI runs it as the
//! `sub-m0-scorecard` job.

use myelin_harness::scorecard::{required_rows, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::M0);

    println!("== SUB-M0 exit-gate scorecard ({date}) — running every required gate row ==");
    for row in required_rows() {
        let perm = if row.permanent { " [permanent — re-run-forever]" } else { "" };
        print!("  {} … ", row.id);
        match run_proof(row.proof_command) {
            Ok(()) => {
                let proof = format!(
                    "[{date}] PASS  `cargo {}`",
                    row.proof_command.join(" ")
                );
                println!("PASS{perm}");
                card.record(RowResult::pass(row.id, proof, &date));
            }
            Err(reason) => {
                println!("RED{perm} — {reason}");
                card.record(RowResult::claimed_not_proven(row.id, reason, &date));
            }
        }
    }

    // Write the dated artifact (the committed scorecard body). Best-effort: a write failure is
    // itself a loud error, not a silent skip.
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
        println!("\nGATE: GREEN — every SUB-M0 row proven; M1 may start.");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nGATE: RED — M1 is BLOCKED.");
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
/// naming the non-zero exit (the claimed-not-proven reason). LOUD: the child's output is
/// inherited so the failing drill's own red artifact prints.
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

/// The committed scorecard path: `<workspace-root>/testing/scorecards/sub-m0.md`. The workspace
/// root is the parent of this crate's manifest dir (`crates/myelin-harness` → up two).
fn scorecard_path() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&crate_dir)
        .to_path_buf();
    root.join("testing").join("scorecards").join("sub-m0.md")
}

/// Today's date as ISO-8601 `YYYY-MM-DD`, derived from the system clock with no external time
/// crate (the harness keeps its dep set tiny). Computed via the civil-from-days algorithm.
fn today_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's days→civil-date algorithm (proleptic Gregorian). `days` is days since the
/// Unix epoch (1970-01-01). Returns (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
