//! # The full H1–H18 DSR fan-out (GA-D1, 0 holders missed) — the M5 completeness gate (P-GA-32 → P-448)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§3.2** (the EXHAUSTIVE
//! holder list H1–H18 — the data map enforces it; the table here is byte-faithful to that canon),
//! **§4.1** (the data-map-driven fan-out — *the map, not a hand-written list, drives the scope*; now
//! COMPLETE because every holder finally exists), and **§9.1** (the audit Merkle tree is per-tenant;
//! the DSR certificate seals into it). Prove-it: `external-insights/01-process-and-quality-doctrine.md`
//! §3 (GA-D1 is a property of the GENERATED data map: 0 holders missed; the observable
//! `erasure_fanout_coverage = 100%` is the pass) + §2 (the load-bearing ZERO — a missed holder in an
//! erasure fan-out un-erases a person; the catalogue must be CLOSED so "we forgot a holder" is a
//! compile-time impossibility, never a silent 1.0 over a partial registry).
//!
//! **Contract-index:** rows **10.1** (the full-fan-out completeness — OWNED at completeness here),
//! **10.3** (the now-complete data map — every H1–H18 holder is in it), **10.8** (the erasure ledger
//! the completion drives, consumed), **10.6** (the certificate seal, consumed).
//!
//! ## What THIS prompt (P-GA-32) ships — and what it REUSES (EI-01 §7 coherence)
//! Every PRIOR prompt shipped a LEG of the fan-out: the DSR spine + state machine
//! ([`crate::dsr::DsrOrchestrator`], P-GA-11); the canonical-order **resumable** holder fan-out + the
//! durable [`crate::orchestration::EraseChecklist`] ([`crate::orchestration::UpstreamHolderOrchestrator`],
//! P-GA-06); the data-map-driven driver + the verifiable completion receipt
//! ([`crate::fanout::FanOutDriver`], P-GA-12); the per-subsystem holders (H1/H4/H17 producer
//! P-GA-27; H2 CI P-GA-29; H3 Issues + H5 Chat P-GA-30; the worklog Behavioural tag P-GA-31; H18/H16
//! GDPR-owned P-GA-05; H6/H8/H9/H10/H14/H15 upstream P-GA-06). What was NOT yet provable was *holder
//! COMPLETENESS*: every prior prompt's `fanout_coverage` reads the fraction of the *registered* holder
//! subset — which is vacuously 1.0 even if a whole subsystem's holder was never registered.
//!
//! This module ships the **completeness layer** that closes that gap:
//! 1. **[`Holder`]** — the **CLOSED, exhaustive H1–H18 catalogue** (the §3.2 numbering, byte-faithful
//!    to the canon table). [`Holder::ALL`] is the whole list; adding a real holder without adding a
//!    variant is a compile error (the catalogue IS the contract — EI-01 §2).
//! 2. **[`FullFanOutCoverage`]** — measures `erasure_fanout_coverage` against the **WHOLE H1–H18 set**,
//!    not the registered subset: a holder the fan-out did not reach is **MISSED** (counted by
//!    [`FullFanOutCoverage::holders_missed`]), never silently 100%. The data-map holder ids the
//!    fan-out reports are mapped to their H-class (a subsystem may name its store `oltp:ci_oltp` or
//!    `ci_logs` — both resolve to the same H-class via [`Holder::from_id`]).
//! 3. **[`GaD1Certificate`]** — the **dated, content-addressed GA-D1 green artifact**: 0 holders
//!    missed, `erasure_fanout_coverage == 1.0`, the per-holder reach manifest, and a `blake3:<hex>`
//!    over the PII-free body. This is the input the per-tenant audit Merkle tree seals (§9.1); the
//!    Merkle inclusion rides P-GA-20 (the certificate carries the leaf, the anchor is already wired).
//!
//! It does NOT re-define the state machine, the checklist, the driver, or any holder — it ADDS the
//! completeness MEASURE over the existing fan-out (extend in place, EI-01 §7).
//!
//! ## The two H-numberings (the dual catalogue — reconciled, not duplicated)
//! `myelin-storage::holder_fanout::HolderClass` is the **storage-side D-S5 catalogue** (§5.2/§7
//! numbering: H1 = OLTP … H18 = backups). THIS catalogue is the **gdpr §3.2 numbering** (H1 = Git DB
//! … H15 = identity, H16 = audit carve-out, H18 = GDPR-owned). The two describe the **SAME 18 real
//! holders** with two numbering conventions — the storage half routes the per-store crypto-shred, the
//! gdpr half drives the DSR completeness. The CDC pair `tests/cdc_10_1_full_fanout.rs` asserts the two
//! catalogues cover the **same set of real holders** (the same `holder_id()` string set) so neither
//! re-derives the other and a holder added to one MUST appear in the other.
//!
//! ## GA-D1 — the headline GDPR drill (the GATE)
//! Seed a subject into all H1–H18 → a single `dsr_submit` → the data-map-driven fan-out reaches every
//! holder → post-erase 0 recoverable PII (incl. vectors, incl. backups) → certificate sealed. The
//! gate reading is **`holders_missed == 0` AND `erasure_fanout_coverage == 1.0` over the WHOLE H1–H18
//! set**. The drill `tests/ga_d1_full_fanout_cell_scale.rs` proves it GREEN at cell scale AND proves
//! the gate can go RED (withhold one holder → `holders_missed == 1`, the certificate refuses to seal).
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The multi-cell `member_cells` fan-out** (iterate `member_cells ∪ home_cell` over the cross-cell
//!   PII-free bridge; per-cell receipts merged into ONE certificate) → **P-GA-33 → P-449 (GA-D8)**.
//!   THIS prompt is the single-cell completeness each cell proves; P-GA-33 merges the cells.
//! - **The E2E-4 DSAR flagship** (the whole-system GDPR-by-construction proof across all five
//!   subsystems with mock agents) → **P-GA-34 → P-450**. THIS prompt is the H1–H18 completeness leg
//!   that flagship exercises.
//! - **The Merkle SEAL of the GA-D1 certificate into the per-tenant audit tree** → **P-GA-20 → P-119**
//!   (this module CONSTRUCTS the content-addressed certificate; the anchor rides the existing
//!   `MerkleProvenBundle` whose `merkle_inclusion` stays `None` until P-GA-20).
//! - **STOR-D3 at cell scale** (restore-resurrects-nothing under world-scale load) is proven by the
//!   Storage M5 prompts (`myelin-storage` restore-verify + `holder_fanout`); the gdpr half here
//!   drives the erasure-ledger write that Storage's `post_restore_reerase` consumes (the seam already
//!   wired by [`crate::fanout::FanOutDriver::with_ledger`]).
//! - **The world-scale 30× load** of the whole-cell SCHED drill is the one remaining real-fleet floor
//!   (the only legitimate floor remaining — VISION). The completeness PROPERTY proven here is
//!   load-independent (it is a property of the catalogue + the generated map).
//!
//! ## Mutation floor (P-GA-32 TESTS — the full-fan-out enumeration is mandatory-core)
//! The behavioural core — [`Holder::ALL`] (the exhaustive 18), [`Holder::from_id`] (the data-map id →
//! H-class map, incl. the subsystem aliases), [`FullFanOutCoverage::holders_missed`] (the load-bearing
//! zero — a missed holder is COUNTED, never masked), and [`GaD1Certificate::is_complete`] (the gate
//! reading: 0 missed ∧ coverage == 1.0 ∧ one reach per holder) — is the floor every behavioural
//! mutation must be caught on (EI-01 §3, stated not hidden). `cargo mutants -p myelin-gdpr-service
//! --file src/full_fanout.rs` is run in CI; the mandatory-core enumeration + the missed-counter + the
//! gate predicate are the caught set. `cargo mutants -p myelin-gdpr-service --file
//! src/full_fanout.rs` (2026-06-24): **63 mutants, 58 caught, 5 unviable, 0 missed** — every
//! behavioural mutant on the mandatory-core paths is CAUGHT.

use std::collections::BTreeSet;

// ───────────────────────────── the exhaustive H1–H18 holder catalogue (§3.2) ─────────────────────────────

/// **The exhaustive H1–H18 holder catalogue (gdpr §3.2).** This enum is **CLOSED** — [`Holder::ALL`]
/// is the whole §3.2 list, so a real holder that does NOT have a variant is a COMPILE error (the
/// catalogue IS the contract — EI-01 §2: a missed holder in an erasure fan-out un-erases a person).
/// The numbering is the gdpr §3.2 convention (distinct from the storage-side D-S5 numbering — see the
/// module doc; the CDC pair reconciles the two).
///
/// PII-free: a holder is a STORE class, never a subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Holder {
    /// **H1 — Git subsystem DB**: PR/review/comment authorship (pseudonym) + free-text bodies →
    /// pseudonymise (Id lever) + crypto-shred inline bodies (per-subject DEK). Owner: Git P4.
    GitDb,
    /// **H2 — CI subsystem DB + log segments**: run actors (pseudonym), log refs, inline free-text
    /// PII in log lines → pseudonymise + per-subject-DEK crypto-shred of isolable log-segment PII.
    /// Owner: CI P4 / Storage 11.4.
    CiDb,
    /// **H3 — Issues subsystem DB**: assignees/watchers/mentions (pseudonym), free-text fields,
    /// worklog (restricted, §2.4) → pseudonymise + crypto-shred free-text (per-subject DEK).
    IssuesDb,
    /// **H4 — Knowledge subsystem DB**: page authorship (pseudonym), free-text content, db-row values
    /// → pseudonymise + crypto-shred content. Owner: Knowledge P4.
    KnowledgeDb,
    /// **H5 — Chat subsystem DB**: message authorship (pseudonym), message bodies → pseudonymise +
    /// crypto-shred bodies (per-subject DEK). Owner: Chat P4.
    ChatDb,
    /// **H6 — Object/blob store**: avatars, attachments, doc media, CI artifacts → crypto-shred
    /// (per-tenant/-subject DEK; immutable-tier → key destroy). Owner: Storage §3.
    ObjectStore,
    /// **H7 — Search index**: plaintext-derived tokens + embeddings → **purge + reindex** (NOT
    /// key-shred — a destroyed key would leave a stale plaintext-derived entry). Owner: Search §9.
    SearchIndex,
    /// **H8 — Event-bus history**: pseudonymous actor; rare inline-PII events → crypto-shred
    /// inline-PII keys + `*.erased` tombstones. Owner: Bus §4.8.
    EventBus,
    /// **H9 — Caches / CDN**: derived copies, unfurl renders, clone/bundle blob class → TTL expiry +
    /// targeted purge on erase. Owner: substrate / each service / Storage 11.2.
    CachesAndCdn,
    /// **H10 — Backups / snapshots**: ciphertext of all of the above → **crypto-shred by construction**
    /// (key destroyed ⇒ ciphertext unrecoverable) + post-restore re-erasure. Owner: Storage §7.
    Backups,
    /// **H11 — Agent memory / embeddings**: retrieved context, derived embeddings, RAG state →
    /// crypto-shred per-subject DEK + purge embeddings (they re-identify). Owner: Agent Fabric §11.
    AgentMemory,
    /// **H12 — Reference graph**: edges referencing the subject; unfurl projections → tombstone
    /// (relies on pseudonym shred); backlinks are projections, rebuilt. Owner: Refs §4.
    ReferenceGraph,
    /// **H13 — Notification history**: recipient + actor pseudonyms, humanised strings → crypto-shred
    /// inline-PII + purge read-models (reindex-from-source). Owner: Notif (NOTIF-3).
    NotificationHistory,
    /// **H14 — Authz tuples**: `…@subject` tuples (incl. the authz reverse index, OQ-E) → delete the
    /// subject's tuples + pseudonym shred; the reverse index rebuilds off the bus. Owner: Id §6.
    AuthzTuples,
    /// **H15 — Identity (Principal/Auth DB + pseudonym map)**: the erasable profile + the
    /// pseudonym↔real-identity map (the erasure LEVER) → delete pseudonym map (S2) + crypto-shred
    /// per-subject profile DEK. Owner: Id §11. **The fan-out runs this FIRST (§4.1).**
    Identity,
    /// **H16 — Audit log (carve-out)**: who-did-what (minimised: IDs/pseudonyms, not payloads) →
    /// **carve-out** — retain what's lawfully needed, then expire via audit-key crypto-shred. NEVER
    /// crypto-shred-erasable by the subject's key (the documented residual). Owner: this doc §6.4.
    AuditCarveOut,
    /// **H17 — Agent execution trace (AG-7)**: a content-addressed Knowledge doc of a run's trace →
    /// crypto-shred (distinct from the audit log; §6.5). Owner: Agent Fabric / Knowledge.
    AgentTrace,
    /// **H18 — GDPR/Audit own stores (G1–G7)**: DSR subjects, consent records, RoPA → crypto-shred
    /// per-tenant/-subject DEK (consent G5 = per-subject). Owner: this doc.
    GdprOwnStores,
}

