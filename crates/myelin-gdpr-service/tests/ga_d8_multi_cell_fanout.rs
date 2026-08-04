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

fn erase_in_cell(scope_token: &str) -> GaD1Certificate {
    let mut cov = FullFanOutCoverage::new();
    for &h in Holder::ALL {
        cov.record_reached(h);
    }
    assert_eq!(cov.holders_missed(), 0, "the cell erased every holder");
    GaD1Certificate::seal(scope_token, &cov).expect("the cell's fan-out seals")
}

fn pii_free_pointer(home: &str) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
        ArtifactType::Issue,
        CorrelationId("corr-dsr-1".into()),
        cell(home),
    )
}

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

#[test]
fn ga_d8_multi_cell_fan_out_0_cells_missed() {
    let set = cell_scale_member_set();
    let pointer = pii_free_pointer("cell-fr-par-1");
    let mut cells_resolved: Vec<String> = Vec::new();

    let cert = MultiCellFanOut::new()
        .fan_out("acme/u-1", &set, &pointer, |c, p| {
            assert_eq!(
                p.subject().artifact_ref().0,
                "myelin://01J0ACME/issues/issue/42",
                "the carrier is the opaque artifact ref, never a person"
            );
            cells_resolved.push(c.as_str().to_string());
            erase_in_cell(&format!("acme/u-1@{}", c.as_str()))
        })
        .expect("the complete multi-cell fan-out seals");

    assert_eq!(cert.cells_missed, 0, "GA-D8: 0 cells missed");
    assert_eq!(cert.cells_total, 5, "home ∪ 4 members = 5 cells");
    assert_eq!(cert.per_cell.len(), 5, "complete per-cell receipt set");
    assert!(cert.is_complete(), "the merged certificate is complete");
    assert!(cert.content_hash.starts_with("blake3:"));
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
    assert!(
        cert.per_cell.iter().all(PerCellReceipt::cell_is_complete),
        "every cell fully erased its own H1–H18 holders"
    );
    eprintln!(
        "GA-D8 (2026-06-24): cells_total={} per_cell_receipts={} cells_missed={} cert={}",
        cert.cells_total,
        cert.per_cell.len(),
        cert.cells_missed,
        cert.content_hash,
    );
}

#[test]
fn ga_d8_a_dropped_cell_is_missed_and_refuses_to_seal() {
    let set = cell_scale_member_set();
    let mut cov = MultiCellCoverage::new(set);
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

#[test]
fn ga_d8_a_half_erased_cell_is_missed() {
    let set = MemberCellSet::union(cell("cell-fr-par-1"), &[cell("cell-fr-par-2")]);
    let mut cov = MultiCellCoverage::new(set);
    cov.record_receipt(PerCellReceipt::new(
        cell("cell-fr-par-1"),
        erase_in_cell("acme/u@1"),
    ));
    let mut half = erase_in_cell("acme/u@2");
    half.holders_missed = 1;
    half.erasure_fanout_coverage = 17.0 / 18.0;
    half.reach[0].reached = false;
    cov.record_receipt(PerCellReceipt::new(cell("cell-fr-par-2"), half));

    assert_eq!(cov.cells_missed(), 1, "a half-erased cell is a missed cell");
    assert_eq!(cov.missed(), vec![cell("cell-fr-par-2")]);
    assert!(MultiCellCertificate::seal("acme/u", &cov).is_err());
}
