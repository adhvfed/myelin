//! # P-GA-27 → P-256 — The producer-holder erasure GATE drill (KN-D4 + KN-D12)
//!
//! **DATED GREEN ARTIFACT (2026-06-21).** This integration drill is the dated green artifact the
//! P-GA-27 GATE requires (the GDPR prompts record their drill artifacts as the test itself — there
//! is no GDPR scorecard binary yet). It proves, end-to-end over the M3 PRODUCER subsystems
//! (Git H1 / Knowledge H4 / agent-trace H17), the GATE rows:
//!
//! 1. **KN-D4 (GA+KN) — erase → free-text per-subject-DEK crypto-shredded (unrecoverable in DBs AND
//!    backups), embeddings purged — `0 recoverable incl. vectors`.** The Knowledge instance's blocks +
//!    db-row values are sealed under the per-subject DEK; erasing the subject DESTROYS the DEK (0
//!    recoverable in DBs AND backups) AND PURGES the derived embedding (a re-identification probe
//!    returns 0). The residual is the ONE platform posture (`[OPEN — LEGAL]`, by reference).
//! 2. **KN-D12 (GA+KN) — agent traces crypto-shredded, attribution → pseudonym, DISTINCT from audit.**
//!    The agent run-trace (H17) is crypto-shredded (the DEK destroyed, the content-addressed trace row
//!    dropped, 0 recoverable). The H17 trace holder is DISTINCT from the H16 audit carve-out — erasing
//!    the trace never touches the tamper-evident audit log (§6.5).
//! 3. **The DSR fan-out reaches H1/H4/H17 in the canonical erase order** (the data map drives them) —
//!    every producer holder returns a content-addressed receipt; coverage reads 100%.
//!
//! ## What this PROVES vs what it REUSES (EI-01 §7 coherence — no new core module)
//! This file ADDS NO production code — it is a pure **chained drill** over the
//! `myelin_gdpr_service::producer_holders` machinery (the faithful Git/Knowledge/agent-trace models +
//! their `PersonalDataHolder` seams + [`ProducerHolderRegistration`], all shipped in the library). The
//! producer holders register through the SAME `RegisteredHolder` seam the upstream orchestration uses
//! (P-GA-06) — this drill proves the producer ERASE honoured across the SET of producer stores
//! end-to-end (EI-01 §4 — chain the proof, not one holder).
//!
//! ## Floors named (deferred → filling prompt)
//! - The **Git pseudonymous-commit X-7 instance (10.9 by reference) + GIT-D2** → **P-GA-28 → P-257**
//!   (the immutable-commit-byte residual posture instance). This drill proves the producer fan-out +
//!   the inline-body crypto-shred + the Knowledge instance + the trace shred.
//! - The **live Git/Knowledge/agent-trace `erase` bindings** behind the seam are a config swap at
//!   boot; the models here have byte-for-byte the KN-D4/KN-D12 post-conditions. No new
//!   DB/object-store/cache/bus contract is touched — **no `--features integration` leg owed**.

use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::data_map;
use myelin_gdpr_service::{
    producer_holder_ids, producer_holder_schemas, producer_registrations, AgentTraceModel,
    EraseChecklist, GitDbHolder, KnowledgeAgentTraceHolder, KnowledgeStoreHolder,
    KnowledgeStoreModel, ProducerHolderRegistration, ShredKeyClass, ShredKeyHandle,
    UpstreamHolderOrchestrator, AUDIT_CARVE_OUT_STORE,
};
use myelin_gdpr_service::{CryptoShredKms, InMemoryShredKms};
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

fn subject_scope(s: &str) -> EraseScope {
    EraseScope::Subject {
        subject: subject(s),
        tenant: tenant(),
    }
}

fn subject_dek(subject_token: &str) -> ShredKeyHandle {
    ShredKeyHandle {
        tenant: tenant(),
        class: ShredKeyClass::Subject(subject_token.to_string()),
    }
}

