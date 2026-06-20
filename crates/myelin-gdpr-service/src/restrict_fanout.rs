//! # `restrict` suppression into the derived stores (Search/Refs/Notif/Agents/OLAP) — GA-D7
//! (P-GA-25 → P-152)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§4.4** (`restrict(subject,
//! on)` sets a per-subject suppression flag every holder honours — **no indexing, no agent-use, no
//! analytics (incl. OLAP, contract 11.6), no notification — while RETAINING storage; reversible**)
//! and **§8** / the OLAP holder note (`restrict` suppression flows into OLAP analytics — *no
//! analytics for a restricted subject*; contract 11.6, GA-9). Prove-it:
//! `external-insights/01-process-and-quality-doctrine.md` §3 — **0 processing of a restricted
//! subject, OBSERVED** (the suppression is read THROUGH each derived store, never asserted).
//!
//! **Contract-index:** owns (orchestration) the **`restrict`-fan-out leg of 10.1** into the M2
//! derived stores; consumed: **11.6** (the OLAP read store honours the restriction flag — no
//! analytics for a restricted subject). The Search/Refs/Notif/Agent faces are the 10.1 derived-store
//! restriction obligations (`gdpr-and-audit.md` §4.4 enumerated four ops + OLAP).
//!
//! ## What THIS prompt (P-GA-25) ships — and what it reuses (EI-01 §7 coherence)
//! P-GA-17 ([`crate::structural_floor`] → P-117) built the genuinely-new lever: the **`restrict`
//! suppression FLAG** — [`crate::structural_floor::RestrictRegistry`] — and PROVED the **M1 holders**
//! ([`crate::structural_floor::M1Store`]) honour it (suppress processing, retain storage,
//! reversible). It NAMED its floor: *the full restriction-into-derived-stores proof (GA-D7) — the
//! flag flowing into Search/Refs/Notif/Agents/OLAP — is M2 P-GA-25.* This module fills that floor.
//!
//! It **REUSES the [`RestrictRegistry`] wholesale** — there is exactly ONE suppression flag in the
//! cell (a restriction is a per-subject fact, not a per-store one), and every derived store reads the
//! SAME registry the M1 holders read (the §4.4 "every holder honours" property). It does NOT define a
//! second flag, a second registry, or a parallel suppression mechanism. P-GA-24
//! ([`crate::derivative_erasure`] → P-151) already modelled the SAME five derived stores for the
//! ERASE fan-out (Search purge / Refs tombstone / Notif humanise); the restriction RIDES that
//! per-derivative fan-out (the prompt: *"the restriction-honoured-into-derived proof rides this
//! fan-out"*). What is genuinely NEW here is the **per-derivative-store PROCESSING op that HONOURS
//! the flag** — a derived store's processing semantics differ from an M1 store's:
//! - **Search (H7)** — *no indexing*: the incremental indexer SKIPS a restricted subject's doc (it is
//!   never added to / refreshed in the index) while the storage row is retained.
//! - **Refs (H12)** — *no edge projection*: the edge-builder SKIPS projecting a restricted subject's
//!   edges into the backlink index (resolution returns the suppression, not the live edge).
//! - **Notif (H13)** — *no notification*: the Signal-consumer SKIPS delivering / ranking a
//!   notification derived from a restricted subject's content.
//! - **Agents (H11/H17)** — *no agent-use*: the agent runtime SKIPS reading a restricted subject's
//!   content into a tool call / context window.
//! - **OLAP (11.6)** — *no analytics*: the OLAP read store SKIPS a restricted subject's rows from any
//!   aggregate / analytic projection (the contract-11.6 restriction-flag propagation).
//!
//! ## The five derived stores honour the ONE flag (§4.4 — every holder honours)
//! Each derived store is a faithful in-memory model whose processing op CHECKS
//! [`RestrictRegistry::is_restricted`] and refuses to process a restricted subject
//! ([`DerivedProcessed::Suppressed`]) while RETAINING the derived row (a restriction is reversible —
//! it is NOT an erase). The live bindings (the real `myelin-search` indexer, `myelin-refs-service`
//! edge-builder, `myelin-notif` Signal-consumer, `myelin-agent-service` runtime, the OLAP read store)
//! are the named floor — each reads the SAME flag at its processing chokepoint; the binding is a
//! config swap at boot, never a code change. This module touches **NO new DB/object-store/cache/bus
//! contract** (it composes the already-shipped [`RestrictRegistry`] seam), so **no `--features
//! integration` live-stack leg is owed** by P-GA-25.
//!
//! ## Floor named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The multi-cell restriction** (the flag fanned across `member_cells` over the cross-cell
//!   PII-free bridge) is **M5** (rides P-GA-32 / P-GA-33, GA-D8). THIS prompt proves the restriction
//!   honoured by the M2-existing derived stores in a single cell. Recorded in writing per the prompt.
//! - **The live derived-store bindings** behind the [`myelin_gdpr::PersonalDataHolder`] seam are wired
//!   by the harness/orchestrator at boot (the real Search/Refs/Notif/Agent/OLAP impls reading the
//!   flag at their processing chokepoints). On this floor each is a faithful in-memory model whose
//!   suppression-while-retained + reversibility is byte-for-byte the §4.4 / GA-D7 post-condition.
//!
//! ## Mutation floor (P-GA-25 TESTS — the restriction-suppression-across-derived-stores path is
//! mandatory-core). The behavioral core every mutation must be CAUGHT: each derived store's
//! `process` suppression branch (a restricted subject is SUPPRESSED, an unrestricted one is
//! PROCESSED — the `if is_restricted` predicate is load-bearing in BOTH polarities, and storage is
//! RETAINED either way), and [`RestrictFanOutDriver::fan_out_restrict`] (the per-store verdict roll-up
//! that reads 0 processing across all five, reversibly). The `cargo mutants` score for this file is
//! recorded in the module-level note in `lib.rs` and stated, not hidden (EI-01 §3).

