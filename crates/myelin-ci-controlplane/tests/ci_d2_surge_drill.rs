use myelin_ci_controlplane::{
    drive_ci_d2_surge, CiSurgeControls, CiSurgeGate, CI_SURGE_MULTIPLIER,
};
use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, StormProfile,
};
use myelin_substrate::shed::{RunClass, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

fn surge_multiplier_from_thresholds() -> u32 {
    let t = Thresholds::load_canonical().expect("the versioned thresholds file must load");
    let m = t.surge.multiplier;
    assert!(m > 0, "the surge multiplier must be a positive factor");
    m
}

fn derived_ci_storm_ops(surging: &TenantId, base_requests: u64, multiplier: u32) -> u64 {
    let m = Multiplier::custom(multiplier).expect("a positive surge multiplier");
    let gen = LoadGenerator::new(
        base_requests,
        m,
        PrincipalMix::agent_skewed(),
        StormProfile::ci_surge(),
        vec![surging.clone()],
    )
    .expect("a non-empty tenant list");
    let mut sink = RecordingSink::default();
    gen.drive(&mut sink);
    let ci_ops = sink
        .received
        .iter()
        .filter(|r| r.load_kind == LoadPrincipalKind::Ci)
        .count() as u64;
    assert!(
        ci_ops > 0,
        "the agent/CI-skewed surge mix must issue CI dispatch ops (the storm the batch lane sheds)"
    );
    ci_ops
}

#[test]
fn ci_d2_surge_interactive_holds_batch_sheds_cross_tenant_zero_reaper_zero_orphans() {
    let multiplier = surge_multiplier_from_thresholds();
    assert_eq!(
        multiplier, CI_SURGE_MULTIPLIER,
        "the thresholds-file surge multiplier must match the documented CI default-to-beat \
         (a divergence is a LOUD failure, never a silent weakening - EI-01 §3)"
    );

    let surging = TenantId("noisy-ci-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());

    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let controls =
        CiSurgeControls::from_thresholds(&thresholds).expect("the CI-surge controls from the file");
    assert_eq!(
        controls.multiplier(),
        multiplier,
        "the controls read the same 30× multiplier"
    );

    let storm_ops = derived_ci_storm_ops(&surging, 16, multiplier) as u32;
    assert!(
        storm_ops > controls.per_tenant_in_flight_cap(),
        "the derived storm must exceed the per-tenant cap so the lane genuinely sheds"
    );

    let report = drive_ci_d2_surge(&controls, storm_ops, &surging, &quiet, "fr-par");

    assert!(
        report.is_ci_d2_green(),
        "CI-D2 must be GREEN: {}",
        report.summary()
    );
    assert!(
        report.surging_batch_shed_count > 0,
        "the CI storm MUST be absorbed by SHEDDING (429+Retry-After), not unbounded latency"
    );
    assert!(
        report.batch_shed_retry_after_secs > 0,
        "every batch/CI-lane shed carries a Retry-After (myelin ci honours it - no retry storm)"
    );
    assert_eq!(
        report.surging_interactive_shed_count, 0,
        "the protected INTERACTIVE lane HELD on the surging tenant (a PR-check never queues behind a matrix)"
    );
    assert!(
        report.surging_interactive_admitted,
        "the surging tenant's OWN interactive PR-check was admitted (held last)"
    );
    assert!(
        report.quiet_interactive_admitted,
        "the quiet co-tenant's interactive dispatch was admitted within budget (untouched)"
    );
    assert_eq!(
        report.cross_tenant_shed_count, 0,
        "cross-tenant impact is 0 - the storm is contained to the surging tenant's bounded run-queue"
    );
    assert_eq!(
        report.orphan_count, 0,
        "0 ORPHANS - every killed-runner lease re-queued within the lease TTL (the headline zero)"
    );
    assert!(
        report.requeued_count > 0,
        "the killed runner's jobs re-queued (claimable again) within the lease TTL (the reaper recovered them)"
    );

    assert!(
        report.fair_key_wait_p99_ticks <= report.starvation_trigger_ticks,
        "the per-fair_key wait p99 ({}t) stayed within the starvation trigger ({}t) - flat DRR holds",
        report.fair_key_wait_p99_ticks,
        report.starvation_trigger_ticks
    );
    assert!(
        !report.hierarchical_scheduler_owed,
        "the measured starvation signal did NOT fire → the hierarchical scheduler stays a named floor (CI-P29)"
    );
    assert!(
        !thresholds.ci_surge.hierarchical_scheduler_promotion_owed,
        "the FROZEN file records the hierarchical scheduler as a named floor (promotion not owed)"
    );

    println!(
        "[P-490 CI-D2 GREEN 2026-06-25] {} (storm_ops={storm_ops} derived from the P-S02 generator at \
         {multiplier}× CI-surge; tuned cap={} == CiDispatch shed budget)",
        report.summary(),
        controls.per_tenant_in_flight_cap()
    );
}

#[test]
fn ci_d2_cross_tenant_isolation_quiet_tenant_admitted_during_the_storm() {
    let mut gate = CiSurgeGate::with_budget(SurfaceBudget {
        per_tenant_in_flight_cap: 8,
        human_lane_reservation: 0,
        retry_after_secs: 5,
    });
    let surging = TenantId("noisy".into());
    let quiet = TenantId("quiet".into());

    for _ in 0..64 {
        let _ = gate.admit(&surging, RunClass::BatchCi);
    }
    assert!(
        gate.shed_count(RunClass::BatchCi) > 0,
        "the surging tenant's batch lane is saturated and sheds"
    );

    assert!(
        gate.admit(&quiet, RunClass::Human).is_ok(),
        "the quiet tenant's interactive dispatch is admitted DURING the surge (per-tenant blast radius)"
    );
    assert!(
        gate.admit(&quiet, RunClass::BatchCi).is_ok(),
        "the quiet tenant's batch dispatch is admitted DURING the surge (the storm is contained)"
    );
    assert_eq!(
        gate.in_flight(&quiet),
        2,
        "the quiet tenant's in-flight is exactly its OWN two ops - 0 cross-tenant bleed"
    );

    println!(
        "[P-490 CI-D2 cross-tenant 2026-06-25] surging batch_shed={} quiet admitted (in_flight={}) → \
         the per-tenant bounded run-queue contains the storm",
        gate.shed_count(RunClass::BatchCi),
        gate.in_flight(&quiet)
    );
}

#[test]
fn ci_d2_unbounded_lane_is_not_green() {
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let controls = CiSurgeControls::from_thresholds(&thresholds).expect("controls");
    let report = drive_ci_d2_surge(
        &controls,
        1,
        &TenantId("s".into()),
        &TenantId("q".into()),
        "fr-par",
    );
    assert_eq!(
        report.surging_batch_shed_count, 0,
        "a sub-cap storm never sheds"
    );
    assert!(
        !report.is_ci_d2_green(),
        "with 0 shed the surge property is not exercised → NOT green (the green must be earned)"
    );
}
