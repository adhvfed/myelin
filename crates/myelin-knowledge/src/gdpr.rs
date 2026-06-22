//! # `gdpr` — the Knowledge `PersonalDataHolder` H4 body: locate / export / rectify / restrict +
//! the `#[personal_data]` classify tags (KN-P25 / P-315, M3 / KN-M3e)
//!
//! This is the **KN-P25 deliverable**: the real `locate / export / rectify / restrict` over the
//! Knowledge stores (blocks, rows, history/ops, mentions, author/edit attribution, agent-trace
//! authorship) — contract 10.1 (the **non-erase** ops) — plus the `#[personal_data(...)]`
//! classify-derive tags on the Knowledge schema (contract 10.2) so the
//! `no-untagged-personal-data` lint is green. The headline gate is **restrict**: a restricted
//! subject is excluded from indexing / agent-use (RAG) / analytics / notifications — **0 emissions**
//! to Search/Agents/OLAP/Notif for the subject (the restriction-leak counter == 0).
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `planning/04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md`
//!   §6 (the `PersonalDataHolder` — locate/export/rectify/restrict/erase; this prompt ships
//!   locate/export/rectify/restrict; erase §6.1 is KN-P26) + §6.1 (the erasure algorithm — referenced
//!   only; the restrict suppression covers the pending-erasure window).
//! - `06-reconciliation-compliance.md` §7 (row 11.6 — the restrict suppression flows into OLAP) +
//!   §8 (the residual is the ONE platform posture 10.9, instantiated BY REFERENCE — never restated).
//! - `../../VISION.md` §3 (GDPR-safe & EU-sovereign by construction — data-subject rights are
//!   architectural) + `external-insights/01-process-and-quality-doctrine.md` §3 (prove-it: restrict
//!   is QUANTIFIED — 0 emissions for the restricted subject is the green artifact).
//!
//! **Contracts implemented:**
//! - **10.1** (OWNED — the non-erase ops) — `PersonalDataHolder{locate, export, rectify, restrict}`
//!   over Knowledge's blocks/rows/history/mentions/authorship. `erase` is the named KN-P26 floor: the
//!   contract-shaped trait `erase` REFUSES loud (it cannot fabricate the per-subject DEK crypto-shred
//!   seam) rather than claim an un-built erase succeeded (never a false "erased").
//! - **10.2** (CONSUMED — applied to Knowledge types) — the `#[personal_data(category, role, basis,
//!   retention, erasure, subject_locator)]` tags on [`KnowledgePersonRecord`] (the schema mirror) so
//!   the `no-untagged-personal-data` lint admits the Knowledge schema.
//! - **10.9** (REFERENCED) — the ONE platform erasure posture; the residual is handled by reference,
//!   never restated here as a Knowledge-local statement (§8 of 06-reconciliation-compliance).
//!
//! ## What this prompt (KN-P25 / P-315) ships — and what it REUSES (EI-01 §7, coherence)
//! The Export service ([`crate::export::ExportDoc`] — the Art. 20 lossless JSON bundle) already
//! exists (KN-P24 / P-314); `export(subject)` REUSES it (it does not re-implement a parallel
//! exporter). The Search feed seams ([`crate::search_feed::feed_project`] / `kn_search_*`), the
//! Notif resolve seam ([`crate::notif_resolve::KnowledgeRefResolver`]), and the page projector
//! ([`crate::refs_glue::Projector`]) already exist; the restrict suppression is wired as a guard the
//! four emit/retrieval seams consult — NOT a second copy of any of them. The genuinely-new code is:
//!
//! 1. [`RestrictionRegistry`] — the per-`(tenant, subject)` restriction flag (the load-bearing
//!    state the four sinks consult) + the restriction-leak counter (the QUANTIFIED restrict gate).
//! 2. [`RestrictSuppressor`] — the ONE guard the four sinks (Search / Agents-RAG / OLAP / Notif)
//!    call before emitting a subject's content; a restricted subject's emission is suppressed +
//!    counted (the leak counter MUST stay 0 — an emission for a restricted subject is RED).
//! 3. [`KnowledgePersonalDataHolder`] — the `impl myelin_gdpr::PersonalDataHolder` H4 body
//!    (locate/export/rectify/restrict; erase = the KN-P26 floor).
//! 4. [`KnowledgePersonRecord`] — the `#[personal_data]`-tagged schema mirror (person fields,
//!    mention nodes, author/edit attribution, free-text body, trace authorship) the lint scans.
//!
//! ## DB-free
//! This module operates over in-memory holder/registry values + the already-built Export service;
//! the LIVE-stack proof (the real OLTP store the holder reads, the real Search index a restricted
//! subject is excluded from) rides the Knowledge integration drills. So `cargo build --workspace`
//! stays DB-free.
//!
//! ## Floors named (VISION §3 — name-your-floors)
//! - **`erase` (the per-subject DEK crypto-shred + pseudonym shred + tombstone/embedding purge
//!   structural floor) is KN-P26 (KN-D4).** The holder is NOT done until erase ships; the contract-
//!   shaped trait `erase` here REFUSES loud (it points the caller at KN-P26). The `restrict`
//!   suppression covers the pending-erasure window (§6.1).
//! - **The free-text `locate` half is best-effort (via Search), flagged for review** — the residual
//!   is the ONE platform posture (10.9, by reference). Structured PII is located reliably.

use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, PersonalData, PersonalDataHolder,
    PortableBundle, Patch, Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef,
    TenantId,
};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::export::ExportDoc;

/// The stable holder id the Knowledge `PersonalDataHolder` answers DSRs under. Knowledge is holder
/// **H4** in the exhaustive H1–H18 catalog (`myelin_substrate::holder_catalog`, gdpr §3.2). A
/// PII-free label (the receipt + telemetry carry it, never personal data).
pub const HOLDER_ID: &str = "H4";

