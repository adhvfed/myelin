//! **Reindex-from-source: rebuild byte-parity — the recovery path, ONE code path, no backdoor**
//! (REF-P16 / P-165; contract 5.8 OWNED `reindex(scope)`, contract 2.6 CONSUMED the re-emit + replay).
//!
//! **Owning architecture doc:** `reference-graph.md` §4.7 (reindex-from-source — CONFIRMED unchanged:
//! `events::reindex(scope)` → each owner's `replay(scope, since)` emits `*.snapshot`
//! sub-artifact-granular → `refs-edge-builder` ingests idempotently → **the rebuilt edge index
//! byte-matches the live index**; ONE code path for steady-state + cold rebuild → they cannot drift;
//! on a Refs↔typed-table TE-7 drift a scoped reindex reconverges Refs to the typed table — **the typed
//! table always wins**; Refs **never reads an owner's DB**), §3.7 (R1: the `edge` projection is
//! derived/rebuildable), §3.3 (the TE-7 mirror discipline the reconvergence runs over). **Contract-index
//! rows 5.8** (`reindex(scope)`, never reads owner DBs) **+ 2.6** (the reindex re-emit + `*.snapshot`/
//! `*.erased` sub-artifact-granular). **External insight:** `04-hard-problems.md` §5.3
//! (reindex-from-source the ONLY recovery path — the index never reads owner DBs, so steady-state and
//! recovery use ONE code path and cannot drift); `01-process-and-quality-doctrine.md` §3 (prove it — the
//! byte-parity + typed-wins are drilled green, not asserted in prose). **VISION §3** (GDPR-safe: a
//! reindex re-emits an already-established fact; an ERASED aggregate is NOT re-snapshotted, so the
//! erasure stays erased across a rebuild — X-7).
//!
//! ## What REF-P16 (P-165) ships — the OWNED half of 5.8
//! Two things, and ONE code path for both:
//!
//! 1. **[`RefsReindexSource`] — Refs' [`myelin_events::ReindexSource`] body** (contract 2.6, the Refs
//!    `replay`). It owns the §6.2 `refs` owner token and replays a sub-artifact-granular scope as
//!    `refs.edge.snapshot` drafts off the owner's SOURCE OF TRUTH (the edge log) — NEVER off the
//!    derived index. At M2 the source of truth is a deterministic in-memory edge log
//!    ([`RefsReindexSource::record`]); the REAL per-blob replay (Git diffs / KN blocks re-emitting
//!    their structured nodes) lands with the producers in **R-M3 (REF-P17 Git per-blob / REF-P18 KN
//!    block-granular)** — the SEAM (the `ReindexSource` impl, the `*.snapshot` shape, the deterministic
//!    id) is real + drilled here. Each snapshot carries the SAME envelope shape a live `refs.edge.*`
//!    carries (so the consumer cannot tell cold from live), at the DETERMINISTIC
//!    [`myelin_events::snapshot_event_id`] from `(aggregate, version)` (so a re-run is an idempotent
//!    no-op).
//!
//! 2. **[`RefsReindexer::reindex`] — the `reindex(scope)` surface** (contract 5.8). It (a) drives
//!    [`myelin_events::reindex`] (the §5.6 seam) → the snapshots land in the outbox at their
//!    deterministic ids; (b) reads each emitted snapshot row and ingests it through **the SAME
//!    [`RefsEdgeBuilder::handle`]** the live consumer runs — there is **NO** "load the edge table from
//!    an owner's DB" backdoor (the no-cross-db floor: this module depends on NO other subsystem's
//!    storage; the only ingest verb is the builder's `handle`); (c) returns a [`ReindexReceipt`] with
//!    the rebuilt partition's **parity hash** (the §4.7 green artifact). **Steady-state == cold-rebuild
//!    is the SAME code path** — `handle` does not branch on cold-vs-live, because a `*.snapshot` is the
//!    same envelope shape as a live event.
//!
//! ## REF-D4 (the reindex-parity drill, CI variant) — byte-parity + typed-wins
//! - **byte-parity:** build a LIVE projection by ingesting the live edge log; WIPE a second
//!   projection; rebuild it ONLY from the reindex-from-source `*.snapshot` replay through the live
//!   consumer; assert [`EdgeProjection::parity_hash`] is IDENTICAL (the green artifact, §4.7).
//! - **typed-wins:** introduce a synthetic TE-7 drift (a spurious lifecycle edge the typed table does
//!   NOT back), then a scoped reindex reconverges Refs to the typed snapshot via
//!   [`crate::mirror::reconverge`] — the drifted edge is tombstoned, the typed table wins (§3.3/§4.7).
//!   ONE code path: the reconvergence rides the SAME reindex pass.
//!
//! ## Telemetry — `reindex_parity` (contract 1.8; observability is part of the pass)
//! [`RefsReindexer::REINDEX_PARITY_SIGNAL`] (`refs.reindex_parity`) is the §4.7 / contract-1.8 signal:
//! the parity verdict of the last reindex — `1` iff the rebuilt partition byte-matches the live
//! partition (the recovery succeeded), `0` iff it drifted (a recovery that did NOT reconverge is a LOUD
//! failed drill, never a silent partial rebuild). A drill asserts against the named constant, never a
//! literal (EI-01 §3).
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **The per-owner `replay` body is SYNTHETIC at M2.** [`RefsReindexSource`] replays an in-memory
//!   deterministic edge log (the owner's source-of-truth model). The REAL per-blob / block-granular
//!   replay over real producer content lands in **R-M3 (REF-P17 Git, REF-P18 Knowledge)**; the
//!   `ReindexSource` SEAM (the trait impl, the `*.snapshot` shape, the deterministic id, the
//!   no-cross-db discipline) is real + drilled here. Named so the CI-variant byte-parity is **not**
//!   mistaken for the at-scale proof.
//! - **The CI-variant drill (small corpus) gates this band.** The FULL-SCALE REF-D4 (reindex parity at
//!   the 30× world-scale corpus across BOTH TE-7 mirrors) is **R-M5 (REF-P24)** — registered there,
//!   linked to this CI floor. This module proves the byte-parity PROPERTY + the typed-wins
//!   reconvergence over a small corpus; it does NOT prove the at-scale throughput/latency.
//! - **The edge projection is the in-memory model** ([`EdgeProjection`]); the REAL reindex over the
//!   per-tenant-DEK-encrypted Postgres `edge` table (wipe the partition → re-drive the upserts from the
//!   replayed snapshots → byte-match the live table) is PROVEN against the live dev-stack Postgres in
//!   `tests/integration_ref_p16_reindex_parity.rs` (the `integration` feature). The seam shape (the
//!   `ReindexSource`, the deterministic id, the ONE `handle` ingest path, the no-cross-db discipline)
//!   does NOT change.
//! - **Mutation floor (mandatory-core).** The reindex decision logic — the deterministic-id replay
//!   (a re-run emits 0 new), the WIPE-then-rebuild-from-snapshots-only path (no owner-DB backdoor), the
//!   byte-parity verdict (`parity_hash` equality), and the TE-7 typed-wins reconvergence (the drifted
//!   edge is tombstoned, the typed snapshot becomes live) — is the mutation-tested core. The floor is
//!   stated + met by the unit + chained + CDC tests below: a mutant that reads the index instead of the
//!   snapshots, skips the wipe, inverts the parity verdict, lets a drifted edge survive reconvergence,
//!   or mis-derives a snapshot id is caught. The world-scale parity drill (REF-D4 full) is REF-P24.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::{
    reindex as bus_reindex, snapshot_event_id, AggregateKey, ArtifactRef, DataRole,
    EmitContextBase, EventHandler, EventType, OutboxStore, ReindexError as BusReindexError,
    ReindexSource, SnapshotDraft, SnapshotScope, Visibility,
};
use myelin_tenancy::{Region, TenantId};

