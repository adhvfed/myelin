//! # The agent-trace holder seam (8.8, H17) — distinct from the audit log (P-GA-26 → P-153)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§3.2 H17** (the agent
//! execution trace, AG-7 — *a content-addressed Knowledge doc of a run's trace*, crypto-shreddable,
//! **deliberately DISTINCT from the audit log**: trace = the run's reasoning record [erasable];
//! audit = the complete tamper-evident who-did-what) and **§6.5** (*three separate holders kept
//! separate on purpose — telemetry, agent execution trace [H17, erasable], audit log [H16, the
//! complete tamper-evident record]; an agent's APPLIED EFFECT lands in the audit log like a human's
//! action, its REASONING lands in its trace; keeping the three distinct means none weakens
//! another*). Prove-it: `external-insights/01-process-and-quality-doctrine.md` §3 — the
//! distinct-from-audit boundary is a committed **architecture test**, not a claim.
//!
//! **Contract-index:** wires row **8.8** (*AG-7 agent trace — Knowledge accepts a content-addressed
//! agent-trace write + registers it as an **erasable holder**; **distinct from the audit log***).
//! The PROVIDER half (the Knowledge content-addressed trace write + the H17 holder BODY) lands with
//! Knowledge in **M3 (P-GA-27 → P-256)**; this prompt wires the GDPR-side **SEAM** — the holder id +
//! its canonical erase phase + the distinct-from-audit boundary the orchestrator registers the H17
//! impl through.
//!
//! ## What "the seam" means here (the floor split — EI-01 §3 name-your-floors)
//! The agent SUBSYSTEM already registers its trace store as a [`myelin_substrate::Holder::H17AgentTrace`]
//! holder, distinct from the audit log, in `myelin-agent-service::holder` (AG-P3 / P-131; the store
//! is `agent_fabric_trace`, classified to H17). What did NOT yet exist is the **GDPR-orchestration
//! seam**: the point at which the DSR fan-out (P-GA-12 / [`crate::orchestration`]) registers the
//! H17 holder at its canonical erase phase and treats it as DISTINCT from the H16 audit carve-out.
//! This module ships exactly that seam:
//! 1. **[`AGENT_TRACE_HOLDER_ID`]** — the stable, PII-free holder id the orchestrator registers H17
//!    under (aligned to the agent subsystem's `agent_fabric_trace` store name — ONE name, EI-01 §7).
//! 2. **[`agent_trace_phase`]** — H17's canonical erase phase. The trace is a TRAILING derived copy
//!    (a run's reasoning record, crypto-shred-erasable) → [`CanonicalErasePhase::CachesAndDerivedCopies`]
//!    (alongside notif/agent-memory — §4.1 step 4), so the combined fan-out erases it after the
//!    pseudonym map + the per-subject DEK are already destroyed.
//! 3. **[`trace_is_distinct_from_audit`]** — the architecture-test predicate (trace ≠ audit): the
//!    H17 trace holder id is NOT the H16 audit carve-out holder id, and the trace's erase MECHANISM
//!    is **crypto-shred** (the trace IS erasable) while the audit carve-out is **retain** (never
//!    rewrite the chain). The two are kept separate so erasing a person's trace never touches the
//!    tamper-evident audit log (§6.5).
//! 4. **[`AgentTraceHolderSeam`]** — the registerable holder placeholder. Its op bodies are the
//!    **named floor**: they return a LOUD [`myelin_gdpr::DsrError`] (`H17 impl is M3 P-GA-27`) rather
//!    than silently no-op (EI-01 §3 — make a not-yet-built surface loud, never a false green). The
//!    REAL bodies (the content-addressed trace `locate`/`export`/`erase` over the Knowledge block
//!    model) land when Knowledge ships in **M3 (P-GA-27)**; at that point this seam's
//!    [`AGENT_TRACE_HOLDER_ID`] + [`agent_trace_phase`] are the registration coordinates the live
//!    impl plugs into — the registration shape does not change.
//!
//! ## Why a loud placeholder, not a silent stub (the honest floor)
//! A holder that silently returns a success receipt for an erase it did not perform is a FALSE
//! green — the DSR would report "erased" over a trace it never touched. So the seam's bodies return
//! an error naming the filling prompt; the orchestrator's checklist records the holder as
//! NOT-done (un-receipted) until the live impl is wired. This is the EI-01 §3 discipline: a floor is
//! NAMED + LOUD, never a hidden no-op. The H17 holder is REGISTERABLE (the seam exists, the phase is
//! declared, the distinctness is proven) but its erase is honestly deferred to P-GA-27.

use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::Holder;

use crate::holders::AUDIT_CARVE_OUT_STORE;
use crate::orchestration::CanonicalErasePhase;

/// The stable, PII-free holder id the DSR orchestrator registers **H17 (agent execution trace)**
/// under (gdpr §3.2 H17). It is the SAME name the agent subsystem registers its trace store under
/// (`myelin-agent-service`'s `AGENT_TRACE_STORE` = `agent_fabric_trace`) — ONE name across the seam
/// (EI-01 §7 coherence; no parallel second id). The live H17 impl (P-GA-27) registers through THIS
/// id at [`agent_trace_phase`].
pub const AGENT_TRACE_HOLDER_ID: &str = "agent_fabric_trace";

