//! # KN-D8 — the all-hands-doc surge controls + the concurrent-same-gap LexoRank storm
//! (KN-P32 / P-487, M5 — the F6 surge family leg for Knowledge)
//!
//! **Drill catalogue (testing-strategy/01-…-catalogue.md, row KN-D8):** an all-hands doc with
//! thousands of concurrent readers/editors → the per-doc op cap + read-fanout bound + active-editor
//! lane reservation hold within budget; other tenants unaffected; the concurrent-same-gap LexoRank
//! insert storm → 0 reorder. **The F6 surge family leg:** the human (active-editor) lane holds, the
//! agent lane sheds `429 + Retry-After`, cross-tenant impact 0.
//!
//! This drill is the SCHED-gate green artifact. It drives a REAL 30× storm (the multiplier read from
//! the FROZEN thresholds file, never hardcoded — EI-01 §3) of thousands of concurrent agent edits +
//! passive viewer reads on ONE hot doc on the surging tenant, and asserts:
//!
//! 1. **the active-editor lane reservation holds** — the surging tenant's human active edit is admitted
//!    (shed last) while the agent edit lane + the passive-viewer lane shed (`429 + Retry-After`);
//! 2. **the per-doc op cap + the read-fanout bound hold the hot doc within budget** — both shed under
//!    the storm (the thundering-herd discipline; one edit never becomes an unbounded broadcast);
//! 3. **other tenants are unaffected** — a quiet co-tenant's human edit is admitted within its
//!    independent per-tenant budget (cross-tenant impact 0);
//! 4. **the concurrent-same-gap LexoRank insert storm → 0 reorder + bounded rebalance** — thousands of
//!    concurrent inserts into the SAME sibling gap each produce a distinct order key, all within the
//!    gap, none tripping the 48-char rebalance trigger.
//!
//! The budget is read from `myelin_substrate::Thresholds` (the single source of truth); no gate is
//! weakened — the surge runs a real 30× storm.

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

/// **The 30× multiplier is read from the FROZEN thresholds file** (never hardcoded). The storm-op
/// counts below are derived FROM it — a divergence is a loud failure (EI-01 §3).
fn surge_multiplier(thresholds: &Thresholds) -> u32 {
    let m = thresholds.surge.multiplier;
    assert_eq!(
        m, COLLAB_SURGE_MULTIPLIER,
        "the surge multiplier in the file must equal the documented default-to-beat"
    );
    m
}

