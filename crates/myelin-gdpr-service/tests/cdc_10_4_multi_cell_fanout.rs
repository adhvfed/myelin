use myelin_gdpr_service::full_fanout::{FullFanOutCoverage, GaD1Certificate, Holder};
use myelin_gdpr_service::{MemberCellSet, MultiCellCertificate, MultiCellFanOut, PerCellReceipt};
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId,
};

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

fn pii_free_pointer(home: &str) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
        ArtifactType::Issue,
        CorrelationId("corr-dsr-1".into()),
        cell(home),
    )
}

#[test]
fn multi_cell_submitter_fans_out_across_member_cells_and_merges() {
    let home = cell("cell-fr-par-1");
    let members = vec![cell("cell-fr-par-2"), cell("cell-fr-par-3")];
    let set = MemberCellSet::union(home, &members);
    let pointer = pii_free_pointer("cell-fr-par-1");

    let cert = MultiCellFanOut::new()
        .fan_out("acme/u-1", &set, &pointer, |c, p| {
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
    let gap = MultiCellCertificate::seal("acme/u", &cov)
        .expect_err("a missed cell does NOT produce a certificate");
    assert_eq!(gap.cells_missed, 1);
    assert_eq!(gap.missed, vec![cell("cell-fr-par-3")]);
    assert_eq!(gap.cells_total, 3);
}

#[test]
fn the_cross_cell_carrier_is_pii_free_four_field() {
    let p = pii_free_pointer("cell-fr-par-9");
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
