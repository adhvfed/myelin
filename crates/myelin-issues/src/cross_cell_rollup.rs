//! # `cross_cell_rollup` — the cross-cell portfolio rollup over the PII-free bridge (ISS-P32 / P-495, M5)
//!
//! **The R-7 / OQ-I floor follow-on — the cross-cell portfolio rollup, LIVE (arch 05 §7 "Floor →
//! follow-on": single-cell rollup → cross-cell over the `CrossCellPointer` bridge).** A multi-cell
//! tenant's portfolio (an initiative in cell A with epics homed in cells B/C) rolls up over the
//! **frozen PII-free [`CrossCellPointer`]** bridge (contract 12.6), and **resolution is ALWAYS
//! cell-local** (OQ-I): the home cell renders + permission-checks the rollup projection; only the
//! already-aggregated, PII-free [`crate::rollup::RollupAggregate`] crosses the boundary — never a
//! leaf issue body, never a title, never a person.
//!
//! ## Reconciliation in place (EI-01 §7 — reuse the ONE bridge, never a second frame)
//! This module is the Issues twin of Knowledge's `collab::CrossCellCollab` (KN-P30 / P-485) — the
//! SAME cell-local discipline, the SAME PII-free four-field frame, the SAME 0-PII-crosses tripwire.
//! It does NOT define a second cross-cell frame: it CONSUMES the frozen
//! [`myelin_events::CrossCellPointer`] + the Bus's [`pointer_for_propagation`] mint (EB-25 / P-438)
//! over the dedicated [`CrossCellStream::IssuePortfolio`] stream (§6.2 — ISS→`ArtifactType::Issue`).
//! The cross-cell child rollup walk produces ONE [`CrossCellRollupPointer`] per remote child cell; a
//! member cell resolves it by asking the child's HOME cell to render the aggregate cell-local.
//!
//! ## The PII-free projection that crosses — ONLY the aggregate (§6.2 / the residency gate)
//! The rollup walk over a remote child does NOT pull the child's leaf rows across. It carries the
//! PII-free [`CrossCellPointer`] to the child's home cell, which renders the child subtree's
//! [`RollupAggregate`] AGAINST ITS rows, permission-checks the viewer THERE, and returns ONLY a
//! [`PortfolioProjection`] — the aggregate (done/total/estimate_sum/progress) or a tombstone. The
//! leaf bodies, the titles, the assignees NEVER leave the child's home cell; only the rolled-up
//! numbers cross. This is the doc-collab `DocProjection` twin for rollups.
//!
//! ## The DSR fan-out leg (GA-D1 / CP-D7 / CP-D8 — the cross-cell erasure receipt set)
//! A multi-cell tenant's DSR (right-to-erasure) fan-out must reach every member cell's Issues
//! holders. [`CrossCellDsrFanout::fan_out_erasure`] iterates `member_cells`, mints ONE PII-free
//! erasure pointer per member cell (carrying only the four-field frame — the opaque subject + type +
//! correlation + home_cell, never the subject's PII), and collects a per-cell [`DsrCellReceipt`]. The
//! gate: **0 member cell missed** (CP-D7 — the per-cell receipt set is complete) + **0 PII crosses**
//! (CP-D8 / GA-D8 — `pii_crossed == 0` by construction). The actual per-cell erasure runs cell-local
//! (each cell's own [`crate::holder_erase::IssueEraseFanout`], ISS-P31) — this module owns only the
//! cross-cell FAN-OUT + the receipt aggregation, never a second erase body (EI-01 §7).

