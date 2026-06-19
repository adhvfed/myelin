//! P-ST-11 (global P-059) GATE / DRILL — STOR-D2 (the RPO half), dated green artifact.
//!
//! **STOR-D2, the RPO half (storage.md §7.1 / testing-strategy §4.2 row STOR-D2):** continuous WAL
//! archiving holds **RPO ≤ 5 min** (the WAL tail). Telemetry: `backup_rpo_seconds ≤ 300`. This
//! drill streams a steady write workload into the [`ContinuousArchiver`], continuously archives the
//! WAL tail (the §7.1 mechanism), and asserts the MEASURED data-at-risk window stays within the RPO
//! bound — even at the moment of a simulated cell kill (the worst case: the un-archived tail is the
//! data that would be lost).
//!
//! ## The threshold is READ, never hardcoded (EI-01 §3)
//! The RPO bound comes from the **versioned `thresholds.toml`** (`[rpo_rto] rpo_max_mins`, the
//! single source of truth, P-038) — NOT a magic number in the test. Weakening it to pass is
//! forbidden; a red is a dated `[[claimed_not_proven]]` scorecard row. The measured number is
//! emitted on the SAME [`SignalSource`] every drill uses (observability is part of the pass) via
//! the existing [`SignalName::RestoreRpoSecs`] signal (reused from P-056, never re-defined).
//!
//! ## Scope (named, EI-01 §4)
//! This is the M1 single-tenant-scale RPO drill against the modeled WAL archiver (the real
//! WAL-shipping driver is the P-S12/P-S15 floor; the cell-scale re-confirm is P-ST-30 / M5). The
//! RTO / cell-kill recovery-TIME leg of STOR-D2 is the sibling P-ST-14 (global P-100); the no-loss
//! STOR-D1 CI gate is P-ST-13 (global P-061). All named in the prompt + the crate docs.

use std::path::Path;

use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_storage::{ContinuousArchiver, WalSegment};

/// Read the `rpo_max_mins` bound from the workspace-root `thresholds.toml` (the versioned source of
/// truth). A missing threshold is a LOUD failure — never a silent default (the thresholds-file
/// discipline, P-038).
fn rpo_max_secs_from_thresholds() -> u64 {
    // This crate sits at crates/myelin-storage; the workspace root is two levels up.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest");
    let path = root.join("thresholds.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the versioned thresholds file must load at {path:?}: {e}"));
    let doc: toml::Value = text.parse().expect("thresholds.toml must be valid TOML");
    let mins = doc
        .get("rpo_rto")
        .and_then(|t| t.get("rpo_max_mins"))
        .and_then(|v| v.as_integer())
        .expect("rpo_rto.rpo_max_mins must be present (a missing threshold is a LOUD error)");
    assert!(mins > 0, "the RPO bound must be a positive duration");
    (mins as u64) * 60
}

