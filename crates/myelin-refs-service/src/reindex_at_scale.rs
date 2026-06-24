//! # `reindex_at_scale` — world-scale reindex-parity across both TE-7 mirrors (REF-P24 / P-455, M5)
//!
//! **The full-scale REF-D4.** This module promotes the REF-P16 **CI-variant** reindex-parity drill
//! (`reindex.rs` / `cdc_5_8_reindex.rs` — a 3-edge corpus) to its **full-scale** form: wipe the edge
//! index, `reindex` it ONLY from the reindex-from-source replay, and prove the rebuilt index
//! **byte-matches** the live index across the **FULL five-producer corpus** (Git, Knowledge, CI, Chat,
//! Issues) **INCLUDING BOTH TE-7 lifecycle mirrors** (Knowledge `page_parent` + Issues
//! `issue_relation`). The `reindex_parity` telemetry (contract 1.8) fires `1` iff the rebuild
//! byte-matched.
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §4.7 (reindex-from-source — `events::reindex(scope)` → each owner's `replay` emits `*.snapshot`
//! sub-artifact-granular → the builder ingests idempotently → the rebuilt index byte-matches the live
//! index; ONE code path for steady-state + cold rebuild → they cannot drift; on a Refs↔typed-table
//! TE-7 drift a scoped reindex reconverges Refs to the typed table — the typed table always wins),
//! §7 **D-4 the scale variant** (the at-scale form of the reindex-parity drill). **Contract-index
//! rows 5.8** (`reindex(scope)` at scale, never reads owner DBs) **+ 1.8** (`reindex_parity`
//! telemetry). **External insight:** `04-hard-problems.md` §5.3 (reindex-from-source the ONLY recovery
//! path — steady-state and recovery use ONE code path and cannot drift);
//! `01-process-and-quality-doctrine.md` §3 (prove it under scale — the byte-parity is DRILLED green
//! across the real producer corpus, not asserted in prose; never weaken a threshold to pass). **VISION
//! §3** (GDPR-safe, world-scale: a reindex re-emits an already-established fact; an ERASED aggregate is
//! NOT re-snapshotted, so the erasure stays erased across a rebuild — X-7).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! This is the **at-scale DRILL HARNESS over the EXISTING reindex engine**, not a second reindexer.
//! The byte-parity rebuild rides the SAME [`crate::RefsReindexer::reindex`] (the reference-class
//! `*.snapshot` replay through the one [`crate::RefsEdgeBuilder::handle`] ingest path, no owner-DB
//! backdoor) and the SAME [`crate::RefsReindexer::reconverge_typed`] (the TE-7 typed-wins
//! reconvergence) that REF-P16 froze. No new mutation-core module is introduced — the reindex decision
//! logic is fixed; this module SCALES the corpus the existing engine runs over and proves the property
//! holds across all five producers + both mirrors at once. The corpus is built from the SAME
//! [`crate::SourceEdge`] (reference edges, the owner source-of-truth model) and
//! [`crate::SyntheticTypedEvent`] (the typed lifecycle snapshots) the CI variant uses.
//!
//! ## The two TE-7 mirrors this drill covers (both, at once)
//! The §3.3 TE-7 discipline has two real lifecycle mirrors, each over a real typed table:
//! - **Knowledge `page_parent`** ([`crate::mirror_page_parent`], REF-P18) → [`crate::LifecycleRel::Parent`]
//!   + the inverse `child`. A page-tree's `page_parent` rows.
//! - **Issues `issue_relation`** ([`crate::reconverge_issue_relations`], REF-P20) →
//!   [`crate::LifecycleRel::Blocks`]/`blocked_by` + `relates`. An issue dependency graph's
//!   `issue_relation` rows.
//!
//! A full-scale rebuild must reconverge BOTH: after the reference-edge replay, the reindex re-emits
//! each mirror's typed snapshot and reconverges Refs to it. The drill asserts the rebuilt index
//! byte-matches live with BOTH mirrors present — a rebuild that drops, drifts, or fails to reconverge
//! EITHER mirror flips the parity hash and fails LOUDLY (never a silent partial rebuild).
//!
//! ## The corpus is SCALED + DETERMINISTIC (the property is proven; the fleet load is the floor)
//! [`build_full_scale_corpus`] generates a [`FiveProducerCorpus`] sized by a `scale` factor across all
//! five producers + both mirrors. The corpus is DETERMINISTIC (the same scale yields the same URNs in
//! the same order), so the live build and the cold rebuild see byte-identical inputs — that is the
//! cold==live invariant the parity hash captures. The default drill scale produces a corpus large
//! enough to exercise every producer namespace + both mirror vocabularies; the world-scale corpus
//! cardinality is bounded ONLY by the named fleet-hardware floor below.
//!
//! ## Telemetry — `reindex_parity` (contract 1.8)
//! The drill drives the existing [`crate::RefsReindexer::verify_parity`], which sets the
//! [`crate::RefsReindexer::REINDEX_PARITY_SIGNAL`] (`refs.reindex_parity`) to `1` on a byte-match, `0`
//! on drift. A full-scale drill asserts against the named constant + the `1` verdict — the dated green
//! artifact the DoD names is the matching `parity_hash` reported by [`FullScaleParityReport`].
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet
//!   hardware) — [`WORLD_SCALE_FLEET_LOAD_FLOOR`]. This module proves the byte-parity PROPERTY across
//!   the full five-producer corpus + both TE-7 mirrors with a deterministic scaled corpus; it does NOT
//!   carry the real PgStore-backed edge index over fleet hardware at the full 30× cardinality. The
//!   reindex SEAM (the `ReindexSource` replay, the deterministic snapshot id, the ONE `handle` ingest
//!   path, the no-cross-db discipline, the typed-wins reconvergence) does NOT change shape when the
//!   fleet carries it — the property is byte-parity-identical at any scale, which is exactly why the
//!   hash is a content-address.
//! - **The real per-blob / per-block producer replay** (Git diffs / KN blocks re-emitting their
//!   structured nodes over real content) is REF-P17/REF-P18; here each producer's source-of-truth is
//!   the [`crate::SourceEdge`] model the CI-variant drill froze. The seam is real + drilled; the
//!   per-blob body is the named floor inherited from `reindex.rs`.
//! - **The real Postgres edge partition** (wipe the per-tenant partition → re-drive the upserts from
//!   the replayed snapshots → byte-match the live table) is the dev-stack integration proof in
//!   `tests/integration_ref_p16_reindex_parity.rs`; this module proves the at-scale property over the
//!   in-memory [`crate::EdgeProjection`] (the §3.2 `edge` table's semantics). The byte-image the parity
//!   hash is taken over is the SAME canonical edge image either backend produces.
//!
//! ## Mutation floor — inherited, NOT re-authored (EI-01 §2/§7)
//! The reindex mutation-core (the deterministic-id replay, the WIPE-then-rebuild-from-snapshots-only
//! path, the byte-parity verdict, the typed-wins reconvergence) is the mutation-tested core in
//! `reindex.rs` / `mirror.rs` and STILL HOLDS at scale — this module adds NO new decision logic to
//! mutate, it scales the corpus the frozen engine runs over. The drill's own counter-case (a dropped
//! mirror, a skipped wipe, a drifted edge) flips the parity hash, proving the at-scale green is earned.

use myelin_events::{EmitContextBase, EventHandler, OutboxStore, SnapshotScope};
use myelin_tenancy::{Region, TenantId};

use crate::edge_builder::{EdgeProjection, RefsEdgeBuilder};
use crate::mirror::{project_typed_event, LifecycleRel, SyntheticTypedEvent};
use crate::reindex::{
    RefsReindexSource, RefsReindexer, ReindexError, SourceEdge, REFS_OWNER_TOKEN,
};
use myelin_refs::ArtifactRef;

/// **FLOOR (the ONE legitimate remaining floor): the 30× world-scale load on REAL FLEET HARDWARE.**
/// This module proves the reindex-parity PROPERTY (rebuilt == live, byte-match) across the full
/// five-producer corpus + both TE-7 mirrors with a DETERMINISTIC scaled corpus over the in-memory edge
/// projection (the §3.2 `edge` table's semantics). The real 30× cardinality carried by the
/// PgStore-backed edge index over real fleet hardware is the named floor — it does NOT change the seam
/// or the property (the parity hash is a content-address: identical bytes ⇒ identical hash at any
/// scale). EI-01 §3: name the floor; never claim a green you did not earn.
pub const WORLD_SCALE_FLEET_LOAD_FLOOR: &str =
    "REF-D4 at full 30x world-scale cardinality over the PgStore-backed edge index on real fleet \
     hardware (the ONE legitimate remaining floor); the byte-parity property + both-mirror \
     reconvergence are proven here over a deterministic scaled corpus";

/// The five real reference producers a full-scale corpus spans (§4.7 — the owners a `reindex` asks to
/// re-emit). Each is a distinct URN namespace in the corpus so the rebuilt index covers every producer
/// surface at once. PII-free tokens.
pub const FIVE_PRODUCERS: [&str; 5] = ["git", "knowledge", "ci", "chat", "issue"];

/// **A full-scale five-producer corpus (REF-D4 at scale).** The reference-class edge log (all five
/// producers) the reindex re-emits FROM, plus the two TE-7 typed-lifecycle snapshots (KN `page_parent`
/// and Issues `issue_relation`) the reindex reconverges to. Deterministic for a given scale (same
/// URNs, same order) — so the live build and the cold rebuild see byte-identical inputs (the
/// cold==live invariant). PII-free: every URN is opaque; every `origin_actor` is a pseudonymous ref.
#[derive(Clone, Debug)]
pub struct FiveProducerCorpus {
    /// The reference-class source-of-truth edge log across all five producers (the thing the index is a
    /// projection OF — re-emitted as `refs.edge.snapshot` on a rebuild).
    pub reference_edges: Vec<SourceEdge>,
    /// The Knowledge `page_parent` typed-lifecycle snapshot (the FIRST TE-7 mirror — `parent`/`child`).
    pub page_parent_snapshot: Vec<SyntheticTypedEvent>,
    /// The `target_root`s the page_parent snapshot is authoritative over (the reconvergence scope).
    pub page_parent_roots: Vec<ArtifactRef>,
    /// The Issues `issue_relation` typed-lifecycle snapshot (the SECOND TE-7 mirror — `blocks`/`relates`).
    pub issue_relation_snapshot: Vec<SyntheticTypedEvent>,
    /// The `target_root`s the issue_relation snapshot is authoritative over (the reconvergence scope).
    pub issue_relation_roots: Vec<ArtifactRef>,
}

impl FiveProducerCorpus {
    /// The total reference-class edge count (the steady-state edges all five producers minted).
    pub fn reference_count(&self) -> usize {
        self.reference_edges.len()
    }

    /// The total typed-lifecycle event count across BOTH mirrors (the lifecycle edges the rebuild must
    /// reconverge — `page_parent` + `issue_relation`).
    pub fn mirror_event_count(&self) -> usize {
        self.page_parent_snapshot.len() + self.issue_relation_snapshot.len()
    }
}

/// **Build a deterministic full-scale five-producer corpus (REF-D4 at scale).** For each producer in
/// [`FIVE_PRODUCERS`] mint `scale` reference edges (opaque per-producer URNs, the three reference rels
/// round-robined). Then mint `scale` `page_parent` lifecycle events (a KN page tree) + `scale`
/// `issue_relation` lifecycle events (an issue dependency graph) — BOTH TE-7 mirrors. Same `scale` ⇒
/// same corpus (byte-reproducible). `scale` must be `> 0` (a non-empty corpus exercises every surface).
pub fn build_full_scale_corpus(tenant: &str, scale: usize) -> FiveProducerCorpus {
    assert!(scale > 0, "the full-scale corpus must be non-empty");
    let reference_rels = ["mentions", "links", "embeds"];

    // --- The reference-class edge log across all five producers (the source of truth). ---
    let mut reference_edges = Vec::with_capacity(FIVE_PRODUCERS.len() * scale);
    for (p_idx, producer) in FIVE_PRODUCERS.iter().enumerate() {
        for i in 0..scale {
            let rel = reference_rels[(p_idx + i) % reference_rels.len()];
            let source = format!("myelin://{tenant}/{producer}/artifact/{producer}-src-{i}");
            let target = format!("myelin://{tenant}/{producer}/artifact/{producer}-tgt-{i}");
            reference_edges.push(SourceEdge {
                // A globally-unique per-producer aggregate so the deterministic snapshot ids never
                // collide across producers (the rebuild re-emits each exactly once).
                aggregate: format!("refs.edge:{producer}:{i}"),
                version: 1,
                source: ArtifactRef(source),
                target: ArtifactRef(target),
                rel: rel.into(),
                origin_actor: format!("p-opaque-{producer}-{}", i % 7),
                zookie: Some(format!("zk-{producer}-{i}")),
            });
        }
    }

    // --- TE-7 mirror 1: Knowledge `page_parent` (parent/child) — a deterministic page tree. ---
    let mut page_parent_snapshot = Vec::with_capacity(scale);
    let mut page_parent_roots = Vec::with_capacity(scale);
    for i in 0..scale {
        let parent = ArtifactRef(format!("myelin://{tenant}/knowledge/page/page-{}", i / 4));
        let child = ArtifactRef(format!("myelin://{tenant}/knowledge/page/page-{i}"));
        page_parent_snapshot.push(SyntheticTypedEvent {
            source: parent.clone(),
            target: child.clone(),
            rel: LifecycleRel::Parent,
            origin_event: format!("page_parent-{i}"),
            origin_actor: format!("p-opaque-knowledge-{}", i % 7),
            zookie: None,
        });
        // The reconvergence is authoritative over the child root (the `parent` edge's target_root) AND
        // the inverse `child` edge's target_root (the parent). Both must be covered so the rebuild's
        // tombstone-of-drift pass is scoped to exactly the tree this snapshot backs.
        page_parent_roots.push(strip_sub(&child));
        page_parent_roots.push(strip_sub(&parent));
    }

    // --- TE-7 mirror 2: Issues `issue_relation` (blocks/relates) — a deterministic dependency graph. ---
    let mut issue_relation_snapshot = Vec::with_capacity(scale);
    let mut issue_relation_roots = Vec::with_capacity(scale);
    for i in 0..scale {
        // Alternate blocks / relates so BOTH inverse shapes (paired + symmetric) are exercised.
        let rel = if i % 2 == 0 {
            LifecycleRel::Blocks
        } else {
            LifecycleRel::Relates
        };
        let source = ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-{i}"));
        let target = ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-{}", i + scale));
        issue_relation_snapshot.push(SyntheticTypedEvent {
            source: source.clone(),
            target: target.clone(),
            rel,
            origin_event: format!("issue_relation-{i}"),
            origin_actor: format!("p-opaque-issue-{}", i % 7),
            zookie: None,
        });
        issue_relation_roots.push(strip_sub(&target));
        issue_relation_roots.push(strip_sub(&source));
    }

    FiveProducerCorpus {
        reference_edges,
        page_parent_snapshot,
        page_parent_roots,
        issue_relation_snapshot,
        issue_relation_roots,
    }
}

/// Strip a `#sub` anchor from a URN to its index `*_root` key (the SAME key the projection indexes on —
/// the reconvergence's `covered_roots` are roots, never sub-anchored refs). No `#` ⇒ the URN is its own
/// root.
fn strip_sub(r: &ArtifactRef) -> ArtifactRef {
    match r.0.split_once('#') {
        Some((root, _)) => ArtifactRef(root.to_string()),
        None => r.clone(),
    }
}

/// The dated green artifact a full-scale REF-D4 run emits (the §4.7 reindex-parity hash + the corpus
/// shape it covers). `parity_matched` is the byte-parity verdict; `parity_hash` is the content-address
/// the live and rebuilt partitions agree on (identical ⇒ the recovery succeeded).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullScaleParityReport {
    /// `true` iff the rebuilt partition byte-matched the live partition across the FULL corpus + both
    /// mirrors (the green verdict).
    pub parity_matched: bool,
    /// The §4.7 reindex-parity hash both partitions agree on (the dated green artifact).
    pub parity_hash: String,
    /// The reference-class edges in the corpus (all five producers).
    pub reference_edges: usize,
    /// The typed-lifecycle events across BOTH mirrors (page_parent + issue_relation).
    pub mirror_events: usize,
    /// The total edges ingested into the rebuilt index through the live consumer path.
    pub reference_ingested: usize,
    /// The lifecycle edge pairs re-projected on the page_parent mirror reconvergence.
    pub page_parent_reprojected: usize,
    /// The lifecycle edge pairs re-projected on the issue_relation mirror reconvergence.
    pub issue_relation_reprojected: usize,
    /// The `refs.reindex_parity` telemetry sample after the rebuild (`1` on match, `0` on drift).
    pub reindex_parity_signal: u64,
}