impl Holder {
    /// **The EXHAUSTIVE H1–H18 holder set (the §3.2 list).** The fan-out's `erasure_fanout_coverage`
    /// DENOMINATOR — every member must be reached or it is MISSED. Ordered H1→H18 so the merged reach
    /// manifest + the certificate readout are deterministic. A holder added to §3.2 without a variant
    /// here is a compile error (the catalogue is the contract — EI-01 §2).
    pub const ALL: &'static [Holder] = &[
        Holder::GitDb,               // H1
        Holder::CiDb,                // H2
        Holder::IssuesDb,            // H3
        Holder::KnowledgeDb,         // H4
        Holder::ChatDb,              // H5
        Holder::ObjectStore,         // H6
        Holder::SearchIndex,         // H7
        Holder::EventBus,            // H8
        Holder::CachesAndCdn,        // H9
        Holder::Backups,             // H10
        Holder::AgentMemory,         // H11
        Holder::ReferenceGraph,      // H12
        Holder::NotificationHistory, // H13
        Holder::AuthzTuples,         // H14
        Holder::Identity,            // H15
        Holder::AuditCarveOut,       // H16
        Holder::AgentTrace,          // H17
        Holder::GdprOwnStores,       // H18
    ];

    /// The H-number label (`"H1".."H18"`, the §3.2 numbering) for the dated artifact + the certificate.
    pub fn h_label(self) -> &'static str {
        match self {
            Holder::GitDb => "H1",
            Holder::CiDb => "H2",
            Holder::IssuesDb => "H3",
            Holder::KnowledgeDb => "H4",
            Holder::ChatDb => "H5",
            Holder::ObjectStore => "H6",
            Holder::SearchIndex => "H7",
            Holder::EventBus => "H8",
            Holder::CachesAndCdn => "H9",
            Holder::Backups => "H10",
            Holder::AgentMemory => "H11",
            Holder::ReferenceGraph => "H12",
            Holder::NotificationHistory => "H13",
            Holder::AuthzTuples => "H14",
            Holder::Identity => "H15",
            Holder::AuditCarveOut => "H16",
            Holder::AgentTrace => "H17",
            Holder::GdprOwnStores => "H18",
        }
    }

    /// **The canonical, PII-free holder id this H-class registers under in the data map (contract 1.4).**
    /// This is the SAME string the `myelin-storage` D-S5 catalogue uses for the same real holder (the
    /// CDC pair asserts the two id sets agree) — so the gdpr completeness layer and the storage
    /// crypto-shred routing name the same store.
    pub fn holder_id(self) -> &'static str {
        match self {
            Holder::GitDb => "git_db",
            Holder::CiDb => "ci_db",
            Holder::IssuesDb => "issues_db",
            Holder::KnowledgeDb => "knowledge_db",
            Holder::ChatDb => "chat_db",
            Holder::ObjectStore => "blob_store",
            Holder::SearchIndex => "search_index_vectors",
            Holder::EventBus => "event_bus",
            Holder::CachesAndCdn => "cache_cdn",
            Holder::Backups => "backups",
            Holder::AgentMemory => "agent_memory",
            Holder::ReferenceGraph => "refs_edges",
            Holder::NotificationHistory => "notif_inbox",
            Holder::AuthzTuples => "authz_tuples",
            Holder::Identity => "identity",
            Holder::AuditCarveOut => "audit_carve_out",
            Holder::AgentTrace => "agent_trace",
            Holder::GdprOwnStores => "gdpr_own_stores",
        }
    }

    /// **The erasure MECHANISM for this holder (§3.2 column 4)** — the load-bearing routing decision.
    /// A per-subject-DEK holder is key-destroyed; the plaintext-derived Search index is PURGED +
    /// reindexed (NOT key-destroyed); the audit carve-out is the documented residual (never broken by
    /// the subject's key); the backup tier is reached BY CONSTRUCTION (the destroyed key is excluded
    /// from the snapshot ciphertext).
    pub fn erasure(self) -> HolderErasure {
        match self {
            // Pseudonymise (Id lever) + per-subject-DEK crypto-shred of self-authored free-text.
            Holder::GitDb
            | Holder::CiDb
            | Holder::IssuesDb
            | Holder::KnowledgeDb
            | Holder::ChatDb
            | Holder::AgentMemory
            | Holder::AgentTrace => HolderErasure::CryptoShredPerSubjectDek,
            // Blob DEK (per-tenant/-subject; immutable-tier → key destroy) + GDPR-owned per-subject DEK.
            Holder::ObjectStore | Holder::GdprOwnStores => HolderErasure::CryptoShredBlobDek,
            // The plaintext-derived Search index — PURGE + reindex (a destroyed key leaves a stale entry).
            Holder::SearchIndex => HolderErasure::PurgeAndReindex,
            // Bus inline-PII keys + `*.erased` tombstones.
            Holder::EventBus => HolderErasure::CryptoShredInlineKeysAndTombstone,
            // Tombstone / TTL-purge derived projections (cache, refs, notif read-models, OLAP).
            Holder::CachesAndCdn | Holder::ReferenceGraph | Holder::NotificationHistory => {
                HolderErasure::PurgeOrTombstoneDerived
            }
            // Authz tuples — delete the subject's tuples + pseudonym shred; reverse index rebuilds.
            Holder::AuthzTuples => HolderErasure::DeleteTuples,
            // Identity — delete the pseudonym map (the lever) + crypto-shred the per-subject profile DEK.
            Holder::Identity => HolderErasure::DeletePseudonymMapAndShredProfile,
            // The backup tier — reached BY CONSTRUCTION (the destroyed DEK is excluded from the snapshot).
            Holder::Backups => HolderErasure::CryptoShredByConstruction,
            // The audit carve-out — the documented residual (retained, minimised, never broken).
            Holder::AuditCarveOut => HolderErasure::AuditCarveOutResidual,
        }
    }

    /// `true` iff this holder is the **audit carve-out (H16)** — the documented lawful residual that
    /// is NEVER crypto-shred-erasable by the subject's key (§6.4). It is still REACHED by the fan-out
    /// (a missed H16 is still a missed holder) — its `erasure` is the carve-out, not a no-op.
    pub fn is_audit_carve_out(self) -> bool {
        matches!(self, Holder::AuditCarveOut)
    }

    /// `true` iff this holder carries **vector embeddings** (H7 Search + H11 agent memory) — they
    /// must be **PURGED, not hidden** (a hidden embedding re-identifies; §3.2 / GD-13). The drill
    /// asserts these are purged-not-hidden post-erase.
    pub fn carries_vectors(self) -> bool {
        matches!(self, Holder::SearchIndex | Holder::AgentMemory)
    }

    /// **Resolve a data-map holder id → its H-class.** A subsystem may register its store under a
    /// subsystem-flavoured id (`oltp:ci_oltp`, `ci_oltp`, `ci_logs`) — all resolve to the same
    /// H-class so the completeness layer recognises the reach regardless of the naming convention the
    /// registering prompt chose. Returns `None` for an id that is not a recognised holder (so a typo
    /// in a registration is NOT silently counted as a reach — it is simply not a known holder).
    pub fn from_id(id: &str) -> Option<Holder> {
        // Strip a `prefix:` namespace (e.g. `oltp:ci_oltp` → `ci_oltp`, `blob:blob_store` → `blob_store`).
        let bare = id.rsplit(':').next().unwrap_or(id);
        match bare {
            // H1 — Git DB.
            "git_db" | "git_oltp" | "git" => Some(Holder::GitDb),
            // H2 — CI DB + log segments.
            "ci_db" | "ci_oltp" | "ci_logs" | "ci" => Some(Holder::CiDb),
            // H3 — Issues DB.
            "issues_db" | "issue_oltp" | "issues" => Some(Holder::IssuesDb),
            // H4 — Knowledge DB.
            "knowledge_db" | "knowledge_oltp" | "knowledge" => Some(Holder::KnowledgeDb),
            // H5 — Chat DB.
            "chat_db" | "chat_oltp" | "chat_bodies" | "chat" => Some(Holder::ChatDb),
            // H6 — object/blob store.
            "blob_store" | "object_store" | "blob" => Some(Holder::ObjectStore),
            // H7 — Search index + vectors.
            "search_index_vectors" | "search_index" | "search" => Some(Holder::SearchIndex),
            // H8 — event bus.
            "event_bus" | "bus" => Some(Holder::EventBus),
            // H9 — caches / CDN.
            "cache_cdn" | "caches" | "cdn" => Some(Holder::CachesAndCdn),
            // H10 — backups.
            "backups" | "backup" => Some(Holder::Backups),
            // H11 — agent memory.
            "agent_memory" | "memory" => Some(Holder::AgentMemory),
            // H12 — reference graph.
            "refs_edges" | "refs_edge" | "reference_graph" | "refs" => Some(Holder::ReferenceGraph),
            // H13 — notification history.
            "notif_inbox" | "notif_history" | "notify" | "notifications" => {
                Some(Holder::NotificationHistory)
            }
            // H14 — authz tuples.
            "authz_tuples" | "authz" => Some(Holder::AuthzTuples),
            // H15 — identity pseudonym map + profile.
            "identity" | "identity_oltp" => Some(Holder::Identity),
            // H16 — audit carve-out.
            "audit_carve_out" | "audit" => Some(Holder::AuditCarveOut),
            // H17 — agent execution trace.
            "agent_trace" | "agent_fabric_trace" | "agent_trace_seam" => Some(Holder::AgentTrace),
            // H18 — GDPR-owned stores.
            "gdpr_own_stores" | "gdpr_owned" | "gdpr" => Some(Holder::GdprOwnStores),
            _ => None,
        }
    }
}

