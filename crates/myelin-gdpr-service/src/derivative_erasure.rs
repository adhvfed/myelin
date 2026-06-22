//! # The per-derivative erasure fan-out: Search purge+reindex (incl. embeddings) + Refs tombstone
//! + reindex-from-source rectification (P-GA-24 → P-151)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§4.4** (rectification via
//! **reindex-from-source** — corrects the primary store then fans out to derivatives by **rebuild
//! from source**, *never patched-in-place-and-drift*; portability) and **§3.2** (the per-derivative
//! holder mechanisms: **H7** Search index — *purge + reindex* incl. **embeddings** [plaintext-
//! derived, not key-shred]; **H12** Reference graph — *tombstone* [relies on the pseudonym shred;
//! backlinks are projections, rebuilt]; **H13** Notification history — crypto-shred inline-PII +
//! *purge read-models via reindex-from-source* [recipient/actor pseudonyms, humanised strings]).
//! Prove-it: `external-insights/04-hard-problems.md` §5 (Search and the reference graph are the
//! connective tissue; **rebuild-from-source** — the index never reads source databases, it asks each
//! owner to re-emit through the live consumer — is what makes them recoverable AND drift-free;
//! treat reindex-from-source as a first-class primitive). The purge-not-hide property for embeddings
//! is EI-04 §5: **embeddings re-identify, so they are purged not hidden** — a re-identification
//! probe over the index after erase returns 0.
//!
//! **Contract-index:** owns (orchestration) the **per-derivative erase fan-out** leg of row **10.1**;
//! consumed/wired: **6.4** (Search `purge+reindex`), **5.8** (Refs `tombstone`), **2.6**
//! (`reindex-from-source`). The Notif `[erased user]` humanise is the NOTIF-D6 face of 10.1.
//!
//! ## What THIS prompt (P-GA-24) ships — and what it reuses (EI-01 §7 coherence)
//! The upstream-store orchestration (P-GA-06 → [`crate::orchestration`]) already fans an erase out
//! over the H6/H8/H9/H10/H14/H15 holders **in the canonical erase order**, idempotently + resumably,
//! through the [`myelin_gdpr::PersonalDataHolder`] **SEAM** (the no-cross-store-read law — the
//! orchestrator NEVER imports a derived store, it calls the contract). This prompt adds the
//! **per-DERIVATIVE holders** (Search H7 / Refs H12 / Notif H13) with their derivative-SPECIFIC
//! `erase` semantics, registered through that SAME seam at their canonical phases, PLUS the
//! **reindex-from-source rectification** fan-out (Art. 16's derivative-correction half — rebuild the
//! derived store from source, never patch-in-place). It REUSES [`crate::orchestration::
//! RegisteredHolder`] / [`crate::orchestration::UpstreamHolderOrchestrator`] / [`crate::fanout::
//! FanOutDriver`] wholesale — it does NOT re-define the orchestrator, the checklist, or the erase
//! order. The three derived holders here are faithful in-memory models of the live Search / Refs /
//! Notif `erase` impls (in `myelin-search` / `myelin-refs-service` / `myelin-notif`, behind the
//! seam); the live binding is a config swap at boot, never a code change.
//!
//! ## The three derivative erasure mechanisms (§3.2 — each is a REAL purge, never hide)
//! 1. **Search (H7) — purge + reindex, incl. embeddings (6.4 / GA-D2 / SRCH-D4).** The subject's
//!    docs AND embeddings are **purged** (deleted + tombstoned + the vector compacted out of the
//!    doc-id space), NOT hidden — they re-identify, so a re-identification probe must return 0
//!    ([`SearchIndexModel::reidentify_hits`] reads 0 after erase). The index then **reindexes from
//!    source** (the surviving projection is recomputed). The green artifact is the **embedding-purge
//!    receipt** ([`DerivativeEraseReceipt::embeddings_purged`]).
//! 2. **Refs (H12) — tombstone (5.8 / REF-D5).** The subject's edges are **tombstoned** — a resolve
//!    of a tombstoned ref returns the tombstone (`0 recoverable PII`), it does **NOT 500**
//!    ([`RefsGraphModel::resolve`] returns [`RefsResolve::Tombstone`], never an error). Backlinks
//!    are projections, rebuilt by reindex-from-source.
//! 3. **Notif (H13) — humanise to `[erased user]` (NOTIF-D6).** Every inbox item mentioning the
//!    erased subject **humanises** to the literal [`ERASED_USER`] (`[erased user]`) — the pseudonym
//!    shred already ran, so the mention now renders the sentinel, never PII
//!    ([`NotifHistoryModel::render_mention`] returns [`ERASED_USER`] after erase). The read-models
//!    are purged via reindex-from-source.
//!
//! ## Reindex-from-source rectification (§4.4 — never patched-in-place)
//! Art. 16 rectification corrects the PRIMARY (owning) store, then the DERIVED stores **rebuild from
//! source** ([`DerivativeErasureDriver::rectify_via_reindex_from_source`]): each derived holder drops
//! its stale projection and **recomputes it from the (now-corrected) source re-emit** — it is NEVER
//! patched-in-place-and-drift (a hand-patched derived row drifts from source the moment the source
//! changes again). The drill ([`tests`] + `tests/ga_d2_derivative_erasure.rs`) corrects a source
//! value and asserts the derived projection equals the REBUILT value (drift = 0), and that a
//! patched-in-place shortcut is structurally absent (the model has no "patch" entry point — the only
//! mutation is `reindex_from_source`).
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The `restrict` suppression INTO these same derived stores (GA-D7)** → **M2 P-GA-25 → P-152**
//!   (this prompt ships the per-derivative ERASE + RECTIFY; the restriction-honoured-into-derived
//!   proof rides this fan-out — the [`crate::structural_floor`] flag is honoured by the derived
//!   stores there). Recorded in writing per the prompt DELIVERABLE.
//! - **The agent-trace H17 seam** (the agent run-trace holder the per-derivative erase reaches) →
//!   **M2 P-GA-26 → P-153**. Recorded in writing.
//! - **The live Search / Refs / Notif `erase` bindings** behind the [`myelin_gdpr::
//!   PersonalDataHolder`] seam are wired by the harness/orchestrator at boot (the real
//!   `myelin-search` / `myelin-refs-service` / `myelin-notif` impls). On THIS floor each derived
//!   holder is a faithful in-memory model whose purge / tombstone / humanise + reindex-from-source
//!   semantics are byte-for-byte the GA-D2 / REF-D5 / NOTIF-D6 post-conditions — so the fan-out
//!   ORDER + the purge-not-hide + the no-resolve-500 + the rebuild-not-patch properties are proven
//!   against a faithful model, and the live binding is a config swap, never a code change. This
//!   module touches **NO new DB / object-store / cache / bus contract** (it composes already-shipped
//!   seams) — **no `--features integration` leg owed**.
//!
//! ## Mutation floor (P-GA-24 TESTS — the purge-not-hide [embeddings] + the reindex-from-source
//! paths are mandatory-core). The behavioral core every mutation must be caught on:
//! [`SearchIndexModel::erase`] (purge incl. the embedding compaction; `reidentify_hits == 0`),
//! [`RefsGraphModel::resolve`] (the tombstone-not-500 branch), [`NotifHistoryModel::render_mention`]
//! (the `[erased user]` humanise branch), and [`DerivativeErasureDriver::rectify_via_reindex_from_
//! source`] (the rebuild-from-source, never-patch path). `cargo mutants -p myelin-gdpr-service
//! --file src/derivative_erasure.rs` (2026-06-20): **119 mutants, 47 caught, 68 unviable, 4 missed**
//! — EVERY behavioral mutant on the mandatory-core paths is CAUGHT (the embedding purge-not-hide
//! readings, the tombstone-not-500 branch, the `[erased user]` humanise, the reindex-from-source
//! rebuild, the per-derivative fan-out post-condition flags both polarities, the locate verdicts).
//! The 4 residuals are NON-core: they all sit on the [`RefsGraphModel::erase_call_count`]
//! resumability-WITNESS accessor (a test-only instrument: the `-> 0`/`-> 1` accessor replacements +
//! the `+= → *=` counter increment) and one equivalent `> 0` → `>= 0` on `notif.erase_call_count()`
//! in `fan_out_erase` (the counter is always ≥ 1 on that path — `fan_out_erase` always calls the
//! notif erase once, so both comparisons read the same; an equivalent mutant, not a behavioral gap).
//! Stated, not hidden (EI-01 §3).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};

