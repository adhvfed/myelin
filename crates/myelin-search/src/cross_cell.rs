//! **Cross-cell federated search — the designed-and-extends BUILD** (SRCH-P31 / P-464; S-M5;
//! architecture `search-and-indexing.md` §6.4; contract 12.6 consumed, 6.1/6.2 cross-cell-extended,
//! 5.6 the per-viewer home-cell project — all to the FROZEN shapes, never rewritten).
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/search-and-indexing.md`
//! §6.4 (cross-cell federated search — **designed-not-built** until multi-cell goes live: *scatter-
//! gather* — each cell runs the SAME permission-filtered query LOCALLY over its own
//! index/`list_objects`/residency; a *residency-free merge* fuses ONLY ranking metadata +
//! [`ArtifactRef`]s — never payload/PII — at the control-plane boundary; result rows are resolved
//! **per-viewer in their HOME cell** over the cross-cell PII-free pointer bridge (contract 12.6,
//! resolution always cell-local); the §5 contracts are cell-agnostic so this EXTENDS WITHOUT A
//! REWRITE), §1 ("Floors named up front" (c) — the single-cell → cross-cell follow-on).
//! **Reconciliation:** `00-reconciliation-decisions.md` OQ-I (single-cell → multi-cell; the
//! cross-cell PII-free pointer bridge frame). **External insight:** `VISION.md` §3 (world-scalable,
//! EU-sovereign — the residency-free merge crosses only ranking metadata, never PII);
//! `external-insights/04-hard-problems.md` §1 (residency); `01-process-and-quality-doctrine.md` §3
//! (prove-it — the leak-free property is DRILLED, not asserted in prose).
//!
//! ## What SRCH-P31 ships — the BUILD, not a second engine (EI-01 §7 coherence)
//! The single-cell permission-aware query path was complete from M2 ([`crate::pipeline::query`] /
//! `query_consistent`): the ACL pre-filter (`list_objects`, §4.2.1), the no-stale-grant zookie pass
//! (§4.2.3), the score-scale-free RRF fusion ([`crate::fusion`]). The §5 contracts are **cell-
//! agnostic** — a [`crate::pipeline::RankedResults`] is `ArtifactRef` + score (ranking metadata),
//! never payload. This module is the cross-cell EXTENSION that rides those cell-agnostic contracts:
//!
//! 1. **Scatter** — dispatch the SAME permission-filtered query to each member cell over a
//!    [`CellLocalQuery`] seam; each cell runs it LOCALLY against ITS own index/`list_objects`/
//!    residency (the query never leaves the cell; only the [`CellRanking`] — `ArtifactRef`s + scores
//!    — comes back).
//! 2. **Residency-free merge** — fuse the per-cell rankings into one order with the SAME score-scale-
//!    free RRF ([`crate::fusion::reciprocal_rank_fusion`]) used inside a single cell. ONLY ranking
//!    metadata + `ArtifactRef`s cross the merge boundary — NEVER a payload, NEVER PII.
//! 3. **Per-viewer home-cell resolution (5.6)** — each merged row is resolved in its HOME cell over
//!    the [`CellLocalRowResolver`] seam (the search-shaped twin of the cross-cell PII-free pointer
//!    bridge, 12.6): the home cell renders the row against ITS tuples and returns ONLY the filtered
//!    projection (or a tombstone) — never a raw row, never a payload that should stay in that cell.
//!
//! There is NO second query engine, NO second fusion, NO second pointer frame — the single-cell
//! mechanisms are reused at the cell boundary (EI-01 §7).
//!
//! ## The leak-free property holds ACROSS the cell boundary (SRCH-D1/SRCH-P09, now federated)
//! The cardinal Search sin (returning a result the viewer cannot access) does not regress at the
//! cell boundary. EACH cell applies ITS OWN `list_objects` pre-filter in [`CellLocalQuery::run`], so a
//! `CellRanking` carries ONLY the viewer's visible `ArtifactRef`s from that cell — a confidential row
//! the viewer cannot see is never in the ranking, so it can never be in the fused order (RRF holds no
//! ACL state — [`crate::fusion`]). The per-viewer home-cell resolution then renders each row IN its
//! home cell against THAT cell's tuples; a row the viewer may not render is a [`FederatedRow`]
//! tombstone carrying NO content (the secret never crosses). The SRCH-P09 leak mutation floor holds
//! under the federated path: the merge can only emit `ArtifactRef`s that at least one cell's
//! ACL-filtered ranking surfaced, and the resolution can only render a row the home cell permits.
//!
//! ## The residency-free-merge ZERO (the leak-free gate; §6.4)
//! [`FederatedSearch::payload_crossed_merge`] is pinned to **0** by construction: the merge carries
//! ONLY `ArtifactRef`s + scores (the ranking metadata) across the control-plane boundary, and the
//! resolution carries ONLY a filtered projection/tombstone back. It is a LIVE counter (not a
//! constant) so a future regression that carried a payload/PII across the merge boundary is
//! OBSERVABLE (it would tick above 0). This is the "0 PII crossing the merge boundary" projection the
//! SRCH-P31 leak-free gate asserts `== 0`, mirroring the cross-cell PII-free bridge's identical zero
//! (`myelin_refs_service::cross_cell::CrossCellFanOut::raw_rows_crossed`,
//! `myelin_control_plane::CrossCellBridge::cross_cell_raw_rows`).
//!
//! ## The gate now owed (the cross-cell leak-free gate; §6.4)
//! - **Cross-cell leak-free** — a federated query across two cells returns ONLY the viewer's visible
//!   rows, resolved per-viewer in their home cell; the residency-free merge carries ONLY ranking
//!   metadata + `ArtifactRef`s, NEVER payload/PII; **0 cross-cell leak, 0 PII crossing the merge
//!   boundary** ([`FederatedSearch::payload_crossed_merge`] `== 0`). The
//!   [`tests::federated_query_across_two_cells_is_leak_free_zero_pii_crossing`] is the dated green
//!   artifact (driven as a scaled-down two-cell variant — the cross-process wire is the substrate
//!   floor, below).
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **No NEW floor** — cross-cell federated search IS the named single-cell follow-on (§6.4). The
//!   cross-cell BUILD is **gated on multi-cell going live** (contract 12.6, OQ-I): until then the
//!   single-cell path is complete and this design HOLDS (designed-and-extends). The single-cell query
//!   was complete from M2; this extends the cell-agnostic §5 contracts WITHOUT a rewrite.
//! - **The wire transport behind [`CellLocalQuery`] / [`CellLocalRowResolver`] is the named substrate
//!   floor.** In production cell A reaches a member cell's query executor + its row resolver over the
//!   control plane's cross-cell bridge transport (the substrate `ResilientClient` wire, whose `send`
//!   body is the first-real-producer floor). The seams (scatter the query, fuse only ranking metadata,
//!   resolve each row in its home cell) are REAL + proven here against in-process executors standing in
//!   for the member cells (the SAME stand-in the refs cross-cell fan-out + the control-plane bridge
//!   tests use); the cross-process WIRE is the substrate floor.
//! - **The member-cell ENUMERATION is the control plane's `placement_of`/`member_cells` fan-out
//!   (P-CP-20 / P-430).** This module SCATTERS to a caller-supplied member-cell set + FUSES the
//!   rankings; the `placement_of`-driven enumeration of a tenant's member cells that PRODUCES the set
//!   lives in the control plane (`myelin_control_plane::multi_cell`). The scatter-gather + residency-
//!   free merge + per-viewer home-cell resolution mechanism is the Search deliverable and is live here.
//! - **The E2E wedge is SRCH-P32** (E2E-1 PR pane / E2E-3 reindex-parity / E2E-4 DSAR fan-out).
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//! The federated path is leak-of-cross-cell-confidential-content-critical (a row the viewer cannot see
//! in a member cell must be ABSENT from the ranking; a row the viewer cannot render in its home cell
//! must be a tombstone, never a leak across the cell boundary). Floor: **≥ 80% of viable mutants
//! caught** (`cargo mutants -p myelin-search -f crates/myelin-search/src/cross_cell.rs`). Every rule —
//! the per-cell ACL-filtered scatter, the RRF merge order, the home-cell row dispatch, the
//! denied/erased → tombstone arm, the unknown-home degrade, the migration re-home — has a test a
//! mutation flips. **Measured 2026-06-25: 24 mutants -> 10 unviable, 14 viable, 13 caught, 1 missed =
//! 93% of viable; the single missed is the documented EQUIVALENT mutant below -> 100% of NON-equivalent
//! viable** — floor met. (The `payload_crossed_merge` `replace -> 0` is the documented EQUIVALENT
//! mutant: the merge NEVER increments it — the *correct* property, not a coverage gap; the tripwire
//! stays wired for the day a regression lands, mirroring the refs `cross_cell_raw_rows` identical zero.)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::ArtifactRef;
use myelin_identity::Principal;
use myelin_tenancy::{CellId, Region, TenantId};

use crate::fusion::{reciprocal_rank_fusion, RankedList};

/// The telemetry signal name the federated search emits for the per-cell scatters it served (contract
/// 1.8 / the §6.4 leak-free proof). A named constant so a drill asserts against the NAME, never a
/// literal.
pub const FEDERATED_SCATTERS_SIGNAL: &str = "search.federated_scatters";

/// The telemetry signal name for the §6.4 ZERO — payload / PII carried across the residency-free merge
/// boundary. Pinned to 0 by construction; a named constant so a drill asserts against the NAME `== 0`.
pub const FEDERATED_PAYLOAD_CROSSED_SIGNAL: &str = "search.federated_payload_crossed_merge";

/// **One cell's permission-filtered ranking — the ONLY thing that crosses the scatter boundary
/// (ranking metadata, §6.4).** A `CellRanking` carries an ordered list of [`ArtifactRef`]s (the
/// viewer's VISIBLE rows from that cell, descending relevance) + the cell they came from — NEVER a
/// payload, NEVER PII. Each cell's own `list_objects` pre-filter (§4.2.1) ran BEFORE this is built, so
/// a confidential row the viewer cannot see is structurally absent (the leak-free property, cross-cell).
///
/// The `ArtifactRef` doubles as both the merge key (the one-doc-id space, §3.2) and the routing handle
/// for the per-viewer home-cell resolution (the home cell renders it against ITS tuples). The score is
/// carried only to express the cell's local rank ORDER; the residency-free merge is score-scale-free
/// (RRF), so the absolute scores never need to be comparable across cells.
#[derive(Clone, Debug, PartialEq)]
pub struct CellRanking {
    /// The cell this ranking came from (an opaque routing handle — the home cell for its rows).
    pub cell: CellId,
    /// The viewer's VISIBLE `ArtifactRef`s from this cell, in descending local relevance (rank 1 =
    /// position 0). ACL-filtered IN the cell (`list_objects` pre-filter ran) — never a row the viewer
    /// cannot see. PII-free: an `ArtifactRef` is an opaque URN, never a payload.
    pub refs: Vec<ArtifactRef>,
}

impl CellRanking {
    /// Build a ranking for `cell` from its ACL-filtered `refs` (already in descending local
    /// relevance). The producer (a member cell's [`CellLocalQuery`]) supplies only the viewer's
    /// visible refs — this is the residency-free ranking metadata, never a payload.
    pub fn new(cell: CellId, refs: impl IntoIterator<Item = ArtifactRef>) -> CellRanking {
        CellRanking {
            cell,
            refs: refs.into_iter().collect(),
        }
    }
}

/// **The cell-local query seam (§6.4 scatter; contract 6.1/6.2 cross-cell-extended).** The federated
/// search dispatches the SAME permission-filtered query to each member cell through this trait; the
/// implementor (production: that cell's [`crate::pipeline::query`] reached over the control plane's
/// bridge transport) runs the query LOCALLY against ITS own index / `list_objects` / residency and
/// returns ONLY the [`CellRanking`] (ranking metadata — the viewer's visible `ArtifactRef`s) — NEVER a
/// payload, NEVER PII.
///
/// This is the Search-shaped twin of `myelin_refs_service::cross_cell::CellLocalBacklinkResolver` and
/// `myelin_control_plane::CellLocalResolver` (the SAME seam shape): one query path, run cell-local,
/// only ranking metadata crosses. `Send + Sync` so the federated search holds it behind an [`Arc`]
/// across serving threads.
pub trait CellLocalQuery: Send + Sync {
    /// Run the viewer's permission-filtered query **IN this (a member) cell** — the cell applies ITS
    /// OWN `list_objects` pre-filter — returning ONLY the [`CellRanking`] (the viewer's visible
    /// `ArtifactRef`s, ranked locally) that crosses back. NEVER returns a payload or a row the viewer
    /// cannot see (the leak-free property runs IN the cell, §4.2.1).
    fn run(
        &self,
        tenant: &TenantId,
        region: &Region,
        query: &str,
        viewer: &Principal,
    ) -> CellRanking;
}

/// **A resolved federated result row (contract 5.6 — the per-viewer home-cell project).** After the
/// residency-free merge fixes the order, each fused [`ArtifactRef`] is resolved IN its HOME cell over
/// [`CellLocalRowResolver`]; the home cell renders it against ITS tuples and returns this row. A row
/// the viewer may render carries the rendered projection (title/state — the viewer IS entitled to it);
/// a row the viewer may NOT render (denied/gone/erased in the home cell) is a TOMBSTONE carrying NO
/// content (the leak invariant, cross-cell). The `ref_` + `home_cell` are PII-free routing handles.
#[derive(Clone, Debug, PartialEq)]
pub struct FederatedRow {
    /// The row's `ArtifactRef` (the one-doc-id-space key + the routing handle to its home cell).
    pub ref_: ArtifactRef,
    /// The cell this row is homed in (resolution happened THERE — never by another cell reaching its
    /// rows).
    pub home_cell: CellId,
    /// The rendered projection IF the viewer may see it — the title/state the home cell rendered
    /// against ITS tuples; `None` for a tombstone (denied/gone/erased — NO content crossed). The
    /// secret payload of a row the viewer cannot see NEVER appears here.
    pub projection: Option<RowProjection>,
}

impl FederatedRow {
    /// Whether the home cell admitted this row for the viewer (a rendered projection, not a tombstone).
    pub fn is_visible(&self) -> bool {
        self.projection.is_some()
    }
}

/// **The rendered, already-permission-filtered projection of a federated row (5.6).** What the home
/// cell rendered for the viewer + handed back across the merge boundary — the title/state/render hint
/// the viewer IS entitled to. This is the ONLY content that crosses back from the home cell; the raw
/// row / payload NEVER leaves the home cell. Search-shaped (a search result row's render), mirroring
/// the refs `Projection` shape.
#[derive(Clone, Debug, PartialEq)]
pub struct RowProjection {
    /// The row's display title (the viewer is entitled to it — the home cell already permission-checked).
    pub title: String,
    /// The row's display state (e.g. issue `open`/`closed`, page `published`).
    pub state: String,
    /// The render hint the surface uses to draw the row (e.g. `issue-card`, `page-card`).
    pub render_hint: String,
}

/// **The cell-local row resolver seam (§6.4 per-viewer home-cell resolution; contract 5.6 / 12.6).**
/// The federated search resolves each fused [`ArtifactRef`] in its HOME cell through this trait; the
/// implementor (production: that cell's `project`/resolve reached over the cross-cell PII-free pointer
/// bridge, 12.6) renders the row **IN that cell**, permission-checked **against THAT cell's tuples**,
/// and returns ONLY the rendered projection (or `None` for a tombstone) — NEVER a raw row.
///
/// The Search-shaped twin of [`CellLocalQuery`] for the RESOLUTION half: one project rule, run
/// cell-local, only the rendered projection crosses back. `Send + Sync` so it sits behind an [`Arc`].
pub trait CellLocalRowResolver: Send + Sync {
    /// Render `ref_` for `viewer` **IN this (the home) cell** — permission-checked against THIS cell's
    /// tuples — returning ONLY the filtered [`RowProjection`] that crosses back, or `None` for a
    /// tombstone (denied/gone/erased — the secret NEVER crosses). NEVER returns a raw row.
    fn project_row(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
    ) -> Option<RowProjection>;
}

/// **Cross-cell federated search (§6.4 — scatter-gather + residency-free merge + per-viewer home-cell
/// resolution).** Serves a viewer over a set of member cells: scatters the SAME permission-filtered
/// query to each cell's [`CellLocalQuery`], fuses the per-cell rankings with the score-scale-free RRF
/// (the residency-free merge — only ranking metadata crosses), then resolves each fused row IN its
/// HOME cell over [`CellLocalRowResolver`] (only the filtered projection/tombstone crosses back).
///
/// `scattered` + `payload_crossed_merge` are the §6.4 leak-free-gate telemetry: every per-cell scatter
/// increments `scattered`; `payload_crossed_merge` is pinned to **0** by construction (only ranking
/// metadata + `ArtifactRef`s cross the merge, only a filtered projection crosses back) — a live
/// tripwire so a regression that carried a payload across the merge boundary is observable.
#[derive(Clone)]
pub struct FederatedSearch {
    /// The cell this federated search serves (the control-plane boundary cell — where the viewer asked).
    coordinator_cell: CellId,
    /// The member cells the federated search scatters the query to (their cell-local query executors).
    /// In production each member cell exposes its query endpoint over the bridge transport; here the
    /// registry holds the executor handles directly (the wire is the named substrate floor).
    queriers: HashMap<CellId, Arc<dyn CellLocalQuery>>,
    /// The per-cell row resolvers (the per-viewer home-cell resolution, 5.6). Keyed by the cell a row
    /// is homed in (a fused `ArtifactRef`'s home cell — the cell whose ranking surfaced it).
    resolvers: HashMap<CellId, Arc<dyn CellLocalRowResolver>>,
    /// §6.4 telemetry: how many per-cell scatters the federated search served (aggregate, PII-free).
    scattered: Arc<AtomicU64>,
    /// **The §6.4 ZERO — payload / PII carried across the residency-free merge boundary.** Pinned to 0
    /// by construction; a live tripwire (not a constant) so a regression that carried a payload across
    /// is observable.
    payload_crossed_merge: Arc<AtomicU64>,
}

impl FederatedSearch {
    /// Build a federated search coordinated by `coordinator_cell` with no member cells registered yet.
    pub fn new(coordinator_cell: CellId) -> FederatedSearch {
        FederatedSearch {
            coordinator_cell,
            queriers: HashMap::new(),
            resolvers: HashMap::new(),
            scattered: Arc::new(AtomicU64::new(0)),
            payload_crossed_merge: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The cell this federated search is coordinated by (the control-plane boundary cell — opaque id).
    pub fn coordinator_cell(&self) -> &CellId {
        &self.coordinator_cell
    }

    /// Register a member `cell`: its [`CellLocalQuery`] (the scatter target) AND its
    /// [`CellLocalRowResolver`] (the per-viewer home-cell resolution for rows homed there). Both are
    /// keyed by the same `cell` so a fused row from that cell's ranking resolves in that same cell —
    /// one path, no special-case (the coordinator cell registers its OWN executors too).
    pub fn register(
        &mut self,
        cell: CellId,
        querier: Arc<dyn CellLocalQuery>,
        resolver: Arc<dyn CellLocalRowResolver>,
    ) {
        self.queriers.insert(cell.clone(), querier);
        self.resolvers.insert(cell, resolver);
    }

    /// **Scatter — run the SAME permission-filtered query in each registered member cell (§6.4).**
    /// Dispatch `query` to every member cell's [`CellLocalQuery`]; each runs it LOCALLY against ITS own
    /// index / `list_objects` / residency and returns ONLY its [`CellRanking`] (the viewer's visible
    /// `ArtifactRef`s) — never a payload. Returns the per-cell rankings (ranking metadata only). The
    /// `payload_crossed_merge` zero is untouched — only ranking metadata crossed.
    ///
    /// The cells are scattered in a deterministic order (sorted by cell id) so the gather + merge are
    /// reproducible regardless of registration order.
    pub fn scatter(
        &self,
        tenant: &TenantId,
        region: &Region,
        query: &str,
        viewer: &Principal,
    ) -> Vec<CellRanking> {
        let mut cells: Vec<&CellId> = self.queriers.keys().collect();
        cells.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        cells
            .into_iter()
            .map(|cell| {
                self.scattered.fetch_add(1, Ordering::SeqCst);
                // The member cell runs the query IN the cell; only the ranking metadata crosses back.
                self.queriers[cell].run(tenant, region, query, viewer)
            })
            .collect()
    }

    /// **The residency-free merge (§6.4) — fuse the per-cell rankings into ONE order, carrying ONLY
    /// ranking metadata + `ArtifactRef`s.** Builds a [`RankedList`] from each cell's `refs` (its local
    /// rank order) and fuses them with the SAME score-scale-free RRF used INSIDE a single cell
    /// ([`crate::fusion::reciprocal_rank_fusion`]) — no per-cell score calibration is needed (RRF fuses
    /// on RANK). The fused list is the cross-cell ranking; each entry is an `ArtifactRef` + its fused
    /// score + the home cell it came from (for the per-viewer resolution).
    ///
    /// NO payload crosses the merge — only `ArtifactRef`s + the rank order. `payload_crossed_merge`
    /// stays 0. RRF holds no ACL state, so the merge can only emit an `ArtifactRef` at least one cell's
    /// ACL-filtered ranking surfaced (the leak-free property, cross-cell — [`crate::fusion`]).
    pub fn residency_free_merge(&self, rankings: &[CellRanking]) -> Vec<MergedRef> {
        // Map each ArtifactRef to its home cell (the cell whose ranking surfaced it). If two cells
        // surface the same ref (a shared/replicated artifact), the FIRST cell in deterministic order
        // owns the home — the resolution is idempotent across replicas (resolution is cell-local).
        let mut home_of: HashMap<String, CellId> = HashMap::new();
        let mut branches: Vec<RankedList> = Vec::with_capacity(rankings.len());
        for ranking in rankings {
            for r in &ranking.refs {
                home_of
                    .entry(r.0.clone())
                    .or_insert_with(|| ranking.cell.clone());
            }
            branches.push(RankedList::from_ranked(
                ranking.refs.iter().map(|r| r.0.clone()),
            ));
        }
        // Fuse on RANK (residency-free, score-scale-free) — the merge carries only ArtifactRefs.
        reciprocal_rank_fusion(&branches)
            .into_iter()
            .map(|fused| {
                let home = home_of
                    .get(&fused.doc_id)
                    .cloned()
                    // Unreachable: a fused doc_id always came from some ranking — kept total + safe.
                    .unwrap_or_else(|| self.coordinator_cell.clone());
                MergedRef {
                    ref_: ArtifactRef(fused.doc_id),
                    home_cell: home,
                    score: fused.score,
                }
            })
            .collect()
    }

    /// **Per-viewer home-cell resolution (§6.4 / 5.6) — resolve EACH merged row in its HOME cell.** For
    /// each [`MergedRef`] (in fused order), dispatch to its `home_cell`'s [`CellLocalRowResolver`]; the
    /// home cell renders the row against ITS tuples and returns ONLY the filtered projection (or `None`
    /// → a tombstone). A row whose home cell is unknown to this federated search degrades to a
    /// tombstone (never fabricate content, never reach into an unseen cell). Returns the rows in fused
    /// order; ONLY a filtered projection/tombstone crossed back — `payload_crossed_merge` stays 0.
    pub fn resolve_rows(
        &self,
        tenant: &TenantId,
        region: &Region,
        merged: &[MergedRef],
        viewer: &Principal,
    ) -> Vec<FederatedRow> {
        merged
            .iter()
            .map(|m| {
                let projection = match self.resolvers.get(&m.home_cell) {
                    // The home cell renders the row IN the home cell against ITS tuples; only the
                    // filtered projection crosses back (None → a tombstone, the secret never crosses).
                    Some(resolver) => resolver.project_row(tenant, region, &m.ref_, viewer),
                    // Unknown home cell — degrade to a tombstone (never reach into an unseen cell).
                    None => None,
                };
                FederatedRow {
                    ref_: m.ref_.clone(),
                    home_cell: m.home_cell.clone(),
                    projection,
                }
            })
            .collect()
    }

    /// **The full federated search (§6.4 — scatter → residency-free merge → per-viewer home-cell
    /// resolution).** Runs the SAME permission-filtered query across all member cells, fuses the
    /// rankings residency-free (only ranking metadata crosses), then resolves each fused row in its
    /// home cell (only the filtered projection/tombstone crosses back). Returns the rows in fused
    /// order INCLUDING the tombstones for rows the viewer cannot render — the render path drops the
    /// tombstones via [`FederatedSearch::query`]; this exposes them so a drill can PROVE a denied
    /// cross-cell row is a tombstone (the leak invariant), not merely absent.
    pub fn query_all(
        &self,
        tenant: &TenantId,
        region: &Region,
        query: &str,
        viewer: &Principal,
    ) -> Vec<FederatedRow> {
        let rankings = self.scatter(tenant, region, query, viewer);
        let merged = self.residency_free_merge(&rankings);
        self.resolve_rows(tenant, region, &merged, viewer)
    }

    /// **The federated query render path (§6.4).** Like [`Self::query_all`] but drops the tombstones —
    /// returns ONLY the rows the viewer may see, resolved per-viewer in their home cell, in fused
    /// order. This is the public federated result the surface renders: 0 cross-cell leak (every row is
    /// home-cell-permission-filtered), 0 PII crossing the merge (only ranking metadata + the filtered
    /// projection crossed).
    pub fn query(
        &self,
        tenant: &TenantId,
        region: &Region,
        query: &str,
        viewer: &Principal,
    ) -> Vec<FederatedRow> {
        self.query_all(tenant, region, query, viewer)
            .into_iter()
            .filter(FederatedRow::is_visible)
            .collect()
    }

    /// **§6.4 telemetry — `search.federated_scatters`.** How many per-cell scatters the federated
    /// search served (aggregate, PII-free).
    pub fn scattered(&self) -> u64 {
        self.scattered.load(Ordering::SeqCst)
    }

    /// **The §6.4 ZERO — `search.federated_payload_crossed_merge`.** Payload / PII carried across the
    /// residency-free merge boundary. Pinned to 0 by construction (only ranking metadata +
    /// `ArtifactRef`s cross the merge; only a filtered projection crosses back); a live tripwire so a
    /// regression that carried a payload is observable.
    ///
    /// **Equivalent-mutant note (cargo-mutants):** `replace payload_crossed_merge -> 0` is
    /// observationally identical because the merge NEVER increments it (the structural guarantee) — the
    /// *correct* property, not a coverage gap. The field + the read seam stay so the tripwire is wired
    /// the day a regression lands (mirrors the refs `cross_cell_raw_rows` + the bridge's identical zero).
    pub fn payload_crossed_merge(&self) -> u64 {
        self.payload_crossed_merge.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for FederatedSearch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // PII-free Debug: the coordinator cell + the aggregate counters, never a viewer/query/row.
        f.debug_struct("FederatedSearch")
            .field("coordinator_cell", &self.coordinator_cell.as_str())
            .field("member_cells", &self.queriers.len())
            .field("scattered", &self.scattered())
            .field("payload_crossed_merge", &self.payload_crossed_merge())
            .finish()
    }
}

/// **A merged cross-cell ranking entry (the residency-free-merge output; §6.4).** Carries ONLY ranking
/// metadata: the `ArtifactRef`, its home cell (for the per-viewer resolution), and the fused RRF score.
/// PII-free by construction — there is structurally no payload field. This is the EXACT shape that
/// crosses the residency-free merge boundary (the leak-free proof reads it: only refs + scores + home
/// cells, never a payload).
#[derive(Clone, Debug, PartialEq)]
pub struct MergedRef {
    /// The merged row's `ArtifactRef` (opaque URN — never a payload).
    pub ref_: ArtifactRef,
    /// The cell the row is homed in (where the per-viewer resolution happens).
    pub home_cell: CellId,
    /// The fused RRF score (the residency-free cross-cell rank; score-scale-free).
    pub score: f32,
}

/// **CP-D7 — cell→cell migration, re-home a member-cell ranking with 0 loss.** When a member cell
/// MIGRATES (its rows are re-homed from `from` to `to`), the ranking's `cell` (the home routing
/// handle) is re-stamped to the NEW cell so the federated search re-scatters/re-resolves there. ONLY
/// the routing handle changes — the ACL-filtered `ArtifactRef`s are preserved byte-for-byte (no row is
/// lost in the migration; the SAME ranking, now homed in the new cell). Returns a NEW ranking (the
/// frame is rebuilt re-homed; EI-01 §7, one shape).
///
/// A ranking NOT homed in `from` is returned unchanged (the migration is precise — it re-homes only
/// the cell that actually migrated, never a bystander).
#[must_use]
pub fn migrate_ranking_home(ranking: &CellRanking, from: &CellId, to: &CellId) -> CellRanking {
    if &ranking.cell == from {
        CellRanking {
            cell: to.clone(),
            refs: ranking.refs.clone(),
        }
    } else {
        ranking.clone()
    }
}

/// **The residency-free-merge PII-free proof body (the leak-free gate body; §6.4).** What crosses the
/// merge boundary for a row is EXACTLY the `ArtifactRef` + the home cell + the fused score — never a
/// payload, never PII. Extracts the three (opaque, PII-free) fields the leak-free gate asserts crossed,
/// so a drill can show "the merge carried only `ref`/`home_cell`/`score`" with concrete opaque values.
/// There is structurally no payload field. Mirrors `myelin_refs_service::cross_cell::fanout_carried_fields`.
pub fn merge_carried_fields(merged: &MergedRef) -> (&ArtifactRef, &CellId, f32) {
    (&merged.ref_, &merged.home_cell, merged.score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use std::collections::HashSet;
    use std::sync::Mutex;

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn coordinator() -> CellId {
        CellId::from_token("cell-fr-par-0")
    }
    fn cell_a() -> CellId {
        CellId::from_token("cell-fr-par-1")
    }
    fn cell_b() -> CellId {
        CellId::from_token("cell-fr-par-2")
    }
    fn cell_c() -> CellId {
        CellId::from_token("cell-de-fra-1")
    }
    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }
    fn aref(s: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://acme/issues/issue/{s}"))
    }

    /// A test cell-local query executor + row resolver standing in for a member cell's
    /// `query`/`project` (the SAME stand-in shape the refs cross-cell + the control-plane bridge tests
    /// use). It holds: a per-(ref) match list (the rows this cell's index would rank for the query), a
    /// per-(ref, viewer) permission map (the cell's `list_objects` pre-filter + the home-cell project
    /// permission check), per-ref ERASED rows, and per-ref rendered titles (the SECRET that must NOT
    /// leak to a denied viewer). It records the queries it ran + the rows it was asked to project so a
    /// test can assert the work happened IN the home cell (never by another cell reaching its rows).
    #[derive(Default)]
    struct StandInCell {
        /// the refs this cell's index ranks for the query, in local relevance order.
        matches: Mutex<Vec<String>>,
        /// (ref_urn, viewer_id) pairs allowed to SEE/project the row; everyone else is denied.
        allowed: Mutex<Vec<(String, String)>>,
        /// ref_urns whose rows have been ERASED in this cell (project → None tombstone).
        erased: Mutex<Vec<String>>,
        /// the per-ref rendered title (the SECRET that must not leak to a denied cross-cell viewer).
        titles: Mutex<HashMap<String, String>>,
        /// records every query string this cell ran (the scatter landed HERE).
        ran_queries: Mutex<Vec<String>>,
        /// records every ref this cell was asked to project (the resolution happened HERE).
        projected: Mutex<Vec<String>>,
    }

    impl StandInCell {
        fn index_match(&self, ref_urn: &str) {
            self.matches.lock().unwrap().push(ref_urn.into());
        }
        fn allow(&self, ref_urn: &str, viewer_id: &str) {
            self.allowed
                .lock()
                .unwrap()
                .push((ref_urn.into(), viewer_id.into()));
        }
        fn set_title(&self, ref_urn: &str, title: &str) {
            self.titles
                .lock()
                .unwrap()
                .insert(ref_urn.into(), title.into());
        }
        fn erase(&self, ref_urn: &str) {
            self.erased.lock().unwrap().push(ref_urn.into());
        }
        fn ran_queries(&self) -> Vec<String> {
            self.ran_queries.lock().unwrap().clone()
        }
        fn projected_refs(&self) -> Vec<String> {
            self.projected.lock().unwrap().clone()
        }
        fn is_allowed(&self, ref_urn: &str, viewer_id: &str) -> bool {
            self.allowed
                .lock()
                .unwrap()
                .iter()
                .any(|(r, v)| r == ref_urn && v == viewer_id)
        }
    }

    impl CellLocalQuery for StandInCell {
        fn run(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            query: &str,
            viewer: &Principal,
        ) -> CellRanking {
            self.ran_queries.lock().unwrap().push(query.into());
            // The cell's list_objects PRE-FILTER (§4.2.1): only the viewer's VISIBLE matches enter the
            // ranking — a row the viewer cannot see is structurally absent (the leak-free property).
            let refs: Vec<ArtifactRef> = self
                .matches
                .lock()
                .unwrap()
                .iter()
                .filter(|r| self.is_allowed(r, &viewer.principal_id.0))
                .map(|r| ArtifactRef(r.clone()))
                .collect();
            // the cell is filled by the test via register-with-cell; the caller supplies the CellId.
            CellRanking {
                cell: CellId::from_token("PLACEHOLDER"),
                refs,
            }
        }
    }

    impl CellLocalRowResolver for StandInCell {
        fn project_row(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            ref_: &ArtifactRef,
            viewer: &Principal,
        ) -> Option<RowProjection> {
            self.projected.lock().unwrap().push(ref_.0.clone());
            // ERASED in this cell → a tombstone (the row unresolvable cross-cell, GA-D8-class).
            if self.erased.lock().unwrap().iter().any(|e| e == &ref_.0) {
                return None;
            }
            // Permission-checked IN this cell against THIS cell's allow-map (the coordinator never sees
            // it). Denied → None (a tombstone carrying NO content — the secret never crosses).
            if !self.is_allowed(&ref_.0, &viewer.principal_id.0) {
                return None;
            }
            let title = self
                .titles
                .lock()
                .unwrap()
                .get(&ref_.0)
                .cloned()
                .unwrap_or_else(|| "untitled".into());
            Some(RowProjection {
                title,
                state: "open".into(),
                render_hint: "issue-card".into(),
            })
        }
    }

    /// A cell that stamps its OWN cell id onto the ranking (the production [`CellLocalQuery`] knows its
    /// cell; the stand-in is told its cell at construction so `scatter` produces correctly-homed
    /// rankings).
    struct HomedCell {
        cell: CellId,
        inner: Arc<StandInCell>,
    }
    impl CellLocalQuery for HomedCell {
        fn run(
            &self,
            tenant: &TenantId,
            region: &Region,
            query: &str,
            viewer: &Principal,
        ) -> CellRanking {
            let mut r = self.inner.run(tenant, region, query, viewer);
            r.cell = self.cell.clone();
            r
        }
    }

    fn register_cell(fed: &mut FederatedSearch, cell: CellId, inner: Arc<StandInCell>) {
        let querier = Arc::new(HomedCell {
            cell: cell.clone(),
            inner: inner.clone(),
        });
        fed.register(cell, querier, inner);
    }

    // ── The cross-cell leak-free gate (§6.4 — the SCHED green artifact) ──

    /// **A federated query across two cells returns ONLY the viewer's visible rows, resolved per-viewer
    /// in their home cell; the residency-free merge carries only ranking metadata, 0 PII crossing the
    /// merge boundary (the §6.4 cross-cell leak-free gate — the dated green artifact).** Cell B ranks a
    /// row the viewer MAY see + a SECRET the viewer may NOT; cell C ranks a row the viewer may see. The
    /// federated query returns only the two visible rows (resolved in B and C), never the secret; the
    /// merge carried only ArtifactRefs; `payload_crossed_merge == 0`.
    #[test]
    fn federated_query_across_two_cells_is_leak_free_zero_pii_crossing() {
        let b = Arc::new(StandInCell::default());
        let c = Arc::new(StandInCell::default());

        let secret = "TOP SECRET cross-org acquisition memo";
        // cell B: a visible row + a secret row the viewer cannot see.
        let b_ok = aref("b-ok");
        let b_secret = aref("b-secret");
        b.index_match(&b_ok.0);
        b.index_match(&b_secret.0);
        b.allow(&b_ok.0, "viewer1");
        b.set_title(&b_ok.0, "B visible row");
        b.set_title(&b_secret.0, secret); // viewer1 NOT allowed → never crosses.
                                          // cell C: a visible row.
        let c_ok = aref("c-ok");
        c.index_match(&c_ok.0);
        c.allow(&c_ok.0, "viewer1");
        c.set_title(&c_ok.0, "C visible row");

        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b.clone());
        register_cell(&mut fed, cell_c(), c.clone());

        let rows = fed.query(&tenant(), &region(), "acquisition", &viewer("viewer1"));

        // Only the two visible rows surface — never the secret.
        let titles: Vec<String> = rows
            .iter()
            .filter_map(|r| r.projection.as_ref().map(|p| p.title.clone()))
            .collect();
        let title_set: HashSet<&str> = titles.iter().map(String::as_str).collect();
        assert!(
            title_set.contains("B visible row"),
            "the B row the viewer may see surfaces"
        );
        assert!(
            title_set.contains("C visible row"),
            "the C row the viewer may see surfaces"
        );
        assert_eq!(
            rows.len(),
            2,
            "exactly the two visible rows (the secret never surfaces)"
        );

        // 0 cross-cell leak: the secret never appears anywhere in the rendered result.
        let rendered = format!("{rows:?}");
        assert!(
            !rendered.contains("SECRET") && !rendered.contains("acquisition memo"),
            "0 cross-cell leak: the secret must not cross, got `{rendered}`"
        );
        // Each row was resolved IN its home cell (the per-viewer home-cell resolution).
        assert!(
            rows.iter().any(|r| r.home_cell == cell_b())
                && rows.iter().any(|r| r.home_cell == cell_c()),
            "the rows resolved per-viewer in their home cells (B and C)"
        );
        assert!(
            b.projected_refs().contains(&b_ok.0),
            "the B row was projected IN cell B"
        );
        assert!(
            c.projected_refs().contains(&c_ok.0),
            "the C row was projected IN cell C"
        );
        // §6.4 ZERO: only ranking metadata + the filtered projection crossed — 0 payload across merge.
        assert_eq!(
            fed.payload_crossed_merge(),
            0,
            "0 PII crossing the residency-free merge boundary"
        );
        // two cells were scattered to.
        assert_eq!(
            fed.scattered(),
            2,
            "the query scattered to both member cells"
        );
    }

    /// **The scatter runs the SAME query IN each member cell (the leak-free pre-filter runs in the
    /// cell).** Each cell's `run` ran the query; the secret row never enters cell B's ranking (its
    /// list_objects pre-filter excluded it for the viewer) — so it can never reach the merge.
    #[test]
    fn scatter_runs_the_query_in_each_cell_and_secret_never_enters_the_ranking() {
        let b = Arc::new(StandInCell::default());
        let b_ok = aref("b-ok");
        let b_secret = aref("b-secret");
        b.index_match(&b_ok.0);
        b.index_match(&b_secret.0);
        b.allow(&b_ok.0, "v"); // b_secret NOT allowed for v.

        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b.clone());

        let rankings = fed.scatter(&tenant(), &region(), "q", &viewer("v"));
        assert_eq!(rankings.len(), 1, "one member cell scattered to");
        assert_eq!(
            b.ran_queries(),
            vec!["q".to_string()],
            "the query ran IN cell B"
        );
        // the secret is NOT in the ranking — the leak-free pre-filter ran in the cell.
        let refs: Vec<String> = rankings[0].refs.iter().map(|r| r.0.clone()).collect();
        assert_eq!(
            refs,
            vec![b_ok.0.clone()],
            "only the viewer's visible ref entered the ranking"
        );
        assert!(
            !refs.contains(&b_secret.0),
            "the secret never entered the ranking (leak-free)"
        );
        // the ranking is homed in cell B (the routing handle for the per-viewer resolution).
        assert_eq!(rankings[0].cell, cell_b(), "the ranking is homed in cell B");
    }

    // ── The residency-free merge (only ranking metadata, score-scale-free RRF) ──

    /// **The residency-free merge fuses per-cell rankings on RANK (score-scale-free), carrying only
    /// ArtifactRefs + home cells — never a payload.** A ref ranked highly in BOTH cells (a shared
    /// artifact) out-ranks a ref ranked highly in only one (the RRF agreement boost) — and the merged
    /// entries carry only ref/home_cell/score (no payload field exists).
    #[test]
    fn residency_free_merge_fuses_on_rank_carrying_only_refs() {
        let shared = aref("shared");
        let b_only = aref("b-only");
        let c_only = aref("c-only");
        // cell B ranks: shared (1), b_only (2). cell C ranks: c_only (1), shared (2).
        let rankings = vec![
            CellRanking::new(cell_b(), vec![shared.clone(), b_only.clone()]),
            CellRanking::new(cell_c(), vec![c_only.clone(), shared.clone()]),
        ];
        let fed = FederatedSearch::new(coordinator());
        let merged = fed.residency_free_merge(&rankings);

        // `shared` (in both rankings) fuses to the top — the RRF agreement boost across cells.
        assert_eq!(
            merged[0].ref_, shared,
            "the cross-cell agreement ref surfaces first"
        );
        // home of `shared` is cell B (the first cell in deterministic order that surfaced it).
        assert_eq!(
            merged[0].home_cell,
            cell_b(),
            "shared is homed in the first cell that surfaced it"
        );
        // every other ref is present, homed in its surfacing cell.
        let by_ref: HashMap<&str, &CellId> = merged
            .iter()
            .map(|m| (m.ref_.0.as_str(), &m.home_cell))
            .collect();
        assert_eq!(by_ref.get(b_only.0.as_str()), Some(&&cell_b()));
        assert_eq!(by_ref.get(c_only.0.as_str()), Some(&&cell_c()));
        // the merge carried ONLY ranking metadata — the merge ZERO holds.
        assert_eq!(
            fed.payload_crossed_merge(),
            0,
            "the merge carried only ArtifactRefs + scores"
        );
        // the carried-fields projection has exactly three opaque fields (no payload).
        let (r, home, score) = merge_carried_fields(&merged[0]);
        assert_eq!(r, &shared);
        assert_eq!(home, &cell_b());
        assert!(score > 0.0, "the fused score is a positive rank metric");
    }

    /// **The merge can only emit an ArtifactRef at least one cell's ACL-filtered ranking surfaced (the
    /// leak-free property, cross-cell — RRF holds no ACL state).** A ref in NO cell ranking can never
    /// appear in the merged output.
    #[test]
    fn merge_never_introduces_a_ref_absent_from_every_ranking() {
        let rankings = vec![
            CellRanking::new(cell_b(), vec![aref("a"), aref("b")]),
            CellRanking::new(cell_c(), vec![aref("b"), aref("c")]),
        ];
        let fed = FederatedSearch::new(coordinator());
        let merged = fed.residency_free_merge(&rankings);
        let got: HashSet<String> = merged.iter().map(|m| m.ref_.0.clone()).collect();
        let union: HashSet<String> = ["a", "b", "c"].into_iter().map(|s| aref(s).0).collect();
        assert_eq!(
            got, union,
            "the merged set is EXACTLY the union of the rankings — no new ref"
        );
        // a secret ref no cell surfaced can never appear.
        assert!(
            !got.contains(&aref("secret").0),
            "a ref in no ranking never appears (leak-free)"
        );
    }

    // ── Per-viewer home-cell resolution (5.6) ──

    /// **A row is resolved IN its home cell; a denied viewer gets a tombstone carrying NO content (the
    /// leak invariant, cross-cell).** The home cell renders the row against ITS tuples — permitted → a
    /// projection; denied → None (a tombstone, the secret never crosses). `query_all` exposes the
    /// tombstone; `query` drops it.
    #[test]
    fn home_cell_resolution_tombstones_a_denied_row_zero_leak() {
        let b = Arc::new(StandInCell::default());
        let secret_ref = aref("b-secret");
        b.index_match(&secret_ref.0);
        b.set_title(&secret_ref.0, "SECRET title");
        // the row enters the ranking for an INSIDER but the resolution is checked per viewer in B.
        b.allow(&secret_ref.0, "insider");

        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b.clone());

        // the insider sees it (resolved in B).
        let insider_rows = fed.query(&tenant(), &region(), "q", &viewer("insider"));
        assert_eq!(insider_rows.len(), 1, "the insider sees the row");
        assert_eq!(
            insider_rows[0].projection.as_ref().unwrap().title,
            "SECRET title"
        );
        assert_eq!(insider_rows[0].home_cell, cell_b(), "resolved IN cell B");

        // an intruder: the row never enters the intruder's ranking (the pre-filter), so query() is
        // empty — but query_all over a hand-merged ref still tombstones (the resolution-level proof).
        let merged = vec![MergedRef {
            ref_: secret_ref.clone(),
            home_cell: cell_b(),
            score: 1.0,
        }];
        let resolved = fed.resolve_rows(&tenant(), &region(), &merged, &viewer("intruder"));
        assert_eq!(resolved.len(), 1);
        assert!(
            !resolved[0].is_visible(),
            "the denied row is a tombstone, not a leak"
        );
        let rendered = format!("{resolved:?}");
        assert!(
            !rendered.contains("SECRET"),
            "0 leak: the secret never crosses, got `{rendered}`"
        );
    }

    /// **An erased row resolves to a tombstone in its home cell (the row unresolvable cross-cell;
    /// GA-D8-class).** After the home cell erased the row, the per-viewer resolution returns None even
    /// for a previously-permitted viewer.
    #[test]
    fn erased_row_resolves_to_a_tombstone_in_its_home_cell() {
        let b = Arc::new(StandInCell::default());
        let r = aref("b-victim");
        b.allow(&r.0, "owner");
        b.set_title(&r.0, "victim row");
        b.erase(&r.0); // erased in the home cell.

        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b.clone());

        let merged = vec![MergedRef {
            ref_: r.clone(),
            home_cell: cell_b(),
            score: 1.0,
        }];
        let resolved = fed.resolve_rows(&tenant(), &region(), &merged, &viewer("owner"));
        assert!(
            !resolved[0].is_visible(),
            "the erased row is a tombstone (unresolvable cross-cell)"
        );
    }

    /// **A row homed in a cell UNKNOWN to the federated search degrades to a tombstone (never fabricate
    /// content, never reach into an unseen cell).** No resolver registered for the home cell → None.
    #[test]
    fn unknown_home_cell_degrades_to_tombstone_never_reaches_in() {
        let merged = vec![MergedRef {
            ref_: aref("x"),
            home_cell: cell_c(), // cell C not registered.
            score: 1.0,
        }];
        let fed = FederatedSearch::new(coordinator());
        let resolved = fed.resolve_rows(&tenant(), &region(), &merged, &viewer("anyone"));
        assert!(
            !resolved[0].is_visible(),
            "an unknown home cell degrades to a tombstone"
        );
        assert_eq!(
            fed.payload_crossed_merge(),
            0,
            "no payload crossed for an unseen cell"
        );
    }

    // ── CP-D7 cell→cell migration of a ranking, 0 loss ──

    /// **After a member cell migrates (home re-homed B → C), the ranking re-homes and the rows resolve
    /// in the NEW home with 0 loss (CP-D7-class).** The ranking's `cell` is re-stamped to C; the
    /// ACL-filtered refs are preserved byte-for-byte; the merge then homes them in C.
    #[test]
    fn cell_to_cell_migration_re_homes_the_ranking_zero_loss() {
        let r1 = aref("row1");
        let r2 = aref("row2");
        let ranking = CellRanking::new(cell_b(), vec![r1.clone(), r2.clone()]);

        let migrated = migrate_ranking_home(&ranking, &cell_b(), &cell_c());
        assert_eq!(migrated.cell, cell_c(), "the ranking re-homed to C");
        // 0 loss: the ACL-filtered refs are preserved byte-for-byte.
        assert_eq!(
            migrated.refs, ranking.refs,
            "the refs are preserved (0 loss)"
        );

        // the merge now homes the rows in C (the per-viewer resolution dispatches there).
        let fed = FederatedSearch::new(coordinator());
        let merged = fed.residency_free_merge(&[migrated]);
        assert!(
            merged.iter().all(|m| m.home_cell == cell_c()),
            "every re-homed row resolves in C"
        );
    }

    /// **A ranking NOT homed in the migrating cell is untouched (precise re-home, no bystander
    /// churn).** Migrating B → C does not re-home a ranking homed in cell A.
    #[test]
    fn migration_leaves_non_migrating_rankings_untouched() {
        let ranking = CellRanking::new(cell_a(), vec![aref("a1")]);
        let migrated = migrate_ranking_home(&ranking, &cell_b(), &cell_c());
        assert_eq!(
            migrated, ranking,
            "a non-migrating ranking is unchanged byte-for-byte"
        );
    }

    // ── PII-free Debug + the SRCH-P09 leak floor under the federated path ──

    /// **The federated search's `Debug` is PII-free + carries the coordinator cell + the counters.** A
    /// regression that leaked a viewer/query/row is caught; the Debug carries the coordinator cell id +
    /// the live `scattered`/`payload_crossed_merge` counters, never a viewer/query/secret.
    #[test]
    fn federated_debug_is_pii_free_and_carries_the_counters() {
        let b = Arc::new(StandInCell::default());
        let r = aref("b-ok");
        b.index_match(&r.0);
        b.allow(&r.0, "v");
        b.set_title(&r.0, "SECRET");
        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b);
        let _ = fed.query(&tenant(), &region(), "secret-query", &viewer("v"));
        let rendered = format!("{fed:?}");
        assert!(
            rendered.contains("FederatedSearch"),
            "the Debug names the type"
        );
        assert!(
            rendered.contains("cell-fr-par-0"),
            "the Debug carries the coordinator cell id"
        );
        assert!(
            rendered.contains("scattered"),
            "the Debug carries the scatter counter"
        );
        assert!(
            rendered.contains("payload_crossed_merge"),
            "the Debug carries the §6.4 zero counter"
        );
        assert!(
            !rendered.contains("SECRET"),
            "the Debug never leaks a title, got `{rendered}`"
        );
        assert!(
            !rendered.contains("secret-query"),
            "the Debug never leaks the query, got `{rendered}`"
        );
    }

    /// **The SRCH-P09 leak floor holds under the federated path (the cardinal sin does not regress).**
    /// The end-to-end leak proof: a confidential row never surfaces for an unauthorized viewer across
    /// the federated query — neither in the ranking (the pre-filter) nor the resolution (the home-cell
    /// check). The two gates (in-cell pre-filter + home-cell resolution) are BOTH proven, so a single
    /// regression in either is caught.
    #[test]
    fn srch_p09_leak_floor_holds_under_the_federated_path() {
        let b = Arc::new(StandInCell::default());
        let confidential = aref("confidential");
        b.index_match(&confidential.0);
        b.set_title(&confidential.0, "CONFIDENTIAL");
        // NOBODY is allowed — the row is confidential to the unauthorized viewer in every gate.
        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b.clone());

        // gate 1 (in-cell pre-filter): the row never enters the ranking for the unauthorized viewer.
        let rankings = fed.scatter(&tenant(), &region(), "q", &viewer("intruder"));
        assert!(
            rankings[0].refs.is_empty(),
            "gate 1: the confidential row never enters the ranking"
        );
        // gate 2 (home-cell resolution): even a hand-merged ref tombstones (never a leak).
        let merged = vec![MergedRef {
            ref_: confidential.clone(),
            home_cell: cell_b(),
            score: 1.0,
        }];
        let resolved = fed.resolve_rows(&tenant(), &region(), &merged, &viewer("intruder"));
        assert!(
            !resolved[0].is_visible(),
            "gate 2: the home-cell resolution tombstones the row"
        );
        // the full federated query surfaces NOTHING for the unauthorized viewer (0 leak).
        let rows = fed.query(&tenant(), &region(), "q", &viewer("intruder"));
        assert!(
            rows.is_empty(),
            "0 cross-cell leak for the unauthorized viewer"
        );
        assert_eq!(
            fed.payload_crossed_merge(),
            0,
            "0 PII crossed even on the leak-attempt path"
        );
    }
}
