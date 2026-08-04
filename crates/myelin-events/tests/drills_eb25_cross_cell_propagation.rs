use myelin_events::{Actor, AggregateKey, CorrelationId};
use myelin_events::{
    ArtifactRef, CellId, CrossCellPropagator, CrossCellStream, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn iss_envelope_with_payload_pii() -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J0EVT".into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        subject: ArtifactRef("myelin://01J0ACME/issues/issue/42".into()),
        aggregate: AggregateKey("issue:PROJ-1".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J0CHAIN".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: true,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        payload: serde_json::json!({ "assignee_email": "alice@example.com", "body": "the secret plan" }),
    }
}

#[test]
fn eb25_cross_cell_propagation_carries_zero_pii() {
    let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
    let env = iss_envelope_with_payload_pii();
    let member_cells = vec![
        CellId::from_token("cell-a"),
        CellId::from_token("cell-b"),
        CellId::from_token("cell-c"),
    ];

    let fanned = prop.fan_out(&env, &member_cells);
    let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
    assert_eq!(
        dests,
        vec!["cell-b", "cell-c"],
        "the cross-cell ISS event fans out to the tenant's OTHER cells (the home cell skipped)"
    );

    for pp in &fanned {
        assert_eq!(pp.stream, CrossCellStream::IssuePortfolio);
        assert_eq!(
            pp.pointer.subject().artifact_ref().0,
            "myelin://01J0ACME/issues/issue/42"
        );
        assert_eq!(
            pp.pointer.correlation_id(),
            &CorrelationId("01J0CHAIN".into())
        );
        assert_eq!(pp.pointer.home_cell().as_str(), "cell-a");
        let wire = serde_json::to_string(&pp.pointer).expect("pointer serialises");
        assert!(
            !wire.contains("alice@example.com"),
            "the payload email NEVER crosses the boundary: {wire}"
        );
        assert!(
            !wire.contains("the secret plan"),
            "the payload body NEVER crosses the boundary: {wire}"
        );
    }

    let pii_crossed = prop.pii_fields_crossed();
    assert_eq!(
        pii_crossed, 0,
        "0 PII crosses the cell boundary (the EB-25 zero)"
    );
    assert_eq!(
        prop.pointers_propagated(),
        2,
        "two cross-cell pointer-events propagated (one per other member cell)"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, pii_crossed as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-438 EB-25 GA-D8/CP-D8 GREEN 2026-06-24] Bus cross-cell EVENT-PROPAGATION half LIVE: a \
         cross-cell ISS event in the home cell (cell-a) with a PII-bearing payload fanned a PII-free \
         CrossCellPointer out to the tenant's OTHER member cells (cell-b, cell-c) - each carried ONLY \
         subject/type/correlation_id/home_cell (the four frozen fields); the payload PII \
         (assignee_email/body) was STRUCTURALLY absent. pointers_propagated={}, PII fields across the \
         boundary={} (the EB-25 zero). The resolution half (per-viewer cell-local resolve, \
         unauthorised → tombstone) + per-cell receipts + 0 migration loss green in \
         myelin-control-plane (P-429 CP-D8 / P-430 GA-D8/CP-D7). FLOOR: the cell→cell transport wire is \
         the control-plane bridge + resilient client; the [OPEN - LEGAL] bridge-residency proof ships \
         regardless of ratification (PII-free by construction).",
        prop.pointers_propagated(),
        pii_crossed,
    );
}

#[test]
fn eb25_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a PII field crossing the cell boundary MUST read RED - the EB-25 zero is a real tripwire"
    );
}

#[test]
fn eb25_propagation_is_pure_so_survives_migration_zero_loss() {
    let env = iss_envelope_with_payload_pii();
    let members = vec![CellId::from_token("cell-a"), CellId::from_token("cell-b")];

    let before = CrossCellPropagator::new(CellId::from_token("cell-a"));
    let fan_before = before.fan_out(&env, &members);

    let after = CrossCellPropagator::new(CellId::from_token("cell-a"));
    let fan_after = after.fan_out(&env, &members);

    assert_eq!(
        fan_before, fan_after,
        "propagation is a pure function of (envelope, member_cells) - a re-drive after migration is \
         byte-identical (CP-D7 0 loss)"
    );
}