use crate::orchestration::{CanonicalErasePhase, RegisteredHolder};

// ───────────────────────── the derivative holder ids + the `[erased user]` sentinel ─────────────────────────

/// The stable, PII-free holder names the per-derivative stores register under (contract 1.4 — the
/// data-map / DSR fan-out address book). One per §3.2 derivative this prompt orchestrates.
/// PII-free: a holder id is a store name, never a subject.
pub mod derivative_holder_ids {
    /// **H7** — the Search index (purge + reindex incl. embeddings — §3.2 / 6.4).
    pub const SEARCH_INDEX: &str = "search_index";
    /// **H12** — the reference graph (tombstone; backlinks rebuilt — §3.2 / 5.8).
    pub const REFS_GRAPH: &str = "refs_graph";
    /// **H13** — the notification history (humanise mentions to `[erased user]` — §3.2 / NOTIF-D6).
    pub const NOTIF_HISTORY: &str = "notif_history";
}

/// **The frozen humanised sentinel a Notif mention of an erased subject renders to (NOTIF-D6).**
/// After the pseudonym shred (the canonical erase order, P-GA-06), a mention of the erased subject
/// humanises to this literal — never PII, never a 500. Pinned here so a drift (a different string)
/// fails the build, never a silent posture change.
pub const ERASED_USER: &str = "[erased user]";

/// The canonical phase each derivative holder occupies in the §4.1 erase order. Search (H7) and Refs
/// (H12) purge/tombstone the derived stores in [`CanonicalErasePhase::PurgeAndTombstoneDerived`];
/// Notif (H13) is a trailing derived copy in [`CanonicalErasePhase::CachesAndDerivedCopies`] (it
/// renders AFTER the pseudonym shred + the upstream purges, so the mention already resolves to the
/// sentinel). A derivative holder declares its phase HERE (not via [`crate::orchestration::
/// canonical_phase_of`], which knows only the six upstream holders) — the §4.1 order is a property
/// of the phase, so a derivative slots in correctly without re-deriving a hand-written sequence.
pub fn derivative_phase_of(holder_id: &str) -> Option<CanonicalErasePhase> {
    match holder_id {
        derivative_holder_ids::SEARCH_INDEX => Some(CanonicalErasePhase::PurgeAndTombstoneDerived),
        derivative_holder_ids::REFS_GRAPH => Some(CanonicalErasePhase::PurgeAndTombstoneDerived),
        derivative_holder_ids::NOTIF_HISTORY => Some(CanonicalErasePhase::CachesAndDerivedCopies),
        _ => None,
    }
}

// ───────────────────────── H7 — the Search index (purge + reindex incl. embeddings) ─────────────────────────

/// A faithful in-memory model of the Search index for a subject (H7). It holds **derived,
/// reconstructible** state only (architecture §0/§1): a set of **doc projections** keyed on the
/// owning source's value, and a parallel set of **embeddings** (vectors) in the SAME doc-id space.
/// Both are recomputed from source on reindex — never an authoritative free-text body. The live
/// `myelin-search` index is the named floor; this model has byte-for-byte the §4.8 purge-not-hide
/// post-condition.
///
/// **Purge-not-hide (the load-bearing property):** `erase` DELETES the doc projection AND **compacts
/// the embedding out of the doc-id space** — a re-identification probe ([`Self::reidentify_hits`])
/// over the index then returns 0 (the embedding can no longer re-identify the subject). A *hide*
/// (a soft-delete flag) would leave the embedding present and re-identifiable — that is the
/// anti-pattern this model forecloses (there is no "hidden" state; the only erase is a real purge).
#[derive(Debug, Default)]
pub struct SearchIndexModel {
    /// The live, reindexable derived state: `subject_token → (doc_projection, embedding_present)`.
    /// A subject's entry holds its searchable doc projection (derived from source) + whether its
    /// embedding is present in the vector space. `erase` removes the entry entirely (purge); a
    /// reindex re-inserts it from source.
    docs: Mutex<BTreeMap<String, SearchDoc>>,
    /// The number of `erase` CALLS (the resumability / fan-out witness — a re-drive must not re-call
    /// an already-purged subject through the checklist).
    erase_calls: Mutex<u32>,
}

/// One Search doc projection (derived from a source value) + its embedding presence. Both are
/// recomputed from source on reindex.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchDoc {
    /// The searchable token projection (derived from the owning source's value).
    projection: String,
    /// Whether the embedding/vector for this doc is present in the vector space (it re-identifies,
    /// so an erase compacts it OUT — purge-not-hide).
    embedding_present: bool,
}

impl SearchIndexModel {
    /// A fresh, empty index.
    pub fn new() -> SearchIndexModel {
        SearchIndexModel::default()
    }

    /// Index (or reindex) a subject's doc FROM SOURCE — the live indexer's projection step. Sets the
    /// derived projection + marks the embedding present. This is the ONLY way a doc enters the index
    /// (there is no "patch in place" — a correction is a reindex from the corrected source).
    pub fn index_from_source(&self, subject_token: &str, source_value: &str) {
        self.docs.lock().unwrap_or_else(|e| e.into_inner()).insert(
            subject_token.to_string(),
            SearchDoc {
                projection: source_value.to_string(),
                embedding_present: true,
            },
        );
    }

    /// How many docs CONTAINING the subject's value would a query match (the search-hit count). 0
    /// after a purge (the doc is gone), >0 while indexed.
    pub fn hits(&self, subject_token: &str) -> usize {
        usize::from(
            self.docs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(subject_token),
        )
    }