/// **STOR-D2 (RPO half): continuous WAL archiving holds RPO ≤ 5 min — dated green artifact.**
///
/// The scenario: a steady write workload commits into the OLTP tier while the archiver continuously
/// ships the WAL tail off-host on a short cadence. At every step (including the simulated cell kill)
/// the measured RPO — the un-archived committed window — must stay within the `rpo_max_mins` bound.
#[test]
fn stor_d2_rpo_within_bound_under_continuous_archiving() {
    let rpo_bound_secs = rpo_max_secs_from_thresholds();

    let mut archiver = ContinuousArchiver::new();
    let mut signals = SignalSource::new();

    // The steady workload: a write commits every `commit_period` seconds; the archiver ships the
    // WAL tail every `archive_period` seconds (the continuous-archiving cadence). The archive
    // cadence must be << the RPO bound, so the un-archived window never grows past the bound.
    let commit_period: u64 = 30; // a commit every 30 s
    let archive_period: u64 = 60; // the WAL tail ships every 60 s (continuous archiving)
    let steps: u64 = 200; // ~100 minutes of steady operation
    let mut offset: u64 = 0;
    let mut peak_rpo: u64 = 0;

    for step in 1..=steps {
        let now = step * commit_period;
        // A write commits (the WAL advances).
        offset += 10;
        archiver.record_commit(offset, now);

        // The archiver ships the WAL tail on its cadence (continuous archiving). It ships the
        // segment covering the latest committed offset, with a small flush lag (5 s).
        if now.is_multiple_of(archive_period) {
            let flush_lag = 5;
            archiver
                .archive_segment(WalSegment {
                    end_offset: offset,
                    committed_at: now.saturating_sub(flush_lag),
                })
                .expect("continuous archiving is strictly forward");
            // Periodically take a base backup (the PITR anchor) — every ~10 minutes.
            if now.is_multiple_of(600) {
                archiver.take_base_backup(now);
            }
        }

        // MEASURE the RPO at this instant — the data-at-risk window if the cell were killed NOW.
        let rpo = archiver.measure_rpo();
        peak_rpo = peak_rpo.max(rpo);

        // The load-bearing assertion: the RPO NEVER exceeds the bound (a single breach is a FAIL —
        // the threshold is not weakened to pass, EI-01 §3).
        assert!(
            rpo <= rpo_bound_secs,
            "STOR-D2 FLOOR BREACHED at t={now}s: measured RPO {rpo}s exceeds the {rpo_bound_secs}s \
             ({}-min) bound — continuous archiving fell behind the RPO window",
            rpo_bound_secs / 60
        );
    }

    // The simulated CELL KILL: everything committed but not yet archived is the data lost. The RPO
    // at the kill instant is the worst case — it must STILL be within the bound (continuous
    // archiving bounds the loss). Record the measured number onto the telemetry source.
    let rpo_at_kill = archiver.measure_rpo();
    peak_rpo = peak_rpo.max(rpo_at_kill);
    signals.set_scalar(SignalName::RestoreRpoSecs, peak_rpo as i64);

    // THE green artifact: assert the measured RPO is within the bound, observably (the SAME
    // assertion surface every drill uses). A non-zero RPO over the bound reads RED.
    signals
        .assert_signal(SignalName::RestoreRpoSecs, Predicate::Lte(rpo_bound_secs as i64))
        .expect_green();

    assert!(
        peak_rpo <= rpo_bound_secs,
        "the peak RPO {peak_rpo}s across the run + cell-kill must be within {rpo_bound_secs}s"
    );
    assert!(archiver.base_backup_count() > 0, "at least one PITR base-backup anchor was taken");

    println!(
        "[P-059 DRILL GREEN 2026-06-19] STOR-D2 (RPO half): continuous WAL archiving over \
         {steps} commits ({}min steady run, commit every {commit_period}s, WAL tail archived every \
         {archive_period}s, {} base backups) -> PEAK backup_rpo_seconds={peak_rpo}s <= bound \
         {rpo_bound_secs}s ({}min) [read from thresholds.toml, NOT hardcoded]; RPO at simulated \
         cell-kill={rpo_at_kill}s. RTO/cell-kill leg -> P-ST-14 (P-100); STOR-D1 CI gate -> \
         P-ST-13 (P-061); cell-scale re-confirm -> P-ST-30 (M5).",
        steps * commit_period / 60,
        archiver.base_backup_count(),
        rpo_bound_secs / 60,
    );
}

/// The drill harness must actually CATCH a too-slow archiver (the assertion is real, not vacuous):
/// if continuous archiving falls behind so the un-archived window grows past the RPO bound, the
/// measured RPO exceeds it and the telemetry assertion reads RED. This proves the gate would fail
/// on a regression (EI-01 §3 — a drill that cannot go red is not a gate).
#[test]
fn stor_d2_catches_an_archiver_that_falls_behind() {
    let rpo_bound_secs = rpo_max_secs_from_thresholds();

    let mut archiver = ContinuousArchiver::new();
    // A commit at t=0... and the archiver NEVER ships it (a stalled archive_command — the failure
    // mode). After more than the RPO bound elapses with a fresh commit, the un-archived window
    // exceeds the bound.
    archiver.record_commit(10, 0);
    // Time advances well past the bound with no archiving; a new commit lands at the breach instant.
    let breach_at = rpo_bound_secs + 120; // 2 min past the bound
    archiver.record_commit(20, breach_at);

    let rpo = archiver.measure_rpo();
    assert!(
        rpo > rpo_bound_secs,
        "a stalled archiver must measure an RPO past the bound (got {rpo}s, bound {rpo_bound_secs}s)"
    );

    // The telemetry assertion reads RED (the gate would FAIL CI) — proving the threshold bites.
    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::RestoreRpoSecs, rpo as i64);
    let verdict = signals.assert_signal(SignalName::RestoreRpoSecs, Predicate::Lte(rpo_bound_secs as i64));
    assert!(
        !verdict.is_green(),
        "a stalled archiver (RPO past the bound) MUST read RED on the STOR-D2 assertion"
    );
}
