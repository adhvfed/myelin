//! # P-GA-33 → P-449 — GA-D8 at cell scale: the multi-cell DSR fan-out (0 cells missed)
//!
//! **DATED GREEN ARTIFACT (2026-06-24).** This integration drill is the dated green artifact the
//! P-GA-33 GATE requires — the **multi-cell erasure drill (GA-D8, SCHED/FLOOR)**: a subject seeded
//! across **`member_cells ∪ home_cell`** → a single multi-cell `dsr_submit` → the fan-out **iterates
//! all member cells** → each cell erases its OWN H1–H18 holders (cell-local resolution over the
//! PII-free `CrossCellPointer` bridge) + returns a PII-free per-cell receipt → the receipts **merge
//! into ONE certificate** → **0 cells missed**. The **per-cell receipt set is the green artifact**
//! (the GA-D8 catalogue row). The Merkle inclusion of the merged certificate rides P-GA-20.
//!
//! ## What this proves (the P-GA-33 multi-cell gate) vs what it REUSES (EI-01 §7 coherence)
//! P-GA-32 ([`full_fanout`]) proved the SINGLE-cell completeness (0 holders missed over the WHOLE
//! H1–H18 catalogue, per cell). This drill closes the *cells-missed* gap exactly as P-GA-32 closed
//! the *holders-missed* gap: it runs the EXISTING single-cell completeness layer IN each cell and
//! measures coverage against the WHOLE `member_cells ∪ home_cell` set — so a cell the fan-out did not
//! reach (or did not fully erase) is **MISSED**, never silently complete over a partial cell set.
//!
//! ## The two faces of the gate (green AND red — the gate can go red)
//! 1. **GREEN:** the full `member_cells ∪ home_cell` set, each cell fully erased → 0 cells missed →
//!    the merged certificate seals.
//! 2. **RED:** a wave that drops ONE member cell → that cell is MISSED → the certificate REFUSES to
//!    seal (returns a [`MultiCellGap`] naming the missed cell). A drill that cannot go red proves
//!    nothing — this asserts the gate IS load-bearing.
//!
//! ## Cell-local resolution (OQ-I — the structural PII-free invariant proven here)
//! The carrier that crosses every cell boundary is the four-field PII-free `CrossCellPointer`
//! (`subject` is an opaque `ArtifactRef`-class id, NEVER a person). The orchestrator passes ONLY the
//! pointer to each cell and receives ONLY a PII-free per-cell certificate — it NEVER reads a cell's
//! personal data. This drill asserts a cell only ever sees the PII-free pointer (no name/email
//! accessor exists on it) and the merged certificate carries only opaque cell ids + content hashes.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The cross-cell ordering/atomicity** (a *globally-atomic* multi-cell erase, vs the
//!   resumable-per-cell checklist) remains the **control-plane floor even at M5** — the orchestrator
//!   runs in EACH cell; the control plane sequences the wave, never holding PII. Named owner:
//!   **P6 control-plane + multi-cell tenancy** (architecture §4.3 / §8). A partial-wave failure
//!   surfaces as `cells_missed > 0` (re-driven by the control plane), NOT rolled back here.
//! - **The E2E-4 DSAR flagship** (the whole-system proof across all five subsystems + `member_cells ∪
//!   home_cell` with mock agents) → **P-GA-34 → P-450**. THIS is the multi-cell merge leg that
//!   flagship exercises.
//! - **The live per-cell store-`erase` bindings + the live cross-cell transport** are the same
//!   in-memory model floor every M1 store carries (P-007/P-S12). This drill proves the multi-cell
//!   COMPLETENESS PROPERTY over the cell set + the PII-free carrier — a property that is load- and
//!   transport-independent — touching NO new DB/object-store/cache/bus contract, so no
//!   `--features integration` live-stack leg is owed by P-GA-33.
//! - **The world-scale 30× load** of the whole-cell SCHED drill is the one remaining real-fleet floor
//!   (the completeness PROPERTY here is load-independent — a property of the cell set + the per-cell
//!   catalogue).

use myelin_gdpr_service::full_fanout::{FullFanOutCoverage, GaD1Certificate, Holder};
use myelin_gdpr_service::{
    MemberCellSet, MultiCellCertificate, MultiCellCoverage, MultiCellFanOut, PerCellReceipt,
};
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId,
};

fn cell(token: &str) -> CellId {
    CellId::from_token(token)
}

/// A cell-local single-cell fan-out: seed the subject into all H1–H18 holders IN this cell, run the
/// completeness layer, and seal the cell's GA-D1 certificate (0 holders missed IN the cell). This is
/// the per-cell work `resolve_in_cell` performs — the cell erases its OWN holders, returns a PII-free
/// certificate.
fn erase_in_cell(scope_token: &str) -> GaD1Certificate {
    let mut cov = FullFanOutCoverage::new();
    // the cell's data map declares all H1–H18 holders (every holder finally exists at M5).
    for &h in Holder::ALL {
        cov.record_reached(h);
    }
    assert_eq!(cov.holders_missed(), 0, "the cell erased every holder");
    GaD1Certificate::seal(scope_token, &cov).expect("the cell's fan-out seals")
}

/// The PII-free cross-cell carrier the orchestrator passes to each cell.
fn pii_free_pointer(home: &str) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
        ArtifactType::Issue,
        CorrelationId("corr-dsr-1".into()),
        cell(home),
    )
}

/// A **cell-scale** `member_cells ∪ home_cell` set (a multi-cell tenant spread across several
/// same-region cells in `fr-par`).
fn cell_scale_member_set() -> MemberCellSet {
    let home = cell("cell-fr-par-1");
    let members = vec![
        cell("cell-fr-par-2"),
        cell("cell-fr-par-3"),
        cell("cell-fr-par-4"),
        cell("cell-fr-par-5"),
    ];
    MemberCellSet::union(home, &members)
}