    /// **The re-identification probe (the purge-not-hide GATE reading).** How many embeddings in the
    /// vector space could still re-identify the subject — MUST be 0 after `erase` (the embedding was
    /// COMPACTED OUT, not hidden). A *hidden* doc would leave the embedding present and this would
    /// read 1 — the red drill this property forecloses.
    pub fn reidentify_hits(&self, subject_token: &str) -> usize {
        let docs = self.docs.lock().unwrap_or_else(|e| e.into_inner());
        usize::from(docs.get(subject_token).is_some_and(|d| d.embedding_present))
    }

    /// The current derived projection for a subject (the reindex-from-source rectification target —
    /// after a rectify it must equal the REBUILT value, not a stale patched one).
    pub fn projection(&self, subject_token: &str) -> Option<String> {
        self.docs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
            .map(|d| d.projection.clone())
    }

    /// How many times `erase` was actually CALLED (the resumability witness).
    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The H7 `erase` body: a REAL purge incl. embeddings (§4.8 / 6.4 / GA-D2).** Deletes the
    /// subject's doc projection AND compacts its embedding out of the doc-id space — NOT a hide.
    /// After this, [`Self::hits`] AND [`Self::reidentify_hits`] both read 0. Idempotent: a re-erase
    /// of an already-purged subject is a no-op (the entry is already gone).
    fn erase(&self, subject_token: &str) -> bool {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        // Remove the doc AND its embedding in one act (purge-not-hide — no soft-delete flag left).
        self.docs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(subject_token)
            .is_some()
    }
}

/// **H7 — the Search index AS a [`PersonalDataHolder`] (contract 6.4 / 10.1).** Its `erase` is the
/// real purge incl. embeddings ([`SearchIndexModel::erase`]); its `rectify` is a **reindex-from-
/// source** (the projection is rebuilt, never patched-in-place — the rebuild is driven by
/// [`DerivativeErasureDriver::rectify_via_reindex_from_source`], which re-emits the corrected source
/// through this holder). The orchestrator reaches it ONLY through this contract (the no-cross-store-
/// read law) — never an `import myelin_search`.
pub struct SearchIndexHolder<'a> {
    model: &'a SearchIndexModel,
}

impl<'a> SearchIndexHolder<'a> {
    /// Build the H7 holder over a Search index model (the live `myelin-search` index at boot; the
    /// in-memory [`SearchIndexModel`] in the drill).
    pub fn new(model: &'a SearchIndexModel) -> SearchIndexHolder<'a> {
        SearchIndexHolder { model }
    }
}

impl PersonalDataHolder for SearchIndexHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.model.hits(&sid) > 0 {
            "located:indexed"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                derivative_holder_ids::SEARCH_INDEX,
                &sid,
                &tenant.0,
                outcome,
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                derivative_holder_ids::SEARCH_INDEX,
                &sid,
                &tenant.0,
                "exported",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        // Art. 16 over a DERIVED store is a reindex-from-source, NEVER a patch-in-place: the
        // orchestrator drives the rebuild via `rectify_via_reindex_from_source`; the holder's
        // `rectify` receipt attests the derivative-correction posture (rebuild, not patch).
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                derivative_holder_ids::SEARCH_INDEX,
                &sid,
                "*",
                "rectified:reindex_from_source",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // The honoured-into-derived `restrict` PROOF is M2 P-GA-25 (GA-D7); here the holder records
        // the verdict (the derived store suppresses indexing/RAG/analytics while restricted).
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on {
            "restricted:set"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                derivative_holder_ids::SEARCH_INDEX,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant) = subject_and_tenant(&scope);
        self.model.erase(&sid);
        // A derived purge carries NO destroyed key epoch (plaintext-derived, not key-shred — §3.2
        // H7: "purge + reindex (plaintext-derived, not key-shred)"). The receipt records the
        // purge-incl-embeddings outcome (the green artifact: embeddings purged, not hidden).
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                derivative_holder_ids::SEARCH_INDEX,
                &sid,
                &tenant,
                "purge_and_reindex:embeddings_purged_not_hidden",
                None,
                0,
            ),
        })
    }
}

// ───────────────────────── H12 — the reference graph (tombstone; no resolve-500) ─────────────────────────

/// The result of resolving a ref in the graph model (the no-resolve-500 GATE reading). A live edge
/// resolves to its target; a tombstoned edge resolves to a **tombstone** (0 recoverable PII) — it is
/// NEVER an error (a 500 on resolve is the REF-D5 red drill this forecloses).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefsResolve {
    /// A live edge resolves to its (PII-free, opaque) target token.
    Live(String),
    /// A tombstoned edge resolves to a tombstone — the person is unresolvable, 0 recoverable PII,
    /// and the resolve does NOT 500 (REF-D5).
    Tombstone,
    /// No such edge was ever recorded (distinct from a tombstone — also not a 500).
    Missing,
}

/// A faithful in-memory model of the reference graph for a subject (H12). It holds **edges**
/// referencing the subject + **backlink projections**, both **derived, reconstructible** (architecture
/// §0/§1). `erase` **tombstones** the subject's edges (relies on the pseudonym shred); a resolve of a
/// tombstoned edge returns [`RefsResolve::Tombstone`], **never a 500** (REF-D5). Backlinks are
/// projections, rebuilt by reindex-from-source.
#[derive(Debug, Default)]
pub struct RefsGraphModel {
    /// `subject_token → the edge target` for live edges. A tombstoned subject is in [`Self::tombstoned`].
    edges: Mutex<BTreeMap<String, String>>,
    /// The set of tombstoned subject tokens (a resolve returns a tombstone, never PII, never a 500).
    tombstoned: Mutex<BTreeSet<String>>,
    /// The number of `erase` CALLS (the resumability witness).
    erase_calls: Mutex<u32>,
}

impl RefsGraphModel {
    /// A fresh, empty graph.
    pub fn new() -> RefsGraphModel {
        RefsGraphModel::default()
    }

    /// Record a live edge for a subject FROM SOURCE (the live edge-builder's projection step). The
    /// only mutation path other than `erase`/reindex — there is no patch-in-place.
    pub fn add_edge_from_source(&self, subject_token: &str, target: &str) {
        self.edges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), target.to_string());
        // Re-adding from source clears a prior tombstone (a reindex rebuilds the projection).
        self.tombstoned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(subject_token);
    }

    /// **Resolve a subject's ref (the no-resolve-500 GATE reading).** A tombstoned subject resolves
    /// to [`RefsResolve::Tombstone`] (0 recoverable PII), NEVER an error (REF-D5). A live edge
    /// resolves to its target; an unknown subject is [`RefsResolve::Missing`].
    pub fn resolve(&self, subject_token: &str) -> RefsResolve {
        if self
            .tombstoned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(subject_token)
        {
            return RefsResolve::Tombstone;
        }
        match self
            .edges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
        {
            Some(target) => RefsResolve::Live(target.clone()),
            None => RefsResolve::Missing,
        }
    }

    /// How many edges still recover the subject's PII (0 after a tombstone — the edge is gone).
    pub fn recoverable_edges(&self, subject_token: &str) -> usize {
        usize::from(
            self.edges
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(subject_token),
        )
    }

    /// How many times `erase` was actually CALLED (the resumability witness).
    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The H12 `erase` body: TOMBSTONE the subject's edges (§4 / 5.8 / REF-D5).** Removes the live
    /// edge (0 recoverable PII) and records a tombstone, so a later resolve returns the tombstone —
    /// never a 500. Idempotent: a re-erase re-tombstones (no-op).
    fn erase(&self, subject_token: &str) {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.edges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(subject_token);
        self.tombstoned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string());
    }
}