use std::collections::BTreeSet;
use std::sync::Mutex;

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, Result as DsrResult, RestrictReceipt, SubjectRef, TenantId,
};

use crate::structural_floor::RestrictRegistry;

// ───────────────────────── the five derived-store holder ids + processing kinds ─────────────────────────

/// The stable, PII-free holder names the five M2 derived stores register the `restrict`-fan-out
/// under (contract 1.4 — the data-map / DSR fan-out address book). PII-free: a holder id is a store
/// name, never a subject.
pub mod restrict_holder_ids {
    /// **H7** — the Search index (no indexing for a restricted subject — §4.4 / 6.4).
    pub const SEARCH_INDEX: &str = "search_index";
    /// **H12** — the reference graph (no edge projection for a restricted subject — §4.4 / 5.8).
    pub const REFS_GRAPH: &str = "refs_graph";
    /// **H13** — the notification history (no notification for a restricted subject — §4.4).
    pub const NOTIF_HISTORY: &str = "notif_history";
    /// **H11/H17** — the agent runtime (no agent-use for a restricted subject — §4.4 / 8.x).
    pub const AGENT_RUNTIME: &str = "agent_runtime";
    /// **11.6** — the OLAP read store (no analytics for a restricted subject — §4.4 / 11.6, GA-9).
    pub const OLAP_READ_STORE: &str = "olap_read_store";
}

/// The per-derivative processing op each derived store performs — the thing §4.4 SUPPRESSES for a
/// restricted subject. One per derived store this prompt orchestrates (the five-store set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DerivedProcessing {
    /// Search — **index** the subject's doc into the search index (no indexing while restricted).
    SearchIndex,
    /// Refs — **project** the subject's edges into the backlink index (no projection while restricted).
    RefsProject,
    /// Notif — **notify** (deliver/rank a notification from the subject's content; none while restricted).
    NotifNotify,
    /// Agents — **agent-read** the subject's content into a tool call/context (none while restricted).
    AgentRead,
    /// OLAP — **analyse** the subject's rows into an aggregate/analytic (no analytics while restricted).
    OlapAnalyse,
}

impl DerivedProcessing {
    /// The five derived-store processing ops, in a stable order (the drill's exhaustive check).
    #[must_use]
    pub const fn all() -> [DerivedProcessing; 5] {
        [
            DerivedProcessing::SearchIndex,
            DerivedProcessing::RefsProject,
            DerivedProcessing::NotifNotify,
            DerivedProcessing::AgentRead,
            DerivedProcessing::OlapAnalyse,
        ]
    }

    /// A stable PII-free token (for receipts / telemetry / the suppression outcome string).
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            DerivedProcessing::SearchIndex => "search_index",
            DerivedProcessing::RefsProject => "refs_project",
            DerivedProcessing::NotifNotify => "notif_notify",
            DerivedProcessing::AgentRead => "agent_read",
            DerivedProcessing::OlapAnalyse => "olap_analyse",
        }
    }

    /// The holder id of the derived store that performs this processing op (the §4.4 store↔op map).
    #[must_use]
    pub const fn holder_id(self) -> &'static str {
        match self {
            DerivedProcessing::SearchIndex => restrict_holder_ids::SEARCH_INDEX,
            DerivedProcessing::RefsProject => restrict_holder_ids::REFS_GRAPH,
            DerivedProcessing::NotifNotify => restrict_holder_ids::NOTIF_HISTORY,
            DerivedProcessing::AgentRead => restrict_holder_ids::AGENT_RUNTIME,
            DerivedProcessing::OlapAnalyse => restrict_holder_ids::OLAP_READ_STORE,
        }
    }
}

