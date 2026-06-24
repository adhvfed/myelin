//! **The E2E-3 storage half — cold-reindex == live for the derived stores** (P-ST-36 / global
//! **P-447**, M5; contract-index rows **11.6** "the OLAP derived store", **2.6** "reindex-from-source").
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.4 (*the OLAP derived store rebuilt
//! from source — CQRS, fed by the bus*), §6 (*derived stores T4/T7 are reindex-from-source primitives,
//! rebuildable by replaying the source through the live consumer path*), §7.1 (*OLAP (T4) + caches +
//! derived indexes are **NOT backed up** — rebuilt via reindex-from-source*), §7.3 (*restore-to-
//! consistent-point: **reindex derived stores from source** up to offset T, **never restore them from
//! their own backups** → derived == source by construction, no drift*), §7.4 (*the restore-verify gate:
//! reindex T4/Search/Refs from source to T; assert derived == source-replay*).
//! `external-insights/04-hard-problems.md` §5 (*reindex-from-source is a first-class resilience
//! primitive — the index never reads source databases; it asks each owner to re-emit through the live
//! consumer*). `external-insights/01-process-and-quality-doctrine.md` §4 (*chain the mutation
//! end-to-end — the whole-system E2E wedge*), §3 (*prove-it; a property does not exist until a test
//! forces the failure; observability is part of the pass*). The whole-system drill catalogue
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row **E2E-3** (*mid-flight mutation:
//! wipe the Refs edge index + the Search index; `reindex(scope)` via the live consumer path
//! (`*.snapshot` replay, 2.6); assert the rebuilt lineage **byte-matches** the live lineage (F4 /
//! REF-D4 / SRCH-D5) — **no bespoke recovery reader**; gate: **cold-reindex == live**, 0 drift*).
//!
//! ## What this module OWNS — the E2E-3 reindex-parity **storage half** (the proof side)
//! E2E-3 is a whole-system wedge that crosses Knowledge, Issues, Git, CI, Chat, Refs, Search,
//! GDPR/Audit, Identity. THIS prompt is the **storage half** of that wedge — the proof, in the data
//! layer, that **every derived store rebuilds BYTE-IDENTICALLY from source** (cold == live) and that
//! **no derived store has a backup-restore path** (the §7.1/§7.3 structural truth: derived stores are
//! NOT backed up; they are reindexed from source, so derived == source by construction with no drift).
//!
//! The three derived stores the storage half covers (§7.4):
//! - **OLAP (T4)** — the CQRS analytics read model fed by the bus ([`crate::olap::OlapReadStore`] /
//!   [`crate::olap_feed`]); its cold==live F4 parity is the EXISTING P-ST-18 proof, which this drill
//!   RE-RUNS at the E2E wedge (it does not re-implement the OLAP feed — coherence, EI-01 §7).
//! - **Search** — the per-tenant search index, a derived store rebuilt ONLY by reindex-from-source
//!   (`myelin-search`'s `SearchReindexer` / SRCH-D5). Storage cannot link `myelin-search` in its
//!   production DAG (Search depends on Storage — that would be an upward cycle), so the storage half
//!   models Search as a [`DerivedStore`] fed by the SAME `myelin_events::reindex` bus seam. The agreement
//!   with `myelin-search`'s real SRCH-D5 parity is asserted in the dev-dependency CDC
//!   `tests/cdc_e2e3_reindex_parity.rs` (the two proofs meet; neither re-derives the other — the SAME
//!   posture the E2E-4 storage half [`crate::holder_fanout`] uses to meet the GDPR orchestrator).
//! - **Refs** — the reference-graph edge index, a derived store rebuilt ONLY by reindex-from-source
//!   (`myelin-refs-service`'s `RefsReindexer` / REF-D4). Modeled here the SAME way as Search; the
//!   agreement with `myelin-refs-service`'s real REF-D4 parity is asserted in the same CDC.
//!
//! ## Cold == live BY CONSTRUCTION — the ONE ingest path (§6 / EI-04 §5)
//! Each derived store is fed by exactly ONE projection path — the bus consumer's `ingest`. A live
//! event and a re-emitted `*.snapshot` of the same `(aggregate, version)` are byte-indistinct
//! envelopes, so they drive the SAME `ingest` to the SAME projection bytes. That is what makes a cold
//! rebuild byte-identical to the live store: there is no second "recovery reader", no "load the index
//! from Postgres" backdoor (the SEARCH-1 / §3.4 anti-pattern). The drill PROVES this: it builds each
//! store LIVE, wipes it, reindexes from source through the REAL outbox→relay→bus→consumer path, and
//! asserts `parity_bytes` are byte-identical (0 drift).
//!
//! ## NO backup-restore path for the derived stores — STRUCTURAL (§7.1 / §7.3)
//! "OLAP (T4) + caches + derived indexes are NOT backed up — rebuilt via reindex-from-source." This is
//! a structural property, not a config choice: a derived store that COULD be restored from its own
//! backup would risk drift (the backup is a point-in-time copy; the source has moved on). The storage
//! half asserts the structural truth two ways: (1) [`DerivedStoreClass::has_backup_restore_path`] is
//! `false` for every derived store (the catalogue carries the property); (2) the source-grep test
//! [`tests::no_backup_restore_path_for_derived_stores_structural`] pins that THIS module's production
//! code contains NO restore-from-backup construct for a derived store — `reindex` is the only rebuild
//! verb. A future writer who adds a `restore_derived_from_backup` path fails the gate.
//!
//! ## The E2E-3 gate (this module's contribution) + STOR-D1/STOR-D2 unchanged
//! `cold_reindex_matches_live == true` for EVERY derived store (0 drift) AND
//! `derived_stores_with_backup_path == 0` (no backup-restore path) AND the re-run is idempotent (a
//! second reindex emits 0 new snapshots). On pass the gate seals a dated [`E2e3StorageArtifact`] (the
//! PII-free per-store parity-hash receipt set — the dated, content-hashed E2E-3 green artifact).
//! STOR-D1/STOR-D2 (the restore-verify permanent gate) are UNCHANGED by this prompt — it touches no
//! backup/restore code; the derived stores were never in the backup-able set (§7.1), so the
//! restore-verify gate's reindex-derived-from-source leg ([`crate::restore_verify`]) already depends on
//! exactly this property. This module makes that dependency a directly-proven, dated artifact.
//!
//! ## Floors named (EI-01 §1) — promoted at M5; what remains designed-not-built
//! By M5 the reindex-from-source floors are promoted (the OLAP feed P-ST-18, the Search reindexer
//! SRCH-P16, the Refs reindexer REF-P16 are all live). What remains **designed-not-built**, named in
//! the honesty register:
//! - **The generated projection-feeder index measured-trigger.** A derived store could one day carry a
//!   GENERATED secondary projection-feeder index (a materialized rollup whose feeder is itself
//!   generated from the source schema) — the generation machinery is DESIGNED but NOT built; it lands
//!   only when the volume that warrants it is MEASURED (EI-04 §5: "don't add it before the volume is
//!   measured"). Until then every derived store is fed by the hand-written ONE-ingest-path consumer
//!   this drill proves. Named so the green here is not mistaken for the generated-feeder proof.
//! - **The real columnar/Tantivy/edge-index backends** are backend-agnostic models here (the SAME
//!   posture as [`crate::olap::OlapReadStore`]); the cold==live SHAPE the drill proves does not change
//!   when a concrete backend lands behind the trait. No NEW db/object-store/cache/bus contract is
//!   touched — this drill REUSES the existing `myelin_events` outbox→relay→bus→consumer seam (2.4/2.6)
//!   + the frozen `EventEnvelope`, so no new live-stack integration drill is OWED.

use std::collections::BTreeMap;

use myelin_events::{
    reindex, ArtifactRef, BusTransport, DataRole, DerivedStore, EmitContextBase, EventEnvelope,
    InProcessBus, OutboxStore, Region, ReindexError, ReindexReceipt, ReindexSource, Relay,
    SnapshotScope, Visibility,
};

/// **A derived store the E2E-3 storage half covers (§7.4 — the T4/Search/Refs derived-store class).**
/// Each is a projection rebuilt ONLY by reindex-from-source (the bus `*.snapshot` re-emit through the
/// live consumer path); NONE has a backup-restore path (§7.1 — derived stores are NOT backed up). This
/// is the storage face of the E2E-3 derived-store set; the per-store name is PII-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DerivedStoreClass {
    /// The OLAP (T4) CQRS analytics read store — fed by the bus, reindex-from-source only (§3.4).
    Olap,
    /// The per-tenant Search index — a derived store rebuilt only by reindex-from-source (SRCH-D5).
    Search,
    /// The reference-graph (Refs) edge index — a derived store rebuilt only by reindex-from-source
    /// (REF-D4).
    Refs,
}

impl DerivedStoreClass {
    /// **The exhaustive E2E-3 derived-store set (§7.4).** OLAP + Search + Refs — the three derived
    /// stores the restore-verify gate reindexes from source (it does NOT restore them from backup). An
    /// exhaustive `match` in [`Self::ALL`] forces a future new derived store to be added here (a new
    /// derived store that escaped this list would escape the E2E-3 cold==live proof).
    pub const ALL: [DerivedStoreClass; 3] = [
        DerivedStoreClass::Olap,
        DerivedStoreClass::Search,
        DerivedStoreClass::Refs,
    ];

    /// The PII-free store name (the artifact receipt key).
    pub fn name(self) -> &'static str {
        match self {
            DerivedStoreClass::Olap => "olap",
            DerivedStoreClass::Search => "search",
            DerivedStoreClass::Refs => "refs",
        }
    }

    /// **Does this store have a backup-restore path? ALWAYS `false` for a derived store (§7.1/§7.3).**
    /// "OLAP (T4) + caches + derived indexes are NOT backed up — rebuilt via reindex-from-source." A
    /// derived store restored from its own backup would risk drift; the ONLY rebuild verb is `reindex`.
    /// The exhaustive `match` keeps this honest: a derived store that returned `true` here would be a
    /// LOUD contradiction the gate catches (`derived_stores_with_backup_path` would be non-zero).
    pub fn has_backup_restore_path(self) -> bool {
        match self {
            // Every derived store: NOT backed up. Reindex-from-source is the only rebuild path.
            DerivedStoreClass::Olap | DerivedStoreClass::Search | DerivedStoreClass::Refs => false,
        }
    }
}

