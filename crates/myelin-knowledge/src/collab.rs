//! # `collab` — true cross-cell collab op fan-out over the PII-free CrossCellPointer bridge (KN-P30 / P-485, M5)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md`
//! §3.3/§3.4 (the CRDT op fan-out this cross-cell path EXTENDS — the transport is engine-agnostic, the
//! op-log + `op_seq` cursor + idempotent apply are identical for CAS and Yrs) +
//! `06-reconciliation-compliance.md` (row 12.6: *a doc's collab session is cell-pinned; the control
//! plane carries only the PII-free `CrossCellPointer`; resolution is cell-local*; R6-5: true cross-cell
//! collab op fan-out, KQ-7). **Contract:** `contract-index.md` row 12.6 (the cross-cell PII-free pointer
//! bridge — CONSUMED here for cross-cell fan-out; the frame is owned by `myelin-tenancy`, the
//! event-propagation half by `myelin-events::crosscell_propagation`). **Reconciliation:**
//! `00-reconciliation-decisions.md` OQ-I (multi-cell after single-cell — cross-cell op fan-out over the
//! bridge). **External insight:** `VISION.md` §3 (world-scale; EU-sovereign by construction — the bridge
//! is PII-free); `external-insights/01-process-and-quality-doctrine.md` §3 (prove-it: cross-cell fan-out
//! with 0 PII over the bridge, MEASURED not asserted), §7 (extend in place, never a second frame).
//!
//! ## The single-cell floor this LIFTS (KN-P05, named resolved)
//! v1 ([`crate::transport::CollabTransport`]) pinned a doc's collab session to ONE cell: the
//! `(tenant, page_id)` pin ([`crate::transport::CollabTransport::tenant`] documents *"the single-cell
//! collab floor, KN-P30 lifts the cross-cell pin"*). Every collaborator on a doc had to live in the
//! doc's home cell, because the op fan-out ([`crate::transport::CollabTransport::send_op`]) published
//! only on the cell-local firehose. **This module lifts that pin** to a true cross-cell op fan-out for a
//! **multi-cell tenant**: when the home cell applies a doc op, the collaborators in the tenant's *other*
//! cells learn an op occurred over the PII-free [`CrossCellPointer`] bridge — while the op bytes (the
//! Yrs/CAS payload, which may carry inline PII under a DEK ref, 01 §3) and the rendered content NEVER
//! leave the doc's residency cell. The KN-P05 single-cell → cross-cell floor is **resolved**.
//!
//! ## What crosses the bridge (the residency invariant — §7.4 / row 12.6)
//! **0 PII crosses a cell boundary.** Exactly ONE thing crosses per fan-out: the four-field
//! [`CrossCellPointer`] (`subject` opaque [`crate::emit::page_ref`] / `type` PII-free kind /
//! `correlation_id` causal-root / `home_cell` routing handle). What does NOT cross, by construction:
//! - the [`crate::transport::DocOp::payload`] (the Yrs/CAS op bytes — there is no field on the frame for
//!   it to go);
//! - the [`crate::transport::DocOp::pii_key_ref`] / any inline-PII DEK material;
//! - the actor, the op kind, the rendered block content — none of it. A member cell that receives the
//!   pointer holds ONLY a pointer; it cannot reconstruct the doc from what crossed.
//!
//! ## Resolution stays cell-local (the residency gate — row 12.6, R6-5)
//! A collaborator in a member cell does NOT receive the doc content. When they want to render the
//! cross-cell doc, the member cell asks the doc's **home cell** to resolve it ([`CellLocalDocResolution`]):
//! the home cell renders the doc against ITS rows + permission-checks the viewer THERE, and returns ONLY
//! the already-filtered projection (or a tombstone) — never a raw row, never the op-log, never a payload.
//! The doc's content never leaves its residency cell (§6.2). This is the SAME cell-local-resolution
//! discipline the control-plane `cross_cell_bridge` (P-429) and the search federated path (P-464) hold —
//! reused here, never re-invented (EI-01 §7).
//!
//! ## EI-01 §7 reconciliation — no second frame, no second propagator
//! This module CONSUMES the already-built cross-cell machinery rather than re-defining it:
//! - the **frame** is `myelin_events::CrossCellPointer` (the `myelin-tenancy` authority, re-exported on
//!   the frozen Bus path) — NOT re-defined;
//! - the **event-propagation half** is `myelin_events::crosscell_propagation::CrossCellPropagator` +
//!   [`pointer_for_propagation`] (EB-25 / P-438) — the Bus mints the pointer + selects the member-cell
//!   fan-out; Knowledge's collab layer drives it for the `knowledge.doc.updated` op stream
//!   ([`CrossCellStream::KnowledgeCollab`]) and adds the doc-collab-shaped seam (lift the session pin,
//!   resolve cell-local). There is NO second `CrossCellPropagator`, NO second pointer frame.
//!
//! ## DAG position (why the fan-out PRODUCTION lives here, not the control plane)
//! `myelin-knowledge` depends on `myelin-events` (ABOVE `myelin-control-plane` in the §2.9 DAG). So the
//! Bus owns the pointer-event *production* (`crosscell_propagation`); the control plane CONSUMES the
//! produced pointer and carries it cell→cell over the wire. Knowledge's collab layer SITS ON the Bus's
//! production: it classifies a doc op into the `KnowledgeCollab` stream and fans the PII-free pointer to
//! the tenant's other member cells. The actual cell→cell wire is the control-plane transport (the named
//! floor below — the same `ResilientClient` wire every cross-cell consumer rides).
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **The control-plane transport wire** (the actual cell→cell carriage of the pointer + the cell-local
//!   `resolve()` round-trip) is the control plane's `cross_cell_bridge` + the resilient-client transport
//!   (P-429/P-437); this module produces the pointer-events the wire carries and drives the resolution
//!   seam against an in-process home-cell resolver standing in for the home cell (the SAME stand-in the
//!   refs cross-cell fan-out + the control-plane bridge tests + the search federated path use). The
//!   cross-process WIRE is the substrate floor — NOT re-built here.
//! - **The member-cell ENUMERATION is the control plane's `placement_of`/`member_cells` fan-out**
//!   (P-CP-20 / P-430). This module fans out to a caller-supplied member-cell set; the
//!   `placement_of`-driven enumeration that PRODUCES the set lives in the control plane.
//!
//! ## The mandatory-core mutation floor (EI-01 §3 / VISION §4 prove-it)
//! A PII leak across the bridge is a **sovereignty breach** — the cross-cell-PII-free discipline is
//! mandatory-core. The cargo-mutants mutation-score floor for this module is **100% caught** on the
//! fan-out + residency seams: any mutant that lets a non-pointer field cross the bridge, that fails to
//! lift the single-cell pin (drops a member cell), that self-hops the home cell, or that lets the
//! cell-local resolver leak a raw row instead of a projection is KILLED by
//! [`tests::cross_cell_fan_out_carries_only_the_pointer_zero_pii`] /
//! [`tests::resolution_stays_cell_local_only_the_projection_crosses`] /
//! [`tests::single_cell_pin_lifted_fan_out_reaches_every_other_member_cell`]. (The `pii_crossed == 0`
//! read is the documented equivalent-mutant: `replace -> 0` is observationally identical because the
//! layer NEVER increments it — the *correct* property, the tripwire stays wired.)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_content::events::KNOWLEDGE_DOC_UPDATED;
use myelin_events::crosscell_propagation::{
    pointer_for_propagation, CrossCellStream, PropagatedPointer,
};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CellId, CorrelationId, CrossCellPointer, DataRole,
    EventEnvelope, EventId, EventType, OpaqueSubjectId, Timestamp, Visibility,
};
use myelin_identity::Principal;
use myelin_tenancy::{Region, TenantId};

