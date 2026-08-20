use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::Holder;

use crate::holders::AUDIT_CARVE_OUT_STORE;
use crate::orchestration::CanonicalErasePhase;

pub const AGENT_TRACE_HOLDER_ID: &str = "agent_fabric_trace";

pub const AGENT_TRACE_IMPL_PROMPT: &str =
    "P-GA-27 (M3) - the Knowledge instance + the H17 trace holder body";

pub fn agent_trace_phase() -> CanonicalErasePhase {
    CanonicalErasePhase::CachesAndDerivedCopies
}

pub fn trace_is_distinct_from_audit() -> bool {
    distinctness_holds(
        AGENT_TRACE_HOLDER_ID,
        AUDIT_CARVE_OUT_STORE,
        Holder::H17AgentTrace.tag(),
        Holder::H16AuditLog.tag(),
        AGENT_TRACE_ERASABLE,
        AUDIT_LOG_ERASABLE,
    )
}

fn distinctness_holds(
    trace_id: &str,
    audit_id: &str,
    trace_h: &str,
    audit_h: &str,
    trace_erasable: bool,
    audit_erasable: bool,
) -> bool {
    let distinct_ids = trace_id != audit_id;
    let distinct_holders = trace_h != audit_h;
    let distinct_erasability = trace_erasable && !audit_erasable;
    distinct_ids && distinct_holders && distinct_erasability
}

pub const AGENT_TRACE_ERASABLE: bool = true;

pub const AUDIT_LOG_ERASABLE: bool = false;

#[derive(Clone, Copy, Debug, Default)]
pub struct AgentTraceHolderSeam;

impl AgentTraceHolderSeam {
    pub fn new() -> AgentTraceHolderSeam {
        AgentTraceHolderSeam
    }

    pub fn holder_id(&self) -> &'static str {
        AGENT_TRACE_HOLDER_ID
    }

    fn deferred(op: &str) -> DsrError {
        DsrError(format!(
            "H17 agent-trace `{op}` is the M3 floor - the content-addressed trace holder body lands in {AGENT_TRACE_IMPL_PROMPT}"
        ))
    }
}

impl PersonalDataHolder for AgentTraceHolderSeam {
    fn locate(&self, _subject: &SubjectRef, _tenant: TenantId) -> DsrResult<LocateReport> {
        Err(AgentTraceHolderSeam::deferred("locate"))
    }

    fn export(&self, _subject: &SubjectRef, _tenant: TenantId) -> DsrResult<PortableBundle> {
        Err(AgentTraceHolderSeam::deferred("export"))
    }

    fn rectify(&self, _subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Err(AgentTraceHolderSeam::deferred("rectify"))
    }

    fn restrict(&self, _subject: &SubjectRef, _on: bool) -> DsrResult<RestrictReceipt> {
        Err(AgentTraceHolderSeam::deferred("restrict"))
    }

    fn erase(&self, _scope: EraseScope) -> DsrResult<EraseReceipt> {
        Err(AgentTraceHolderSeam::deferred("erase"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_trace_seam_is_distinct_from_the_audit_log() {
        assert!(
            trace_is_distinct_from_audit(),
            "H17 trace is distinct from H16 audit"
        );
        assert_ne!(
            AGENT_TRACE_HOLDER_ID, AUDIT_CARVE_OUT_STORE,
            "the trace store id is not the audit carve-out store id"
        );
        assert_ne!(
            Holder::H17AgentTrace.tag(),
            Holder::H16AuditLog.tag(),
            "H17 ≠ H16"
        );
        assert_ne!(
            AGENT_TRACE_ERASABLE, AUDIT_LOG_ERASABLE,
            "the trace is erasable; the audit log is the retain carve-out - distinct mechanisms"
        );
        assert!(
            core::hint::black_box(AGENT_TRACE_ERASABLE),
            "the trace is erasable"
        );
        assert!(
            !core::hint::black_box(AUDIT_LOG_ERASABLE),
            "the audit log is the retain carve-out"
        );
    }

    #[test]
    fn each_distinctness_conjunct_is_load_bearing() {
        assert!(distinctness_holds(
            "trace", "audit", "H17", "H16", true, false
        ));
        assert!(!distinctness_holds(
            "same", "same", "H17", "H16", true, false
        ));
        assert!(!distinctness_holds(
            "trace", "audit", "H16", "H16", true, false
        ));
        assert!(!distinctness_holds(
            "trace", "audit", "H17", "H16", false, false
        ));
        assert!(!distinctness_holds(
            "trace", "audit", "H17", "H16", true, true
        ));
    }

    #[test]
    fn the_holder_id_matches_the_agent_subsystem_store_name() {
        assert_eq!(AGENT_TRACE_HOLDER_ID, "agent_fabric_trace");
    }

    #[test]
    fn the_agent_trace_phase_is_a_trailing_derived_copy() {
        assert_eq!(
            agent_trace_phase(),
            CanonicalErasePhase::CachesAndDerivedCopies
        );
        assert!(agent_trace_phase() > CanonicalErasePhase::IdentityPseudonymMap);
        assert!(agent_trace_phase() > CanonicalErasePhase::CryptoShredDek);
    }

    #[test]
    fn the_seam_bodies_are_a_loud_named_floor_not_a_silent_stub() {
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        use myelin_tenancy::TenantId as TyTenantId;
        let seam = AgentTraceHolderSeam::new();
        let principal = Principal::stub(
            PrincipalId("u-1".into()),
            PrincipalKind::Human,
            TyTenantId("acme".into()),
        );
        let subject = SubjectRef::new(principal);
        let tenant = TyTenantId("acme".into());

        let err = seam
            .erase(EraseScope::Subject {
                subject: subject.clone(),
                tenant: tenant.clone(),
            })
            .expect_err("the H17 erase body is the M3 floor - loud, not a silent green");
        assert!(
            err.0.contains("P-GA-27"),
            "the floor names its filling prompt: {}",
            err.0
        );
        assert!(err.0.contains("H17"), "the error names the holder");

        assert!(seam.locate(&subject, tenant.clone()).is_err());
        assert!(seam.export(&subject, tenant).is_err());
        assert_eq!(seam.holder_id(), AGENT_TRACE_HOLDER_ID);
    }
}