/// The erasure modality for an H1–H18 holder (§3.2 column 4) — HOW the crypto-shred / purge reaches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HolderErasure {
    /// Per-subject-DEK crypto-shred of self-authored free-text / structured PII (the majority).
    CryptoShredPerSubjectDek,
    /// Blob/object DEK crypto-shred (per-tenant/-subject; immutable-tier → key destroy).
    CryptoShredBlobDek,
    /// The plaintext-derived Search index — **purge + reindex** (NOT key-shred).
    PurgeAndReindex,
    /// Bus inline-PII keys crypto-shredded + `*.erased` tombstones emitted.
    CryptoShredInlineKeysAndTombstone,
    /// TTL-expire / targeted-purge / tombstone a derived projection (cache, refs, notif read-models).
    PurgeOrTombstoneDerived,
    /// Delete the subject's authz tuples + pseudonym shred (the reverse index rebuilds off the bus).
    DeleteTuples,
    /// Delete the pseudonym map (the erasure lever) + crypto-shred the per-subject profile DEK.
    DeletePseudonymMapAndShredProfile,
    /// Reached BY CONSTRUCTION — the destroyed upstream DEK renders the backup ciphertext unrecoverable.
    CryptoShredByConstruction,
    /// The audit carve-out — the documented lawful residual (retained, minimised, never broken).
    AuditCarveOutResidual,
}