use crate::emit::page_ref;
use crate::transport::DocOp;

/// **A cross-cell doc op the home cell fans out (KQ-7 / R6-5).** Pairs the doc identity (`tenant`,
/// `page_id`) the op belongs to with the [`DocOp`] the transport applied. This is the input to the
/// cross-cell fan-out: the home cell has already applied the op to its op-log
/// ([`crate::transport::CollabTransport::send_op`]); this is the cross-cell *notification* fan-out that
/// rides on top. The op `payload` (Yrs/CAS bytes, possibly DEK-wrapped inline PII) is held so the
/// fan-out can PROVE it never crosses — it is read only to assert its absence on the wire.
#[derive(Clone, Debug)]
pub struct CrossCellDocOp<'a> {
    /// The multi-cell tenant the doc belongs to.
    pub tenant: &'a TenantId,
    /// The doc's `page_id` (the bounded scope's resource id — the single-cell pin this lifts).
    pub page_id: &'a str,
    /// The op the home cell applied (its payload/PII never crosses — only the pointer does).
    pub op: &'a DocOp,
}

/// **A cross-cell doc-op pointer addressed to one member cell.** What the Knowledge collab layer hands
/// the control plane to carry to one of the tenant's *other* cells so a collaborator THERE learns a doc
/// op occurred. It carries ONLY the PII-free [`CrossCellPointer`] (the four frozen fields) + the
/// destination routing handle — NEVER the op payload, NEVER inline PII, NEVER the rendered content.
///
/// A member cell that receives this does not get the doc — it gets a pointer; to render it, it asks the
/// home cell to resolve cell-local ([`CellLocalDocResolution`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossCellDocPointer {
    /// The destination member cell the pointer is carried TO (an opaque routing handle).
    pub to_cell: CellId,
    /// The PII-free cross-cell pointer (the four frozen fields — never payload, never PII).
    pub pointer: CrossCellPointer,
}

