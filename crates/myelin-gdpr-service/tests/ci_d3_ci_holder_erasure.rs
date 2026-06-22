//! # P-GA-29 → P-332 — The CI consumer-holder erasure GATE drill (CI-D3)
//!
//! **DATED GREEN ARTIFACT (2026-06-22).** This integration drill is the dated green artifact the
//! P-GA-29 GATE requires (the GDPR prompts record their drill artifacts as the test itself — there is
//! no GDPR scorecard binary yet). It proves, end-to-end over the CI consumer holder, the GATE row:
//!
//! **CI-D3 (SCHED, GDPR-anchored) — erase fans to CI → PII in logs/artifacts/caches/run-state
//! destroyed (per-subject DEK where isolable, per-tenant fallback) incl. backups; structure survives;
//! 0 dangling leak.** The drill fans a DSR erase through the orchestrator to the CI holder (H2) and
//! proves:
//! 1. **Per-subject DEK where isolable.** The erased subject's per-subject CI-log DEK is destroyed —
//!    their isolable inline log PII is unrecoverable ciphertext, live AND in backups (0 recoverable).
//! 2. **0 dangling leak / the per-subject reach.** A DIFFERENT subject's CI log AND the per-tenant
//!    fallback key survive a single-subject erase (the C1/P5 reach is per-subject, not a blunt
//!    per-tenant erase) — no dangling leak of the erased subject, no over-erase of the others.
//! 3. **Per-tenant fallback.** A tenant offboarding destroys the per-tenant CI-log DEK fallback (the
//!    non-isolable interleaved PII goes with the tenant).
//! 4. **Structure survives.** The run-graph topology remains after the erase (the PII is shredded, the
//!    structure remains — §3.2).
//!
//! The telemetry: the DSR receipt + the per-subject-DEK key-shred count are the green artifacts; the
//! data-map diff surfaces H2 (no drift).
//!
//! ## What this PROVES vs what it REUSES (EI-01 §7 coherence — no new core module)
//! This file ADDS NO production code — it is a pure **chained drill** over the
//! `myelin_gdpr_service::ci_instance` machinery (the faithful CI holder + its `PersonalDataHolder`
//! seam + the `CiHolderRegistration` + the `UpstreamHolderOrchestrator`, all shipped in the library).
//! H2 registers through the SAME `RegisteredHolder` seam the upstream orchestration uses (P-GA-06) —
//! this drill proves the CI per-subject-DEK reach + the per-tenant fallback + structure-survives
//! end-to-end.
//!
//! ## Floors named (deferred → filling prompt)
//! - The **per-subject-where-isolable / per-tenant-fallback split** is the honest answer (named): the
//!   structural reach (the per-subject CI-log DEK shred) ships here; the non-isolable interleaved
//!   residual is the ONE platform-posture residual (10.9), `[OPEN — LEGAL]` like every other
//!   subsystem's residual.
//! - The **Issues (H3) + Chat (H5) consumer holders** over this SAME consumer-holder pattern →
//!   **P-GA-30 → P-333**.
//! - The **live CI `erase` binding** behind the seam is a config swap at boot; the per-subject CI-log
//!   DEK mechanism is `myelin-storage`'s, its OWN live-stack integration proof owned storage-side
//!   (P-329 / STOR-D4-C1). No new DB/object-store/cache/bus contract is touched — **no
//!   `--features integration` leg owed** here.