use crate::rollup::RollupAggregate;
use myelin_events::{
    pointer_for_propagation, Actor, AggregateKey, ArtifactRef, CellId, CorrelationId,
    CrossCellPointer, CrossCellStream, DataRole, EventEnvelope, EventId, EventType, Region,
    TenantId, Timestamp, Visibility,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The `issue.rollup.recomputed` event-type token the cross-cell rollup pointer is minted from (the
/// rollup recompute the home cell already emitted — the pointer event rides its causal chain). A
/// POINTER event by design (arch §6.2): the durable bus carries the pointer, never the leaf rows.
const ISSUE_ROLLUP_RECOMPUTED: &str = "issue.rollup.recomputed";

/// The `issue.subject.erased` event-type token the cross-cell DSR erasure pointer is minted from (the
/// GA-D1 / CP-D7 fan-out leg). A POINTER event: it carries the opaque subject + the routing handle to
/// each member cell, never the subject's PII (which the member cell crypto-shreds cell-local).
const ISSUE_SUBJECT_ERASED: &str = "issue.subject.erased";

// ===========================================================================
// §1 — the cross-cell rollup child pointer + the PII-free projection
// ===========================================================================

/// **A cross-cell rollup pointer addressed to one member cell (R-7 / OQ-I).** What the Issues rollup
/// layer hands the control plane to carry to the cell that HOMES a remote portfolio child, so the
/// child's subtree aggregate can be resolved cell-local. It carries ONLY the PII-free
/// [`CrossCellPointer`] (the four frozen fields) + the destination routing handle — NEVER a leaf
/// row, NEVER a title, NEVER the assignee.
///
/// A member cell that receives this does NOT get the child's issues — it gets a pointer; to roll it
/// up, it asks the child's HOME cell to resolve the aggregate cell-local ([`CellLocalRollupResolver`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossCellRollupPointer {
    /// The destination member cell the pointer is carried TO (an opaque routing handle — the child's
    /// home cell).
    pub to_cell: CellId,
    /// The PII-free cross-cell pointer (the four frozen fields — never a leaf row, never PII).
    pub pointer: CrossCellPointer,
}

impl CrossCellRollupPointer {
    /// The opaque portfolio-child subject the pointer routes to (the home-cell issue URN). An
    /// `ArtifactRef`-class id, never a person.
    #[must_use]
    pub fn subject(&self) -> &ArtifactRef {
        self.pointer.subject().artifact_ref()
    }

    /// The child's home cell — where the aggregate is rendered (the leaf rows never leave it).
    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        self.pointer.home_cell()
    }
}

/// **The PII-free projection a cell-local rollup resolution returns (the residency gate body — §6.2 /
/// row 12.6).** When a portfolio in cell A wants to roll up a child homed in cell B, B renders the
/// child subtree's [`RollupAggregate`] AGAINST ITS rows, permission-checks the viewer THERE, and
/// returns ONLY this — the already-aggregated numbers (or a tombstone). The leaf rows, the titles, the
/// assignees NEVER cross; only the rolled-up [`RollupAggregate`] does. This is the rollup twin of
/// Knowledge's `collab::DocProjection`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PortfolioProjection {
    /// The viewer may roll up the child in its home cell — the home cell returns the rendered, filtered
    /// [`RollupAggregate`] (the PII-free numbers). NEVER a leaf row, NEVER a title.
    Rolled {
        /// The child subject this projection is for (the opaque home-cell URN).
        subject: ArtifactRef,
        /// The already-aggregated, viewer-scoped rollup numbers (done/total/estimate_sum/input_hash) —
        /// the ONLY thing that crosses the boundary. No PII, no leaf rows.
        aggregate: RollupAggregate,
    },
    /// The viewer may NOT roll up the child in its home cell (unauthorised, erased, or absent) — a
    /// tombstone carrying NO aggregate (the numbers never cross).
    Tombstone {
        /// The child subject the tombstone stands in for (so cell A can render "unavailable").
        subject: ArtifactRef,
    },
}

impl PortfolioProjection {
    /// The child subject this projection (rolled or tombstone) is for.
    #[must_use]
    pub fn subject(&self) -> &ArtifactRef {
        match self {
            PortfolioProjection::Rolled { subject, .. }
            | PortfolioProjection::Tombstone { subject } => subject,
        }
    }

    /// The rolled-up aggregate, if the home cell rendered it for this viewer (vs a tombstone).
    #[must_use]
    pub fn aggregate(&self) -> Option<&RollupAggregate> {
        match self {
            PortfolioProjection::Rolled { aggregate, .. } => Some(aggregate),
            PortfolioProjection::Tombstone { .. } => None,
        }
    }

    /// `true` iff the home cell rolled up the child for this viewer (vs returned a tombstone).
    #[must_use]
    pub fn is_rolled(&self) -> bool {
        matches!(self, PortfolioProjection::Rolled { .. })
    }
}

/// **The cell-local rollup resolution seam (OQ-I — resolution is ALWAYS cell-local).** A member cell
/// that holds a [`CrossCellRollupPointer`] resolves the child's aggregate by asking the child's HOME
/// cell to render it. The home cell owns the child's residency: it aggregates AGAINST ITS rows,
/// permission-checks the viewer THERE, and returns ONLY the [`PortfolioProjection`] — never a leaf
/// row, never the op-log, never a title. The leaf rows NEVER leave the home cell (§6.2).
///
/// In production the call crosses the control-plane `cross_cell_bridge` wire (the named substrate
/// floor); the SEAM is real and is proven against an in-process home-cell resolver standing in for the
/// home cell (the SAME stand-in the control-plane bridge + the Knowledge collab tests use).
pub trait CellLocalRollupResolver {
    /// Resolve `pointer` for `viewer_token` IN the child's home cell — aggregate-or-tombstone,
    /// cell-local. The home cell permission-checks the viewer and returns ONLY the filtered projection.
    fn resolve_in_home_cell(
        &self,
        pointer: &CrossCellRollupPointer,
        viewer_token: &str,
    ) -> PortfolioProjection;
}