impl CrossCellDocPointer {
    /// The opaque doc subject the pointer routes to (the home-cell [`crate::emit::page_ref`] URN —
    /// `myelin://<tenant>/knowledge/page/<page_id>`). An `ArtifactRef`-class id, never a person.
    #[must_use]
    pub fn subject(&self) -> &ArtifactRef {
        self.pointer.subject().artifact_ref()
    }

    /// The doc's home cell — where resolution happens (the content never leaves it).
    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        self.pointer.home_cell()
    }
}

/// **The PII-free projection a cell-local resolution returns (the residency gate body — row 12.6).**
/// When a collaborator in a member cell wants to render a cross-cell doc, the doc's HOME cell renders it
/// against ITS rows, permission-checks the viewer THERE, and returns ONLY this — the already-filtered
/// projection (or a tombstone). The op-log, the raw rows, the payload bytes NEVER cross; only the
/// rendered, viewer-scoped projection does (§6.2). This is the doc-collab twin of the control-plane
/// `cross_cell_bridge`'s `ResolvedProjection` + the search `FederatedRow` — the SAME cell-local discipline.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DocProjection {
    /// The viewer may render the doc in its home cell — the home cell returns the rendered, filtered
    /// projection (a doc-shaped opaque render token here; the real render is the home cell's editor
    /// path, KN-4). NEVER the raw op-log or payload.
    Rendered {
        /// The doc subject this projection is for (the opaque home-cell URN).
        subject: ArtifactRef,
        /// The already-rendered, viewer-scoped projection bytes (what the home cell chose to expose —
        /// never a raw row, never the op payload).
        rendered: String,
    },
    /// The viewer may NOT render the doc in its home cell (unauthorised, erased, or absent) — a
    /// tombstone carrying NO content (the secret never crosses).
    Tombstone {
        /// The doc subject the tombstone stands in for (so the member cell can render "unavailable").
        subject: ArtifactRef,
    },
}

impl DocProjection {
    /// The doc subject this projection (rendered or tombstone) is for.
    #[must_use]
    pub fn subject(&self) -> &ArtifactRef {
        match self {
            DocProjection::Rendered { subject, .. } | DocProjection::Tombstone { subject } => {
                subject
            }
        }
    }

    /// `true` iff the home cell rendered the doc for this viewer (vs returned a tombstone).
    #[must_use]
    pub fn is_rendered(&self) -> bool {
        matches!(self, DocProjection::Rendered { .. })
    }
}

/// **The cell-local resolution seam (row 12.6 — resolution is ALWAYS cell-local).** A member cell that
/// holds a [`CrossCellDocPointer`] resolves the doc by asking the doc's HOME cell to render it for a
/// specific viewer. The home cell owns the doc's residency: it renders against ITS rows, permission-
/// checks the viewer THERE, and returns ONLY the [`DocProjection`] (rendered-or-tombstone) — never a raw
/// row, never the op-log, never the payload. The content NEVER leaves the home cell (§6.2).
///
/// In production the call crosses the control-plane `cross_cell_bridge` wire (the named substrate floor);
/// the SEAM is real and is proven here against an in-process home-cell resolver standing in for the home
/// cell (the SAME stand-in the control-plane bridge + search federated tests use).
pub trait CellLocalDocResolution {
    /// Resolve `pointer` for `viewer` IN the doc's home cell — render-or-tombstone, cell-local. The
    /// home cell permission-checks `viewer` and returns ONLY the filtered projection.
    fn resolve_in_home_cell(
        &self,
        pointer: &CrossCellDocPointer,
        viewer: &Principal,
    ) -> DocProjection;
}