use crate::edge_builder::{EdgeProjection, RefsEdgeBuilder};
use crate::mirror::{reconverge, MirrorError, SyntheticTypedEvent};

/// The §6.2 owner token Refs replays under (`refs`). [`myelin_events::reindex`] dispatches a scope to
/// the [`ReindexSource`] whose `owner_token()` matches `scope.owner`. PII-free token.
pub const REFS_OWNER_TOKEN: &str = "refs";

/// The `refs.edge.snapshot` event type a Refs replay emits — the §4.7 sub-artifact-granular snapshot.
/// It carries the SAME `created`-shaped edge payload (`source`/`target`/`rel`/`zookie`) a live
/// `refs.edge.created` carries, so [`RefsEdgeBuilder::handle`] ingests it identically (cold == live;
/// the builder's `"snapshot"` branch routes to `apply_created`). PII-free token family.
pub const REFS_EDGE_SNAPSHOT_TYPE: &str = "refs.edge.snapshot";

/// **One edge in the owner's SOURCE OF TRUTH** (the model of what a real producer's `replay` reads off
/// its own content — a Git blob's structured nodes, a KN block's inline refs). NOT the derived index:
/// this is the AUTHORITATIVE log the reindex re-emits FROM, the thing the index is a projection OF.
/// PII-free: `source`/`target` are opaque `ArtifactRef` URNs; `origin_actor` is the PSEUDONYMOUS
/// Principal ref (erasure-safe, §4.6). FLOOR: the REAL per-blob replay is REF-P17/REF-P18.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEdge {
    /// The aggregate key the snapshot re-emits under (the per-aggregate ordering key + half the
    /// deterministic id). Per-edge granularity at M2 (`refs.edge:<source>-><target>`); the real
    /// producers replay at their own sub-artifact granularity (per-blob / per-block).
    pub aggregate: String,
    /// The aggregate's version (the OTHER half of the deterministic id — a re-recorded edge at a higher
    /// version is a DISTINCT snapshot, so an updated edge re-snapshots correctly).
    pub version: u64,
    /// The referencing side (the `source` URN).
    pub source: ArtifactRef,
    /// The referenced side (the `target` URN).
    pub target: ArtifactRef,
    /// The edge relation token (`mentions`/`links`/`embeds`).
    pub rel: String,
    /// The PSEUDONYMOUS Principal ref that authored the edge (erasure-safe; never the name).
    pub origin_actor: String,
    /// The consistency token at edge-write time (§4.4).
    pub zookie: Option<String>,
}