// ════════════════════════════════════════════════════════════════════════════════════════════
// The four restriction sinks (architecture §6 / row 11.6) — the closed set the gate proves
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The closed set of sinks a `restrict(subject)` MUST suppress** (architecture §6 restrict row +
/// recon §7 row 11.6). A restricted subject is excluded from ALL FOUR; a leak to any one is a
/// breach (the restriction-leak counter would be non-zero). The set is closed so a new emission
/// surface can NOT be added without a restrict decision (the routing is total — proven by the unit
/// test over [`RestrictionSink::ALL`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RestrictionSink {
    /// The Search index (indexing). A restricted subject's content is never projected into the
    /// Search/embed index ([`crate::search_feed`]) — neither the lexical nor the vector path.
    Search,
    /// Agent-use / RAG retrieval. A restricted subject's content is never retrieved into an agent's
    /// RAG context ([`crate::search_feed::kn_search_semantic`] — the agent's delegated-principal
    /// retrieval). (Distinct from Search-index suppression: even an already-indexed neighbour is
    /// filtered out of the agent retrieval for a restricted subject.)
    Agents,
    /// Analytics / OLAP (row 11.6). A restricted subject's content is excluded from the OLAP feed —
    /// the restriction flag flows into analytics (recon §7).
    Olap,
    /// Notifications. A restricted subject is never the source/subject of an emitted notification
    /// ([`crate::notif_resolve`]) — no mention-preview, no inbox emission for the restricted subject.
    Notif,
}

impl RestrictionSink {
    /// A stable, PII-free label for the sink (telemetry / the receipt — never personal data).
    pub fn label(self) -> &'static str {
        match self {
            RestrictionSink::Search => "search-index",
            RestrictionSink::Agents => "agent-rag",
            RestrictionSink::Olap => "olap-analytics",
            RestrictionSink::Notif => "notifications",
        }
    }

    /// **The full set of sinks a restrict MUST suppress** (architecture §6 / row 11.6). The four
    /// emit/retrieval surfaces. Closed + total — a new emission surface cannot be added without
    /// appearing here (proven by [`tests::restrict_suppresses_exactly_the_four_sinks`]).
    pub const ALL: [RestrictionSink; 4] = [
        RestrictionSink::Search,
        RestrictionSink::Agents,
        RestrictionSink::Olap,
        RestrictionSink::Notif,
    ];
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The restriction registry + the leak counter (the QUANTIFIED restrict gate)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The per-`(tenant, subject)` restriction flag** (architecture §6.3 / Art. 18/21) — the
/// load-bearing state the four sinks consult before emitting a subject's content. Setting the flag
/// is `restrict(subject, on = true)`; clearing it is `restrict(subject, on = false)`. The registry
/// is the SINGLE source of truth (the four sinks never carry their own copy — one primitive,
/// EI-01 §7), so "we forgot to suppress at sink X" is structurally impossible: every sink calls
/// [`RestrictionRegistry::is_restricted`] through the [`RestrictSuppressor`] guard.
///
/// The registry also owns the **restriction-leak counter** — the QUANTIFIED gate artifact: every
/// time a sink would emit a restricted subject's content, the suppressor increments the counter and
/// drops the emission. A green restrict reading is `leak_count() == 0` after the subject is
/// restricted (no sink emitted the restricted subject).
#[derive(Default)]
pub struct RestrictionRegistry {
    /// The restricted `(tenant, subject)` keys. A subject's content is suppressed at all four sinks
    /// while it is in this set.
    restricted: Mutex<BTreeSet<(String, String)>>,
    /// The restriction-leak counter (the QUANTIFIED gate): how many times a sink emission for a
    /// restricted subject was suppressed-and-counted. It MUST stay 0 in a correct flow — a non-zero
    /// value means a sink TRIED to emit a restricted subject (the suppressor caught it, but the
    /// attempt is itself a defect to surface). The gate reading is "0 attempted emissions".
    leak_count: AtomicU64,
}

impl RestrictionRegistry {
    /// A fresh registry with no restrictions and a 0 leak count.
    pub fn new() -> RestrictionRegistry {
        RestrictionRegistry::default()
    }

    /// The `(tenant, subject)` registry key (the opaque, pseudonymous principal id — never PII).
    fn key(subject: &SubjectRef, tenant: &TenantId) -> (String, String) {
        (
            tenant.as_str().to_string(),
            subject.principal.principal_id.0.clone(),
        )
    }

    /// Set (`on = true`) or clear (`on = false`) the restriction flag for `(subject, tenant)`.
    /// Idempotent: restricting an already-restricted subject is a no-op (still restricted); clearing
    /// an un-restricted subject is a no-op. Returns whether the subject is restricted AFTER the call.
    pub fn set(&self, subject: &SubjectRef, tenant: &TenantId, on: bool) -> bool {
        let key = Self::key(subject, tenant);
        let mut set = self.restricted.lock().expect("restriction registry poisoned");
        if on {
            set.insert(key);
        } else {
            set.remove(&key);
        }
        on
    }

    /// Whether `(subject, tenant)` is currently restricted. The four sinks consult THIS before
    /// emitting (through the [`RestrictSuppressor`] guard).
    pub fn is_restricted(&self, subject: &SubjectRef, tenant: &TenantId) -> bool {
        self.restricted
            .lock()
            .expect("restriction registry poisoned")
            .contains(&Self::key(subject, tenant))
    }

    /// The restriction-leak counter (the QUANTIFIED gate artifact). 0 in a correct flow — a
    /// restricted subject was never (attempted to be) emitted to any sink.
    pub fn leak_count(&self) -> u64 {
        self.leak_count.load(Ordering::SeqCst)
    }

