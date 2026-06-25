//! The Agent-Fabric stores AS `PersonalDataHolder`s — the REGISTRATION seam + the harness
//! auto-registration (AG-P3 / P-132; contract 1.4 + 10.1).
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/agent-fabric.md`
//! §4 (every table is a `PersonalDataHolder`, residency-pinned, crypto-shred-capable —
//! "`(tenant, region)` first … per-tenant envelope-encrypted, crypto-shred-capable,
//! `PersonalDataHolder` (ADR-11/12; contracts 1.4/10.1)"), §4.5 (the `trace` is a distinct,
//! content-addressed erasable holder; `run.trace_ref` is its `ArtifactRef`), §3 (the structural
//! trace-erasure floor: per-subject DEK crypto-shred + pseudonym shred → the full DSR fan-out is
//! the M5 follow-on AG-P23).
//!
//! **Contract-index:** rows **1.4** (`PersonalDataHolder` auto-registration on every store opened —
//! the harness's ONE door, `HolderRegistry::open`), **10.1** (the holder trait
//! `{locate, export, rectify, restrict, erase}`), **1.6** (the `no-untagged-personal-data` lint the
//! tagged schema in [`crate::schema`] already passes). Implemented to the frozen shapes.
//!
//! **VISION §3** (GDPR-safe by construction + name-your-floors). **EI-04 §1** (the subject id is the
//! opaque pseudonymous Principal id — never a raw name/email). **EI-01 §5** (the holder-registered +
//! holder-completeness assertions are loud committed gates) / **§7** (REUSE the substrate registry —
//! no parallel second holder mechanism; this mirrors `myelin-refs-service::holder` /
//! `myelin-search::holder`, the M1 registration pattern).
//!
//! ## The two Agent-Fabric holders (the §3.2 mapping)
//! The Fabric opens two logical store classes; each maps to one of the exhaustive H1–H18 holders
//! (gdpr §3.2, the canonical table in `gdpr-and-audit.md` §5):
//! 1. the **Fabric OLTP schema** — the `(tenant, region)`-first `run` / `tool_def` /
//!    `proposed_effect` / `hitl_gate` tables (AG-P2 / P-131, [`crate::schema`]); its H-holder is
//!    **H11 (`AgentMemory`)** — the agent's operational/working state (the run record, the proposed
//!    effects, the gate state). Declared here through the substrate [`StoreClassifier`] (an OLTP
//!    store needs a per-store holder declaration, gdpr §3.2);
//! 2. the **trace store** — the content-addressed execution-trace + the per-run conversation history
//!    the run owns ([`crate::schema::TraceRow`], `run.trace_ref` its `ArtifactRef`); its H-holder is
//!    **H17 (`AgentTrace`)**, deliberately DISTINCT from the audit log (AG-7; gdpr §6.5). Also an
//!    OLTP-class pointer store, so it too is declared in the classifier.
//!
//! ## Why a REGISTRATION SEAM now (the named floor — AG-P3 ships registration, AG-P23 ships bodies)
//! AG-P3 ships the **registration seam ONLY**: every Fabric store is opened through the substrate
//! [`HolderRegistry`] (the ONE door — opening IS registering, contract 1.4) so it is a registered
//! holder by construction, AND the no-untagged-personal-data lint passes over the tagged schema. The
//! holder is therefore **registered + classified + callable**, but its bodies are
//! **registered-no-op-with-named-follow-on** (NOT a silent gap — VISION §3): `locate`/`export`
//! return **empty-but-correct** content-addressed receipts and `erase`/`rectify`/`restrict` are
//! well-defined no-ops that NAME their AG-P23 follow-on in the receipt outcome. The structural
//! erasure LEVER already exists (the per-subject DEK + the `Pseudonymise`/`CryptoShred(subject_dek)`
//! tags on the schema, AG-P2); the **full DSR fan-out** across all Fabric holders (the run table,
//! the trace, agent memory) — `locate` → rows/trace naming the subject; `erase` → crypto-shred the
//! per-subject DEK + pseudonym-shred the attribution edges + tombstone (drill AG-D10) — is the M5
//! follow-on **AG-P23 (→ P-479)**. The trace HOLDER BODY (the content-addressed write into
//! Knowledge + its erasure) lands with Knowledge in M3 (**AG-P19 → P-268**, KN-D11/KN-D12). The
//! point of registering NOW: the M5 DSAR fan-out cannot silently miss the Agent Fabric (10.1
//! exhaustiveness).
//!
//! ## RECONCILED at AG-P23 (→ P-479): the holder BODIES now exist ([`crate::dsr`])
//! The AG-P3 floor named here is FILLED: [`crate::dsr::AgentFabricHolder`] ships the REAL
//! locate/export/erase bodies — `erase(subject)` crypto-shreds the per-subject DEK (the free-text
//! `input_payload`/`risk_summary`/`trace_body` becomes unrecoverable live + in backups), tombstones
//! the run rows, pseudonymises the attribution edges (attribution → opaque pseudonym, 4.8), and
//! drives the post-restore re-erasure ([`crate::dsr::FabricErasureLedger`]). The agent long-term
//! memory/RAG leg remains the named seam (v1 stateless except the trace — AG-P25). The
//! [`AgentOltpHolder`] / [`AgentTraceHolder`] structs HERE stay the REGISTRATION seam (opening IS
//! registering, contract 1.4) — their empty-but-correct receipts are the registration witness; the
//! erasure WORK is in [`crate::dsr`] (one mechanism, no parallel second holder — EI-01 §7).

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreKind};

/// The stable, PII-free name of the Agent-Fabric **OLTP schema** store (the `run` / `tool_def` /
/// `proposed_effect` / `hitl_gate` tables — the holder's **H11** store). Frozen here so the AG-P2
/// migrations, the data-map (P-GA-09), and the DSR fan-out (AG-P23) all address exactly this store.
/// PII-free: a store identifier, never personal data.
pub const AGENT_OLTP_STORE: &str = "agent_fabric_oltp";

/// The stable, PII-free name of the Agent-Fabric **trace** store (the content-addressed
/// execution-trace + the per-run conversation history — the holder's **H17** store). Frozen here so
/// the AG-P19 content-addressed trace write + the DSR fan-out (AG-P23) address exactly this store.
/// PII-free: a store identifier, never personal data.
pub const AGENT_TRACE_STORE: &str = "agent_fabric_trace";

/// The typed receipt that an Agent-Fabric store was auto-registered as a [`PersonalDataHolder`] —
/// the proof the registration fired for a given store (mirrors `myelin_substrate::HolderRegistration`,
/// the substrate-side receipt). The harness collects these; the holder-registered architecture test
/// reads them to assert no Fabric store escaped registration. PII-free: a (kind, name) tag.
pub type AgentHolderRegistration = HolderRegistration;

/// Build the Agent-Fabric [`StoreClassifier`] — the data-map declaration that the Fabric OLTP store
/// belongs to holder **H11 (`AgentMemory`)** and the trace store to **H17 (`AgentTrace`)** (gdpr
/// §3.2 / §5). Both are OLTP-class stores, so each needs a per-store declaration here (the three
/// non-OLTP kinds blob/cache/search classify structurally; the Fabric opens neither at AG-P2/P3).
/// The substrate completeness assertion joins the harness's [`HolderRegistry`] against this
/// classifier: every opened Fabric store must map to an H-holder, or it is an orphan (contract 1.4 +
/// gdpr §3.2).
pub fn agent_store_classifier() -> StoreClassifier {
    StoreClassifier::of([
        // The run/tool_def/proposed_effect/hitl_gate OLTP schema → H11 (agent operational state).
        myelin_substrate::StoreHolder::new(
            StoreKind::Oltp,
            AGENT_OLTP_STORE,
            Holder::H11AgentMemory,
        ),
        // The execution-trace + conversation-history store → H17 (distinct from the audit log).
        myelin_substrate::StoreHolder::new(
            StoreKind::Oltp,
            AGENT_TRACE_STORE,
            Holder::H17AgentTrace,
        ),
    ])
}

/// **Register the Agent-Fabric's stores as `PersonalDataHolder`s through the harness
/// auto-registration (contract 1.4).** Opens both Fabric stores through the substrate
/// [`HolderRegistry`] — the ONE door — so each is a registered holder by construction. Returns the
/// registry (carrying the two receipts) so a caller / test can assert exactly which stores
/// registered + that they classify to their H-holders (H11 OLTP schema, H17 trace).
///
/// At AG-P3 this is the REGISTRATION SEAM only — `serve` (the SKELETON runtime, AG-P4 → P-216) will
/// open the real stores (re-running this exact classification) on boot; registering now makes "the
/// DSAR fan-out forgot the Agent Fabric" structurally impossible (10.1 exhaustiveness). The holder
/// BODIES (the full DSR fan-out, AG-D10) are the named floor AG-P23 (→ P-479).
pub fn register_agent_holders() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    // The Fabric OLTP schema (run/tool_def/proposed_effect/hitl_gate) — declared H11 above.
    registry.open(StoreKind::Oltp, AGENT_OLTP_STORE);
    // The execution-trace + conversation-history store — declared H17 above.
    registry.open(StoreKind::Oltp, AGENT_TRACE_STORE);
    registry
}

/// The DSR-body floor marker note (PII-free) — names where the real body lands so the stub is never
/// mistaken for the whole erasure answer (VISION §3 name-your-floors). The stub bodies are
/// **empty-but-correct**, never panicking, so the registration + fan-out path is exercisable now.
fn floor_note(store: &str) -> String {
    format!(
        "Agent-Fabric `{store}` is the AG-P3 REGISTRATION SEAM (the holder is registered + tagged so \
         the DSAR fan-out reaches it). The real bodies — locate over the run/trace rows naming the \
         subject; erase = crypto-shred the per-subject DEK (the CryptoShred(subject_dek) tags) + \
         pseudonym-shred the agent_principal/on_behalf_of attribution edges + tombstone (drill \
         AG-D10) — land in AG-P23 (→ P-479); the trace holder body (content-addressed write into \
         Knowledge + erasure) in AG-P19 (→ P-268, KN-D11/KN-D12)."
    )
}

/// The Agent-Fabric **OLTP schema** AS a [`PersonalDataHolder`] (H11; contract 10.1). At AG-P3 a
/// REGISTRATION SEAM: the bodies are empty-but-correct receipts that NAME their AG-P23 follow-on —
/// `locate`/`export` return empty-but-correct content-addressed receipts (the full row/effect/gate
/// walk lands in AG-P23), and `erase`/`rectify`/`restrict` are well-defined no-ops. The structural
/// erasure lever (per-subject DEK + pseudonym tags) exists on [`crate::schema`] today.
#[derive(Clone, Copy, Debug, Default)]
pub struct AgentOltpHolder;

impl AgentOltpHolder {
    /// Register this holder through the substrate registry (the `serve`-called auto-registration
    /// seam), returning the receipt — the proof the OLTP schema registered as holder H11.
    pub fn register(&self, registry: &mut HolderRegistry) -> AgentHolderRegistration {
        registry.open(StoreKind::Oltp, AGENT_OLTP_STORE)
    }

    /// The opaque, PII-free subject id the receipt body keys on (the pseudonymous Principal id) —
    /// never a name/email. This is the `agent_principal`/`on_behalf_of` pseudonym posture (EI-04 §1).
    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for AgentOltpHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        // EMPTY-BUT-CORRECT: the registration seam attests the locate completed over the Fabric OLTP
        // surface (NOT an error — the holder is a real, callable stub). The full row/effect/gate
        // walk naming the subject lands in AG-P23 (the AG-D10 fan-out).
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                AGENT_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "registration-seam (AG-P3: holder registered + tagged; locate body AG-P23 → P-479)",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        // EMPTY-BUT-CORRECT: an empty portable bundle. The full export of the subject's run/effect/
        // gate rows lands in AG-P23.
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                AGENT_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "registration-seam (AG-P3: holder registered + tagged; export body AG-P23 → P-479)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                AGENT_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (AG-P3 registration seam; rectify body AG-P23 → P-479)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                AGENT_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                &format!(
                    "no-op on={on} (AG-P3 registration seam; suppression body AG-P23 → P-479)"
                ),
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        // No-op crypto-shred: the registration seam ships the holder; the real structural erasure —
        // crypto-shred the per-subject DEK (the CryptoShred(subject_dek) tags) + pseudonym-shred the
        // agent_principal/on_behalf_of attribution edges + tombstone (drill AG-D10) — lands in
        // AG-P23. The lever (per-subject DEK + pseudonym tags) already exists on crate::schema.
        let (subject_id, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        let _ = floor_note(AGENT_OLTP_STORE); // the floor is named in the receipt outcome below.
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                AGENT_OLTP_STORE,
                &subject_id,
                &tenant,
                "no-op (AG-P3 registration seam; structural crypto-shred + pseudonym shred AG-P23 → P-479)",
                None,
                0,
            ),
        })
    }
}

/// The Agent-Fabric **trace** store AS a [`PersonalDataHolder`] (H17; contract 10.1 / 8.8). At AG-P3
/// a REGISTRATION SEAM mirroring [`AgentOltpHolder`]: the bodies are empty-but-correct receipts /
/// no-ops that NAME their follow-on. The trace is the run's reasoning record + conversation history,
/// deliberately DISTINCT from the audit log (AG-7; gdpr §6.5). The holder BODY (the content-addressed
/// write into Knowledge + its erasure) lands in AG-P19 (→ P-268); the DSR fan-out in AG-P23.
#[derive(Clone, Copy, Debug, Default)]
pub struct AgentTraceHolder;

impl AgentTraceHolder {
    /// Register the trace store through the substrate registry, returning the receipt — the proof
    /// the trace store registered as holder H17.
    pub fn register(&self, registry: &mut HolderRegistry) -> AgentHolderRegistration {
        registry.open(StoreKind::Oltp, AGENT_TRACE_STORE)
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for AgentTraceHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                AGENT_TRACE_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "registration-seam (AG-P3: trace holder registered + tagged; body AG-P19 → P-268 / fan-out AG-P23)",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                AGENT_TRACE_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "registration-seam (AG-P3: trace holder registered + tagged; export body AG-P23 → P-479)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                AGENT_TRACE_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (AG-P3 registration seam; trace is content-addressed — rectify-by-rewrite AG-P19 → P-268)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                AGENT_TRACE_STORE,
                &Self::subject_id(subject),
                "",
                &format!(
                    "no-op on={on} (AG-P3 registration seam; suppression body AG-P23 → P-479)"
                ),
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        // No-op crypto-shred: the trace_body is tagged CryptoShred(subject_dek) (crate::schema); the
        // real erase — crypto-shred the per-subject DEK so the content-addressed trace is
        // unrecoverable live + in backups (the AG-D10 "erasure reaches the trace" drill) — lands in
        // AG-P23, with the Knowledge content-addressed write/erasure in AG-P19.
        let (subject_id, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        let _ = floor_note(AGENT_TRACE_STORE);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                AGENT_TRACE_STORE,
                &subject_id,
                &tenant,
                "no-op (AG-P3 registration seam; trace crypto-shred AG-D10 → AG-P23 / P-479; write AG-P19)",
                None,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::{
        assert_all_holders_registered, assert_holder_completeness, classify_store, DeclaredStore,
        StoreManifest,
    };

    fn subject(id: &str) -> SubjectRef {
        // The subject is identified by its opaque pseudonymous Principal id (EI-04 §1); the kind is
        // immaterial to the registration seam. `Human` matches the M1 holder-test convention.
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ))
    }

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    /// **The Agent Fabric registers BOTH its stores as holders through the one door (contract
    /// 1.4).** The OLTP schema + the trace store are opened through the substrate registry, so each
    /// is a registered holder by construction — 0 stores escape registration. This is the AG-P3 GATE
    /// (assert the registry lists each store).
    #[test]
    fn agent_registers_both_stores_as_holders() {
        let registry = register_agent_holders();
        assert!(registry.is_registered(StoreKind::Oltp, AGENT_OLTP_STORE));
        assert!(registry.is_registered(StoreKind::Oltp, AGENT_TRACE_STORE));
        assert_eq!(
            registry.len(),
            2,
            "exactly the two Agent-Fabric stores registered"
        );
    }

    /// **Re-registration is idempotent** — `serve` (AG-P4) re-running the registration on a restart
    /// records each Fabric store exactly once (the registry is idempotent on (kind, name)).
    #[test]
    fn re_registration_is_idempotent() {
        let mut registry = register_agent_holders();
        AgentOltpHolder.register(&mut registry);
        AgentTraceHolder.register(&mut registry);
        assert_eq!(
            registry.len(),
            2,
            "re-opening the same Fabric stores does not double-register"
        );
    }

    /// **The Fabric stores classify to their H-holders — 0 orphans (contract 1.4 + gdpr §3.2).** The
    /// OLTP schema maps to **H11 (`AgentMemory`)** + the trace to **H17 (`AgentTrace`)** via the
    /// Fabric classifier. The substrate completeness assertion is GREEN — no Fabric store falls
    /// outside the exhaustive H1–H18 list, so the M5 DSAR fan-out cannot miss the Agent Fabric.
    #[test]
    fn agent_stores_classify_to_h11_and_h17_no_orphan() {
        let registry = register_agent_holders();
        let classifier = agent_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, AGENT_OLTP_STORE, &classifier),
            Some(Holder::H11AgentMemory),
            "the Fabric OLTP schema is holder H11 (agent operational state)"
        );
        assert_eq!(
            classify_store(StoreKind::Oltp, AGENT_TRACE_STORE, &classifier),
            Some(Holder::H17AgentTrace),
            "the execution-trace store is holder H17 (distinct from the audit log)"
        );
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "every Fabric store is in the exhaustive H1–H18 list — 0 orphan stores"
        );
    }

    /// **The holder-registered architecture test: a Fabric store opened OUTSIDE the harness FAILS
    /// (contract 1.4 — the enforcement).** The fixture an unregistered PII store fails the harness
    /// check: the manifest declares both Fabric stores; a registry missing one (a store opened
    /// outside the one door) is a loud violation. The conforming registry (both opened through the
    /// door) passes.
    #[test]
    fn unregistered_fabric_store_fails_the_harness_check() {
        let manifest = StoreManifest::of([
            DeclaredStore::new(StoreKind::Oltp, AGENT_OLTP_STORE),
            DeclaredStore::new(StoreKind::Oltp, AGENT_TRACE_STORE),
        ]);
        // CONFORMING: both opened through the one door → 0 violations.
        let good = register_agent_holders();
        assert_eq!(
            assert_all_holders_registered(&manifest, &good),
            Ok(()),
            "both Fabric stores opened through the harness → the architecture test passes"
        );
        // VIOLATING: the trace store was opened OUTSIDE the harness (never registered) → a loud
        // violation naming exactly the escaped store.
        let mut rogue = HolderRegistry::new();
        rogue.open(StoreKind::Oltp, AGENT_OLTP_STORE); // only the OLTP store went through the door.
        let err = assert_all_holders_registered(&manifest, &rogue).expect_err(
            "a Fabric store opened outside the harness must FAIL the architecture test",
        );
        assert_eq!(
            err.len(),
            1,
            "exactly the unregistered trace store is the violation"
        );
        assert!(
            err[0].message().contains(AGENT_TRACE_STORE),
            "the failure names the escaped Fabric store: {}",
            err[0].message()
        );
    }

    /// **The holder bodies are empty-but-correct (the registration seam) — never a panic.** Both
    /// holders respond to `locate`/`export` with content-addressed receipts over an EMPTY surface
    /// (NOT an error — a real, callable stub); each names its AG-P19/AG-P23 follow-on. The bodies are
    /// deterministic + PII-free.
    #[test]
    fn holder_bodies_are_empty_but_correct_and_name_their_floor() {
        for holder in [
            &AgentOltpHolder as &dyn PersonalDataHolder,
            &AgentTraceHolder as &dyn PersonalDataHolder,
        ] {
            let subj = subject("psn:agent-7");
            let locate = holder
                .locate(&subj, tenant())
                .expect("locate over the seam succeeds");
            assert_eq!(locate.receipt.operation, "locate");
            assert!(locate.receipt.content_hash.starts_with("blake3:"));
            assert!(
                locate.receipt.key_epoch_destroyed.is_none(),
                "locate shreds no key"
            );

            let export = holder
                .export(&subj, tenant())
                .expect("export over the seam succeeds");
            assert_eq!(export.receipt.operation, "export");
            assert!(export.receipt.content_hash.starts_with("blake3:"));
        }
    }

    /// **`erase` is a well-defined no-op now (the registration seam) returning a receipt — never a
    /// panic — and names its AG-P23 (AG-D10) follow-on.** Idempotent: the same scope yields the same
    /// content-addressed receipt (no DEK shredded yet — the structural crypto-shred body is AG-P23).
    #[test]
    fn erase_is_a_no_op_receipt_idempotent_and_names_ag_p23() {
        for holder in [
            &AgentOltpHolder as &dyn PersonalDataHolder,
            &AgentTraceHolder as &dyn PersonalDataHolder,
        ] {
            let scope = EraseScope::Subject {
                subject: subject("psn:agent-7"),
                tenant: tenant(),
            };
            let r1 = holder
                .erase(scope.clone())
                .expect("seam erase succeeds (no-op)");
            let r2 = holder.erase(scope).expect("seam erase is idempotent");
            assert_eq!(
                r1, r2,
                "the same erase scope yields the identical content-addressed receipt"
            );
            assert!(
                r1.receipt.key_epoch_destroyed.is_none(),
                "no DEK shredded (body is AG-P23)"
            );
            assert_eq!(r1.receipt.operation, "erase");
            assert!(r1.receipt.content_hash.starts_with("blake3:"));
        }
    }

    /// **The floor note names the AG-P23 + AG-P19 follow-ons (VISION §3 — a registered
    /// no-op-with-named-follow-on, not a silent gap).** The note is PII-free and points at the exact
    /// filling prompts.
    #[test]
    fn floor_note_names_the_follow_on_prompts() {
        let note = floor_note(AGENT_OLTP_STORE);
        assert!(
            note.contains("AG-P23"),
            "names the DSR fan-out follow-on: {note}"
        );
        assert!(
            note.contains("AG-P19"),
            "names the trace holder-body follow-on: {note}"
        );
        assert!(note.contains("AG-D10"), "names the erasure drill: {note}");
    }

    /// **The holders are object-safe** — held behind `dyn PersonalDataHolder` exactly as the DSR
    /// orchestrator / holder registry need (a heterogeneous holder set, contract 10.1).
    #[test]
    fn holders_are_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> =
            vec![Box::new(AgentOltpHolder), Box::new(AgentTraceHolder)];
        let subj = subject("psn:agent-9");
        for h in &holders {
            assert!(
                h.locate(&subj, tenant()).is_ok(),
                "each holder responds to the contract"
            );
        }
    }
}