/// **The Knowledge cross-cell collab op fan-out (contract 12.6 consumed — the doc-collab leg, LIVE).**
/// Serves a multi-cell tenant's doc **home cell** and, for an applied doc op, fans the PII-free
/// [`CrossCellPointer`] out to the tenant's *other* member cells over the bridge — carrying ONLY the
/// four-field frame across, NEVER the op payload, NEVER inline PII, NEVER the rendered content. This
/// LIFTS the KN-P05 single-cell session pin to true cross-cell fan-out.
///
/// `ops_fanned_out` + `cross_cell_pii_crossed` are the **PII-free fan-out proof** telemetry (the gate):
/// every fanned-out pointer increments `ops_fanned_out`; `cross_cell_pii_crossed` is pinned to **0** by
/// construction (the layer only ever emits the four-field frame), exposed as a live tripwire so a future
/// regression that carried payload/PII across the bridge would be observable (it would tick above 0).
/// This is the "0 PII crosses the bridge" projection the cross-cell fan-out gate asserts `== 0`.
#[derive(Clone)]
pub struct CrossCellCollab {
    /// The doc home cell this collab layer serves (an opaque id). A member cell == the home cell is
    /// skipped on fan-out (no self-hop — that cell already applied the op locally).
    home_cell: CellId,
    /// The fan-out telemetry: how many cross-cell doc-op pointers were fanned out (PII-free).
    ops_fanned_out: Arc<AtomicU64>,
    /// **The ZERO — PII fields carried across a cell boundary by the collab fan-out.** Pinned to 0 by
    /// construction (the layer only ever emits the four-field PII-free frame). A live counter (not a
    /// constant) so a future regression — a code path that carried payload/PII across — is observable.
    /// This is the "0 PII crosses the bridge" projection the gate asserts `== 0`.
    cross_cell_pii_crossed: Arc<AtomicU64>,
}

impl CrossCellCollab {
    /// Build a collab fan-out serving doc `home_cell`.
    #[must_use]
    pub fn new(home_cell: CellId) -> CrossCellCollab {
        CrossCellCollab {
            home_cell,
            ops_fanned_out: Arc::new(AtomicU64::new(0)),
            cross_cell_pii_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The doc home cell this collab layer serves (opaque id).
    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    /// **Fan an applied doc op out to the tenant's other member cells (KQ-7 / R6-5) — the cross-cell op
    /// fan-out.** For the multi-cell tenant's doc, mint the PII-free [`CrossCellPointer`] (homed in THIS
    /// cell, subject = the opaque [`crate::emit::page_ref`] URN, type = the `KnowledgeCollab` kind,
    /// correlation = the op's causal-root) and produce one [`CrossCellDocPointer`] per `member_cell`
    /// that is **not** this home cell (no self-hop — that cell already applied the op locally). A
    /// single-home-cell tenant (only the home cell in `member_cells`) produces an EMPTY fan-out — the
    /// single-cell path is unchanged (the KN-P05 floor preserved for single-cell tenants).
    ///
    /// Across EVERY produced pointer, ONLY the four frozen frame fields cross — never the op payload,
    /// never inline PII (`cross_cell_pii_crossed` stays 0). The op's `payload`/`pii_key_ref` are NOT
    /// read into the pointer — there is structurally no field on the frame for them to go. The
    /// `correlation_id` ties the cross-cell pointers to the originating op's causal chain (BUS-5).
    pub fn fan_out_doc_op(
        &self,
        doc_op: &CrossCellDocOp<'_>,
        correlation_id: &CorrelationId,
        member_cells: &[CellId],
    ) -> Vec<CrossCellDocPointer> {
        // Build the `knowledge.doc.updated` envelope the Bus's propagation half mints the pointer from.
        // The envelope's `subject` is the OPAQUE page_ref URN (never a person); `correlation_id` is the
        // op's causal-root. The op payload/PII is NOT placed on the envelope subject — only the page id.
        let envelope = self.doc_updated_envelope(doc_op, correlation_id);
        // CONSUME the Bus's pointer mint (EB-25) — no second propagator, no second frame. The
        // KnowledgeCollab stream supplies the PII-free artifact-type kind.
        let pointer = pointer_for_propagation(
            &envelope,
            CrossCellStream::KnowledgeCollab,
            self.home_cell.clone(),
        );
        member_cells
            .iter()
            .filter(|to| **to != self.home_cell) // no self-hop — the home cell already applied it.
            .map(|to| {
                // Each produced pointer carries ONLY the four-field frame — 0 PII crosses the bridge.
                self.ops_fanned_out.fetch_add(1, Ordering::SeqCst);
                CrossCellDocPointer {
                    to_cell: to.clone(),
                    pointer: pointer.clone(),
                }
            })
            .collect()
    }

    /// Mint the `knowledge.doc.updated` [`EventEnvelope`] for a doc op — the Bus's propagation half mints
    /// the cross-cell pointer FROM this. The envelope's `subject` is the OPAQUE [`page_ref`] URN; its
    /// `payload` is EMPTY (the op bytes live in the home cell's op-log, never on this envelope — the
    /// `knowledge.doc.updated` event is a *pointer* event by design, arch §7 / ADR-04.5: the durable bus
    /// carries only the pointer, never the collab op bytes).
    fn doc_updated_envelope(
        &self,
        doc_op: &CrossCellDocOp<'_>,
        correlation_id: &CorrelationId,
    ) -> EventEnvelope {
        let subject = page_ref(doc_op.tenant, doc_op.page_id);
        EventEnvelope {
            event_id: EventId(format!("doc-op-{}", doc_op.op.op_id.wire())),
            type_: EventType(KNOWLEDGE_DOC_UPDATED.into()),
            schema_ver: 1,
            tenant: doc_op.tenant.clone(),
            // Region is the doc's residency region; the cross-cell pointer routes by `home_cell`.
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                myelin_identity::PrincipalId("collab-fanout".into()),
                myelin_identity::PrincipalKind::Service,
                doc_op.tenant.clone(),
            )),
            subject: subject.clone(),
            aggregate: AggregateKey(format!("page:{}", doc_op.page_id)),
            causation_id: None,
            correlation_id: correlation_id.clone(),
            caused_by: None,
            depth: 0,
            // The pointer event carries NO personal data — the op payload (which may) never rides it.
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
            // EMPTY payload — the collab op bytes NEVER ride the cross-cell pointer event (arch §7).
            payload: serde_json::json!({}),
        }
    }

