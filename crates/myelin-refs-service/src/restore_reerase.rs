//! # `restore_reerase` — restore + re-erase at backup scale (REF-P25 / P-456, M5; REF-D5 at scale)
//!
//! **The full backup-scale REF-D5.** This module promotes the REF-P15 **CI-variant** erase drill
//! (`holder.rs` / `integration_ref_p15_holder_erase.rs` — one subject, one cached title) to its
//! **backup-scale** form: erase a subject + a referenced artifact across a SCALED Refs index, RESTORE
//! the edge index (OLTP/cache/per-subject DEKs) from a PRE-erase backup to a consistent point, then
//! RE-ERASE from the **erasure ledger** (contract 10.8) — and prove **0 resurrected PII** past the
//! erasure. The references stay tombstoned; the person stays unresolvable; there is no `500` on resolve.
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §4.6 tail (the small structural erasure surface — the per-subject DEK crypto-shred of a cached
//! title + the `*.erased`-driven edge tombstone, NO erasure backdoor), §7 **D-5 the scale variant**
//! (the backup-scale form of the erasure drill). **Contract-index rows 10.1** (the erase holder at
//! backup scale, OWNED) **+ 10.8** (the PII-free, non-shred-erasable erasure ledger that drives
//! post-restore re-erasure, CONSUMED) **+ 11.5** (backup / restore cross-seam — the per-subject DEK
//! crypto-shred is excluded from backup, so a restore cannot resurrect it; the re-erase replay covers
//! anything an OLDER backup brought back). **External insight:** `04-hard-problems.md` §1 (**the key
//! stays destroyed even after a backup is restored** — no resurrected PII past an erasure);
//! `01-process-and-quality-doctrine.md` §3 (prove it under scale — the 0-recoverable property is
//! DRILLED green over a backup-scale corpus, never asserted in prose; never weaken a threshold to
//! pass). **VISION §3** (GDPR-safe, world-scale).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second erase path)
//! This is the **backup-scale DRILL HARNESS over the EXISTING erase surface**, not a second eraser. The
//! per-subject cache-PII purge rides the SAME [`crate::RefsCacheHolder::erase`] §4.6 body (the ONE
//! `invalidate`/crypto-shred path) over the SAME REF-P12 [`crate::R2ProjectionCache`] + REF-P4
//! [`crate::RefsDekPin`] crypto-shred unit. The edge tombstone rides the SAME
//! [`crate::EdgeProjection::tombstone`] the `*.erased` consumer drives (no backdoor). The re-erase
//! ledger is the SAME PII-free, non-shred-erasable shape the Bus's `BusErasureLedger` froze
//! (`myelin-events::reerase`, EI-01 §7 cold == live) — recording an OPAQUE subject discriminator + the
//! per-subject DEK key refs shredded + the edge surface, so a post-restore replay re-runs the IDENTICAL
//! erase. No new mutation-core erase decision logic is introduced; this module SCALES the corpus the
//! frozen surface runs over and adds the restore→re-erase cross-seam the REF-P15 CI variant named as
//! its REF-P25 floor.
//!
//! ## Why the ledger is PII-free and non-shred-erasable (10.8)
//! The ledger is the ONE thing that must OUTLIVE a crypto-shred AND a restore: if erasing a subject also
//! erased the record that the subject was erased, a restore of a pre-erase backup could resurrect the
//! subject with nothing to re-apply. So the ledger carries only the OPAQUE pseudonymous subject id (the
//! `origin_actor` opaque id — never a name) + the opaque per-subject DEK `PiiKeyRef` strings (a key
//! NAME, not key material) + the `(tenant, region)` cell + a timestamp. It is itself NOT a
//! `PersonalDataHolder` target (a DSR does not erase the fact-of-erasure record — that would be
//! self-defeating); this is the §4.4 / 10.8 "non-shred-erasable" property the contract names.
//!
//! ## The restore + re-erase cross-seam (11.5, the §4.6 backup-scale story)
//! 1. **Steady-state.** A backup-scale corpus is built: many subjects, each authoring edges whose
//!    cached projection titles hold a name (the only name-bearing PII), warmed into the cache sealed
//!    under each subject's per-subject DEK backstop (REF-P4 §3.6).
//! 2. **Erase + record.** Each erased subject's cache PII is purged AND its per-subject DEK is
//!    crypto-shredded (the cached title is unrecoverable, live AND in backup — §7.5 excludes a shredded
//!    key from backup) AND its edges are tombstoned. The erase is RECORDED in the ledger (10.8).
//! 3. **Restore a PRE-erase backup.** A restore of an OLDER backup brings the pre-erase state back: the
//!    cached title is re-warmed, the per-subject DEK is resurrected (re-sealed), the edge tombstones are
//!    gone. (This is exactly what restoring a backup taken before the erase does.)
//! 4. **Re-erase from the ledger.** [`re_erase_at_backup_scale`] REPLAYS the ledger: for every
//!    ledger-listed subject it re-runs the IDENTICAL §4.6 erase (re-purge + re-shred the resurrected
//!    DEK + re-tombstone the restored edges) — idempotent (re-shredding a dead key is a no-op success).
//! 5. **Verify.** [`BackupScaleReEraseReport`] proves **0 recoverable PII** post-restore: 0 live
//!    cached titles, 0 resurrected per-subject DEKs, every edge re-tombstoned, the person
//!    unresolvable, no `500` on resolve (a crypto-shredded cache reads as a clean MISS).
//!
//! ## The REF-P15 → REF-P25 floor promotion (linked in the commit)
//! REF-P15 (`holder.rs`) shipped the §4.6 erase surface + named "the world-scale 0-recoverable shred
//! drill (REF-D5 at backup scale) is REF-P25". This module IS that drill: it promotes the CI-variant
//! erase (one subject) to its backup-scale form (a scaled corpus + the restore→re-erase cross-seam) and
//! proves the 0-recoverable property holds across a restore. The REF-P15 erase mutation floor
//! (`holder.rs` §, 13/13 viable = 100%) STILL HOLDS at scale — this module adds NO new erase decision
//! logic to mutate; it scales the corpus the frozen surface runs over and the drill's own counter-case
//! (a missed ledger entry → a resurrected key) flips the 0-recoverable verdict, proving the green is
//! earned.
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet
//!   hardware) — [`WORLD_SCALE_BACKUP_FLEET_FLOOR`]. This module proves the 0-recoverable-PII PROPERTY
//!   across a backup-scale corpus + the restore→re-erase cross-seam over the in-memory
//!   [`crate::R2ProjectionCache`] (real crypto-shred — the cached title is genuinely undecryptable after
//!   a DEK destroy) + the [`crate::EdgeProjection`] (the §3.2 `edge` table's semantics). The real
//!   backup/restore of the PgStore-backed edge partition at the full 30× cardinality is the named floor —
//!   it does NOT change the seam (the ledger replay + the crypto-shred are the SAME at any scale; a
//!   shredded key is unrecoverable by construction). The dev-stack Valkey-backed half is the REF-P15
//!   integration test (`integration_ref_p15_holder_erase.rs`) — this module's in-memory cache is the
//!   SAME `R2ProjectionCache` crypto-shred path with an `InMemoryCache` backing.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{EventEnvelope, EventHandler};
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::cache::R2ProjectionCache;
use crate::dek::RefsDekPin;
use crate::edge_builder::{EdgeProjection, RefsEdgeBuilder};
use crate::holder::RefsCacheHolder;
use crate::resolve::{Projection, ProjectionCacheRead};

/// **FLOOR (the ONE legitimate remaining floor): the 30× world-scale backup/restore on REAL FLEET
/// HARDWARE.** This module proves the 0-recoverable-PII PROPERTY (no resurrected PII past an erasure
/// across a restore) over a DETERMINISTIC backup-scale corpus with REAL crypto-shred (a per-subject DEK
/// destroy makes the cached title genuinely undecryptable) over the in-memory [`R2ProjectionCache`] +
/// [`EdgeProjection`] (the §3.2 `edge` table's semantics). The real backup/restore of the PgStore-backed
/// edge partition at the full 30× cardinality on real fleet hardware is the named floor — it does NOT
/// change the seam (the ledger replay + the crypto-shred are identical at any scale; a shredded key is
/// unrecoverable by construction). EI-01 §3: name the floor; never claim a green you did not earn.
pub const WORLD_SCALE_BACKUP_FLEET_FLOOR: &str =
    "REF-D5 at full 30x world-scale backup cardinality over the PgStore-backed edge partition + the \
     KMS/Valkey backup on real fleet hardware (the ONE legitimate remaining floor); the \
     0-recoverable-PII property + the restore→re-erase cross-seam are proven here over a deterministic \
     backup-scale corpus with REAL crypto-shred";

/// The telemetry signal name a backup-scale re-erase emits (contract 1.8): `0` recoverable PII after the
/// post-restore re-erase pass (the gate reading). A named constant — drills assert against the NAME,
/// never a literal (EI-01 §3 observability).
pub const REERASE_RECOVERABLE_PII_SIGNAL: &str = "refs.reerase_recoverable_pii";

// ════════════════════════════════════════════════════════════════════════════════════════════
// One ledger entry — the PII-free record of an erased subject (contract 10.8)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// One erasure-ledger entry — a PII-free record that a Refs subject was erased and which per-subject DEK
/// key refs were crypto-shredded + which edges were tombstoned (contract 10.8 / §4.6). It carries ONLY
/// opaque ids: the pseudonymous `origin_actor` subject id (never a name), the per-subject DEK
/// `PiiKeyRef` strings (a key NAME, not key material), the `(tenant, region)` cell, the deterministic
/// edge ids tombstoned, and a timestamp. It must survive the crypto-shred it records AND a restore — so
/// the re-erase pass can replay it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsErasedSubject {
    /// The opaque pseudonymous subject discriminator that was erased (the `origin_actor` opaque id —
    /// already pseudonymous, never real-identity PII).
    pub subject_id: String,
    /// The cell the erasure ran within (Refs is residency-pinned; the re-erase replays in the SAME
    /// cell, never across one).
    pub tenant: TenantId,
    /// The region partition the erasure ran within.
    pub region: Region,
    /// The DISTINCT per-subject DEK refs that were crypto-shredded for this subject's cached titles. A
    /// re-erase re-destroys each (idempotent). Sorted/deduped — a key NAME, never key material.
    pub key_refs: Vec<String>,
    /// The deterministic edge ids the erasure tombstoned (the §4.6 `*.erased` edge surface). A re-erase
    /// re-tombstones each restored row. PII-free: opaque edge identifiers.
    pub edge_ids: Vec<String>,
    /// When the erasure was recorded (the audit timestamp). PII-free.
    pub erased_at: String,
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The PII-free, non-shred-erasable Refs erasure ledger (contract 10.8, CONSUMED)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// Refs' slice of the PII-free erasure ledger (contract 10.8, **CONSUMED**) — the SAME shape the Bus's
/// `BusErasureLedger` froze (EI-01 §7 cold == live). It durably records which subjects Refs erased +
/// which per-subject DEKs it shredded + which edges it tombstoned, so [`re_erase_at_backup_scale`] can
/// replay them after a restore. PII-free + non-shred-erasable: it must OUTLIVE the keys it records and
/// SURVIVE a restore (that is the whole point — a restored pre-erase backup must not resurrect a subject
/// the ledger remembers erasing). Keyed by `(tenant, region, subject_id)` so a re-erase of an
/// already-recorded subject MERGES (idempotent record). `BTreeMap` so the replay order is deterministic
/// (the drill artifact is reproducible).
#[derive(Clone, Default)]
pub struct RefsErasureLedger {
    entries: Arc<Mutex<LedgerMap>>,
}

/// The ledger's keyed store: `(tenant, region, subject_id)` → the PII-free erased-subject record. A
/// `BTreeMap` so the replay order is deterministic (the drill artifact is reproducible).
type LedgerMap = BTreeMap<LedgerKey, RefsErasedSubject>;

/// The ledger key — the `(tenant, region, subject_id)` triple (all opaque strings; residency-pinned).
type LedgerKey = (String, String, String);

impl RefsErasureLedger {
    /// A fresh, empty erasure ledger.
    pub fn new() -> RefsErasureLedger {
        RefsErasureLedger::default()
    }

    fn key(tenant: &TenantId, region: &Region, subject_id: &str) -> LedgerKey {
        (tenant.0.clone(), region.0.clone(), subject_id.to_string())
    }

    /// Record that `subject_id` was erased in `(tenant, region)`, crypto-shredding `key_refs` and
    /// tombstoning `edge_ids` (contract 10.8). Idempotent: recording a subject already present MERGES
    /// the key refs + edge ids (a later erase may have located more) and keeps the FIRST `erased_at`.
    /// Called after a successful §4.6 erase (or by [`erase_and_record_at_scale`], which does both).
    pub fn record(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject_id: &str,
        key_refs: &[String],
        edge_ids: &[String],
        erased_at: &str,
    ) {
        let mut g = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = g
            .entry(Self::key(tenant, region, subject_id))
            .or_insert_with(|| RefsErasedSubject {
                subject_id: subject_id.to_string(),
                tenant: tenant.clone(),
                region: region.clone(),
                key_refs: Vec::new(),
                edge_ids: Vec::new(),
                erased_at: erased_at.to_string(),
            });
        for k in key_refs {
            if !entry.key_refs.contains(k) {
                entry.key_refs.push(k.clone());
            }
        }
        for e in edge_ids {
            if !entry.edge_ids.contains(e) {
                entry.edge_ids.push(e.clone());
            }
        }
        entry.key_refs.sort();
        entry.edge_ids.sort();
    }

    /// Whether the ledger remembers erasing `subject_id` in `(tenant, region)` (the fail-closed read).
    /// True once `record`ed; a restore CANNOT clear it (non-shred-erasable).
    pub fn is_erased(&self, tenant: &TenantId, region: &Region, subject_id: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&Self::key(tenant, region, subject_id))
    }

    /// Every recorded erasure, in deterministic (cell-then-subject-sorted) order — what the re-erase
    /// pass replays. PII-free.
    pub fn entries(&self) -> Vec<RefsErasedSubject> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// How many subjects the ledger has recorded as erased.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// A deterministic backup-scale corpus (REF-D5 at scale)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **A deterministic backup-scale erasure corpus (REF-D5 at scale).** `subjects` subjects, each
/// authoring `edges_per_subject` edges whose cached projection title holds a name (the only name-bearing
/// PII). Deterministic for a given `(subjects, edges_per_subject)` — so the steady-state warm and the
/// post-restore re-warm see byte-identical inputs (the cold==live invariant). PII-free at rest: every
/// URN is opaque; every `origin_actor` is a pseudonymous ref; the "name" lives ONLY in the cached title
/// sealed under the per-subject DEK.
#[derive(Clone, Debug)]
pub struct BackupScaleErasureCorpus {
    /// The cell the corpus lives in.
    pub tenant: TenantId,
    /// The region partition.
    pub region: Region,
    /// The pseudonymous subject ids (the `origin_actor`s) — the erasure targets.
    pub subjects: Vec<String>,
    /// Per subject: the edge events it authored (source/target refs + the deterministic edge id).
    pub edges: Vec<CorpusEdge>,
}

/// One corpus edge — a reference the subject authored. The cached `source` title holds the name (the
/// only name-bearing PII); the edge itself is opaque (the pseudonymous `origin_actor`).
#[derive(Clone, Debug)]
pub struct CorpusEdge {
    /// The pseudonymous subject (`origin_actor`) that authored this edge.
    pub subject_id: String,
    /// The source ref (whose cached title holds the name).
    pub source: ArtifactRef,
    /// The target ref.
    pub target: ArtifactRef,
    /// The deterministic edge id (the §4.1 `ON CONFLICT` key — used to assert the tombstone surface).
    pub edge_id: String,
    /// The cached projection title for the source (a NAME — the PII purged/crypto-shredded on erase).
    pub cached_title: String,
}

/// **Build a deterministic backup-scale erasure corpus.** `subjects` pseudonymous subjects, each
/// authoring `edges_per_subject` edges whose source cached title holds the subject's display name. Same
/// `(subjects, edges_per_subject)` ⇒ same corpus (byte-reproducible). Both must be `> 0`.
pub fn build_backup_scale_corpus(
    tenant: &TenantId,
    region: &Region,
    subjects: usize,
    edges_per_subject: usize,
) -> BackupScaleErasureCorpus {
    assert!(subjects > 0, "the backup-scale corpus needs ≥1 subject");
    assert!(
        edges_per_subject > 0,
        "each subject must author ≥1 edge (a name-bearing cached title)"
    );
    let mut subject_ids = Vec::with_capacity(subjects);
    let mut edges = Vec::with_capacity(subjects * edges_per_subject);
    for s in 0..subjects {
        let subject_id = format!("p-opaque-{s}");
        subject_ids.push(subject_id.clone());
        for e in 0..edges_per_subject {
            let source = ArtifactRef(format!("myelin://{}/chat/message/m-{s}-{e}", tenant.0));
            let target = ArtifactRef(format!("myelin://{}/knowledge/page/p-{s}-{e}", tenant.0));
            edges.push(CorpusEdge {
                subject_id: subject_id.clone(),
                edge_id: crate::edge_builder::edge_id(tenant, &source.0, &target.0, "mentions"),
                source,
                target,
                // A NAME in the cached title — the only name-bearing PII, sealed under the per-subject
                // DEK (so a crypto-shred makes it unrecoverable, live AND in backup).
                cached_title: format!("Subject {s} Name (#{e})"),
            });
        }
    }
    BackupScaleErasureCorpus {
        tenant: tenant.clone(),
        region: region.clone(),
        subjects: subject_ids,
        edges,
    }
}

impl BackupScaleErasureCorpus {
    /// The total edge count across all subjects.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// The edges authored by `subject_id` (the subject's erasure surface).
    pub fn edges_of<'a>(
        &'a self,
        subject_id: &'a str,
    ) -> impl Iterator<Item = &'a CorpusEdge> + 'a {
        self.edges
            .iter()
            .filter(move |e| e.subject_id == subject_id)
    }

    /// Build the live `refs.edge.created` envelope for a corpus edge — the SAME payload shape the live
    /// consumer + the snapshot replay carry (the cold==live invariant), driven through the live `handle`.
    fn edge_event(&self, edge: &CorpusEdge) -> EventEnvelope {
        use myelin_events::{
            Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
        };
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        EventEnvelope {
            event_id: EventId(format!("live-{}", edge.edge_id)),
            type_: EventType("refs.edge.created".into()),
            schema_ver: 1,
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            actor: Actor(Principal::stub(
                PrincipalId(edge.subject_id.clone()),
                PrincipalKind::Human,
                self.tenant.clone(),
            )),
            subject: edge.source.clone(),
            aggregate: AggregateKey(format!("edge:{}->{}", edge.source.0, edge.target.0)),
            causation_id: None,
            correlation_id: CorrelationId(format!("live-{}", edge.edge_id)),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
            payload: serde_json::json!({
                "source": edge.source.0,
                "target": edge.target.0,
                "rel": "mentions",
            }),
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The backup-scale re-erase report (the REF-D5-at-scale green artifact)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The dated artifact a backup-scale restore + re-erase pass returns (the REF-D5-at-scale green). It is
/// the PROOF the erasure stays applied across a restore: how many subjects were re-erased, how many
/// cached titles / per-subject DEKs the RESTORE resurrected (the honest "what the backup brought back"
/// signal), how many edges were re-tombstoned, and the post-pass `recoverable_pii` which MUST be **0**
/// (the gate threshold). PII-free: opaque subject ids + counts, never payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupScaleReEraseReport {
    /// The cell the re-erase ran within (Refs never crosses it).
    pub tenant: TenantId,
    /// The region partition.
    pub region: Region,
    /// How many ledger-listed subjects were replayed through the §4.6 re-erase.
    pub re_erased_subjects: usize,
    /// How many cached titles the RESTORE resurrected (a live, decryptable name again BEFORE the
    /// re-erase re-purged/re-shredded them) — the honest signal of what the older backup brought back.
    pub cached_titles_resurrected_by_restore: usize,
    /// How many per-subject DEKs the RESTORE resurrected (live/resolvable again before the re-shred).
    pub deks_resurrected_by_restore: usize,
    /// How many edge rows the re-erase re-tombstoned (the restored rows lost their tombstone).
    pub edges_re_tombstoned: usize,
    /// **THE GATE READING:** how many cached titles are STILL recoverable (decrypt to a name) AFTER the
    /// re-erase pass — MUST be **0** (the re-erase re-purged + re-shredded everything the restore
    /// resurrected). A non-zero value is a RED drill: a restored backup resurrected an erased subject's
    /// name past the erasure.
    pub recoverable_pii: usize,
    /// **THE SECONDARY GATE:** how many per-subject DEKs are STILL live (resolvable) AFTER the re-erase —
    /// MUST be **0** (a live DEK means the cached title could be decrypted from a backup blob).
    pub live_deks_post_reerase: usize,
    /// **THE EDGE GATE:** how many of the corpus edges are STILL live (not tombstoned) for a re-erased
    /// subject AFTER the pass — MUST be **0** (every reference stays tombstoned; the person unresolvable).
    pub live_edges_post_reerase: usize,
    /// When the pass ran (the dated artifact).
    pub ran_at: String,
}

impl BackupScaleReEraseReport {
    /// Whether the backup-scale REF-D5 is GREEN: **0 recoverable PII** post-restore across every surface
    /// (0 decryptable cached titles, 0 live per-subject DEKs, 0 live edges for a re-erased subject).
    pub fn is_ref_d5_backup_scale_green(&self) -> bool {
        self.recoverable_pii == 0
            && self.live_deks_post_reerase == 0
            && self.live_edges_post_reerase == 0
    }

    /// A one-line human summary (the dated artifact's body).
    pub fn summary(&self) -> String {
        format!(
            "REF-D5 backup-scale: re_erased={} (restore resurrected {} titles / {} DEKs) \
             re_tombstoned={} → recoverable_pii={} live_deks={} live_edges={} green={}",
            self.re_erased_subjects,
            self.cached_titles_resurrected_by_restore,
            self.deks_resurrected_by_restore,
            self.edges_re_tombstoned,
            self.recoverable_pii,
            self.live_deks_post_reerase,
            self.live_edges_post_reerase,
            self.is_ref_d5_backup_scale_green(),
        )
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// Step (2): erase + record at backup scale (the §4.6 erase over the live surface + the ledger)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The crypto-shred unit for a cached title (§3.6 / REF-P4).** Seal a subject's cached projection
/// titles under that subject's per-subject DEK backstop, so a per-subject DEK destroy makes the cached
/// title genuinely undecryptable (a MISS, never a plaintext fall-through). The cached title is the only
/// name-bearing PII; the per-subject DEK is the GD-4 individual lever.
///
/// `subject_cache` fills the subject's edges' source titles into the cache sealed under the subject's
/// per-subject DEK, returning the distinct per-subject DEK refs minted (the ledger's `key_refs`).
fn warm_subject_titles(
    corpus: &BackupScaleErasureCorpus,
    cache: &R2ProjectionCache,
    dek: &RefsDekPin,
    subject_id: &str,
) -> Vec<String> {
    // Reserve (idempotently) the subject's per-subject DEK backstop — the crypto-shred unit for its
    // cached titles. The per-tenant DEK seals the cache write; the per-subject backstop is the
    // subject-grained shred lever (we crypto-shred IT on erase). The cache encrypts under the per-tenant
    // DEK; we ALSO reserve + (on erase) destroy the per-subject backstop so the shred is subject-grained.
    let key_ref = dek
        .reserve_subject_backstop(&corpus.tenant, &corpus.region, subject_id)
        .expect("reserve per-subject DEK backstop");
    for edge in corpus.edges_of(subject_id) {
        let proj = Projection {
            ref_: edge.source.clone(),
            title: edge.cached_title.clone(),
            state: "open".into(),
            icon: "doc".into(),
            render_hint: "card".into(),
            sub_anchor: None,
            flag: None,
        };
        cache
            .fill(&corpus.tenant, &corpus.region, &edge.source, &proj)
            .expect("warm the subject's cached title");
    }
    vec![key_ref.to_uri()]
}

/// **Erase ONE subject at backup scale + record it in the ledger (step 2).** Runs the IDENTICAL §4.6
/// erase the REF-P15 holder runs — the cache-PII purge through [`RefsCacheHolder::erase`] (the ONE
/// `invalidate` path, no backdoor), the per-subject DEK crypto-shred (REF-P4 — the cached title becomes
/// unrecoverable live AND in backup), and the edge tombstone through the SAME
/// [`EdgeProjection::tombstone`] the `*.erased` consumer drives — then RECORDS the erasure in the
/// PII-free ledger (10.8) so a post-restore re-erase can replay it. Idempotent.
#[allow(clippy::too_many_arguments)]
fn erase_and_record_at_scale(
    corpus: &BackupScaleErasureCorpus,
    cache: &Arc<R2ProjectionCache>,
    dek: &RefsDekPin,
    projection: &EdgeProjection,
    ledger: &RefsErasureLedger,
    subject_id: &str,
    key_refs: &[String],
    now: &str,
) {
    // (a) cache-PII purge through the REAL §4.6 holder erase (the ONE invalidate path).
    let holder = RefsCacheHolder::with_cache(Arc::clone(cache), projection.clone());
    holder
        .erase(EraseScope::Subject {
            subject: subject_ref(subject_id, &corpus.tenant),
            tenant: gtenant(&corpus.tenant),
        })
        .expect("§4.6 cache-PII purge");

    // (b) per-subject DEK crypto-shred — the cached title is now unrecoverable, live AND in backup
    //     (§7.5 excludes a shredded key from backup). This is the backup-backstop the restore can only
    //     defeat by restoring an OLDER backup (which step 3 simulates, and step 4 re-shreds).
    dek.destroy_subject_backstop(&corpus.tenant, subject_id);

    // (c) edge tombstone through the SAME path the `*.erased` consumer drives (no backdoor). The
    //     references stay tombstoned → a resolve degrades to a Tombstone, never a 500.
    let mut edge_ids = Vec::new();
    for edge in corpus.edges_of(subject_id) {
        projection.tombstone(
            &corpus.tenant,
            &corpus.region,
            &edge.edge_id,
            &format!("erased:{subject_id}"),
        );
        edge_ids.push(edge.edge_id.clone());
    }

    // (d) RECORD in the PII-free, non-shred-erasable ledger (10.8) — the durable fact-of-erasure the
    //     re-erase pass replays after a restore.
    ledger.record(
        &corpus.tenant,
        &corpus.region,
        subject_id,
        key_refs,
        &edge_ids,
        now,
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// THE backup-scale restore + re-erase drill (REF-P25 / REF-D5 at scale)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **THE backup-scale REF-D5 drill (REF-P25): erase → restore a pre-erase backup → re-erase from the
/// ledger → 0 recoverable PII.** Rides the EXISTING §4.6 erase surface (no second eraser):
///
/// 1. **Steady-state.** Ingest every corpus edge through [`RefsEdgeBuilder::handle`] (the live consumer
///    path) and warm each subject's cached titles into the cache, sealed under its per-subject DEK.
/// 2. **Erase + record.** For every `subjects_to_erase` subject: run the IDENTICAL §4.6 erase (cache-PII
///    purge + per-subject DEK crypto-shred + edge tombstone) and RECORD it in the ledger.
/// 3. **Restore a PRE-erase backup.** Re-warm every erased subject's cached titles, re-seal (resurrect)
///    its per-subject DEK, and re-ingest (un-tombstone) its edges — exactly what restoring an OLDER
///    backup does.
/// 4. **Re-erase from the ledger.** REPLAY the ledger: re-run the IDENTICAL erase for every
///    ledger-listed subject (idempotent — re-shredding a dead key is a no-op).
/// 5. **Verify.** Probe every surface: 0 decryptable cached titles, 0 live per-subject DEKs, 0 live
///    edges for a re-erased subject (the person unresolvable; no 500 on resolve). Return the
///    [`BackupScaleReEraseReport`] (the dated green artifact).
///
/// `cache` is the REAL [`R2ProjectionCache`] (an `InMemoryCache` backing in the drill, a Valkey backing
/// in prod — the SAME crypto-shred path). `dek` is the REAL [`RefsDekPin`]. The crypto-shred is REAL: a
/// per-subject DEK destroy makes the sealed cached title undecryptable by construction.
#[allow(clippy::too_many_arguments)]
pub fn re_erase_at_backup_scale(
    corpus: &BackupScaleErasureCorpus,
    builder: &RefsEdgeBuilder,
    cache: &Arc<R2ProjectionCache>,
    dek: &RefsDekPin,
    ledger: &RefsErasureLedger,
    subjects_to_erase: &[String],
    now: &str,
) -> BackupScaleReEraseReport {
    let projection = builder.projection().clone();

    // ── (1) STEADY-STATE: ingest every edge + warm every cached title (sealed under per-subject DEKs). ──
    for edge in &corpus.edges {
        builder.handle(&corpus.edge_event(edge), &mut myelin_events::HandlerTx::none());
    }
    let mut subject_keys: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for subject_id in &corpus.subjects {
        let keys = warm_subject_titles(corpus, cache, dek, subject_id);
        subject_keys.insert(subject_id.clone(), keys);
    }

    // ── (2) ERASE + RECORD each target subject (the §4.6 erase + the 10.8 ledger). ──
    for subject_id in subjects_to_erase {
        let keys = subject_keys.get(subject_id).cloned().unwrap_or_default();
        erase_and_record_at_scale(
            corpus,
            cache,
            dek,
            &projection,
            ledger,
            subject_id,
            &keys,
            now,
        );
    }

    // ── (3) RESTORE a PRE-erase backup: re-warm titles, resurrect DEKs, un-tombstone edges. ──
    // (a) resurrect each erased subject's per-subject DEK (a restore re-seals the pre-erase key) +
    //     re-warm the cached title (a restore brings the name-bearing blob back).
    let mut cached_titles_resurrected_by_restore = 0usize;
    let mut deks_resurrected_by_restore = 0usize;
    for subject_id in subjects_to_erase {
        // The restore resurrects the per-subject DEK (re-reserve = re-seal the pre-erase key).
        dek.reserve_subject_backstop(&corpus.tenant, &corpus.region, subject_id)
            .expect("restore re-seals the per-subject DEK");
        deks_resurrected_by_restore += 1;
        // The restore re-warms the cached title (the name-bearing blob is back, decryptable again).
        for edge in corpus.edges_of(subject_id) {
            let proj = Projection {
                ref_: edge.source.clone(),
                title: edge.cached_title.clone(),
                state: "open".into(),
                icon: "doc".into(),
                render_hint: "card".into(),
                sub_anchor: None,
                flag: None,
            };
            cache
                .fill(&corpus.tenant, &corpus.region, &edge.source, &proj)
                .expect("restore re-warms the cached title");
            // Confirm the restore genuinely resurrected the PII (the honest "what came back" probe).
            if cache
                .read(&corpus.tenant, &corpus.region, &edge.source)
                .is_some()
            {
                cached_titles_resurrected_by_restore += 1;
            }
        }
    }
    // (b) un-tombstone (re-ingest) every restored edge — a restore brings the live row back WITHOUT its
    //     post-backup tombstone.
    for edge in &corpus.edges {
        if subjects_to_erase.contains(&edge.subject_id) {
            builder.handle(&corpus.edge_event(edge), &mut myelin_events::HandlerTx::none());
        }
    }

    // ── (4) RE-ERASE FROM THE LEDGER (10.8): replay every recorded erasure (cold == live, idempotent). ──
    let mut edges_re_tombstoned = 0usize;
    for entry in ledger.entries() {
        if entry.tenant != corpus.tenant || entry.region != corpus.region {
            continue; // residency-pin: re-erase only within this cell.
        }
        // (a) re-purge the cached PII through the IDENTICAL §4.6 holder erase.
        let holder = RefsCacheHolder::with_cache(Arc::clone(cache), projection.clone());
        holder
            .erase(EraseScope::Subject {
                subject: subject_ref(&entry.subject_id, &corpus.tenant),
                tenant: gtenant(&corpus.tenant),
            })
            .expect("re-erase cache purge");
        // (b) re-destroy the resurrected per-subject DEK (idempotent — a dead key is a no-op).
        for _ in &entry.key_refs {
            dek.destroy_subject_backstop(&corpus.tenant, &entry.subject_id);
        }
        // (c) re-tombstone every restored edge (re-tombstoning is idempotent).
        for edge_id in &entry.edge_ids {
            projection.tombstone(
                &corpus.tenant,
                &corpus.region,
                edge_id,
                &format!("re-erased:{}", entry.subject_id),
            );
            edges_re_tombstoned += 1;
        }
    }

    // ── (5) VERIFY: 0 recoverable PII across every surface (the gate reading). ──
    let mut recoverable_pii = 0usize;
    let mut live_edges_post_reerase = 0usize;
    for subject_id in subjects_to_erase {
        for edge in corpus.edges_of(subject_id) {
            // A decryptable cached title = recoverable PII (the per-subject DEK shred must make it a MISS,
            // never a plaintext fall-through). After re-erase + re-shred, this MUST be a clean MISS.
            if cache
                .read(&corpus.tenant, &corpus.region, &edge.source)
                .is_some()
            {
                recoverable_pii += 1;
            }
            // A live (non-tombstoned) edge for a re-erased subject = the person still resolvable. MUST be 0.
            if projection
                .get(&corpus.tenant, &corpus.region, &edge.edge_id)
                .map(|r| !r.tombstoned)
                .unwrap_or(false)
            {
                live_edges_post_reerase += 1;
            }
        }
    }
    // A live (resolvable) per-subject DEK for a re-erased subject = the cached title could be decrypted
    // from a backup blob. MUST be 0 (the crypto-shred is the backup-backstop).
    let mut live_deks_post_reerase = 0usize;
    for subject_id in subjects_to_erase {
        // READ-ONLY probe (it does NOT resurrect the key it checks) — a live DEK means the cached title
        // could be decrypted from a backup blob. MUST be 0 after the re-erase re-shred.
        if dek.subject_backstop_is_live(&corpus.tenant, &corpus.region, subject_id) {
            live_deks_post_reerase += 1;
        }
    }

    BackupScaleReEraseReport {
        tenant: corpus.tenant.clone(),
        region: corpus.region.clone(),
        re_erased_subjects: ledger
            .entries()
            .iter()
            .filter(|e| e.tenant == corpus.tenant && e.region == corpus.region)
            .count(),
        cached_titles_resurrected_by_restore,
        deks_resurrected_by_restore,
        edges_re_tombstoned,
        recoverable_pii,
        live_deks_post_reerase,
        live_edges_post_reerase,
        ran_at: now.to_string(),
    }
}

/// Build a [`SubjectRef`] from an opaque pseudonymous subject id (never a name — the §4.6 posture).
fn subject_ref(subject_id: &str, tenant: &TenantId) -> SubjectRef {
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    SubjectRef::new(Principal::stub(
        PrincipalId(subject_id.into()),
        PrincipalKind::Human,
        tenant.clone(),
    ))
}

/// The gdpr `TenantId` for a tenancy `TenantId` (gdpr's `TenantId` IS the tenancy one — a type alias).
fn gtenant(tenant: &TenantId) -> GdprTenantId {
    tenant.clone()
}

#[cfg(test)]
mod tests;
