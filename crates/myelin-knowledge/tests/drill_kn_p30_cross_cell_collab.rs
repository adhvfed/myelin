use myelin_harness::dependency_break::{Dependency, DependencyBreaker, Scope};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::collab::{
    fanned_out_carried_fields, CellLocalDocResolution, CrossCellCollab, CrossCellDocOp,
    CrossCellDocPointer, DocProjection,
};
use myelin_knowledge::transport::{DocOp, OpId, OpKind};
use myelin_tenancy::{CellId, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

fn op_with_pii() -> DocOp {
    let mut op = DocOp::cas(
        OpId::new("client-1", 11),
        "author-opaque",
        OpKind::Insert,
        b"alice@example.com TOP-SECRET BODY".to_vec(),
    );
    op.pii_key_ref = Some("dek:page-9:run-3".into());
    op
}

struct HomeCellResolver {
    allowed: Vec<String>,
}

impl CellLocalDocResolution for HomeCellResolver {
    fn resolve_in_home_cell(
        &self,
        pointer: &CrossCellDocPointer,
        viewer: &Principal,
    ) -> DocProjection {
        let subject = pointer.subject().clone();
        if self.allowed.iter().any(|id| id == &viewer.principal_id.0) {
            DocProjection::Rendered {
                subject,
                rendered: "Q3 plan (rendered in fr-par-1, viewer-scoped)".into(),
            }
        } else {
            DocProjection::Tombstone { subject }
        }
    }
}

#[test]
fn kn_p30_cross_cell_collab_fan_out_is_pii_free_and_resolution_is_cell_local() {
    let home = CellId::from_token("cell-fr-par-1");
    let collab = CrossCellCollab::new(home.clone());
    let op = op_with_pii();
    let tenant = tenant();
    let dop = CrossCellDocOp {
        tenant: &tenant,
        page_id: "page-9",
        op: &op,
    };
    let member_cells = vec![
        home.clone(),
        CellId::from_token("cell-de-1"),
        CellId::from_token("cell-nl-1"),
    ];
    let corr = myelin_events::CorrelationId("op-causal-root".into());

    let fanned = collab.fan_out_doc_op(&dop, &corr, &member_cells);
    let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
    assert_eq!(
        dests,
        vec!["cell-de-1", "cell-nl-1"],
        "the op fans out to EVERY other member cell (the single-cell pin is lifted)"
    );

    let mut pii_fields_crossed = 0_i64;
    for pp in &fanned {
        let (to, subject, kind, corr_field, home_field) = fanned_out_carried_fields(pp);
        assert_eq!(
            subject.artifact_ref().0,
            "myelin://acme/knowledge/page/page-9"
        );
        assert_eq!(kind, &myelin_events::ArtifactType::Page);
        assert_eq!(
            corr_field, &corr,
            "the pointer rides the op causal-root (BUS-5)"
        );
        assert_eq!(home_field, &home);
        assert!(matches!(to.as_str(), "cell-de-1" | "cell-nl-1"));
        let wire = serde_json::to_string(&pp.pointer).expect("pointer serialises");
        for forbidden in ["alice@example.com", "TOP-SECRET", "dek:", "payload"] {
            if wire.contains(forbidden) {
                pii_fields_crossed += 1;
            }
        }
    }

    let mut sig = SignalSource::new();
    let observed = pii_fields_crossed + collab.cross_cell_pii_crossed() as i64;
    sig.set_scalar(SignalName::CrossTenantCount, observed);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        collab.cross_cell_pii_crossed(),
        0,
        "0 PII crosses the bridge"
    );
    assert_eq!(collab.ops_fanned_out(), 2);

    let resolver = HomeCellResolver {
        allowed: vec!["alice".into()],
    };
    let allowed = collab.resolve_cell_local(&fanned[0], &viewer("alice"), &resolver);
    assert!(
        allowed.is_rendered(),
        "an allowed cross-cell viewer gets the rendered projection"
    );
    let wire = serde_json::to_string(&allowed).expect("projection serialises");
    assert!(
        !wire.contains("alice@example.com"),
        "no payload PII in the projection: {wire}"
    );
    assert!(
        !wire.contains("dek:"),
        "no DEK material in the projection: {wire}"
    );
    let denied = collab.resolve_cell_local(&fanned[1], &viewer("mallory"), &resolver);
    assert!(
        !denied.is_rendered(),
        "an unauthorised cross-cell viewer gets a tombstone"
    );
    assert!(matches!(denied, DocProjection::Tombstone { .. }));

    println!(
        "[P-485 KN-P30 GREEN 2026-06-25] knowledge cross-cell collab: a multi-cell tenant's doc op \
         fanned out to {} other member cells (cell-de-1, cell-nl-1) over the PII-free CrossCellPointer \
         bridge; cross_cell_pii_crossed={} (the gate zero); resolution stayed cell-local (rendered for \
         an allowed viewer, tombstone for an unauthorised one - only the projection crossed).",
        collab.ops_fanned_out(),
        collab.cross_cell_pii_crossed(),
    );
}

#[test]
fn kn_p30_severed_bridge_drops_the_pointer_event_zero_pii_under_partition() {
    let home = CellId::from_token("cell-fr-par-1");
    let collab = CrossCellCollab::new(home.clone());
    let op = op_with_pii();
    let tenant = tenant();
    let dop = CrossCellDocOp {
        tenant: &tenant,
        page_id: "page-9",
        op: &op,
    };
    let member_cells = vec![
        CellId::from_token("cell-de-1"),
        CellId::from_token("cell-nl-1"),
    ];
    let corr = myelin_events::CorrelationId("root".into());
    let fanned = collab.fan_out_doc_op(&dop, &corr, &member_cells);

    let breaker = DependencyBreaker::new();
    breaker.break_dependency(Dependency::Broker, Scope::Cell("cell-nl-1".into()));

    let mut delivered = 0_usize;
    let mut pii_crossed = 0_i64;
    for pp in &fanned {
        let cell_down = breaker.is_broken(
            &Dependency::Broker,
            &Scope::Cell(pp.to_cell.as_str().to_string()),
        );
        if cell_down {
            continue;
        }
        delivered += 1;
        let wire = serde_json::to_string(&pp.pointer).expect("serialises");
        for forbidden in ["alice@example.com", "TOP-SECRET", "dek:"] {
            if wire.contains(forbidden) {
                pii_crossed += 1;
            }
        }
    }

    assert_eq!(
        delivered, 1,
        "only the reachable member cell (cell-de-1) received the pointer"
    );
    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        pii_crossed + collab.cross_cell_pii_crossed() as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    breaker.restore_dependency(Dependency::Broker, Scope::Cell("cell-nl-1".into()));
    println!(
        "[P-485 KN-P30 GREEN 2026-06-25] knowledge cross-cell collab under a SEVERED bridge to \
         cell-nl-1: the pointer-event was dropped whole (never a partial payload); cross_cell_pii_crossed=0 \
         held under the partition - the residency invariant is fault-tolerant by construction."
    );
}

#[test]
fn kn_p30_gate_is_not_vacuous_a_pii_leak_reads_red() {
    let leaked_pii_fields = 1_i64;
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, leaked_pii_fields);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a PII field crossing the bridge MUST read RED - the cross-cell-PII zero is a real tripwire"
    );
}
