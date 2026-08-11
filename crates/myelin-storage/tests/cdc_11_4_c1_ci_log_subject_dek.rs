use myelin_storage::{CiLogFrame, CiLogTier, KekId, KmsEngine, SegmentKeying, SubjectId};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

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

    fn stream_isolable_step(&mut self, step_no: u32, subject: &SubjectId, log: &str) {
        self.next_seq += 1;
        let frame = CiLogFrame::new(&self.run_id, step_no, log.as_bytes().to_vec());
        self.tier
            .seal_ci_batch_for_subject(subject, &[(self.next_seq, frame)])
            .expect("CI seals an isolable-PII step under the subject DEK (C1)");
    }

    fn stream_non_isolable_step(&mut self, step_no: u32, log: &str) {
        self.next_seq += 1;
        let frame = CiLogFrame::new(&self.run_id, step_no, log.as_bytes().to_vec());
        self.tier
            .seal_ci_batch(&[(self.next_seq, frame)])
            .expect("CI seals a non-isolable step under the tenant DEK (the residual fallback)");
    }
}

#[test]
fn cdc_11_4_c1_ci_keys_isolable_pii_per_subject_and_falls_back_per_tenant() {
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let engine = Arc::new(KmsEngine::new());
    engine
        .ensure_kek(&KekId::new(tenant.clone(), region.clone()))
        .expect("seed the in-memory KEK");

    let mut ci = CiRunLogs::boot(tenant, region, engine, "run-7");
    let alice = SubjectId::new("u-alice");

    ci.stream_isolable_step(1, &alice, "==> deploy\nalice@corp.test triggered deploy\n");
    ci.stream_non_isolable_step(2, "==> test\nmany contributors' interleaved output\n");

    assert_eq!(
        ci.tier.step_keying("run-7", 1).unwrap(),
        vec![SegmentKeying::Subject(alice.clone())],
        "11.4-C1: an isolable-PII step keys under the subject's DEK"
    );
    assert_eq!(
        ci.tier.step_keying("run-7", 2).unwrap(),
        vec![SegmentKeying::Tenant],
        "11.4-C1: a non-isolable step falls back to the per-tenant DEK (the documented residual)"
    );

    assert_eq!(
        ci.tier
            .resolve_step_anchor("myelin://acme/ci/run/run-7#step-1")
            .unwrap(),
        b"==> deploy\nalice@corp.test triggered deploy\n"
    );
    assert_eq!(
        ci.tier
            .resolve_step_anchor("myelin://acme/ci/run/run-7#step-2")
            .unwrap(),
        b"==> test\nmany contributors' interleaved output\n"
    );

    assert_eq!(ci.tier.subject_keyed_count(), 1);
    assert_eq!(
        ci.tier.archiver().telemetry().unencrypted_segment_count(),
        0
    );
    assert!(ci.tier.archiver().telemetry().segment_content_addressed());
}