/// The M3 prompt that fills the H17 trace holder BODY (the content-addressed trace write + the DSR
/// fan-out over it). Named here so the floor's follow-on is in writing (VISION §3).
pub const AGENT_TRACE_IMPL_PROMPT: &str =
    "P-GA-27 (M3) — the Knowledge instance + the H17 trace holder body";

/// **The H17 canonical erase phase (§4.1 step 4).** The agent execution trace is a TRAILING derived
/// copy — a run's reasoning record, crypto-shred-erasable — so it erases in
/// [`CanonicalErasePhase::CachesAndDerivedCopies`] (alongside notif H13 / agent-memory H11), AFTER
/// the pseudonym map (phase 0) + the per-subject DEK (phase 1) are already destroyed. The combined
/// fan-out (P-GA-12) drives holders in phase order, so the trace's crypto-shred runs at the right
/// point in the canonical order.
pub fn agent_trace_phase() -> CanonicalErasePhase {
    CanonicalErasePhase::CachesAndDerivedCopies
}

/// **The distinct-from-audit boundary (gdpr §3.2 H17 / §6.5) — the architecture-test predicate.**
/// The H17 agent trace is DELIBERATELY distinct from the H16 audit log: this returns `true` iff the
/// trace holder id is NOT the audit carve-out store id AND their erase mechanisms differ (trace =
/// crypto-shred [erasable]; audit = retain [never rewrite the chain]). Keeping them separate means
/// erasing a person's agent trace never touches the tamper-evident audit log — neither weakens the
/// other (§6.5). [`tests::the_agent_trace_seam_is_distinct_from_the_audit_log`] is the committed
/// architecture test.
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

/// The pure distinctness predicate (the §6.5 boundary, factored so each conjunct is testable with a
/// FALSE input — a same-id, a same-H-number, or a same-erasability all collapse distinctness). The
/// trace is distinct from the audit log iff: (1) distinct holder ids, AND (2) distinct H-numbers,
/// AND (3) distinct erase semantics (the trace IS erasable; the audit log is NOT — the retain
/// carve-out). Every conjunct is load-bearing — drop any one and an erasure of a person's trace
/// could touch the tamper-evident audit log.
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
    // The trace IS erasable; the audit log is NOT (it is the retain carve-out — §6.4).
    let distinct_erasability = trace_erasable && !audit_erasable;
    distinct_ids && distinct_holders && distinct_erasability
}

/// The H17 trace IS erasable — a run's reasoning record, crypto-shredded on a DSR (§3.2 H17 / §6.5).
pub const AGENT_TRACE_ERASABLE: bool = true;

/// The H16 audit log is NOT freely erasable — it is a carve-out (retain the minimised pseudonym
/// record; never rewrite the chain; expire via audit-key crypto-shred at retention end — §6.4). The
/// distinctness predicate keys on this asymmetry.
pub const AUDIT_LOG_ERASABLE: bool = false;

/// **The registerable H17 agent-trace holder SEAM (the named-floor placeholder body).** The seam
/// exists so the DSR orchestrator can register H17 at [`agent_trace_phase`] under
/// [`AGENT_TRACE_HOLDER_ID`] — the registration coordinates the live M3 impl (P-GA-27) plugs into.
/// Its op bodies are the **honest floor**: they return a LOUD error naming the filling prompt rather
/// than silently no-op (a silent success would be a FALSE green — a DSR reporting "trace erased"
/// over a trace it never touched). The orchestrator's checklist records H17 as NOT-done until the
/// live impl is wired.
///
/// This is the H17 face of the §3.1 holder contract: it implements the five-op
/// [`PersonalDataHolder`] surface (so it is a registerable holder) but every op is the deferred
/// floor. It is **distinct from the H16 audit carve-out** ([`crate::holders::AuditCarveOutHolder`])
/// by construction (a different holder id, a different erase mechanism).
#[derive(Clone, Copy, Debug, Default)]
pub struct AgentTraceHolderSeam;

impl AgentTraceHolderSeam {
    /// The seam (the named-floor H17 placeholder).
    pub fn new() -> AgentTraceHolderSeam {
        AgentTraceHolderSeam
    }

