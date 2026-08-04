use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef};
use myelin_gdpr_service::{
    agent_trace_phase, trace_is_distinct_from_audit, AgentTraceHolderSeam, CanonicalErasePhase,
    AGENT_TRACE_ERASABLE, AGENT_TRACE_HOLDER_ID, AGENT_TRACE_IMPL_PROMPT, AUDIT_LOG_ERASABLE,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;

#[test]
fn provider_seam_is_a_registerable_holder_distinct_from_the_audit_log() {
    let seam = AgentTraceHolderSeam::new();

    assert_eq!(
        seam.holder_id(),
        AGENT_TRACE_HOLDER_ID,
        "the orchestrator registers H17 under this id"
    );
    assert_eq!(
        agent_trace_phase(),
        CanonicalErasePhase::CachesAndDerivedCopies,
        "H17 erases as a trailing derived copy (after the pseudonym map + per-subject DEK)"
    );

    assert!(
        trace_is_distinct_from_audit(),
        "H17 trace is distinct from the H16 audit log"
    );
    assert_ne!(
        AGENT_TRACE_ERASABLE, AUDIT_LOG_ERASABLE,
        "the trace is erasable; the audit log is the retain carve-out - distinct mechanisms"
    );
}

#[test]
fn the_consumer_sees_a_loud_named_floor_not_a_silent_false_green() {
    let seam = AgentTraceHolderSeam::new();
    let principal = Principal::stub(
        PrincipalId("u-1".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    let subject = SubjectRef::new(principal);
    let tenant = TenantId("acme".into());

    let err = seam
        .erase(EraseScope::Subject {
            subject: subject.clone(),
            tenant: tenant.clone(),
        })
        .expect_err("the H17 erase body is the M3 floor - loud, never a silent false green");
    assert!(
        err.0.contains("P-GA-27"),
        "the floor names its filling prompt: {}",
        err.0
    );

    assert!(
        seam.locate(&subject, tenant.clone()).is_err(),
        "locate defers loudly"
    );
    assert!(
        seam.export(&subject, tenant).is_err(),
        "export defers loudly"
    );
    assert!(
        AGENT_TRACE_IMPL_PROMPT.contains("P-GA-27"),
        "the impl floor names P-GA-27"
    );
}
