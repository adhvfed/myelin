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

struct RegionPinnedScheduler<'a> {
    leases: &'a JobLeaseStore,
    pin: &'a RunnerClaimPin,
}

impl RegionPinnedScheduler<'_> {
    fn try_claim(
        &self,
        worker: &str,
        runner_region: &Region,
        now: i64,
    ) -> Result<Option<QueuedJob>, OutOfRegionRunnerClaim> {
        self.pin.admit_claim(runner_region)?;
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

    let leases = JobLeaseStore::new();
    leases.enqueue(QueuedJob::new(
        tenant.clone(),
        region.clone(),
        "run-1",
        "job-1",
        vec!["linux".into()],
        ci_spec("idem-1"),
    ));

    let pin = RunnerClaimPin::for_tenant(tenant.clone(), region.clone());

    let scheduler = RegionPinnedScheduler {
        leases: &leases,
        pin: &pin,
    };

    let refused = scheduler
        .try_claim("runner-eu-north", &Region::new("eu-north"), 1000)
        .expect_err(
            "the control-plane pin refuses the out-of-region runner (the two halves agree)",
        );
    assert_eq!(refused.tenant_region.as_str(), "fr-par");
    assert_eq!(refused.runner_region.as_str(), "eu-north");

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