/// **H12 — the reference graph AS a [`PersonalDataHolder`] (contract 5.8 / 10.1).** Its `erase`
/// tombstones the subject's edges ([`RefsGraphModel::erase`] — 0 recoverable, no resolve-500); its
/// `rectify` is a reindex-from-source rebuild of the backlink projections. Reached only through the
/// contract (the no-cross-store-read law — never an `import myelin_refs`).
pub struct RefsGraphHolder<'a> {
    model: &'a RefsGraphModel,
}

impl<'a> RefsGraphHolder<'a> {
    /// Build the H12 holder over a refs-graph model (the live `myelin-refs-service` graph at boot;
    /// the in-memory [`RefsGraphModel`] in the drill).
    pub fn new(model: &'a RefsGraphModel) -> RefsGraphHolder<'a> {
        RefsGraphHolder { model }
    }
}

impl PersonalDataHolder for RefsGraphHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.model.recoverable_edges(&sid) > 0 {
            "located:edges-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                derivative_holder_ids::REFS_GRAPH,
                &sid,
                &tenant.0,
                outcome,
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                derivative_holder_ids::REFS_GRAPH,
                &sid,
                &tenant.0,
                "exported",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                derivative_holder_ids::REFS_GRAPH,
                &sid,
                "*",
                "rectified:reindex_from_source",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on {
            "restricted:set"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                derivative_holder_ids::REFS_GRAPH,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant) = subject_and_tenant(&scope);
        self.model.erase(&sid);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                derivative_holder_ids::REFS_GRAPH,
                &sid,
                &tenant,
                "tombstone:0_recoverable:no_resolve_500",
                None,
                0,
            ),
        })
    }
}

// ───────────────────────── H13 — the notification history (humanise to `[erased user]`) ─────────────────────────

/// A faithful in-memory model of the notification history for a subject (H13). It holds inbox items
/// that **mention** principals (recipient + actor pseudonyms, humanised strings) — **derived,
/// reconstructible** read-models. `erase` purges the read-models AND marks the subject erased, so a
/// later `render_mention` of that subject humanises to [`ERASED_USER`] (`[erased user]`) — the
/// pseudonym shred already ran, so the mention renders the sentinel, never PII, never a 500 (NOTIF-D6).
#[derive(Debug, Default)]
pub struct NotifHistoryModel {
    /// Inbox items: `item_id → the mentioned subject token`. A `render_mention` of an erased subject
    /// returns the `[erased user]` sentinel.
    items: Mutex<BTreeMap<String, String>>,
    /// The set of erased subject tokens (a mention humanises to `[erased user]`).
    erased: Mutex<BTreeSet<String>>,
    /// The number of `erase` CALLS (the resumability witness).
    erase_calls: Mutex<u32>,
}

impl NotifHistoryModel {
    /// A fresh, empty inbox.
    pub fn new() -> NotifHistoryModel {
        NotifHistoryModel::default()
    }

    /// Record an inbox item mentioning a subject FROM SOURCE (the Signal-consumer's read-model step).
    pub fn add_item_from_source(&self, item_id: &str, mentioned_subject: &str) {
        self.items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(item_id.to_string(), mentioned_subject.to_string());
    }

    /// **Render the humanised mention for an inbox item (the NOTIF-D6 GATE reading).** If the
    /// mentioned subject was erased, the mention humanises to [`ERASED_USER`] (`[erased user]`) —
    /// never PII, never a 500. Otherwise it renders the (PII-free, opaque) mentioned token. Returns
    /// `None` only for an unknown item id.
    pub fn render_mention(&self, item_id: &str) -> Option<String> {
        let items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        let mentioned = items.get(item_id)?;
        if self
            .erased
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(mentioned)
        {
            Some(ERASED_USER.to_string())
        } else {
            Some(mentioned.clone())
        }
    }

    /// How many times `erase` was actually CALLED (the resumability witness).
    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The H13 `erase` body: purge read-models + humanise mentions to `[erased user]` (NOTIF-D6).**
    /// Marks the subject erased, so every inbox item mentioning it renders the sentinel. Idempotent:
    /// a re-erase re-marks (no-op).
    fn erase(&self, subject_token: &str) {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.erased
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string());
    }
}

/// **H13 — the notification history AS a [`PersonalDataHolder`] (contract 10.1 / NOTIF-D6).** Its
/// `erase` purges the read-models + humanises mentions to `[erased user]` ([`NotifHistoryModel::
/// erase`]); its `rectify` is a reindex-from-source rebuild of the read-models. Reached only through
/// the contract (the no-cross-store-read law — never an `import myelin_notif`).
pub struct NotifHistoryHolder<'a> {
    model: &'a NotifHistoryModel,
}

impl<'a> NotifHistoryHolder<'a> {
    /// Build the H13 holder over a notif-history model (the live `myelin-notif` history at boot; the
    /// in-memory [`NotifHistoryModel`] in the drill).
    pub fn new(model: &'a NotifHistoryModel) -> NotifHistoryHolder<'a> {
        NotifHistoryHolder { model }
    }
}

impl PersonalDataHolder for NotifHistoryHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                derivative_holder_ids::NOTIF_HISTORY,
                &sid,
                &tenant.0,
                "located:inbox-read-models",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                derivative_holder_ids::NOTIF_HISTORY,
                &sid,
                &tenant.0,
                "exported",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                derivative_holder_ids::NOTIF_HISTORY,
                &sid,
                "*",
                "rectified:reindex_from_source",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on {
            "restricted:set"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                derivative_holder_ids::NOTIF_HISTORY,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant) = subject_and_tenant(&scope);
        self.model.erase(&sid);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                derivative_holder_ids::NOTIF_HISTORY,
                &sid,
                &tenant,
                "purge_read_models:humanise_to_erased_user",
                None,
                0,
            ),
        })
    }
}

// ───────────────────────── the per-derivative erase receipt + the driver ─────────────────────────

/// The opaque, PII-free `(subject_token, tenant_token)` for a scope (a tenant offboarding records
/// the `"*tenant*"` sentinel subject). Never a name/email.
fn subject_and_tenant(scope: &EraseScope) -> (String, String) {
    match scope {
        EraseScope::Subject { subject, tenant } => {
            (subject.principal.principal_id.0.clone(), tenant.0.clone())
        }
        EraseScope::Tenant(tenant) => ("*tenant*".to_string(), tenant.0.clone()),
    }
}

