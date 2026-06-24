//! # CDC 1.11 — the protected-human-lane shed order + bounded-everything (P-S19 → P-035)
//!
//! **Contract-index:** row 1.11 (`Protected-human-lane shed order` — speculative → batch/CI →
//! agent → human-last; `429 + Retry-After`; per-surface shed budgets as named v1 floors). This
//! consumer-driven contract test exercises the 1.11 shape from OUTSIDE the crate (a gateway/public
//! surface is the consumer of the shed lane): it drives the lane to saturation and asserts the shed
//! order, the protected per-tenant human lane, and the bounded-everything fast-fail — the two halves
//! the gateway and the substrate agree on.
//!
//! The provider side is [`myelin_substrate::shed`] (the `ShedLane` + `BoundedQueue` + the §7.6 v1
//! floor table); this is the consumer (the public surface deriving a run-class from the injected
//! `Principal` and admitting/shedding). It is the dated green artifact's CDC half (the unit half is
//! `shed::tests`; the SUB-D3 surge drill is M5/P-S32 — named, not run here).

use myelin_identity::{PrincipalKind, RuntimeRef};
use myelin_substrate::{
    BoundedQueue, RunClass, RunClassHeader, ShedBudgetTable, ShedDecision, ShedLane, ShedSurface,
    SurfaceBudget,
};
use myelin_tenancy::TenantId;

fn tenant(s: &str) -> TenantId {
    TenantId(s.to_string())
}

/// **CDC 1.11 (a) — the gateway derives the run-class from `Principal.kind` + the injected header,
/// then the shed order fires speculative → batch/CI → agent → human-last.** The header may only
/// DOWN-class; the human lane is structurally unspoofable (no human header exists).
#[test]
fn cdc_1_11_run_class_derives_from_principal_and_header_never_up_classes() {
    // a Service principal is batch/ci by default; a header can down-class it to speculative.
    assert_eq!(
        RunClass::derive(&PrincipalKind::Service, None),
        RunClass::BatchCi
    );
    assert_eq!(
        RunClass::derive(&PrincipalKind::Service, Some(RunClassHeader::Speculative)),
        RunClass::Speculative,
        "a header down-classes (sheds earlier)"
    );
    // an Agent → the agent lane; a verified Human → the protected human lane. A machine principal
    // can NEVER name itself human (there is no human header), so the human lane is unspoofable.
    let agent = PrincipalKind::Agent {
        runtime_ref: RuntimeRef("rt".into()),
        on_behalf_of: None,
    };
    assert_eq!(RunClass::derive(&agent, None), RunClass::Agent);
    assert_eq!(
        RunClass::derive(&PrincipalKind::Human, None),
        RunClass::Human
    );

    // the variant order IS the shed priority (a lower class sheds first).
    assert!(RunClass::Speculative < RunClass::BatchCi);
    assert!(RunClass::BatchCi < RunClass::Agent);
    assert!(RunClass::Agent < RunClass::Human);
}

/// **CDC 1.11 (a) — the shed order sheds in priority and the human is admitted (shed last) while a
/// machine lane is being shed with `429 + Retry-After`.**
#[test]
fn cdc_1_11_shed_order_protects_the_human_lane_with_retry_after() {
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 10,
        human_lane_reservation: 4,
        retry_after_secs: 5,
    };
    let mut lane = ShedLane::with_budget(ShedSurface::HttpIntake, budget);
    let t = tenant("acme");
    // non_human_budget = 6, step = 1 → speculative ceiling 4, batch 5, agent 6.
    for _ in 0..4 {
        assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit);
    }
    // speculative sheds FIRST, with a 429 + Retry-After (the typed form the gateway maps to HTTP).
    match lane.admit(&t, RunClass::Speculative) {
        ShedDecision::Shed { retry_after_secs } => assert_eq!(retry_after_secs, 5),
        ShedDecision::Admit => panic!("speculative must shed first"),
    }
    // batch/ci then agent shed as fill rises; the HUMAN is admitted throughout (shed last).
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
    // the human lane: NOT shed — it uses the reserved slots.
    assert_eq!(lane.admit(&t, RunClass::Human), ShedDecision::Admit);
    assert_eq!(
        lane.shed_count(RunClass::Human),
        0,
        "the human lane has not been shed"
    );
    // the per-lane shed-count signals are exported (contract-1.8).
    assert!(lane.shed_count(RunClass::Speculative) >= 1);
    assert!(lane.total_shed_count() >= 3);
}

/// **CDC 1.11 (a) — per-tenant: one tenant's surge never sheds another tenant's human (blast
/// radius).** The consumer-visible guarantee the product depends on.
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
    // saturate the noisy tenant completely.
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
    // the quiet tenant is fully unaffected — its human is admitted.
    assert_eq!(
        lane.admit(&quiet, RunClass::Human),
        ShedDecision::Admit,
        "a surge on one tenant must NEVER shed another tenant's human"
    );
}

/// **CDC 1.11 (b) — bounded everything: every queue/pool fast-fails rather than growing latency
/// unboundedly (Little's Law, §7.1).** The consumer (any of: consumer prefetch / DB pool / bulkhead
/// / per-tenant in-flight / HTTP intake) gets a fast-fail, never an unbounded buffer.
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

/// **CDC 1.11 (c) — the §7.6 per-surface shed-budget v1 floor table: every surface bounded, with a
/// reserved human lane (except CI, the batch lane) and a Retry-After.**
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
        seen, 9,
        "the v1 floor names the four §7.6 surfaces + the Git front door (GIT-P15) + the two Refs surfaces (REF-P22) + the Search query surface (SRCH-P25) + the generic HTTP intake"
    );
    // CI is the batch lane — no human reservation; the human-facing surfaces reserve a lane.
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
