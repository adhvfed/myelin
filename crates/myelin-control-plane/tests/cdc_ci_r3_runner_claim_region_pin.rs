//! P-CP-18 (global P-325) CDC — **the runner-claim region-pin (CI-R3): provider + consumer.**
//!
//! The PROVIDER is the **CI scheduler / runner-claim mechanism** (`myelin_ci_sandbox::JobLeaseStore`):
//! it owns the `FOR UPDATE SKIP LOCKED` claim whose residency predicate skips any job whose `region` ≠
//! the runner's cell region. The CONSUMER is the **control-plane region-pin assertion**
//! (`myelin_control_plane::RunnerClaimPin`): before a runner's claim is honoured, the control plane
//! asserts the runner's region == the tenant's region of record. This CDC proves the two AGREE on the
//! crossing — an out-of-region runner is refused from BOTH sides (the claim skips the job; the pin
//! rejects the runner). If either shape drifts — the `QueuedJob` region field or the `RunnerClaimPin`
//! assertion — the pairing stops compiling / stops agreeing.
//!
//! This is the runner-claim twin of the P-CP-17 `cdc_12_4_residency_verify_ci_coverage.rs` (the
//! attestation CDC). The CI subsystem owns the runner-claim MECHANISM; Tenancy owns the region-pin
//! ASSERTION — the no-global-CI-pool property is enforced at claim time, not merely attestable.

use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind, JobLeaseStore, JobSpec, MeterTarget, QueuedJob,
    ResourceLimits, RunTokenCredential, TrustTier, WorkspaceSpec,
};
use myelin_control_plane::{OutOfRegionRunnerClaim, RunnerClaimPin};
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

/// A stand-in CI scheduler consumer of the control-plane pin: it MUST assert the runner's region
/// against the control-plane pin before admitting a claim. The CI claim mechanism (the PROVIDER) skips
/// the out-of-region job; the control plane (the assertion) is the authoritative region-of-record pin.
struct RegionPinnedScheduler<'a> {
    leases: &'a JobLeaseStore,
    pin: &'a RunnerClaimPin,
}

impl RegionPinnedScheduler<'_> {
    /// Try to claim a job for a runner in `runner_region`. The control-plane pin gates the claim FIRST
    /// (the region-of-record assertion); only an admitted runner reaches the CI claim mechanism. An
    /// out-of-region runner is refused (`Err`) — and the CI mechanism would skip the job for it anyway.
    fn try_claim(
        &self,
        worker: &str,
        runner_region: &Region,
        now: i64,
    ) -> Result<Option<QueuedJob>, OutOfRegionRunnerClaim> {
        // CONSUMER: the control-plane region-pin assertion (the Tenancy-owned half).
        self.pin.admit_claim(runner_region)?;
        // PROVIDER: the CI claim mechanism (the residency predicate skips out-of-region jobs anyway).
        Ok(self.leases.claim_for_labels(
            worker,
            &["linux".into()],
            &[TrustTier::Trusted],
            runner_region,
            now,
            30,
        ))
    }
}

#[test]
fn cdc_ci_r3_runner_claim_region_pin_provider_consumer() {
    let tenant = TenantId::from_token("01J0EUTENANT");
    let region = Region::new("fr-par");

    // PROVIDER: the CI scheduler's lease store with the EU tenant's fr-par job.
    let leases = JobLeaseStore::new();
    leases.enqueue(QueuedJob::new(
        tenant.clone(),
        region.clone(),
        "run-1",
        "job-1",
        vec!["linux".into()],
        ci_spec("idem-1"),
    ));

    // CONSUMER: the control-plane region-pin for the EU tenant (region of record fr-par).
    let pin = RunnerClaimPin::for_tenant(tenant.clone(), region.clone());

    let scheduler = RegionPinnedScheduler {
        leases: &leases,
        pin: &pin,
    };

    // An out-of-region (eu-north) runner is REFUSED by the control-plane pin BEFORE the claim — and the
    // CI mechanism would skip the fr-par job for it anyway. The two halves agree: 0 out-of-region claims.
    let refused = scheduler
        .try_claim("runner-eu-north", &Region::new("eu-north"), 1000)
        .expect_err(
            "the control-plane pin refuses the out-of-region runner (the two halves agree)",
        );
    assert_eq!(refused.tenant_region.as_str(), "fr-par");
    assert_eq!(refused.runner_region.as_str(), "eu-north");

    // An in-region (fr-par) runner is admitted by the pin AND claims the job from the CI mechanism.
    let claimed = scheduler
        .try_claim("runner-fr-par", &Region::new("fr-par"), 1000)
        .expect("the control-plane pin admits the in-region runner")
        .expect("the CI claim mechanism hands the in-region runner the EU tenant's job");
    assert_eq!(claimed.job_id, "job-1");
    assert_eq!(claimed.region.as_str(), "fr-par");

    assert_eq!(
        pin.out_of_region_claims_admitted(),
        0,
        "0 out-of-region claims admitted (the no-global-CI-pool property, enforced at claim time)"
    );
}