/// **GA-D8 GREEN — the multi-cell fan-out iterates all `member_cells ∪ home_cell`; the per-cell
/// receipts merge into one certificate; 0 cells missed.** The dated green artifact.
#[test]
fn ga_d8_multi_cell_fan_out_0_cells_missed() {
    let set = cell_scale_member_set();
    let pointer = pii_free_pointer("cell-fr-par-1");
    let mut cells_resolved: Vec<String> = Vec::new();

    let cert = MultiCellFanOut::new()
        .fan_out("acme/u-1", &set, &pointer, |c, p| {
            // cell-local resolution over the PII-free pointer: the cell receives ONLY the opaque
            // cell id + the four-field PII-free pointer (no name/email accessor exists on it).
            assert_eq!(
                p.subject().artifact_ref().0,
                "myelin://01J0ACME/issues/issue/42",
                "the carrier is the opaque artifact ref, never a person"
            );
            cells_resolved.push(c.as_str().to_string());
            erase_in_cell(&format!("acme/u-1@{}", c.as_str()))
        })
        .expect("the complete multi-cell fan-out seals");

    // GATE: 0 cells missed over `member_cells ∪ home_cell`.
    assert_eq!(cert.cells_missed, 0, "GA-D8: 0 cells missed");
    assert_eq!(cert.cells_total, 5, "home ∪ 4 members = 5 cells");
    // the per-cell receipt set is the green artifact — one PII-free receipt per cell.
    assert_eq!(cert.per_cell.len(), 5, "complete per-cell receipt set");
    assert!(cert.is_complete(), "the merged certificate is complete");
    assert!(cert.content_hash.starts_with("blake3:"));
    // every cell was resolved cell-locally (not one skipped).
    assert_eq!(cells_resolved.len(), 5, "every cell fanned out");
    for c in [
        "cell-fr-par-1",
        "cell-fr-par-2",
        "cell-fr-par-3",
        "cell-fr-par-4",
        "cell-fr-par-5",
    ] {
        assert!(cells_resolved.contains(&c.to_string()), "{c} was reached");
    }
    // every per-cell receipt is itself complete (0 holders missed IN each cell).
    assert!(
        cert.per_cell.iter().all(PerCellReceipt::cell_is_complete),
        "every cell fully erased its own H1–H18 holders"
    );
    // MEASURED NUMBERS (the dated artifact body): 5 cells, 5 receipts, 0 missed, every cell 0-holders-missed.
    eprintln!(
        "GA-D8 (2026-06-24): cells_total={} per_cell_receipts={} cells_missed={} cert={}",
        cert.cells_total,
        cert.per_cell.len(),
        cert.cells_missed,
        cert.content_hash,
    );
}

/// **GA-D8 RED — a wave that drops ONE member cell is detected: `cells_missed == 1`, the missed cell
/// named, the certificate REFUSES to seal.** The gate is load-bearing (a missed cell un-erases a
/// person in that cell — the multi-cell load-bearing zero).
#[test]
fn ga_d8_a_dropped_cell_is_missed_and_refuses_to_seal() {
    let set = cell_scale_member_set();
    let mut cov = MultiCellCoverage::new(set);
    // the wave reaches every cell EXCEPT cell-fr-par-4 (a control-plane wave that stalled mid-fan).
    for c in [
        "cell-fr-par-1",
        "cell-fr-par-2",
        "cell-fr-par-3",
        "cell-fr-par-5",
    ] {
        cov.record_receipt(PerCellReceipt::new(
            cell(c),
            erase_in_cell(&format!("acme/u-1@{c}")),
        ));
    }
    assert_eq!(cov.cells_missed(), 1, "the dropped cell is COUNTED");
    assert_eq!(
        cov.missed(),
        vec![cell("cell-fr-par-4")],
        "named: cell-fr-par-4"
    );
    let gap = MultiCellCertificate::seal("acme/u-1", &cov)
        .expect_err("a missed cell does NOT seal a green artifact");
    assert_eq!(gap.cells_missed, 1);
    assert_eq!(gap.missed, vec![cell("cell-fr-par-4")]);
    assert_eq!(gap.cells_total, 5);
}

/// **GA-D8 — a cell that returns a receipt but did NOT fully erase its own holders is MISSED** (the
/// merge requires per-cell completeness, not merely a present receipt — a half-erased cell leaves a
/// person partially recoverable in that cell).
#[test]
fn ga_d8_a_half_erased_cell_is_missed() {
    let set = MemberCellSet::union(cell("cell-fr-par-1"), &[cell("cell-fr-par-2")]);
    let mut cov = MultiCellCoverage::new(set);
    cov.record_receipt(PerCellReceipt::new(
        cell("cell-fr-par-1"),
        erase_in_cell("acme/u@1"),
    ));
    // cell-fr-par-2 returns a receipt, but its inner fan-out missed a holder (tampered to incomplete).
    let mut half = erase_in_cell("acme/u@2");
    half.holders_missed = 1;
    half.erasure_fanout_coverage = 17.0 / 18.0;
    half.reach[0].reached = false;
    cov.record_receipt(PerCellReceipt::new(cell("cell-fr-par-2"), half));

    assert_eq!(cov.cells_missed(), 1, "a half-erased cell is a missed cell");
    assert_eq!(cov.missed(), vec![cell("cell-fr-par-2")]);
    assert!(MultiCellCertificate::seal("acme/u", &cov).is_err());
}