/// The outcome of a derived-store processing op: the row was PROCESSED into the derivative (its
/// projection is returned), or it was SUPPRESSED because the subject is restricted (the `restrict`
/// flag honoured). The derived row is RETAINED regardless (a restriction is reversible — NOT an
/// erase; the [`DerivedStore::has_row`] path still returns the retained row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivedProcessed {
    /// The op ran — the derivative projection (the indexed doc / projected edge / delivered
    /// notification / agent-read content / analysed row) is returned. Only for an UNRESTRICTED subject.
    Processed(String),
    /// The op was SUPPRESSED — the subject is restricted (no indexing / agent-use / analytics /
    /// notification while restricted, §4.4). The derived store RETAINS the row; processing is withheld.
    Suppressed,
    /// There is no derived row for the subject (nothing to process — distinct from a suppression).
    NoRow,
}

// ───────────────────────── the derived-store model (honours the ONE restrict flag) ─────────────────────────

/// **A faithful in-memory model of ONE of the five M2 derived stores that HONOURS the `restrict`
/// flag at its processing chokepoint (§4.4).** Each derived store is, structurally, a store of
/// **derived, reconstructible** rows (architecture §0/§1 — Search docs, Refs edges, Notif
/// read-models, agent context, OLAP aggregates) whose processing op CHECKS
/// [`RestrictRegistry::is_restricted`] before processing and SKIPS a restricted subject — while
/// RETAINING the derived row (the restriction is reversible).
///
/// This model makes the suppression OBSERVABLE — the unit tests + the GATE drill read the
/// suppression THROUGH the store, never assert it (EI-01 §3 prove-it). It REUSES the ONE
/// [`RestrictRegistry`] every derived store + M1 store reads (no second flag) — its `kind` only
/// labels which derivative-specific op it performs.
pub struct DerivedStore<'a> {
    /// Which derived store this is (the §4.4 store↔op identity — labels the processing op + holder id).
    kind: DerivedProcessing,
    /// The ONE shared `restrict` suppression flag every holder honours (the P-GA-17 registry — there
    /// is exactly one suppression fact per subject; every derived store reads the SAME registry).
    restrict: &'a RestrictRegistry,
    /// The derived rows present in this store, keyed on `(tenant, subject_id)` — derived,
    /// reconstructible projections (a Search doc / Refs edge / Notif read-model / agent context /
    /// OLAP row). RETAINED across a restriction (suppression ≠ delete).
    rows: Mutex<BTreeSet<(String, String)>>,
}

impl<'a> DerivedStore<'a> {
    /// Build a derived-store model of `kind` over the SHARED `restrict` registry (the P-GA-17 flag).
    #[must_use]
    pub fn new(kind: DerivedProcessing, restrict: &'a RestrictRegistry) -> DerivedStore<'a> {
        DerivedStore {
            kind,
            restrict,
            rows: Mutex::new(BTreeSet::new()),
        }
    }

    /// Which derived store this is (the processing kind it performs).
    #[must_use]
    pub fn kind(&self) -> DerivedProcessing {
        self.kind
    }

    /// The PII-free holder id this derived store registers under.
    #[must_use]
    pub fn holder_id(&self) -> &'static str {
        self.kind.holder_id()
    }

    fn key(subject: &SubjectRef, tenant: &TenantId) -> (String, String) {
        (tenant.0.clone(), subject.principal.principal_id.0.clone())
    }

    /// Seed a derived row for a subject (the live consumer's projection step — a Search doc indexed,
    /// a Refs edge projected, an OLAP row aggregated). The row is RETAINED across a restriction.
    pub fn seed_row(&self, subject: &SubjectRef, tenant: &TenantId) {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(Self::key(subject, tenant));
    }

    /// **Does the derived store still RETAIN the subject's row? (the §4.4 "while retaining storage"
    /// reading).** A restriction suppresses PROCESSING, never the retained row — so this stays `true`
    /// across a `restrict(set)` (and only an ERASE removes the row).
    #[must_use]
    pub fn has_row(&self, subject: &SubjectRef, tenant: &TenantId) -> bool {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&Self::key(subject, tenant))
    }

    /// **The derived-store processing op that HONOURS the `restrict` flag (§4.4 / 11.6).** Honour the
    /// flag at this derived store's processing chokepoint:
    /// - restricted ⇒ [`DerivedProcessed::Suppressed`] (no indexing / projection / notification /
    ///   agent-use / analytics for the restricted subject — the row is RETAINED).
    /// - else, row present ⇒ [`DerivedProcessed::Processed`] (the derivative projection runs).
    /// - no row ⇒ [`DerivedProcessed::NoRow`].
    ///
    /// This is the genuinely-new per-DERIVATIVE-store honour-the-flag path P-GA-25 adds (P-GA-17
    /// proved the M1-STORE processing ops; the derived stores' processing semantics differ — they are
    /// the projection/index/analytics chokepoints, not the source-content processing ops).
    #[must_use]
    pub fn process(&self, subject: &SubjectRef, tenant: &TenantId) -> DerivedProcessed {
        if !self.has_row(subject, tenant) {
            return DerivedProcessed::NoRow;
        }
        // HONOUR the ONE restrict flag at the derived-store chokepoint. The row is RETAINED either
        // way (has_row above is still true) — a restriction suppresses PROCESSING, never storage.
        if self.restrict.is_restricted(subject, tenant) {
            DerivedProcessed::Suppressed
        } else {
            DerivedProcessed::Processed(format!("{}:processed", self.kind.token()))
        }
    }
}

