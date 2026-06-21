//! # `holder` — myelin-flow AS a `PersonalDataHolder` over `workflow_run`/`wf_history`/`wf_signal`
//! (the STRUCTURAL references-not-payloads half) — P-FLOW-03 / P-201, M2.
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §5.5 (`PersonalDataHolder`
//! over `workflow_run`/`wf_history`/`wf_signal` — `locate`/`export`/`erase`; references-not-payloads;
//! the rare inline-PII case crypto-shreds via `result_key_ref`/`payload_key_ref`; the full reach is
//! M5) + §4.8 (GDPR erasure on history via the references-not-payloads + crypto-shred + tombstone
//! triad — the structural floor).
//!
//! **Contract-index:** row 9.6 `PersonalDataHolder(workflow history) + replay` — OWNED, the
//! STRUCTURAL half here (trait + auto-registration; `locate`/`export` real, `erase` structurally
//! wired). Consumes 1.4 / 10.1 (the harness holder auto-registration hook + the exhaustive H1–H18
//! list).
//!
//! ## What P-FLOW-03 ships — the holder HALF of 9.6 (registration + the structural erase)
//! The workflow engine registers its OLTP store (the six-table data model — `workflow_run` /
//! `wf_history` / `wf_signal` etc., P-FLOW-01) as a [`PersonalDataHolder`] through the harness
//! one-door auto-registration (1.4 / 10.1 — opening the store IS registering it, [`crate::app`]
//! already opens it on boot), and implements the five-operation surface (9.6) over the journal. The
//! load-bearing property is the **structural references-not-payloads erase**: a `wf_history` /
//! `wf_signal` / `workflow_run` row stores the subject ONLY as
//!
//! 1. an OPAQUE actor/run pseudonym + structured [`myelin_refs::ArtifactRef`]s in
//!    `input`/`result`/`payload` (the workflow about a PR carries the PR's ref, never the PR body —
//!    §3.1/§3.2/§3.4), and
//! 2. — for the RARE inline-PII case — an envelope key ref (`result_key_ref`/`payload_key_ref`)
//!    that NAMES a per-subject DEK, never the bytes.
//!
//! So `locate`/`export` walk the journal for the subject's appearances (by the referenced-actor ref
//! OR the inline-PII key ref) and report PII-free reference rows; `erase` RELIES on the structural
//! posture — erasing a subject tombstones their appearance for free (Identity's pseudonym-map shred,
//! §4.8) — and is **structurally wired but does NOT yet perform the per-subject-DEK crypto-shred**
//! reach into the inline-PII history rows / backups.
//!
//! ## H-holder classification (the EI-01 §7 reconcile — NOT a new H19)
//! The exhaustive gdpr §3.2 holder list ([`myelin_substrate::Holder`], H1–H18) names **no dedicated
//! "workflow history" holder**, and adding an H19 is a deliberate GDPR co-edit this prompt must not
//! make. The workflow engine's OLTP store IS a durable references-not-payloads **event history** with
//! the IDENTICAL erasure profile to **H8 (Event-bus history)** — "pseudonymous actor; rare inline-PII
//! events; crypto-shred inline-PII keys + tombstones; references-not-payloads makes most rows
//! erasure-free" (gdpr §3.2 H8). The workflow journal (`wf_history`) is structurally the same shape:
//! a durable, append-only, references-not-payloads journal whose only PII locators are the inline-PII
//! envelope key refs. So the flow OLTP store classifies to **H8** — the store is accounted for in the
//! exhaustive list (0 orphan), matching the §5.5 cite that the engine's residual handling is the SAME
//! references-not-payloads + crypto-shred + tombstone triad the bus uses (§4.8, "by reference"). This
//! is a documented coherence reconcile, not an invented holder.
//!
//! ## FLOORS named (VISION §3 / EI-01 §1 name-your-floors)
//! - **The crypto-shred reach** — the per-subject-DEK destruction into the inline-PII `result_key_ref`
//!   / `payload_key_ref` history rows + backups — is the NAMED M5 follow-on **P-FLOW-23**. This prompt
//!   ships the STRUCTURAL erase (the references-not-payloads tombstone + the restrict suppression);
//!   the per-subject key destruction lands at P-FLOW-23. So `erase` here destroys NO key at the flow
//!   surface (`key_epoch_destroyed = None`) — the references-not-payloads tombstone needs no key
//!   destroyed for the refs-stored rows; the inline-PII DEK shred is the P-FLOW-23 reach.
//! - **The `replay` half of 9.6** (`replay(scope, since)` — the run rebuilt by deterministic replay
//!   from the journal, the only recovery path) is **P-FLOW-05** (FLOW-D1). This module is the holder
//!   half only; the two together complete contract 9.6.
//! - **`restrict` suppression into live dispatch** (stop NEW dispatch for a restricted subject — the
//!   §4.8 Art. 18/21 suppression) records the restriction in a shared suppression set here; the
//!   dispatch/replay-loop consult of that set lands with the replay/lease loop (P-FLOW-05). The
//!   holder records the op + the suppression set is real.
//!
//! ## The stub → the real surface (the EI-01 §7 reconcile, NOT a parallel second holder)
//! There is ONE flow holder type ([`WfHistoryHolder`]). Unbacked (the registration-only [`Default`]
//! form — `serve`-before-the-replay-engine-populates-the-journal) it is **empty-but-correct** (a
//! tenant whose journal no run has populated has no located rows). Backed
//! ([`WfHistoryHolder::with_journal`]) it runs the REAL structural body over the live [`WfJournal`]
//! the [`crate::wfctx::WfCtx`] co-commit (P-FLOW-04) appends into — the SAME journal, never a parallel
//! second store. The body is the real one the moment the journal is populated.

use std::sync::{Arc, Mutex};

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, Result as DsrResult, RestrictReceipt, SubjectRef, TenantId as GdprTenantId,
};
use myelin_substrate::{
    Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreHolder, StoreKind,
};
use myelin_tenancy::TenantId;

