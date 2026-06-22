//! # CDC 8.8 — the AG-7 agent-trace holder seam (P-GA-26 → P-153)
//!
//! **Contract:** index row 8.8 — *AG-7 agent trace: Knowledge accepts a content-addressed
//! agent-trace write + registers it as an **erasable holder**; **distinct from the audit log***
//! (gdpr §3.2 H17 / §6.5). The PROVIDER (the content-addressed trace WRITE + the H17 holder BODY)
//! lands with Knowledge in M3 (P-GA-27); this pair proves the GDPR-side **SEAM** the M3 impl plugs
//! into — the holder id, the canonical erase phase, and the distinct-from-audit boundary.
//!
//! The contract-coverage scanner (P-S21) reads BOTH halves of the pair from this file:
//! - **provider** = `myelin_gdpr_service::AgentTraceHolderSeam` — the GDPR-orchestration seam that
//!   registers H17 (the agent execution trace) as a `PersonalDataHolder` distinct from the H16 audit
//!   carve-out. On this floor its op bodies are the LOUD named deferral to P-GA-27 (the live trace
//!   write/erase over the Knowledge block model); the seam coordinates (id, phase, distinctness) are
//!   real now.
//! - **consumer** = the DSR ORCHESTRATOR (the fan-out, P-GA-12) — it registers H17 through the seam's
//!   id + phase and treats it as DISTINCT from the audit log. It verifies the seam is a registerable
//!   holder, that the trace is erasable while the audit log is the retain carve-out, and that the
//!   not-yet-built body fails LOUDLY (never a silent false "erased") so the checklist records H17 as
//!   un-receipted until P-GA-27 wires the live impl.
//!
//! The dated green artifact: the seam is a registerable holder distinct from the audit log (the
//! architecture test passes), and its op bodies are a loud named floor pointing at P-GA-27.

use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef};
use myelin_gdpr_service::{
    agent_trace_phase, trace_is_distinct_from_audit, AgentTraceHolderSeam, CanonicalErasePhase,
    AGENT_TRACE_ERASABLE, AGENT_TRACE_HOLDER_ID, AGENT_TRACE_IMPL_PROMPT, AUDIT_LOG_ERASABLE,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;

#[test]
fn provider_seam_is_a_registerable_holder_distinct_from_the_audit_log() {
    // PROVIDER: the GDPR-side H17 agent-trace seam.
    let seam = AgentTraceHolderSeam::new();

    // CONSUMER (the DSR orchestrator): it registers H17 through the seam's id + phase.
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

    // The distinct-from-audit boundary (the §6.5 architecture-test face): trace ≠ audit.
    assert!(
        trace_is_distinct_from_audit(),
        "H17 trace is distinct from the H16 audit log"
    );
    assert_ne!(
        AGENT_TRACE_ERASABLE, AUDIT_LOG_ERASABLE,
        "the trace is erasable; the audit log is the retain carve-out — distinct mechanisms"
    );
}

#[test]
fn the_consumer_sees_a_loud_named_floor_not_a_silent_false_green() {
    // CONSUMER: the orchestrator drives the seam's holder ops; the not-yet-built body fails LOUDLY
    // (so the checklist records H17 as un-receipted) rather than silently returning a false "erased".
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
        .expect_err("the H17 erase body is the M3 floor — loud, never a silent false green");
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