    /// **Resolve a cross-cell doc pointer cell-local (the residency gate — row 12.6).** A member cell
    /// that received a [`CrossCellDocPointer`] does NOT hold the doc — it holds a pointer. To render the
    /// doc for `viewer`, it asks the doc's HOME cell (via `resolver`) to render it: the home cell
    /// permission-checks `viewer` THERE and returns ONLY the [`DocProjection`] (rendered-or-tombstone).
    /// The doc's content NEVER leaves its residency cell — only the already-filtered projection crosses
    /// back. This call PROVES resolution is cell-local (the member cell resolves THROUGH the home cell,
    /// it never reaches into the home cell's rows itself).
    #[must_use]
    pub fn resolve_cell_local(
        &self,
        pointer: &CrossCellDocPointer,
        viewer: &Principal,
        resolver: &dyn CellLocalDocResolution,
    ) -> DocProjection {
        // The home cell owns the doc's residency — it renders + permission-checks; we receive only the
        // projection. No raw row, no op-log, no payload crosses back (the resolver returns a projection).
        resolver.resolve_in_home_cell(pointer, viewer)
    }

    /// **The gate telemetry — `ops_fanned_out`.** How many cross-cell doc-op pointers the layer fanned
    /// out (aggregate, PII-free).
    #[must_use]
    pub fn ops_fanned_out(&self) -> u64 {
        self.ops_fanned_out.load(Ordering::SeqCst)
    }

