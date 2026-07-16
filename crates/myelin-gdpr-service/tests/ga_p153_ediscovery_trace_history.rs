//! # GATE / DRILL — P-GA-26 (→ P-153): eDiscovery inclusion proof + trace≠audit + history-rewrite skeleton
//!
//! **DATED GREEN ARTIFACT (2026-06-20).** This drill is the dated green artifact the P-GA-26 GATE
//! requires (the GDPR prompts record their drill artifacts as the test itself — there is no separate
//! scorecard file; the green is a passing, committed test, EI-01 §3 prove-it). It proves the three
//! GATE rows the scorecard names ("eDiscovery inclusion proof; trace≠audit"):
//!
//! 1. **eDiscovery export carries an inclusion proof against the Merkle tree + is legal-hold-frozen**
//!    (gdpr §5.4) — a subject-scoped export verifies record-by-record against the per-tenant audit
//!    tree's signed tree head, and the scope is frozen by a legal hold so a concurrent erase defers.
//! 2. **The agent-trace H17 seam is distinct from the audit log** (gdpr §3.2 H17 / §6.5) — different
//!    holder id, different H-number, different erase mechanism (trace = erasable; audit = retain
//!    carve-out).
//! 3. **The history-rewrite skeleton is a resumable, idempotent activity** (gdpr §6.6) — a crash
//!    mid-drive re-runs ONLY the un-receipted phases; the invalidation fan-out is the named M5 floor.

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_gdpr::{EraseScope, SubjectRef};
use myelin_gdpr_service::{
    agent_trace_phase, trace_is_distinct_from_audit, verify_inclusion, AgentTraceHolderSeam,
    AuditAuthority, CellSigningKey, DsrKind, EDiscoveryExporter, EDiscoveryScope,
    HistoryRewriteActivity, HistoryRewriteRequest, HoldVerdict, LegalHoldRegistry, RewritePhase,
    AGENT_TRACE_HOLDER_ID,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{ArtifactRef, TenantId};

fn action(id: &str, tenant: &str, subject: &str) -> EventEnvelope {
    let principal = Principal::stub(
        PrincipalId(format!("u-{id}")),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    let region = principal.region.clone();
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("iam.tuple_written".into()),
        schema_ver: 1,
        tenant: TenantId(tenant.into()),
        region,
        actor: Actor(principal),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey("agg:1".into()),
        causation_id: None,
        correlation_id: CorrelationId("r".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

/// GATE row 1 — **eDiscovery export is inclusion-proof-bearing + legal-hold-frozen** (gdpr §5.4).
#[test]
fn gate_ediscovery_export_is_inclusion_proof_bearing_and_legal_hold_frozen() {
    let authority = AuditAuthority::new(CellSigningKey::from_seed("cell:fr-par:audit-key"));
    let holds = LegalHoldRegistry::new();
    authority.consumer().handle(&action("1", "acme", "u-A"), &mut myelin_events::HandlerTx::none());
    authority.consumer().handle(&action("2", "acme", "u-B"), &mut myelin_events::HandlerTx::none());
    authority.consumer().handle(&action("3", "acme", "u-A"), &mut myelin_events::HandlerTx::none());

    let exporter = EDiscoveryExporter::new(&authority, &holds);
    let scope = EDiscoveryScope::Subject {
        tenant: TenantId("acme".into()),
        subject: ArtifactRef("u-A".into()),
    };
    let bundle = exporter
        .export(&scope, "2026-06-20T01:00:00Z")
        .expect("export");

    // INCLUSION PROOF: every record verifies against the bundle STH (the Merkle tree root).
    assert!(
        bundle.verify(authority.key()),
        "the bundle verifies end-to-end"
    );
    for r in &bundle.records {
        assert!(
            verify_inclusion(&r.inclusion, &bundle.sth),
            "record {} is Merkle-proven",
            r.seq
        );
    }
    assert_eq!(bundle.record_count(), 2, "both u-A records");

    // LEGAL-HOLD-FROZEN: a concurrent erase over the scope is now deferred.
    assert!(bundle.legal_hold_frozen);
    let principal = Principal::stub(
        PrincipalId("u-A".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    let erase = EraseScope::Subject {
        subject: SubjectRef::new(principal),
        tenant: TenantId("acme".into()),
    };
    assert_eq!(
        holds.verdict(DsrKind::Erasure, &erase),
        HoldVerdict::Deferred,
        "the scope is frozen"
    );
}

/// GATE row 2 — **the agent-trace H17 seam is distinct from the audit log** (gdpr §3.2 H17 / §6.5).
#[test]
fn gate_agent_trace_seam_is_distinct_from_the_audit_log() {
    assert!(trace_is_distinct_from_audit(), "trace ≠ audit");
    let seam = AgentTraceHolderSeam::new();
    assert_eq!(seam.holder_id(), AGENT_TRACE_HOLDER_ID);
    // It is a trailing derived-copy holder (erased after identity + per-subject DEK).
    assert!(agent_trace_phase() > myelin_gdpr_service::CanonicalErasePhase::CryptoShredDek);
}

/// GATE row 3 — **the history-rewrite skeleton is a resumable, idempotent activity** (gdpr §6.6).
#[test]
fn gate_history_rewrite_skeleton_is_a_resumable_idempotent_activity() {
    let activity = HistoryRewriteActivity::new();
    let request = HistoryRewriteRequest {
        tenant: TenantId("acme".into()),
        repo: ArtifactRef("myelin://acme/git/repo-1".into()),
        actor_pseudonym: "p-7@acme.noreply".into(),
        rewrite_spec: "filter-repo:remove-blob:b-1".into(),
    };

    let first = activity.drive(&request);
    assert!(first.skeleton_complete(), "every phase checkpointed");
    assert_eq!(
        first.action, "git.history_rewrite",
        "audited as git.history_rewrite"
    );

    // Crash mid-drive losing phase 2+ → re-drive re-runs ONLY the un-receipted phases.
    activity.simulate_crash_losing(RewritePhase::CryptoShredPackTier);
    activity.drive(&request);
    assert_eq!(
        activity.phase_call_count(RewritePhase::Audit),
        1,
        "phase 0 survived"
    );
    assert_eq!(
        activity.phase_call_count(RewritePhase::CryptoShredPackTier),
        2,
        "phase 2 re-ran once"
    );

    // P-451 (P-GA-35) PROMOTED the invalidation fan-out from the M5 floor to a live mechanism — no
    // phase is a deferred floor on the first-class op; the residual is still named, not pretended-
    // solved, and now names the outbound push-mirror residency gate (GA-11, P-GA-36).
    let invalidate = first
        .phase_receipts
        .iter()
        .find(|r| r.phase == RewritePhase::InvalidateCaches)
        .unwrap();
    assert!(
        !invalidate.deferred_floor,
        "the invalidation fan-out is LIVE on the M5 first-class op (no longer a floor)"
    );
    assert!(
        first.residual_named.contains("P-GA-36"),
        "the residual names the outbound push-mirror residency gate follow-on"
    );
}