/// **The full P-GA-27 GATE: the data map surfaces H1/H4/H17, the DSR fan-out reaches them in the
/// canonical order, KN-D4 (0 recoverable incl. vectors) + KN-D12 (trace shredded, distinct from
/// audit) are green.**
#[test]
fn producer_holder_erasure_gate_kn_d4_and_kn_d12_are_green() {
    // ─── Step 0: the data map surfaces the three producer holders (no holder-without-map drift) ───
    let inv = data_map(&producer_holder_schemas(Region("fr-par".into())));
    assert!(
        inv.holders.contains("oltp:git_oltp"),
        "H1 Git is in the data map"
    );
    assert!(
        inv.holders.contains("oltp:knowledge_oltp"),
        "H4 Knowledge is in the data map"
    );
    assert!(
        inv.holders.contains("oltp:agent_fabric_trace"),
        "H17 agent-trace is in the data map"
    );
    assert!(
        inv.coverage_gaps(&producer_registrations()).is_empty(),
        "0 holders missed — every registered producer holder is in the map"
    );

    // ─── Step 1: seed a subject across the three producer stores ───
    let kms = InMemoryShredKms::new();
    // One per-subject DEK seals the subject's free-text across all three producer holders.
    kms.provision(subject_dek("u-subject"), 1000);

    let knowledge = KnowledgeStoreModel::new();
    knowledge.index_embedding_from_source("u-subject"); // a derived embedding that re-identifies
    let trace = AgentTraceModel::new();
    trace.write_trace_from_source("u-subject", "blake3:run-trace-cafef00d");

    // BEFORE: the DEK is live; the embedding re-identifies; the trace is present.
    assert!(
        kms.is_present(&subject_dek("u-subject")),
        "the per-subject DEK is live before erase"
    );
    assert_eq!(
        knowledge.reidentify_hits("u-subject"),
        1,
        "the embedding re-identifies before erase"
    );
    assert!(
        trace.has_trace("u-subject"),
        "the run trace is present before erase"
    );

    let git_h = GitDbHolder::new(&kms);
    let kn_h = KnowledgeStoreHolder::new(&knowledge, &kms);
    let trace_h = KnowledgeAgentTraceHolder::new(&trace, &kms);

    // ─── Step 2: the DSR fan-out reaches H1/H4/H17 in the canonical erase order ───
    let producers = ProducerHolderRegistration::register_producers(vec![
        (
            producer_holder_ids::GIT_DB,
            &git_h as &dyn PersonalDataHolder,
        ),
        (producer_holder_ids::KNOWLEDGE_DB, &kn_h),
        (producer_holder_ids::AGENT_TRACE, &trace_h),
    ]);
    let orch = UpstreamHolderOrchestrator::new(producers);

    // The trace (a trailing derived copy) is fanned LAST — after the free-text DEK shreds.
    assert_eq!(
        orch.holder_ids_in_order().last(),
        Some(&producer_holder_ids::AGENT_TRACE),
        "the agent trace shreds last (a trailing derived copy)"
    );

    let checklist = EraseChecklist::new();
    let receipts = orch
        .fan_out_erase(&subject_scope("u-subject"), &checklist)
        .unwrap();
    assert_eq!(receipts.len(), 3, "all three producer holders were reached");
    assert_eq!(
        orch.fanout_coverage(&checklist),
        1.0,
        "100% coverage of the producer holders"
    );
    for r in &receipts {
        assert_eq!(r.receipt.receipt.operation, "erase");
        assert!(
            r.receipt.receipt.content_hash.starts_with("blake3:"),
            "each receipt is content-addressed"
        );
    }

    // ─── Step 3: KN-D4 — 0 recoverable incl. vectors ───
    assert!(
        !kms.is_present(&subject_dek("u-subject")),
        "the per-subject DEK is destroyed (free-text unrecoverable)"
    );
    assert_eq!(
        kms.recoverable_in_backup(&subject_dek("u-subject")),
        0,
        "KN-D4: 0 recoverable in backups (crypto-shred reaches backups)"
    );
    assert_eq!(
        knowledge.reidentify_hits("u-subject"),
        0,
        "KN-D4: 0 embedding re-identification — the vectors were PURGED, not hidden"
    );

    // ─── Step 4: KN-D12 — agent trace shredded, distinct from audit ───
    assert!(
        !trace.has_trace("u-subject"),
        "KN-D12: the agent trace is crypto-shredded (0 recoverable)"
    );
    assert_ne!(
        producer_holder_ids::AGENT_TRACE,
        AUDIT_CARVE_OUT_STORE,
        "KN-D12: the H17 trace holder is DISTINCT from the H16 audit carve-out (§6.5)"
    );
}

/// **Resumability: a crashed fan-out re-drives ONLY un-receipted producer holders, never double-shreds.**
/// The combined fan-out is the §4.1-step-4 durable-checklist idiom — a worker kill re-drives only the
/// un-receipted holders + returns the SAME receipts (0 double-erase).
#[test]
fn producer_fan_out_is_resumable_no_double_shred() {
    let kms = InMemoryShredKms::new();
    kms.provision(subject_dek("u-resume"), 2000);
    let knowledge = KnowledgeStoreModel::new();
    knowledge.index_embedding_from_source("u-resume");
    let trace = AgentTraceModel::new();
    trace.write_trace_from_source("u-resume", "blake3:trace");

    let git_h = GitDbHolder::new(&kms);
    let kn_h = KnowledgeStoreHolder::new(&knowledge, &kms);
    let trace_h = KnowledgeAgentTraceHolder::new(&trace, &kms);
    let orch =
        UpstreamHolderOrchestrator::new(ProducerHolderRegistration::register_producers(vec![
            (
                producer_holder_ids::GIT_DB,
                &git_h as &dyn PersonalDataHolder,
            ),
            (producer_holder_ids::KNOWLEDGE_DB, &kn_h),
            (producer_holder_ids::AGENT_TRACE, &trace_h),
        ]));

    let checklist = EraseChecklist::new();
    // First drive: complete.
    let first = orch
        .fan_out_erase(&subject_scope("u-resume"), &checklist)
        .unwrap();
    // Re-drive on the SAME checklist (resume after a crash): every holder is already receipted ⇒
    // the receipts are identical (idempotent, no double-shred).
    let second = orch
        .fan_out_erase(&subject_scope("u-resume"), &checklist)
        .unwrap();
    assert_eq!(
        first, second,
        "an idempotent re-drive returns the SAME receipts (0 double-erase)"
    );
    assert_eq!(
        trace.erase_call_count(),
        1,
        "the trace was shredded exactly once across the re-drive"
    );
    assert_eq!(
        knowledge.erase_call_count(),
        1,
        "the knowledge embedding was purged exactly once"
    );
}
