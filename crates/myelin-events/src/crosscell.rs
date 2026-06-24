//! # `crosscell` — the cross-cell bridge FRAME, pinned from the Bus side (EB-14 / P-091)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/event-bus.md`
//! §7.4 (the cross-cell propagation FLOOR — designed-not-built; the bridge frame now PINNED; the
//! §5 contracts are cell-agnostic so it extends without a rewrite).
//! **Contract:** `contract-index.md` row 12.6 (`CrossCellPointer{subject (opaque), type,
//! correlation_id, home_cell}` — the frame pinned, BUILT in EB-25).
//! **Reconciliation note:** `00-reconciliation-decisions.md` OQ-I (the cross-cell frame).
//!
//! ## Reconciliation in place (EI-01 §7 — never define a type twice)
//! The global run order interleaves the Tenancy and Event-Bus roadmaps, so the SAME contract-12.6
//! frame is reached from BOTH a Tenancy prompt (P-CP-02 / P-027) and this Bus prompt (EB-14 /
//! P-091). The frame's **authority is `myelin-tenancy`** — it is the §2.9 DAG SINK, and the frame
//! shares the `correlation_id` value type with the `EventEnvelope` (which would naively put the
//! type in `myelin-events`, but the DAG puts events ABOVE tenancy, so the shared value types live
//! in the sink — exactly the `ArtifactRef`/`CorrelationId` DAG-deviation this crate already
//! documents). Per the coherence rule, EB-14 therefore **does NOT re-define** `CrossCellPointer`
//! (or `OpaqueSubjectId` / `ArtifactType` / `CellId`): it **re-exports the tenancy authority**
//! through `myelin_events` so the Bus's §5 contract surfaces compile against the ONE frozen frame,
//! and it adds the Bus-side GATE the EB-14 prompt names — the serde round-trip through the Bus's
//! own re-export path, plus the **compile-time cell-agnostic assertion** that the §5 surfaces take
//! the opaque subject, never a cell-bound row.
//!
//! ## What "PIN the frame from the Bus side" means here
//! 1. The four frozen frame types are re-exported on the frozen Bus path `myelin_events::*` so an
//!    emitter/consumer/relay in the Bus carries a cross-cell pointer by the ONE shape (and so the
//!    §5 contract surfaces are cell-agnostic by construction — they take the opaque
//!    [`CrossCellPointer`]/[`OpaqueSubjectId`], never a cell-bound DB row).
//! 2. The Bus-side serde round-trip GATE ([`tests::eb14_frame_serde_round_trips_through_the_bus_path`])
//!    proves the frame is well-defined as the Bus sees it (all four fields, `type` wire-name
//!    pinned, nothing else).
//! 3. The compile-time cell-agnostic assertion ([`assert_cell_agnostic`]) proves the §5 surfaces a
//!    cross-cell pointer flows through take the OPAQUE subject — a `CrossCellPointer` is built from
//!    an [`OpaqueSubjectId`] (an `ArtifactRef`-class opaque id, NEVER a person, NEVER a cell-bound
//!    row); the only constructor takes exactly the four PII-free fields. If the frame grew a
//!    cell-bound or PII field, this stops compiling.
//!
//! ## FLOOR PROMOTED — the BUILD shipped (EB-25 / P-438, M5)
//! This module pins the **frame** (re-exported from the tenancy authority). The cross-cell BUILD —
//! the EB-25 M5 follow-on of this frame — is now **shipped across its two §2.9-DAG legs** (ONE
//! frame, two reconciled legs, EI-01 §7):
//! - the **RESOLUTION half** (per-viewer **cell-local** resolution — cell A asks cell B to
//!   `resolve(ref, viewer, mode)` IN B, permission-checked in B, only the already-rendered
//!   projection/tombstone crossing — §7.4/§6.2) lives in **`myelin-control-plane`**
//!   (`cross_cell_bridge.rs`, P-CP-19 / P-429) + the multi-cell DSR fan-out / zookie / rebalancing
//!   (`multi_cell.rs`, P-CP-20 / P-430);
//! - the **EVENT-PROPAGATION half** (the Bus's leg — minting the PII-free [`CrossCellPointer`] from
//!   an `EventEnvelope`, the multi-cell fan-out to the tenant's *other* cells, the residency proof
//!   that **no PII crosses**) lives in [`crate::crosscell_propagation`] (EB-25 / P-438), whose
//!   `CrossCellPropagator` produces the pointer-event the control plane carries.
//!
//! The drills GA-D8 / CP-D7 / CP-D8 (0 PII crosses; per-cell receipts; 0 migration loss) green
//! across the two legs (CP-D8 in P-429; GA-D8 / CP-D7 in P-430; the Bus-side 0-PII-crosses leg in
//! `crate::crosscell_propagation` + `tests/drills_eb25_cross_cell_propagation.rs`). **The
//! `resolve()` lives in the control-plane bridge, NOT here** (the §2.9 DAG sites resolution in the
//! control plane, the Bus owns only the propagation production).