/// **Refs' [`ReindexSource`] (contract 2.6, the Refs `replay`).** Owns the §6.2 `refs` token and an
/// in-memory edge log (the owner's source-of-truth model) keyed by aggregate for a DETERMINISTIC
/// ascending replay → byte-reproducible rebuild. `replay` reads ITS source of truth — NEVER the derived
/// `edge` index (the no-cross-db / reindex-from-source discipline, §4.7). An ERASED aggregate is dropped
/// from the log (so it is never re-snapshotted — the erasure stays erased across a reindex, X-7).
///
/// FLOOR: the real per-blob / block-granular replay over producer content is REF-P17/REF-P18; this is
/// the M2 seam the byte-parity drill runs against.
pub struct RefsReindexSource {
    /// The owner's source of truth: aggregate → the source edge. A `BTreeMap` so the replay order is
    /// deterministic (ascending aggregate) — a rebuild is byte-reproducible.
    truth: BTreeMap<String, SourceEdge>,
}

impl RefsReindexSource {
    /// A fresh, empty Refs reindex source.
    pub fn new() -> RefsReindexSource {
        RefsReindexSource {
            truth: BTreeMap::new(),
        }
    }

    /// **Record/update one edge in the owner's source of truth (the live producer write).** Keyed by
    /// `edge.aggregate` so a re-record of the same aggregate at a higher version overwrites it (the
    /// later version is what `replay` re-emits — an updated edge re-snapshots correctly).
    pub fn record(&mut self, edge: SourceEdge) {
        self.truth.insert(edge.aggregate.clone(), edge);
    }

    /// **Erase one aggregate from the source of truth (the X-7 erasure discipline).** A reindex AFTER
    /// an erasure does NOT re-snapshot the erased aggregate — the erasure stays erased across a rebuild.
    /// Returns `true` iff the aggregate was present (idempotent — erasing an absent aggregate is a
    /// no-op).
    pub fn erase(&mut self, aggregate: &str) -> bool {
        self.truth.remove(aggregate).is_some()
    }

    /// The number of edges in the source of truth.
    pub fn len(&self) -> usize {
        self.truth.len()
    }

    /// `true` iff the source of truth is empty.
    pub fn is_empty(&self) -> bool {
        self.truth.is_empty()
    }
}

impl Default for RefsReindexSource {
    fn default() -> RefsReindexSource {
        RefsReindexSource::new()
    }
}

impl ReindexSource for RefsReindexSource {
    fn owner_token(&self) -> &str {
        REFS_OWNER_TOKEN
    }

