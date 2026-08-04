use myelin_events::{ArtifactRef, CellId, CorrelationId, Region, TenantId};
use myelin_issues::CrossCellDsrFanout;

#[test]
fn ga_d1_cp_d7_cp_d8_dsr_reaches_every_member_cell_zero_pii() {
    let origin = CellId::from_token("cell-fr-par-1");
    let dsr = CrossCellDsrFanout::new(origin.clone());

    let member_cells = vec![
        CellId::from_token("cell-fr-par-1"),
        CellId::from_token("cell-de-fra-1"),
        CellId::from_token("cell-nl-ams-1"),
    ];
    let subject = ArtifactRef("myelin://acme/identity/pseudonym/p-anon-7".into());

    let acked = std::cell::RefCell::new(Vec::<String>::new());
    let receipts = dsr.fan_out_erasure(
        &TenantId("acme".into()),
        &Region("fr-par".into()),
        &subject,
        &CorrelationId("dsr-correlation-root".into()),
        &member_cells,
        &|cell, subj| {
            acked.borrow_mut().push(cell.as_str().to_string());
            assert_eq!(
                subj, &subject,
                "the cell-local erase keys on the opaque subject"
            );
            true
        },
    );

    assert_eq!(receipts.len(), 3, "one receipt per member cell");
    assert_eq!(
        acked.borrow().len(),
        3,
        "the fan-out reached every member cell's cell-local erasure"
    );

    assert!(
        CrossCellDsrFanout::reached_every_cell(&receipts, &member_cells),
        "GA-D1: 0 member cell missed - the erasure reached every cell"
    );

    assert_eq!(
        dsr.pii_crossed(),
        0,
        "CP-D8: 0 PII crosses the cross-cell DSR bridge"
    );

    for r in &receipts {
        assert_eq!(r.subject, subject);
        assert!(
            r.acknowledged,
            "every member cell acknowledged its cell-local erasure"
        );
    }
}

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
        &|cell, _subj| cell.as_str() != "cell-nl-ams-1",
    );

    assert!(
        !CrossCellDsrFanout::reached_every_cell(&receipts, &member_cells),
        "an unacknowledged member cell is a LOUD fan-out gap (never a silent recoverable residual)"
    );
    assert_eq!(dsr.pii_crossed(), 0);
}
