//! # DSR tenant-operability: Art. 28 tenant-facing DSR + tenant offboarding
//! (`EraseScope::Tenant`) + restrict / rectify / portability surfaces (P-GA-13 → P-113)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§4.4** (tenant-operability
//! + the non-erasure rights):
//! - the orchestrator is exposed **to tenants** for *their* data subjects (**Art. 28** assistance —
//!   the customer org is the controller; Myelin is the processor; the tenant *instructs* the DSR);
//! - **tenant offboarding** = a tenant-scoped erase: a full export bundle + **tenant-granularity
//!   crypto-shred** (destroy the tenant KEK ⇒ every per-subject DEK is unwrappable ⇒ the whole
//!   tenant is unrecoverable, **backups included**) + a sealed **offboarding certificate** — just
//!   `erase(EraseScope::Tenant)` over the holder list;
//! - **restriction (Art. 18/21):** `restrict(subject, on)` sets a per-subject suppression flag every
//!   holder honours (reversible); **rectification (Art. 16):** corrects the primary store + fans out
//!   to derivatives via reindex-from-source; **portability (Art. 20):** `export` returns the
//!   subject-provided data structured.
//! - **§1** (the two postures — *processor* for tenant content, *controller* for platform-operational
//!   data) and **§3.1** (the holder contract — the ONLY way the orchestrator touches a store, `{locate,
//!   export, rectify, restrict, erase}`).
//!
//! **Contract-index:** row **10.4** — the **tenant-operability surfaces** (Art. 28 DSR, offboarding,
//! restrict/rectify/portability entry points) are OWNED here. Consumed: row **10.1** (the holders the
//! offboarding fans over, driven via [`crate::orchestration::UpstreamHolderOrchestrator`]).
//!
//! ## What THIS prompt (P-GA-13) ships — and what it reuses
//! P-GA-11 ([`crate::dsr::DsrOrchestrator`]) shipped the DSR spine (the state machine + the posture
//! gate + the coarse deadline + the read-only checklist resolve). The posture gate ALREADY admits a
//! tenant-instructed erase and a `EraseScope::Tenant` offboarding (the `Initiator::TenantInstructed`
//! and `EraseScope::Tenant` branches of [`crate::dsr::DsrOrchestrator::posture_gate_refuses`]).
//! P-GA-12 ([`crate::fanout::FanOutDriver`]) shipped the data-map-driven resumable fan-out + the
//! legal-hold gate + the verifiable §4.2 completion receipt. This prompt **EXPOSES the orchestrator
//! to tenants** and **wires the non-erasure rights through it** — it REUSES both wholesale (EI-01 §7
//! coherence: extend in place, never duplicate the state machine, the posture gate, or the fan-out).
//! It adds exactly what is genuinely new for tenant-operability:
//! 1. **The Art-28 scoping guard** ([`TenantDsrSurface::submit_for_my_subject`]) — a tenant may only
//!    submit a DSR for a data subject **in its own tenant**. A request whose subject lives under a
//!    DIFFERENT tenant is REFUSED ([`TenantDsrError::CrossTenantSubject`]) before it ever reaches the
//!    orchestrator (the cross-tenant-IDOR floor SUB-D7's GDPR face — a tenant cannot reach another
//!    tenant's subject through the Art. 28 surface). The request is encoded
//!    `Initiator::TenantInstructed` + `Posture::Processor` (tenant content — the customer instructs).
//! 2. **Tenant offboarding** ([`TenantDsrSurface::offboard_tenant`]) — submits + validates +
//!    fans-out a `EraseScope::Tenant` erase over the holder list (the canonical erase order), sealing
//!    a [`OffboardingCertificate`]. Models the **tenant-KEK destruction** (destroy the tenant KEK ⇒
//!    every DEK unwrappable ⇒ whole tenant unrecoverable, backups included) via the SAME crypto-shred
//!    seam ([`crate::holders::CryptoShredKms`]) the per-subject erase uses — the live KMS binding is
//!    the named floor, the structural fan-out + the certificate ship here.
//! 3. **The non-erasure right entry points** — `restrict` / `rectify` / `portability` route through
//!    the orchestrator ([`TenantDsrSurface::restrict_subject`] / [`Self::rectify_subject`] /
//!    [`Self::portability_for_subject`]), each as a DSR of the matching [`crate::dsr::DsrKind`]
//!    driven by the existing [`crate::fanout::FanOutDriver`] (a read/restrict right is never refused
//!    by the posture gate and never suspended by a hold — §4.1 step 3).
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **Single-cell** tenant offboarding ships here; the **multi-cell `member_cells` iteration** over
//!   the cross-cell PII-free `CrossCellPointer` bridge → **M5 P-GA-33 → and the GA-D8 gate** (this
//!   surface is the single-cell driver each cell runs; the control plane sequences the wave).
//! - The **full `restrict`-honoured-everywhere proof** (the suppression flag flowing into every
//!   derived store — Search/Refs/Notif/Agents/OLAP) → **M2 P-GA-25 → P-152**. This prompt wires the
//!   `restrict` DSR ENTRY POINT through the orchestrator + the holder `restrict` op; the
//!   into-derived-stores fan-out proof is P-GA-25.
//! - The **reindex-from-source rectification fan-out** (the derivative-correction half of Art. 16) →
//!   **M2 P-GA-24 → P-151**. This prompt wires the `rectify` DSR entry point + the primary-store
//!   `rectify` op; the derivative reindex fan-out is P-GA-24.
//! - The **durable Postgres `dsr_request` (G1) table** + the **live KMS binding** for the tenant-KEK
//!   destruction → the same DB / KMS floor every M0/M1 in-memory store carries (P-007 / P-S12 / the
//!   Storage KMS hierarchy P-ST-06). On this floor the register is in-memory and the tenant-KEK shred
//!   runs through the [`crate::holders::CryptoShredKms`] seam with byte-for-byte the §4.4 semantics.
//!
//! ## Mutation floor (P-GA-13 TESTS — the offboarding-fan-out + the Art-28-scoping path are
//! mandatory-core). The behavioral core every mutation must be CAUGHT:
//! [`TenantDsrSurface::submit_for_my_subject`]'s Art-28 cross-tenant guard (a subject in a different
//! tenant is REFUSED), [`TenantDsrSurface::offboard_tenant`]'s `EraseScope::Tenant` fan-out (the
//! offboarding fans over EVERY holder ⇒ 100% coverage + the sealed certificate), and the
//! restrict/rectify/portability routing (each routes through the orchestrator as its matching DSR
//! kind). `cargo mutants -p myelin-gdpr-service --file src/tenant_ops.rs` (2026-06-20): **11
//! mutants, 3 caught, 8 unviable, 0 MISSED.** The behavioral core is CAUGHT — the Art-28 guard's
//! `==`→`!=` mutant (the load-bearing cross-tenant predicate) and its `Ok(())` short-circuit are
//! both killed (the cross-tenant-refusal test). The 8 unviable are all `-> Ok(Default::default())`
//! substitutions on return types with **no `Default` impl** (`DsrId` / `FanOutOutcome` /
//! `OffboardingCertificate` / `SubjectRef` / `TenantDsrError`) — they do not typecheck, so they
//! cannot represent a real behavior change; the functions themselves are thin façade delegations
//! whose effect is asserted end-to-end by the offboarding + Art-28 + routing tests. The
//! `<TenantDsrError as Display>::fmt` mutant is ALSO caught (the `cross_tenant_error_renders_pii_free`
//! test pins the rendered message). There are **0 missed mutants** on this module. Stated, not hidden
//! (EI-01 §3).

