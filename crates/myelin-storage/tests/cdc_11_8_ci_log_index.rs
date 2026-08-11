use myelin_storage::{CiLogError, CiLogFrame, CiLogTier, KekId, KmsEngine};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

struct CheckStatus {
    conclusion: &'static str,
    details_ref: String,
}

struct CiRunLogs {
    tier: CiLogTier,
    run_id: String,
    next_seq: u64,
}

impl CiRunLogs {
    fn boot(tenant: TenantId, region: Region, engine: Arc<KmsEngine>, run_id: &str) -> CiRunLogs {
        CiRunLogs {
            tier: CiLogTier::with_tenant_dek(run_id, tenant, region, engine),
            run_id: run_id.to_string(),
            next_seq: 0,
        }
    }

    fn stream_step(&mut self, step_no: u32, log: &str) {
        self.next_seq += 1;
        let frame = CiLogFrame::new(&self.run_id, step_no, log.as_bytes().to_vec());
        self.tier
            .seal_ci_batch(&[(self.next_seq, frame)])
            .expect("CI seals its step log into a content-addressed T2 segment");
    }

    fn mint_check_status(&self, failing_step: u32) -> CheckStatus {
        CheckStatus {
            conclusion: "failure",
            details_ref: format!("myelin://acme/ci/run/{}#step-{failing_step}", self.run_id),
        }
    }

    fn jump_to_failure(&self, check: &CheckStatus) -> Result<Vec<u8>, CiLogError> {
        self.tier.resolve_step_anchor(&check.details_ref)
    }
}

#[test]
fn cdc_11_8_ci_resolves_details_ref_step_anchor_to_exact_failing_bytes() {
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let engine = Arc::new(KmsEngine::new());
    engine
        .ensure_kek(&KekId::new(tenant.clone(), region.clone()))
        .expect("seed the in-memory KEK");

    let mut ci = CiRunLogs::boot(tenant, region, engine, "run-42");

    ci.stream_step(1, "==> checkout\nFetching repo... ok\n");
    ci.stream_step(2, "==> build\ncargo build... ok\n");
    ci.stream_step(
        3,
        "==> test\ntest auth::login ... FAILED\nassertion failed at line 88\n",
    );

    let check = ci.mint_check_status(3);
    assert_eq!(check.conclusion, "failure");
    assert!(check.details_ref.ends_with("#step-3"));
    assert!(!check.details_ref.contains("FAILED"));

    let bytes = ci
        .jump_to_failure(&check)
        .expect("resolve the #step-3 jump-to-failure");
    assert_eq!(
        bytes, b"==> test\ntest auth::login ... FAILED\nassertion failed at line 88\n",
        "11.8/C2: the details_ref #step-<n> resolves to step 3's EXACT bytes, not a neighbour's"
    );

    let step1 = ci
        .tier
        .resolve_step_anchor("myelin://acme/ci/run/run-42#step-1")
        .expect("resolve step 1");
    assert_eq!(step1, b"==> checkout\nFetching repo... ok\n");

    assert_eq!(
        ci.tier.archiver().telemetry().unencrypted_segment_count(),
        0
    );
    assert!(ci.tier.archiver().telemetry().segment_content_addressed());
    assert_eq!(ci.tier.indexed_step_count(), 3);
}