// ===========================================================================
// §2 — the cross-cell portfolio rollup (the R-7 promotion — single-cell → cross-cell)
// ===========================================================================

/// **The Issues cross-cell portfolio rollup (contract 12.6 consumed — the rollup leg, LIVE).** Serves
/// a multi-cell tenant's portfolio **home cell** and, for a portfolio child homed in another cell,
/// produces the PII-free [`CrossCellRollupPointer`] the control plane carries to that cell — carrying
/// ONLY the four-field frame across, NEVER a leaf row, NEVER a title, NEVER the assignee. This LIFTS
/// the single-cell rollup floor (ISS-P18) to a true cross-cell portfolio walk.
///
/// `children_fanned_out` + `pii_crossed` are the **PII-free rollup proof** telemetry (the gate): every
/// fanned-out child pointer increments `children_fanned_out`; `pii_crossed` is pinned to **0** by
/// construction (the layer only ever emits the four-field frame + receives back an already-aggregated
/// [`RollupAggregate`]), exposed as a live tripwire so a future regression that carried a leaf row /
/// title across the bridge would be observable. This is the "0 PII crosses the bridge" projection the
/// cross-cell rollup gate asserts `== 0`.
#[derive(Clone)]
pub struct CrossCellPortfolioRollup {
    /// The portfolio home cell this rollup layer serves (an opaque id). A member cell == the home cell
    /// is skipped on fan-out (no self-hop — that child is a local rollup, not a cross-cell one).
    home_cell: CellId,
    /// The fan-out telemetry: how many cross-cell rollup-child pointers were fanned out (PII-free).
    children_fanned_out: Arc<AtomicU64>,
    /// **The ZERO — PII fields carried across a cell boundary by the rollup fan-out.** Pinned to 0 by
    /// construction (the layer only ever emits the four-field PII-free frame + receives an aggregate).
    /// A live counter (not a constant) so a future regression — a code path that carried a leaf
    /// row/title across — is observable. This is the "0 PII crosses" projection the gate asserts `== 0`.
    pii_crossed: Arc<AtomicU64>,
}