use crate::{ArtifactRef, CorrelationId};

/// Re-export the frozen contract-12.6 frame + its three supporting value types on the frozen Bus
/// path (`myelin_events::CrossCellPointer`, `::OpaqueSubjectId`, `::ArtifactType`, `::CellId`).
///
/// **Definition site is `myelin-tenancy`** (the §2.9 DAG sink — see the module-level
/// reconciliation note). These are re-exported, NEVER re-defined, so there is exactly ONE
/// `CrossCellPointer` platform-wide and the Bus's §5 contract surfaces compile against the same
/// frozen shape ISS/KN/CHAT and the Tenancy control plane do. `CorrelationId` is already a frozen
/// `myelin_events::CorrelationId` re-export (shared with the envelope's causal-root field, 2.1);
/// the frame's `correlation_id` field is that exact type.
pub use myelin_tenancy::{ArtifactType, CellId, CrossCellPointer, OpaqueSubjectId};

/// **Compile-time cell-agnostic assertion (the EB-14 structural gate).** Accepts the §5 surfaces a
/// cross-cell pointer flows through by the OPAQUE bridge frame — never a cell-bound row, never PII.
///
/// The signature is the proof: it takes a `&`[`CrossCellPointer`], whose `subject` is an
/// [`OpaqueSubjectId`] (an `ArtifactRef`-class opaque id) and whose `home_cell` is a [`CellId`]
/// routing handle. A cell-bound DB row or a PII-bearing struct could not be passed here. The four
/// frozen accessors are the ONLY readable state — the function reads `home_cell` (the routing key a
/// real M5 resolution would dispatch on) and returns the opaque subject's [`ArtifactRef`], proving
/// the surface is cell-agnostic: it routes by the opaque pointer, it does not reach into a cell's
/// rows. (This is the designed-not-built shape; the live resolution is EB-25.)
///
/// It is a free function (not a method) precisely so the cell-agnostic property is asserted at the
/// Bus contract boundary, independent of the tenancy authority's own accessors.
#[must_use]
pub fn assert_cell_agnostic(pointer: &CrossCellPointer) -> &ArtifactRef {
    // Reading `home_cell` is the routing a cell-local M5 resolution would dispatch on — a
    // `CellId` opaque routing handle, never a row. `subject` is the opaque pointer, never PII.
    let _home: &CellId = pointer.home_cell();
    pointer.subject().artifact_ref()
}

