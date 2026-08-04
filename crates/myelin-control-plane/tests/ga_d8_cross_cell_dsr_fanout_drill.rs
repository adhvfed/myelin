use std::sync::Arc;

use myelin_control_plane::{
    CellDsrReceipt, CellLocalEraser, CrossCellDsrFanOut, CrossCellZookieReader, ZookieStaleness,
    ZOOKIE_STALENESS_BUDGET_SECS,
};
use myelin_events::Timestamp;
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_identity::Zookie;
use myelin_tenancy::{ArtifactRef, CellId, OpaqueSubjectId, TenantId};

struct MemberCell {
    cell: CellId,
}
impl MemberCell {
    fn new(cell: &str) -> MemberCell {
        MemberCell {
            cell: CellId::from_token(cell),
        }
    }
}
impl CellLocalEraser for MemberCell {
    fn erase_in_cell(
        &self,
        _tenant: &TenantId,
        subject: &OpaqueSubjectId,
        _now: &Timestamp,
    ) -> CellDsrReceipt {
        CellDsrReceipt {
            cell: self.cell.clone(),
            subject: subject.clone(),
            receipt: format!(
                "receipt:{}:{}",
                self.cell.as_str(),
                subject.artifact_ref().0
            ),
        }
    }
}

fn subject(s: &str) -> OpaqueSubjectId {
    OpaqueSubjectId::from_ref(ArtifactRef(s.into()))
}

#[test]
fn ga_d8_cross_cell_dsr_fan_out_misses_zero_cells() {
    let mut fanout = CrossCellDsrFanOut::new();
    fanout.register(
        CellId::from_token("cell-b"),
        Arc::new(MemberCell::new("cell-b")),
    );
    fanout.register(
        CellId::from_token("cell-c"),
        Arc::new(MemberCell::new("cell-c")),
    );
    fanout.register(
        CellId::from_token("cell-d"),
        Arc::new(MemberCell::new("cell-d")),
    );

    let set = fanout.fan_out(
        &subject("p1"),
        &TenantId::from_token("01J0ACME"),
        &CellId::from_token("cell-b"),
        &[CellId::from_token("cell-c"), CellId::from_token("cell-d")],
        Timestamp("2026-06-24T00:00:00Z".into()),
    );

    assert_eq!(
        set.fan_out_cells.len(),
        3,
        "{{home}} ∪ member_cells = 3 cells"
    );
    assert_eq!(set.receipts.len(), 3, "one receipt per cell");
    let cells_missed = set.cells_missed();
    assert_eq!(cells_missed, 0, "0 cells missed (the GA-D8 zero)");
    assert!(set.is_complete(), "the merged receipt set is COMPLETE");

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, cells_missed as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-430 GA-D8 GREEN 2026-06-24] cross-cell DSR fan-out: a multi-cell erasure iterated \
         {{home_cell}} ∪ member_cells = {} cells (cell-b ∪ cell-c, cell-d), merged a COMPLETE per-cell \
         receipt set ({} receipts), cells_missed={} (the GA-D8 zero). {} FLOOR: member_cells \
         MULTI-ELEMENT is PROMOTED (P-CP-08 → P-CP-20); no new floor.",
        set.fan_out_cells.len(),
        set.receipts.len(),
        cells_missed,
        set.summary(),
    );
}

#[test]
fn ga_d8_gate_is_not_vacuous_an_unreachable_cell_reads_red() {
    let mut fanout = CrossCellDsrFanOut::new();
    fanout.register(
        CellId::from_token("cell-b"),
        Arc::new(MemberCell::new("cell-b")),
    );
    let set = fanout.fan_out(
        &subject("p1"),
        &TenantId::from_token("01J0ACME"),
        &CellId::from_token("cell-b"),
        &[CellId::from_token("cell-c")],
        Timestamp("2026-06-24T00:00:00Z".into()),
    );
    let cells_missed = set.cells_missed();
    assert_eq!(
        cells_missed, 1,
        "the unreachable cell is MISSED (not dropped)"
    );
    assert!(!set.is_complete(), "an incomplete fan-out is RED");

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, cells_missed as i64);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a missed cell in an erasure fan-out MUST read RED - the GA-D8 zero is a real tripwire"
    );
}

#[test]
fn ga_d8_cross_cell_zookie_consistency_leg() {
    let reader = CrossCellZookieReader::new();
    let home_zookie = Zookie("cell-b@snap-1042".into());

    let within = reader.read_through(&home_zookie, 1000, 940);
    assert!(
        within.is_within_bound(),
        "60s ≤ {ZOOKIE_STALENESS_BUDGET_SECS}s budget → admitted (bounded-stale)"
    );
    let within_lag = within.observed_staleness_secs();
    assert_eq!(within_lag, 60);

    let past = reader.read_through(&home_zookie, 1000, 300);
    assert!(
        !past.is_within_bound(),
        "700s > {ZOOKIE_STALENESS_BUDGET_SECS}s budget → REFUSED (never a stale serve)"
    );
    assert!(matches!(past, ZookieStaleness::PastBound { .. }));
    let past_lag = past.observed_staleness_secs();
    assert_eq!(past_lag, 700);

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::FailStaticStalenessSecs, within_lag as i64);
    sig.assert_signal(
        SignalName::FailStaticStalenessSecs,
        Predicate::Lte(ZOOKIE_STALENESS_BUDGET_SECS as i64),
    )
    .expect_green();
    let mut sig_red = SignalSource::new();
    sig_red.set_scalar(SignalName::FailStaticStalenessSecs, past_lag as i64);
    assert!(
        !sig_red
            .assert_signal(
                SignalName::FailStaticStalenessSecs,
                Predicate::Lte(ZOOKIE_STALENESS_BUDGET_SECS as i64)
            )
            .is_green(),
        "a read past the bound exceeds the staleness budget - the read-through REFUSES it"
    );

    println!(
        "[P-430 GA-D8 zookie-consistency GREEN 2026-06-24] cross-cell zookie consistency (the hardest \
         sub-problem, §6.3): a zookie minted in the home cell (cell-b@snap-1042) read in a member cell \
         observed {within_lag}s staleness ≤ {ZOOKIE_STALENESS_BUDGET_SECS}s budget → admitted \
         bounded-stale; the SAME zookie read at {past_lag}s staleness > budget → REFUSED (never a \
         stale-read past the bound; the cross-cell new-enemy guard)."
    );
}