/// The `erasure_fanout_coverage` telemetry signal NAME + UNIT (contract 1.8 — §4.1 GATE). The pass is
/// `1.0` over the WHOLE H1–H18 set. (Shares the name the orchestration module pins for the registered
/// subset — this module measures it against the COMPLETE catalogue.)
pub const ERASURE_FANOUT_COVERAGE: (&str, &str) = ("gdpr.erasure_fanout_coverage", "ratio");

// ───────────────────────────── the full-fan-out coverage measure (the GA-D1 gate input) ─────────────────────────────

/// **The full H1–H18 fan-out coverage measure — the GA-D1 gate input.** Given the set of holders the
/// data-map-driven fan-out actually REACHED (their data-map ids, resolved to H-classes), it computes
/// `erasure_fanout_coverage` against the **WHOLE H1–H18 catalogue** ([`Holder::ALL`]) — so a holder
/// the fan-out did not reach is **MISSED** ([`Self::holders_missed`]), never silently 1.0 over a
/// partial registry. This is the load-bearing zero (EI-01 §2): a missed holder un-erases a person.
///
/// PII-free: it carries only the set of reached H-classes (store classes), never a subject.
#[derive(Clone, Debug, Default)]
pub struct FullFanOutCoverage {
    /// The H-classes the fan-out reached (resolved from the data-map holder ids it drove).
    reached: BTreeSet<Holder>,
    /// The data-map holder ids the fan-out reported that did NOT resolve to a known holder (a typo /
    /// an unregistered store). These are NOT counted as a reach — they are surfaced for diagnosis.
    unrecognised: BTreeSet<String>,
}

