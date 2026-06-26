//! The **make-it-real evidence gate runner** (MR-005 — the internal P-540/541 evidence spine).
//!
//! RED BY DEFAULT, fails closed. Runs every required make-it-real row's proof command
//! ([`Band::MakeItReal`]), CAPTURES its output, computes a blake3 attestation binding a PASS to
//! that output, records PASS / claimed-not-proven into a [`Scorecard`], writes the attested JSON
//! manifest (`testing/scorecards/make-it-real.json`) + the human markdown mirror
//! (`testing/scorecards/make-it-real.md`), then RE-VALIDATES the manifest (present + proven +
//! hash-valid + fresh) and **exits non-zero unless EVERY required row is a fresh, hash-valid,
//! attested GREEN**.
//!
//! Because the spine work is not done (MR-009/010/011/012/013 have not landed), the proof
//! commands FAIL and the gate reads RED over the current tree — that is correct and expected; it
//! is never faked green (EI-01 §1). This is a *binary you run*, not a `cargo test` — the build
//! stays green; the gate's behaviour is asserted on FIXTURES in `tests/make_it_real_gate.rs`.
//!
//! Usage: `cargo run -p myelin-harness --bin make-it-real-scorecard`.

use myelin_harness::make_it_real::{
    output_bytes, AttestedScorecard, RowAttestation, DEFAULT_MAX_AGE_DAYS,
};
use myelin_harness::scorecard::{today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let date = today_iso();
    let mut card = Scorecard::new(Band::MakeItReal);

    println!(
        "== make-it-real evidence gate ({date}) — RED BY DEFAULT; running + attesting every \
         required row =="
    );
    for row in Band::MakeItReal.required_rows() {
        print!("  {} … ", row.id);
        match run_and_capture(row.proof_command) {
            Ok(output) => {
                let argv: Vec<String> = row.proof_command.iter().map(|s| s.to_string()).collect();
                let att = RowAttestation::compute(row.id, &argv, &date, &output);
                let proof = format!(
                    "[{date}] PASS `cargo {}` (attested blake3:{})",
                    row.proof_command.join(" "),
                    &att.hash[..12]
                );
                println!("PASS [attested {}]", &att.hash[..12]);
                card.record(RowResult::pass_attested(row.id, proof, &date, att));
            }
            Err(reason) => {
                println!("RED — {reason}");
                card.record(RowResult::claimed_not_proven(row.id, reason, &date));
            }
        }
    }

    // Write both artifacts: the machine-readable attested manifest (the source of truth the gate
    // re-validates) and the human markdown mirror (reusing the existing renderer).
    let manifest = AttestedScorecard::from_scorecard(&card, &date);
    if let Err(code) = write_artifact("make-it-real.json", &manifest.to_json()) {
        return code;
    }
    if let Err(code) = write_artifact("make-it-real.md", &card.render_markdown(&date)) {
        return code;
    }
    println!("\nattested manifest + markdown written to testing/scorecards/");

    // Re-validate the manifest (present + proven + hash-valid + fresh) — the fail-closed verdict.
    let verdict = manifest.validate(&date, DEFAULT_MAX_AGE_DAYS);
    if verdict.is_green() {
        println!("\nGATE: GREEN — every make-it-real row is fresh, hash-valid, attested PASS.");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "\nGATE: RED — the spine cannot claim production-real ({} problem(s); red by default):",
            verdict.problems.len()
        );
        for p in &verdict.problems {
            eprintln!("  {p}");
        }
        ExitCode::FAILURE
    }
}

/// Run one proof command via `cargo <args>`, CAPTURING stdout+stderr. `Ok(bytes)` (the captured
/// output to attest) iff cargo exited 0; otherwise an `Err` naming the non-zero exit (the
/// claimed-not-proven reason). The child's output is also echoed so a failing drill's red is
/// visible.
fn run_and_capture(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new(env!("CARGO"))
        .args(args)
        .output()
        .map_err(|e| format!("could not spawn `cargo {}`: {e}", args.join(" ")))?;
    // Echo so the human sees the real drill output (LOUD — no silent swallow).
    if !out.stdout.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&out.stdout));
    }
    if !out.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
    }
    let code = out.status.code().unwrap_or(-1);
    if out.status.success() {
        Ok(output_bytes(code, &out.stdout, &out.stderr))
    } else {
        Err(format!(
            "`cargo {}` exited non-zero ({code}) — the drill read RED (this floor is not yet real)",
            args.join(" ")
        ))
    }
}

/// Write `name` under `<workspace-root>/testing/scorecards/`. A write failure is a loud fatal.
fn write_artifact(name: &str, body: &str) -> Result<(), ExitCode> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&crate_dir)
        .to_path_buf();
    let dir = root.join("testing").join("scorecards");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("FATAL: could not create {}: {e}", dir.display());
        return Err(ExitCode::FAILURE);
    }
    let path = dir.join(name);
    if let Err(e) = std::fs::write(&path, body) {
        eprintln!("FATAL: could not write {}: {e}", path.display());
        return Err(ExitCode::FAILURE);
    }
    Ok(())
}