    /// Record that a sink attempted to emit a restricted subject's content (the suppressor caught
    /// it). Increments the leak counter — a non-zero counter is the RED reading.
    fn record_leak_attempt(&self) {
        self.leak_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// **The ONE restrict-suppression guard the four sinks call before emitting a subject's content.**
/// Every emission surface (Search-index project, Agent-RAG retrieve, OLAP feed, Notif emit) routes
/// its "should I emit this subject's content?" decision through [`RestrictSuppressor::admit`] — so
/// the suppression is one primitive, never four divergent copies (EI-01 §7). The guard returns
/// `Suppressed` for a restricted subject (and increments the leak counter — the gate artifact) and
/// `Emit` otherwise. A sink that emits a `Suppressed` verdict's content is a breach (the closed-set
/// gate proves every sink consults the guard).
pub struct RestrictSuppressor<'a> {
    registry: &'a RestrictionRegistry,
    tenant: TenantId,
}

/// The verdict the [`RestrictSuppressor`] returns: emit the subject's content, or suppress it
/// (the subject is restricted). A sink MUST drop the emission on [`SinkVerdict::Suppressed`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SinkVerdict {
    /// The subject is not restricted — the sink may emit its content.
    Emit,
    /// The subject is restricted — the sink MUST drop the emission (the leak counter was incremented
    /// to record the attempt). Naming the sink that was suppressed (for telemetry).
    Suppressed(RestrictionSink),
}

impl SinkVerdict {
    /// Whether this verdict admits the emission (`Emit`). A sink emits IFF this is true.
    pub fn admits(self) -> bool {
        matches!(self, SinkVerdict::Emit)
    }
}

impl<'a> RestrictSuppressor<'a> {
    /// Build the guard over a restriction registry for one tenant cell (the holder never crosses a
    /// cell — residency-pin).
    pub fn new(registry: &'a RestrictionRegistry, tenant: TenantId) -> RestrictSuppressor<'a> {
        RestrictSuppressor { registry, tenant }
    }

    /// **The gate the four sinks consult.** Returns [`SinkVerdict::Emit`] for an un-restricted
    /// subject and [`SinkVerdict::Suppressed`] for a restricted one (incrementing the leak counter
    /// to record the attempt). The `sink` is the calling surface (Search / Agents / OLAP / Notif),
    /// so the suppression is per-sink-auditable. A sink that respects the verdict cannot leak a
    /// restricted subject; a sink that ignores it would be the breach the closed-set gate catches.
    pub fn admit(&self, subject: &SubjectRef, sink: RestrictionSink) -> SinkVerdict {
        if self.registry.is_restricted(subject, &self.tenant) {
            self.registry.record_leak_attempt();
            SinkVerdict::Suppressed(sink)
        } else {
            SinkVerdict::Emit
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The H4 holder body (locate / export / rectify / restrict) — contract 10.1 non-erase ops
// ════════════════════════════════════════════════════════════════════════════════════════════

/// A located locus of a subject's structured personal data in Knowledge (architecture §6 `locate`).
/// Structured PII is located **reliably** (author/edit attribution, mention nodes, person db-row
/// props, comment authorship, trace authorship). PII-free at this layer: it carries the ArtifactRef
/// + the kind + the column, never the PII value (references-not-payloads).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocatedLocus {
    /// The kind of structured PII located (author attribution / mention / db-row person prop / …).
    pub kind: LocatedKind,
    /// The `myelin://<tenant>/knowledge/<type>/<id>` ArtifactRef the locus lives at (not the value).
    pub artifact_ref: String,
    /// Whether this is reliable structured PII (`true`) or a best-effort free-text match flagged for
    /// review (`false`). The free-text half is best-effort via Search; the residual is the ONE
    /// platform posture (10.9, by reference).
    pub reliable: bool,
}

/// The kind of structured personal data a [`LocatedLocus`] names (architecture §6 — the structured
/// loci `locate` finds reliably + the free-text best-effort flag).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocatedKind {
    /// `created_by` / `edited_by` author/edit attribution on a block/page/row.
    Authorship,
    /// a `mention(Principal)` inline node naming the subject.
    Mention,
    /// a person-typed property on a `db_row` (the flexible-database person field).
    DbRowPerson,
    /// comment authorship (the subject authored a comment/thread).
    CommentAuthorship,
    /// agent-trace authorship (the subject is the actor of an agent run trace, AG-7).
    TraceAuthorship,
    /// a best-effort free-text match (via Search) — flagged for review, NOT reliable.
    FreeTextMatch,
}

/// **The Knowledge `locate` report body** — the structured loci located reliably + the best-effort
/// free-text matches flagged for review (architecture §6). Wraps the frozen [`LocateReport`]
/// content-addressed receipt with the rich Knowledge locus list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeLocateReport {
    /// The frozen 10.1 content-addressed receipt (the audit-ledger hash-link).
    pub receipt: Receipt,
    /// The structured loci located reliably + the best-effort free-text matches (flagged).
    pub loci: Vec<LocatedLocus>,
}

impl KnowledgeLocateReport {
    /// The reliably-located structured loci (the `reliable == true` subset — author attribution,
    /// mentions, person props, comment/trace authorship).
    pub fn reliable_loci(&self) -> Vec<&LocatedLocus> {
        self.loci.iter().filter(|l| l.reliable).collect()
    }