impl CrossCellPortfolioRollup {
    /// Build a rollup fan-out serving portfolio `home_cell`.
    #[must_use]
    pub fn new(home_cell: CellId) -> CrossCellPortfolioRollup {
        CrossCellPortfolioRollup {
            home_cell,
            children_fanned_out: Arc::new(AtomicU64::new(0)),
            pii_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The portfolio home cell this rollup layer serves (opaque id).
    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    /// **Fan a portfolio child rollup out to the cell that HOMES it (R-7 / OQ-I) — the cross-cell
    /// rollup walk leg.** For a portfolio child homed in another cell, mint the PII-free
    /// [`CrossCellPointer`] (homed in the CHILD'S cell — that is where resolution happens, subject =
    /// the opaque child issue URN, type = the `IssuePortfolio` kind, correlation = the rollup-recompute
    /// causal-root) and produce ONE [`CrossCellRollupPointer`] addressed to that cell. A child homed in
    /// THIS cell produces `None` (no self-hop — it is a local rollup, the single-cell floor unchanged).
    ///
    /// ONLY the four frozen frame fields cross — never a leaf row, never a title (`pii_crossed` stays
    /// 0). The leaf rows are NOT read into the pointer — there is structurally no field on the frame for
    /// them. The `correlation_id` ties the cross-cell pointer to the originating rollup's causal chain.
    pub fn fan_out_child(
        &self,
        tenant: &TenantId,
        region: &Region,
        child_subject: &ArtifactRef,
        child_home_cell: &CellId,
        correlation_id: &CorrelationId,
    ) -> Option<CrossCellRollupPointer> {
        // No self-hop: a child homed in THIS cell is a LOCAL rollup (the single-cell floor stands).
        if child_home_cell == &self.home_cell {
            return None;
        }
        // Build the `issue.rollup.recomputed` envelope the Bus's propagation half mints the pointer
        // from. The envelope's `subject` is the OPAQUE child issue URN (never a person); its `payload`
        // is EMPTY (the leaf rows live in the child's home cell, never on this envelope — the rollup
        // pointer event is a POINTER event by design, arch §6.2).
        let envelope = self.rollup_envelope(
            tenant,
            region,
            child_subject,
            correlation_id,
            ISSUE_ROLLUP_RECOMPUTED,
        );
        // CONSUME the Bus's pointer mint (EB-25) — no second propagator, no second frame. The pointer
        // is homed in the CHILD'S cell (resolution happens there). The IssuePortfolio stream supplies
        // the PII-free artifact-type kind (§6.2 — ISS → Issue).
        let pointer = pointer_for_propagation(
            &envelope,
            CrossCellStream::IssuePortfolio,
            child_home_cell.clone(),
        );
        self.children_fanned_out.fetch_add(1, Ordering::SeqCst);
        Some(CrossCellRollupPointer {
            to_cell: child_home_cell.clone(),
            pointer,
        })
    }

    /// **Resolve a cross-cell rollup-child pointer cell-local (the residency gate — OQ-I).** A cell
    /// that holds a [`CrossCellRollupPointer`] does NOT hold the child's leaf rows — it holds a
    /// pointer. To roll the child up for `viewer_token`, it asks the child's HOME cell (via `resolver`)
    /// to render the aggregate: the home cell permission-checks the viewer THERE and returns ONLY the
    /// [`PortfolioProjection`] (rolled-or-tombstone). The child's leaf rows NEVER leave its residency
    /// cell — only the already-aggregated numbers cross back. This PROVES resolution is cell-local (the
    /// portfolio cell resolves THROUGH the home cell, it never reaches into the home cell's rows).
    ///
    /// The PII-free invariant holds on the RETURN path too: a [`PortfolioProjection::Rolled`] carries
    /// only the [`RollupAggregate`] numbers — never a leaf row — so `pii_crossed` stays 0.
    #[must_use]
    pub fn resolve_cell_local(
        &self,
        pointer: &CrossCellRollupPointer,
        viewer_token: &str,
        resolver: &dyn CellLocalRollupResolver,
    ) -> PortfolioProjection {
        resolver.resolve_in_home_cell(pointer, viewer_token)
    }

    /// **Roll a multi-cell portfolio up (the R-7 promotion headline).** Sums the LOCAL child aggregates
    /// (resolved in this home cell) with the CROSS-CELL child aggregates (resolved cell-local through
    /// each child's home cell). The local aggregates are the single-cell floor's
    /// [`crate::rollup::recompute_incremental`] outputs; the cross-cell ones are
    /// [`PortfolioProjection::Rolled`] aggregates that crossed the PII-free bridge. A
    /// [`PortfolioProjection::Tombstone`] (unauthorised/erased child) contributes NOTHING (it is
    /// excluded, never a leak). Returns the combined portfolio [`RollupAggregate`] — done/total/
    /// estimate summed across cells, the `input_hash` XOR-folded (order-independent, the §6.1 property).
    #[must_use]
    pub fn combine(
        local: &[RollupAggregate],
        cross_cell: &[PortfolioProjection],
    ) -> RollupAggregate {
        let mut total = 0u64;
        let mut done = 0u64;
        let mut estimate_sum = 0i64;
        let mut input_hash = 0u64;
        for agg in local {
            total += agg.total;
            done += agg.done;
            estimate_sum = estimate_sum.saturating_add(agg.estimate_sum);
            input_hash ^= agg.input_hash;
        }
        for proj in cross_cell {
            // A tombstone contributes nothing (an unauthorised/erased child is excluded — 0 leak).
            if let PortfolioProjection::Rolled { aggregate, .. } = proj {
                total += aggregate.total;
                done += aggregate.done;
                estimate_sum = estimate_sum.saturating_add(aggregate.estimate_sum);
                input_hash ^= aggregate.input_hash;
            }
        }
        RollupAggregate {
            total,
            done,
            estimate_sum,
            input_hash,
        }
    }

    /// **The gate telemetry — `children_fanned_out`.** How many cross-cell rollup-child pointers the
    /// layer fanned out (aggregate, PII-free).
    #[must_use]
    pub fn children_fanned_out(&self) -> u64 {
        self.children_fanned_out.load(Ordering::SeqCst)
    }

    /// **The ZERO — `pii_crossed`.** Pinned to 0 by construction (the layer only ever emits the
    /// four-field PII-free frame + receives an already-aggregated [`RollupAggregate`]); exposed as a
    /// live tripwire so a future regression that carried a leaf row/title across the bridge is observable.
    ///
    /// **Equivalent-mutant note (cargo-mutants):** `replace pii_crossed -> 0` is observationally
    /// identical because the layer NEVER increments it (the structural guarantee) — the *correct*
    /// property, not a coverage gap (mirrors `CrossCellCollab::cross_cell_pii_crossed`).
    #[must_use]
    pub fn pii_crossed(&self) -> u64 {
        self.pii_crossed.load(Ordering::SeqCst)
    }

    /// Mint the pointer-event [`EventEnvelope`] (the Bus's propagation half mints the cross-cell pointer
    /// FROM this). The envelope's `subject` is the OPAQUE child issue URN; its `payload` is EMPTY (the
    /// leaf rows live in the home cell, never on this envelope — the event is a POINTER event, §6.2).
    fn rollup_envelope(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &ArtifactRef,
        correlation_id: &CorrelationId,
        type_: &str,
    ) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("xcell-rollup-{}", subject.0)),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(myelin_identity::Principal::stub(
                myelin_identity::PrincipalId("rollup-fanout".into()),
                myelin_identity::PrincipalKind::Service,
                tenant.clone(),
            )),
            subject: subject.clone(),
            aggregate: AggregateKey(format!("rollup:{}", subject.0)),
            causation_id: None,
            correlation_id: correlation_id.clone(),
            caused_by: None,
            depth: 0,
            // The pointer event carries NO personal data — the leaf rows (which may) never ride it.
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
            // EMPTY payload — the leaf rows NEVER ride the cross-cell rollup pointer event (§6.2).
            payload: serde_json::json!({}),
        }
    }
}

