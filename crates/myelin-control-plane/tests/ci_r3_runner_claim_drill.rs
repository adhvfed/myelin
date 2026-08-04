use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind, JobLeaseStore, JobSpec, MeterTarget, QueuedJob,
    ResourceLimits, RunTokenCredential, TrustTier, WorkspaceSpec,
};
use myelin_control_plane::ResidencyStoreClass;
use myelin_control_plane::{CiStoreWritePinError, OutOfRegionRunnerClaim, RunnerClaimPin};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{Region, TenantId};

fn ci_spec(idem: &str) -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef000000000000000000000000000000000000000000000000").unwrap(),
        vec!["cargo".into(), "test".into()],
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs: 600,
        },
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        RunTokenCredential::new("test-run-bearer", "jti-1", 60).unwrap(),
        MeterTarget {
            reserve_id: "res-1".into(),
        },
        IdemToken(idem.into()),
    )
    .unwrap()
}

#[test]
fn ci_r3_residency_pinned_runners() {
    let tenant = TenantId::from_token("01J0EUTENANT");
    let region = Region::new("fr-par");
    let pin = RunnerClaimPin::for_tenant(tenant.clone(), region.clone());

    pin.admit_claim(&Region::new("fr-par"))
        .expect("an in-region runner claims the EU tenant's CI run (gate GREEN)");
    let refused: OutOfRegionRunnerClaim = pin
        .admit_claim(&Region::new("eu-north"))
        .expect_err("an out-of-region runner cannot claim the EU tenant's CI run (gate RED)");
    assert_eq!(refused.tenant_region.as_str(), "fr-par");
    assert_eq!(refused.runner_region.as_str(), "eu-north");
    assert!(
        refused.to_string().contains("ONLY by an in-region runner"),
        "loud: {refused}"
    );
    assert_eq!(
        pin.out_of_region_claims_admitted(),
        0,
        "0 out-of-region claims admitted"
    );

    let q = JobLeaseStore::new();
    q.enqueue(QueuedJob::new(
        tenant.clone(),
        region.clone(),
        "run-1",
        "job-1",
        vec!["linux".into()],
        ci_spec("idem-1"),
    ));
    let tiers = [TrustTier::Trusted];

    let out_of_region_claim = q.claim_for_labels(
        "runner-eu-north",
        &["linux".into()],
        &tiers,
        &Region::new("eu-north"),
        1000,
        30,
    );
    assert!(
        out_of_region_claim.is_none(),
        "the CI claim mechanism SKIPS the EU tenant's fr-par job for an eu-north runner (no global pool)"
    );
    assert!(
        pin.admit_claim(&Region::new("eu-north")).is_err(),
        "the control-plane pin REJECTS the eu-north runner - the two halves agree (0 out-of-region claims)"
    );

    let in_region_claim = q
        .claim_for_labels(
            "runner-fr-par",
            &["linux".into()],
            &tiers,
            &Region::new("fr-par"),
            1000,
            30,
        )
        .expect("an in-region (fr-par) runner claims the EU tenant's job");
    assert_eq!(in_region_claim.job_id, "job-1");
    assert_eq!(in_region_claim.region.as_str(), "fr-par");
    pin.admit_claim(&in_region_claim.region)
        .expect("the control-plane pin admits the in-region runner - the two halves agree");

    for surface in ResidencyStoreClass::CI_SET {
        pin.pin_ci_store_write(surface, &Region::new("fr-par"))
            .unwrap_or_else(|e| {
                panic!("in-region CI write to `{}` admitted: {e}", surface.label())
            });
        let leak: CiStoreWritePinError = pin
            .pin_ci_store_write(surface, &Region::new("eu-north"))
            .expect_err("an out-of-region CI write is REJECTED (it never leaves the region)");
        assert!(
            matches!(leak, CiStoreWritePinError::OutOfRegion { .. }),
            "the out-of-region CI write is the named breach: {leak}"
        );
    }
    assert_eq!(
        pin.out_of_region_ci_writes_admitted(),
        0,
        "0 out-of-region CI-store writes admitted (logs/artifacts/caches stay in region)"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        pin.out_of_region_claims_admitted() as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-325 CI-R3 GREEN 2026-06-22] residency-pinned runners: EU tenant 01J0EUTENANT (fr-par) - an \
         in-region (fr-par) runner claimed the CI run; an out-of-region (eu-north) runner was REJECTED \
         from BOTH the CI claim mechanism (the claim predicate skipped the job) AND the control-plane \
         region-pin assertion (out_of_region_claims_admitted={}). Every CI-store write (log tier / \
         artifact store / cache namespaces) passed the residency-pin leg in-region and was REJECTED out \
         of region (out_of_region_ci_writes_admitted={}) - logs/artifacts/caches never leave the region. \
         The CI subsystem owns the runner-claim MECHANISM; Tenancy owns the region-pin ASSERTION; the \
         two halves AGREE. This completes the CI residency posture begun in P-CP-17 (P-324, the \
         attestation leg) - NO floor here.",
        pin.out_of_region_claims_admitted(),
        pin.out_of_region_ci_writes_admitted(),
    );
}

#[test]
fn ci_r3_runner_claim_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "an out-of-region runner claim MUST read RED - the 0-out-of-region-claims zero is a real tripwire"
    );
}
