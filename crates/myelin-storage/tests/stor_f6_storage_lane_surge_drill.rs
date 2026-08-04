use std::path::Path;

use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, RunClass,
    StormProfile,
};
use myelin_storage::{
    run_storage_lane_surge, StorageAdmission, StorageLaneClass, StorageLaneGate,
    STORAGE_SURGE_MULTIPLIER,
};
use myelin_tenancy::TenantId;

fn surge_multiplier_from_thresholds() -> u32 {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest");
    let path = root.join("thresholds.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the versioned thresholds file must load at {path:?}: {e}"));
    let doc: toml::Value = text.parse().expect("thresholds.toml must be valid TOML");
    let m = doc
        .get("surge")
        .and_then(|t| t.get("multiplier"))
        .and_then(|v| v.as_integer())
        .expect("surge.multiplier must be present (a missing threshold is a LOUD error)");
    assert!(m > 0, "the surge multiplier must be a positive factor");
    m as u32
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
        "the agent-skewed surge mix must issue CI storage ops (the artifact storm the storage lane sheds)"
    );
    ci_ops
}

#[test]
fn stor_f6_storage_lane_surge_human_holds_ci_sheds_cross_tenant_zero() {
    let multiplier = surge_multiplier_from_thresholds();
    assert_eq!(
        multiplier, STORAGE_SURGE_MULTIPLIER,
        "the thresholds-file surge multiplier must match the documented storage default-to-beat \
         (a divergence is a LOUD failure, never a silent weakening - EI-01 §3)"
    );

    let surging = TenantId("noisy-ci-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());

    let storm_ops = derived_ci_storm_ops(&surging, 32, multiplier);

    let mut gate = StorageLaneGate::new();
    let report = run_storage_lane_surge(&mut gate, &surging, &quiet, storm_ops, multiplier);

    assert!(
        report.is_f6_green(),
        "the F6 storage-lane surge must be GREEN: {report:?}"
    );
    assert!(
        report.surging_tenant_ci_shed_count > 0,
        "the CI artifact storm MUST be absorbed by SHEDDING (429+Retry-After), not unbounded latency"
    );
    assert_eq!(
        report.surging_tenant_human_shed_count, 0,
        "the protected human lane HELD on the surging tenant (shed-last)"
    );
    assert!(
        report.quiet_tenant_human_admitted,
        "the quiet co-tenant's human storage op was admitted within budget (untouched by the storm)"
    );
    assert_eq!(
        report.cross_tenant_impact, 0,
        "cross-tenant impact is 0 - the storm is contained to the surging tenant"
    );

    println!(
        "[P-444 F6 STORAGE GREEN 2026-06-24] {} (storm_ops={storm_ops} derived from the P-S02 \
         generator at {multiplier}× CI-surge)",
        report.summary()
    );
}

#[test]
fn stor_f6_quiet_tenant_human_admitted_even_when_surging_tenant_fully_saturated() {
    let mut gate = StorageLaneGate::new();
    let surging = TenantId("noisy".into());
    let quiet = TenantId("quiet".into());
    let cap = gate.cap();

    for _ in 0..(cap * 4) {
        let _ = gate.admit(&surging, StorageLaneClass::BatchCi);
    }
    assert!(
        gate.shed_count(StorageLaneClass::BatchCi) > 0,
        "the saturated surging tenant's batch-CI lane sheds"
    );

    assert_eq!(
        gate.in_flight(&quiet),
        0,
        "the quiet tenant's storage budget is independent of the surging tenant's storm"
    );
    assert_eq!(
        gate.admit(&quiet, StorageLaneClass::Human),
        StorageAdmission::Admit,
        "the surging tenant's storage storm must NEVER shed another tenant's human (cross-tenant 0)"
    );
}

#[test]
fn stor_f6_an_unbounded_lane_reads_red() {
    let huge = myelin_storage::StorageLaneBudget {
        per_tenant_in_flight_cap: 1_000_000,
        human_lane_reservation: 200_000,
        retry_after_secs: 5,
    };
    let mut gate = StorageLaneGate::with_budget(huge);
    let surging = TenantId("noisy".into());
    let quiet = TenantId("quiet".into());
    let report = run_storage_lane_surge(&mut gate, &surging, &quiet, 100, STORAGE_SURGE_MULTIPLIER);
    assert_eq!(
        report.surging_tenant_ci_shed_count, 0,
        "the unbounded lane swallowed the storm (no shed) - the failure mode the F6 gate catches"
    );
    assert!(
        !report.is_f6_green(),
        "an unbounded storage lane (storm not absorbed by shedding) MUST read RED - never a silent pass"
    );
}

#[test]
fn stor_f6_storage_lane_order_agrees_with_substrate_run_class_order() {
    fn to_run_class(c: StorageLaneClass) -> RunClass {
        match c {
            StorageLaneClass::Speculative => RunClass::Service,
            StorageLaneClass::BatchCi => RunClass::Ci,
            StorageLaneClass::Agent => RunClass::Agent,
            StorageLaneClass::Human => RunClass::Human,
        }
    }
    assert!(StorageLaneClass::Speculative < StorageLaneClass::BatchCi);
    assert!(StorageLaneClass::BatchCi < StorageLaneClass::Agent);
    assert!(StorageLaneClass::Agent < StorageLaneClass::Human);
    assert_eq!(to_run_class(StorageLaneClass::Human), RunClass::Human);
    assert_eq!(to_run_class(StorageLaneClass::Agent), RunClass::Agent);
    assert_eq!(to_run_class(StorageLaneClass::BatchCi), RunClass::Ci);
    assert_eq!(
        to_run_class(StorageLaneClass::Speculative),
        RunClass::Service
    );
}