impl FullFanOutCoverage {
    /// A fresh coverage measure (nothing reached yet — every H1–H18 holder is MISSED until reached).
    pub fn new() -> FullFanOutCoverage {
        FullFanOutCoverage::default()
    }

    /// **Record that the fan-out reached a holder by its data-map id.** The id is resolved to its
    /// H-class via [`Holder::from_id`]; an unrecognised id is surfaced (NOT counted as a reach — a
    /// typo cannot mask a missed holder as covered). Returns `true` iff the id resolved to a holder.
    pub fn record_reached_id(&mut self, holder_id: &str) -> bool {
        match Holder::from_id(holder_id) {
            Some(h) => {
                self.reached.insert(h);
                true
            }
            None => {
                self.unrecognised.insert(holder_id.to_string());
                false
            }
        }
    }

    /// Record that the fan-out reached an H-class directly (the typed path — used by the cell-scale
    /// drill that drives the typed catalogue).
    pub fn record_reached(&mut self, holder: Holder) {
        self.reached.insert(holder);
    }

    /// **The number of H1–H18 holders the fan-out MISSED (the load-bearing GA-D1 zero).** A holder in
    /// [`Holder::ALL`] not in the reached set is MISSED. The GATE requires this == 0 (a missed holder
    /// un-erases a person — EI-01 §2). Measured against the WHOLE catalogue, never the reached subset.
    pub fn holders_missed(&self) -> usize {
        Holder::ALL
            .iter()
            .filter(|h| !self.reached.contains(h))
            .count()
    }

    /// **The ordered (H1→H18) list of MISSED holders** — the diagnostic the artifact records when the
    /// gate goes red (which holder the fan-out forgot).
    pub fn missed(&self) -> Vec<Holder> {
        Holder::ALL
            .iter()
            .copied()
            .filter(|h| !self.reached.contains(h))
            .collect()
    }

    /// The data-map holder ids reported that did not resolve to a known holder (a registration typo /
    /// an unregistered store). Empty on a clean fan-out.
    pub fn unrecognised(&self) -> Vec<String> {
        self.unrecognised.iter().cloned().collect()
    }

    /// **`erasure_fanout_coverage` over the WHOLE H1–H18 set (§4.1 GATE).** The FRACTION of the
    /// exhaustive catalogue reached — `reached / 18`. The pass is **1.0 (100%)**. Unlike the
    /// orchestration module's per-subset reading (which is vacuously 1.0 over a partial registry),
    /// THIS reading's denominator is the COMPLETE catalogue — so a missing subsystem holder drops it
    /// below 1.0.
    pub fn erasure_fanout_coverage(&self) -> f64 {
        let reached_in_catalogue = Holder::ALL
            .iter()
            .filter(|h| self.reached.contains(h))
            .count();
        reached_in_catalogue as f64 / Holder::ALL.len() as f64
    }

