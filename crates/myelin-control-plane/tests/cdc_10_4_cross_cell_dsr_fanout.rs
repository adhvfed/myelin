//! P-CP-20 (global P-430) CDC — **contract 10.4 cross-cell DSR fan-out (provider + consumer): the DSR
//! orchestrator iterating `member_cells ∪ home_cell`.**
//!
//! The PROVIDER is this crate's [`CrossCellDsrFanOut`] driving an erase across a tenant's
//! `member_cells ∪ home_cell` over the bridge transport, merging a complete [`MultiCellDsrReceiptSet`].
//! The CONSUMER stands in for the **GDPR DSR orchestrator** (contract 10.4 — `dsr_submit` iterates
//! `member_cells`, gdpr §4.1 step 2): it takes the merged receipt set and folds it into the DSR's
//! per-holder receipt list + asserts the multi-cell completeness invariant (0 cells missed) BEFORE the
//! DSR may verify. It can read ONLY the PII-free receipt-set fields (the opaque subject/tenant/cells +
//! the opaque per-cell receipt tokens) — there is no raw row / PII on the receipt set. If the receipt-set
//! shape drifts (a field added/removed/retyped), this consumer stops compiling — the point of a
//! glue-crate CDC.
//!
//! This pins the seam the GDPR `dsr_submit` (`myelin-gdpr-service`) consumes when a DSR spans
//! `member_cells`: the control plane owns the cross-cell fan-out MECHANISM; GDPR owns the DSR state
//! machine + the certificate. One fan-out rule, consumed across the seam (EI-01 §7).

use std::sync::Arc;

use myelin_control_plane::{
    CellDsrReceipt, CellLocalEraser, CrossCellDsrFanOut, MultiCellDsrReceiptSet,
};
use myelin_events::Timestamp;
use myelin_tenancy::{ArtifactRef, CellId, OpaqueSubjectId, TenantId};

/// A member cell's erase seam (the provider side's per-cell eraser).
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

/// **The CONSUMER: a stand-in GDPR DSR orchestrator folding the cross-cell fan-out into the DSR (the
/// 10.4 read side).** It reads ONLY the PII-free receipt-set fields; it asserts the multi-cell
/// completeness invariant (0 cells missed) is met BEFORE the DSR may move to verified.
struct DsrFanOutConsumer {
    /// The per-cell receipt tokens the DSR will seal into its certificate (one per member cell).
    collected_receipts: Vec<String>,
    /// Whether the multi-cell fan-out is COMPLETE (0 cells missed) — the gate before `verify`.
    fan_out_complete: bool,
}

impl DsrFanOutConsumer {
    /// Fold a merged [`MultiCellDsrReceiptSet`] into the DSR (the 10.4 consume) — read the PII-free
    /// per-cell receipts + the completeness verdict. A DSR may NOT verify over an incomplete fan-out
    /// (a missed cell is stop-the-bleeding — the GDPR machine cannot declare an erasure done).
    fn consume(set: &MultiCellDsrReceiptSet) -> DsrFanOutConsumer {
        DsrFanOutConsumer {
            collected_receipts: set.receipts.iter().map(|r| r.receipt.clone()).collect(),
            fan_out_complete: set.is_complete(),
        }
    }
}

/// **CDC: provider+consumer for 10.4 cross-cell fan-out — the DSR orchestrator iterating
/// `member_cells`.** A complete multi-cell fan-out (0 cells missed) folds into the DSR's receipt list
/// and admits the DSR to verify.
#[test]
fn cdc_10_4_cross_cell_dsr_fan_out_provider_consumer() {
    // PROVIDER: a multi-cell tenant (home cell-b, members cell-c + cell-d) — all reachable.
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

    // CONSUMER: the GDPR DSR orchestrator folds the merged set in through the frozen 10.4 shape.
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

/// **The completeness invariant is LOAD-BEARING across the seam: an INCOMPLETE fan-out blocks the DSR
/// from verifying.** A member cell the provider could not reach makes `is_complete() == false`, and the
/// consumer (the DSR orchestrator) MUST NOT verify over it — a missed cell is stop-the-bleeding.
#[test]
fn cdc_10_4_incomplete_fan_out_blocks_dsr_verify() {
    let mut fanout = CrossCellDsrFanOut::new();
    fanout.register(
        CellId::from_token("cell-b"),
        Arc::new(MemberCell {
            cell: CellId::from_token("cell-b"),
        }),
    );
    // cell-c is unreachable (not registered).
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