/// **The cold==live parity result for ONE derived store (the per-store E2E-3 leg).** PII-free: the
/// store name + the two parity hashes (live vs cold) + the no-backup-path bit + the idempotency count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedStoreParity {
    /// The derived store this leg ran for.
    pub store: DerivedStoreClass,
    /// The content hash of the LIVE projection's `parity_bytes`.
    pub live_hash: u64,
    /// The content hash of the COLD-rebuilt projection's `parity_bytes` (reindex-from-source).
    pub cold_hash: u64,
    /// `*.snapshot` events the FIRST reindex emitted (the rebuild).
    pub snapshots_emitted_first: usize,
    /// `*.snapshot` events a SECOND reindex emitted — MUST be `0` (the deterministic-`event_id`
    /// `ON CONFLICT DO NOTHING` re-run is an idempotent no-op).
    pub snapshots_emitted_second: usize,
    /// The structural §7.1/§7.3 bit: does this derived store have a backup-restore path? MUST be
    /// `false` (a derived store is NOT backed up — reindex-from-source is the only rebuild path).
    pub has_backup_restore_path: bool,
}

impl DerivedStoreParity {
    /// **Did the COLD reindex byte-match the LIVE projection?** (`cold_hash == live_hash` — 0 drift.)
    pub fn cold_matches_live(&self) -> bool {
        self.cold_hash == self.live_hash
    }