    /// **The ZERO — `cross_cell_pii_crossed`.** Pinned to 0 by construction (the layer only ever emits
    /// the four-field PII-free frame); exposed as a live tripwire so a future regression that carried
    /// payload/PII across the bridge is observable.
    ///
    /// **Equivalent-mutant note (cargo-mutants):** `replace cross_cell_pii_crossed -> 0` is
    /// observationally identical because the layer NEVER increments it (the structural guarantee) — the
    /// *correct* property, not a coverage gap. The field + the read seam stay so the tripwire is wired
    /// the day a regression lands (mirrors `CrossCellPropagator::pii_fields_crossed`).
    #[must_use]
    pub fn cross_cell_pii_crossed(&self) -> u64 {
        self.cross_cell_pii_crossed.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossCellCollab {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // PII-free Debug: the home cell id + the aggregate counters, never a doc op / payload.
        f.debug_struct("CrossCellCollab")
            .field("home_cell", &self.home_cell.as_str())
            .field("ops_fanned_out", &self.ops_fanned_out())
            .field("cross_cell_pii_crossed", &self.cross_cell_pii_crossed())
            .finish()
    }
}

/// **The PII-free fan-out proof (the gate telemetry body).** What crosses the bridge for a fanned-out
/// doc-op pointer is EXACTLY the four frozen [`CrossCellPointer`] fields + the destination routing
/// handle — never the op payload, never inline PII. This helper extracts the (opaque, PII-free) fields a
/// fan-out drill asserts crossed, so a drill can show "the fan-out carried only
/// `subject`/`type`/`correlation_id`/`home_cell` + `to_cell`" with the concrete opaque values. It
/// returns the fields by reference — there is structurally no payload field to return.
#[must_use]
pub fn fanned_out_carried_fields(
    fanned: &CrossCellDocPointer,
) -> (
    &CellId,
    &OpaqueSubjectId,
    &myelin_events::ArtifactType,
    &CorrelationId,
    &CellId,
) {
    (
        &fanned.to_cell,
        fanned.pointer.subject(),
        fanned.pointer.artifact_type(),
        fanned.pointer.correlation_id(),
        fanned.pointer.home_cell(),
    )
}

/// Adapt a [`CrossCellDocPointer`] to the Bus's [`PropagatedPointer`] shape (the SAME four-field frame
/// under the `KnowledgeCollab` stream) — proving the Knowledge fan-out is the Bus's propagation, not a
/// parallel one (EI-01 §7). Used by the CDC pair to round-trip the doc-collab pointer through the frozen
/// 12.6 wire shape.
#[must_use]
pub fn as_propagated(fanned: &CrossCellDocPointer) -> PropagatedPointer {
    PropagatedPointer {
        to_cell: fanned.to_cell.clone(),
        pointer: fanned.pointer.clone(),
        stream: CrossCellStream::KnowledgeCollab,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{OpId, OpKind};
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn viewer() -> Principal {
        Principal::stub(
            PrincipalId("viewer-opaque".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    /// A doc op whose PAYLOAD carries inline PII (a DEK-wrapped run, 01 §3) — so the tests can prove the
    /// payload + the PII-key ref NEVER cross the bridge. The bytes spell a secret on the wire.
    fn op_with_pii() -> DocOp {
        let mut op = DocOp::cas(
            OpId::new("client-1", 7),
            "author-opaque",
            OpKind::Insert,
            b"alice@example.com SECRET BODY".to_vec(),
        );
        op.pii_key_ref = Some("dek:page-9:run-3".into());
        op
    }

    fn doc_op<'a>(op: &'a DocOp, page_id: &'a str) -> CrossCellDocOp<'a> {
        // The tenant is built fresh per call but compared by value (TenantId is a newtype String).
        CrossCellDocOp {
            tenant: Box::leak(Box::new(tenant())),
            page_id,
            op,
        }
    }

    /// An in-process home-cell resolver standing in for the doc's home cell (the SAME stand-in the
    /// control-plane bridge + search federated tests use). It renders the doc IFF the viewer is allowed
    /// THERE — and returns ONLY a projection, never a raw row / op-log / payload.
    struct HomeCellResolver {
        /// Which viewer ids the home cell permits to render (the cell-local permission check).
        allowed: Vec<String>,
        /// The rendered projection the home cell exposes (never the raw op-log / payload).
        rendered: String,
    }

    impl CellLocalDocResolution for HomeCellResolver {
        fn resolve_in_home_cell(
            &self,
            pointer: &CrossCellDocPointer,
            viewer: &Principal,
        ) -> DocProjection {
            let subject = pointer.subject().clone();
            if self.allowed.iter().any(|id| id == &viewer.principal_id.0) {
                DocProjection::Rendered {
                    subject,
                    rendered: self.rendered.clone(),
                }
            } else {
                // Unauthorised in the home cell → a tombstone with NO content (the secret never crosses).
                DocProjection::Tombstone { subject }
            }
        }
    }

    /// **THE FAN-OUT GATE — cross-cell op fan-out carries ONLY the PII-free pointer (0 PII crosses).** A
    /// multi-cell tenant's doc op (with a PII-bearing payload + a DEK ref) fans out to the tenant's other
    /// cells; each produced pointer carries ONLY the four frozen fields; the serialised frame contains
    /// neither the payload email/body nor the DEK ref; `cross_cell_pii_crossed == 0`.
    #[test]
    fn cross_cell_fan_out_carries_only_the_pointer_zero_pii() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-fr-par-1"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-9");
        let members = vec![
            CellId::from_token("cell-fr-par-1"), // the home cell — skipped (no self-hop).
            CellId::from_token("cell-de-1"),
            CellId::from_token("cell-nl-1"),
        ];
        let corr = CorrelationId("op-causal-root".into());
        let fanned = collab.fan_out_doc_op(&dop, &corr, &members);

        // One pointer per OTHER member cell (the home cell filtered out — no self-hop).
        let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
        assert_eq!(dests, vec!["cell-de-1", "cell-nl-1"]);
        assert_eq!(collab.ops_fanned_out(), 2);

        for pp in &fanned {
            let (to, subject, kind, corr_field, home) = fanned_out_carried_fields(pp);
            // The subject is the OPAQUE page URN — never the payload, never a person.
            assert_eq!(
                subject.artifact_ref().0,
                "myelin://acme/knowledge/page/page-9"
            );
            assert_eq!(kind, &myelin_events::ArtifactType::Page);
            assert_eq!(corr_field, &corr);
            assert_eq!(home.as_str(), "cell-fr-par-1");
            assert!(matches!(to.as_str(), "cell-de-1" | "cell-nl-1"));

            // The frame serialises to EXACTLY the four fields — the payload PII + DEK ref are absent.
            let wire = serde_json::to_string(&pp.pointer).expect("pointer serialises");
            assert!(
                !wire.contains("alice@example.com"),
                "payload email NEVER crosses: {wire}"
            );
            assert!(
                !wire.contains("SECRET"),
                "payload body NEVER crosses: {wire}"
            );
            assert!(!wire.contains("dek:"), "the DEK ref NEVER crosses: {wire}");
            assert!(
                !wire.contains("payload"),
                "no payload field on the frame: {wire}"
            );
        }
        // THE GATE: 0 PII fields crossed the bridge.
        assert_eq!(collab.cross_cell_pii_crossed(), 0);
    }

    /// **The single-cell pin is LIFTED — the fan-out reaches EVERY other member cell (KN-P05 resolved).**
    /// v1 pinned the session to one cell; the fan-out now produces a pointer for each of the tenant's
    /// other member cells. The pointer rides the op's causal-root (BUS-5).
    #[test]
    fn single_cell_pin_lifted_fan_out_reaches_every_other_member_cell() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-a"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-1");
        let members = vec![
            CellId::from_token("cell-a"),
            CellId::from_token("cell-b"),
            CellId::from_token("cell-c"),
            CellId::from_token("cell-d"),
        ];
        let corr = CorrelationId("root".into());
        let fanned = collab.fan_out_doc_op(&dop, &corr, &members);

        let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
        assert_eq!(
            dests,
            vec!["cell-b", "cell-c", "cell-d"],
            "every OTHER member cell reached"
        );
        for pp in &fanned {
            assert_eq!(
                pp.pointer.correlation_id(),
                &corr,
                "rides the op causal-root"
            );
        }
    }

    /// **A single-home-cell tenant propagates nothing (the KN-P05 floor preserved for single-cell).**
    /// With no OTHER member cell, the fan-out is empty — the single-cell path is unchanged.
    #[test]
    fn single_home_cell_tenant_fans_out_nothing() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-a"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-1");
        let fanned = collab.fan_out_doc_op(
            &dop,
            &CorrelationId("root".into()),
            &[CellId::from_token("cell-a")],
        );
        assert!(
            fanned.is_empty(),
            "a single-home-cell tenant has nowhere to fan out"
        );
        assert_eq!(collab.ops_fanned_out(), 0);
        assert_eq!(collab.cross_cell_pii_crossed(), 0);
    }

    /// **THE RESIDENCY GATE — resolution stays cell-local, only the projection crosses.** A collaborator
    /// in a member cell resolves a cross-cell doc THROUGH the home cell: the home cell renders + permits,
    /// returning ONLY a [`DocProjection`] (rendered) — never the raw op-log / payload. The member cell
    /// never reaches into the home cell's rows itself.
    #[test]
    fn resolution_stays_cell_local_only_the_projection_crosses() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-fr-par-1"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-9");
        let fanned = collab.fan_out_doc_op(
            &dop,
            &CorrelationId("root".into()),
            &[CellId::from_token("cell-de-1")],
        );
        let pointer = &fanned[0];

        // The HOME cell renders for an allowed viewer — returns ONLY the projection.
        let home = HomeCellResolver {
            allowed: vec!["viewer-opaque".into()],
            rendered: "Doc title + body (rendered in fr-par-1, viewer-scoped)".into(),
        };
        let proj = collab.resolve_cell_local(pointer, &viewer(), &home);
        assert!(
            proj.is_rendered(),
            "an allowed viewer gets the rendered projection"
        );
        assert_eq!(proj.subject().0, "myelin://acme/knowledge/page/page-9");
        if let DocProjection::Rendered { rendered, .. } = &proj {
            // What crossed back is the home cell's rendered projection — never the raw op payload.
            assert!(rendered.contains("rendered in fr-par-1"));
            assert!(
                !rendered.contains("alice@example.com"),
                "no payload PII in the projection"
            );
            assert!(
                !rendered.contains("dek:"),
                "no DEK material in the projection"
            );
        }
    }

    /// **The residency gate — an unauthorised cross-cell viewer resolves to a TOMBSTONE (no content).**
    /// The home cell permission-checks THERE; a viewer it does not permit gets a tombstone carrying NO
    /// content (the doc's content never leaves its residency cell).
    #[test]
    fn unauthorised_cross_cell_viewer_resolves_to_a_tombstone() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-fr-par-1"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-9");
        let fanned = collab.fan_out_doc_op(
            &dop,
            &CorrelationId("root".into()),
            &[CellId::from_token("cell-de-1")],
        );
        let home = HomeCellResolver {
            allowed: vec![], // the home cell permits no one → tombstone.
            rendered: "should never be returned".into(),
        };
        let proj = collab.resolve_cell_local(&fanned[0], &viewer(), &home);
        assert!(
            !proj.is_rendered(),
            "an unauthorised viewer gets a tombstone"
        );
        assert!(matches!(proj, DocProjection::Tombstone { .. }));
        // The tombstone carries no content — the secret never crossed.
        let wire = serde_json::to_string(&proj).expect("projection serialises");
        assert!(!wire.contains("alice@example.com"));
        assert!(!wire.contains("should never be returned"));
    }

    /// **The CDC pair for contract 12.6 (Knowledge's consumer half).** A PROVIDER (Knowledge's collab
    /// fan-out) emits the doc-op pointer to its frozen 12.6 wire shape; a CONSUMER (the control plane's
    /// cross-cell carriage, stood in by [`PropagatedPointer`]) deserialises that exact wire shape and
    /// reads back ONLY the four frozen fields under the `KnowledgeCollab` stream. The Knowledge fan-out
    /// IS the Bus's propagation — one frame, conformant both ways (EI-01 §7).
    #[test]
    fn cdc_12_6_knowledge_consumer_reads_only_the_four_fields() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-fr-par-1"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-9");
        let fanned = collab.fan_out_doc_op(
            &dop,
            &CorrelationId("root".into()),
            &[CellId::from_token("cell-de-1")],
        );
        let provider = &fanned[0];

        // PROVIDER emits the pointer frame to its canonical 12.6 wire shape.
        let wire = serde_json::to_string(&provider.pointer).expect("provider emits the frame");
        // The on-wire frame carries EXACTLY the four frozen fields.
        let json: serde_json::Value = serde_json::from_str(&wire).expect("valid json");
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["correlation_id", "home_cell", "subject", "type"]);

        // CONSUMER (the control-plane carriage) reads it back as the Bus's KnowledgeCollab propagation.
        let consumer: CrossCellPointer =
            serde_json::from_str(&wire).expect("consumer reads the frame");
        assert_eq!(
            consumer, provider.pointer,
            "the CDC wire shape is conformant both ways"
        );
        // And the Knowledge fan-out adapts to the Bus's PropagatedPointer under the KnowledgeCollab kind.
        let propagated = as_propagated(provider);
        assert_eq!(propagated.stream, CrossCellStream::KnowledgeCollab);
        assert_eq!(
            propagated.pointer.artifact_type(),
            &myelin_events::ArtifactType::Page
        );
    }

    /// The `CrossCellCollab` Debug is PII-free + aggregate-only (the home cell id + counters, never a
    /// doc op / payload). Mirrors the `CrossCellPropagator` PII-free log discipline.
    #[test]
    fn collab_debug_is_pii_free() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-fr-par-1"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-9");
        let _ = collab.fan_out_doc_op(
            &dop,
            &CorrelationId("root".into()),
            &[CellId::from_token("cell-de-1")],
        );
        let dbg = format!("{collab:?}");
        assert!(
            dbg.contains("cell-fr-par-1"),
            "Debug shows the home cell id: {dbg}"
        );
        assert!(
            dbg.contains("ops_fanned_out"),
            "Debug shows the counter: {dbg}"
        );
        assert!(
            !dbg.contains("alice@example.com"),
            "Debug leaks no payload PII: {dbg}"
        );
        assert!(!dbg.contains("dek:"), "Debug leaks no DEK material: {dbg}");
    }
}