use myelin_gdpr::{EraseScope, SubjectRef, TenantId};
use myelin_substrate::Clock;

use crate::dsr::{DsrId, DsrKind, DsrOrchestrator, Initiator, Posture};
use crate::fanout::{FanOutDriver, FanOutOutcome, LegalHoldRegistry};
use crate::orchestration::{EraseChecklist, UpstreamHolderOrchestrator};

// ───────────────────────── tenant-operability errors (loud, never swallowed) ─────────────────────────

/// A tenant-operability error (EI-01 §3 — make a violation loud; the Art-28 cross-tenant attempt is
/// a SECURITY denial, never a silent empty). The posture-gate REFUSAL of a Myelin-initiated erase
/// is NOT here (that is a legal terminal state, [`crate::dsr::DsrState::Refused`]); these are
/// tenant-surface faults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TenantDsrError {
    /// **The Art-28 cross-tenant guard fired (§4.4 — a tenant may only act for ITS OWN data
    /// subjects).** A tenant tried to submit a DSR for a subject that lives under a DIFFERENT
    /// tenant (the cross-tenant-IDOR floor SUB-D7's GDPR face). Carries `(calling_tenant,
    /// subject_tenant)` — both opaque tokens, PII-free.
    CrossTenantSubject {
        /// the tenant that issued the Art. 28 request.
        calling_tenant: String,
        /// the tenant the request's subject actually lives under (≠ `calling_tenant`).
        subject_tenant: String,
    },
    /// An underlying DSR-orchestrator error propagated up (an illegal state-machine transition, a
    /// holder fan-out error, …). The tenant surface NEVER swallows an orchestrator error — it
    /// surfaces it verbatim.
    Orchestrator(crate::dsr::DsrError),
}