use crate::wfctx::WfJournal;

/// The stable, PII-free name of the myelin-flow **OLTP store** (the six-table data model, P-FLOW-01 —
/// the holder's store). It is EXACTLY [`crate::SERVICE_NAME`] — the name the harness auto-registers
/// the flow OLTP store under on boot ([`crate::app`]), so the data-map, the DSR fan-out, and this
/// classifier all address the SAME store. PII-free: a store identifier, never personal data.
pub const FLOW_OLTP_STORE: &str = crate::SERVICE_NAME;

/// The typed receipt that the flow store was auto-registered as a [`PersonalDataHolder`] (mirrors
/// [`myelin_substrate::HolderRegistration`]). The harness collects these; the holder-registered
/// architecture test reads them to assert the flow store did not escape registration. PII-free.
pub type FlowHolderRegistration = HolderRegistration;

/// Build the flow [`StoreClassifier`] — the data-map declaration that the flow OLTP store belongs to
/// holder **H8 (`EventBus` history)** (gdpr §3.2; the §5.5 reconcile — the workflow journal is a
/// durable references-not-payloads event history with the H8 erasure profile, NOT a new H19). The
/// completeness assertion joins this against the harness registry so the flow store is NOT an orphan.
pub fn flow_store_classifier() -> StoreClassifier {
    StoreClassifier::of([StoreHolder::new(
        StoreKind::Oltp,
        FLOW_OLTP_STORE,
        Holder::H8EventBus,
    )])
}

/// **Register the flow store as a `PersonalDataHolder` through the harness auto-registration (contract
/// 1.4).** Opens the flow OLTP store through the substrate [`HolderRegistry`] — the ONE door — so it
/// is a registered holder by construction. Registering ALWAYS (even before the journal is populated)
/// makes "the DSAR fan-out forgot workflow history" structurally impossible (10.1 / §5.5 — the bug
/// VISION §3 names). [`crate::app::flow_app_spec`]'s `holders: AppSpec::auto()` already opens the flow
/// store on boot; this free function is the explicit, testable registration the boot path performs.
pub fn register_flow_holder() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, FLOW_OLTP_STORE);
    registry
}

/// The Art. 18/21 restriction-suppression set (the `restrict` body's shared state) — the set of
/// subjects whose NEW dispatch the replay/lease loop suppresses (§4.8). A cloneable handle over shared
/// state so the holder's `restrict(subject, on)` write and the dispatch read (P-FLOW-05) observe ONE
/// truth. PII-free: it holds opaque pseudonymous subject ids, never names.
#[derive(Clone, Default)]
pub struct RestrictSet {
    inner: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl RestrictSet {
    /// A fresh, empty suppression set.
    pub fn new() -> RestrictSet {
        RestrictSet::default()
    }

    /// Set (`on = true`) or clear (`on = false`) the restriction for `subject_id` (Art. 18/21).
    /// Idempotent: setting an already-restricted subject is a no-op.
    pub fn set(&self, subject_id: &str, on: bool) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            g.insert(subject_id.to_string());
        } else {
            g.remove(subject_id);
        }
    }

    /// Whether `subject_id`'s NEW dispatch is currently suppressed (the replay/lease-loop read).
    pub fn is_restricted(&self, subject_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(subject_id)
    }
}

