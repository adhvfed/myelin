//! # ISS-P36 / P-499 (M5) — the whole-system E2E-3 wedge: spec-to-ship traceability (Issues' slice)
//!
//! **Spec-to-ship traceability** — *spec → issue → PR → CI → deploy → chat* (testing-strategy
//! `01-whole-system-e2e-and-drill-catalogue.md` §E2E-3). An artifact's full causal lineage is
//! reconstructable PER-VIEWER from the reference graph + the tamper-evident audit log, and survives a
//! reindex-from-cold. This file is the Issues-side cross-module proof of the three E2E-3 legs (the
//! in-module `src/e2e_lineage.rs` tests pin the lineage-walk + the cold-reindex predicate; here we add
//! the DURABLE audit-tamper leg over the REAL GDPR hash-chain + the CDC re-asserts):
//!
//! 1. **Complete lineage per-viewer** — re-asserted via the named green artifact (the insider walks the
//!    whole chain; an outsider's confidential hop tombstones carrying the root, 0 leak; the lineage
//!    degrades gracefully).
//! 2. **Cold-reindex == live (2.6)** — re-asserted via the artifact (the cold-rebuilt issue/relation set
//!    byte-matches the live truth, 0 drift) — the SAME `IssueReindexSource::replay` floor, under E2E load.
//! 3. **Audit tamper detected (GA-D3)** — the deploy that ships the lineage records a tamper-evident
//!    audit entry whose `subject` is the Issues lineage anchor (`lineage_audit_anchor` — the PII-free
//!    initiative ref). The entry rides the REAL `myelin_gdpr_service::audit` per-tenant BLAKE3 hash-chain
//!    (P-GA-19); the pristine chain verifies, and a RETROACTIVE EDIT to the lineage entry BREAKS the
//!    chain (`verify_entries_for_test == false`). Issues authors NO second tamper-evidence frame — it
//!    feeds the lineage anchor into the ONE GDPR chain (EI-01 §7).
//!
//! **No new contract** — this EXERCISES the frozen contracts (5.6 project; 2.6 reindex-from-source; 10.4
//! the DSR/audit anchor) end-to-end. Runs under the MOCK agent runtime (real-LLM is the post-M5 swap,
//! R-10). FLOOR named: none new (the world-scale 30x load is ISS-P33's named floor). The GDPR chain is a
//! DEV-dependency edge (test-support), not a runtime DAG node — the normal graph stays acyclic.

use myelin_issues::{
    lineage_audit_anchor, run_e2e_3_lineage, run_issues_e2e_3, E2E_LINEAGE_SCENARIO,
};

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId, EventType,
    HandleOutcome, Timestamp, Visibility,
};
use myelin_gdpr_service::{audit, AuditAuthority, CellSigningKey};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{ArtifactRef, TenantId};

/// **Leg 1+2 — the named green artifact: complete lineage per-viewer (0 leak) + cold-reindex == live.**
/// The whole Issues-side E2E-3 scenario is driven end-to-end; `is_green()` is the earned verdict.
#[test]
fn e2e_3_named_green_artifact_is_green() {
    let art = run_e2e_3_lineage();
    assert_eq!(
        art.scenario, E2E_LINEAGE_SCENARIO,
        "the E2E-3 scenario token"
    );
    assert!(art.is_green(), "E2E-3 must be green: {}", art.evidence);
    assert_eq!(
        art.leaks, 0,
        "0 title/count/backlink leak: {}",
        art.evidence
    );
    assert!(
        art.evidence.contains("lineage_complete=true")
            && art
                .evidence
                .contains("cold-reindex==live (2.6)=true (drift=0)"),
        "the artifact attests the complete lineage + the cold==live parity: {}",
        art.evidence
    );
    // The whole-wedge driver returns exactly the one Issues-side E2E-3 leg.
    let arts = run_issues_e2e_3();
    assert_eq!(arts.len(), 1);
    assert!(arts[0].is_green());
}