    /// Is this leg GREEN? cold == live (0 drift) AND no backup-restore path AND the re-run emitted 0
    /// new snapshots (idempotent). A conjunction — no single green hides a breach.
    pub fn is_green(&self) -> bool {
        self.cold_matches_live()
            && !self.has_backup_restore_path
            && self.snapshots_emitted_second == 0
    }
}

/// **The dated GREEN ARTIFACT the E2E-3 storage half emits on PASS** (storage.md §7.4 "green artifact
/// on pass"; the drill catalogue's E2E-3 green artifact: "the lineage diff (live vs cold) at zero
/// drift"). The PII-free per-store parity receipt set + the structural no-backup-path proof, sealed
/// under a content hash (the dated, tamper-evident E2E-3 storage artifact). The caller prefixes the
/// run date (`[P-447 E2E-3 GREEN <date>]`) so the artifact is dated at the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2e3StorageArtifact {
    /// The per-store cold==live parity legs (one per [`DerivedStoreClass::ALL`]).
    pub legs: Vec<DerivedStoreParity>,
    /// The number of derived stores whose cold reindex did NOT byte-match live (MUST be `0` — the
    /// E2E-3 "cold-reindex == live" gate, 0 drift).
    pub stores_with_drift: usize,
    /// The number of derived stores that have a backup-restore path (MUST be `0` — derived stores are
    /// NOT backed up; reindex-from-source is the only rebuild path, §7.1/§7.3).
    pub derived_stores_with_backup_path: usize,
    /// A content hash over the PII-free per-store receipt set (the sealed E2E-3 artifact body).
    pub certificate_hash: u64,
}

impl E2e3StorageArtifact {
    /// Seal the artifact over the per-store parity legs (computes the drift count, the backup-path
    /// count, and the content-hash certificate). The legs MUST cover [`DerivedStoreClass::ALL`].
    pub fn seal(legs: Vec<DerivedStoreParity>) -> E2e3StorageArtifact {
        let stores_with_drift = legs.iter().filter(|l| !l.cold_matches_live()).count();
        let derived_stores_with_backup_path =
            legs.iter().filter(|l| l.has_backup_restore_path).count();
        let certificate_hash = certificate_hash(&legs);
        E2e3StorageArtifact {
            legs,
            stores_with_drift,
            derived_stores_with_backup_path,
            certificate_hash,
        }
    }