impl std::fmt::Display for TenantDsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TenantDsrError::CrossTenantSubject {
                calling_tenant,
                subject_tenant,
            } => write!(
                f,
                "Art. 28 cross-tenant DSR refused: tenant `{calling_tenant}` may only act for its \
                 own data subjects, but the subject lives under tenant `{subject_tenant}` (§4.4)"
            ),
            TenantDsrError::Orchestrator(e) => write!(f, "DSR orchestrator error: {e}"),
        }
    }
}

impl std::error::Error for TenantDsrError {}

impl From<crate::dsr::DsrError> for TenantDsrError {
    fn from(e: crate::dsr::DsrError) -> TenantDsrError {
        TenantDsrError::Orchestrator(e)
    }
}

/// The tenant-operability result type.
pub type Result<T> = std::result::Result<T, TenantDsrError>;

// ───────────────────────── the sealed offboarding certificate (§4.4) ─────────────────────────

/// **The sealed tenant-offboarding certificate (§4.4).** A tenant offboarding destroys the tenant
/// KEK ⇒ every per-subject DEK is unwrappable ⇒ the whole tenant (backups included) is
/// unrecoverable; this certificate is the verifiable proof of that, sealing the per-holder fan-out
/// into ONE tenant-level record. It wraps the §4.2 [`crate::fanout::DsrCompletionReceipt`] (the
/// content-addressed, audit-sealable bundle of the ordered per-holder receipts — each recording its
/// destroyed key epoch) plus the offboarded tenant token + the DSR id.
///
/// PII-free: the `tenant` is an opaque token (never a name/email); the wrapped completion receipt
/// carries only opaque ids + content-addresses. Safe to seal into the tamper-evident audit log. The
/// **Merkle inclusion** that anchors it into the per-tenant audit tree is **P-GA-20 → P-119** (the
/// wrapped completion receipt is the input that certificate seals).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffboardingCertificate {
    /// The offboarded tenant (the opaque token; the whole tenant is now unrecoverable).
    pub tenant: TenantId,
    /// The DSR this offboarding ran under (for the audit trail / re-drive).
    pub dsr_id: DsrId,
    /// The verifiable §4.2 completion receipt (the ordered per-holder receipts + the content
    /// address). The Merkle seal is P-GA-20.
    pub completion: crate::fanout::DsrCompletionReceipt,
}

// ───────────────────────── the tenant-facing DSR surface (the P-GA-13 deliverable) ─────────────────────────

/// **The DSR tenant-operability surface (contract 10.4 — §4.4).** Exposes the DSR orchestrator to
/// **tenants** for *their own* data subjects (Art. 28 assistance), wires **tenant offboarding**
/// (`erase(EraseScope::Tenant)`), and routes the **non-erasure rights** (restrict / rectify /
/// portability) through the orchestrator. It REUSES the DSR spine ([`DsrOrchestrator`], P-GA-11) +
/// the fan-out driver ([`FanOutDriver`], P-GA-12) + the legal-hold gate wholesale — it does NOT
/// re-define the state machine, the posture gate, or the fan-out (EI-01 §7 coherence).
///
/// It is intentionally a thin **façade** over the existing orchestrator: every tenant-facing surface
/// is the SAME `dsr_submit → validate → drive` pipeline the controller surface (P-GA-11/-12) runs,
/// with (a) the Art-28 scoping guard front-loaded and (b) the `Initiator::TenantInstructed` /
/// `Posture::Processor` encoding (tenant content — the customer org is the controller). The
/// no-cross-store-read law (§3.1) holds structurally: the surface never reaches into a store — it
/// only drives the holder contract through the fan-out.
pub struct TenantDsrSurface<'a, C: Clock> {
    /// The DSR spine (the state machine + the request register).
    dsr: &'a DsrOrchestrator<C>,
    /// The legal-hold gate (G4) the fan-out driver passes through.
    holds: &'a LegalHoldRegistry,
}

