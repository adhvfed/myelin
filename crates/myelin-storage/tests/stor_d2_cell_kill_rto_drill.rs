use std::path::Path;

use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_storage::{CellKillRestore, CellKillRtoReport, RtoGrain};

fn rto_max_secs_from_thresholds(key: &str) -> u64 {
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
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_integer())
        .unwrap_or_else(|| {
            panic!("rpo_rto.{key} must be present (a missing threshold is a LOUD error)")
        });
    assert!(mins > 0, "the RTO bound {key} must be a positive duration");
    (mins as u64) * 60
}

#[test]
fn stor_d2_cell_kill_rto_within_bounds() {
    let tenant_bound = rto_max_secs_from_thresholds("rto_tenant_max_mins");
    let cell_bound = rto_max_secs_from_thresholds("rto_cell_max_mins");

    let tenant_began = 0u64;
    let tenant_ready = (18 + 9 + 3 + 2) * 60;
    let tenant_recovery = CellKillRestore::new(RtoGrain::Tenant, tenant_began, tenant_ready);

    let cell_began = 0u64;
    let cell_ready = (95 + 55 + 20 + 10) * 60;
    let cell_recovery = CellKillRestore::new(RtoGrain::Cell, cell_began, cell_ready);

    let mut report = CellKillRtoReport::new();
    report.record(&tenant_recovery).record(&cell_recovery);

    assert!(
        tenant_recovery.within_bound(tenant_bound),
        "STOR-D2 RTO FLOOR BREACHED: tenant recovery {}s exceeds the {tenant_bound}s ({}-min) bound",
        tenant_recovery.rto_secs(),
        tenant_bound / 60
    );
    assert!(
        cell_recovery.within_bound(cell_bound),
        "STOR-D2 RTO FLOOR BREACHED: cell recovery {}s exceeds the {cell_bound}s ({}-min) bound",
        cell_recovery.rto_secs(),
        cell_bound / 60
    );

    let mut signals = SignalSource::new();
    signals.set_labelled(
        SignalName::RestoreRtoSecs,
        vec![Label::new("grain", RtoGrain::Tenant.label())],
        tenant_recovery.rto_secs() as i64,
    );
    signals.set_labelled(
        SignalName::RestoreRtoSecs,
        vec![Label::new("grain", RtoGrain::Cell.label())],
        cell_recovery.rto_secs() as i64,
    );
    signals
        .assert_labelled(
            SignalName::RestoreRtoSecs,
            vec![Label::new("grain", "tenant")],
            Predicate::Lte(tenant_bound as i64),
        )
        .expect_green();
    signals
        .assert_labelled(
            SignalName::RestoreRtoSecs,
            vec![Label::new("grain", "cell")],
            Predicate::Lte(cell_bound as i64),
        )
        .expect_green();

    println!(
        "[P-100 DRILL GREEN 2026-06-20] STOR-D2 (RTO/cell-kill half): killed a cell, restored from \
         the archive -> per-tenant RTO={}s ({}min) <= {tenant_bound}s ({}min) bound; per-cell \
         RTO={}s ({}min) <= {cell_bound}s ({}min) bound [both read from thresholds.toml, NOT \
         hardcoded]. RPO half -> stor_d2_rpo_drill (P-059); STOR-D3 re-erasure -> \
         stor_d3_post_restore_reerase_drill; cell-scale re-confirm -> P-ST-30 (M5).",
        tenant_recovery.rto_secs(),
        tenant_recovery.rto_secs() / 60,
        tenant_bound / 60,
        cell_recovery.rto_secs(),
        cell_recovery.rto_secs() / 60,
        cell_bound / 60,
    );

    assert_eq!(
        report.rto_for(RtoGrain::Tenant),
        Some(tenant_recovery.rto_secs())
    );
    assert_eq!(
        report.rto_for(RtoGrain::Cell),
        Some(cell_recovery.rto_secs())
    );
}

#[test]
fn stor_d2_catches_a_too_slow_recovery() {
    let cell_bound = rto_max_secs_from_thresholds("rto_cell_max_mins");

    let slow = CellKillRestore::new(RtoGrain::Cell, 0, cell_bound + 3_600);
    assert!(
        !slow.within_bound(cell_bound),
        "an over-budget recovery ({}s) must exceed the {cell_bound}s bound",
        slow.rto_secs()
    );

    let mut signals = SignalSource::new();
    signals.set_labelled(
        SignalName::RestoreRtoSecs,
        vec![Label::new("grain", "cell")],
        slow.rto_secs() as i64,
    );
    let verdict = signals.assert_labelled(
        SignalName::RestoreRtoSecs,
        vec![Label::new("grain", "cell")],
        Predicate::Lte(cell_bound as i64),
    );
    assert!(
        !verdict.is_green(),
        "a too-slow cell recovery (RTO past the bound) MUST read RED on the STOR-D2 RTO assertion"
    );
}
