use myelin_storage::{CiLogFrame, CiLogTier, DekId, KekId, KeyClass, KmsEngine};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn engine() -> Arc<KmsEngine> {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant(), region()))
        .expect("seed the in-memory KEK");
    Arc::new(kms)
}

#[test]
fn p328_gate_step_anchor_resolves_to_exact_failing_step_bytes() {
    let tier = CiLogTier::with_tenant_dek("run-42", tenant(), region(), engine());

    let step1 = b"==> checkout\nok\n".to_vec();
    let step2 = b"==> build\ncargo build --workspace ... ok\n".to_vec();
    let step3 = b"==> test\ntest auth::login ... FAILED\npanicked at line 88\n".to_vec();
    let seg = tier
        .seal_ci_batch(&[
            (1, CiLogFrame::new("run-42", 1, step1.clone())),
            (2, CiLogFrame::new("run-42", 2, step2.clone())),
            (3, CiLogFrame::new("run-42", 3, step3.clone())),
        ])
        .expect("seal the CI log batch into a content-addressed T2 segment");

    assert!(seg
        .content_hash
        .to_multihash_string()
        .starts_with("blake3:"));
    assert_eq!(tier.archiver().telemetry().unencrypted_segment_count(), 0);
    assert!(tier.archiver().telemetry().segment_content_addressed());

    let resolved = tier
        .resolve_step_anchor("myelin://acme/ci/run/run-42#step-3")
        .expect("resolve the #step-3 jump-to-failure");
    assert_eq!(
        resolved, step3,
        "GATE: #step-3 resolves to step 3's EXACT bytes (not step 2's, not the whole segment)"
    );

    assert_eq!(
        tier.step_log_len("run-42", 3),
        step3.len() as u64,
        "GATE telemetry: the indexed byte-range length matches the step's bytes"
    );
    assert_eq!(
        tier.resolve_step_anchor("myelin://acme/ci/run/run-42#step-1")
            .unwrap(),
        step1
    );
    assert_eq!(
        tier.resolve_step_anchor("myelin://acme/ci/run/run-42#step-2")
            .unwrap(),
        step2
    );

    println!(
        "[P-328 DRILL GREEN 2026-06-22] T3 CI log tier (C2): sealed 3 step logs into \
         content-addressed segment {} under the per-tenant DEK; the (job,step,byte-range) index \
         resolved #step-3 to its EXACT {} bytes (jump-to-failure), neighbours step-1/step-2 byte-exact; \
         resolved_byte_range_len={}, unencrypted_segment_count=0, segment_content_addressed={}.",
        seg.content_hash.to_multihash_string(),
        step3.len(),
        tier.step_log_len("run-42", 3),
        tier.archiver().telemetry().segment_content_addressed(),
    );
}

#[test]
fn p328_gate_multi_segment_step_resolves_in_order() {
    let tier = CiLogTier::with_tenant_dek("run-9", tenant(), region(), engine());
    tier.seal_ci_batch(&[(1, CiLogFrame::new("run-9", 2, b"line-1\n".to_vec()))])
        .expect("seal 1");
    tier.seal_ci_batch(&[(2, CiLogFrame::new("run-9", 2, b"line-2\n".to_vec()))])
        .expect("seal 2");
    tier.seal_ci_batch(&[(3, CiLogFrame::new("run-9", 2, b"FAILED line-3\n".to_vec()))])
        .expect("seal 3");

    let resolved = tier
        .resolve_step_anchor("myelin://acme/ci/run/run-9#step-2")
        .expect("resolve the multi-segment step");
    assert_eq!(
        resolved, b"line-1\nline-2\nFAILED line-3\n",
        "the multi-segment step reconstructs in firehose-seq order"
    );
    println!(
        "[P-328 DRILL GREEN 2026-06-22] multi-segment step: #step-2 spanning 3 sealed segments \
         resolved to its {}-byte in-order concatenation.",
        resolved.len()
    );
}

#[test]
fn p328_gate_destroyed_tenant_dek_crypto_shreds_the_ci_log_step_live_and_in_backups() {
    let eng = engine();
    let tier = CiLogTier::with_tenant_dek("run-1", tenant(), region(), eng.clone());
    tier.seal_ci_batch(&[(
        1,
        CiLogFrame::new("run-1", 1, b"inline-PII-step-log".to_vec()),
    )])
    .expect("seal");
    assert!(
        tier.resolve_step("run-1", 1).is_ok(),
        "the step resolves before the shred"
    );

    assert!(
        eng.destroy_dek(&DekId::new(tenant(), KeyClass::Tenant))
            .expect("the in-memory key registry remains available"),
        "the tenant DEK was destroyed (the crypto-shred lever)"
    );

    let live = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tier.resolve_step("run-1", 1)
    }));
    assert!(
        live.is_err(),
        "live: a crypto-shredded CI log step is unrecoverable (LOUD), never served"
    );

    let snapshot = eng
        .backup_snapshot()
        .expect("the in-memory key registry remains available");
    let in_backup = snapshot
        .iter()
        .any(|(id, _)| *id == DekId::new(tenant(), KeyClass::Tenant));
    assert!(
        !in_backup,
        "in backups: the shredded tenant DEK is excluded from the backup snapshot (CI log step stays dead)"
    );

    println!(
        "[P-328 DRILL GREEN 2026-06-22] crypto-shred inheritance: destroying the per-tenant DEK \
         renders the CI log step's bytes unrecoverable LIVE (loud refusal) and IN BACKUPS (the DEK is \
         excluded from backup_snapshot) - STOR-D1 erasure-held inherited by the T3 CI log tier."
    );
}