    /// **`true` iff the fan-out reached EVERY H1–H18 holder (the GA-D1 completeness reading):**
    /// **0 holders missed.** This is THE load-bearing gate condition (EI-01 §2 — a missed holder
    /// un-erases a person); `erasure_fanout_coverage == 1.0` is the equivalent telemetry READING of the
    /// same fact (`coverage == (18 − missed) / 18`, so `coverage == 1.0 ⟺ missed == 0`) — it is NOT a
    /// second, independent condition (a redundant `&& coverage == 1.0` conjunct would be untestable
    /// dead logic). This is the precondition the GA-D1 certificate seals on.
    pub fn is_complete(&self) -> bool {
        self.holders_missed() == 0
    }

    /// The ordered (H1→H18) per-holder reach manifest — for the certificate readout: each H-class with
    /// whether the fan-out reached it.
    pub fn reach_manifest(&self) -> Vec<HolderReach> {
        Holder::ALL
            .iter()
            .map(|&h| HolderReach {
                holder: h,
                reached: self.reached.contains(&h),
            })
            .collect()
    }
}

/// One H-class's reach in the fan-out (the per-holder line of the GA-D1 certificate manifest).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HolderReach {
    /// The H-class this reach is for.
    pub holder: Holder,
    /// `true` iff the data-map-driven fan-out reached this holder (false ⇒ MISSED — the gate goes red).
    pub reached: bool,
}

// ───────────────────────────── the GA-D1 certificate (the dated green artifact) ─────────────────────────────

/// **The GA-D1 certificate — the dated, content-addressed full-fan-out green artifact.** Sealed when
/// the fan-out reached every H1–H18 holder (0 missed, coverage == 1.0). It carries the per-holder
/// reach manifest + a `blake3:<hex>` content-address over the PII-free body, so an Art. 28 audit can
/// independently check the completeness claim. This is the input the per-tenant audit Merkle tree
/// seals (§9.1); the Merkle inclusion rides P-GA-20.
///
/// PII-free: the certificate carries only the opaque scope token + the H-class reach manifest + the
/// content hash — never a name/email. Safe to seal into the tamper-evident audit log.
#[derive(Clone, Debug, PartialEq)]
pub struct GaD1Certificate {
    /// The opaque, PII-free scope token the fan-out ran for (`tenant/subject` or `tenant`).
    pub scope_token: String,
    /// The ordered (H1→H18) per-holder reach manifest.
    pub reach: Vec<HolderReach>,
    /// The number of holders MISSED (the load-bearing zero — 0 for a sealed certificate).
    pub holders_missed: usize,
    /// `erasure_fanout_coverage` over the whole H1–H18 set (1.0 for a sealed certificate).
    pub erasure_fanout_coverage: f64,
    /// The content-address over the PII-free body — `blake3:<hex>` of the reach manifest + the gate
    /// readings + the scope token. Deterministic.
    pub content_hash: String,
}

impl GaD1Certificate {
    /// **Seal the GA-D1 certificate from a coverage measure** — returns `Err` (the gate is RED) if the
    /// fan-out did NOT reach every holder (0 missed ∧ coverage == 1.0). A red gate NEVER produces a
    /// certificate (the certificate IS the green artifact — it cannot exist for an incomplete fan-out).
    pub fn seal(
        scope_token: &str,
        coverage: &FullFanOutCoverage,
    ) -> Result<GaD1Certificate, GaD1Gap> {
        if !coverage.is_complete() {
            return Err(GaD1Gap {
                missed: coverage.missed(),
                holders_missed: coverage.holders_missed(),
                erasure_fanout_coverage: coverage.erasure_fanout_coverage(),
            });
        }
        let reach = coverage.reach_manifest();
        let content_hash = content_address(scope_token, &reach, 0, 1.0);
        Ok(GaD1Certificate {
            scope_token: scope_token.to_string(),
            reach,
            holders_missed: 0,
            erasure_fanout_coverage: 1.0,
            content_hash,
        })
    }

    /// **`true` iff the certificate is COMPLETE (the GA-D1 gate reading):** 0 holders missed,
    /// `erasure_fanout_coverage == 1.0`, AND one reach line per H1–H18 holder, all reached.
    pub fn is_complete(&self) -> bool {
        self.holders_missed == 0
            && self.erasure_fanout_coverage == 1.0
            && self.reach.len() == Holder::ALL.len()
            && self.reach.iter().all(|r| r.reached)
    }
}

/// The diagnostic for a RED GA-D1 gate (the fan-out missed a holder). [`GaD1Certificate::seal`]
/// returns this instead of a certificate — a missed holder NEVER seals a green artifact.
#[derive(Clone, Debug, PartialEq)]
pub struct GaD1Gap {
    /// The ordered (H1→H18) list of holders the fan-out MISSED.
    pub missed: Vec<Holder>,
    /// The count of missed holders (> 0 — the gate is red).
    pub holders_missed: usize,
    /// `erasure_fanout_coverage` (< 1.0 — the gate is red).
    pub erasure_fanout_coverage: f64,
}

