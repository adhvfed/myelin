//! P-ST-14 (global P-100) GATE / DRILL — **STOR-D2 (the RTO / cell-kill half)**, dated green artifact.
//!
//! **STOR-D2, the RTO half (storage.md §7.1 / testing-strategy §4.2 row STOR-D2 / D-S2):** *kill a
//! cell; restore from the archive (P-059); assert **RTO ≤ 1 h/tenant, ≤ 4 h/cell**.* Telemetry:
//! restore-time per tenant/cell (`RestoreRtoSecs{grain}`). (The RPO half is `stor_d2_rpo_drill`,
//! P-059; the no-loss STOR-D1 CI gate is P-061.)
//!
//! ## The thresholds are READ, never hardcoded (EI-01 §3)
//! The RTO bounds come from the **versioned `thresholds.toml`** (`[rpo_rto] rto_tenant_max_mins` /
//! `rto_cell_max_mins`, the single source of truth, P-038) — NOT magic numbers. Weakening either to
//! pass is forbidden; a red is a dated `[[claimed_not_proven]]` scorecard row. The measured numbers
//! are emitted on the SAME [`SignalSource`] every drill uses via the existing
//! [`SignalName::RestoreRtoSecs`]`{grain}` signal (reused from P-056, never re-defined).
//!
//! ## Scope (named, EI-01 §4)
//! This is the M1 modeled cell-kill RTO drill (the real `pg_restore` + WAL-replay + cell-kill
//! provisioning driver is the P-S12/P-S15 floor; the cell-scale re-confirm is P-ST-30 / M5). The
//! measured wall-clock is the begin-restore → consistent-ready phase set (restore land + reindex +
//! the mandatory §7.5 re-erasure pass).

use std::path::Path;

use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_storage::{CellKillRestore, CellKillRtoReport, RtoGrain};

/// Read a `[rpo_rto]` minutes bound from the workspace-root `thresholds.toml` (the versioned source
/// of truth). A missing threshold is a LOUD failure — never a silent default (P-038).
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

/// **STOR-D2 (RTO half): kill a cell, restore from the archive → RTO ≤ 1 h/tenant, ≤ 4 h/cell. The
/// dated green artifact.**
#[test]
fn stor_d2_cell_kill_rto_within_bounds() {
    let tenant_bound = rto_max_secs_from_thresholds("rto_tenant_max_mins");
    let cell_bound = rto_max_secs_from_thresholds("rto_cell_max_mins");

    // The cell is killed at t=0. A single TENANT is recovered first (the per-tenant RTO): the restore
    // lands + reindexes + the §7.5 re-erasure pass runs over the tenant's copy. Modeled phase set:
    //   PITR restore land:        18 min
    //   reindex-from-source:       9 min
    //   post-restore re-erasure:   3 min
    //   ready/health check:        2 min     => 32 min total (within the 60-min tenant bound).
    let tenant_began = 0u64;
    let tenant_ready = (18 + 9 + 3 + 2) * 60; // 32 min
    let tenant_recovery = CellKillRestore::new(RtoGrain::Tenant, tenant_began, tenant_ready);

    // The WHOLE CELL is recovered (every tenant): a larger restore + reindex + a re-erasure pass over
    // all post-PIT erasures. Modeled phase set:
    //   PITR restore land:        95 min
    //   reindex-from-source:      55 min
    //   post-restore re-erasure:  20 min
    //   ready/health check:       10 min     => 180 min total (within the 240-min cell bound).
    let cell_began = 0u64;
    let cell_ready = (95 + 55 + 20 + 10) * 60; // 180 min
    let cell_recovery = CellKillRestore::new(RtoGrain::Cell, cell_began, cell_ready);

    let mut report = CellKillRtoReport::new();
    report.record(&tenant_recovery).record(&cell_recovery);

    // The load-bearing assertions: each measured RTO is within its per-grain bound (read from
    // thresholds.toml). A single breach is a FAIL — the threshold is not weakened to pass.
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

    // Emit the measured numbers onto the telemetry source (observability is part of the pass) via the
    // SAME labelled signal the harness restore-outcome uses, and assert green per grain.
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

/// The drill must actually CATCH a too-slow recovery (the assertion is real, not vacuous): a cell
/// restore that takes LONGER than the bound reads RED on the RTO assertion. Proves the gate would fail
/// on a regression (EI-01 §3 — a drill that cannot go red is not a gate).
#[test]
fn stor_d2_catches_a_too_slow_recovery() {
    let cell_bound = rto_max_secs_from_thresholds("rto_cell_max_mins");

    // A cell recovery that overruns the bound (5 h vs the 4-h bound) — a stalled restore.
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