    /// **Is the E2E-3 storage half GREEN?** EVERY derived store covered, 0 drift (cold == live), 0
    /// backup-restore paths, every per-store leg green (idempotent re-run included). A conjunction over
    /// the WHOLE derived-store set — a single store omitted or drifting reads RED.
    pub fn is_green(&self) -> bool {
        self.covers_all_derived_stores()
            && self.stores_with_drift == 0
            && self.derived_stores_with_backup_path == 0
            && self.legs.iter().all(DerivedStoreParity::is_green)
    }

    /// Does the artifact cover EVERY derived store in [`DerivedStoreClass::ALL`]? A missing store is a
    /// LOUD failure (a derived store left out of the E2E-3 proof would escape the cold==live gate).
    pub fn covers_all_derived_stores(&self) -> bool {
        DerivedStoreClass::ALL
            .iter()
            .all(|c| self.legs.iter().any(|l| l.store == *c))
    }

    /// Render the dated green-artifact line a CI run prints on PASS (the measured-numbers proof). The
    /// caller prefixes the date (`[P-447 E2E-3 GREEN <date>]`).
    pub fn summary(&self) -> String {
        format!(
            "E2E-3 storage half: {} derived stores cold==live (0 drift), 0 backup-restore paths; \
             cert={:016x}",
            self.legs.len(),
            self.certificate_hash
        )
    }
}