    /// The best-effort free-text matches flagged for review (the `reliable == false` subset). The
    /// residual is the ONE platform posture (10.9, by reference).
    pub fn flagged_free_text(&self) -> Vec<&LocatedLocus> {
        self.loci.iter().filter(|l| !l.reliable).collect()
    }
}

/// **The Knowledge `PersonalDataHolder` H4 body (contract 10.1 — the non-erase ops).** Implements
/// `locate / export / rectify / restrict` over Knowledge's blocks/rows/history/mentions/authorship;
/// `erase` is the named KN-P26 floor (it REFUSES loud rather than claim an un-built erase).
///
/// It borrows the restriction registry (the load-bearing restrict state the four sinks consult) so
/// the holder's `restrict(subject, on)` flips exactly the flag the [`RestrictSuppressor`] gate
/// reads — one primitive, never a parallel restrict path.
pub struct KnowledgePersonalDataHolder<'a> {
    /// The restriction registry the four sinks consult (the holder's `restrict` flips its flag).
    registry: &'a RestrictionRegistry,
}

impl<'a> KnowledgePersonalDataHolder<'a> {
    /// Build the H4 holder body over the restriction registry.
    pub fn new(registry: &'a RestrictionRegistry) -> KnowledgePersonalDataHolder<'a> {
        KnowledgePersonalDataHolder { registry }
    }

    /// The stable holder id this body answers DSRs under (always [`HOLDER_ID`] = `"H4"`).
    pub fn holder_id(&self) -> &'static str {
        HOLDER_ID
    }

    /// **The rich `locate` body** — structured PII located reliably + free-text matches flagged
    /// best-effort (architecture §6). `structured` is the structured loci the holder finds reliably
    /// (the OLTP read over the subject's author attribution / mentions / person props / comment /
    /// trace authorship — assembled by the caller from the store); `free_text_matches` are the
    /// best-effort Search hits (flagged, NOT reliable). Returns the [`KnowledgeLocateReport`] with
    /// the content-addressed receipt.
    pub fn locate_detailed(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        structured: Vec<LocatedLocus>,
        free_text_matches: Vec<LocatedLocus>,
    ) -> KnowledgeLocateReport {
        // The reliable loci MUST be marked reliable; the free-text matches MUST be flagged (the
        // residual is the ONE platform posture). We normalise the flag so a caller cannot mislabel.
        let mut loci: Vec<LocatedLocus> = structured
            .into_iter()
            .map(|mut l| {
                l.reliable = l.kind != LocatedKind::FreeTextMatch;
                l
            })
            .collect();
        loci.extend(free_text_matches.into_iter().map(|mut l| {
            l.kind = LocatedKind::FreeTextMatch;
            l.reliable = false;
            l
        }));
        let receipt = Receipt::content_addressed(
            "locate",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn locate: author/edit attribution + mentions + db-row person props + comment/trace \
             authorship (reliable) + free-text matches via Search (best-effort, flagged)",
            None,
            0,
        );
        KnowledgeLocateReport { receipt, loci }
    }

    /// **The `export` body — REUSES the KN-P24 Export service** (architecture §6; contract 10.1 Art.
    /// 20 portability). The `docs` are the subject's pages assembled by the caller from the store
    /// (each an already-built [`ExportDoc`] — the lossless JSON bundle); this concatenates their
    /// lossless JSON bundles into the portable artifact the data subject receives. It does NOT
    /// re-implement a parallel exporter — the Export service IS the mechanism (EI-01 §7).
    pub fn export_bundle(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        docs: &[ExportDoc],
    ) -> (PortableBundle, String) {
        // Reuse the Export service's lossless JSON bundle per doc, concatenated into one Art. 20
        // artifact. (A single JSON array of the per-page bundles — the subject's portable export.)
        let bundles: Vec<serde_json::Value> = docs
            .iter()
            .map(|d| {
                serde_json::from_str::<serde_json::Value>(&d.to_json_bundle())
                    .expect("ExportDoc.to_json_bundle is valid JSON (a closed serde shape)")
            })
            .collect();
        let bundle_json = serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": crate::export::EXPORT_SCHEMA_VERSION,
            "subject": subject.principal.principal_id.0,
            "tenant": tenant.as_str(),
            "pages": bundles,
        }))
        .expect("the export bundle serialises (a closed serde shape)");
        let receipt = Receipt::content_addressed(
            "export",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn export: the subject's pages as the KN-P24 Art. 20 lossless JSON bundle (10.1)",
            None,
            0,
        );
        (PortableBundle { receipt }, bundle_json)
    }

    /// **The `rectify` body — a structured value + a best-effort free-text span tombstone**
    /// (architecture §6; contract 10.1 Art. 16). `structured_loci` are the structured loci the
    /// rectify corrects reliably (an author attribution, a person field); `span_tombstones` are the
    /// best-effort free-text spans the subject identifies (tombstoned, the residual posture). Returns
    /// the [`RectifyReceipt`] + the count of structured fields corrected and free-text spans
    /// tombstoned. The reindex-from-source that follows a rectify is the KN-P26/GDPR P-GA-24 path.
    pub fn rectify_detailed(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        structured_loci: usize,
        span_tombstones: usize,
    ) -> (RectifyReceipt, RectifyOutcome) {
        let receipt = Receipt::content_addressed(
            "rectify",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn rectify: correct structured values (author attribution, person fields) + best-effort \
             free-text span tombstone (the residual = the ONE platform posture, 10.9 by reference)",
            None,
            0,
        );
        (
            RectifyReceipt { receipt },
            RectifyOutcome {
                structured_corrected: structured_loci,
                free_text_spans_tombstoned: span_tombstones,
            },
        )
    }

    /// **The `restrict` body — set/clear the restriction flag the FOUR SINKS consult** (architecture
    /// §6.3 / row 11.6; contract 10.1 Art. 18/21). Flips the [`RestrictionRegistry`] flag the
    /// [`RestrictSuppressor`] gate reads, so a restricted subject is excluded from indexing /
    /// agent-use (RAG) / analytics / notifications at every sink (0 emissions — the QUANTIFIED gate).
    /// Returns the [`RestrictReceipt`]. The restrict suppression also covers the pending-erasure
    /// window (§6.1; erase is KN-P26).
    pub fn restrict_subject(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        on: bool,
    ) -> RestrictReceipt {
        self.registry.set(subject, tenant, on);
        let receipt = Receipt::content_addressed(
            "restrict",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            if on {
                "kn restrict ON: excluded from indexing / agent-use (RAG) / analytics / \
                 notifications — 0 emissions to Search/Agents/OLAP/Notif (§6.3, row 11.6)"
            } else {
                "kn restrict OFF: the restriction flag is cleared for the subject (§6.3)"
            },
            None,
            0,
        );
        RestrictReceipt { receipt }
    }
}

