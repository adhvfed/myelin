//! Contract 11.4-C1 CDC pair — the per-subject CI-log DEK (P-ST-27 / global P-329), with **CI as
//! the consumer**.
//!
//! The prompt requires "a provider+consumer pair for 11.4-C1 (CI as the consumer)". This is the
//! consumer-driven contract test:
//!
//! - The **PROVIDER** is `myelin-storage`'s [`CiLogTier`] (the C1 per-subject CI-log DEK this prompt
//!   ships — an isolable inline-PII CI log segment sealed under the subject's per-subject DEK; a
//!   non-isolable / interleaved segment falls back to the per-tenant DEK, the documented 10.9
//!   residual).
//! - The **CONSUMER** is CI: when a runner attributes a CI log chunk's inline PII to one subject
//!   (isolable), CI seals it via `seal_ci_batch_for_subject(subject, …)`; when the chunk interleaves
//!   many subjects' free text (non-isolable), CI seals it via the plain `seal_ci_batch(…)`. CI then
//!   reads back the keying (`step_keying`) to confirm a subject's CI log content is isolated to their
//!   DEK (so their erasure crypto-shreds exactly it), and resolves `#step-<n>` to the exact bytes.
//!
//! The test pins the frozen 11.4-C1 call shape CI relies on: `seal_ci_batch_for_subject` keys under
//! the subject DEK (`SegmentKeying::Subject`), `seal_ci_batch` falls back to the tenant DEK
//! (`SegmentKeying::Tenant`), and both resolve byte-exact. If that shape drifts, this stops
//! compiling/passing.

use myelin_storage::{CiLogFrame, CiLogTier, KekId, KmsEngine, SegmentKeying, SubjectId};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

/// The CONSUMER: CI streaming a run's step logs through the C1 provider, choosing per-subject (when
/// isolable) vs per-tenant (when not), then confirming the keying + resolving the bytes.
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

    /// Stream an ISOLABLE inline-PII step chunk attributed to one subject → the per-subject DEK (C1).
    fn stream_isolable_step(&mut self, step_no: u32, subject: &SubjectId, log: &str) {
        self.next_seq += 1;
        let frame = CiLogFrame::new(&self.run_id, step_no, log.as_bytes().to_vec());
        self.tier
            .seal_ci_batch_for_subject(subject, &[(self.next_seq, frame)])
            .expect("CI seals an isolable-PII step under the subject DEK (C1)");
    }

    /// Stream a NON-ISOLABLE (interleaved) step chunk → the per-tenant DEK fallback (the residual).
    fn stream_non_isolable_step(&mut self, step_no: u32, log: &str) {
        self.next_seq += 1;
        let frame = CiLogFrame::new(&self.run_id, step_no, log.as_bytes().to_vec());
        self.tier
            .seal_ci_batch(&[(self.next_seq, frame)])
            .expect("CI seals a non-isolable step under the tenant DEK (the residual fallback)");
    }
}

/// THE CDC pair: CI streams an isolable-PII step (per-subject DEK) and a non-isolable step (tenant
/// DEK fallback), then reads back the keying + resolves both byte-exact — the provider
/// (`myelin-storage`'s `CiLogTier`) honours the frozen 11.4-C1 per-subject-vs-tenant key-choice.
#[test]
fn cdc_11_4_c1_ci_keys_isolable_pii_per_subject_and_falls_back_per_tenant() {
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let engine = Arc::new(KmsEngine::new());
    engine.ensure_kek(&KekId::new(tenant.clone(), region.clone()));

    let mut ci = CiRunLogs::boot(tenant, region, engine, "run-7");
    let alice = SubjectId::new("u-alice");

    // step 1: an isolable inline-PII chunk for alice → per-subject DEK (C1).
    ci.stream_isolable_step(1, &alice, "==> deploy\nalice@corp.test triggered deploy\n");
    // step 2: interleaved free-text from many → per-tenant DEK fallback (the 10.9 residual).
    ci.stream_non_isolable_step(2, "==> test\nmany contributors' interleaved output\n");

    // The provider keyed step 1 under alice's DEK (the C1 lever) and step 2 under the tenant DEK.
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

    // Both resolve byte-exact through the matching archiver (the C2 #step-<n> path is unchanged).
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

    // The C1 lever is live (one subject's DEK in use) and the C2 telemetry rides through.
    assert_eq!(ci.tier.subject_keyed_count(), 1);
    assert_eq!(
        ci.tier.archiver().telemetry().unencrypted_segment_count(),
        0
    );
    assert!(ci.tier.archiver().telemetry().segment_content_addressed());
}
