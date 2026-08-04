use std::sync::Arc;

use myelin_control_plane::{
    CellDsrReceipt, CellLocalEraser, CrossCellDsrFanOut, MultiCellDsrReceiptSet,
};
use myelin_events::Timestamp;
use myelin_tenancy::{ArtifactRef, CellId, OpaqueSubjectId, TenantId};

struct MemberCell {
    cell: CellId,
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
            receipt: format!("receipt:{}", self.cell.as_str()),
        }
    }
}

struct DsrFanOutConsumer {
    collected_receipts: Vec<String>,
    fan_out_complete: bool,
}

impl DsrFanOutConsumer {
    fn consume(set: &MultiCellDsrReceiptSet) -> DsrFanOutConsumer {
        DsrFanOutConsumer {
            collected_receipts: set.receipts.iter().map(|r| r.receipt.clone()).collect(),
            fan_out_complete: set.is_complete(),
        }
    }
}

#[test]
fn cdc_10_4_cross_cell_dsr_fan_out_provider_consumer() {
    let mut fanout = CrossCellDsrFanOut::new();
    fanout.register(
        CellId::from_token("cell-b"),
        Arc::new(MemberCell {
            cell: CellId::from_token("cell-b"),
        }),
    );
    fanout.register(
        CellId::from_token("cell-c"),
        Arc::new(MemberCell {
            cell: CellId::from_token("cell-c"),
        }),
    );
    fanout.register(
        CellId::from_token("cell-d"),
        Arc::new(MemberCell {
            cell: CellId::from_token("cell-d"),
        }),
    );
    let set = fanout.fan_out(
        &OpaqueSubjectId::from_ref(ArtifactRef("p1".into())),
        &TenantId::from_token("01J0ACME"),
        &CellId::from_token("cell-b"),
        &[CellId::from_token("cell-c"), CellId::from_token("cell-d")],
        Timestamp("2026-06-24T00:00:00Z".into()),
    );

    let consumer = DsrFanOutConsumer::consume(&set);
    assert_eq!(
        consumer.collected_receipts.len(),
        3,
        "one per-cell receipt per member cell (home ∪ members)"
    );
    assert!(
        consumer.fan_out_complete,
        "a complete fan-out (0 cells missed) admits the DSR to verify"
    );
}

#[test]
fn cdc_10_4_incomplete_fan_out_blocks_dsr_verify() {
    let mut fanout = CrossCellDsrFanOut::new();
    fanout.register(
        CellId::from_token("cell-b"),
        Arc::new(MemberCell {
            cell: CellId::from_token("cell-b"),
        }),
    );
    let set = fanout.fan_out(
        &OpaqueSubjectId::from_ref(ArtifactRef("p1".into())),
        &TenantId::from_token("01J0ACME"),
        &CellId::from_token("cell-b"),
        &[CellId::from_token("cell-c")],
        Timestamp("2026-06-24T00:00:00Z".into()),
    );
    let consumer = DsrFanOutConsumer::consume(&set);
    assert!(
        !consumer.fan_out_complete,
        "an incomplete fan-out (a missed cell) MUST block the DSR from verifying"
    );
    assert_eq!(set.cells_missed(), 1);
}