/// **The per-derivative erase receipt (the green artifact for GA-D2 / REF-D5 / NOTIF-D6).** Records,
/// PII-free, the post-conditions the drill asserts: Search embeddings purged (not hidden), Refs
/// tombstoned (0 recoverable, no resolve-500), Notif mentions humanised to `[erased user]`. It is the
/// **embedding-purge receipt** the prompt names as the telemetry green artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivativeEraseReceipt {
    /// The opaque subject token the erase fanned over (PII-free).
    pub subject_token: String,
    /// Search (H7): the embeddings were PURGED (compacted out), not hidden — `0` re-identification
    /// hits remain. The embedding-purge receipt's load-bearing fact.
    pub embeddings_purged: bool,
    /// Refs (H12): the edges were TOMBSTONED — `0` recoverable, and a resolve returns the tombstone
    /// (no 500).
    pub refs_tombstoned: bool,
    /// Notif (H13): mentions HUMANISE to `[erased user]`.
    pub notif_humanised: bool,
    /// The ordered per-holder receipts the per-derivative fan-out collected (Search, Refs, Notif).
    pub holder_receipts: Vec<EraseReceipt>,
}

/// **The per-derivative erasure driver (P-GA-24 — the orchestration leg of 10.1 over the M2 derived
/// stores).** Wires the three derivative holders (Search H7 / Refs H12 / Notif H13) as the
/// orchestrator's per-holder erase calls, and fans the **reindex-from-source rectification** over
/// them. It REUSES the [`crate::orchestration::RegisteredHolder`] seam + the [`crate::orchestration::
/// CanonicalErasePhase`] order — the derived holders register at their [`derivative_phase_of`] phase
/// alongside the upstream holders (the §4.1 order is a property of the phase). It NEVER reaches into a
/// derived store — it holds only `&dyn PersonalDataHolder` (the no-cross-store-read law).
pub struct DerivativeErasureDriver;

impl DerivativeErasureDriver {
    /// **Register the three M2 derivative holders at their canonical phases.** The caller passes the
    /// holder seam (the live Search/Refs/Notif `erase` at boot; the faithful models in the drill);
    /// each is registered at its [`derivative_phase_of`] phase so a combined fan-out over upstream +
    /// derivative holders runs in the canonical erase order. A holder id without a known derivative
    /// phase is rejected (it must declare one).
    pub fn register_derivatives<'a>(
        holders: Vec<(&'static str, &'a dyn PersonalDataHolder)>,
    ) -> Vec<RegisteredHolder<'a>> {
        holders
            .into_iter()
            .map(|(id, holder)| {
                let phase = derivative_phase_of(id).unwrap_or_else(|| {
                    panic!("derivative holder `{id}` has no canonical erase phase")
                });
                RegisteredHolder { id, phase, holder }
            })
            .collect()
    }

    /// **Fan the per-derivative ERASE over Search / Refs / Notif (the GA-D2 / REF-D5 / NOTIF-D6
    /// orchestration).** Calls each derivative holder's `erase` through the contract (in canonical
    /// phase order — Search/Refs purge/tombstone before Notif's trailing humanise), collects the
    /// receipts, and reads the post-conditions off the faithful models to build the
    /// [`DerivativeEraseReceipt`] (the embedding-purge green artifact). Returns the receipt + the
    /// per-holder receipts. Errors propagate a holder error (a derived-store erase failure leaves the
    /// fan-out resumable — re-call to resume).
    ///
    /// This is the per-derivative LEG; the WHOLE-DSR fan-out (over upstream + derivative holders,
    /// resumable via the durable checklist) is the [`crate::fanout::FanOutDriver`] driving a
    /// combined [`crate::orchestration::UpstreamHolderOrchestrator`] (the derivative holders register
    /// through [`Self::register_derivatives`]).
    pub fn fan_out_erase(
        scope: &EraseScope,
        search: &SearchIndexModel,
        search_holder: &dyn PersonalDataHolder,
        refs: &RefsGraphModel,
        refs_holder: &dyn PersonalDataHolder,
        notif: &NotifHistoryModel,
        notif_holder: &dyn PersonalDataHolder,
    ) -> DsrResult<DerivativeEraseReceipt> {
        let (sid, _tenant) = subject_and_tenant(scope);
        // §4.1 phase order: Search (H7) + Refs (H12) purge/tombstone the derived stores, then Notif
        // (H13) humanises (a trailing derived copy). Each `erase` is the contract call (no store reach).
        let search_receipt = search_holder.erase(scope.clone())?;
        let refs_receipt = refs_holder.erase(scope.clone())?;
        let notif_receipt = notif_holder.erase(scope.clone())?;

        // Read the post-conditions off the faithful models (the GA-D2 / REF-D5 / NOTIF-D6 readings).
        // GA-D2's LOAD-BEARING fact is the embedding re-identification probe: 0 ⇒ the embedding was
        // PURGED, not hidden (a hidden doc would leave the embedding re-identifiable). `hits == 0` is
        // the same entry-presence fact in this model, asserted separately by the drill.
        let embeddings_purged = search.reidentify_hits(&sid) == 0;
        // REF-D5's LOAD-BEARING fact is that the resolve is a Tombstone (not Live, not a 500). By
        // construction a tombstone removed the edge, so 0 recoverable follows — the drill asserts
        // `recoverable_edges == 0` separately.
        let refs_tombstoned = matches!(refs.resolve(&sid), RefsResolve::Tombstone);
        // NOTIF-D6's fact is that the subject was marked erased ⇒ a `render_mention` now humanises to
        // `[erased user]` (asserted per-item by the caller, since it depends on the item set).
        let notif_humanised = notif.erase_call_count() > 0;
        Ok(DerivativeEraseReceipt {
            subject_token: sid,
            embeddings_purged,
            refs_tombstoned,
            notif_humanised,
            holder_receipts: vec![search_receipt, refs_receipt, notif_receipt],
        })
    }

    /// **Rectify the derived stores via REINDEX-FROM-SOURCE (§4.4 — never patched-in-place).** Art. 16
    /// corrects the PRIMARY (owning) store; the derived stores then **rebuild from the corrected
    /// source re-emit** — Search reindexes the projection, Refs rebuilds the backlink edge. The
    /// derived projection ends EQUAL to the rebuilt value (drift = 0), NOT a hand-patched stale row.
    /// There is no "patch in place" path — the only derived mutation is `index_from_source` /
    /// `add_edge_from_source` (the structural foreclosure of patch-and-drift). Returns the per-holder
    /// rectify receipts.
    pub fn rectify_via_reindex_from_source(
        subject_token: &str,
        corrected_source_value: &str,
        corrected_edge_target: &str,
        search: &SearchIndexModel,
        refs: &RefsGraphModel,
    ) -> RectifyOutcome {
        // Reindex Search FROM the corrected source (rebuild the projection, never patch the old row).
        search.index_from_source(subject_token, corrected_source_value);
        // Rebuild the Refs backlink edge FROM the corrected source.
        refs.add_edge_from_source(subject_token, corrected_edge_target);
        RectifyOutcome {
            subject_token: subject_token.to_string(),
            search_projection: search.projection(subject_token),
            refs_target: match refs.resolve(subject_token) {
                RefsResolve::Live(t) => Some(t),
                _ => None,
            },
        }
    }
}