/// **KN-D8 + the F6 leg: the all-hands-doc surge holds within budget, 0 reorder, other tenants
/// unaffected.** The dated green artifact for the SCHED gate.
#[test]
fn kn_d8_allhands_surge_holds_within_budget() {
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let multiplier = surge_multiplier(&thresholds);

    // The gate opens against the CollabOpStream budget FROM the file (the tuned OQ-K numbers) for the
    // per-tenant op-stream lane; the per-doc op cap + read-fanout bound are a tuned fraction of the
    // surface cap (one hot doc never consumes the whole surface — the thundering-herd discipline), set
    // explicitly so they are the binding bound for the ONE hot doc while the lane saturates on the
    // spread. The lane reservation + the surge multiplier remain FROM the file (never hardcoded).
    let lane_budget = thresholds
        .shed_budget(ShedSurface::CollabOpStream)
        .expect("present");
    let cap = lane_budget.per_tenant_in_flight_cap; // 128 from the file
                                                    // per-doc cap + fanout = 1/4 of the surface cap (well under the non-reserved budget so per-doc/fanout
                                                    // are the binding bound for ONE doc); a tuned hot-doc fraction, named-floor-style.
    let per_doc_cap = cap / 4;
    let mut gate = CollabSurgeGate::with_budget_and_bounds(lane_budget, per_doc_cap, per_doc_cap);
    assert_eq!(gate.surface(), ShedSurface::CollabOpStream);

    let surging = tenant("all-hands-co");
    let quiet = tenant("quiet-co");

    // The 30× all-hands storm: thousands of concurrent ops. The base offered load is sized to the
    // surface cap; 30× over the cap guarantees every bound is genuinely exceeded — a REAL storm, not a
    // token one. Agent edits + passive viewer reads.
    let storm_ops = u64::from(cap) * u64::from(multiplier); // 128 × 30 = 3840 per lane

    let report = run_collab_surge(
        &mut gate,
        &surging,
        &quiet,
        "all-hands-doc",
        storm_ops,
        storm_ops,
        multiplier,
    );

    // Observability is part of the pass — emit the dated green-artifact row.
    println!("[KN-D8 GREEN ARTIFACT] {}", report.summary());

    // (1) the active-editor lane reservation holds: the surging tenant's human editor is served while
    // the agent + viewer machine lanes shed.
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
        "the agent edit lane shed (429 + Retry-After) — absorbed, not unbounded"
    );
    assert!(
        report.surging_viewer_shed_count > 0,
        "the passive-viewer lane shed FIRST (viewers shed before editors)"
    );

    // (2) the per-doc op cap + the read-fanout bound held the hot doc within budget (both shed).
    assert!(
        report.hot_doc_op_cap_shed_count > 0,
        "the per-doc op cap bounded the hot doc's op fan-out (thundering-herd discipline)"
    );
    assert!(
        report.hot_doc_read_fanout_shed_count > 0,
        "the read-fanout bound bounded one edit's broadcast under the viewer storm"
    );

    // (3) other tenants unaffected — cross-tenant impact 0; the quiet co-tenant's human edit held.
    assert_eq!(
        report.cross_tenant_impact, 0,
        "the storm never spent the quiet tenant's budget (per-tenant blast-radius)"
    );
    assert!(
        report.quiet_human_admitted,
        "the quiet co-tenant's human editor was admitted within its independent budget"
    );

    // the whole F6 + KN-D8 op-stream predicate is GREEN.
    assert!(report.is_green(), "KN-D8 + F6 surge: {}", report.summary());
}

/// **KN-D8: the concurrent-same-gap LexoRank insert storm → 0 reorder + bounded rebalance (§3.5).**
/// Thousands of concurrent inserts into the SAME sibling gap. The 30× multiplier sizes the storm.
#[test]
fn kn_d8_lexorank_concurrent_same_gap_storm_has_zero_reorder() {
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let multiplier = surge_multiplier(&thresholds);

    // a single sibling gap (lo, hi); the storm drives thousands of concurrent inserts into it.
    let lo = OrderKey::parse("U00").expect("lo");
    let hi = OrderKey::parse("V00").expect("hi");
    // a genuine all-hands storm: 100 base × 30× = 3000 concurrent inserts into the one gap.
    let inserts = 100 * multiplier as usize;

    let report = run_lexorank_storm(Some(&lo), Some(&hi), inserts);
    println!("[KN-D8 GREEN ARTIFACT] {}", report.summary());

    assert_eq!(
        report.distinct_keys, inserts,
        "every concurrent same-gap insert produced a DISTINCT order key — 0 key-collision reorder"
    );
    assert!(
        report.all_within_gap,
        "every key sorts strictly within (lo, hi) — no reorder relative to the rest of the list"
    );
    assert_eq!(
        report.rebalance_triggers, 0,
        "the single-gap storm forced 0 rebalance — bounded rebalance cost (§3.5)"
    );
    assert!(
        report.is_green(),
        "KN-D8 LexoRank storm: {}",
        report.summary()
    );
}

/// **The F6 leg is NOT vacuous — without the gate (an unbounded surface) the storm would NOT shed.**
/// A guard that the green above is EARNED (the gate is what sheds, not an absent storm).
#[test]
fn kn_d8_f6_leg_is_not_vacuous() {
    use myelin_substrate::shed::SurfaceBudget;
    // an unbounded gate (cap absurdly large, no per-doc/fanout bound) swallows the storm → RED.
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
        "an unbounded surface swallows the storm — proves the gate (not an absent storm) is what sheds"
    );
    assert!(!report.is_green(), "an unbounded surface MUST read RED");
    // sanity: the shed lane the gate fronts is the substrate's CollabOpStream (reused, not forked).
    let _ = RunClass::Human;
}