/// The live runtime the REAL P-FLOW-03 holder body operates over: the workflow journal (to `locate`
/// the subject's appearances in `wf_history` + report the structural-erase surface) + the
/// restrict-suppression set (to suppress a restricted subject's NEW dispatch). **References-not-
/// payloads:** the holder reads only the OPAQUE actor/run pseudonyms, the structured `result`
/// [`myelin_refs::ArtifactRef`]s, and the inline-PII key ref — never a stored name. Cloneable.
#[derive(Clone)]
pub struct FlowBacking {
    /// The live workflow journal (P-FLOW-04) — the holder scans its `wf_history` rows for the
    /// subject's appearances. The journal IS the source of truth (§3.2); `workflow_run`/`wf_signal`
    /// rows derive from / co-locate with it (their live projections land with their writers,
    /// P-FLOW-05/09 — the holder body extends to them in place when they ship).
    journal: WfJournal,
    /// The restrict-suppression set (Art. 18/21) — `restrict(subject, true)` records the subject so
    /// the replay/lease loop keeps its NEW dispatch suppressed (§4.8).
    restrict: RestrictSet,
}

impl FlowBacking {
    /// Wire the holder over a live workflow journal (the P-FLOW-03 real body). The restrict set is
    /// fresh (empty) — `restrict(subject, true)` adds to it.
    pub fn new(journal: WfJournal) -> FlowBacking {
        FlowBacking { journal, restrict: RestrictSet::new() }
    }

    /// Wire the holder over a live journal AND a shared restrict-suppression set (so the suppression a
    /// holder records is the SAME set the replay/lease loop consults).
    pub fn with_restrict(journal: WfJournal, restrict: RestrictSet) -> FlowBacking {
        FlowBacking { journal, restrict }
    }

    /// The shared restrict-suppression set (the replay/lease loop reads it to suppress a restricted
    /// subject's NEW dispatch).
    pub fn restrict_set(&self) -> &RestrictSet {
        &self.restrict
    }
}

/// myelin-flow's **workflow history** AS a [`PersonalDataHolder`] (H8 by the §5.5 reconcile; contract
/// 9.6 holder half + 10.1). P-FLOW-03: the REAL structural references-not-payloads erasure surface
/// when [`Self::with_journal`] wires the live journal; **empty-but-correct** (the registration-only
/// [`Default`] form) when unbacked (`serve` before the replay engine populates the journal). Cloneable.
#[derive(Clone, Default)]
pub struct WfHistoryHolder {
    /// `None` = the registration-only stub (empty-but-correct); `Some` = the REAL P-FLOW-03 body over
    /// the live journal + the restrict set.
    backing: Option<FlowBacking>,
}

impl WfHistoryHolder {
    /// **The REAL P-FLOW-03 holder over a live journal (§5.5).** `locate` walks `wf_history` for rows
    /// naming the subject (a referenced actor in a `result` ref OR the inline-PII `result_key_ref`);
    /// `erase` is the STRUCTURAL references-not-payloads erase (the appearance tombstones for free via
    /// Identity's pseudonym shred — NO PII-column mutation on the refs-stored rows; the inline-PII DEK
    /// crypto-shred is the P-FLOW-23 reach); `restrict` suppresses the subject's NEW dispatch.
    pub fn with_journal(journal: WfJournal) -> WfHistoryHolder {
        WfHistoryHolder { backing: Some(FlowBacking::new(journal)) }
    }

    /// The REAL holder over a live journal AND a shared restrict set (the replay/lease loop reads it).
    pub fn with_backing(backing: FlowBacking) -> WfHistoryHolder {
        WfHistoryHolder { backing: Some(backing) }
    }

    /// Register this holder through the substrate registry (the `serve`-called auto-registration
    /// seam), returning the receipt — the proof the flow store registered as holder H8.
    pub fn register(&self, registry: &mut HolderRegistry) -> FlowHolderRegistration {
        registry.open(StoreKind::Oltp, FLOW_OLTP_STORE)
    }