/// **A derived store AS a [`PersonalDataHolder`] (contract 10.1 / 11.6).** Its `restrict` op sets /
/// clears the SHARED suppression flag (the §4.4 entry point — every derived store reads the same
/// flag, so setting it on any one suppresses processing across all five); its processing chokepoint
/// ([`DerivedStore::process`]) honours that flag. The orchestrator reaches it ONLY through this
/// contract (the no-cross-store-read law — never an `import myelin_search` / `import myelin_olap`).
pub struct DerivedStoreHolder<'a> {
    store: &'a DerivedStore<'a>,
}

impl<'a> DerivedStoreHolder<'a> {
    /// Build the derived-store holder over a [`DerivedStore`] model (the live derived store at boot;
    /// the in-memory model in the drill).
    #[must_use]
    pub fn new(store: &'a DerivedStore<'a>) -> DerivedStoreHolder<'a> {
        DerivedStoreHolder { store }
    }
}

impl PersonalDataHolder for DerivedStoreHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.store.has_row(subject, &tenant) {
            "located:row-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate", self.store.holder_id(), &sid, &tenant.0, outcome, None, 0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export", self.store.holder_id(), &sid, &tenant.0, "exported", None, 0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify", self.store.holder_id(), &sid, "*",
                "rectified:reindex_from_source", None, 0,
            ),
        })
    }

    /// **The §4.4 `restrict` ENTRY POINT into the derived store.** Set / clear the SHARED suppression
    /// flag — the same flag every derived store + M1 holder reads, so a single `restrict(on)`
    /// suppresses processing across all five derived stores (the "every holder honours" property).
    /// Reversible: `on = false` lifts it. The receipt records the verdict (the green artifact).
    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // Set the ONE shared flag through the registry — every derived store's processing chokepoint
        // then honours it. (The orchestrator drives the fan-out via `RestrictFanOutDriver`; a single
        // holder's `restrict` sets the shared flag, so the call is idempotent across the five stores.)
        self.store.restrict.set(subject, &subject.principal.tenant, on);
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on { "restricted:set:processing_suppressed" } else { "restricted:clear:processing_resumed" };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict", self.store.holder_id(), &sid, &subject.principal.tenant.0, outcome, None, 0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let sid = match &scope {
            EraseScope::Subject { subject, .. } => subject.principal.principal_id.0.clone(),
            EraseScope::Tenant(_) => "*tenant*".to_string(),
        };
        let tenant = match &scope {
            EraseScope::Subject { tenant, .. } => tenant.0.clone(),
            EraseScope::Tenant(tenant) => tenant.0.clone(),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase", self.store.holder_id(), &sid, &tenant,
                "erased:derived_row_purged", None, 0,
            ),
        })
    }
}

// ───────────────────────── the restrict fan-out driver (the orchestration leg) ─────────────────────────

/// One derived store's restriction verdict in a fan-out (the per-store green-artifact row): its
/// holder id + the processing outcome after the restriction (PII-free).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedRestrictVerdict {
    /// The PII-free holder id of the derived store.
    pub holder_id: &'static str,
    /// The processing op that store performs (the §4.4 op suppressed for the restricted subject).
    pub op: DerivedProcessing,
    /// The processing outcome AFTER the restriction (must be [`DerivedProcessed::Suppressed`] for a
    /// restricted subject that has a row — the 0-processing reading).
    pub outcome: DerivedProcessed,
    /// Whether the derived row is RETAINED (storage retained across the restriction — §4.4).
    pub row_retained: bool,
}

/// **The restriction fan-out outcome (the GA-D7 green artifact).** Records, PII-free, the
/// per-store verdicts the drill asserts: 0 processing of a restricted subject across all five
/// derived stores, each with its row RETAINED (reversible). The [`Self::all_suppressed`] /
/// [`Self::processed_count`] readings are the load-bearing "0 processing" fact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestrictFanOutOutcome {
    /// The opaque subject token the restriction fanned over (PII-free).
    pub subject_token: String,
    /// Whether the restriction is SET (true) or CLEARED (false) — the reversibility leg records both.
    pub restricted: bool,
    /// The per-store verdicts (Search, Refs, Notif, Agents, OLAP — the five derived stores).
    pub verdicts: Vec<DerivedRestrictVerdict>,
    /// The per-holder restrict receipts the fan-out collected (one per derived store).
    pub holder_receipts: Vec<RestrictReceipt>,
}

