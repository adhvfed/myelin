use myelin_knowledge::{
    run_collab_surge, run_lexorank_storm, CollabSurgeGate, COLLAB_SURGE_MULTIPLIER,
};
use myelin_query::field::OrderKey;
use myelin_substrate::shed::{RunClass, Surface as ShedSurface};
use myelin_substrate::Thresholds;
use myelin_tenancy::TenantId;

fn tenant(s: &str) -> TenantId {
    TenantId(s.to_string())
}

fn surge_multiplier(thresholds: &Thresholds) -> u32 {
    let m = thresholds.surge.multiplier;
    assert_eq!(
        m, COLLAB_SURGE_MULTIPLIER,
        "the surge multiplier in the file must equal the documented default-to-beat"
    );
    m
}

#[test]
fn kn_d8_allhands_surge_holds_within_budget() {
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let multiplier = surge_multiplier(&thresholds);

    let lane_budget = thresholds
        .shed_budget(ShedSurface::CollabOpStream)
        .expect("present");
    let cap = lane_budget.per_tenant_in_flight_cap;
    let per_doc_cap = cap / 4;
    let mut gate = CollabSurgeGate::with_budget_and_bounds(lane_budget, per_doc_cap, per_doc_cap);
    assert_eq!(gate.surface(), ShedSurface::CollabOpStream);

    let surging = tenant("all-hands-co");
    let quiet = tenant("quiet-co");

    let storm_ops = u64::from(cap) * u64::from(multiplier);

    let report = run_collab_surge(
        &mut gate,
        &surging,
        &quiet,
        "all-hands-doc",
        storm_ops,
        storm_ops,
        multiplier,
    );

    println!("[KN-D8 GREEN ARTIFACT] {}", report.summary());

    assert!(
        report.surging_human_admitted,
        "the active human editor holds the protected lane (shed last)"
    );
    assert_eq!(
        report.surging_human_shed_count, 0,
        "the human active-editor lane was NEVER shed"
    );
    assert!(
        report.surging_agent_shed_count > 0,
        "the agent edit lane shed (429 + Retry-After) - absorbed, not unbounded"
    );
    assert!(
        report.surging_viewer_shed_count > 0,
        "the passive-viewer lane shed FIRST (viewers shed before editors)"
    );

    assert!(
        report.hot_doc_op_cap_shed_count > 0,
        "the per-doc op cap bounded the hot doc's op fan-out (thundering-herd discipline)"
    );
    assert!(
        report.hot_doc_read_fanout_shed_count > 0,
        "the read-fanout bound bounded one edit's broadcast under the viewer storm"
    );

    assert_eq!(
        report.cross_tenant_impact, 0,
        "the storm never spent the quiet tenant's budget (per-tenant blast-radius)"
    );
    assert!(
        report.quiet_human_admitted,
        "the quiet co-tenant's human editor was admitted within its independent budget"
    );

    assert!(report.is_green(), "KN-D8 + F6 surge: {}", report.summary());
}

#[test]
fn kn_d8_lexorank_concurrent_same_gap_storm_has_zero_reorder() {
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let multiplier = surge_multiplier(&thresholds);

    let lo = OrderKey::parse("U00").expect("lo");
    let hi = OrderKey::parse("V00").expect("hi");
    let inserts = 100 * multiplier as usize;

    let report = run_lexorank_storm(Some(&lo), Some(&hi), inserts);
    println!("[KN-D8 GREEN ARTIFACT] {}", report.summary());

    assert_eq!(
        report.distinct_keys, inserts,
        "every concurrent same-gap insert produced a DISTINCT order key - 0 key-collision reorder"
    );
    assert!(
        report.all_within_gap,
        "every key sorts strictly within (lo, hi) - no reorder relative to the rest of the list"
    );
    assert_eq!(
        report.rebalance_triggers, 0,
        "the single-gap storm forced 0 rebalance - bounded rebalance cost (§3.5)"
    );
    assert!(
        report.is_green(),
        "KN-D8 LexoRank storm: {}",
        report.summary()
    );
}

#[test]
fn kn_d8_f6_leg_is_not_vacuous() {
    use myelin_substrate::shed::SurfaceBudget;
    let huge = SurfaceBudget {
        per_tenant_in_flight_cap: 10_000_000,
        human_lane_reservation: 2_000_000,
        retry_after_secs: 2,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(huge, 10_000_000, 10_000_000);
    let report = run_collab_surge(
        &mut gate,
        &tenant("noisy"),
        &tenant("quiet"),
        "all-hands-doc",
        1000,
        1000,
        COLLAB_SURGE_MULTIPLIER,
    );
    assert_eq!(
        report.surging_agent_shed_count, 0,
        "an unbounded surface swallows the storm - proves the gate (not an absent storm) is what sheds"
    );
    assert!(!report.is_green(), "an unbounded surface MUST read RED");
    let _ = RunClass::Human;
}