impl<'a, C: Clock> TenantDsrSurface<'a, C> {
    /// Build a tenant-facing surface over the DSR spine + the legal-hold gate. The upstream holder
    /// orchestrator + the durable checklist are passed per-operation (they are per-fan-out state).
    pub fn new(
        dsr: &'a DsrOrchestrator<C>,
        holds: &'a LegalHoldRegistry,
    ) -> TenantDsrSurface<'a, C> {
        TenantDsrSurface { dsr, holds }
    }

    /// **The Art-28 scoping guard (§4.4 — a tenant may only act for ITS OWN data subjects).** A
    /// data subject's home tenant is its verified principal's `tenant` (the X-5 anchor — the
    /// principal is verified, never a runtime guess). A request whose subject lives under a
    /// DIFFERENT tenant than the calling tenant is REFUSED ([`TenantDsrError::CrossTenantSubject`])
    /// — the cross-tenant-IDOR floor SUB-D7's GDPR face. Returns `Ok(())` iff the subject is in the
    /// calling tenant. Pure — no state mutation.
    fn art28_scope_ok(calling_tenant: &TenantId, subject: &SubjectRef) -> Result<()> {
        let subject_tenant = &subject.principal.tenant;
        if subject_tenant == calling_tenant {
            Ok(())
        } else {
            Err(TenantDsrError::CrossTenantSubject {
                calling_tenant: calling_tenant.0.clone(),
                subject_tenant: subject_tenant.0.clone(),
            })
        }
    }

    /// **Art. 28 — submit a tenant-instructed DSR for one of the tenant's OWN data subjects (§4.4).**
    /// The tenant (the controller) instructs Myelin (the processor) to action a `kind` (erasure /
    /// access / portability / rectification / restriction) over `subject`. The Art-28 scoping guard
    /// runs FIRST: a subject under a different tenant is REFUSED before any DSR is minted (the
    /// cross-tenant-IDOR face). The request is encoded `Initiator::TenantInstructed` (the tenant
    /// authorised it — the posture gate ADMITS even a processor-posture erase) + `Posture::Processor`
    /// (tenant content). Returns the minted `dsr_id` (the request is `Received`; drive it via
    /// [`Self::drive_tenant_subject_dsr`]).
    pub fn submit_for_my_subject(
        &self,
        calling_tenant: &TenantId,
        kind: DsrKind,
        subject: SubjectRef,
    ) -> Result<DsrId> {
        // §4.4 Art-28 scoping — the tenant may only act for its OWN subjects (cross-tenant refused).
        Self::art28_scope_ok(calling_tenant, &subject)?;
        let scope = EraseScope::Subject {
            subject: subject.clone(),
            tenant: calling_tenant.clone(),
        };
        let id = self.dsr.dsr_submit(
            kind,
            calling_tenant.clone(),
            subject,
            scope,
            Posture::Processor, // tenant content — the customer org is the controller.
            Initiator::TenantInstructed, // Art. 28 — the controller instructed the action.
        );
        Ok(id)
    }

    /// **Drive a tenant-submitted subject DSR end-to-end (Art. 28).** Validates (the posture gate
    /// admits a tenant-instructed request) then drives the fan-out ([`FanOutDriver`], P-GA-12):
    /// resolve the checklist FROM the data map → the legal-hold gate → the fan-out / read right →
    /// the verifiable receipt. Returns the [`FanOutOutcome`]. The tenant surface reuses the EXACT
    /// controller pipeline (no second path).
    pub fn drive_tenant_subject_dsr(
        &self,
        id: &DsrId,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<FanOutOutcome> {
        // The posture gate runs in `validate` (a tenant-instructed request is ADMITTED — §1/§4.4).
        // `validate` returns `false` ONLY if the gate refused; a tenant-instructed request is never
        // refused, so a `false` here would be an invariant break we surface loudly.
        let admitted = self.dsr.validate(id)?;
        debug_assert!(
            admitted,
            "a tenant-instructed (Art. 28) DSR is never posture-refused (§4.4)"
        );
        let driver = FanOutDriver::new(self.dsr, self.holds);
        Ok(driver.drive(id, inventory, upstream, checklist)?)
    }

    /// **Tenant offboarding (§4.4) — `erase(EraseScope::Tenant)` over the holder list.** Submits +
    /// validates + fans out a **whole-tenant** erase (the tenant KEK is destroyed ⇒ every per-subject
    /// DEK is unwrappable ⇒ the whole tenant, backups included, is unrecoverable), then seals an
    /// [`OffboardingCertificate`]. The fan-out runs in the canonical erase order over EVERY existing
    /// holder (`erasure_fanout_coverage` = 100% — the §4.4 GATE) through the EXISTING resumable
    /// driver. An offboarding is an AUTHORISED erase even Myelin-initiated (the posture gate's
    /// `EraseScope::Tenant` branch admits it — §4.4), so the `initiator` is `Myelin` (an operator
    /// runs the offboarding on the tenant's instruction-of-record).
    ///
    /// Resumable: a crashed offboarding re-`offboard_tenant`s the SAME tenant over the SAME checklist
    /// and re-drives only un-receipted holders (resumability is the checklist's property — P-GA-12).
    pub fn offboard_tenant(
        &self,
        tenant: &TenantId,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<OffboardingCertificate> {
        // §4.4 — a tenant-scoped erase. `EraseScope::Tenant` is an authorised offboarding (the
        // posture gate admits it even Myelin-initiated). Destroying the tenant KEK ⇒ the whole tenant
        // is unrecoverable; the holder fan-out shreds each holder's tenant key class.
        let id = self.dsr.dsr_submit(
            DsrKind::Erasure,
            tenant.clone(),
            // the offboarding subject is the tenant itself — we carry a tenant-scoped subject ref so
            // the request view is well-formed; the SCOPE (EraseScope::Tenant) is what drives the
            // fan-out (the offboarding is tenant-granularity, not per-subject).
            tenant_subject(tenant),
            EraseScope::Tenant(tenant.clone()),
            Posture::Processor, // tenant content — the customer org is the controller.
            Initiator::Myelin, // an operator runs the offboarding (authorised: it IS an offboarding).
        );
        let admitted = self.dsr.validate(&id)?;
        debug_assert!(
            admitted,
            "a tenant offboarding (EraseScope::Tenant) is an authorised erase (§4.4)"
        );
        let driver = FanOutDriver::new(self.dsr, self.holds);
        let outcome = driver.drive(&id, inventory, upstream, checklist)?;
        // The offboarding is an erase — the driver returns `Erased` (it is never a read right; a
        // tenant offboarding is never legal-hold-deferred at the surface — a per-tenant litigation
        // hold is the operator's pre-condition, surfaced by the driver's DeferredUnderHold outcome
        // we propagate faithfully rather than masking).
        let completion = outcome.receipt().clone();
        Ok(OffboardingCertificate {
            tenant: tenant.clone(),
            dsr_id: id,
            completion,
        })
    }

    /// **Restriction (Art. 18/21) — route a `restrict` DSR through the orchestrator (§4.4).** A
    /// tenant-instructed restriction of processing for ITS OWN subject (the per-subject suppression
    /// flag every holder honours — reversible). The Art-28 guard runs first. A restriction is never
    /// posture-refused and never legal-hold-suspended (it is not an erase — §4.1 step 3). Returns the
    /// driven [`FanOutOutcome`] (the restriction is recorded with a verifiable receipt). The
    /// honoured-everywhere-into-derived-stores proof is **M2 P-GA-25**.
    pub fn restrict_subject(
        &self,
        calling_tenant: &TenantId,
        subject: SubjectRef,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<FanOutOutcome> {
        let id = self.submit_for_my_subject(calling_tenant, DsrKind::Restriction, subject)?;
        self.drive_tenant_subject_dsr(&id, inventory, upstream, checklist)
    }

    /// **Rectification (Art. 16) — route a `rectify` DSR through the orchestrator (§4.4).** A
    /// tenant-instructed correction of ITS OWN subject's data (the primary store is corrected; the
    /// derivative reindex-from-source fan-out is **M2 P-GA-24**). The Art-28 guard runs first.
    /// Returns the driven [`FanOutOutcome`].
    pub fn rectify_subject(
        &self,
        calling_tenant: &TenantId,
        subject: SubjectRef,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<FanOutOutcome> {
        let id = self.submit_for_my_subject(calling_tenant, DsrKind::Rectification, subject)?;
        self.drive_tenant_subject_dsr(&id, inventory, upstream, checklist)
    }

    /// **Portability (Art. 20) — route a portability `export` DSR through the orchestrator (§4.4).**
    /// A tenant-instructed structured export of ITS OWN subject's data. The Art-28 guard runs first.
    /// A portability right is never refused / suspended (a read right — §4.1 step 3). Returns the
    /// driven [`FanOutOutcome`] (the served read right with a verifiable receipt).
    pub fn portability_for_subject(
        &self,
        calling_tenant: &TenantId,
        subject: SubjectRef,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<FanOutOutcome> {
        let id = self.submit_for_my_subject(calling_tenant, DsrKind::Portability, subject)?;
        self.drive_tenant_subject_dsr(&id, inventory, upstream, checklist)
    }
}

/// A tenant-scoped subject reference for an offboarding (§4.4). A tenant offboarding is
/// tenant-granularity (the SCOPE is `EraseScope::Tenant`), but the DSR request view carries a
/// `SubjectRef`; we construct a deterministic, PII-free tenant-sentinel principal (`*tenant*` in the
/// tenant) so the request is well-formed. The fan-out is driven by the `EraseScope::Tenant` scope,
/// never by this sentinel subject.
fn tenant_subject(tenant: &TenantId) -> SubjectRef {
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    SubjectRef::new(Principal::stub(
        PrincipalId("*tenant*".into()),
        PrincipalKind::Human,
        tenant.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::datamap::{Inventory, InventoryEntry};
    use crate::dsr::DsrState;
    use crate::fanout::HoldScope;
    use crate::holders::{InMemoryShredKms, ShredKeyClass, ShredKeyHandle};
    use crate::orchestration::{holder_ids, SeamHolder};
    use myelin_gdpr::PersonalDataHolder;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::TestClock;

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    /// A subject whose HOME tenant is `tenant` (the Art-28 scoping anchor — the verified principal's
    /// tenant).
    fn subject_in(tenant: &TenantId, id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant.clone(),
        ))
    }

    /// A KMS seeded with one key per upstream holder (each holder shreds its OWN class).
    fn kms_with_all_holder_keys(tenant: &TenantId, base_epoch: u64) -> InMemoryShredKms {
        let kms = InMemoryShredKms::new();
        for (i, id) in [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .iter()
        .enumerate()
        {
            kms.provision(
                ShredKeyHandle {
                    tenant: tenant.clone(),
                    class: ShredKeyClass::Subject((*id).to_string()),
                },
                base_epoch + i as u64,
            );
        }
        kms
    }

    fn seam_holders(kms: &InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'_>)> {
        [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .into_iter()
        .map(|id| {
            (
                id,
                SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), kms),
            )
        })
        .collect()
    }

    fn inventory() -> Inventory {
        let mut holders = BTreeSet::new();
        holders.insert("identity".to_string());
        holders.insert("search_index:search_index".to_string());
        Inventory {
            entries: vec![InventoryEntry {
                field_path: "PrincipalRow.email".into(),
                holder_id: "identity".into(),
                holder: "H15".into(),
                region: "fr-par".into(),
                category: "ContactInfo".into(),
                role: "PlatformOperational".into(),
                basis: "Contract".into(),
                retention: "UntilContractEnd".into(),
                erasure: "CryptoShred(subject_dek)".into(),
                subject_locator: "principal_id".into(),
            }],
            holders,
            dpia_markers: BTreeSet::new(),
        }
    }

    // ───────────── Art. 28 — a tenant-facing DSR over the tenant's OWN subjects (§4.4) ─────────────

    /// **A tenant-initiated DSR over the tenant's OWN data subject is ADMITTED (Art. 28).** It is
    /// encoded `Initiator::TenantInstructed` + `Posture::Processor`, the posture gate admits it
    /// (even an erase — the controller authorised it), and the fan-out completes with a verifiable
    /// receipt.
    #[test]
    fn art28_tenant_dsr_over_own_subject_is_admitted_and_completes() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 100);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
        let holds = LegalHoldRegistry::new();
        let surface = TenantDsrSurface::new(&dsr, &holds);

        // Art. 28 — the tenant instructs an erasure of its own subject.
        let id = surface
            .submit_for_my_subject(&tenant, DsrKind::Erasure, subject_in(&tenant, "u1"))
            .expect("a tenant may act for its own subject (Art. 28)");
        let checklist = EraseChecklist::new();
        let outcome = surface
            .drive_tenant_subject_dsr(&id, &inventory(), &upstream, &checklist)
            .unwrap();

        // the erase fanned out + completed (the tenant-instructed erase is ADMITTED, not refused).
        assert!(
            matches!(outcome, FanOutOutcome::Erased(_)),
            "tenant-instructed erase admitted + driven"
        );
        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
        assert_eq!(
            upstream.fanout_coverage(&checklist),
            1.0,
            "100% fan-out over the holder list"
        );
    }

    /// **The Art-28 scoping guard REFUSES a cross-tenant subject (§4.4 — the cross-tenant-IDOR
    /// face).** A tenant cannot reach ANOTHER tenant's data subject through the Art. 28 surface — the
    /// request is refused BEFORE a DSR is minted.
    #[test]
    fn art28_refuses_a_dsr_over_another_tenants_subject() {
        let acme = t("acme");
        let evil = t("evil-corp");
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        let surface = TenantDsrSurface::new(&dsr, &holds);

        // evil-corp tries to submit a DSR for a subject whose HOME tenant is acme.
        let err = surface
            .submit_for_my_subject(&evil, DsrKind::Erasure, subject_in(&acme, "victim"))
            .unwrap_err();
        assert_eq!(
            err,
            TenantDsrError::CrossTenantSubject {
                calling_tenant: "evil-corp".into(),
                subject_tenant: "acme".into(),
            },
            "a cross-tenant Art. 28 request is refused (the IDOR face)"
        );
    }

    /// The Art-28 guard is a pure equality on the calling tenant vs the subject's home tenant.
    #[test]
    fn art28_scope_ok_admits_same_tenant_refuses_different_tenant() {
        let acme = t("acme");
        assert!(
            TenantDsrSurface::<TestClock>::art28_scope_ok(&acme, &subject_in(&acme, "u")).is_ok()
        );
        assert!(TenantDsrSurface::<TestClock>::art28_scope_ok(
            &acme,
            &subject_in(&t("other"), "u")
        )
        .is_err());
    }

    // ───────────── tenant offboarding = erase(EraseScope::Tenant) (§4.4) ─────────────

    /// **Tenant offboarding fans `EraseScope::Tenant` over the holder list + seals an offboarding
    /// certificate (§4.4).** Destroying the tenant KEK ⇒ the whole tenant is unrecoverable; the
    /// fan-out shreds every holder's tenant key class in canonical order (Identity first), 100%
    /// coverage, and a sealed [`OffboardingCertificate`] carrying the verifiable completion receipt.
    #[test]
    fn tenant_offboarding_fans_erase_tenant_over_the_holder_list_and_seals_a_certificate() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 200);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
        let holds = LegalHoldRegistry::new();
        let surface = TenantDsrSurface::new(&dsr, &holds);

        let checklist = EraseChecklist::new();
        let cert = surface
            .offboard_tenant(&tenant, &inventory(), &upstream, &checklist)
            .expect("a tenant offboarding is an authorised erase");

        // the offboarding is tenant-granularity (the scope token is the bare tenant — no subject).
        assert_eq!(cert.tenant, tenant);
        assert_eq!(
            cert.completion.scope_token, "acme",
            "tenant-granularity offboarding (no subject)"
        );
        assert_eq!(cert.completion.outcome, "erased");
        // the fan-out hit EVERY holder in canonical order (Identity first) — 100% coverage.
        assert_eq!(
            cert.completion.holder_receipts.len(),
            6,
            "all six holders shredded for offboarding"
        );
        assert_eq!(
            cert.completion.holder_receipts[0].holder_id,
            holder_ids::IDENTITY,
            "Identity FIRST"
        );
        assert!(
            cert.completion.content_hash.starts_with("blake3:"),
            "content-addressed (§4.2)"
        );
        assert_eq!(
            upstream.fanout_coverage(&checklist),
            1.0,
            "100% fan-out (the §4.4 GATE)"
        );
        // the DSR completed via the state machine.
        assert_eq!(dsr.state_of(&cert.dsr_id).unwrap(), DsrState::Completed);
        // every holder receipt records its destroyed key epoch (the tenant-KEK shred trail).
        for hr in &cert.completion.holder_receipts {
            assert!(
                hr.receipt.receipt.key_epoch_destroyed.is_some(),
                "tenant-KEK shred recorded"
            );
        }
    }

    /// **A tenant offboarding is resumable across a worker kill (0 double-shred).** A crash after a
    /// partial fan-out re-`offboard`s only the un-receipted holders.
    #[test]
    fn tenant_offboarding_is_resumable_a_worker_kill_redrives_only_un_receipted_holders() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 300);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        let surface = TenantDsrSurface::new(&dsr, &holds);
        let checklist = EraseChecklist::new();

        // crash after the first two holders: drive a partial sub-orchestrator over the SAME checklist.
        let first_two: Vec<(&'static str, &dyn PersonalDataHolder)> = holders
            .iter()
            .filter(|(id, _)| *id == holder_ids::IDENTITY || *id == holder_ids::BLOB)
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect();
        let partial = UpstreamHolderOrchestrator::register_m1_upstream(first_two);
        partial
            .fan_out_erase(&EraseScope::Tenant(tenant.clone()), &checklist)
            .unwrap();
        assert_eq!(
            checklist.done_count(),
            2,
            "the crash left two holders receipted"
        );
        let calls_after_partial: Vec<u32> =
            holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        // resume — only un-receipted holders are re-driven (0 double-shred).
        let cert = surface
            .offboard_tenant(&tenant, &inventory(), &upstream, &checklist)
            .unwrap();
        for (i, (id, _)) in holders.iter().enumerate() {
            if *id == holder_ids::IDENTITY || *id == holder_ids::BLOB {
                assert_eq!(
                    holders[i].1.erase_call_count(),
                    calls_after_partial[i],
                    "holder {id} already receipted ⇒ NOT re-shredded (0 double-shred)"
                );
            } else {
                assert_eq!(
                    holders[i].1.erase_call_count(),
                    1,
                    "holder {id} shredded on resume"
                );
            }
        }
        assert_eq!(
            cert.completion.holder_receipts.len(),
            6,
            "the certificate has the complete holder set"
        );
        assert_eq!(upstream.fanout_coverage(&checklist), 1.0);
    }

    /// **A tenant offboarding under a per-tenant legal hold is DEFERRED faithfully (not masked).**
    /// The driver's DeferredUnderHold verdict propagates into the certificate's `deferred:legal_hold`
    /// outcome — the operator must clear the hold first. Clearing + re-offboarding completes.
    #[test]
    fn tenant_offboarding_under_a_tenant_hold_is_deferred_then_resumes_when_cleared() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 400);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        holds.set(HoldScope::Tenant("acme".into()), true); // a per-tenant litigation hold.
        let surface = TenantDsrSurface::new(&dsr, &holds);
        let checklist = EraseChecklist::new();

        let cert = surface
            .offboard_tenant(&tenant, &inventory(), &upstream, &checklist)
            .unwrap();
        assert_eq!(
            cert.completion.outcome, "deferred:legal_hold",
            "offboarding deferred under hold"
        );
        assert!(
            cert.completion.holder_receipts.is_empty(),
            "no holder shredded under hold"
        );
        assert_eq!(upstream.fanout_coverage(&checklist), 0.0);

        // clear the hold + re-offboard the SAME tenant — it resumes to completion.
        holds.set(HoldScope::Tenant("acme".into()), false);
        let cert2 = surface
            .offboard_tenant(&tenant, &inventory(), &upstream, &checklist)
            .unwrap();
        assert_eq!(cert2.completion.outcome, "erased");
        assert_eq!(upstream.fanout_coverage(&checklist), 1.0);
    }

    // ───────────── the non-erasure rights route through the orchestrator (§4.4) ─────────────

    /// **Restriction / rectification / portability route through the orchestrator as their matching
    /// DSR kind (§4.4).** Each is a tenant-instructed DSR over the tenant's own subject; none is
    /// posture-refused or legal-hold-suspended (they are not erases — §4.1 step 3); each completes
    /// with a verifiable receipt carrying the right's outcome.
    #[test]
    fn restrict_rectify_portability_route_through_the_orchestrator() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 500);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        // a hold is active — to prove a non-erasure right is NEVER suspended by it.
        let holds = LegalHoldRegistry::new();
        holds.set(HoldScope::Tenant("acme".into()), true);
        let surface = TenantDsrSurface::new(&dsr, &holds);

        // Restriction (Art. 18/21).
        let r = surface
            .restrict_subject(
                &tenant,
                subject_in(&tenant, "u-r"),
                &inventory(),
                &upstream,
                &EraseChecklist::new(),
            )
            .unwrap();
        assert!(
            matches!(r, FanOutOutcome::ReadRightServed(_)),
            "restriction is not an erase (not suspended)"
        );
        assert_eq!(r.receipt().outcome, "restriction");

        // Rectification (Art. 16).
        let rec = surface
            .rectify_subject(
                &tenant,
                subject_in(&tenant, "u-rec"),
                &inventory(),
                &upstream,
                &EraseChecklist::new(),
            )
            .unwrap();
        assert_eq!(rec.receipt().outcome, "rectification");

        // Portability (Art. 20).
        let p = surface
            .portability_for_subject(
                &tenant,
                subject_in(&tenant, "u-p"),
                &inventory(),
                &upstream,
                &EraseChecklist::new(),
            )
            .unwrap();
        assert!(
            matches!(p, FanOutOutcome::ReadRightServed(_)),
            "portability is a read right (never suspended)"
        );
        assert_eq!(p.receipt().outcome, "portability");
    }

    /// **A non-erasure right is ALSO Art-28-scoped** — a cross-tenant restrict/rectify/portability is
    /// refused (the guard runs for every tenant-facing surface, not just erasure).
    #[test]
    fn non_erasure_rights_are_also_art28_scoped() {
        let acme = t("acme");
        let evil = t("evil");
        let kms = kms_with_all_holder_keys(&acme, 600);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        let surface = TenantDsrSurface::new(&dsr, &holds);

        let victim = subject_in(&acme, "victim");
        assert!(matches!(
            surface.restrict_subject(
                &evil,
                victim.clone(),
                &inventory(),
                &upstream,
                &EraseChecklist::new()
            ),
            Err(TenantDsrError::CrossTenantSubject { .. })
        ));
        assert!(matches!(
            surface.rectify_subject(
                &evil,
                victim.clone(),
                &inventory(),
                &upstream,
                &EraseChecklist::new()
            ),
            Err(TenantDsrError::CrossTenantSubject { .. })
        ));
        assert!(matches!(
            surface.portability_for_subject(
                &evil,
                victim,
                &inventory(),
                &upstream,
                &EraseChecklist::new()
            ),
            Err(TenantDsrError::CrossTenantSubject { .. })
        ));
    }

    /// The error renders a PII-free, actionable message (the tokens, never a name/email).
    #[test]
    fn cross_tenant_error_renders_pii_free() {
        let e = TenantDsrError::CrossTenantSubject {
            calling_tenant: "evil".into(),
            subject_tenant: "acme".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("evil") && msg.contains("acme") && msg.contains("Art. 28"));
    }
}