impl FullScaleParityReport {
    /// The full-scale REF-D4 green predicate: the rebuild byte-matched live AND the telemetry fired `1`
    /// AND both mirrors reconverged at least their corpus cardinality. A drill asserts this — never a
    /// silent partial rebuild.
    pub fn is_ref_d4_full_scale_green(&self) -> bool {
        self.parity_matched
            && self.reindex_parity_signal == 1
            && self.page_parent_reprojected > 0
            && self.issue_relation_reprojected > 0
    }

    /// A one-line human summary (the dated artifact's body).
    pub fn summary(&self) -> String {
        format!(
            "REF-D4 full-scale: rebuilt==live={} parity={} refs={} (ingested {}) mirrors={} \
             (page_parent reproj {}, issue_relation reproj {}) reindex_parity={}",
            self.parity_matched,
            self.parity_hash,
            self.reference_edges,
            self.reference_ingested,
            self.mirror_events,
            self.page_parent_reprojected,
            self.issue_relation_reprojected,
            self.reindex_parity_signal,
        )
    }
}

/// **THE full-scale REF-D4 drill (REF-P24): wipe the edge index, reindex, byte-match live across the
/// full five-producer corpus + BOTH TE-7 mirrors.** Rides the EXISTING reindex engine (no second
/// reindexer):
/// 1. Build the LIVE index — ingest every reference edge through [`RefsEdgeBuilder::handle`] (the live
///    consumer path) and project BOTH mirror snapshots through [`project_typed_event`] (the §3.3 first
///    + second mirror). This is steady-state.
/// 2. WIPE + reindex the reference edges ONLY from the reindex-from-source `*.snapshot` replay through
///    the SAME `handle` (no owner-DB backdoor), via [`RefsReindexer::reindex`].
/// 3. Reconverge BOTH mirrors to their typed snapshots via [`RefsReindexer::reconverge_typed`] — the
///    typed table always wins; a rebuild that drops a mirror flips the hash.
/// 4. [`RefsReindexer::verify_parity`] the rebuilt partition against the live partition (sets the
///    `reindex_parity` telemetry) — return the [`FullScaleParityReport`] (the dated green artifact).
///
/// Returns a [`ReindexError`] if the reindex seam, a poison snapshot, or a malformed typed snapshot
/// fails LOUDLY (never a silent partial rebuild).
pub fn run_full_scale_reindex_parity(
    tenant: &TenantId,
    region: &Region,
    corpus: &FiveProducerCorpus,
    ctx_base: EmitContextBase,
) -> Result<FullScaleParityReport, ReindexError> {
    // --- (1) The LIVE index (steady-state): reference edges + both mirrors. ---
    let live = RefsEdgeBuilder::new(EdgeProjection::new());
    let mut truth = RefsReindexSource::new();
    for edge in &corpus.reference_edges {
        truth.record(edge.clone());
        // Ingest the live reference event — the SAME payload shape the snapshot replay carries (that IS
        // the cold==live invariant), driven through the live `handle`.
        live.handle(&live_reference_event(tenant, region, edge, &ctx_base));
    }
    let live_proj = live.projection();
    // Project BOTH TE-7 mirrors into the live index (the first + second real mirror).
    for ev in &corpus.page_parent_snapshot {
        project_typed_event(live_proj, tenant, region, ev)?;
    }
    for ev in &corpus.issue_relation_snapshot {
        project_typed_event(live_proj, tenant, region, ev)?;
    }
    let live_snapshot = live_proj.clone();

    // --- (2) WIPE + reindex the reference edges ONLY from the replayed snapshots (no backdoor). ---
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let scope = SnapshotScope::new(REFS_OWNER_TOKEN, "edge:all");
    let mut outbox = OutboxStore::new();
    let receipt = reindexer.reindex(&scope, None, &truth, &mut outbox, ctx_base)?;

    // --- (3) Reconverge BOTH TE-7 mirrors to their typed snapshots (typed wins, both at once). ---
    let (page_parent_reprojected, _) = reindexer.reconverge_typed(
        tenant,
        region,
        &corpus.page_parent_snapshot,
        &corpus.page_parent_roots,
        "reindex-full-scale-page-parent",
    )?;
    let (issue_relation_reprojected, _) = reindexer.reconverge_typed(
        tenant,
        region,
        &corpus.issue_relation_snapshot,
        &corpus.issue_relation_roots,
        "reindex-full-scale-issue-relation",
    )?;

    // --- (4) Byte-parity verdict + telemetry (the §4.7 green artifact). ---
    let parity_matched = reindexer.verify_parity(&live_snapshot, tenant, region);
    let parity_hash = reindexer.projection().parity_hash(tenant, region);

    Ok(FullScaleParityReport {
        parity_matched,
        parity_hash,
        reference_edges: corpus.reference_count(),
        mirror_events: corpus.mirror_event_count(),
        reference_ingested: receipt.ingested,
        page_parent_reprojected,
        issue_relation_reprojected,
        reindex_parity_signal: reindexer.reindex_parity(),
    })
}

