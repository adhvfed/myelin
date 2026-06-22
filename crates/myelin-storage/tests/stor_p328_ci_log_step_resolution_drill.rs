//! P-ST-26 (global P-328) GATE / DRILL — the T3 CI log tier `(job, step, byte-range)` index (C2) +
//! the X-1 `#step-<n>` jump-to-failure resolution. Emits a dated green artifact.
//!
//! **The GATE (storage.md §3.3 / prompt P-ST-26):** the `details_ref` `#step-<n>` resolves to the
//! EXACT failing step's bytes via the `(job, step, byte-range)` index (the storage realisation of
//! GIT-D10 / CI-D8's jump-to-failure). Telemetry: **the resolved byte-range matches the step's bytes.**
//!
//! **STOR-D1 / STOR-D2 must remain green (re-run):** those are the two permanent restore-verify gates
//! (their own drill files — `stor_d1_*`, `stor_d2_*` — run in the SAME `cargo test --workspace`
//! invocation as this one; this prompt adds an INDEX over the existing P-ST-20 sealing path and touches
//! NO restore/backup code, so they stay green by construction, and the whole-suite green is the re-run
//! evidence). The CI log segment is a T2 segment under the per-tenant DEK, so it INHERITS their
//! crypto-shred reach (the leg proven below).
//!
//! A green here is PROVEN (the resolved bytes printed), never claimed (EI-01 §3).
//!
//! ## Scope (named, EI-01 §4)
//! The C2 index SHAPE + the byte-exact `#step-<n>` resolution at unit scale over the in-process
//! `CiLogIndex` map + the P-ST-20 fs-backed archiver. FLOORS NAMED: the per-SUBJECT CI-log DEK (C1) is
//! **P-ST-27 (M4)** (a key-class swap on the same `DekContentWrap` seam); the real OLTP
//! `ci_log_index` table (`UNIQUE(job, step, seq)`) is the **P-S12/P-S15** backing swap; the real broker
//! firehose + object-store segment backing are inherited from the P-ST-20 archiver (P-S12 / P-ST-30).

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
    kms.ensure_kek(&KekId::new(tenant(), region()));
    Arc::new(kms)
}

/// **The P-328 GATE.** The X-1 `details_ref` `#step-<n>` resolves to the EXACT failing step's bytes
/// via the `(job, step, byte-range)` index; the resolved byte-range matches the step's bytes.
#[test]
fn p328_gate_step_anchor_resolves_to_exact_failing_step_bytes() {
    let tier = CiLogTier::with_tenant_dek("run-42", tenant(), region(), engine());

    // A run's step logs streamed into content-addressed, DEK-encrypted T2 segments (CI is the
    // heaviest log producer; the durable archive carries the index, never a per-line wake).
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

    // ── leg 1: content-addressed T2 segment (the P-ST-20 sealing rides through). ──
    assert!(seg
        .content_hash
        .to_multihash_string()
        .starts_with("blake3:"));
    assert_eq!(tier.archiver().telemetry().unencrypted_segment_count(), 0);
    assert!(tier.archiver().telemetry().segment_content_addressed());

    // ── leg 2: the #step-<n> jump-to-failure resolves to the EXACT failing step's bytes. ──
    let resolved = tier
        .resolve_step_anchor("myelin://acme/ci/run/run-42#step-3")
        .expect("resolve the #step-3 jump-to-failure");
    assert_eq!(
        resolved, step3,
        "GATE: #step-3 resolves to step 3's EXACT bytes (not step 2's, not the whole segment)"
    );

    // ── leg 3: the resolved byte-range matches the step's bytes (the GATE telemetry). ──
    assert_eq!(
        tier.step_log_len("run-42", 3),
        step3.len() as u64,
        "GATE telemetry: the indexed byte-range length matches the step's bytes"
    );
    // The neighbouring steps resolve to THEIR exact bytes too (byte-exact, no bleed).
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

/// **A step spanning MULTIPLE sealed segments resolves to its in-order concatenation** (the
/// streaming-log case: a long step's log arrives in many chunks across many seals).
#[test]
fn p328_gate_multi_segment_step_resolves_in_order() {
    let tier = CiLogTier::with_tenant_dek("run-9", tenant(), region(), engine());
    // step 2's log streams in three separate sealed batches (three segments).
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

/// **The crypto-shred-inheritance leg (the STOR-D1/D2 re-run tie-in).** A CI log segment IS a T2
/// segment under the per-tenant DEK: destroying that DEK renders the step's bytes unrecoverable — live
/// AND in backups by construction (§7.5). The per-SUBJECT DEK that scopes this to a single subject is
/// the C1 sibling P-ST-27.
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

    // Crypto-shred the per-tenant DEK the CI log segment is sealed under.
    assert!(
        eng.destroy_dek(&DekId::new(tenant(), KeyClass::Tenant)),
        "the tenant DEK was destroyed (the crypto-shred lever)"
    );

    // LIVE: the step is now unrecoverable — a LOUD failure, never a silent serve.
    let live = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tier.resolve_step("run-1", 1)
    }));
    assert!(
        live.is_err(),
        "live: a crypto-shredded CI log step is unrecoverable (LOUD), never served"
    );

    // IN BACKUPS: the destroyed DEK is EXCLUDED from the KMS backup snapshot (§7.5) — a restore
    // resurrects no shredded CI log step (the STOR-D1 erasure-held invariant inherited).
    let snapshot = eng.backup_snapshot();
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
         excluded from backup_snapshot) — STOR-D1 erasure-held inherited by the T3 CI log tier."
    );
}