    /// Replay every edge in `scope` whose version is `> since` (the cursor) → the `refs.edge.snapshot`
    /// drafts, in DETERMINISTIC order (ascending aggregate) so a rebuild is byte-reproducible. The
    /// snapshot payload is the SAME references-not-payloads shape a live `refs.edge.created` carries
    /// (`source`/`target`/`rel`/`zookie`) — so the builder cannot tell cold from live. `since = None`
    /// replays the whole scope (the full rebuild); `since = Some(v)` is the incremental backfill.
    fn replay(&self, _scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        self.truth
            .values()
            .filter(|e| since.is_none_or(|s| e.version > s))
            .map(|e| SnapshotDraft {
                aggregate: AggregateKey(e.aggregate.clone()),
                version: e.version,
                type_: EventType(REFS_EDGE_SNAPSHOT_TYPE.into()),
                subject: e.source.clone(),
                // The SAME edge payload a live `refs.edge.created` carries (references-not-payloads),
                // PLUS the ORIGINAL `origin_actor` — so the rebuild preserves authorship provenance
                // (the reindex DRIVER's principal is the envelope actor, not the edge's author). The
                // builder prefers this payload `origin_actor` over the envelope actor on ingest, which
                // is what makes the rebuilt index byte-match the live index + keeps erasure-by-actor
                // correct after a recovery rebuild (§4.6/§4.7). The opaque pseudonymous id, never a name.
                payload: serde_json::json!({
                    "source": e.source.0,
                    "target": e.target.0,
                    "rel": e.rel,
                    "zookie": e.zookie,
                    "origin_actor": e.origin_actor,
                }),
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
            })
            .collect()
    }
}

/// The receipt a [`RefsReindexer::reindex`] run returns (the REF-D4 green artifact). It carries the
/// rebuilt partition's **parity hash** (the §4.7 "reindex-parity hash"), the snapshot counts (so a
/// re-run is PROVEN idempotent — a second run emits 0 new), and the count of edges ingested into the
/// rebuilt index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReindexReceipt {
    /// The §4.7 parity hash of the rebuilt `(tenant, region)` partition — the green artifact the drill
    /// emits (the `blake3:<hex>` content-address of the canonical edge image).
    pub parity_hash: String,
    /// `*.snapshot` rows newly emitted (NOT already at their deterministic id — the first run).
    pub snapshots_emitted: usize,
    /// `*.snapshot` rows skipped because their deterministic id was already in the outbox (a re-run's
    /// `ON CONFLICT DO NOTHING` idempotency — a re-run reports these instead of emitting).
    pub snapshots_skipped_duplicate: usize,
    /// The number of snapshot envelopes ingested through the builder's `handle` (the live consumer
    /// path) into the rebuilt index.
    pub ingested: usize,
}

/// An error from the Refs reindex surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReindexError {
    /// The Bus reindex seam failed (no owner for the scope / outbox emit failed) — a LOUD error, never
    /// a silent empty rebuild (that would mask a wiring bug, EI-02 §4).
    Bus(String),
    /// A replayed snapshot was a structurally-malformed edge (a missing `source`/`target`/`rel`) — the
    /// builder's poison surfaces here so a corrupt snapshot fails the rebuild LOUDLY, never silently
    /// corrupts the rebuilt index (fail-closed, EI-01 §5).
    Poison(String),
    /// The TE-7 reconvergence failed (an unknown lifecycle rel in the typed snapshot) — the typed table
    /// must be well-formed, a malformed typed snapshot is a LOUD rejection (REF-3).
    Mirror(String),
}

impl std::fmt::Display for ReindexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReindexError::Bus(e) => write!(f, "refs reindex: bus seam failed: {e}"),
            ReindexError::Poison(e) => write!(f, "refs reindex: poison snapshot: {e}"),
            ReindexError::Mirror(e) => write!(f, "refs reindex: typed reconvergence failed: {e}"),
        }
    }
}

impl std::error::Error for ReindexError {}

impl From<BusReindexError> for ReindexError {
    fn from(e: BusReindexError) -> ReindexError {
        ReindexError::Bus(e.to_string())
    }
}

impl From<MirrorError> for ReindexError {
    fn from(e: MirrorError) -> ReindexError {
        match e {
            MirrorError::UnknownRel(r) => {
                ReindexError::Mirror(format!("unknown lifecycle rel `{r}`"))
            }
        }
    }
}

