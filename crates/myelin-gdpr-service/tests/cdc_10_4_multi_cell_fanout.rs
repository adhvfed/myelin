//! # CDC 10.4 — the multi-cell DSR fan-out orchestrator + the cross-cell PII-free bridge
//! (P-GA-33 → P-449)
//!
//! **Contract:** index row 10.4 — the **multi-cell `member_cells` iteration** (completing the
//! single-cell P-GA-14 floor) over the contract-12.3/12.6 cross-cell PII-free `CrossCellPointer`
//! bridge. This is the consumer-driven contract test the coverage scanner reads both halves of:
//!
//! - **provider** = the [`MultiCellFanOut`] orchestrator (§4.3): given the `member_cells ∪ home_cell`
//!   set ([`MemberCellSet`]) + the PII-free [`CrossCellPointer`] carrier, it iterates EVERY cell,
//!   resolves cell-locally (each cell erases its OWN holders, returns only a PII-free
//!   [`PerCellReceipt`]), and merges the receipts into ONE [`MultiCellCertificate`] (0 cells missed).
//! - **consumer** = (a) a **multi-cell DSR submitter** running one fan-out across `member_cells ∪
//!   home_cell` and reading the merged certificate (a complete per-cell receipt set, 0 cells missed);
//!   (b) an **auditor** verifying the per-cell receipts merge into a content-addressed certificate
//!   that refuses to seal when a cell is missed; (c) the **cross-cell bridge** (the
//!   `CrossCellPointer` carrier) proving the carrier the orchestrator passes to each cell is PII-free
//!   (a fifth, PII-bearing field cannot be smuggled onto the frame — the frozen four-field shape).
//!
//! The dated green artifact: a subject seeded across `member_cells ∪ home_cell` → one multi-cell
//! fan-out → 0 cells missed → the per-cell receipts merge into one content-addressed certificate; a
//! missed cell REFUSES to seal. If 10.4's multi-cell iteration shape, the cell-local resolution, or
//! the PII-free bridge frame drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr_service::full_fanout::{FullFanOutCoverage, GaD1Certificate, Holder};
use myelin_gdpr_service::{MemberCellSet, MultiCellCertificate, MultiCellFanOut, PerCellReceipt};
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId,
};

/// PROVIDER helper: a complete single-cell GA-D1 certificate (every H1–H18 reached IN the cell).
fn complete_cell_cert(scope: &str) -> GaD1Certificate {
    let mut cov = FullFanOutCoverage::new();
    for &h in Holder::ALL {
        cov.record_reached(h);
    }
    GaD1Certificate::seal(scope, &cov).expect("a complete cell fan-out seals")
}

fn cell(token: &str) -> CellId {
    CellId::from_token(token)
}

/// The PII-free cross-cell carrier — the `CrossCellPointer` the orchestrator passes to each cell. Its
/// `subject` is an opaque `ArtifactRef`-class id (NEVER a person).
fn pii_free_pointer(home: &str) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
        ArtifactType::Issue,
        CorrelationId("corr-dsr-1".into()),
        cell(home),
    )
}

/// **CONSUMER (a): a multi-cell DSR submitter runs ONE fan-out across `member_cells ∪ home_cell` and
/// reads the merged certificate — 0 cells missed, one PII-free receipt per cell.**
#[test]
fn multi_cell_submitter_fans_out_across_member_cells_and_merges() {
    let home = cell("cell-fr-par-1");
    let members = vec![cell("cell-fr-par-2"), cell("cell-fr-par-3")];
    let set = MemberCellSet::union(home, &members);
    let pointer = pii_free_pointer("cell-fr-par-1");

    let cert = MultiCellFanOut::new()
        .fan_out("acme/u-1", &set, &pointer, |c, p| {
            // PROVIDER: cell-local resolution — the cell receives ONLY the opaque cell id + the
            // PII-free pointer, erases its OWN holders, returns a PII-free certificate.
            assert_eq!(
                p.subject().artifact_ref().0,
                "myelin://01J0ACME/issues/issue/42"
            );
            complete_cell_cert(&format!("acme/u-1@{}", c.as_str()))
        })
        .expect("a complete multi-cell fan-out seals");

    assert_eq!(cert.cells_missed, 0, "0 cells missed");
    assert_eq!(cert.cells_total, 3, "member_cells ∪ home_cell = 3 cells");
    assert_eq!(cert.per_cell.len(), 3, "one PII-free receipt per cell");
    assert!(cert.is_complete());
    assert!(cert.content_hash.starts_with("blake3:"));
}

/// **CONSUMER (b): an auditor verifies a MISSED cell refuses to seal** — the per-cell receipt set is
/// incomplete, the certificate returns a gap naming the missed cell.
#[test]
fn auditor_a_missed_cell_refuses_to_seal() {
    let home = cell("cell-fr-par-1");
    let set = MemberCellSet::union(home, &[cell("cell-fr-par-2"), cell("cell-fr-par-3")]);
    let mut cov = myelin_gdpr_service::MultiCellCoverage::new(set);
    cov.record_receipt(PerCellReceipt::new(
        cell("cell-fr-par-1"),
        complete_cell_cert("acme/u@1"),
    ));
    cov.record_receipt(PerCellReceipt::new(
        cell("cell-fr-par-2"),
        complete_cell_cert("acme/u@2"),
    ));
    // cell-fr-par-3 never reported — the gate is RED.
    let gap = MultiCellCertificate::seal("acme/u", &cov)
        .expect_err("a missed cell does NOT produce a certificate");
    assert_eq!(gap.cells_missed, 1);
    assert_eq!(gap.missed, vec![cell("cell-fr-par-3")]);
    assert_eq!(gap.cells_total, 3);
}

/// **CONSUMER (c): the cross-cell carrier is PII-free** — the `CrossCellPointer` exposes exactly the
/// four frozen fields; `subject` is an opaque `ArtifactRef` (no name/email accessor). The negative
/// half — a fifth PII field cannot be added to the frame — is the `compile_fail` doc-test on the
/// frozen type in `myelin-tenancy` (this asserts the positive half: the frame is exactly four PII-free
/// fields the orchestrator passes between cells).
#[test]
fn the_cross_cell_carrier_is_pii_free_four_field() {
    let p = pii_free_pointer("cell-fr-par-9");
    // exactly the four frozen PII-free fields are readable — `subject` is an opaque artifact ref.
    let _subject: &OpaqueSubjectId = p.subject();
    let _type: &ArtifactType = p.artifact_type();
    let _corr: &CorrelationId = p.correlation_id();
    let _home: &CellId = p.home_cell();
    assert_eq!(
        p.subject().artifact_ref().0,
        "myelin://01J0ACME/issues/issue/42",
        "the subject is an opaque artifact ref, never a person"
    );
    assert_eq!(p.home_cell().as_str(), "cell-fr-par-9");
}