/// Build the live `refs.edge.created` envelope for a corpus reference edge — the SAME payload the
/// snapshot replay carries (that IS the cold==live invariant the parity hash captures), driven through
/// the live consumer `handle`.
fn live_reference_event(
    tenant: &TenantId,
    region: &Region,
    edge: &SourceEdge,
    ctx_base: &EmitContextBase,
) -> myelin_events::EventEnvelope {
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    myelin_events::EventEnvelope {
        event_id: EventId(format!("live-{}-{}", edge.aggregate, edge.version)),
        type_: EventType("refs.edge.created".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: region.clone(),
        actor: Actor(Principal::stub(
            PrincipalId(edge.origin_actor.clone()),
            PrincipalKind::Human,
            tenant.clone(),
        )),
        subject: edge.source.clone(),
        aggregate: AggregateKey(edge.aggregate.clone()),
        causation_id: None,
        correlation_id: CorrelationId(format!("live-{}", edge.aggregate)),
        caused_by: None,
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: ctx_base.occurred_at.clone(),
        recorded_at: ctx_base.recorded_at.clone(),
        payload: serde_json::json!({
            "source": edge.source.0,
            "target": edge.target.0,
            "rel": edge.rel,
            "zookie": edge.zookie,
            "origin_actor": edge.origin_actor,
        }),
    }
}

#[cfg(test)]
mod tests;