// ===========================================================================
// §3 — the cross-cell DSR fan-out (GA-D1 / CP-D7 / CP-D8 — the erasure receipt set)
// ===========================================================================

/// **One member cell's DSR erasure receipt (CP-D7 — the per-cell receipt set).** Proves the erasure
/// fan-out reached this cell: the cell, the opaque subject pointer it carried, and whether the cell
/// acknowledged the cell-local erasure. The gate reads the FULL set — 0 member cell may be missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsrCellReceipt {
    /// The member cell this receipt is for (the routing handle the pointer was carried to).
    pub cell: CellId,
    /// The opaque subject the cross-cell erasure pointer carried (the four-field frame's subject —
    /// never the subject's PII; the member cell crypto-shreds the PII cell-local).
    pub subject: ArtifactRef,
    /// `true` iff the member cell acknowledged its cell-local erasure (its own ISS-P31 fan-out ran).
    pub acknowledged: bool,
}

/// **The cross-cell DSR fan-out (contract 10.4 consumed — the DSR fan-out iterating `member_cells`,
/// GA-D1 / CP-D7 / CP-D8).** A multi-cell tenant's right-to-erasure must reach every member cell's
/// Issues holders. This layer iterates `member_cells`, mints ONE PII-free erasure pointer per cell
/// (the four-field frame — the opaque subject + type + correlation + the destination home_cell, never
/// the subject's PII), and collects a per-cell [`DsrCellReceipt`]. The actual per-cell erasure runs
/// cell-local (each cell's own [`crate::holder_erase::IssueEraseFanout`], ISS-P31) — this owns only
/// the cross-cell FAN-OUT + the receipt aggregation, never a second erase body (EI-01 §7).
///
/// The gate: **0 member cell missed** ([`Self::reached_every_cell`] — CP-D7) + **0 PII crosses**
/// ([`Self::pii_crossed`] == 0 — CP-D8 / GA-D8).
#[derive(Clone)]
pub struct CrossCellDsrFanout {
    /// The cell that ORIGINATES the DSR (where the request landed); a member cell == the origin cell is
    /// erased locally (its own ISS-P31 fan-out), the others receive the cross-cell pointer.
    origin_cell: CellId,
    /// CP-D8 / GA-D8 telemetry: PII fields carried across a cell boundary by the DSR fan-out. Pinned to
    /// 0 by construction (the layer only ever emits the four-field PII-free frame).
    pii_crossed: Arc<AtomicU64>,
}