/// **The Refs `reindex(scope)` surface (contract 5.8 OWNED).** Drives the §4.7 recovery path: the Bus
/// re-emit (contract 2.6) → the snapshots through the SAME [`RefsEdgeBuilder::handle`] ingest path → the
/// rebuilt index, with its parity verdict on the `reindex_parity` telemetry (contract 1.8). ONE code
/// path, no owner-DB backdoor.
#[derive(Clone)]
pub struct RefsReindexer {
    /// The builder whose `handle` IS the one ingest path (steady-state == cold-rebuild). The reindexer
    /// does NOT re-implement ingestion — it re-drives the builder over the replayed snapshots.
    builder: RefsEdgeBuilder,
    /// The live `refs.reindex_parity` verdict (contract 1.8): `1` iff the last reindex byte-matched the
    /// live partition, `0` iff it drifted. Starts at `1` (a fresh reindexer that has not drifted).
    reindex_parity: Arc<AtomicU64>,
}

impl RefsReindexer {
    /// The telemetry signal name this reindexer emits (contract 1.8, §4.7 "reindex_parity"). A named
    /// constant — drills assert against the NAME, never a literal (EI-01 §3 observability).
    pub const REINDEX_PARITY_SIGNAL: &'static str = "refs.reindex_parity";

    /// Build a reindexer over `builder` (the ONE ingest path it re-drives).
    pub fn new(builder: RefsEdgeBuilder) -> RefsReindexer {
        RefsReindexer {
            builder,
            reindex_parity: Arc::new(AtomicU64::new(1)),
        }
    }

    /// The builder this reindexer re-drives (read access for the parity comparison / tests).
    pub fn builder(&self) -> &RefsEdgeBuilder {
        &self.builder
    }

    /// The edge projection the rebuild lands in (the rebuilt index).
    pub fn projection(&self) -> &EdgeProjection {
        self.builder.projection()
    }

    /// The live `refs.reindex_parity` sample (contract 1.8): the parity verdict of the last reindex —
    /// `1` iff the rebuilt partition byte-matched the live partition, `0` iff it drifted.
    pub fn reindex_parity(&self) -> u64 {
        self.reindex_parity.load(Ordering::SeqCst)
    }