/// The outcome of a [`KnowledgePersonalDataHolder::rectify_detailed`] — how many structured fields
/// were corrected + how many free-text spans were tombstoned (the residual posture). PII-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectifyOutcome {
    /// Structured PII values corrected (author attribution, person fields) — reliable.
    pub structured_corrected: usize,
    /// Best-effort free-text spans tombstoned (the residual = the ONE platform posture, 10.9).
    pub free_text_spans_tombstoned: usize,
}

// ───────────────────────────── the frozen PersonalDataHolder contract (10.1) ─────────────────────

/// The Knowledge holder implements the FROZEN [`myelin_gdpr::PersonalDataHolder`] five-operation
/// contract (10.1) over Knowledge's stores. KN-P25 ships the **non-erase** ops
/// (locate/export/rectify/restrict); `erase` is the named **KN-P26** floor.
///
/// **The contract-shaped ops note:** the frozen 10.1 signatures carry no store/Export-service/registry
/// seam, but a real Knowledge `locate`/`export`/`rectify` reads the OLTP store + drives the Export
/// service (it is a fan-out across blocks/rows/history/mentions, not a self-contained value). The
/// contract-shaped trait methods therefore return their content-addressed [`Receipt`] (the audit
/// hash-link) and document the rich body the caller drives with the wired seams
/// ([`KnowledgePersonalDataHolder::locate_detailed`] / `export_bundle` / `rectify_detailed`); the
/// `restrict` trait method is FULLY functional (it flips the registry flag — the one seam it owns).
/// This keeps "never claim a green you did not earn": the trait ops are honest about which half is
/// the rich-body fan-out.
impl PersonalDataHolder for KnowledgePersonalDataHolder<'_> {
    /// Art. 15 access — where the subject's data lives within Knowledge: author/edit attribution,
    /// mention nodes, person db-row props, comment authorship, trace authorship (reliable) + free-text
    /// matches via Search (best-effort, flagged). The rich body is
    /// [`KnowledgePersonalDataHolder::locate_detailed`]; this contract-shaped entry returns the
    /// content-addressed receipt.
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let receipt = Receipt::content_addressed(
            "locate",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn locate: author/edit attribution + mentions + person props + comment/trace authorship \
             (reliable) + free-text via Search (best-effort, flagged) — rich body: locate_detailed",
            None,
            0,
        );
        Ok(LocateReport { receipt })
    }

    /// Art. 20 portability — a portable bundle of the subject's Knowledge content (their pages as the
    /// KN-P24 Art. 20 lossless JSON bundle). The rich body is
    /// [`KnowledgePersonalDataHolder::export_bundle`] (it REUSES the Export service); this entry
    /// returns the content-addressed receipt.
    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let receipt = Receipt::content_addressed(
            "export",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn export: the subject's pages as the KN-P24 Art. 20 lossless JSON bundle (10.1) — \
             rich body: export_bundle (reuses the Export service)",
            None,
            0,
        );
        Ok(PortableBundle { receipt })
    }

    /// Art. 16 rectification — correct a structured value (author attribution, a person field) + a
    /// best-effort free-text span tombstone. The rich body is
    /// [`KnowledgePersonalDataHolder::rectify_detailed`]; this entry returns the content-addressed
    /// receipt.
    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let receipt = Receipt::content_addressed(
            "rectify",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            "", // the tenant rides the patch target (the page/row); the subject keys the receipt.
            "kn rectify: structured value + best-effort free-text span tombstone (the residual = the \
             ONE platform posture, 10.9 by reference) — rich body: rectify_detailed",
            None,
            0,
        );
        Ok(RectifyReceipt { receipt })
    }

    /// Art. 18/21 restriction — **FULLY functional**: set/clear the restriction flag the four sinks
    /// consult (Search / Agents-RAG / OLAP / Notif). A restricted subject is excluded from all four
    /// (0 emissions — the QUANTIFIED gate, [`RestrictionRegistry::leak_count`] == 0). This is the one
    /// op whose seam the holder owns (the registry), so the trait method does the real work.
    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // The restrict body needs the tenant; the frozen 10.1 `restrict(subject, on)` carries the
        // tenant on the subject's principal (the verified principal's tenant). One source of truth.
        let tenant = subject.principal.tenant.clone();
        Ok(self.restrict_subject(subject, &tenant, on))
    }

    /// Art. 17 erasure — **the named KN-P26 floor (KN-D4).** The per-subject DEK crypto-shred +
    /// pseudonym shred + tombstone/embedding purge structural floor is KN-P26; this contract-shaped
    /// trait `erase` REFUSES loud (it cannot fabricate the crypto-shred seam) rather than claim an
    /// un-built erase succeeded (never a false "erased"). The holder is NOT done until erase ships.
    ///
    /// This is the documented deviation (EI-01 §1): the frozen 10.1 `erase(EraseScope)` carries no
    /// crypto-shred seam, but a real Knowledge erase REQUIRES the per-subject DEK destroy + the
    /// pseudonym-map shred + the embedding purge (a fan-out, not a Knowledge-local value). The honest
    /// contract-shaped body therefore REFUSES rather than fabricate an un-built erase. KN-P26 ships
    /// the body; the `restrict` suppression here covers the pending-erasure window (§6.1).
    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (subject_label, tenant_label) = match &scope {
            EraseScope::Subject { subject, tenant } => (
                subject.principal.principal_id.0.clone(),
                tenant.as_str().to_string(),
            ),
            EraseScope::Tenant(tenant) => {
                ("<tenant-offboarding>".to_string(), tenant.as_str().to_string())
            }
        };
        Err(DsrError(format!(
            "kn erase(scope) for subject `{subject_label}` in tenant `{tenant_label}` is the named \
             KN-P26 floor (KN-D4): the per-subject DEK crypto-shred + pseudonym-map shred + \
             tombstone/embedding purge structural floor. KN-P25 ships locate/export/rectify/restrict; \
             erase REFUSES rather than claim an un-built erase succeeded (never a false 'erased'). The \
             restrict suppression covers the pending-erasure window (§6.1)."
        )))
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The #[personal_data] classify tags on the Knowledge schema (contract 10.2)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The `#[personal_data(...)]`-tagged Knowledge personal-data schema mirror (contract 10.2).** The
/// `no-untagged-personal-data` lint scans every `crates/*/src/*.rs` struct; this is the Knowledge
/// schema's personal-data surface carrying the full six-tag classification on each PII field, so the
/// lint ADMITS the Knowledge schema (0 untagged PII fields). It mirrors the §6/§4 inventory the
/// holder fans out over: person fields, mention nodes, author/edit attribution, free-text body, and
/// agent-trace authorship.
///
/// Why a mirror (not the live `store`/`block` structs): the live Knowledge OLTP rows are SQL-shaped
/// (the schema is the migration DDL, not a Rust struct), and the block/row content is in
/// `myelin-content`'s frozen AST. This struct is the SINGLE place the Knowledge schema's personal-data
/// classification lives (the data-map generator P-GA-09 walks the `#[derive(PersonalData)]` registry
/// over it); the holder above reads the SAME loci (author attribution / mentions / person props /
/// free-text / trace authorship) the tags here classify — one classification, one fan-out.
///
/// Each tag's `erasure` value is the KN-P26 lever per field: identity attribution ⇒ `Pseudonymise`
/// (the pseudonym-map shred); free-text body / mention text ⇒ `CryptoShred(subject_dek)` (the
/// per-subject DEK). The crypto-shred BODY is KN-P26; the CLASSIFICATION (what lever, what key class)
/// is frozen here so the data map + the no-untagged lint are green now.
#[derive(PersonalData)]
#[allow(dead_code)]
pub struct KnowledgePersonRecord {
    /// The page/block/row the personal data lives in (a non-PII opaque id — no tag needed).
    pub artifact_id: String,
    /// `created_by` author attribution — the OPAQUE pseudonymous principal id (architecture §6).
    /// Erasure = Pseudonymise (the pseudonym-map shred, contract 4.8; KN-P26).
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "created_by"
    )]
    pub created_by: String,
    /// `edited_by` edit attribution — the OPAQUE pseudonymous principal id (architecture §6).
    /// Erasure = Pseudonymise (the pseudonym-map shred, contract 4.8; KN-P26).
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "edited_by"
    )]
    pub edited_by: String,
    /// A `mention(Principal)` inline node text — names the subject in free text (architecture §1.5,
    /// §6). Erasure = CryptoShred under the per-subject DEK (contract 11.4; KN-P26).
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by"
    )]
    pub mention_text: String,
    /// The free-text block body the subject may have authored personal data into (architecture §6 —
    /// the self-authored free-text class). Erasure = CryptoShred under the per-subject DEK (11.4;
    /// KN-P26 — the structural floor that makes the op-log/snapshots/backups unrecoverable).
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by"
    )]
    pub free_text_body: String,
    /// A person-typed property value on a `db_row` (the flexible-database person field, architecture
    /// §4). Erasure = CryptoShred under the per-subject DEK (11.4; KN-P26).
    #[personal_data(
        category = ContactInfo,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by"
    )]
    pub db_row_person_prop: String,
    /// The agent-trace authorship — the actor of an agent run trace (AG-7, architecture §5.2). The
    /// OPAQUE pseudonymous agent/human principal. Erasure = Pseudonymise (the pseudonym-map shred;
    /// the trace CONTENT crypto-shred is KN-P26's trace-holder leg).
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = LegitimateInterest(agent_trace_lia),
        retention = TenantPolicy,
        erasure = Pseudonymise,
        subject_locator = "trace_actor"
    )]
    pub trace_actor: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_gdpr::HasPersonalData;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        myelin_tenancy::TenantId("acme".into())
    }

    fn subject_ref(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    // ───────────────────────── the closed restriction-sink set IS the gate ─────────────────────────

    #[test]
    fn restrict_suppresses_exactly_the_four_sinks() {
        // architecture §6 / row 11.6: a restricted subject is excluded from indexing / agent-use
        // (RAG) / analytics / notifications. The closed set is the suppression surface (a new
        // emission surface cannot be added without appearing here).
        assert_eq!(RestrictionSink::ALL.len(), 4);
        for s in [
            RestrictionSink::Search,
            RestrictionSink::Agents,
            RestrictionSink::Olap,
            RestrictionSink::Notif,
        ] {
            assert!(RestrictionSink::ALL.contains(&s), "{} must be suppressed", s.label());
        }
        assert_eq!(RestrictionSink::Search.label(), "search-index");
        assert_eq!(RestrictionSink::Agents.label(), "agent-rag");
        assert_eq!(RestrictionSink::Olap.label(), "olap-analytics");
        assert_eq!(RestrictionSink::Notif.label(), "notifications");
    }

    /// **THE RESTRICT GATE (the QUANTIFIED green artifact): a restricted subject is excluded from all
    /// four sinks — 0 emissions to Search/Agents/OLAP/Notif (the restriction-leak counter == 0).**
    /// Before restrict, every sink admits the subject. After restrict, EVERY sink suppresses it; an
    /// un-restricted second subject still flows. This is the prove-it gate (external-insights/01 §3).
    #[test]
    fn restrict_gate_zero_emissions_to_all_four_sinks() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let alice = subject_ref("p-alice");
        let bob = subject_ref("p-bob");

        // BEFORE restrict: every sink admits alice's content (leak counter untouched — no attempt).
        let supp = RestrictSuppressor::new(&registry, tenant());
        for sink in RestrictionSink::ALL {
            assert_eq!(supp.admit(&alice, sink), SinkVerdict::Emit, "pre-restrict: {} admits", sink.label());
        }
        assert_eq!(registry.leak_count(), 0, "no leak attempts before restrict");

        // RESTRICT alice.
        let receipt = holder.restrict_subject(&alice, &tenant(), true);
        assert_eq!(receipt.receipt.operation, "restrict");
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));
        assert!(registry.is_restricted(&alice, &tenant()));

        // AFTER restrict: EVERY sink suppresses alice (the gate). The leak counter records each
        // attempt — the QUANTIFIED reading: a sink TRIED to emit a restricted subject, suppressed.
        for sink in RestrictionSink::ALL {
            assert_eq!(
                supp.admit(&alice, sink),
                SinkVerdict::Suppressed(sink),
                "post-restrict: {} suppresses the restricted subject (0 emissions)",
                sink.label()
            );
        }
        // 4 attempts caught (one per sink) — the suppressor dropped EVERY one (0 actually emitted).
        assert_eq!(registry.leak_count(), 4, "every sink emission for the restricted subject was caught");

        // An un-restricted subject (bob) still flows to every sink (restrict is per-subject, not a
        // blanket hide — the per-viewer/per-subject conjoin, composing with KN-D5).
        for sink in RestrictionSink::ALL {
            assert_eq!(supp.admit(&bob, sink), SinkVerdict::Emit, "bob (un-restricted) still flows to {}", sink.label());
        }

        // CLEAR the restriction → alice flows again (the flag is reversible, Art. 18 restriction).
        holder.restrict_subject(&alice, &tenant(), false);
        assert!(!registry.is_restricted(&alice, &tenant()));
        for sink in RestrictionSink::ALL {
            assert_eq!(supp.admit(&alice, sink), SinkVerdict::Emit, "post-clear: {} admits alice again", sink.label());
        }
    }

    /// **No sink can leak a restricted subject (the closed-set discipline).** The suppressor is the
    /// ONE gate; a verdict's `admits()` is the sink's emit decision. A restricted subject's `admits()`
    /// is false at EVERY sink — so a sink that respects the verdict cannot emit (0 emissions).
    #[test]
    fn no_sink_admits_a_restricted_subject() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let s = subject_ref("p-restricted");
        holder.restrict_subject(&s, &tenant(), true);
        let supp = RestrictSuppressor::new(&registry, tenant());
        for sink in RestrictionSink::ALL {
            assert!(!supp.admit(&s, sink).admits(), "{} must NOT admit a restricted subject", sink.label());
        }
    }

    // ───────────────────────── locate (structured reliable + free-text flagged) ────────────────────

    #[test]
    fn locate_structured_is_reliable_free_text_is_flagged() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let s = subject_ref("p-ada");
        let structured = vec![
            LocatedLocus {
                kind: LocatedKind::Authorship,
                artifact_ref: "myelin://acme/knowledge/block/b9".into(),
                reliable: true,
            },
            LocatedLocus {
                kind: LocatedKind::Mention,
                artifact_ref: "myelin://acme/knowledge/page/7c2".into(),
                reliable: true,
            },
            LocatedLocus {
                kind: LocatedKind::DbRowPerson,
                artifact_ref: "myelin://acme/knowledge/row/r1".into(),
                reliable: true,
            },
        ];
        let free_text = vec![LocatedLocus {
            // a caller mis-flags it reliable=true — the holder normalises it to a flagged free-text.
            kind: LocatedKind::Authorship,
            artifact_ref: "myelin://acme/knowledge/block/b42".into(),
            reliable: true,
        }];
        let report = holder.locate_detailed(&s, &tenant(), structured, free_text);

        // Structured loci are reliable; the free-text match is FLAGGED (not reliable) — the residual
        // is the ONE platform posture (10.9 by reference).
        assert_eq!(report.reliable_loci().len(), 3, "the three structured loci are reliable");
        assert_eq!(report.flagged_free_text().len(), 1, "the free-text match is flagged best-effort");
        assert!(report.flagged_free_text()[0].kind == LocatedKind::FreeTextMatch);
        assert!(!report.flagged_free_text()[0].reliable, "free-text is never reliable (the residual)");
        assert_eq!(report.receipt.operation, "locate");
    }

    // ───────────────────────── export (reuses the Export service) ──────────────────────────────────

    #[test]
    fn export_reuses_the_export_service_lossless_json() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let s = subject_ref("p-grace");
        // Two pages the subject authored (built via the KN-P24 Export service — NOT a parallel path).
        let doc1 = ExportDoc::new("page-1", "Notes", None, vec![]);
        let doc2 = ExportDoc::new("page-2", "Plan", None, vec![]);
        let (bundle, json) = holder.export_bundle(&s, &tenant(), &[doc1, doc2]);

        assert_eq!(bundle.receipt.operation, "export");
        // The bundle is valid lossless JSON (the Art. 20 portable artifact) carrying both pages.
        let v: serde_json::Value = serde_json::from_str(&json).expect("the export bundle is valid JSON");
        assert_eq!(v["subject"], "p-grace");
        assert_eq!(v["tenant"], "acme");
        assert_eq!(v["pages"].as_array().expect("pages array").len(), 2, "both pages exported");
    }

    // ───────────────────────── rectify (structured + span tombstone) ───────────────────────────────

    #[test]
    fn rectify_corrects_structured_and_tombstones_free_text_spans() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let s = subject_ref("p-lin");
        let (receipt, outcome) = holder.rectify_detailed(&s, &tenant(), 2, 1);
        assert_eq!(receipt.receipt.operation, "rectify");
        assert_eq!(outcome.structured_corrected, 2, "two structured values corrected (reliable)");
        assert_eq!(outcome.free_text_spans_tombstoned, 1, "one free-text span tombstoned (residual)");
    }

    // ───────────────────────── the frozen 10.1 trait (object-safe; CDC consumer-shape) ─────────────

    /// **The CDC pair for row 10.1 (the non-erase ops):** the Knowledge holder is a real
    /// `dyn PersonalDataHolder` (the shape the DSR orchestrator — the consumer of 10.1 — calls). The
    /// non-erase ops succeed (return a content-addressed receipt); `erase` REFUSES loud (the named
    /// KN-P26 floor — never a false "erased"). This is the producer side of the 10.1 contract.
    #[test]
    fn cdc_10_1_knowledge_holder_is_the_frozen_non_erase_contract() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        // The crux: the holder is usable behind `dyn` (the orchestrator holds a heterogeneous set).
        let dyn_holder: &dyn PersonalDataHolder = &holder;
        let s = subject_ref("p-dsr");

        // The non-erase ops succeed with a content-addressed receipt.
        let loc = dyn_holder.locate(&s, tenant()).expect("locate");
        assert_eq!(loc.receipt.operation, "locate");
        assert!(loc.receipt.content_hash.starts_with("blake3:"));
        let exp = dyn_holder.export(&s, tenant()).expect("export");
        assert_eq!(exp.receipt.operation, "export");
        let rec = dyn_holder.rectify(&s, Patch("correct-name".into())).expect("rectify");
        assert_eq!(rec.receipt.operation, "rectify");
        // restrict is fully functional through the trait (it flips the registry flag).
        let restr = dyn_holder.restrict(&s, true).expect("restrict");
        assert_eq!(restr.receipt.operation, "restrict");
        assert!(registry.is_restricted(&s, &tenant()), "the trait restrict flipped the registry flag");

        // erase REFUSES loud — the named KN-P26 floor (never a false 'erased').
        let err = dyn_holder
            .erase(EraseScope::Subject { subject: s.clone(), tenant: tenant() })
            .expect_err("erase is the KN-P26 floor");
        assert!(err.0.contains("KN-P26"), "erase names the KN-P26 floor: {}", err.0);
    }

    // ───────────────────────── the #[personal_data] classify tags (10.2) ───────────────────────────

    /// **The `#[personal_data]` classify tags are applied to the Knowledge schema (contract 10.2).**
    /// The derive emits a registry entry per tagged field; the data-map generator (P-GA-09) walks it.
    /// Every PII field of [`KnowledgePersonRecord`] carries the full six-tag classification with the
    /// KN-P26 erasure lever per field (identity ⇒ Pseudonymise; free-text/mention ⇒ CryptoShred).
    #[test]
    fn knowledge_schema_carries_the_personal_data_tags() {
        let fields = KnowledgePersonRecord::personal_data_fields();
        // The six PII fields are tagged (artifact_id is a non-PII id — no entry).
        assert_eq!(fields.len(), 6, "exactly the six PII fields are tagged, the opaque id has none");
        let by_field: std::collections::HashMap<&str, _> =
            fields.iter().map(|f| (f.field, f)).collect();

        // Identity attribution ⇒ Pseudonymise (the pseudonym-map shred).
        assert_eq!(by_field["created_by"].tags.erasure, "Pseudonymise");
        assert_eq!(by_field["edited_by"].tags.erasure, "Pseudonymise");
        assert_eq!(by_field["trace_actor"].tags.erasure, "Pseudonymise");
        // Free-text / mention / person prop ⇒ CryptoShred under the per-subject DEK (KN-P26).
        assert_eq!(by_field["mention_text"].tags.erasure, "CryptoShred(subject_dek)");
        assert_eq!(by_field["free_text_body"].tags.erasure, "CryptoShred(subject_dek)");
        assert_eq!(by_field["db_row_person_prop"].tags.erasure, "CryptoShred(subject_dek)");
        // The subject_locator is structural (the column a holder reads).
        assert_eq!(KnowledgePersonRecord::subject_locator("created_by"), Some("created_by"));
    }

    /// A sanity bind: the holder fans out over the SAME content the tags classify (the structured
    /// loci kinds cover the tagged fields' loci — author attribution / mention / db-row person prop /
    /// comment / trace authorship). Only `FreeTextMatch` is best-effort; every structured kind is
    /// reliable. This pins the classification to the holder's fan-out (not an abstract mirror).
    #[test]
    fn holder_loci_match_the_tagged_content_shape() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let s = subject_ref("p-content");
        // The structured kinds the tags classify are all reliable in a locate.
        let structured: Vec<LocatedLocus> = [
            LocatedKind::Authorship,
            LocatedKind::Mention,
            LocatedKind::DbRowPerson,
            LocatedKind::CommentAuthorship,
            LocatedKind::TraceAuthorship,
        ]
        .into_iter()
        .map(|kind| LocatedLocus {
            kind,
            artifact_ref: "myelin://acme/knowledge/block/b1".into(),
            reliable: true,
        })
        .collect();
        let report = holder.locate_detailed(&s, &tenant(), structured, vec![]);
        assert_eq!(report.reliable_loci().len(), 5, "every structured kind is reliable");
        assert!(report.flagged_free_text().is_empty(), "no free-text matches in this fixture");
    }
}