    /// The PII-free holder id this seam registers under ([`AGENT_TRACE_HOLDER_ID`]).
    pub fn holder_id(&self) -> &'static str {
        AGENT_TRACE_HOLDER_ID
    }

    /// The loud "not yet built" error every op returns (naming the filling prompt — EI-01 §3). A
    /// floor is named + loud, never a hidden no-op.
    fn deferred(op: &str) -> DsrError {
        DsrError(format!(
            "H17 agent-trace `{op}` is the M3 floor — the content-addressed trace holder body lands in {AGENT_TRACE_IMPL_PROMPT}"
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
        // The trace IS erasable (crypto-shred) — but the BODY is the M3 floor. Loud, never a silent
        // success (a false "erased" receipt would be a coverage lie — §3 prove-it).
        Err(AgentTraceHolderSeam::deferred("erase"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The H17 agent-trace seam is DISTINCT from the audit log (the architecture test — gdpr §3.2
    /// H17 / §6.5).** Different holder id, different H-number, different erase mechanism (trace =
    /// erasable crypto-shred; audit = retain carve-out). This is the §6.5 "kept separate on purpose"
    /// guarantee: erasing a person's trace never touches the tamper-evident audit log.
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
        // The asymmetry the distinctness keys on: the trace is erasable; the audit log is a
        // carve-out. The two erasability flags MUST differ (the trace IS erasable, the audit log
        // is NOT) — comparing them is the load-bearing assertion (not a tautology on either alone).
        assert_ne!(
            AGENT_TRACE_ERASABLE, AUDIT_LOG_ERASABLE,
            "the trace is erasable; the audit log is the retain carve-out — distinct mechanisms"
        );
        // The polarity matters: trace = erasable, audit = NOT (the carve-out). Routed through
        // black_box so the check is a real runtime assertion, not a const-folded tautology.
        assert!(
            core::hint::black_box(AGENT_TRACE_ERASABLE),
            "the trace is erasable"
        );
        assert!(
            !core::hint::black_box(AUDIT_LOG_ERASABLE),
            "the audit log is the retain carve-out"
        );
    }

    /// **Each distinctness conjunct is load-bearing (mutation-core).** The pure predicate returns
    /// `false` if ANY single conjunct is falsified — a same id, a same H-number, the trace made
    /// non-erasable, OR the audit log made erasable. This kills the `-> true` and `&& -> ||` mutants
    /// (an `||` would wrongly admit distinctness on a single satisfied conjunct).
    #[test]
    fn each_distinctness_conjunct_is_load_bearing() {
        // The honest case holds.
        assert!(distinctness_holds(
            "trace", "audit", "H17", "H16", true, false
        ));
        // Falsify conjunct 1 (same id) → not distinct.
        assert!(!distinctness_holds(
            "same", "same", "H17", "H16", true, false
        ));
        // Falsify conjunct 2 (same H-number) → not distinct.
        assert!(!distinctness_holds(
            "trace", "audit", "H16", "H16", true, false
        ));
        // Falsify conjunct 3a (trace not erasable) → not distinct.
        assert!(!distinctness_holds(
            "trace", "audit", "H17", "H16", false, false
        ));
        // Falsify conjunct 3b (audit erasable) → not distinct.
        assert!(!distinctness_holds(
            "trace", "audit", "H17", "H16", true, true
        ));
    }

    /// The trace HOLDER ID is the SAME name the agent subsystem registers its trace store under (ONE
    /// name across the seam — EI-01 §7). If the agent crate ever renames `agent_fabric_trace` this
    /// test is the loud co-edit reminder. (We assert the literal so a drift is caught here too.)
    #[test]
    fn the_holder_id_matches_the_agent_subsystem_store_name() {
        assert_eq!(AGENT_TRACE_HOLDER_ID, "agent_fabric_trace");
    }

    /// **H17 erases at the trailing derived-copy phase (§4.1 step 4)** — after the pseudonym map +
    /// the per-subject DEK are destroyed.
    #[test]
    fn the_agent_trace_phase_is_a_trailing_derived_copy() {
        assert_eq!(
            agent_trace_phase(),
            CanonicalErasePhase::CachesAndDerivedCopies
        );
        // It is AFTER the identity pseudonym-map (phase 0) and the per-subject DEK shred (phase 1).
        assert!(agent_trace_phase() > CanonicalErasePhase::IdentityPseudonymMap);
        assert!(agent_trace_phase() > CanonicalErasePhase::CryptoShredDek);
    }

    /// **The seam's bodies are the LOUD named floor (EI-01 §3) — never a silent false green.** Every
    /// op returns an error naming the M3 filling prompt; the orchestrator records H17 as NOT-done.
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

        // erase returns a LOUD error naming P-GA-27 (never a silent success receipt).
        let err = seam
            .erase(EraseScope::Subject {
                subject: subject.clone(),
                tenant: tenant.clone(),
            })
            .expect_err("the H17 erase body is the M3 floor — loud, not a silent green");
        assert!(
            err.0.contains("P-GA-27"),
            "the floor names its filling prompt: {}",
            err.0
        );
        assert!(err.0.contains("H17"), "the error names the holder");

        // locate / export likewise defer loudly.
        assert!(seam.locate(&subject, tenant.clone()).is_err());
        assert!(seam.export(&subject, tenant).is_err());
        assert_eq!(seam.holder_id(), AGENT_TRACE_HOLDER_ID);
    }

    /// The filling-prompt constant names the M3 follow-on (the floor is in writing — VISION §3).
    #[test]
    fn the_impl_floor_names_its_follow_on() {
        assert!(AGENT_TRACE_IMPL_PROMPT.contains("P-GA-27"));
    }
}
