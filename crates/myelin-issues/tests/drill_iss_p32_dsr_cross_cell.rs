//! **ISS-P32 / P-495 (M5) — GA-D1 / CP-D7 / CP-D8: the cross-cell DSR fan-out drill (the SCHED green
//! artifact).**
//!
//! The drill catalogue rows GA-D1 / CP-D7 / CP-D8: a multi-cell tenant's right-to-erasure (DSR) fan-out
//! reaches every member cell's Issues holders, with a per-cell receipt set, and 0 PII crosses the
//! bridge. This drill drives the [`CrossCellDsrFanout`] across a three-cell tenant and asserts:
//! - **GA-D1 / CP-D7 — 0 member cell missed.** Every `member_cells` entry has a per-cell receipt and
//!   acknowledged its cell-local erasure; `reached_every_cell` is true.
//! - **CP-D8 / GA-D8 — 0 PII crosses.** The fan-out carries ONLY the four-field PII-free
//!   `CrossCellPointer` (the opaque pseudonymised subject + type + correlation + the destination
//!   home_cell) — never the subject's name/email; the `pii_crossed` tripwire stays 0.
//! - **A fan-out GAP is LOUD.** A member cell that fails to acknowledge fails the gate (never a silent
//!   recoverable residual — a GDPR breach).
//!
//! **Reconciliation (EI-01 §7).** The cross-cell bridge is the frozen `myelin_events::CrossCellPointer`
//! (contract 12.6) + the Bus's `IssuePortfolio` propagation stream (EB-25) — consumed, never a second
//! frame. The per-cell erasure runs cell-local through each cell's own ISS-P31
//! `holder_erase::IssueEraseFanout` (the acknowledgement) — this drill owns the cross-cell FAN-OUT, not
//! a second erase body.
//!
//! **Contract 10.4 consumed** — the DSR fan-out iterating `member_cells`.

use myelin_events::{ArtifactRef, CellId, CorrelationId, Region, TenantId};
use myelin_issues::CrossCellDsrFanout;

/// **GA-D1 / CP-D7 / CP-D8: a multi-cell DSR fan-out reaches every member cell, per-cell receipt set,
/// 0 PII crosses.** A three-cell tenant; the DSR originates in cell A; every cell (incl. the origin)
/// runs its cell-local ISS-P31 erasure and acknowledges. The receipt set is complete; 0 PII crosses.
#[test]
fn ga_d1_cp_d7_cp_d8_dsr_reaches_every_member_cell_zero_pii() {
    let origin = CellId::from_token("cell-fr-par-1");
    let dsr = CrossCellDsrFanout::new(origin.clone());

    let member_cells = vec![
        CellId::from_token("cell-fr-par-1"), // the origin cell
        CellId::from_token("cell-de-fra-1"),
        CellId::from_token("cell-nl-ams-1"),
    ];
    // the subject is the OPAQUE pseudonymised id (the frozen `<pseudonym>@<tenant>.noreply` grammar) —
    // never a person's name/email; the member cell crypto-shreds the actual PII cell-local.
    let subject = ArtifactRef("myelin://acme/identity/pseudonym/p-anon-7".into());

    // track which cells were asked to erase (proving the fan-out reached each one) — the per-cell
    // cell-local ISS-P31 erase acknowledgement.
    let acked = std::cell::RefCell::new(Vec::<String>::new());
    let receipts = dsr.fan_out_erasure(
        &TenantId("acme".into()),
        &Region("fr-par".into()),
        &subject,
        &CorrelationId("dsr-correlation-root".into()),
        &member_cells,
        &|cell, subj| {
            acked.borrow_mut().push(cell.as_str().to_string());
            // the member cell's cell-local ISS-P31 fan-out ran over the OPAQUE subject (never PII).
            assert_eq!(
                subj, &subject,
                "the cell-local erase keys on the opaque subject"
            );
            true
        },
    );

    // CP-D7: a per-cell receipt for EVERY member cell.
    assert_eq!(receipts.len(), 3, "one receipt per member cell");
    assert_eq!(
        acked.borrow().len(),
        3,
        "the fan-out reached every member cell's cell-local erasure"
    );

    // GA-D1 / CP-D7: 0 member cell missed.
    assert!(
        CrossCellDsrFanout::reached_every_cell(&receipts, &member_cells),
        "GA-D1: 0 member cell missed — the erasure reached every cell"
    );

    // CP-D8 / GA-D8: 0 PII crosses the bridge (only the four-field PII-free frame).
    assert_eq!(
        dsr.pii_crossed(),
        0,
        "CP-D8: 0 PII crosses the cross-cell DSR bridge"
    );

    // every receipt carries the OPAQUE subject pointer, never the subject's PII.
    for r in &receipts {
        assert_eq!(r.subject, subject);
        assert!(
            r.acknowledged,
            "every member cell acknowledged its cell-local erasure"
        );
    }
}

/// **A fan-out GAP is a LOUD gate failure (never a silent residual).** If one member cell fails to
/// acknowledge its cell-local erasure, `reached_every_cell` is false — the DSR fan-out is incomplete
/// and the gate FAILS (a GDPR fan-out gap is surfaced, never silently passed).
#[test]
fn dsr_fan_out_gap_is_a_loud_gate_failure() {
    let dsr = CrossCellDsrFanout::new(CellId::from_token("cell-fr-par-1"));
    let member_cells = vec![
        CellId::from_token("cell-fr-par-1"),
        CellId::from_token("cell-de-fra-1"),
        CellId::from_token("cell-nl-ams-1"),
    ];
    let subject = ArtifactRef("myelin://acme/identity/pseudonym/p-anon-7".into());

    let receipts = dsr.fan_out_erasure(
        &TenantId("acme".into()),
        &Region("fr-par".into()),
        &subject,
        &CorrelationId("dsr-correlation-root".into()),
        &member_cells,
        // the nl-ams cell FAILS to acknowledge (e.g. unreachable) — a fan-out gap.
        &|cell, _subj| cell.as_str() != "cell-nl-ams-1",
    );

    assert!(
        !CrossCellDsrFanout::reached_every_cell(&receipts, &member_cells),
        "an unacknowledged member cell is a LOUD fan-out gap (never a silent recoverable residual)"
    );
    // even on a gap, 0 PII crossed (the gap is an incomplete erase, not a leak).
    assert_eq!(dsr.pii_crossed(), 0);
}