/// Build the deploy's audit-action envelope: an `agent.effect_applied` action whose `subject` is the
/// Issues lineage anchor (the PII-free initiative ref the lineage anchors on). This is the deploy that
/// ships the lineage — the audit chain records WHAT shipped (the ref), never a title/body.
fn deploy_audit_action(id: &str, subject: &ArtifactRef) -> EventEnvelope {
    let principal = Principal::stub(
        PrincipalId("u-deployer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    let region = principal.region.clone();
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("agent.effect_applied".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region,
        actor: Actor(principal),
        subject: subject.clone(),
        aggregate: AggregateKey("deploy:prod".into()),
        causation_id: None,
        correlation_id: CorrelationId("spec-to-ship".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

/// **Leg 3 — audit tamper detected (GA-D3): the lineage's deploy audit entry rides the REAL GDPR hash-
/// chain; a retroactive edit to the lineage entry BREAKS the chain.** The chain machinery is GDPR-owned
/// (`AuditAuthority` + `audit::verify_entries_for_test`) — Issues feeds its lineage anchor into the ONE
/// tamper-evidence frame (no second chain). The pristine chain verifies (the baseline — the detection is
/// not trivially-always-true); editing the lineage entry's `subject` flips the verifier to `false`.
#[test]
fn e2e_3_audit_tamper_on_the_lineage_anchor_is_detected() {
    let auth = AuditAuthority::new(CellSigningKey::from_seed("cell:fr-par:audit"));
    let tenant = TenantId("acme".into());

    // The lineage anchor the deploy entry carries (the PII-free initiative ref — Issues' E2E-3 anchor).
    let anchor = lineage_audit_anchor();
    // The anchor carries NO title/body (PII-free) — the audit chain records the ref, never the secret.
    assert!(
        !anchor.0.contains("SECRET") && !anchor.0.contains("weights"),
        "the lineage anchor must be PII-free"
    );

    // The deploy audit surface: a few prior actions, then the deploy that ships the lineage (its
    // `subject` is the lineage anchor) — driven through the audit consumer (the outbox-only write path).
    for i in 0..4 {
        let outcome = auth.consumer().handle(&deploy_audit_action(
            &format!("01J-pre-{i}"),
            &ArtifactRef(format!("myelin://acme/x/{i}")),
        ));
        assert_eq!(outcome, HandleOutcome::Done);
    }
    let lineage_entry_seq = auth.consumer().log().len_for(&tenant);
    let outcome = auth
        .consumer()
        .handle(&deploy_audit_action("01J-deploy-lineage", &anchor));
    assert_eq!(outcome, HandleOutcome::Done);
    // The chat go/no-go decision references the deploy (the lineage tail) — so the lineage entry is a
    // MIDDLE entry (a tail truncation alone leaves a dense chain; a middle deletion breaks the seq).
    let outcome = auth.consumer().handle(&deploy_audit_action(
        "01J-chat-gonogo",
        &ArtifactRef("myelin://acme/chat/thread/go-no-go".into()),
    ));
    assert_eq!(outcome, HandleOutcome::Done);

    // The pristine chain verifies intact (the baseline).
    assert!(
        auth.consumer().log().verify_chain(&tenant),
        "baseline: the lineage audit chain verifies intact"
    );
    let entries = auth.consumer().log().entries_for(&tenant);
    assert!(
        audit::verify_entries_for_test(&entries),
        "baseline: the explicit entry vector verifies"
    );
    // The lineage entry exists and carries the anchor as its subject.
    let lineage_entry = entries
        .iter()
        .find(|e| e.seq == lineage_entry_seq)
        .expect("the lineage deploy entry is in the chain");
    assert_eq!(
        lineage_entry.subject, anchor,
        "the deploy entry's subject is the Issues lineage anchor"
    );

    // ── THE TAMPER: a retroactive edit to the lineage entry's subject (a DB-level attack — the chain
    //    store is crate-private, so we model the tampered store the way a verifier reads it). ──
    let mut tampered = entries.clone();
    let idx = tampered
        .iter()
        .position(|e| e.seq == lineage_entry_seq)
        .unwrap();
    tampered[idx].subject = ArtifactRef("myelin://acme/issue/ENG-FORGED".into());

    // DETECTION — the hash-chain breaks (the recomputed leaf no longer matches; the chain link breaks
    // forward from the edited lineage entry). Tamper detected.
    assert!(
        !audit::verify_entries_for_test(&tampered),
        "GA-D3: a retroactive edit to the lineage audit anchor breaks the hash-chain (tamper detected)"
    );

    // A DELETED lineage entry is detected too (the seq sequence is no longer dense + the link breaks).
    let mut deleted = entries.clone();
    deleted.remove(idx);
    assert!(
        !audit::verify_entries_for_test(&deleted),
        "GA-D3: a deleted lineage audit entry breaks the chain (seq gap + link break)"
    );
}

/// **The audit anchor is STABLE across runs (the lineage records the SAME ref deterministically).** A
/// non-deterministic anchor would make the cold-reindex/audit correlation flaky — the anchor is a frozen
/// PII-free URN derived from the initiative key.
#[test]
fn e2e_3_audit_anchor_is_stable() {
    assert_eq!(
        lineage_audit_anchor(),
        lineage_audit_anchor(),
        "the lineage audit anchor is deterministic"
    );
}