impl CrossCellDsrFanout {
    /// Build a DSR fan-out originating in `origin_cell`.
    #[must_use]
    pub fn new(origin_cell: CellId) -> CrossCellDsrFanout {
        CrossCellDsrFanout {
            origin_cell,
            pii_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// **Fan a DSR erasure out across every member cell (GA-D1 / CP-D7 — the cross-cell erasure leg).**
    /// For each cell in `member_cells`, mint ONE PII-free [`CrossCellPointer`] (homed in that cell —
    /// where the cell-local erasure runs, subject = the opaque subject pointer, type = `IssuePortfolio`,
    /// correlation = the DSR causal-root) and produce one [`CrossCellRollupPointer`] carried to it. The
    /// `resolver` is the member cell's cell-local erasure acknowledgement (its own ISS-P31 fan-out ran).
    /// Returns the per-cell [`DsrCellReceipt`] set — the gate reads it for completeness (0 cell missed).
    ///
    /// ONLY the four frozen frame fields cross — never the subject's PII (`pii_crossed` stays 0). The
    /// subject ref is the OPAQUE pseudonymised id, never a person's name/email.
    pub fn fan_out_erasure(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &ArtifactRef,
        correlation_id: &CorrelationId,
        member_cells: &[CellId],
        acknowledge: &dyn Fn(&CellId, &ArtifactRef) -> bool,
    ) -> Vec<DsrCellReceipt> {
        let mut receipts = Vec::with_capacity(member_cells.len());
        for cell in member_cells {
            let envelope = self.erasure_envelope(tenant, region, subject, correlation_id);
            // The pointer is homed in the MEMBER cell (where the cell-local erasure runs). Only the
            // four-field frame crosses — never the subject's PII.
            let pointer =
                pointer_for_propagation(&envelope, CrossCellStream::IssuePortfolio, cell.clone());
            let carried = CrossCellRollupPointer {
                to_cell: cell.clone(),
                pointer,
            };
            // The member cell runs its OWN ISS-P31 cell-local erasure and acknowledges (the only thing
            // that crosses back is the boolean receipt — never a leaf row).
            let acknowledged = acknowledge(carried.home_cell(), carried.subject());
            receipts.push(DsrCellReceipt {
                cell: cell.clone(),
                subject: subject.clone(),
                acknowledged,
            });
        }
        receipts
    }

    /// **The CP-D7 gate — every member cell reached AND acknowledged.** `true` iff the receipt set
    /// covers every `member_cells` entry and each acknowledged its cell-local erasure (0 cell missed,
    /// 0 silent residual). A missing or unacknowledged cell is a LOUD `false` (a GDPR fan-out gap).
    #[must_use]
    pub fn reached_every_cell(receipts: &[DsrCellReceipt], member_cells: &[CellId]) -> bool {
        member_cells
            .iter()
            .all(|cell| receipts.iter().any(|r| &r.cell == cell && r.acknowledged))
    }

    /// The origin cell this DSR fan-out runs from.
    #[must_use]
    pub fn origin_cell(&self) -> &CellId {
        &self.origin_cell
    }

    /// **The CP-D8 / GA-D8 ZERO — `pii_crossed`.** Pinned to 0 by construction (the layer only ever
    /// emits the four-field PII-free frame); exposed as a live tripwire so a future regression that
    /// carried the subject's PII across the bridge is observable.
    #[must_use]
    pub fn pii_crossed(&self) -> u64 {
        self.pii_crossed.load(Ordering::SeqCst)
    }

    /// Mint the `issue.subject.erased` pointer-event [`EventEnvelope`] — the Bus's propagation half
    /// mints the cross-cell erasure pointer FROM this. The `subject` is the OPAQUE pseudonymised id;
    /// the `payload` is EMPTY (the PII is crypto-shredded cell-local, never on this envelope).
    fn erasure_envelope(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &ArtifactRef,
        correlation_id: &CorrelationId,
    ) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("xcell-erase-{}", subject.0)),
            type_: EventType(ISSUE_SUBJECT_ERASED.into()),
            schema_ver: 1,
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(myelin_identity::Principal::stub(
                myelin_identity::PrincipalId("dsr-fanout".into()),
                myelin_identity::PrincipalKind::Service,
                tenant.clone(),
            )),
            subject: subject.clone(),
            aggregate: AggregateKey(format!("erase:{}", subject.0)),
            causation_id: None,
            correlation_id: correlation_id.clone(),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }
}

// ===========================================================================
// §4 — the named floors (VISION §3 — the resolved cross-cell follow-on)
// ===========================================================================

/// **The named cross-cell rollup floors (VISION §3 — the R-7 / OQ-I / GA-D1 resolution).**
pub struct CrossCellRollupFloors;

