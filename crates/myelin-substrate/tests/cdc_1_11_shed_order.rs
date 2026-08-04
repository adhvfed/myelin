use myelin_identity::{PrincipalKind, RuntimeRef};
use myelin_substrate::{
    BoundedQueue, RunClass, RunClassHeader, ShedBudgetTable, ShedDecision, ShedLane, ShedSurface,
    SurfaceBudget,
};
use myelin_tenancy::TenantId;

fn tenant(s: &str) -> TenantId {
    TenantId(s.to_string())
}

#[test]
fn cdc_1_11_run_class_derives_from_principal_and_header_never_up_classes() {
    assert_eq!(
        RunClass::derive(&PrincipalKind::Service, None),
        RunClass::BatchCi
    );
    assert_eq!(
        RunClass::derive(&PrincipalKind::Service, Some(RunClassHeader::Speculative)),
        RunClass::Speculative,
        "a header down-classes (sheds earlier)"
    );
    let agent = PrincipalKind::Agent {
        runtime_ref: RuntimeRef("rt".into()),
        on_behalf_of: None,
    };
    assert_eq!(RunClass::derive(&agent, None), RunClass::Agent);
    assert_eq!(
        RunClass::derive(&PrincipalKind::Human, None),
        RunClass::Human
    );

    assert!(RunClass::Speculative < RunClass::BatchCi);
    assert!(RunClass::BatchCi < RunClass::Agent);
    assert!(RunClass::Agent < RunClass::Human);
}

#[test]
fn cdc_1_11_shed_order_protects_the_human_lane_with_retry_after() {
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 10,
        human_lane_reservation: 4,
        retry_after_secs: 5,
    };
    let mut lane = ShedLane::with_budget(ShedSurface::HttpIntake, budget);
    let t = tenant("acme");
    for _ in 0..4 {
        assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit);
    }
    match lane.admit(&t, RunClass::Speculative) {
        ShedDecision::Shed { retry_after_secs } => assert_eq!(retry_after_secs, 5),
        ShedDecision::Admit => panic!("speculative must shed first"),
    }
    assert_eq!(lane.admit(&t, RunClass::BatchCi), ShedDecision::Admit);
    assert!(matches!(
        lane.admit(&t, RunClass::BatchCi),
        ShedDecision::Shed { .. }
    ));
    assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit);
    assert!(matches!(
        lane.admit(&t, RunClass::Agent),
        ShedDecision::Shed { .. }
    ));
    assert_eq!(lane.admit(&t, RunClass::Human), ShedDecision::Admit);
    assert_eq!(
        lane.shed_count(RunClass::Human),
        0,
        "the human lane has not been shed"
    );
    assert!(lane.shed_count(RunClass::Speculative) >= 1);
    assert!(lane.total_shed_count() >= 3);
}

#[test]
fn cdc_1_11_shedding_is_per_tenant() {
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 4,
        human_lane_reservation: 1,
        retry_after_secs: 3,
    };
    let mut lane = ShedLane::with_budget(ShedSurface::AgentMention, budget);
    let noisy = tenant("noisy");
    let quiet = tenant("quiet");
    for _ in 0..3 {
        assert_eq!(lane.admit(&noisy, RunClass::Agent), ShedDecision::Admit);
    }
    assert!(matches!(
        lane.admit(&noisy, RunClass::Agent),
        ShedDecision::Shed { .. }
    ));
    assert_eq!(lane.admit(&noisy, RunClass::Human), ShedDecision::Admit);
    assert!(matches!(
        lane.admit(&noisy, RunClass::Human),
        ShedDecision::Shed { .. }
    ));
    assert_eq!(
        lane.admit(&quiet, RunClass::Human),
        ShedDecision::Admit,
        "a surge on one tenant must NEVER shed another tenant's human"
    );
}

#[test]
fn cdc_1_11_bounded_queue_fast_fails() {
    let mut q = BoundedQueue::new(3);
    assert!(q.try_acquire());
    assert!(q.try_acquire());
    assert!(q.try_acquire());
    assert!(
        !q.try_acquire(),
        "a full bounded queue fast-fails (sheds), never grows"
    );
    assert_eq!(q.in_flight(), 3, "in-flight never exceeds the bound");
    assert_eq!(q.shed_count(), 1, "the shed is observable");
}

#[test]
fn cdc_1_11_v1_floor_table_is_bounded_with_a_reserved_human_lane() {
    let table = ShedBudgetTable::v1_floor();
    let mut seen = 0;
    for surface in table.surfaces() {
        let b = table.budget(surface);
        assert!(b.per_tenant_in_flight_cap > 0, "{surface:?} bounded");
        assert!(b.human_lane_reservation <= b.per_tenant_in_flight_cap);
        assert!(
            b.retry_after_secs > 0,
            "{surface:?} sheds with a Retry-After (clients honour it)"
        );
        seen += 1;
    }
    assert_eq!(
        seen, 10,
        "the v1 floor names the four §7.6 surfaces + the Git front door (GIT-P15) + the two Refs surfaces (REF-P22) + the Search query surface (SRCH-P25) + the durable-workflow start surface (P-FLOW-27) + the generic HTTP intake"
    );
    assert_eq!(
        table.budget(ShedSurface::CiDispatch).human_lane_reservation,
        0
    );
    assert!(
        table
            .budget(ShedSurface::ConnectionTier)
            .human_lane_reservation
            > 0
    );
}