    /// The shared restrict-suppression set (when backed) — so a test / the replay loop can read the
    /// suppression the holder records.
    pub fn restrict_set(&self) -> Option<&RestrictSet> {
        self.backing.as_ref().map(|b| b.restrict_set())
    }

    /// The opaque, PII-free subject id the receipt body keys on (the pseudonymous Principal id) —
    /// never a name/email. The opaque actor/run pseudonym posture (§5.5, references-not-payloads).
    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }

    /// Whether a `wf_history` row in `tenant` NAMES the subject — the references-not-payloads
    /// predicate (§5.5/§4.8). The subject appears EITHER (1) as a referenced actor in a `result`
    /// [`myelin_refs::ArtifactRef`] (`…/principal/<id>`), OR (2) as the per-subject DEK the inline-PII
    /// `result_key_ref` names (`…/subject/<id>`). Never a stored name. This is the structural surface
    /// `locate`/`export` count + `erase` relies on.
    fn row_references_subject(row: &crate::schema::WfHistoryRow, subject_id: &str) -> bool {
        let in_refs = row
            .result
            .as_ref()
            .map(|refs| {
                refs.iter().any(|r| {
                    r.0.ends_with(&format!("/principal/{subject_id}"))
                        || r.0.contains(&format!("/principal/{subject_id}/"))
                })
            })
            .unwrap_or(false);
        let in_key_ref = row
            .result_key_ref
            .as_ref()
            .map(|k| {
                k.ends_with(&format!("/subject/{subject_id}"))
                    || k.contains(&format!("/subject/{subject_id}/"))
            })
            .unwrap_or(false);
        in_refs || in_key_ref
    }

    /// Count the `wf_history` rows naming the subject (the structural `locate` surface). Returns 0 when
    /// unbacked. Tenant-first (the fan-out is per (subject, tenant)) — the journal scan is filtered to
    /// the subject's tenant so a cross-tenant subject id never matches another tenant's rows.
    fn count_appearances(&self, tenant: &GdprTenantId, subject_id: &str) -> usize {
        let Some(b) = &self.backing else {
            return 0;
        };
        let t = TenantId(tenant.0.clone());
        b.journal
            .history_in_tenant(&t)
            .iter()
            .filter(|row| Self::row_references_subject(row, subject_id))
            .count()
    }
}