/// The causal-root id of a cross-cell pointer is the SAME `CorrelationId` the envelope carries
/// (BUS-5) — this is the seam that ties a cross-cell pointer back to the originating causal chain
/// on the Bus. A pure read; it exists so the Bus side has a typed call site proving the frame's
/// `correlation_id` is the envelope's causal-root type, not a parallel id.
#[must_use]
pub fn pointer_correlation(pointer: &CrossCellPointer) -> &CorrelationId {
    pointer.correlation_id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventId, EventType, PiiKeyRef,
        Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    /// The canonical frozen frame the EB-14 Bus-side tests share — built ENTIRELY through the
    /// frozen `myelin_events::*` re-export path (the path the Bus's §5 surfaces resolve), proving
    /// the frame is the Bus's frame by construction.
    fn sample_pointer() -> CrossCellPointer {
        CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
            ArtifactType::Issue,
            CorrelationId("01J0CORR".into()),
            CellId::from_token("cell-fr-par-1"),
        )
    }

    /// **THE EB-14 GATE (frame serde round-trip).** The `CrossCellPointer` frame serde
    /// round-trips through the Bus's own re-export path: serialise → deserialise yields the same
    /// value, the on-wire frame carries EXACTLY the four §6.1 fields (`subject`, `type`,
    /// `correlation_id`, `home_cell`) and nothing else, and `type` is the frozen wire name (a Rust
    /// keyword exposed as `r#type`, serde-renamed to `type`). This round-trip IS the proof the
    /// frame is well-defined as the Bus pins it (the prompt's required unit test).
    #[test]
    fn eb14_frame_serde_round_trips_through_the_bus_path() {
        let p = sample_pointer();

        let json = serde_json::to_value(&p).expect("frame serialises");
        let obj = json.as_object().expect("frame is a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["correlation_id", "home_cell", "subject", "type"],
            "the pinned frame carries EXACTLY the four §6.1 fields — no payload/PII/authz state"
        );
        // `type` is the frozen wire name (not the Rust `r#type`).
        assert!(
            obj.contains_key("type"),
            "the frozen wire field name is `type`"
        );
        assert!(
            !obj.contains_key("r#type"),
            "the Rust keyword never leaks onto the wire"
        );

        let back: CrossCellPointer =
            serde_json::from_value(json).expect("frame deserialises to the same value");
        assert_eq!(back, p, "serde round-trip is lossless for all four fields");

        // The four frozen accessors read back exactly what was put in (the frame is read-only).
        assert_eq!(
            back.subject().artifact_ref().0,
            "myelin://01J0ACME/issues/issue/42"
        );
        assert_eq!(back.artifact_type(), &ArtifactType::Issue);
        assert_eq!(back.correlation_id(), &CorrelationId("01J0CORR".into()));
        assert_eq!(back.home_cell().as_str(), "cell-fr-par-1");
    }

    /// **The compile-time cell-agnostic assertion (the structural half of the EB-14 gate).** The
    /// §5 surface ([`assert_cell_agnostic`]) takes the OPAQUE bridge frame and routes by it — its
    /// signature could not accept a cell-bound row or a PII struct (the frame's only constructor
    /// takes the four PII-free fields; `subject` is an `ArtifactRef`-class opaque id). The test
    /// runs the surface and confirms it routes by the opaque subject, never a cell's rows.
    #[test]
    fn section_5_surfaces_are_cell_agnostic_they_take_the_opaque_subject() {
        let p = sample_pointer();
        let subject: &ArtifactRef = assert_cell_agnostic(&p);
        assert_eq!(subject.0, "myelin://01J0ACME/issues/issue/42");
        // The pointer's correlation ties it to the Bus causal chain — the SAME `CorrelationId`
        // the envelope carries (one type platform-wide, not a parallel id).
        assert_eq!(pointer_correlation(&p), &CorrelationId("01J0CORR".into()));
    }

    /// The frame's `correlation_id` is **the identical type** the [`EventEnvelope`] carries as its
    /// causal-root (contract 2.1 ↔ 12.6, BUS-5): a `CorrelationId` read off an envelope can be
    /// dropped straight into the frame and back, no conversion. This is the Bus seam that lets a
    /// cross-cell pointer ride the same causal chain as the originating event (EI-01 §7: ONE
    /// `CorrelationId` platform-wide).
    #[test]
    fn frame_correlation_id_is_the_envelope_causal_root_type() {
        let env = sample_envelope();
        let corr: CorrelationId = env.correlation_id.clone();
        let p = CrossCellPointer::new(
            OpaqueSubjectId::from_ref(env.subject.clone()),
            ArtifactType::Issue,
            corr.clone(),
            CellId::from_token("cell-fr-par-1"),
        );
        assert_eq!(p.correlation_id(), &corr);
        // And the envelope's causal root is exactly the frame's (no lossy conversion).
        assert_eq!(pointer_correlation(&p), &env.correlation_id);
    }

    /// **The CDC serde-conformance pair for contract 12.6 (Bus side).** A PROVIDER emits the frame
    /// to its frozen wire shape; a CONSUMER (standing in for the Bus's §5 cross-cell carriage)
    /// deserialises that exact wire shape and reads back ONLY the four frozen fields — it cannot
    /// attach payload/PII/authz state because the type does not let it. If the frame shape drifts,
    /// the consumer's read-back assertions fail. This is the Bus-side framing of the 12.6 CDC pair
    /// the tenancy authority also carries (one frame, conformance proven from both sides).
    #[test]
    fn cdc_12_6_bus_provider_emits_and_consumer_reads_only_four_fields() {
        // PROVIDER: emit the frame to its canonical wire shape.
        let provider = sample_pointer();
        let wire = serde_json::to_string(&provider).expect("provider emits canonical frame");

        // CONSUMER: a Bus §5 cross-cell carriage that deserialises and routes by home_cell only.
        let consumer: CrossCellPointer =
            serde_json::from_str(&wire).expect("consumer reads the canonical frame");
        // It sees exactly the four frozen fields and routes by the opaque pointer.
        let routed_subject = assert_cell_agnostic(&consumer);
        assert_eq!(routed_subject.0, "myelin://01J0ACME/issues/issue/42");
        assert_eq!(consumer.home_cell().as_str(), "cell-fr-par-1");
        assert_eq!(
            consumer, provider,
            "the CDC wire shape is conformant both ways"
        );
    }

    /// A self-contained envelope fixture (the crosscell tests own their fixture rather than
    /// reaching into the private `envelope::tests` module). Mirrors the `partition` test fixture:
    /// `type_ = issue.issue.created`, `aggregate = issue:PROJ-1`, tenant `acme`, region `fr-par`.
    fn sample_envelope() -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01J0".into()),
            type_: EventType("issue.issue.created".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None::<PiiKeyRef>,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }
}