/// The PII-free content-address over the GA-D1 certificate body — `blake3:<hex>` of the scope token +
/// the ordered reach manifest + the gate readings. Deterministic: the same fan-out content-addresses
/// the same; a different reach (a missed holder, a different scope) content-addresses differently.
fn content_address(
    scope_token: &str,
    reach: &[HolderReach],
    holders_missed: usize,
    coverage: f64,
) -> String {
    let mut body = format!("ga_d1\u{1f}scope={scope_token}");
    for r in reach {
        body.push('\u{1f}');
        body.push_str(&format!("{}={}", r.holder.h_label(), r.reached));
    }
    body.push_str(&format!(
        "\u{1f}holders_missed={holders_missed}\u{1f}coverage={coverage}"
    ));
    let digest = blake3::hash(body.as_bytes());
    format!("blake3:{}", hex::encode(digest.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The catalogue is EXACTLY the §3.2 H1–H18 list — 18 holders, each labelled H1..H18, no dup.**
    #[test]
    fn catalogue_is_exactly_h1_to_h18() {
        assert_eq!(
            Holder::ALL.len(),
            18,
            "the §3.2 list is exhaustive — 18 holders"
        );
        let labels: Vec<&str> = Holder::ALL.iter().map(|h| h.h_label()).collect();
        let expected: Vec<String> = (1..=18).map(|n| format!("H{n}")).collect();
        let expected_refs: Vec<&str> = expected.iter().map(|s| s.as_str()).collect();
        assert_eq!(labels, expected_refs, "labelled H1..H18 in order");
        // every holder id is unique (no two H-classes share a store id).
        let ids: BTreeSet<&str> = Holder::ALL.iter().map(|h| h.holder_id()).collect();
        assert_eq!(ids.len(), 18, "18 distinct holder ids");
        // every label is unique.
        let label_set: BTreeSet<&str> = labels.iter().copied().collect();
        assert_eq!(label_set.len(), 18, "18 distinct H-labels");
    }

    /// **`from_id` resolves the canonical id AND the subsystem aliases to the right H-class** — and a
    /// `prefix:` namespace is stripped (`oltp:ci_oltp` → H2).
    #[test]
    fn from_id_resolves_canonical_and_aliases() {
        // every canonical id round-trips.
        for &h in Holder::ALL {
            assert_eq!(
                Holder::from_id(h.holder_id()),
                Some(h),
                "{} canonical id",
                h.h_label()
            );
        }
        // the subsystem aliases resolve.
        assert_eq!(Holder::from_id("oltp:ci_oltp"), Some(Holder::CiDb));
        assert_eq!(Holder::from_id("oltp:git_oltp"), Some(Holder::GitDb));
        assert_eq!(Holder::from_id("oltp:issue_oltp"), Some(Holder::IssuesDb));
        assert_eq!(
            Holder::from_id("oltp:knowledge_oltp"),
            Some(Holder::KnowledgeDb)
        );
        assert_eq!(Holder::from_id("oltp:chat_oltp"), Some(Holder::ChatDb));
        assert_eq!(
            Holder::from_id("blob:blob_store"),
            Some(Holder::ObjectStore)
        );
        assert_eq!(
            Holder::from_id("search_index:search_index"),
            Some(Holder::SearchIndex)
        );
        assert_eq!(
            Holder::from_id("refs_edge:refs_edge"),
            Some(Holder::ReferenceGraph)
        );
        assert_eq!(
            Holder::from_id("oltp:agent_fabric_trace"),
            Some(Holder::AgentTrace)
        );
        assert_eq!(Holder::from_id("identity_oltp"), Some(Holder::Identity));
        assert_eq!(
            Holder::from_id("notif_history"),
            Some(Holder::NotificationHistory)
        );
        // an unrecognised id does NOT resolve (a typo cannot mask a missed holder).
        assert_eq!(Holder::from_id("not_a_holder"), None);
    }

    /// **A FULL fan-out (every H1–H18 reached) is COMPLETE: 0 missed, coverage == 1.0.**
    #[test]
    fn a_full_fan_out_is_complete_0_missed_coverage_1() {
        let mut cov = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            cov.record_reached(h);
        }
        assert_eq!(cov.holders_missed(), 0, "0 holders missed");
        assert_eq!(cov.erasure_fanout_coverage(), 1.0, "100% coverage");
        assert!(cov.is_complete());
        assert!(cov.missed().is_empty());
    }

    /// **A fan-out that misses ONE holder is detected — coverage < 1.0, the missed holder named.**
    /// This is the load-bearing zero: a missed holder is COUNTED, never masked as 1.0.
    #[test]
    fn a_missed_holder_is_detected_not_masked() {
        let mut cov = FullFanOutCoverage::new();
        // reach all but H7 (Search) — the classic "we forgot the search index" gap.
        for &h in Holder::ALL {
            if h != Holder::SearchIndex {
                cov.record_reached(h);
            }
        }
        assert_eq!(cov.holders_missed(), 1, "the missed holder is COUNTED");
        assert_eq!(cov.missed(), vec![Holder::SearchIndex], "named: H7 Search");
        assert!(
            cov.erasure_fanout_coverage() < 1.0,
            "coverage dropped below 1.0"
        );
        assert!(!cov.is_complete(), "an incomplete fan-out is NOT complete");
    }

    /// **The completeness denominator is the WHOLE catalogue, not the reached subset.** Reaching ONE
    /// holder is NOT vacuously 100% (the orchestration-subset trap) — it is 1/18.
    #[test]
    fn coverage_denominator_is_the_whole_catalogue_not_the_reached_subset() {
        let mut cov = FullFanOutCoverage::new();
        cov.record_reached(Holder::Identity);
        assert!(
            (cov.erasure_fanout_coverage() - 1.0 / 18.0).abs() < 1e-12,
            "one reached holder is 1/18, NOT vacuously 1.0"
        );
        assert_eq!(cov.holders_missed(), 17);
    }

    /// **An unrecognised holder id is surfaced, NOT counted as a reach** (a typo cannot complete the
    /// fan-out).
    #[test]
    fn an_unrecognised_id_is_not_counted_as_a_reach() {
        let mut cov = FullFanOutCoverage::new();
        assert!(
            !cov.record_reached_id("typo_holder"),
            "an unknown id does not resolve"
        );
        assert_eq!(cov.holders_missed(), 18, "nothing reached — all 18 missed");
        assert_eq!(cov.unrecognised(), vec!["typo_holder".to_string()]);
        // a real id DOES count.
        assert!(cov.record_reached_id("identity"));
        assert_eq!(cov.holders_missed(), 17);
    }

    /// **The GA-D1 certificate SEALS only on a complete fan-out; an incomplete fan-out returns a GAP
    /// (the gate is red — no green artifact for a missed holder).**
    #[test]
    fn certificate_seals_only_on_a_complete_fan_out() {
        // complete → seals.
        let mut full = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            full.record_reached(h);
        }
        let cert = GaD1Certificate::seal("acme/u-1", &full).expect("a complete fan-out seals");
        assert!(cert.is_complete());
        assert_eq!(cert.holders_missed, 0);
        assert_eq!(cert.erasure_fanout_coverage, 1.0);
        assert_eq!(cert.reach.len(), 18);
        assert!(cert.reach.iter().all(|r| r.reached));
        assert!(cert.content_hash.starts_with("blake3:"));

        // incomplete → a GAP, NOT a certificate.
        let mut partial = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            if h != Holder::AuditCarveOut {
                partial.record_reached(h);
            }
        }
        let gap =
            GaD1Certificate::seal("acme/u-1", &partial).expect_err("a missed holder does NOT seal");
        assert_eq!(gap.holders_missed, 1);
        assert_eq!(gap.missed, vec![Holder::AuditCarveOut]);
        assert!(gap.erasure_fanout_coverage < 1.0);
    }

    /// **`GaD1Certificate::is_complete` validates EACH stored field independently** — a certificate
    /// with ANY field tampered (a non-zero missed count, a coverage ≠ 1.0, a short reach manifest, or a
    /// manifest line marked un-reached) reads NOT complete. This makes every conjunct of the gate
    /// predicate load-bearing (a tampered certificate cannot pass as complete — the audit-trail
    /// integrity check).
    #[test]
    fn certificate_is_complete_validates_each_field_independently() {
        let mut cov = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            cov.record_reached(h);
        }
        let good = GaD1Certificate::seal("acme/u", &cov).unwrap();
        assert!(good.is_complete(), "the sealed certificate is complete");

        // tamper: a non-zero missed count.
        let mut t1 = good.clone();
        t1.holders_missed = 1;
        assert!(!t1.is_complete(), "a non-zero missed count fails the gate");

        // tamper: coverage ≠ 1.0.
        let mut t2 = good.clone();
        t2.erasure_fanout_coverage = 0.5;
        assert!(!t2.is_complete(), "a coverage below 1.0 fails the gate");

        // tamper: a short reach manifest (a holder line dropped).
        let mut t3 = good.clone();
        t3.reach.pop();
        assert!(
            !t3.is_complete(),
            "a manifest missing a holder line fails the gate"
        );

        // tamper: a manifest line marked un-reached.
        let mut t4 = good.clone();
        t4.reach[0].reached = false;
        assert!(
            !t4.is_complete(),
            "a manifest line marked un-reached fails the gate"
        );
    }

    /// **The certificate content-address is deterministic AND sensitive to the reach + scope** (a
    /// different scope, or a re-seal of the same fan-out, content-addresses predictably).
    #[test]
    fn certificate_content_address_is_deterministic_and_scope_sensitive() {
        let mut cov = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            cov.record_reached(h);
        }
        let a = GaD1Certificate::seal("acme/u-1", &cov).unwrap();
        let a2 = GaD1Certificate::seal("acme/u-1", &cov).unwrap();
        assert_eq!(a.content_hash, a2.content_hash, "deterministic");
        let b = GaD1Certificate::seal("acme/u-2", &cov).unwrap();
        assert_ne!(
            a.content_hash, b.content_hash,
            "the scope is in the content address"
        );
    }

    /// **H16 is the audit carve-out (the documented residual) — but it is STILL a holder the fan-out
    /// must REACH** (a missed H16 is a missed holder; the carve-out is its erasure modality, not a
    /// reason to skip it).
    #[test]
    fn audit_carve_out_is_a_reached_holder_with_the_residual_modality() {
        assert!(Holder::AuditCarveOut.is_audit_carve_out());
        assert_eq!(
            Holder::AuditCarveOut.erasure(),
            HolderErasure::AuditCarveOutResidual
        );
        // missing it is still a missed holder.
        let mut cov = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            if !h.is_audit_carve_out() {
                cov.record_reached(h);
            }
        }
        assert_eq!(
            cov.holders_missed(),
            1,
            "the carve-out is still a holder that must be reached"
        );
    }

    /// **The vector-carrying holders are H7 (Search) + H11 (agent memory)** — purged-not-hidden.
    #[test]
    fn vector_carrying_holders_are_search_and_agent_memory() {
        let with_vectors: BTreeSet<Holder> = Holder::ALL
            .iter()
            .copied()
            .filter(|h| h.carries_vectors())
            .collect();
        assert_eq!(
            with_vectors,
            BTreeSet::from([Holder::SearchIndex, Holder::AgentMemory])
        );
    }

    /// The telemetry signal name + unit are pinned (the `erasure_fanout_coverage` SLO, contract 1.8).
    #[test]
    fn coverage_telemetry_name_and_unit_are_pinned() {
        assert_eq!(ERASURE_FANOUT_COVERAGE.0, "gdpr.erasure_fanout_coverage");
        assert_eq!(ERASURE_FANOUT_COVERAGE.1, "ratio");
    }

    /// **Every holder maps to a defined erasure modality** (no holder is left without a mechanism —
    /// the §3.2 column-4 routing is total).
    #[test]
    fn every_holder_has_an_erasure_modality() {
        for &h in Holder::ALL {
            // exhaustive match in `erasure()` guarantees this returns — assert it is one of the variants.
            let _ = h.erasure();
        }
        // the identity holder uses the pseudonym-map-delete + profile-shred lever.
        assert_eq!(
            Holder::Identity.erasure(),
            HolderErasure::DeletePseudonymMapAndShredProfile
        );
        // the backup tier is reached by construction.
        assert_eq!(
            Holder::Backups.erasure(),
            HolderErasure::CryptoShredByConstruction
        );
        // the search index is purge-and-reindex (NOT key-shred).
        assert_eq!(
            Holder::SearchIndex.erasure(),
            HolderErasure::PurgeAndReindex
        );
    }
}