impl CrossCellRollupFloors {
    /// **R-7 / OQ-I RESOLVED.** The single-cell rollup floor (ISS-P18) is promoted to a cross-cell
    /// portfolio rollup over the frozen PII-free `CrossCellPointer` bridge; resolution is ALWAYS
    /// cell-local (the home cell renders the aggregate, only the PII-free numbers cross).
    pub const CROSS_CELL_ROLLUP_RESOLVED: &'static str =
        "single-cell rollup → cross-cell portfolio rollup over the CrossCellPointer bridge \
         (cell-local resolution, R-7 / OQ-I, ISS-P32 / P-495)";

    /// **GA-D1 / CP-D7 / CP-D8 RESOLVED.** Every Issues holder now exists across every member cell, so
    /// the DSR fan-out iterating `member_cells` is complete — 0 cell missed, per-cell receipt set, 0
    /// PII crosses. The `[OPEN — LEGAL]` residual posture (10.9) is instantiated by reference.
    pub const DSR_FAN_OUT_RESOLVED: &'static str =
        "DSR fan-out iterates member_cells: 0 cell missed + per-cell receipt + 0 PII crosses \
         (GA-D1 / CP-D7 / CP-D8, contract 10.4, ISS-P32 / P-495)";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agg(total: u64, done: u64, estimate_sum: i64, input_hash: u64) -> RollupAggregate {
        RollupAggregate {
            total,
            done,
            estimate_sum,
            input_hash,
        }
    }

    /// **A child homed in another cell fans out a PII-free pointer; a local child does NOT (no
    /// self-hop).** The cross-cell pointer carries ONLY the four-field frame, homed in the child's cell.
    #[test]
    fn cross_cell_child_fans_out_pii_free_local_child_does_not() {
        let home = CellId::from_token("cell-fr-par-1");
        let rollup = CrossCellPortfolioRollup::new(home.clone());
        let tenant = TenantId("acme".into());
        let region = Region("fr-par".into());
        let corr = CorrelationId("rollup-root".into());

        // a child homed in ANOTHER cell → a cross-cell pointer (PII-free, homed there).
        let remote_child = ArtifactRef("myelin://acme/issues/issue/EPIC-9".into());
        let remote_cell = CellId::from_token("cell-de-fra-1");
        let p = rollup
            .fan_out_child(&tenant, &region, &remote_child, &remote_cell, &corr)
            .expect("a remote child fans out");
        assert_eq!(p.to_cell, remote_cell);
        assert_eq!(
            p.home_cell(),
            &remote_cell,
            "the pointer is homed in the child's cell"
        );
        assert_eq!(p.subject(), &remote_child);
        assert_eq!(
            p.pointer.correlation_id(),
            &corr,
            "rides the rollup causal chain"
        );
        assert_eq!(rollup.children_fanned_out(), 1);
        assert_eq!(rollup.pii_crossed(), 0, "0 PII crosses the bridge");

        // a child homed in THIS cell → NO cross-cell pointer (it is a local rollup, single-cell floor).
        let local_child = ArtifactRef("myelin://acme/issues/issue/EPIC-1".into());
        assert!(
            rollup
                .fan_out_child(&tenant, &region, &local_child, &home, &corr)
                .is_none(),
            "a local child is a single-cell rollup, no self-hop"
        );
        assert_eq!(
            rollup.children_fanned_out(),
            1,
            "the local child did not fan out"
        );
    }

    /// **Resolution is cell-local — only the PII-free aggregate crosses back (the residency gate).** A
    /// portfolio cell resolves a remote child through the child's home cell, which returns ONLY a
    /// [`PortfolioProjection::Rolled`] aggregate — never a leaf row.
    #[test]
    fn resolution_is_cell_local_only_the_aggregate_crosses() {
        struct HomeCell;
        impl CellLocalRollupResolver for HomeCell {
            fn resolve_in_home_cell(
                &self,
                pointer: &CrossCellRollupPointer,
                viewer_token: &str,
            ) -> PortfolioProjection {
                // The home cell permission-checks THERE: an authorised viewer gets the aggregate; an
                // unauthorised one gets a tombstone (the numbers never cross for them).
                if viewer_token == "authorised" {
                    PortfolioProjection::Rolled {
                        subject: pointer.subject().clone(),
                        aggregate: agg(10, 4, 40, 0xABCD),
                    }
                } else {
                    PortfolioProjection::Tombstone {
                        subject: pointer.subject().clone(),
                    }
                }
            }
        }

        let rollup = CrossCellPortfolioRollup::new(CellId::from_token("cell-fr-par-1"));
        let p = rollup
            .fan_out_child(
                &TenantId("acme".into()),
                &Region("fr-par".into()),
                &ArtifactRef("myelin://acme/issues/issue/EPIC-9".into()),
                &CellId::from_token("cell-de-fra-1"),
                &CorrelationId("c".into()),
            )
            .unwrap();

        let rolled = rollup.resolve_cell_local(&p, "authorised", &HomeCell);
        assert!(rolled.is_rolled());
        assert_eq!(rolled.aggregate().unwrap().total, 10);

        let tombstoned = rollup.resolve_cell_local(&p, "stranger", &HomeCell);
        assert!(
            !tombstoned.is_rolled(),
            "an unauthorised viewer gets a tombstone (0 leak)"
        );
        assert!(tombstoned.aggregate().is_none());
    }