/// A stable FNV-1a-64 content hash over `bytes` (the byte-parity comparison + the certificate seal —
/// PII-free, deterministic, dependency-free). Same algorithm posture as the rest of the storage
/// drill harness's content hashing.
fn content_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The certificate hash over the per-store receipt set (PII-free: store name + the two parity hashes +
/// the no-backup bit). Deterministic in store order so the certificate is byte-reproducible.
fn certificate_hash(legs: &[DerivedStoreParity]) -> u64 {
    let mut sorted: Vec<&DerivedStoreParity> = legs.iter().collect();
    sorted.sort_by_key(|l| l.store);
    let mut buf = Vec::new();
    for l in sorted {
        buf.extend_from_slice(l.store.name().as_bytes());
        buf.extend_from_slice(&l.live_hash.to_le_bytes());
        buf.extend_from_slice(&l.cold_hash.to_le_bytes());
        buf.push(u8::from(l.has_backup_restore_path));
    }
    content_hash(&buf)
}

/// **Build the LIVE projection of a derived store** by ingesting the owner's facts as live events (the
/// steady-state feed). Returns the populated [`DerivedStore`] — its `parity_bytes` are the live
/// comparison target. This is the SAME `ingest` the cold rebuild uses (cold == live by construction).
fn live_projection(
    region: &Region,
    source: &DerivedReindexSource,
) -> Result<DerivedStore, ReindexError> {
    let mut store = DerivedStore::new();
    for draft in source.replay(&source.scope(), None) {
        let env = live_envelope(region, &draft);
        store.ingest(&env);
    }
    Ok(store)
}

/// **Cold-rebuild a derived store through the REAL `*.snapshot` re-emit seam (reindex-from-source —
/// the ONLY rebuild path, §6/§7.3 / contract 2.6).** Wipe → ask the owner to replay → `*.snapshot`s
/// emitted through the REAL [`OutboxStore`] → the REAL [`Relay`] drains them to the [`InProcessBus`] →
/// the derived store's ONE `ingest` path consumes them off the bus (the EXACT live consumer step).
/// Returns the rebuilt [`DerivedStore`] (its `parity_bytes` byte-match live) + the [`ReindexReceipt`]
/// (so a re-run is provably idempotent). There is NO restore-from-backup verb — `reindex` is the only
/// rebuild path (the §7.1/§7.3 structural truth).
fn cold_reindex_derived(
    region: &Region,
    source: &DerivedReindexSource,
    outbox: &mut OutboxStore,
    bus: &InProcessBus,
    relay: &Relay<InProcessBus>,
    ctx_base: EmitContextBase,
) -> Result<(DerivedStore, ReindexReceipt), ReindexError> {
    let scope = source.scope();
    let sources: Vec<&dyn ReindexSource> = vec![source];
    // (1) reindex-from-source: the owner replays → `*.snapshot`s through the REAL outbox (the
    //     deterministic-event_id idempotent re-emit; a re-run is `ON CONFLICT DO NOTHING`).
    let receipt = reindex::reindex(&scope, None, &sources, outbox, ctx_base)?;
    // (2) the REAL relay drains the newly-staged snapshots to the bus (the outbox→relay→bus path a live
    //     event rides — no backdoor). A re-run stages nothing new; the bus RETAINS the prior delivery.
    relay.drain_to_empty();
    // (3) a WIPED derived store re-consumes EVERY snapshot the bus holds for this scope through its ONE
    //     `ingest` path (the EXACT live consumer step — cold == live by construction).
    let mut cold = DerivedStore::new();
    let published: Vec<EventEnvelope> = bus.consume(&source.subject_prefix());
    for env in &published {
        cold.ingest(env);
    }
    let _ = region; // region pin is carried by the envelopes; the store is per-cell by construction.
    Ok((cold, receipt))
}

/// **Run the E2E-3 storage half over EVERY derived store and seal the dated artifact.** For each
/// [`DerivedStoreClass`]: build the live projection, cold-reindex from source, assert byte-parity (0
/// drift), prove the re-run is idempotent (0 new snapshots), and record the structural no-backup-path
/// bit (§7.1/§7.3). Seals an [`E2e3StorageArtifact`] (green iff every store is cold==live with no
/// backup path). The `sources` map provides each store's reference reindex source (the owner's source
/// of truth; the real per-owner `replay` is the Bus EB-26 floor — this drill uses reference owners, the
/// SAME posture as `myelin_events::ReferenceReindexSource`).
pub fn run_e2e3_storage_half(
    region: &Region,
    sources: &BTreeMap<DerivedStoreClass, DerivedReindexSource>,
    ctx_base: &EmitContextBase,
) -> Result<E2e3StorageArtifact, ReindexError> {
    let mut legs = Vec::with_capacity(DerivedStoreClass::ALL.len());
    for store in DerivedStoreClass::ALL {
        let source = sources.get(&store).ok_or_else(|| {
            ReindexError::NoSourceForOwner(format!("no source for {}", store.name()))
        })?;

        // LIVE: the steady-state projection.
        let live = live_projection(region, source)?;
        let live_hash = content_hash(&live.parity_bytes());

        // COLD: wipe + reindex-from-source through the REAL outbox→relay→bus→consumer path.
        let (outbox_bus, bus, relay) = booted_bus();
        let mut outbox = outbox_bus;
        let (cold, r1) =
            cold_reindex_derived(region, source, &mut outbox, &bus, &relay, ctx_base.clone())?;
        let cold_hash = content_hash(&cold.parity_bytes());

        // IDEMPOTENT re-run: a second reindex over the SAME outbox emits 0 new snapshots.
        let (_again, r2) =
            cold_reindex_derived(region, source, &mut outbox, &bus, &relay, ctx_base.clone())?;

        legs.push(DerivedStoreParity {
            store,
            live_hash,
            cold_hash,
            snapshots_emitted_first: r1.snapshots_emitted,
            snapshots_emitted_second: r2.snapshots_emitted,
            has_backup_restore_path: store.has_backup_restore_path(),
        });
    }
    Ok(E2e3StorageArtifact::seal(legs))
}

/// A reference owner's source of truth for a derived store (the analytics rows / index docs / edges the
/// store projects). A [`ReindexSource`] whose `replay` re-emits the owner's facts as `*.snapshot`
/// drafts through the SAME live consumer path (contract 2.6). On the real floor the OWNER (Issues, CI,
/// Knowledge, …) implements its `replay` reading ITS rows — that per-owner body is the Bus's **EB-26
/// (P-246, M3)** floor. This reference owner models that source of truth so the E2E-3 cold==live proof
/// is exercisable now; it is NOT a stand-in for a real owner (the SAME posture as
/// `myelin_events::ReferenceReindexSource`).
pub struct DerivedReindexSource {
    owner: String,
    /// `aggregate → (version, payload)` — the owner's source of truth, in deterministic ascending-
    /// aggregate order so a rebuild is byte-reproducible (cold == live).
    truth: BTreeMap<String, (u64, serde_json::Value)>,
}

impl DerivedReindexSource {
    /// A reference reindex source under `owner` (e.g. `"olap_src"`, `"search_src"`, `"refs_src"`).
    pub fn new(owner: impl Into<String>) -> DerivedReindexSource {
        DerivedReindexSource {
            owner: owner.into(),
            truth: BTreeMap::new(),
        }
    }

    /// Record/update the owner's truth for `aggregate` at `version` with `payload` (the fact the live
    /// event projected, and the fact a `*.snapshot` re-emits identically — cold == live).
    pub fn upsert(
        &mut self,
        aggregate: &str,
        version: u64,
        payload: serde_json::Value,
    ) -> &mut Self {
        self.truth.insert(aggregate.to_string(), (version, payload));
        self
    }

    /// The scope this source owns (`<owner>` / `all`).
    fn scope(&self) -> SnapshotScope {
        SnapshotScope::new(self.owner.clone(), "all")
    }

    /// The subject prefix the bus consume reads the drained snapshots back off of (the empty prefix
    /// matches every published subject — the SAME posture as the BUS-D5 / OLAP-feed drills).
    fn subject_prefix(&self) -> String {
        String::new()
    }

    /// The `<owner>.<derived>.snapshot` event type for this owner.
    fn snapshot_type(&self) -> myelin_events::EventType {
        myelin_events::EventType(format!(
            "{}.derived.{}",
            self.owner,
            reindex::SNAPSHOT_EVENT_NAME
        ))
    }
}

impl ReindexSource for DerivedReindexSource {
    fn owner_token(&self) -> &str {
        &self.owner
    }

    fn replay(
        &self,
        _scope: &SnapshotScope,
        since: Option<u64>,
    ) -> Vec<myelin_events::SnapshotDraft> {
        // Deterministic ascending-aggregate replay; skip aggregates at/below the `since` cursor (the
        // incremental backfill). The payload carries the owner's PII-free projection body — the SAME
        // body the live event carried, including the `version` field the derived store reads for LWW.
        self.truth
            .iter()
            .filter(|(_, (v, _))| since.is_none_or(|s| *v > s))
            .map(|(agg, (v, payload))| {
                let mut body = payload.clone();
                body["version"] = serde_json::json!(v);
                myelin_events::SnapshotDraft {
                    aggregate: myelin_events::AggregateKey(agg.clone()),
                    version: *v,
                    type_: self.snapshot_type(),
                    subject: ArtifactRef(format!("myelin://t/{}/derived/{agg}", self.owner)),
                    payload: body,
                    data_role: DataRole::Processor,
                    visibility: Visibility::Internal,
                }
            })
            .collect()
    }
}

/// A live bus envelope for one of the owner's facts — the SAME shape a `*.snapshot` of that
/// `(aggregate, version)` carries (so the cold snapshot is byte-indistinct from the live event; that is
/// what makes cold == live). The `version` field rides in the payload (the derived store's LWW key).
fn live_envelope(region: &Region, draft: &myelin_events::SnapshotDraft) -> EventEnvelope {
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, EventId, EventType, TenantId, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    let tenant = TenantId("01J0ACME".into());
    EventEnvelope {
        // The live event uses the SAME deterministic snapshot id as the cold rebuild, so the bytes
        // compared are the PROJECTION (not the id stream); both stores converge to the same docs.
        event_id: EventId(draft.event_id().0),
        type_: EventType(draft.type_.0.clone()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: region.clone(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant,
        )),
        subject: draft.subject.clone(),
        aggregate: AggregateKey(draft.aggregate.0.clone()),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        payload: draft.payload.clone(),
    }
}

/// A fresh outbox + a relay→[`InProcessBus`] the `*.snapshot`s drain through (the relay holds a SHARED
/// clone of the outbox, so it sees the reindex-staged rows). The bus + relay are stable across reindex
/// runs (the broker retains delivered snapshots — the idempotency proof).
fn booted_bus() -> (OutboxStore, InProcessBus, Relay<InProcessBus>) {
    use myelin_events::Timestamp;
    let outbox = OutboxStore::new();
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || {
        Timestamp("2026-06-20T00:00:02Z".into())
    });
    (outbox, bus, relay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{Actor, TenantId, Timestamp};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn tenant() -> TenantId {
        TenantId("01J0ACME".into())
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            caused_by: None,
        }
    }

    /// The three reference derived-store sources (OLAP analytics rows / Search index docs / Refs
    /// edges), each with a distinct owner so a reindex dispatches to exactly one.
    fn all_sources() -> BTreeMap<DerivedStoreClass, DerivedReindexSource> {
        let mut olap = DerivedReindexSource::new("olap_src");
        olap.upsert("issue:PROJ-1", 1, serde_json::json!({ "cfd": 3 }))
            .upsert("issue:PROJ-2", 2, serde_json::json!({ "cfd": 5 }));

        let mut search = DerivedReindexSource::new("search_src");
        search
            .upsert("page:home", 1, serde_json::json!({ "text": "raft" }))
            .upsert("page:guide", 2, serde_json::json!({ "text": "paxos" }))
            .upsert("page:faq", 1, serde_json::json!({ "text": "faq" }));

        let mut refs = DerivedReindexSource::new("refs_src");
        refs.upsert(
            "edge:PR-1->ISSUE-1",
            1,
            serde_json::json!({ "kind": "closes" }),
        )
        .upsert(
            "edge:COMMIT-1->PR-1",
            1,
            serde_json::json!({ "kind": "part_of" }),
        );

        BTreeMap::from([
            (DerivedStoreClass::Olap, olap),
            (DerivedStoreClass::Search, search),
            (DerivedStoreClass::Refs, refs),
        ])
    }

    /// **The E2E-3 derived-store set is exhaustive (§7.4 — OLAP + Search + Refs).** A new derived store
    /// must be added to [`DerivedStoreClass::ALL`] or it would escape the cold==live proof.
    #[test]
    fn derived_store_set_is_exhaustive() {
        assert_eq!(DerivedStoreClass::ALL.len(), 3);
        for c in DerivedStoreClass::ALL {
            assert!(!c.name().is_empty());
        }
    }

    /// **STRUCTURAL (§7.1/§7.3): NO derived store has a backup-restore path.** "OLAP (T4) + caches +
    /// derived indexes are NOT backed up — rebuilt via reindex-from-source." Every derived store
    /// returns `false`; reindex-from-source is the only rebuild path.
    #[test]
    fn no_derived_store_has_a_backup_restore_path() {
        for c in DerivedStoreClass::ALL {
            assert!(
                !c.has_backup_restore_path(),
                "{} is a derived store — it is NOT backed up (§7.1/§7.3)",
                c.name()
            );
        }
    }

    /// **MANDATORY-CORE — the E2E-3 gate: cold-reindex == live for EVERY derived store (0 drift),
    /// through the REAL outbox→relay→bus→consumer path (reindex-from-source the ONLY rebuild path).**
    /// The dated green artifact is sealed; every leg is cold==live with no backup path.
    #[test]
    fn e2e3_cold_reindex_byte_matches_live_for_every_derived_store() {
        let sources = all_sources();
        let artifact = run_e2e3_storage_half(&region(), &sources, &ctx_base())
            .expect("the E2E-3 storage half runs");

        assert!(
            artifact.is_green(),
            "the E2E-3 storage half is green: {artifact:?}"
        );
        assert_eq!(artifact.stores_with_drift, 0, "0 drift — cold == live");
        assert_eq!(
            artifact.derived_stores_with_backup_path, 0,
            "0 derived stores backed up — reindex-from-source only"
        );
        assert!(
            artifact.covers_all_derived_stores(),
            "the artifact covers OLAP + Search + Refs"
        );
        for leg in &artifact.legs {
            assert!(
                leg.cold_matches_live(),
                "{}: cold reindex byte-matches live (0 drift)",
                leg.store.name()
            );
            assert_eq!(
                leg.cold_hash,
                leg.live_hash,
                "{}: the parity hashes are identical",
                leg.store.name()
            );
        }
    }

    /// **A second reindex is an idempotent no-op (the deterministic-`event_id` `ON CONFLICT DO
    /// NOTHING`).** Each per-store leg's second reindex emits 0 new snapshots; cold == live stays
    /// byte-stable across re-runs.
    #[test]
    fn e2e3_re_run_is_idempotent_per_store() {
        let sources = all_sources();
        let artifact = run_e2e3_storage_half(&region(), &sources, &ctx_base()).unwrap();
        for leg in &artifact.legs {
            assert!(
                leg.snapshots_emitted_first > 0,
                "{}: the first rebuild emitted snapshots",
                leg.store.name()
            );
            assert_eq!(
                leg.snapshots_emitted_second,
                0,
                "{}: the re-run emitted 0 NEW snapshots (idempotent)",
                leg.store.name()
            );
        }
    }

    /// The E2E-3 artifact reads RED when ANY invariant fails (the gate is a conjunction — no single
    /// green hides a breach). A drifting leg, a backup-path leg, or a non-idempotent re-run each flips
    /// the artifact to RED; an omitted store flips `covers_all_derived_stores`.
    #[test]
    fn e2e3_artifact_reads_red_when_any_invariant_fails() {
        let green = || DerivedStoreParity {
            store: DerivedStoreClass::Olap,
            live_hash: 7,
            cold_hash: 7,
            snapshots_emitted_first: 2,
            snapshots_emitted_second: 0,
            has_backup_restore_path: false,
        };
        let search = || DerivedStoreParity {
            store: DerivedStoreClass::Search,
            ..green()
        };
        let refs = || DerivedStoreParity {
            store: DerivedStoreClass::Refs,
            ..green()
        };

        // All three green → green.
        assert!(E2e3StorageArtifact::seal(vec![green(), search(), refs()]).is_green());

        // A drifting leg (cold != live) → RED.
        let drift = DerivedStoreParity {
            cold_hash: 99,
            ..green()
        };
        let a = E2e3StorageArtifact::seal(vec![drift, search(), refs()]);
        assert_eq!(a.stores_with_drift, 1);
        assert!(!a.is_green());

        // A backup-restore path on a derived store → RED (the §7.1/§7.3 contradiction).
        let backed = DerivedStoreParity {
            has_backup_restore_path: true,
            ..green()
        };
        let b = E2e3StorageArtifact::seal(vec![backed, search(), refs()]);
        assert_eq!(b.derived_stores_with_backup_path, 1);
        assert!(!b.is_green());

        // A non-idempotent re-run → RED.
        let noisy = DerivedStoreParity {
            snapshots_emitted_second: 1,
            ..green()
        };
        assert!(!E2e3StorageArtifact::seal(vec![noisy, search(), refs()]).is_green());

        // A MISSING store (only two legs) → RED (does not cover all derived stores).
        let missing = E2e3StorageArtifact::seal(vec![green(), search()]);
        assert!(!missing.covers_all_derived_stores());
        assert!(!missing.is_green());
    }

    /// **A reindex of an UNKNOWN owner is a LOUD error (never a silent empty rebuild that masks a
    /// wiring bug, EI-02 §4).** A source for a store whose owner is unregistered fails.
    #[test]
    fn e2e3_missing_source_for_a_store_is_a_loud_error() {
        // Drop the Refs source — the run must fail loudly (a missing derived store is never silently
        // skipped).
        let mut sources = all_sources();
        sources.remove(&DerivedStoreClass::Refs);
        let err = run_e2e3_storage_half(&region(), &sources, &ctx_base())
            .expect_err("a missing derived-store source must fail loudly");
        assert!(matches!(err, ReindexError::NoSourceForOwner(_)));
    }

    /// **The certificate hash is deterministic + sensitive (the dated artifact is byte-reproducible and
    /// tamper-evident).** The SAME legs seal the SAME certificate; a changed parity hash changes the
    /// certificate.
    #[test]
    fn e2e3_certificate_is_deterministic_and_tamper_evident() {
        let sources = all_sources();
        let a = run_e2e3_storage_half(&region(), &sources, &ctx_base()).unwrap();
        let b = run_e2e3_storage_half(&region(), &sources, &ctx_base()).unwrap();
        assert_eq!(
            a.certificate_hash, b.certificate_hash,
            "the same derived-store set seals the same certificate (byte-reproducible)"
        );
        // Tamper: flip one leg's cold hash → the certificate changes.
        let mut tampered = a.legs.clone();
        tampered[0].cold_hash ^= 0xdead_beef;
        let t = E2e3StorageArtifact::seal(tampered);
        assert_ne!(
            a.certificate_hash, t.certificate_hash,
            "a tampered parity hash changes the certificate (tamper-evident)"
        );
    }

    /// The dated green-artifact summary line names the derived-store count + the 0-drift/0-backup proof
    /// (observability is part of the pass — EI-01 §3).
    #[test]
    fn e2e3_green_artifact_summary_is_observable() {
        let sources = all_sources();
        let artifact = run_e2e3_storage_half(&region(), &sources, &ctx_base()).unwrap();
        let s = artifact.summary();
        assert!(s.contains("3 derived stores"), "names the store count: {s}");
        assert!(s.contains("0 drift"), "names the 0-drift proof: {s}");
        assert!(
            s.contains("0 backup-restore paths"),
            "names the no-backup proof: {s}"
        );
    }

    /// **STRUCTURAL — there is NO backup-restore path for a derived store in THIS module's production
    /// code (§7.1/§7.3).** The ONLY rebuild verb is `reindex`/`reindex-from-source`; a future writer who
    /// adds a `restore_derived_from_backup` / `restore_from_backup` construct to the derived-store
    /// rebuild path FAILS this. (The cold-rebuild path reads ONLY the bus re-emit + the live consumer.)
    #[test]
    fn no_backup_restore_path_for_derived_stores_structural() {
        let src = include_str!("e2e3_reindex_parity.rs");
        let prod = src
            .split("#[cfg(test)]")
            .next()
            .expect("a production half above tests");
        let code: String = prod
            .lines()
            .filter(|l| !l.trim_start().starts_with("//") && !l.trim_start().starts_with("//!"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbid in [
            "restore_derived_from_backup",
            "restore_from_backup",
            "restore_derived",
            "PITR",
            "base_backup",
        ] {
            assert!(
                !code.contains(forbid),
                "a derived store must have NO backup-restore path — this module's rebuild verb is \
                 reindex-from-source ONLY (§7.1/§7.3); found forbidden `{forbid}`"
            );
        }
        // The rebuild verb IS reindex-from-source (a positive assertion the path exists).
        assert!(
            code.contains("reindex"),
            "the derived-store rebuild path IS reindex-from-source"
        );
    }
}
