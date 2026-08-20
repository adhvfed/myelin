use super::*;
use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef};
use myelin_tenancy::Region;

fn tenant(s: &str) -> TenantId {
    TenantId(s.to_string())
}

fn human(tenant_slug: &str) -> Principal {
    Principal::new(
        tenant(tenant_slug),
        Region("fr-par".into()),
        PrincipalId(format!("h-{tenant_slug}")),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn agent(tenant_slug: &str) -> Principal {
    Principal::new(
        tenant(tenant_slug),
        Region("fr-par".into()),
        PrincipalId(format!("a-{tenant_slug}")),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt".into()),
            on_behalf_of: None,
        },
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn small_lane_budget() -> SurfaceBudget {
    SurfaceBudget {
        per_tenant_in_flight_cap: 6,
        human_lane_reservation: 2,
        retry_after_secs: 3,
    }
}

#[test]
fn the_collab_shed_budget_is_read_from_the_thresholds_file() {
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let gate =
        CollabSurgeGate::from_thresholds(&thresholds).expect("CollabOpStream budget present");
    assert_eq!(gate.surface(), ShedSurface::CollabOpStream);

    let b = thresholds
        .shed_budget(ShedSurface::CollabOpStream)
        .expect("present");
    assert!(
        b.per_tenant_in_flight_cap > 0,
        "CollabOpStream bounded (§7.1)"
    );
    assert!(
        b.human_lane_reservation > 0,
        "CollabOpStream reserves an active-editor (human) lane"
    );
    assert_eq!(thresholds.surge.multiplier, COLLAB_SURGE_MULTIPLIER);
}

#[test]
fn shed_order_serves_the_human_editor_while_the_agent_lane_sheds() {
    let mut gate = CollabSurgeGate::with_budget(small_lane_budget());
    let a = agent("acme");
    let h = human("acme");

    for _ in 0..4 {
        assert!(
            gate.admit_for(&a, "doc1", None).is_ok(),
            "agent edit admitted under budget"
        );
    }
    let shed = gate
        .admit_for(&a, "doc1", None)
        .expect_err("the agent edit storm sheds");
    assert_eq!(shed.lane, RunClass::Agent);
    assert_eq!(shed.reason, CollabShedReason::OpStreamLane);
    assert_eq!(shed.retry_after_secs, 3, "the shed carries a Retry-After");

    assert_eq!(
        gate.admit_for(&h, "doc1", None)
            .expect("the human editor is served while the agent sheds"),
        RunClass::Human
    );
    assert_eq!(gate.shed_count(RunClass::Human), 0, "human lane: 0 shed");
    assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
}

#[test]
fn viewers_shed_before_editors_agents_before_humans() {
    let mut gate = CollabSurgeGate::with_budget(small_lane_budget());
    let t = tenant("acme");
    for _ in 0..2 {
        gate.admit_doc_op(&t, "doc1", RunClass::Agent)
            .expect("agent edit admitted");
    }
    assert!(
        gate.admit_doc_op(&t, "doc1", RunClass::Speculative)
            .is_err(),
        "a passive viewer sheds before an editor"
    );
    gate.admit_doc_op(&t, "doc1", RunClass::BatchCi)
        .expect("batch admitted");
    assert!(
        gate.admit_doc_op(&t, "doc1", RunClass::BatchCi).is_err(),
        "batch/CI sheds next"
    );
    gate.admit_doc_op(&t, "doc1", RunClass::Agent)
        .expect("agent editor admitted");
    assert!(
        gate.admit_doc_op(&t, "doc1", RunClass::Agent).is_err(),
        "the agent editor sheds before the human"
    );
    gate.admit_doc_op(&t, "doc1", RunClass::Human)
        .expect("the active human editor is served - shed last");

    assert_eq!(
        gate.shed_count(RunClass::Speculative),
        1,
        "viewer shed first"
    );
    assert_eq!(gate.shed_count(RunClass::BatchCi), 1);
    assert_eq!(gate.shed_count(RunClass::Agent), 1);
    assert_eq!(gate.shed_count(RunClass::Human), 0, "human editor: 0 shed");
}

#[test]
fn one_tenants_storm_never_sheds_anothers_human_editor() {
    let mut gate = CollabSurgeGate::with_budget(small_lane_budget());
    let noisy = agent("noisy");
    let quiet_human = human("quiet");

    for _ in 0..4 {
        gate.admit_for(&noisy, "doc1", None)
            .expect("noisy agent admitted");
    }
    assert!(
        gate.admit_for(&noisy, "doc1", None).is_err(),
        "noisy agent edit lane sheds"
    );
    assert_eq!(gate.in_flight(&tenant("noisy")), 4, "noisy has 4 in-flight");
    assert_eq!(
        gate.in_flight(&tenant("quiet")),
        0,
        "the quiet tenant's budget is independent"
    );
    assert_eq!(
        gate.admit_for(&quiet_human, "doc2", None)
            .expect("the quiet human editor is served"),
        RunClass::Human,
        "the noisy storm must NEVER shed another tenant's human editor"
    );
}

#[test]
fn a_machine_principal_cannot_spoof_the_human_editor_lane() {
    let a = agent("acme");
    assert_eq!(
        CollabSurgeGate::derive_class(&a, None),
        RunClass::Agent,
        "an agent edit is the agent lane, never the protected human lane"
    );
    let h = human("acme");
    assert_eq!(
        CollabSurgeGate::derive_class(&h, Some(RunClassHeader::Speculative)),
        RunClass::Speculative,
        "a human-issued passive read may down-class itself to a viewer"
    );
    assert_eq!(CollabSurgeGate::derive_class(&h, None), RunClass::Human);
}

#[test]
fn the_per_doc_op_cap_bounds_one_hot_docs_fan_out() {
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 100,
        human_lane_reservation: 25,
        retry_after_secs: 4,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(budget, 3, 100);
    let t = tenant("acme");

    for _ in 0..3 {
        gate.admit_doc_op(&t, "hot", RunClass::Agent)
            .expect("under the per-doc cap");
    }
    assert_eq!(gate.doc_in_flight("hot"), 3, "the hot doc is at its cap");
    let shed = gate
        .admit_doc_op(&t, "hot", RunClass::Agent)
        .expect_err("the hot doc's op fan-out sheds at the per-doc cap");
    assert_eq!(shed.reason, CollabShedReason::PerDocOpCap);
    assert_eq!(shed.retry_after_secs, 4);
    assert_eq!(
        gate.doc_op_shed_count("hot"),
        1,
        "the per-doc shed is counted"
    );
    assert_eq!(
        gate.doc_in_flight("hot"),
        3,
        "in-flight never grows past the cap"
    );

    gate.admit_doc_op(&t, "cool", RunClass::Agent)
        .expect("a different doc has its own cap");
    assert_eq!(gate.doc_in_flight("cool"), 1);
}

#[test]
fn a_per_doc_shed_releases_the_lane_slot_no_double_charge() {
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 100,
        human_lane_reservation: 25,
        retry_after_secs: 4,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(budget, 2, 100);
    let t = tenant("acme");
    gate.admit_doc_op(&t, "hot", RunClass::Agent).expect("1");
    gate.admit_doc_op(&t, "hot", RunClass::Agent).expect("2");
    assert_eq!(gate.in_flight(&t), 2, "2 lane slots taken");
    assert!(gate.admit_doc_op(&t, "hot", RunClass::Agent).is_err());
    assert_eq!(
        gate.in_flight(&t),
        2,
        "a per-doc shed did not leak a lane slot (no double-charge)"
    );
}

#[test]
fn the_read_fanout_bound_caps_one_edits_broadcast() {
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 100,
        human_lane_reservation: 25,
        retry_after_secs: 6,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(budget, 100, 2);
    gate.admit_read_fanout("hot").expect("1");
    gate.admit_read_fanout("hot").expect("2");
    let shed = gate
        .admit_read_fanout("hot")
        .expect_err("the read fan-out sheds at its bound");
    assert_eq!(shed.reason, CollabShedReason::ReadFanout);
    assert_eq!(
        shed.lane,
        RunClass::Speculative,
        "a viewer fan-out is speculative"
    );
    assert_eq!(gate.read_fanout_shed_count("hot"), 1);
    gate.release_read_fanout("hot");
    gate.admit_read_fanout("hot")
        .expect("a released fan-out slot is reusable");
}

#[test]
fn release_op_frees_both_the_lane_and_the_per_doc_slot() {
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 4,
        human_lane_reservation: 1,
        retry_after_secs: 1,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(budget, 2, 100);
    let t = tenant("acme");
    gate.admit_doc_op(&t, "hot", RunClass::Agent).expect("1");
    gate.admit_doc_op(&t, "hot", RunClass::Agent)
        .expect("2 - at per-doc cap");
    assert!(
        gate.admit_doc_op(&t, "hot", RunClass::Agent).is_err(),
        "per-doc cap reached"
    );
    gate.release_op(&t, "hot", RunClass::Agent);
    assert_eq!(gate.doc_in_flight("hot"), 1, "per-doc slot freed");
    gate.admit_doc_op(&t, "hot", RunClass::Agent)
        .expect("a released slot is reusable");
}

#[test]
fn run_collab_surge_is_green() {
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 12,
        human_lane_reservation: 4,
        retry_after_secs: 2,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(budget, 4, 4);
    let surging = tenant("noisy");
    let quiet = tenant("quiet");
    let report = run_collab_surge(
        &mut gate,
        &surging,
        &quiet,
        "all-hands",
        100,
        100,
        COLLAB_SURGE_MULTIPLIER,
    );
    assert!(report.is_green(), "{}", report.summary());
    assert!(report.surging_agent_shed_count > 0, "agent edit lane shed");
    assert!(report.surging_viewer_shed_count > 0, "viewer lane shed");
    assert_eq!(report.surging_human_shed_count, 0, "human editor lane held");
    assert!(
        report.surging_human_admitted,
        "surging tenant's human editor held"
    );
    assert!(
        report.quiet_human_admitted,
        "quiet co-tenant's human editor held"
    );
    assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    assert!(report.hot_doc_op_cap_shed_count > 0, "hot doc op cap held");
    assert!(
        report.hot_doc_read_fanout_shed_count > 0,
        "read fanout held"
    );
}

#[test]
fn an_unbounded_gate_reads_red() {
    let huge = SurfaceBudget {
        per_tenant_in_flight_cap: 1_000_000,
        human_lane_reservation: 200_000,
        retry_after_secs: 2,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(huge, 1_000_000, 1_000_000);
    let report = run_collab_surge(
        &mut gate,
        &tenant("noisy"),
        &tenant("quiet"),
        "all-hands",
        100,
        100,
        COLLAB_SURGE_MULTIPLIER,
    );
    assert_eq!(
        report.surging_agent_shed_count, 0,
        "the unbounded lane swallowed the storm"
    );
    assert!(!report.is_green(), "an unbounded gate MUST read RED");
}

#[test]
fn the_lexorank_storm_has_zero_reorder_and_bounded_rebalance() {
    let lo = OrderKey::parse("U00").expect("lo");
    let hi = OrderKey::parse("V00").expect("hi");
    let report = run_lexorank_storm(Some(&lo), Some(&hi), 2000);
    assert!(report.is_green(), "{}", report.summary());
    assert_eq!(report.inserts, 2000);
    assert_eq!(
        report.distinct_keys, 2000,
        "every concurrent insert produced a DISTINCT key - 0 key-collision reorder"
    );
    assert!(
        report.all_within_gap,
        "every key sorts strictly within the gap - no reorder relative to the rest of the list"
    );
    assert_eq!(
        report.rebalance_triggers, 0,
        "the single-gap storm forced 0 rebalance - bounded rebalance cost (§3.5)"
    );
}

#[test]
fn the_lexorank_storm_is_distinct_even_unbounded_gap() {
    let report = run_lexorank_storm(None, None, 500);
    assert_eq!(
        report.distinct_keys, 500,
        "an unbounded-gap storm still produces distinct keys (the jitter)"
    );
    assert!(
        report.all_within_gap,
        "no bounds → trivially within the (open) gap"
    );
    assert!(report.is_green(), "{}", report.summary());
}

#[test]
fn the_lexorank_report_predicate_is_not_vacuous() {
    let collided = LexoStormReport {
        inserts: 10,
        distinct_keys: 9,
        all_within_gap: true,
        rebalance_triggers: 0,
    };
    assert!(
        !collided.is_green(),
        "a key-collision reorder MUST read RED (the predicate is not vacuous)"
    );
    let runaway = LexoStormReport {
        inserts: 10,
        distinct_keys: 10,
        all_within_gap: true,
        rebalance_triggers: 1,
    };
    assert!(!runaway.is_green(), "an unbounded rebalance MUST read RED");
    let escaped = LexoStormReport {
        inserts: 10,
        distinct_keys: 10,
        all_within_gap: false,
        rebalance_triggers: 0,
    };
    assert!(!escaped.is_green(), "a reorder (escaped gap) MUST read RED");
}