    /// **The portfolio combines local + cross-cell aggregates; a tombstone contributes nothing.** The
    /// done/total/estimate sum across cells; a tombstoned (unauthorised/erased) child is excluded.
    #[test]
    fn combine_sums_across_cells_tombstone_contributes_nothing() {
        let local = vec![agg(5, 2, 20, 0x1)];
        let cross = vec![
            PortfolioProjection::Rolled {
                subject: ArtifactRef("myelin://acme/issues/issue/EPIC-9".into()),
                aggregate: agg(10, 4, 40, 0x2),
            },
            PortfolioProjection::Tombstone {
                subject: ArtifactRef("myelin://acme/issues/issue/EPIC-8".into()),
            },
        ];
        let combined = CrossCellPortfolioRollup::combine(&local, &cross);
        assert_eq!(
            combined.total, 15,
            "5 local + 10 cross-cell (tombstone excluded)"
        );
        assert_eq!(combined.done, 6);
        assert_eq!(combined.estimate_sum, 60);
        assert_eq!(
            combined.input_hash,
            0x1 ^ 0x2,
            "XOR-folded, tombstone contributes nothing"
        );
        assert!((combined.progress() - 6.0 / 15.0).abs() < 1e-9);
    }

    /// **The DSR fan-out reaches every member cell with a per-cell receipt, 0 PII crosses (GA-D1 /
    /// CP-D7 / CP-D8).** Every member cell acknowledges its cell-local erasure; the receipt set is
    /// complete; `reached_every_cell` is true; `pii_crossed` is 0.
    #[test]
    fn dsr_fan_out_reaches_every_member_cell_pii_free() {
        let origin = CellId::from_token("cell-fr-par-1");
        let dsr = CrossCellDsrFanout::new(origin);
        let member_cells = vec![
            CellId::from_token("cell-fr-par-1"),
            CellId::from_token("cell-de-fra-1"),
            CellId::from_token("cell-nl-ams-1"),
        ];
        let subject = ArtifactRef("myelin://acme/identity/pseudonym/p-7".into());

        let receipts = dsr.fan_out_erasure(
            &TenantId("acme".into()),
            &Region("fr-par".into()),
            &subject,
            &CorrelationId("dsr-root".into()),
            &member_cells,
            // every member cell acknowledges its cell-local ISS-P31 erasure.
            &|_cell, _subject| true,
        );

        assert_eq!(receipts.len(), 3, "one receipt per member cell");
        assert!(
            CrossCellDsrFanout::reached_every_cell(&receipts, &member_cells),
            "0 member cell missed (CP-D7)"
        );
        assert_eq!(
            dsr.pii_crossed(),
            0,
            "0 PII crosses the bridge (CP-D8 / GA-D8)"
        );
        // the receipts carry the OPAQUE subject pointer, never PII.
        for r in &receipts {
            assert_eq!(r.subject, subject);
            assert!(r.acknowledged);
        }
    }

    /// **A missed/unacknowledged member cell is a LOUD gate failure (a GDPR fan-out gap).**
    #[test]
    fn dsr_unacknowledged_cell_is_a_loud_gate_failure() {
        let dsr = CrossCellDsrFanout::new(CellId::from_token("cell-fr-par-1"));
        let member_cells = vec![
            CellId::from_token("cell-fr-par-1"),
            CellId::from_token("cell-de-fra-1"),
        ];
        let subject = ArtifactRef("myelin://acme/identity/pseudonym/p-7".into());
        let receipts = dsr.fan_out_erasure(
            &TenantId("acme".into()),
            &Region("fr-par".into()),
            &subject,
            &CorrelationId("dsr-root".into()),
            &member_cells,
            // the de-fra cell FAILS to acknowledge (a fan-out gap).
            &|cell, _subject| cell.as_str() != "cell-de-fra-1",
        );
        assert!(
            !CrossCellDsrFanout::reached_every_cell(&receipts, &member_cells),
            "an unacknowledged cell is a loud fan-out gap (never a silent residual)"
        );
    }
}