    /// **`reindex(scope)` — wipe the partition, rebuild ONLY from the replayed snapshots (§4.7).**
    /// (1) Drive [`myelin_events::reindex`] → the owner's `replay` snapshots land in `outbox` at their
    /// deterministic ids (a re-run emits 0 new). (2) WIPE the rebuilt partition (the cold-rebuild
    /// precondition — there is no owner-DB reload). (3) Ingest each emitted snapshot through the SAME
    /// [`RefsEdgeBuilder::handle`] the live consumer runs (no backdoor) — `handle` does NOT branch on
    /// cold-vs-live. (4) Return the rebuilt partition's parity hash. The `reindex_parity` telemetry is
    /// set by [`verify_parity`] against the live partition — call it after a rebuild.
    ///
    /// `source` is the owner's source of truth (Refs' [`RefsReindexSource`], or a real producer's
    /// `ReindexSource` at REF-P17/P18). The partition the rebuild lands in is `ctx_base.tenant` /
    /// `ctx_base.region` — the SAME `(tenant, region)` the snapshots emit under, so there is no
    /// redundant partition arg to drift out of sync. `since` is the cursor: `None` = full rebuild
    /// (recovery), `Some(v)` = incremental backfill.
    pub fn reindex(
        &self,
        scope: &SnapshotScope,
        since: Option<u64>,
        source: &dyn ReindexSource,
        outbox: &mut OutboxStore,
        ctx_base: EmitContextBase,
    ) -> Result<ReindexReceipt, ReindexError> {
        // The partition the rebuild lands in IS the snapshots' emit partition (no redundant arg).
        let tenant = &ctx_base.tenant;
        let region = &ctx_base.region;
        // (1) The Bus re-emit (contract 2.6): the owner replays the scope → `*.snapshot` drafts land in
        // the outbox at their DETERMINISTIC ids (a re-run is an `ON CONFLICT DO NOTHING` no-op).
        let sources: &[&dyn ReindexSource] = &[source];
        let bus_receipt = bus_reindex(scope, since, sources, outbox, ctx_base.clone())?;

        // (2) WIPE the rebuilt partition — the cold-rebuild precondition. A reindex-from-source REBUILDS
        // from the replayed log, it does NOT reconcile-in-place against an owner DB (no backdoor, §4.7).
        // (On `since = Some(_)` incremental backfill we do NOT wipe — the backfill EXTENDS the index;
        // the full-rebuild recovery path is `since = None`.)
        if since.is_none() {
            self.projection().wipe_partition(tenant, region);
        }

        // (3) Ingest each emitted snapshot through the SAME live-consumer `handle` (cold == live, no
        // backdoor). The drafts re-derive deterministically (the source replays the same order), so we
        // re-drive `replay` to read each snapshot's outbox row by its deterministic id — exactly the
        // path the relay would drive to the consumer.
        let drafts = source.replay(scope, since);
        let mut ingested = 0usize;
        for draft in &drafts {
            let id = snapshot_event_id(&draft.aggregate, draft.version);
            let row = outbox.row(&id).ok_or_else(|| {
                ReindexError::Bus(format!("snapshot row {} absent after emit", id.0))
            })?;
            // The SAME `handle` the live consumer runs — `handle` routes the `.snapshot` type to
            // `apply_created` (cold == live). A poison snapshot surfaces LOUDLY (fail-closed).
            match self.builder.handle(&row.envelope, &mut myelin_events::HandlerTx::none()) {
                myelin_events::HandleOutcome::Done => ingested += 1,
                myelin_events::HandleOutcome::NonRetryable(myelin_events::Reason(r)) => {
                    return Err(ReindexError::Poison(r));
                }
                // The builder never returns `Retry` on a snapshot ingest (a malformed edge is a
                // NonRetryable poison, a well-formed one is Done). A `Retry` here would be an
                // unexpected transient — surface it LOUDLY rather than silently dropping the snapshot.
                myelin_events::HandleOutcome::Retry(_) => {
                    return Err(ReindexError::Poison(format!(
                        "unexpected retryable outcome ingesting snapshot {}",
                        id.0
                    )));
                }
            }
        }

        let parity_hash = self.projection().parity_hash(tenant, region);
        Ok(ReindexReceipt {
            parity_hash,
            snapshots_emitted: bus_receipt.snapshots_emitted,
            snapshots_skipped_duplicate: bus_receipt.snapshots_skipped_duplicate,
            ingested,
        })
    }

    /// **The TE-7 drift reconvergence over a scoped reindex — the typed table always wins (§3.3/§4.7).**
    /// Given the AUTHORITATIVE typed snapshot for a scope (the lifecycle events a reindex re-emits for
    /// it) + the `target_root`s it covers, reconverge the rebuilt projection via
    /// [`crate::mirror::reconverge`]: the typed snapshot's edges (forward + inverse) become live; any
    /// lifecycle edge inbound to a covered root the snapshot does NOT back is tombstoned (drift → the
    /// typed table wins). `reference`-class edges are untouched. Returns `(reprojected-pairs,
    /// tombstoned-drift)`. Rides the SAME reindex pass — ONE code path. Tenant-first.
    pub fn reconverge_typed(
        &self,
        tenant: &TenantId,
        region: &Region,
        typed_snapshot: &[SyntheticTypedEvent],
        covered_roots: &[ArtifactRef],
        reindex_event_id: &str,
    ) -> Result<(usize, usize), ReindexError> {
        Ok(reconverge(
            self.projection(),
            tenant,
            region,
            typed_snapshot,
            covered_roots,
            reindex_event_id,
        )?)
    }

    /// **Verify byte-parity of the rebuilt partition against a `live` reference projection — and set the
    /// `reindex_parity` telemetry (§4.7).** Returns `true` iff the two partitions' canonical byte-images
    /// (and thus their parity hashes) are IDENTICAL (the recovery succeeded). Sets the
    /// `reindex_parity` signal to `1` on match, `0` on drift (a failed recovery is LOUD + observable,
    /// never a silent partial rebuild). The comparison is the §4.7 "the rebuilt index byte-matches the
    /// live index" equality.
    pub fn verify_parity(&self, live: &EdgeProjection, tenant: &TenantId, region: &Region) -> bool {
        let rebuilt = self.projection().parity_hash(tenant, region);
        let reference = live.parity_hash(tenant, region);
        let matched = rebuilt == reference;
        self.reindex_parity
            .store(u64::from(matched), Ordering::SeqCst);
        matched
    }
}

#[cfg(test)]
mod tests;