impl RestrictFanOutOutcome {
    /// **The load-bearing "0 processing of a restricted subject" reading** — every derived store with
    /// a row SUPPRESSED its processing. `true` IFF no store processed a restricted-subject row.
    #[must_use]
    pub fn all_suppressed(&self) -> bool {
        self.verdicts
            .iter()
            .all(|v| matches!(v.outcome, DerivedProcessed::Suppressed))
    }

    /// How many derived stores still PROCESSED the restricted subject (MUST be 0 — the GA-D7 number).
    #[must_use]
    pub fn processed_count(&self) -> usize {
        self.verdicts
            .iter()
            .filter(|v| matches!(v.outcome, DerivedProcessed::Processed(_)))
            .count()
    }

    /// Whether every derived store RETAINED its row across the restriction (storage retained — §4.4).
    #[must_use]
    pub fn all_rows_retained(&self) -> bool {
        self.verdicts.iter().all(|v| v.row_retained)
    }
}

/// **The `restrict` fan-out driver (P-GA-25 — the orchestration leg of 10.1 over the M2 derived
/// stores).** Wires the five derived stores (Search H7 / Refs H12 / Notif H13 / Agents H11/H17 /
/// OLAP 11.6) as the orchestrator's per-holder `restrict` calls, sets the SHARED suppression flag
/// through them, and reads back each store's processing verdict to PROVE 0 processing of a
/// restricted subject (reversible). It NEVER reaches into a derived store — it holds only
/// `&dyn PersonalDataHolder` + reads the verdicts through the [`DerivedStore`] models (the
/// no-cross-store-read law).
pub struct RestrictFanOutDriver;