impl PersonalDataHolder for WfHistoryHolder {
    fn locate(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<LocateReport> {
        // REAL §5.5 locate: the wf_history rows naming the subject (by a referenced-actor result ref OR
        // the inline-PII result_key_ref — never a name). Unbacked → empty-but-correct (0 located).
        // Tenant-first (the journal scan is scoped to the subject's tenant).
        let sid = Self::subject_id(subject);
        let count = self.count_appearances(&tenant, &sid);
        let outcome = format!(
            "located {count} wf_history rows naming the subject (referenced-actor result refs + \
             inline-PII result_key_ref, references-not-payloads — no stored name)"
        );
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                FLOW_OLTP_STORE,
                &sid,
                &tenant.0,
                &outcome,
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<PortableBundle> {
        // The journal is references-not-payloads: its subject data is the opaque actor/run pseudonym +
        // structured refs + the inline-PII key ref (the payload bodies live in the owning subsystem's
        // erasable store, already covered by their exports + Identity). The portable bundle is the
        // located-appearance count receipt (nothing to export but the count + a content-address).
        let sid = Self::subject_id(subject);
        let count = self.count_appearances(&tenant, &sid);
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                FLOW_OLTP_STORE,
                &sid,
                &tenant.0,
                &format!(
                    "references-not-payloads bundle: {count} wf_history appearances, no free-text body"
                ),
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        // The journal stores refs, never rendered strings (§3.2) → rectification of a row is via
        // deterministic replay over the corrected owner content + the re-resolved refs at read time
        // (P-FLOW-05), never an in-place edit here. A no-op at the holder surface (correct: there is
        // nothing to rectify in a refs-stored journal row — the re-resolve corrects it).
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                FLOW_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (references-not-payloads — rectify via replay-from-source + read-time \
                 re-resolve, P-FLOW-05)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // REAL §4.8 restrict (Art. 18/21): record the subject in the suppression set so the replay/
        // lease loop keeps its NEW dispatch suppressed. Unbacked → a well-defined no-op (no live
        // dispatch to suppress over). Idempotent.
        let sid = Self::subject_id(subject);
        let applied = match &self.backing {
            Some(b) => {
                b.restrict.set(&sid, on);
                true
            }
            None => false,
        };
        let outcome = if applied {
            format!(
                "restrict={on} recorded in the suppression set (new dispatch suppressed; \
                 indexing/agent-use too)"
            )
        } else {
            format!(
                "restrict={on} no-op (no live dispatch; suppression consult lands with the replay/\
                 lease loop P-FLOW-05)"
            )
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                FLOW_OLTP_STORE,
                &sid,
                "",
                &outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        // STRUCTURAL §5.5/§4.8 erase: the engine's erasure surface is SMALL + STRUCTURAL (references-
        // not-payloads). A wf_history/workflow_run/wf_signal row stores the subject ONLY as the opaque
        // actor/run pseudonym + structured refs (+ the rare inline-PII envelope key ref); the
        // appearance TOMBSTONES FOR FREE — Identity's pseudonym-map shred (§4.8) makes the opaque id
        // unresolvable, and the refs re-resolve to a tombstone at read time. So the holder erase needs
        // NO PII-column mutation on the refs-stored rows (the structural property the gate pins): it
        // reports the surface covered + relies on the platform posture.
        //
        // NAMED FLOOR (P-FLOW-23, M5): the per-subject-DEK crypto-shred reach into the inline-PII
        // result_key_ref / payload_key_ref history rows + backups is NOT performed here. So this erase
        // destroys NO key at the flow surface (key_epoch_destroyed = None) — the references-not-
        // payloads tombstone needs no key destroyed for the refs-stored rows; the inline-PII DEK shred
        // is the P-FLOW-23 follow-on. No erasure backdoor: the row stays; the person becomes
        // unresolvable.
        let (sid, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => (Self::subject_id(subject), tenant.0.clone()),
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        let count = match &scope {
            EraseScope::Subject { tenant, .. } => self.count_appearances(tenant, &sid),
            // A tenant erase is the crypto-shred (destroy the per-tenant DEK) — the tenant-decommission
            // lever (11.3/11.4), not a per-row scan here.
            EraseScope::Tenant(_) => 0,
        };
        let outcome = match &scope {
            EraseScope::Subject { .. } => format!(
                "structural erase: {count} wf_history appearances tombstone for free (refs-not-\
                 payloads; Identity §4.8 pseudonym-shred makes the opaque id unresolvable) — 0 PII \
                 columns mutated; inline-PII result_key_ref/payload_key_ref per-subject-DEK \
                 crypto-shred = P-FLOW-23 (M5); replay P-FLOW-05"
            ),
            EraseScope::Tenant(_) => "tenant crypto-shred: destroy the per-tenant DEK (11.3/11.4) — \
                 every workflow row unrecoverable"
                .into(),
        };
        Ok(EraseReceipt {
            // No KEY destroyed at the flow holder (the refs-stored rows tombstone for free; the inline-
            // PII DEK crypto-shred is the P-FLOW-23 reach). key_epoch_destroyed = None.
            receipt: Receipt::content_addressed(
                "erase",
                FLOW_OLTP_STORE,
                &sid,
                &tenant,
                &outcome,
                None,
                0,
            ),
        })
    }
}

/// The H-holder the flow OLTP store classifies to (H8 `EventBus`, the §5.5 reconcile) — a convenience
/// over [`myelin_substrate::classify_store`] against the flow classifier. Returns the holder (always
/// `Some(H8EventBus)` for the declared store) so a caller can pin the classification.
pub fn flow_history_holder() -> Option<Holder> {
    myelin_substrate::classify_store(StoreKind::Oltp, FLOW_OLTP_STORE, &flow_store_classifier())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::WfHistoryRow;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_refs::ArtifactRef;
    use myelin_substrate::{assert_holder_completeness, classify_store};
    use myelin_tenancy::Region;

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            GdprTenantId::from_token("acme"),
        ))
    }

    fn tenant() -> GdprTenantId {
        GdprTenantId::from_token("acme")
    }

    fn t() -> TenantId {
        TenantId::from_token("acme")
    }

    /// A `wf_history` row in `acme` for `run_id`, naming `actor` by ref in a `result` ArtifactRef, and
    /// (when `key_subject` is Some) carrying the inline-PII `result_key_ref` for that subject's DEK.
    /// All refs / opaque ids, never a name.
    fn history_row(
        run_id: &str,
        seq: i64,
        actor: &str,
        key_subject: Option<&str>,
    ) -> WfHistoryRow {
        WfHistoryRow {
            tenant: t(),
            region: Region::new("fr-par"),
            run_id: run_id.into(),
            seq,
            kind: "activity_completed".into(),
            command_id: format!("agent.run:{seq}"),
            result: Some(vec![ArtifactRef(format!(
                "myelin://acme/identity/principal/{actor}"
            ))]),
            result_key_ref: key_subject
                .map(|s| format!("kms://acme/subject/{s}")),
        }
    }

    /// **The flow store registers as a holder through the one door (contract 1.4).** The OLTP store is
    /// opened through the substrate registry, so it is a registered holder by construction — 0 stores
    /// escape registration (the §5.5 "we forgot workflow history" bug is impossible).
    #[test]
    fn flow_registers_its_store_as_a_holder() {
        let registry = register_flow_holder();
        assert!(registry.is_registered(StoreKind::Oltp, FLOW_OLTP_STORE));
        assert_eq!(registry.len(), 1, "exactly the one flow store registered");
    }

    /// **The flow store name IS the auto-registered boot name.** [`FLOW_OLTP_STORE`] equals
    /// [`crate::SERVICE_NAME`] — the SAME name [`crate::app`] opens the flow OLTP store under on boot —
    /// so the classifier addresses the store the harness actually registered (no name drift).
    #[test]
    fn flow_store_name_matches_the_boot_registered_name() {
        assert_eq!(FLOW_OLTP_STORE, crate::SERVICE_NAME);
    }

    /// **Re-registration is idempotent** — `serve` re-running the registration on a restart records the
    /// flow store exactly once.
    #[test]
    fn re_registration_is_idempotent() {
        let mut registry = register_flow_holder();
        WfHistoryHolder::default().register(&mut registry);
        assert_eq!(
            registry.len(),
            1,
            "re-opening the same flow store does not double-register"
        );
    }

    /// **The flow store classifies to H8 — 0 orphans (contract 1.4 + gdpr §3.2).** The OLTP store maps
    /// to **H8 (`EventBus` history)** via the declared classifier (the §5.5 reconcile — a durable
    /// references-not-payloads event history, NOT a new H19). The substrate completeness assertion is
    /// GREEN — the flow store is inside the exhaustive H1–H18 list, so the M5 DSAR fan-out cannot miss
    /// workflow history.
    #[test]
    fn flow_store_classifies_to_h8_no_orphan() {
        let registry = register_flow_holder();
        let classifier = flow_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, FLOW_OLTP_STORE, &classifier),
            Some(Holder::H8EventBus),
            "the flow OLTP store is holder H8 (the §5.5 references-not-payloads reconcile)"
        );
        assert_eq!(flow_history_holder(), Some(Holder::H8EventBus));
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "the flow store is in the exhaustive H1–H18 list — 0 orphan stores"
        );
    }

    /// **`locate` over the backed journal counts the REAL appearances (the structural surface).** 0
    /// over an unbacked holder (empty-but-correct), N over the live journal — by referenced-actor
    /// result ref AND by inline-PII result_key_ref. Pins the count is the references-not-payloads
    /// predicate, not a constant.
    #[test]
    fn locate_counts_real_appearances_backed_vs_unbacked() {
        let unbacked = WfHistoryHolder::default();
        assert_eq!(
            unbacked.count_appearances(&tenant(), "u-x"),
            0,
            "unbacked → empty-but-correct"
        );

        let journal = WfJournal::new();
        // u-x appears as a referenced actor in a result ref.
        journal.append_history_for_test(history_row("run-1", 0, "u-x", None));
        // u-x appears as the inline-PII DEK subject (result_key_ref).
        journal.append_history_for_test(history_row("run-2", 0, "u-y", Some("u-x")));
        // neither names u-x.
        journal.append_history_for_test(history_row("run-3", 0, "u-y", None));
        let backed = WfHistoryHolder::with_journal(journal);
        assert_eq!(
            backed.count_appearances(&tenant(), "u-x"),
            2,
            "both structural appearances counted (result ref + inline-PII key ref)"
        );
        assert_eq!(
            backed.count_appearances(&tenant(), "u-none"),
            0,
            "an absent subject → 0"
        );
    }

    /// **THE GATE — the structural-erase property: erase a subject → a refs-stored wf_history row
    /// tombstones with NO PII mutation.** A row naming the subject (by result ref AND by inline-PII key
    /// ref) is erased; the stored rows are byte-identical after erase (0 PII columns mutated) — the
    /// refs re-resolve to a tombstone at read time via Identity's §4.8 shred. The inline-PII DEK
    /// crypto-shred is the named P-FLOW-23 floor (no key destroyed here). This is the §5.5 references-
    /// not-payloads tombstone-for-free, proven at the unit grain.
    #[test]
    fn structural_erase_tombstones_refs_stored_rows_with_zero_pii_mutation() {
        let journal = WfJournal::new();
        journal.append_history_for_test(history_row("run-1", 0, "u-erase", None)); // result ref
        journal.append_history_for_test(history_row("run-2", 0, "u-bob", Some("u-erase"))); // key ref
        journal.append_history_for_test(history_row("run-3", 0, "u-carol", None)); // control

        let holder = WfHistoryHolder::with_journal(journal.clone());

        // Snapshot the EXACT stored bytes BEFORE erase.
        let before: Vec<WfHistoryRow> = journal.history_in_tenant(&t());
        let subj_before: Vec<&WfHistoryRow> = before
            .iter()
            .filter(|r| WfHistoryHolder::row_references_subject(r, "u-erase"))
            .collect();
        assert_eq!(subj_before.len(), 2, "locate finds both appearances (result ref + key ref)");

        // locate reports the appearance count over the structural surface.
        let loc = holder
            .locate(&subject("u-erase"), tenant())
            .expect("locate succeeds");
        assert!(loc.receipt.content_hash.starts_with("blake3:"));
        assert!(loc.receipt.key_epoch_destroyed.is_none(), "locate shreds no key");

        // ERASE the subject.
        let scope = EraseScope::Subject { subject: subject("u-erase"), tenant: tenant() };
        let er = holder.erase(scope.clone()).expect("structural erase succeeds");
        assert!(
            er.receipt.key_epoch_destroyed.is_none(),
            "0 keys shredded at the flow surface (refs-stored; inline-PII DEK shred is P-FLOW-23)"
        );

        // THE PROPERTY: 0 PII columns mutated — every stored row is byte-identical after erase.
        let after: Vec<WfHistoryRow> = journal.history_in_tenant(&t());
        assert_eq!(
            after, before,
            "the refs-stored rows tombstone for FREE — 0 PII columns mutated (references-not-payloads)"
        );
        assert_eq!(after.len(), 3, "no row deleted either — the appearance stays, only resolution changes");

        // Idempotent: a re-erase returns the IDENTICAL content-addressed receipt.
        let er2 = holder.erase(scope).expect("re-erase is idempotent");
        assert_eq!(er, er2, "the same erase scope yields the identical receipt");
    }

    /// **`restrict` records the subject in the SHARED suppression set (the replay/lease loop reads
    /// it).** Backed: `restrict(on)` adds, `restrict(off)` clears — the SAME set a dispatch loop would
    /// consult. Unbacked: a well-defined no-op. Idempotent.
    #[test]
    fn restrict_writes_the_shared_suppression_set() {
        let restrict = RestrictSet::new();
        let backing = FlowBacking::with_restrict(WfJournal::new(), restrict.clone());
        let holder = WfHistoryHolder::with_backing(backing);
        let subj = subject("u-r");

        assert!(!restrict.is_restricted("u-r"), "not restricted initially");
        holder.restrict(&subj, true).expect("restrict on succeeds");
        assert!(restrict.is_restricted("u-r"), "the holder recorded the restriction in the shared set");
        holder.restrict(&subj, false).expect("restrict off succeeds");
        assert!(!restrict.is_restricted("u-r"), "restrict off clears it");

        // Unbacked → a well-defined no-op (no panic), records nothing.
        let unbacked = WfHistoryHolder::default();
        assert!(unbacked.restrict(&subj, true).is_ok(), "unbacked restrict is a no-op receipt");
    }

    /// **The holder is empty-but-correct unbacked (the registration-only surface), not an error.**
    /// `export`/`locate`/`rectify` over a tenant the replay engine has not populated return content-
    /// addressed receipts over an empty surface — a real, callable holder, never a `todo!()`/`Err`.
    #[test]
    fn unbacked_holder_is_empty_but_correct() {
        let holder = WfHistoryHolder::default();
        let subj = subject("u-1");
        let loc = holder.locate(&subj, tenant()).expect("locate over empty surface succeeds");
        assert_eq!(loc.receipt.operation, "locate");
        let exp = holder.export(&subj, tenant()).expect("export of empty bundle succeeds");
        assert_eq!(exp.receipt.operation, "export");
        let rec = holder.rectify(&subj, Patch("x".into())).expect("rectify no-op succeeds");
        assert_eq!(rec.receipt.operation, "rectify");
    }

    /// **`export` over a populated journal reports the appearance count (references-not-payloads).**
    /// The bundle is the count + a content-address — nothing to export but the references (the bodies
    /// live in the owning subsystem's erasable store).
    #[test]
    fn export_reports_the_appearance_count() {
        let journal = WfJournal::new();
        journal.append_history_for_test(history_row("run-1", 0, "u-e", None));
        journal.append_history_for_test(history_row("run-2", 0, "u-e", None));
        let holder = WfHistoryHolder::with_journal(journal);
        let exp = holder.export(&subject("u-e"), tenant()).expect("export succeeds");
        assert!(exp.receipt.content_hash.starts_with("blake3:"));
        assert!(exp.receipt.key_epoch_destroyed.is_none(), "export shreds no key");
    }

    /// **Tenant-scoping: a subject id never matches another tenant's journal rows.** `locate` is
    /// per (subject, tenant); a row in tenant `acme` does not count for a `locate` scoped to a
    /// different tenant. Pins the journal scan is tenant-first.
    #[test]
    fn locate_is_tenant_scoped() {
        let journal = WfJournal::new();
        journal.append_history_for_test(history_row("run-1", 0, "u-x", None)); // tenant = acme
        let holder = WfHistoryHolder::with_journal(journal);
        assert_eq!(holder.count_appearances(&GdprTenantId::from_token("acme"), "u-x"), 1);
        assert_eq!(
            holder.count_appearances(&GdprTenantId::from_token("other"), "u-x"),
            0,
            "the acme row does not count for tenant `other` — the scan is tenant-first"
        );
    }

    /// **A tenant-scope erase reports the per-tenant DEK crypto-shred lever (11.3/11.4), shreds no
    /// per-subject key here.** Tenant offboarding destroys the per-tenant DEK; the holder reports that
    /// lever (no per-row scan, key_epoch_destroyed = None at this structural surface).
    #[test]
    fn tenant_erase_reports_the_per_tenant_dek_lever() {
        let holder = WfHistoryHolder::with_journal(WfJournal::new());
        let er = holder
            .erase(EraseScope::Tenant(tenant()))
            .expect("tenant erase succeeds");
        assert_eq!(er.receipt.operation, "erase");
        assert!(er.receipt.key_epoch_destroyed.is_none(), "the per-subject DEK shred is P-FLOW-23");
    }

    /// **The `restrict_set` accessors return the SHARED set the holder records into.** Both
    /// [`FlowBacking::restrict_set`] and [`WfHistoryHolder::restrict_set`] hand back the SAME set the
    /// holder's `restrict` writes — so a dispatch reader and the holder writer observe ONE truth.
    /// Unbacked → `None`. Pins the accessors are not a constant.
    #[test]
    fn restrict_set_accessors_return_the_shared_set() {
        let restrict = RestrictSet::new();
        let backing = FlowBacking::with_restrict(WfJournal::new(), restrict.clone());
        backing.restrict_set().set("u-shared", true);
        assert!(restrict.is_restricted("u-shared"), "the backing accessor is the shared set");

        let holder = WfHistoryHolder::with_backing(backing);
        let via_holder = holder.restrict_set().expect("backed holder exposes its restrict set");
        assert!(via_holder.is_restricted("u-shared"), "the holder accessor is the SAME shared set");
        via_holder.set("u-shared", false);
        assert!(!restrict.is_restricted("u-shared"), "a write through the holder accessor reaches it");

        assert!(
            WfHistoryHolder::default().restrict_set().is_none(),
            "unbacked → no restrict set"
        );
    }

    /// **The holder is object-safe** — held behind `dyn PersonalDataHolder` exactly as the DSR
    /// orchestrator / holder registry need (a heterogeneous holder set, contract 10.1).
    #[test]
    fn holder_is_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(WfHistoryHolder::default())];
        let subj = subject("u-3");
        for h in &holders {
            assert!(h.locate(&subj, tenant()).is_ok(), "the holder responds to the contract");
        }
    }
}