use myelin_gdpr::{EraseScope, PersonalDataHolder, Receipt, SubjectRef, TenantId};
use myelin_gdpr_service::{
    ci_holder_schemas, ci_registrations, data_map, CiHolderRegistration, CiLogHolder, CiLogModel,
    CryptoShredKms, EraseChecklist, InMemoryShredKms, ShredKeyClass, ShredKeyHandle,
    UpstreamHolderOrchestrator, CI_DB,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::Region;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

fn subject_scope(id: &str) -> EraseScope {
    EraseScope::Subject {
        subject: subject(id),
        tenant: tenant(),
    }
}

fn subject_dek(id: &str) -> ShredKeyHandle {
    ShredKeyHandle {
        tenant: tenant(),
        class: ShredKeyClass::Subject(id.into()),
    }
}

fn tenant_dek() -> ShredKeyHandle {
    ShredKeyHandle {
        tenant: tenant(),
        class: ShredKeyClass::Tenant,
    }
}

/// **The P-GA-29 GATE: CI-D3 — the per-subject CI-log DEK crypto-shred reaches isolable log PII via a
/// DSR fan-out; a different subject + the per-tenant fallback survive; structure survives; 0 dangling
/// leak incl. backups.** Driven end-to-end through the orchestrator (the consumer) to the CI holder
/// (the provider).
#[test]
fn ci_d3_erase_fans_to_ci_per_subject_dek_zero_dangling_leak_structure_survives() {
    let kms = InMemoryShredKms::new();
    // Two subjects' isolable CI-log DEKs + the per-tenant fallback are live before erase.
    kms.provision(subject_dek("u-erase"), 200);
    kms.provision(subject_dek("u-keep"), 201);
    kms.provision(tenant_dek(), 202);

    let model = CiLogModel::new();
    model.index_run_graph_from_source("u-erase");
    model.index_run_graph_from_source("u-keep");

    let ci_h = CiLogHolder::new(&model, &kms);

    // ── H2 is in the data map (no holder-without-map drift — the fan-out reaches it structurally). ──
    let inv = data_map(&ci_holder_schemas(Region("fr-par".into())));
    assert!(
        inv.holders.contains("oltp:ci_oltp"),
        "H2 CI is in the data map"
    );
    assert!(
        inv.coverage_gaps(&ci_registrations()).is_empty(),
        "the registered CI holder is in the map — 0 holders missed"
    );

    // ── The DSR fan-out reaches the CI holder + erases the subject. ──
    let ci = CiHolderRegistration::register_ci(vec![(CI_DB, &ci_h as &dyn PersonalDataHolder)]);
    let orch = UpstreamHolderOrchestrator::new(ci);
    let checklist = EraseChecklist::new();
    let receipts = orch
        .fan_out_erase(&subject_scope("u-erase"), &checklist)
        .unwrap();
    assert_eq!(receipts.len(), 1, "the fan-out reached the CI holder");
    assert_eq!(orch.fanout_coverage(&checklist), 1.0, "100% CI coverage");

    // (1) Per-subject DEK where isolable: the erased subject's per-subject CI-log DEK is destroyed.
    assert!(
        !kms.is_present(&subject_dek("u-erase")),
        "the erased subject's per-subject CI-log DEK is destroyed"
    );
    // incl. backups — 0 recoverable.
    assert_eq!(
        kms.recoverable_in_backup(&subject_dek("u-erase")),
        0,
        "0 recoverable in backups (crypto-shred reaches backups — CI-D3)"
    );

    // (2) 0 dangling leak / the per-subject reach: a DIFFERENT subject + the per-tenant fallback survive.
    assert!(
        kms.is_present(&subject_dek("u-keep")),
        "a different subject's CI log survives (the per-subject reach, not a blunt per-tenant erase)"
    );
    assert!(
        kms.is_present(&tenant_dek()),
        "the per-tenant fallback key survives a single-subject erase"
    );

    // (4) Structure survives: the run-graph topology of BOTH subjects remains.
    assert!(
        model.run_graph_present("u-erase"),
        "the erased subject's run-graph structure survives (PII shredded, structure remains)"
    );
    assert!(
        model.run_graph_present("u-keep"),
        "the other subject's structure is untouched"
    );

    // The DSR receipt is the green artifact (content-addressed, records the destroyed key epoch).
    let r = &receipts[0].receipt.receipt;
    assert_eq!(r.operation, "erase");
    assert!(
        r.content_hash.starts_with("blake3:"),
        "content-addressed DSR receipt"
    );
    assert!(
        r.key_epoch_destroyed.is_some(),
        "the per-subject-DEK key-shred is recorded (the CI-D3 telemetry green artifact)"
    );
    // The receipt names the per-subject CI-log DEK reach (the outcome is folded into the content
    // hash) — proven via the expected content-address.
    let expected = Receipt::content_addressed(
        "erase",
        CI_DB,
        "u-erase",
        "acme",
        "crypto_shred:per_subject_ci_log_dek:isolable_segments;structure_survives",
        r.key_epoch_destroyed,
        0,
    );
    assert_eq!(
        r.content_hash, expected.content_hash,
        "the receipt names the per-subject CI-log DEK reach (the C1/P5 extension)"
    );
}

/// **CI-D3 (the per-tenant fallback leg) — a tenant offboarding destroys the per-tenant CI-log DEK
/// fallback (the non-isolable interleaved PII goes with the tenant).** The OTHER polarity of the
/// per-subject-where-isolable / per-tenant-fallback split.
#[test]
fn ci_d3_tenant_offboarding_destroys_the_per_tenant_fallback() {
    let kms = InMemoryShredKms::new();
    kms.provision(subject_dek("u-iso"), 300);
    kms.provision(tenant_dek(), 301);
    let model = CiLogModel::new();
    let ci_h = CiLogHolder::new(&model, &kms);

    let receipt = ci_h.erase(EraseScope::Tenant(tenant())).unwrap();

    assert!(
        !kms.is_present(&tenant_dek()),
        "a tenant offboarding destroys the per-tenant CI-log DEK fallback"
    );
    assert_eq!(
        kms.recoverable_in_backup(&tenant_dek()),
        0,
        "0 recoverable in backups"
    );
    // The tenant-scope erase names the per-tenant fallback (proven via the expected content-hash; the
    // offboarding subject token is the "*tenant*" sentinel).
    let expected = Receipt::content_addressed(
        "erase",
        CI_DB,
        "*tenant*",
        "acme",
        "crypto_shred:per_tenant_ci_log_dek_fallback:tenant_offboard;structure_survives",
        receipt.receipt.key_epoch_destroyed,
        0,
    );
    assert_eq!(
        receipt.receipt.content_hash, expected.content_hash,
        "the tenant-scope erase names the per-tenant fallback (the non-isolable interleaved PII)"
    );
}
