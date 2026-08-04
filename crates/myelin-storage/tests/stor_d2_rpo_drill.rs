use std::path::Path;

use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_storage::{ContinuousArchiver, WalSegment};

fn rpo_max_secs_from_thresholds() -> u64 {
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

#[test]
fn stor_d2_rpo_within_bound_under_continuous_archiving() {
    let rpo_bound_secs = rpo_max_secs_from_thresholds();

    let mut archiver = ContinuousArchiver::new();
    let mut signals = SignalSource::new();

    let commit_period: u64 = 30;
    let archive_period: u64 = 60;
    let steps: u64 = 200;
    let mut offset: u64 = 0;
    let mut peak_rpo: u64 = 0;

    for step in 1..=steps {
        let now = step * commit_period;
        offset += 10;
        archiver.record_commit(offset, now);

        if now.is_multiple_of(archive_period) {
            let flush_lag = 5;
            archiver
                .archive_segment(WalSegment {
                    end_offset: offset,
                    committed_at: now.saturating_sub(flush_lag),
                })
                .expect("continuous archiving is strictly forward");
            if now.is_multiple_of(600) {
                archiver.take_base_backup(now);
            }
        }

        let rpo = archiver.measure_rpo();
        peak_rpo = peak_rpo.max(rpo);

        assert!(
            rpo <= rpo_bound_secs,
            "STOR-D2 FLOOR BREACHED at t={now}s: measured RPO {rpo}s exceeds the {rpo_bound_secs}s \
             ({}-min) bound - continuous archiving fell behind the RPO window",
            rpo_bound_secs / 60
        );
    }

    let rpo_at_kill = archiver.measure_rpo();
    peak_rpo = peak_rpo.max(rpo_at_kill);
    signals.set_scalar(SignalName::RestoreRpoSecs, peak_rpo as i64);

    signals
        .assert_signal(
            SignalName::RestoreRpoSecs,
            Predicate::Lte(rpo_bound_secs as i64),
        )
        .expect_green();

    assert!(
        peak_rpo <= rpo_bound_secs,
        "the peak RPO {peak_rpo}s across the run + cell-kill must be within {rpo_bound_secs}s"
    );
    assert!(
        archiver.base_backup_count() > 0,
        "at least one PITR base-backup anchor was taken"
    );

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

#[test]
fn stor_d2_catches_an_archiver_that_falls_behind() {
    let rpo_bound_secs = rpo_max_secs_from_thresholds();

    let mut archiver = ContinuousArchiver::new();
    archiver.record_commit(10, 0);
    let breach_at = rpo_bound_secs + 120;
    archiver.record_commit(20, breach_at);

    let rpo = archiver.measure_rpo();
    assert!(
        rpo > rpo_bound_secs,
        "a stalled archiver must measure an RPO past the bound (got {rpo}s, bound {rpo_bound_secs}s)"
    );

    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::RestoreRpoSecs, rpo as i64);
    let verdict = signals.assert_signal(
        SignalName::RestoreRpoSecs,
        Predicate::Lte(rpo_bound_secs as i64),
    );
    assert!(
        !verdict.is_green(),
        "a stalled archiver (RPO past the bound) MUST read RED on the STOR-D2 assertion"
    );
}
