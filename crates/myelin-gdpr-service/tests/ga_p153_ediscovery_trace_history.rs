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
        type_: EventType("identity.tuple.written".into()),
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

#[test]
fn gate_agent_trace_seam_is_distinct_from_the_audit_log() {
    assert!(trace_is_distinct_from_audit(), "trace ≠ audit");
    let seam = AgentTraceHolderSeam::new();
    assert_eq!(seam.holder_id(), AGENT_TRACE_HOLDER_ID);
    assert!(agent_trace_phase() > myelin_gdpr_service::CanonicalErasePhase::CryptoShredDek);
}

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
