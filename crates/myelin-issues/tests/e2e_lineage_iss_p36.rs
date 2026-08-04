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
    let arts = run_issues_e2e_3();
    assert_eq!(arts.len(), 1);
    assert!(arts[0].is_green());
}

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

#[test]
fn e2e_3_audit_tamper_on_the_lineage_anchor_is_detected() {
    let auth = AuditAuthority::new(CellSigningKey::from_seed("cell:fr-par:audit"));
    let tenant = TenantId("acme".into());

    let anchor = lineage_audit_anchor();
    assert!(
        !anchor.0.contains("SECRET") && !anchor.0.contains("weights"),
        "the lineage anchor must be PII-free"
    );

    for i in 0..4 {
        let outcome = auth.consumer().handle(&deploy_audit_action(
            &format!("01J-pre-{i}"),
            &ArtifactRef(format!("myelin://acme/x/{i}")),
        ), &mut myelin_events::HandlerTx::none());
        assert_eq!(outcome, HandleOutcome::Done);
    }
    let lineage_entry_seq = auth.consumer().log().len_for(&tenant);
    let outcome = auth
        .consumer()
        .handle(&deploy_audit_action("01J-deploy-lineage", &anchor), &mut myelin_events::HandlerTx::none());
    assert_eq!(outcome, HandleOutcome::Done);
    let outcome = auth.consumer().handle(&deploy_audit_action(
        "01J-chat-gonogo",
        &ArtifactRef("myelin://acme/chat/thread/go-no-go".into()),
    ), &mut myelin_events::HandlerTx::none());
    assert_eq!(outcome, HandleOutcome::Done);

    assert!(
        auth.consumer().log().verify_chain(&tenant),
        "baseline: the lineage audit chain verifies intact"
    );
    let entries = auth.consumer().log().entries_for(&tenant);
    assert!(
        audit::verify_entries_for_test(&entries),
        "baseline: the explicit entry vector verifies"
    );
    let lineage_entry = entries
        .iter()
        .find(|e| e.seq == lineage_entry_seq)
        .expect("the lineage deploy entry is in the chain");
    assert_eq!(
        lineage_entry.subject, anchor,
        "the deploy entry's subject is the Issues lineage anchor"
    );

    let mut tampered = entries.clone();
    let idx = tampered
        .iter()
        .position(|e| e.seq == lineage_entry_seq)
        .unwrap();
    tampered[idx].subject = ArtifactRef("myelin://acme/issue/ENG-FORGED".into());

    assert!(
        !audit::verify_entries_for_test(&tampered),
        "GA-D3: a retroactive edit to the lineage audit anchor breaks the hash-chain (tamper detected)"
    );

    let mut deleted = entries.clone();
    deleted.remove(idx);
    assert!(
        !audit::verify_entries_for_test(&deleted),
        "GA-D3: a deleted lineage audit entry breaks the chain (seq gap + link break)"
    );
}

#[test]
fn e2e_3_audit_anchor_is_stable() {
    assert_eq!(
        lineage_audit_anchor(),
        lineage_audit_anchor(),
        "the lineage audit anchor is deterministic"
    );
}