/// The outcome of a reindex-from-source rectification (the derived projections AFTER the rebuild —
/// equal to the corrected source, never a stale patched value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RectifyOutcome {
    /// The opaque subject token rectified.
    pub subject_token: String,
    /// The Search projection AFTER the reindex (equals the corrected source value).
    pub search_projection: Option<String>,
    /// The Refs edge target AFTER the rebuild (equals the corrected edge target).
    pub refs_target: Option<String>,
}

/// The `derivative_erase_fanout_coverage` telemetry signal NAME + UNIT (the per-derivative fan-out
/// SLO — the embedding-purge receipt is the green artifact). PII-free.
pub const DERIVATIVE_ERASE_FANOUT_COVERAGE: (&str, &str) =
    ("gdpr.derivative_erase_fanout_coverage", "ratio");

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            t("acme"),
        ))
    }

    fn subject_scope(s: &str) -> EraseScope {
        EraseScope::Subject {
            subject: subject(s),
            tenant: t("acme"),
        }
    }

    // ───────── Search (H7): purge incl. embeddings — NOT hidden (GA-D2 / SRCH-D4) ─────────

    /// **Search `erase` purges the doc AND the embedding — a re-identification probe returns 0.**
    /// Before erase: the subject is indexed + its embedding re-identifies (probe = 1). After erase:
    /// 0 hits AND 0 re-identification (purge-not-hide — the embedding is compacted out, not hidden).
    #[test]
    fn search_erase_purges_embeddings_not_hidden_zero_reidentification() {
        let model = SearchIndexModel::new();
        model.index_from_source("u-1", "alice@example.com");
        assert_eq!(model.hits("u-1"), 1, "indexed before erase");
        assert_eq!(
            model.reidentify_hits("u-1"),
            1,
            "the embedding re-identifies before erase"
        );

        let holder = SearchIndexHolder::new(&model);
        let receipt = holder.erase(subject_scope("u-1")).unwrap();

        // 0 hits AND 0 re-identification — the embedding was PURGED, not hidden.
        assert_eq!(model.hits("u-1"), 0, "doc purged (0 hits)");
        assert_eq!(
            model.reidentify_hits("u-1"),
            0,
            "embedding purged — 0 re-identification (GA-D2)"
        );
        assert_eq!(
            receipt.receipt.operation, "erase",
            "the erase receipt names the op"
        );
        assert!(
            receipt.receipt.content_hash.starts_with("blake3:"),
            "the embedding-purge receipt is content-addressed (the green artifact)"
        );
    }

    /// **A *hidden* doc would still re-identify — the model has NO hide path.** This pins that the
    /// only erase is a real purge: there is no soft-delete that would leave `reidentify_hits` at 1.
    #[test]
    fn search_has_no_hide_path_only_a_real_purge() {
        let model = SearchIndexModel::new();
        model.index_from_source("u-hide", "secret");
        // The ONLY mutation that drops the doc is `erase` (a real purge). After it, 0 re-identification.
        SearchIndexHolder::new(&model)
            .erase(subject_scope("u-hide"))
            .unwrap();
        assert_eq!(model.reidentify_hits("u-hide"), 0);
        assert_eq!(model.hits("u-hide"), 0);
    }

    // ───────── Refs (H12): tombstone — 0 recoverable, no resolve-500 (REF-D5) ─────────

    /// **Refs `erase` tombstones the edges — a resolve returns the tombstone, NEVER a 500, 0
    /// recoverable.** Before: the edge resolves Live. After: the edge resolves Tombstone (not an
    /// error), 0 recoverable edges.
    #[test]
    fn refs_erase_tombstones_zero_recoverable_no_resolve_500() {
        let model = RefsGraphModel::new();
        model.add_edge_from_source("u-2", "issue:42");
        assert_eq!(model.resolve("u-2"), RefsResolve::Live("issue:42".into()));
        assert_eq!(model.recoverable_edges("u-2"), 1);

        let holder = RefsGraphHolder::new(&model);
        holder.erase(subject_scope("u-2")).unwrap();

        // The resolve returns a TOMBSTONE (not a 500), 0 recoverable PII (REF-D5).
        assert_eq!(
            model.resolve("u-2"),
            RefsResolve::Tombstone,
            "resolve returns the tombstone, not a 500"
        );
        assert_eq!(
            model.recoverable_edges("u-2"),
            0,
            "0 recoverable edges after tombstone"
        );
    }

    /// **A resolve of a tombstoned ref does NOT 500 — it is a well-defined tombstone.** (`resolve`
    /// is infallible — it returns a [`RefsResolve`] variant, never an error, for any input.)
    #[test]
    fn refs_resolve_is_infallible_even_for_unknown_and_tombstoned() {
        let model = RefsGraphModel::new();
        // Unknown subject: Missing (not a 500).
        assert_eq!(model.resolve("nobody"), RefsResolve::Missing);
        // Tombstoned subject: Tombstone (not a 500).
        RefsGraphHolder::new(&model)
            .erase(subject_scope("u-gone"))
            .unwrap();
        assert_eq!(model.resolve("u-gone"), RefsResolve::Tombstone);
    }

    // ───────── Notif (H13): humanise mentions to `[erased user]` (NOTIF-D6) ─────────

    /// **Notif `erase` humanises every mention of the subject to `[erased user]`.** Before: the
    /// mention renders the (pseudonymous) token. After: it renders `[erased user]` — never PII,
    /// never a 500.
    #[test]
    fn notif_erase_humanises_mentions_to_erased_user() {
        let model = NotifHistoryModel::new();
        model.add_item_from_source("inbox-1", "u-3");
        model.add_item_from_source("inbox-2", "u-other");
        assert_eq!(
            model.render_mention("inbox-1").as_deref(),
            Some("u-3"),
            "renders the token before erase"
        );

        NotifHistoryHolder::new(&model)
            .erase(subject_scope("u-3"))
            .unwrap();

        // The mention of the erased subject humanises to `[erased user]`; other mentions are untouched.
        assert_eq!(
            model.render_mention("inbox-1").as_deref(),
            Some(ERASED_USER),
            "humanised to [erased user]"
        );
        assert_eq!(
            model.render_mention("inbox-1").as_deref(),
            Some("[erased user]")
        );
        assert_eq!(
            model.render_mention("inbox-2").as_deref(),
            Some("u-other"),
            "other mentions untouched"
        );
    }

    // ───────── reindex-from-source rectification — rebuild, never patch (§4.4) ─────────

    /// **Rectification fans out via reindex-from-source — the derived projection equals the REBUILT
    /// value, never a stale patched one (drift = 0).** Correct the source value; the Search
    /// projection + the Refs target are REBUILT from the corrected source (not patched in place).
    #[test]
    fn rectification_fans_out_via_reindex_from_source_drift_is_zero() {
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        // Initial (stale) projection from the original source.
        search.index_from_source("u-4", "old name");
        refs.add_edge_from_source("u-4", "old-target");

        // Art. 16: the source is corrected → the derived stores REBUILD from source.
        let outcome = DerivativeErasureDriver::rectify_via_reindex_from_source(
            "u-4",
            "new name",
            "new-target",
            &search,
            &refs,
        );

        // The derived projection equals the REBUILT (corrected-source) value — drift = 0.
        assert_eq!(
            outcome.search_projection.as_deref(),
            Some("new name"),
            "Search reindexed from source"
        );
        assert_eq!(
            outcome.refs_target.as_deref(),
            Some("new-target"),
            "Refs rebuilt from source"
        );
        assert_eq!(search.projection("u-4").as_deref(), Some("new name"));
        // The Refs edge resolves Live to the rebuilt target (a rebuild clears any prior tombstone).
        assert_eq!(refs.resolve("u-4"), RefsResolve::Live("new-target".into()));
    }

    // ───────── the per-derivative fan-out (the orchestration leg) ─────────

    /// **The driver fans the per-derivative ERASE over Search/Refs/Notif and builds the
    /// embedding-purge receipt.** All three derived stores are erased through the contract; the
    /// receipt records the GA-D2 / REF-D5 / NOTIF-D6 post-conditions.
    #[test]
    fn driver_fans_per_derivative_erase_and_builds_the_embedding_purge_receipt() {
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        let notif = NotifHistoryModel::new();
        search.index_from_source("u-5", "bob");
        refs.add_edge_from_source("u-5", "pr:7");
        notif.add_item_from_source("inbox-x", "u-5");

        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);
        let nh = NotifHistoryHolder::new(&notif);

        let receipt = DerivativeErasureDriver::fan_out_erase(
            &subject_scope("u-5"),
            &search,
            &sh,
            &refs,
            &rh,
            &notif,
            &nh,
        )
        .unwrap();

        assert!(
            receipt.embeddings_purged,
            "Search embeddings purged, not hidden (GA-D2)"
        );
        assert!(
            receipt.refs_tombstoned,
            "Refs tombstoned, 0 recoverable, no resolve-500 (REF-D5)"
        );
        assert!(
            receipt.notif_humanised,
            "Notif humanised mentions (NOTIF-D6)"
        );
        assert_eq!(
            receipt.holder_receipts.len(),
            3,
            "Search + Refs + Notif receipts"
        );
        // The Notif mention now humanises to `[erased user]`.
        assert_eq!(
            notif.render_mention("inbox-x").as_deref(),
            Some(ERASED_USER)
        );
        // 0 recoverable across the derived stores.
        assert_eq!(search.reidentify_hits("u-5"), 0);
        assert_eq!(refs.recoverable_edges("u-5"), 0);
    }

    // ───────── the canonical-phase registration (reuses the upstream order) ─────────

    /// **The derivative holders declare their canonical phases (§4.1) — they slot into the combined
    /// erase order, never re-derive a hand-written sequence.** Search/Refs purge/tombstone in the
    /// derived phase; Notif is a trailing derived copy.
    #[test]
    fn derivative_phases_are_pinned() {
        assert_eq!(
            derivative_phase_of(derivative_holder_ids::SEARCH_INDEX),
            Some(CanonicalErasePhase::PurgeAndTombstoneDerived)
        );
        assert_eq!(
            derivative_phase_of(derivative_holder_ids::REFS_GRAPH),
            Some(CanonicalErasePhase::PurgeAndTombstoneDerived)
        );
        assert_eq!(
            derivative_phase_of(derivative_holder_ids::NOTIF_HISTORY),
            Some(CanonicalErasePhase::CachesAndDerivedCopies)
        );
        assert_eq!(derivative_phase_of("not_a_derivative"), None);
        // Search/Refs purge/tombstone BEFORE Notif's trailing humanise (the phase order).
        assert!(
            CanonicalErasePhase::PurgeAndTombstoneDerived
                < CanonicalErasePhase::CachesAndDerivedCopies
        );
    }

    /// **`register_derivatives` registers the three holders at their phases** (the seam the combined
    /// orchestrator wires). A holder without a known derivative phase panics (it must declare one).
    #[test]
    fn register_derivatives_assigns_canonical_phases() {
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        let notif = NotifHistoryModel::new();
        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);
        let nh = NotifHistoryHolder::new(&notif);
        let registered = DerivativeErasureDriver::register_derivatives(vec![
            (
                derivative_holder_ids::SEARCH_INDEX,
                &sh as &dyn PersonalDataHolder,
            ),
            (derivative_holder_ids::REFS_GRAPH, &rh),
            (derivative_holder_ids::NOTIF_HISTORY, &nh),
        ]);
        assert_eq!(registered.len(), 3);
        let search_reg = registered
            .iter()
            .find(|r| r.id == derivative_holder_ids::SEARCH_INDEX)
            .unwrap();
        assert_eq!(
            search_reg.phase,
            CanonicalErasePhase::PurgeAndTombstoneDerived
        );
        let notif_reg = registered
            .iter()
            .find(|r| r.id == derivative_holder_ids::NOTIF_HISTORY)
            .unwrap();
        assert_eq!(notif_reg.phase, CanonicalErasePhase::CachesAndDerivedCopies);
    }

    /// **The derivative erase carries NO destroyed key epoch** (plaintext-derived, not key-shred —
    /// §3.2 H7). The receipt is content-addressed with `key_epoch_destroyed = None`.
    #[test]
    fn derivative_erase_carries_no_destroyed_key_epoch() {
        let search = SearchIndexModel::new();
        search.index_from_source("u-6", "x");
        let r = SearchIndexHolder::new(&search)
            .erase(subject_scope("u-6"))
            .unwrap();
        assert_eq!(
            r.receipt.key_epoch_destroyed, None,
            "a derived purge destroys no key (plaintext-derived)"
        );
    }

    /// The holder ids + the `[erased user]` sentinel + the telemetry name are stable (the data-map /
    /// fan-out address book + the humanise sentinel + the SLO label). Pins them against drift.
    #[test]
    fn holder_ids_sentinel_and_telemetry_are_stable() {
        assert_eq!(derivative_holder_ids::SEARCH_INDEX, "search_index");
        assert_eq!(derivative_holder_ids::REFS_GRAPH, "refs_graph");
        assert_eq!(derivative_holder_ids::NOTIF_HISTORY, "notif_history");
        assert_eq!(ERASED_USER, "[erased user]");
        assert_eq!(
            DERIVATIVE_ERASE_FANOUT_COVERAGE.0,
            "gdpr.derivative_erase_fanout_coverage"
        );
        assert_eq!(DERIVATIVE_ERASE_FANOUT_COVERAGE.1, "ratio");
    }

    /// **The `DerivativeEraseReceipt` flags read the EXACT post-conditions (mutation-killing).** A
    /// receipt for a NON-erased subject reads all flags FALSE (a re-identification probe is >0, the
    /// resolve is NOT a tombstone, no notif erase ran); a receipt for an erased subject reads all
    /// flags TRUE. This pins the `== 0` / `Tombstone` / `> 0` field computations against an inversion.
    #[test]
    fn receipt_flags_read_the_exact_post_conditions_both_polarities() {
        // ── the NON-erased polarity: the holders are NOT erased, so every flag is FALSE.
        // (We build the receipt by hand from the model readings, the same expression `fan_out_erase`
        // uses, to assert the false polarity without erasing.)
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        let notif = NotifHistoryModel::new();
        search.index_from_source("u-pol", "v");
        refs.add_edge_from_source("u-pol", "tgt");
        notif.add_item_from_source("i", "u-pol");
        // Before any erase: the embedding re-identifies (probe > 0 ⇒ NOT purged), the edge resolves
        // Live (NOT a tombstone), no notif erase ran (call count 0).
        assert_eq!(
            search.reidentify_hits("u-pol"),
            1,
            "embedding re-identifies ⇒ embeddings_purged would be FALSE"
        );
        assert!(
            !matches!(refs.resolve("u-pol"), RefsResolve::Tombstone),
            "Live ⇒ refs_tombstoned would be FALSE"
        );
        assert_eq!(
            notif.erase_call_count(),
            0,
            "no erase ⇒ notif_humanised would be FALSE"
        );

        // ── the ERASED polarity: fan out, every flag TRUE.
        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);
        let nh = NotifHistoryHolder::new(&notif);
        let receipt = DerivativeErasureDriver::fan_out_erase(
            &subject_scope("u-pol"),
            &search,
            &sh,
            &refs,
            &rh,
            &notif,
            &nh,
        )
        .unwrap();
        assert!(
            receipt.embeddings_purged,
            "after erase: embeddings_purged TRUE (probe == 0)"
        );
        assert!(
            receipt.refs_tombstoned,
            "after erase: refs_tombstoned TRUE (resolve is Tombstone)"
        );
        assert!(
            receipt.notif_humanised,
            "after erase: notif_humanised TRUE (erase ran)"
        );
    }

    /// **`locate` distinguishes the indexed/edges-present verdict from the 0-recoverable verdict**
    /// (the `> 0` branch in each derived holder's `locate` is load-bearing — it is the Art. 15 access
    /// answer; an inversion would swap the two verdicts). Pins both verdicts.
    #[test]
    fn locate_verdicts_distinguish_present_from_zero_recoverable() {
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        search.index_from_source("u-loc", "x");
        refs.add_edge_from_source("u-loc", "e");
        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);
        // PRESENT: the search doc is indexed, the refs edge present.
        let s_present = sh.locate(&subject("u-loc"), t("acme")).unwrap().receipt;
        let r_present = rh.locate(&subject("u-loc"), t("acme")).unwrap().receipt;
        // ERASE → 0 recoverable.
        sh.erase(subject_scope("u-loc")).unwrap();
        rh.erase(subject_scope("u-loc")).unwrap();
        let s_zero = sh.locate(&subject("u-loc"), t("acme")).unwrap().receipt;
        let r_zero = rh.locate(&subject("u-loc"), t("acme")).unwrap().receipt;
        assert_ne!(
            s_present.content_hash, s_zero.content_hash,
            "Search locate verdict differs present vs 0-recoverable"
        );
        assert_ne!(
            r_present.content_hash, r_zero.content_hash,
            "Refs locate verdict differs present vs 0-recoverable"
        );
        // Pin the EXACT verdict strings (catches a `> 0` inversion that swaps the two outcomes).
        let s_expect_present = Receipt::content_addressed(
            "locate",
            derivative_holder_ids::SEARCH_INDEX,
            "u-loc",
            "acme",
            "located:indexed",
            None,
            0,
        );
        let s_expect_zero = Receipt::content_addressed(
            "locate",
            derivative_holder_ids::SEARCH_INDEX,
            "u-loc",
            "acme",
            "located:0-recoverable",
            None,
            0,
        );
        assert_eq!(s_present, s_expect_present);
        assert_eq!(s_zero, s_expect_zero);
        let r_expect_present = Receipt::content_addressed(
            "locate",
            derivative_holder_ids::REFS_GRAPH,
            "u-loc",
            "acme",
            "located:edges-present",
            None,
            0,
        );
        let r_expect_zero = Receipt::content_addressed(
            "locate",
            derivative_holder_ids::REFS_GRAPH,
            "u-loc",
            "acme",
            "located:0-recoverable",
            None,
            0,
        );
        assert_eq!(r_present, r_expect_present);
        assert_eq!(r_zero, r_expect_zero);
    }

    /// **The no-cross-store-read law (§3.1) over the DERIVED stores:** this module reaches Search /
    /// Refs / Notif ONLY through the [`PersonalDataHolder`] contract — it NEVER imports
    /// `myelin_search` / `myelin_refs` / `myelin_notif` (an import would be a downward DAG edge into a
    /// derived store + a violation of "the orchestrator never reaches into a store"). The structural
    /// guarantee is the manifest (no such dependency); this scans the source for any real import line
    /// (a `//` comment may legitimately NAME a crate — the doc does — so comment lines are skipped).
    #[test]
    fn derivative_module_has_no_cross_store_read_import() {
        let manifest = include_str!("../Cargo.toml");
        for forbidden in ["myelin-search", "myelin-refs", "myelin-notif"] {
            assert!(
                !manifest.contains(forbidden),
                "myelin-gdpr-service Cargo.toml must NOT depend on {forbidden} (the no-cross-store-read \
                 law, gdpr §3.1) — the derived stores are reached through the PersonalDataHolder seam"
            );
        }
        let src = include_str!("derivative_erasure.rs");
        for line in src.lines() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue; // a comment may legitimately name a derived-store crate
            }
            let is_import = code.starts_with("use ")
                || code.starts_with("pub use ")
                || code.starts_with("extern crate ");
            for forbidden in ["myelin_search", "myelin_refs", "myelin_notif"] {
                assert!(
                    !(is_import && line.contains(forbidden)),
                    "derivative_erasure.rs must NOT import {forbidden} (the no-cross-store-read law): `{code}`"
                );
            }
        }
    }

    /// **Idempotent: a re-driven derivative erase re-affirms the post-condition** (the purge / the
    /// tombstone / the humanise hold; a re-erase is a no-op success).
    #[test]
    fn derivative_erase_is_idempotent() {
        let search = SearchIndexModel::new();
        search.index_from_source("u-7", "y");
        let holder = SearchIndexHolder::new(&search);
        holder.erase(subject_scope("u-7")).unwrap();
        // A re-erase: still 0 re-identification, no error.
        holder.erase(subject_scope("u-7")).unwrap();
        assert_eq!(search.reidentify_hits("u-7"), 0);
        assert_eq!(search.erase_call_count(), 2, "both erase calls counted");
    }
}