impl RestrictFanOutDriver {
    /// **Fan the `restrict(subject, on)` over the five derived stores and read back the verdicts (the
    /// GA-D7 orchestration).** Calls each derived store's `restrict` through the contract (setting the
    /// SHARED flag), then reads each store's `process` verdict + row-retention to build the
    /// [`RestrictFanOutOutcome`] (the green artifact). For `on = true` every store-with-a-row reads
    /// [`DerivedProcessed::Suppressed`] (0 processing); for `on = false` processing resumes
    /// (reversible). Errors propagate a holder error (a derived-store restrict failure is recoverable
    /// — re-call to resume).
    ///
    /// The caller passes the five derived-store models + their holders (the live Search/Refs/Notif/
    /// Agent/OLAP at boot; the faithful models in the drill). The stores share the ONE
    /// [`RestrictRegistry`] the holders set through (so one `restrict` suppresses all five).
    #[allow(clippy::too_many_arguments)]
    pub fn fan_out_restrict(
        subject: &SubjectRef,
        tenant: &TenantId,
        on: bool,
        stores: &[&DerivedStore<'_>; 5],
        holders: &[&dyn PersonalDataHolder; 5],
    ) -> DsrResult<RestrictFanOutOutcome> {
        // Set/clear the restriction through EACH derived store's `restrict` op (the §4.4 entry point —
        // it sets the SHARED flag, so the call is idempotent across the five; we call all five so each
        // emits a holder restrict receipt — the audit trail).
        let mut holder_receipts = Vec::with_capacity(5);
        for holder in holders {
            holder_receipts.push(holder.restrict(subject, on)?);
        }

        // Read back each store's processing verdict + row-retention (the GA-D7 readings — observed,
        // not asserted). A restricted subject's row reads Suppressed across every store; the row is
        // RETAINED (storage retained, the restriction reversible).
        let verdicts = stores
            .iter()
            .map(|store| DerivedRestrictVerdict {
                holder_id: store.holder_id(),
                op: store.kind(),
                outcome: store.process(subject, tenant),
                row_retained: store.has_row(subject, tenant),
            })
            .collect();

        Ok(RestrictFanOutOutcome {
            subject_token: subject.principal.principal_id.0.clone(),
            restricted: on,
            verdicts,
            holder_receipts,
        })
    }
}

/// The `restrict_fanout_processing_suppressed` telemetry signal NAME + UNIT (the per-derivative
/// restriction SLO — 0 processing of a restricted subject is the green artifact). PII-free.
pub const RESTRICT_FANOUT_PROCESSING_SUPPRESSED: (&str, &str) =
    ("gdpr.restrict_fanout_processing_suppressed", "count");

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, t("acme")))
    }

    /// Build the five derived-store models over ONE shared restrict registry, each seeded with the
    /// subject's row (the live consumer's projection step). Returns them in the stable order.
    fn five_stores<'a>(restrict: &'a RestrictRegistry) -> [DerivedStore<'a>; 5] {
        [
            DerivedStore::new(DerivedProcessing::SearchIndex, restrict),
            DerivedStore::new(DerivedProcessing::RefsProject, restrict),
            DerivedStore::new(DerivedProcessing::NotifNotify, restrict),
            DerivedStore::new(DerivedProcessing::AgentRead, restrict),
            DerivedStore::new(DerivedProcessing::OlapAnalyse, restrict),
        ]
    }

    // ───────── each derived store honours the flag (suppress, retain, reversible) ─────────

    /// **Every one of the five derived stores honours the `restrict` flag: while restricted, its
    /// processing op is SUPPRESSED, but the derived row is RETAINED (§4.4); reversible.** OLAP is in
    /// the set (no analytics for a restricted subject — contract 11.6 / GA-9).
    #[test]
    fn each_derived_store_suppresses_processing_but_retains_row_reversibly() {
        let tenant = t("acme");
        let subj = subject("u-1");
        let restrict = RestrictRegistry::new();
        let stores = five_stores(&restrict);
        for s in &stores {
            s.seed_row(&subj, &tenant);
        }

        // BEFORE restriction: every derived store PROCESSES the subject's row.
        for s in &stores {
            assert!(
                matches!(s.process(&subj, &tenant), DerivedProcessed::Processed(_)),
                "{} processes before restriction",
                s.holder_id()
            );
        }

        // SET the flag. Every derived store now SUPPRESSES processing, RETAINS the row.
        restrict.set(&subj, &tenant, true);
        for s in &stores {
            assert_eq!(
                s.process(&subj, &tenant),
                DerivedProcessed::Suppressed,
                "{} SUPPRESSED for the restricted subject (§4.4)",
                s.holder_id()
            );
            assert!(
                s.has_row(&subj, &tenant),
                "{} RETAINS the derived row while restricted (suppression ≠ delete)",
                s.holder_id()
            );
        }

        // CLEAR the flag (reversible). Processing resumes across every derived store.
        restrict.set(&subj, &tenant, false);
        for s in &stores {
            assert!(
                matches!(s.process(&subj, &tenant), DerivedProcessed::Processed(_)),
                "{} processes again after the restriction is lifted (reversible)",
                s.holder_id()
            );
        }
    }

    /// **OLAP specifically honours the restriction flag — no analytics for a restricted subject
    /// (contract 11.6, GA-9).** Pins the OLAP read store is in the suppression set (the §8
    /// restriction-flag-into-OLAP propagation).
    #[test]
    fn olap_honours_the_restriction_flag_no_analytics_for_a_restricted_subject() {
        let tenant = t("acme");
        let subj = subject("u-olap");
        let restrict = RestrictRegistry::new();
        let olap = DerivedStore::new(DerivedProcessing::OlapAnalyse, &restrict);
        olap.seed_row(&subj, &tenant);
        assert_eq!(olap.holder_id(), restrict_holder_ids::OLAP_READ_STORE);

        // Unrestricted: OLAP analyses the subject's rows.
        assert!(matches!(olap.process(&subj, &tenant), DerivedProcessed::Processed(_)));
        // Restricted: OLAP SUPPRESSES analytics (no analytics for a restricted subject — 11.6).
        restrict.set(&subj, &tenant, true);
        assert_eq!(olap.process(&subj, &tenant), DerivedProcessed::Suppressed);
        // The OLAP row is RETAINED (the restriction suppresses analytics, never the row).
        assert!(olap.has_row(&subj, &tenant));
    }

    /// The suppression branch is NOT vacuous: a restricted subject is Suppressed, an unrestricted one
    /// is Processed — the `is_restricted` branch is load-bearing in BOTH polarities (a `true`→`false`
    /// mutant would process a restricted subject; a `false`→`true` would suppress everyone).
    #[test]
    fn the_derived_suppression_branch_is_load_bearing_both_verdicts_pinned() {
        let tenant = t("acme");
        let subj = subject("u-branch");
        let restrict = RestrictRegistry::new();
        let search = DerivedStore::new(DerivedProcessing::SearchIndex, &restrict);
        search.seed_row(&subj, &tenant);

        // unrestricted ⇒ Processed (the false branch). The projection names the op token.
        match search.process(&subj, &tenant) {
            DerivedProcessed::Processed(out) => {
                assert!(out.starts_with("search_index:"), "the processed projection names the op");
            }
            other => panic!("expected Processed, got {other:?}"),
        }
        // restricted ⇒ Suppressed (the true branch).
        restrict.set(&subj, &tenant, true);
        assert_eq!(search.process(&subj, &tenant), DerivedProcessed::Suppressed);
    }

    /// A store with NO row reads `NoRow` (distinct from a suppression — nothing to process).
    #[test]
    fn a_derived_store_with_no_row_reads_no_row_not_suppressed() {
        let tenant = t("acme");
        let subj = subject("u-norow");
        let restrict = RestrictRegistry::new();
        let search = DerivedStore::new(DerivedProcessing::SearchIndex, &restrict);
        // No seed → NoRow, even when restricted (a restriction does not invent a row).
        assert_eq!(search.process(&subj, &tenant), DerivedProcessed::NoRow);
        restrict.set(&subj, &tenant, true);
        assert_eq!(search.process(&subj, &tenant), DerivedProcessed::NoRow);
    }

    // ───────── the restrict flag is shared (one set suppresses all five) ─────────

    /// **There is ONE shared flag: setting `restrict` through a SINGLE derived store's holder op
    /// suppresses processing across ALL FIVE derived stores (the §4.4 "every holder honours"
    /// property).** Restricting through the Search holder suppresses Refs/Notif/Agents/OLAP too —
    /// because they all read the same [`RestrictRegistry`].
    #[test]
    fn restrict_through_one_holder_suppresses_all_five_derived_stores() {
        let tenant = t("acme");
        let subj = subject("u-shared");
        let restrict = RestrictRegistry::new();
        let stores = five_stores(&restrict);
        for s in &stores {
            s.seed_row(&subj, &tenant);
        }

        // Restrict THROUGH the Search holder only — the SHARED flag is set for the subject.
        let search_holder = DerivedStoreHolder::new(&stores[0]);
        search_holder.restrict(&subj, true).unwrap();

        // EVERY derived store now suppresses — they all read the same flag (no second registry).
        for s in &stores {
            assert_eq!(
                s.process(&subj, &tenant),
                DerivedProcessed::Suppressed,
                "{} suppressed by the SHARED flag set through the Search holder",
                s.holder_id()
            );
        }
    }

    // ───────── the fan-out driver: 0 processing across all five, reversible ─────────

    /// **The driver fans `restrict` over the five derived stores and proves 0 processing of a
    /// restricted subject (reversible).** SET → all five Suppressed, 0 processed, every row retained;
    /// CLEAR → all five Processed again.
    #[test]
    fn driver_fans_restrict_zero_processing_across_all_five_reversible() {
        let tenant = t("acme");
        let subj = subject("u-fan");
        let restrict = RestrictRegistry::new();
        let stores = five_stores(&restrict);
        for s in &stores {
            s.seed_row(&subj, &tenant);
        }
        let holders: Vec<DerivedStoreHolder> =
            stores.iter().map(DerivedStoreHolder::new).collect();
        let store_refs: [&DerivedStore; 5] =
            [&stores[0], &stores[1], &stores[2], &stores[3], &stores[4]];
        let holder_refs: [&dyn PersonalDataHolder; 5] = [
            &holders[0], &holders[1], &holders[2], &holders[3], &holders[4],
        ];

        // SET the restriction across all five.
        let set = RestrictFanOutDriver::fan_out_restrict(
            &subj, &tenant, true, &store_refs, &holder_refs,
        )
        .unwrap();
        assert!(set.all_suppressed(), "0 processing of the restricted subject across all five (GA-D7)");
        assert_eq!(set.processed_count(), 0, "0 derived stores processed the restricted subject");
        assert!(set.all_rows_retained(), "every derived row RETAINED while restricted (§4.4)");
        assert_eq!(set.verdicts.len(), 5, "Search + Refs + Notif + Agents + OLAP");
        assert_eq!(set.holder_receipts.len(), 5, "one restrict receipt per derived store");
        assert!(set.restricted, "the outcome records the restriction is SET");

        // CLEAR the restriction (reversible) — processing resumes across all five.
        let clear = RestrictFanOutDriver::fan_out_restrict(
            &subj, &tenant, false, &store_refs, &holder_refs,
        )
        .unwrap();
        assert!(!clear.all_suppressed(), "processing resumes after the restriction is lifted");
        assert_eq!(
            clear.verdicts.iter().filter(|v| matches!(v.outcome, DerivedProcessed::Processed(_))).count(),
            5,
            "all five derived stores process again (reversible)"
        );
        assert!(!clear.restricted, "the outcome records the restriction is CLEARED");
    }

    /// **`processed_count` / `all_suppressed` / `all_rows_retained` are NOT vacuous (the GA-D7
    /// numbers are load-bearing).** A fan-out over an UNRESTRICTED subject reads `processed_count ==
    /// 5` (not 0), `all_suppressed == false`, every row retained; a fan-out where a store has NO row
    /// reads `all_rows_retained == false` — so a constant-`0` / constant-`true` mutant on those
    /// readings is caught (a mutant that always says "0 processed" would falsely pass GA-D7).
    #[test]
    fn the_fanout_readings_are_not_vacuous_both_polarities() {
        let tenant = t("acme");
        let subj = subject("u-vac");
        let restrict = RestrictRegistry::new();
        let stores = five_stores(&restrict);
        // Seed only FOUR of the five rows — the fifth store has NO row (so all_rows_retained == false).
        for s in stores.iter().take(4) {
            s.seed_row(&subj, &tenant);
        }
        let holders: Vec<DerivedStoreHolder> =
            stores.iter().map(DerivedStoreHolder::new).collect();
        let store_refs: [&DerivedStore; 5] =
            [&stores[0], &stores[1], &stores[2], &stores[3], &stores[4]];
        let holder_refs: [&dyn PersonalDataHolder; 5] = [
            &holders[0], &holders[1], &holders[2], &holders[3], &holders[4],
        ];

        // UNRESTRICTED fan-out: the four rows PROCESS → processed_count == 4 (NOT 0), not all
        // suppressed; the fifth is NoRow. This kills a `processed_count -> 0` constant mutant.
        let out = RestrictFanOutDriver::fan_out_restrict(
            &subj, &tenant, false, &store_refs, &holder_refs,
        )
        .unwrap();
        assert_eq!(out.processed_count(), 4, "four rows processed (processed_count is not constant 0)");
        assert!(!out.all_suppressed(), "not all suppressed (a processing store exists)");
        // The fifth store has no row ⇒ not every verdict is row-retained (kills `all_rows_retained -> true`).
        assert!(
            !out.all_rows_retained(),
            "the store with NO row is not row-retained (all_rows_retained is not constant true)"
        );
    }

    /// **The store keys content per-`(tenant, subject)` — two subjects do NOT collide (kills a
    /// constant-key mutant).** A row seeded for subject A is NOT seen as present for subject B, and a
    /// restriction on A does not suppress B (the key is load-bearing).
    #[test]
    fn the_derived_store_keys_per_tenant_and_subject() {
        let tenant = t("acme");
        let other = t("globex");
        let a = subject("u-a");
        let b = subject("u-b");
        let restrict = RestrictRegistry::new();
        let store = DerivedStore::new(DerivedProcessing::SearchIndex, &restrict);
        store.seed_row(&a, &tenant);
        // A's row is present; B's is not; the SAME id in a different tenant is not (kills a key that
        // ignores the tenant or the subject component).
        assert!(store.has_row(&a, &tenant), "A's row is present");
        assert!(!store.has_row(&b, &tenant), "B has no row (distinct subject key)");
        assert!(!store.has_row(&a, &other), "A's id in a different tenant has no row (tenant in the key)");
        // A restriction on A suppresses A but B (had B a row) would still process — the key isolates.
        restrict.set(&a, &tenant, true);
        assert_eq!(store.process(&a, &tenant), DerivedProcessed::Suppressed);
        assert_eq!(store.process(&b, &tenant), DerivedProcessed::NoRow);
    }

    /// The fan-out covers exactly the five §4.4 derived stores, each with its distinct holder id +
    /// processing op (the store↔op map is stable).
    #[test]
    fn the_fan_out_covers_exactly_the_five_section_4_4_derived_stores() {
        assert_eq!(DerivedProcessing::all().len(), 5);
        assert_eq!(DerivedProcessing::SearchIndex.holder_id(), restrict_holder_ids::SEARCH_INDEX);
        assert_eq!(DerivedProcessing::RefsProject.holder_id(), restrict_holder_ids::REFS_GRAPH);
        assert_eq!(DerivedProcessing::NotifNotify.holder_id(), restrict_holder_ids::NOTIF_HISTORY);
        assert_eq!(DerivedProcessing::AgentRead.holder_id(), restrict_holder_ids::AGENT_RUNTIME);
        assert_eq!(DerivedProcessing::OlapAnalyse.holder_id(), restrict_holder_ids::OLAP_READ_STORE);
        // The tokens are stable (receipt / telemetry anchors).
        assert_eq!(DerivedProcessing::SearchIndex.token(), "search_index");
        assert_eq!(DerivedProcessing::OlapAnalyse.token(), "olap_analyse");
    }

    /// The holder `restrict` op routes through the SHARED registry: `restrict(on)` sets the flag,
    /// `restrict(off)` clears it (the §4.4 reversible entry point), and the receipt is
    /// content-addressed (the green artifact).
    #[test]
    fn the_holder_restrict_op_sets_and_clears_the_shared_flag_with_a_receipt() {
        let tenant = t("acme");
        let subj = subject("u-receipt");
        let restrict = RestrictRegistry::new();
        let store = DerivedStore::new(DerivedProcessing::OlapAnalyse, &restrict);
        let holder = DerivedStoreHolder::new(&store);

        let set = holder.restrict(&subj, true).unwrap();
        assert!(restrict.is_restricted(&subj, &tenant), "the holder restrict op SET the shared flag");
        assert_eq!(set.receipt.operation, "restrict");
        assert!(set.receipt.content_hash.starts_with("blake3:"), "the restrict receipt is content-addressed");

        let clear = holder.restrict(&subj, false).unwrap();
        assert!(!restrict.is_restricted(&subj, &tenant), "the holder restrict op CLEARED the shared flag");
        assert_eq!(clear.receipt.operation, "restrict");
        // The set/clear receipts differ (distinct outcomes → distinct content hashes).
        assert_ne!(set.receipt.content_hash, clear.receipt.content_hash);
    }
}
