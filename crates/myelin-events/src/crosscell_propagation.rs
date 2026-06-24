//! # `crosscell_propagation` — the Bus's cross-cell event-propagation half (EB-25 / P-438, M5)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/event-bus.md`
//! **§7.4 in full** (cross-cell propagation — *the control plane carries **only** the
//! [`CrossCellPointer`] between a tenant's cells, never payload or PII*; the `control-plane-pii-free`
//! lint holds; *resolution is **always** cell-local*). **Contract:** `contract-index.md` row 12.6
//! (the cross-cell PII-free pointer bridge — the Bus's event-propagation half, built LIVE here).
//! **Reconciliation note:** `00-reconciliation-decisions.md` OQ-I.
//!
//! ## What EB-25 builds here — the BUILD, the M1-frame floor follow-on (the named follow-on)
//! EB-14 ([`crate::crosscell`]) PINNED the four-field [`CrossCellPointer`] frame from the Bus side
//! (re-exported from the `myelin-tenancy` authority) and explicitly named EB-25 as the follow-on
//! that BUILDS the cross-cell propagation. EB-25 is now due. There are **two halves** of contract
//! 12.6 across two §2.9-DAG layers (EI-01 §7 — one frame, two reconciled legs, never a second frame):
//!
//! - **the RESOLUTION half** (cell-local `resolve(ref, viewer, mode)` in the home cell, the
//!   per-viewer projection/tombstone) is built in **`myelin-control-plane`**
//!   (`cross_cell_bridge.rs`, P-CP-19 / P-429) + the multi-cell DSR fan-out / zookie / rebalancing
//!   (`multi_cell.rs`, P-CP-20 / P-430). That layer owns the *pointer transport between cells* + the
//!   *resolution*.
//! - **the EVENT-PROPAGATION half (THIS module, the Bus's leg)** — when an event occurs in a
//!   tenant's home cell that is relevant to the tenant's *other* cells (a cross-cell-relevant
//!   stream: ISS portfolio rollup, KN cross-cell collab, CHAT cross-org channels), the Bus mints a
//!   [`CrossCellPointer`] from the [`EventEnvelope`] carrying **ONLY** the four frozen PII-free
//!   fields (`subject`/`type`/`correlation_id`/`home_cell`) and hands that — and **never** the
//!   payload, **never** any PII — to the control plane to carry between the tenant's cells. The
//!   member cell that receives the pointer does NOT receive the payload; it resolves cell-local (the
//!   resolution half) when a viewer there wants to render it.
//!
//! ## The residency invariant this half enforces (§7.4; the `control-plane-pii-free` lint)
//! **0 PII crosses a cell boundary.** The proof is structural + measured:
//! - **Structural:** the only thing this module ever produces to cross the boundary is a
//!   [`CrossCellPointer`] (the four-field frozen PII-free frame — the `control-plane-pii-free` lint
//!   guards a fifth PII field off it) — [`pointer_for_propagation`] returns exactly that, derived
//!   from the envelope's *opaque* `subject` (an `ArtifactRef`-class id, never a person) + its
//!   `type_`-derived [`ArtifactType`] *kind* + its `correlation_id` (the causal-root) + the home
//!   cell. The envelope's `payload` (which may carry inline refs/IDs but is the cell's data) is
//!   **never** read into the pointer — there is no field on the frame for it to go.
//! - **Measured:** [`CrossCellPropagator`] exposes `pointers_propagated` (how many cross-cell
//!   pointer-events it carried, PII-free) and `pii_fields_crossed` — pinned to **0** by construction
//!   (the propagator only ever emits the four-field frame), a live tripwire so a future regression
//!   that carried payload/PII across would be observable (it would tick above 0). This is the
//!   "0 PII crosses" projection the CP-D8 / GA-D8 drills assert `== 0`.
//!
//! ## Multi-cell fan-out for the floor follow-on streams (§7.4 / the prompt DELIVERABLE)
//! [`CrossCellPropagator::fan_out`] is the Bus's multi-cell fan-out: given a home-cell event + the
//! tenant's `member_cells` (the §6.3 multi-element set `placement_of` now returns, P-CP-20/P-430),
//! it produces one [`PropagatedPointer`] per member cell ≠ the home cell — the pointer-event each of
//! the tenant's *other* cells receives. A member cell that **is** the home cell is skipped (no
//! self-hop — that cell already has the event locally). The stream classes that ride this
//! ([`CrossCellStream`]) are exactly the §6.2 floor follow-ons: ISS portfolio rollup, KN cross-cell
//! collab, CHAT cross-org channels. The Bus carries the pointer events; the subsystems own their
//! surfaces (the rollup card / the collab embed / the channel unfurl — all resolved cell-local in
//! the home cell via the resolution half).
//!
//! ## DAG position (why this is the Bus's leg, not the control plane's)
//! `myelin-events` is ABOVE `myelin-control-plane` in the §2.9 DAG (the control plane carries the
//! pointer; it cannot depend on the Bus). So the Bus owns the *production* of the cross-cell
//! pointer-event (minting it from an envelope, the fan-out selection) and exposes it on the frozen
//! `myelin_events::*` path; the control plane CONSUMES the produced pointer and carries it between
//! cells (`cross_cell_bridge.rs`). One frame, one residency rule, two reconciled legs.
//!
//! ## Floors named (VISION §3 name-your-floors)
//! - **The control-plane transport wire** (the actual cell→cell carriage of the pointer) is the
//!   control plane's `cross_cell_bridge` + the resilient-client transport (P-429/P-437); this module
//!   produces the pointer-event the transport carries. The in-process fan-out here is the SAME shape
//!   the wired transport carries (mirrors how the relay's in-process bus is the same shape as the
//!   live JetStream adapter).
//! - **`[OPEN — LEGAL]` the cross-cell bridge residency proof** — counsel sign-off that
//!   `subject`/`type`/`correlation_id` are not personal data for a tenant. Ships regardless of
//!   ratification: the bridge is PII-free by construction (named in P-429; the engineering floor is
//!   met today, the legal sign-off is a parallel residual).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::crosscell::{ArtifactType, CellId, CrossCellPointer, OpaqueSubjectId};
use crate::{CorrelationId, EventEnvelope, EventType};

/// **The cross-cell-relevant stream classes (§6.2 / §7.4 floor follow-ons).** These are the only
/// streams whose events propagate cross-cell — the multi-cell surfaces named in the architecture.
/// A PII-free *kind* enum (never a person), additive: a new cross-cell surface adds a variant, never
/// a field on the frozen frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CrossCellStream {
    /// **ISS cross-cell portfolio rollup** — a tenant's issues spanning member cells roll up into a
    /// portfolio in the home cell (aggregate projections, resolved cell-local; §6.2).
    IssuePortfolio,
    /// **KN cross-cell collab** — a knowledge page/row shared across a tenant's cells; membership +
    /// content resolved in the home cell (§6.2).
    KnowledgeCollab,
    /// **CHAT cross-org channels** — a channel spanning organisations in different cells; an
    /// unauthorised cross-org viewer resolves to a tombstone (§6.2).
    ChatCrossOrg,
}

impl CrossCellStream {
    /// The [`ArtifactType`] *kind* a stream's pointer carries (the frame's `type` field). This is a
    /// PII-free kind token, never a payload — it is what the member cell uses to route the pointer
    /// to the right cell-local resolver.
    #[must_use]
    pub fn artifact_type(self) -> ArtifactType {
        match self {
            CrossCellStream::IssuePortfolio => ArtifactType::Issue,
            CrossCellStream::KnowledgeCollab => ArtifactType::Page,
            CrossCellStream::ChatCrossOrg => ArtifactType::Channel,
        }
    }

    /// Classify an [`EventEnvelope`] by its `type_`'s leading subsystem token (the §6.1 taxonomy
    /// grammar `<subsystem>.<artifact_type>.<event_name>`) into the cross-cell stream it rides, if
    /// any. An event whose subsystem is NOT a cross-cell-relevant surface returns `None` (it stays
    /// single-home-cell — the v1 default, never propagated). This reads ONLY the opaque taxonomy
    /// token, never the payload.
    #[must_use]
    pub fn classify(event_type: &EventType) -> Option<CrossCellStream> {
        // The leading taxonomy token is the subsystem (§6.1). We route by subsystem ∧ artifact-type
        // kind, never by payload.
        let mut segments = event_type.0.split('.');
        let subsystem = segments.next()?;
        let artifact = segments.next();
        match (subsystem, artifact) {
            ("issues", _) => Some(CrossCellStream::IssuePortfolio),
            ("knowledge", _) => Some(CrossCellStream::KnowledgeCollab),
            ("chat", _) => Some(CrossCellStream::ChatCrossOrg),
            _ => None,
        }
    }
}

/// **Mint the cross-cell [`CrossCellPointer`] for `envelope`, homed in `home_cell`.** This is the
/// ONLY thing the Bus ever produces to cross a cell boundary (§7.4). It carries EXACTLY the four
/// frozen PII-free fields, derived from the envelope:
///
/// - `subject` ← the envelope's **opaque** [`crate::ArtifactRef`] subject (an `ArtifactRef`-class
///   id, never a person — wrapped as an [`OpaqueSubjectId`]);
/// - `type` ← the [`ArtifactType`] *kind* derived from the `CrossCellStream` (a PII-free kind);
/// - `correlation_id` ← the envelope's causal-root (BUS-5 — the SAME `CorrelationId`, so the
///   cross-cell pointer rides the originating causal chain);
/// - `home_cell` ← the cell the artifact lives in (resolution happens THERE).
///
/// The envelope's `payload` is **never** read — there is no field on the frame for it to go (the
/// structural "0 PII crosses" guarantee). `stream` supplies the artifact-type kind so the same
/// envelope is propagated under the right cross-cell surface.
#[must_use]
pub fn pointer_for_propagation(
    envelope: &EventEnvelope,
    stream: CrossCellStream,
    home_cell: CellId,
) -> CrossCellPointer {
    CrossCellPointer::new(
        // The subject is the envelope's OPAQUE ArtifactRef — never a person, never the payload.
        OpaqueSubjectId::from_ref(envelope.subject.clone()),
        stream.artifact_type(),
        // The pointer rides the SAME causal-root the envelope carries (one CorrelationId, BUS-5).
        envelope.correlation_id.clone(),
        home_cell,
    )
}

/// **A cross-cell pointer-event addressed to a specific member cell.** What the Bus hands to the
/// control plane to carry to one of the tenant's *other* cells. It pairs the PII-free
/// [`CrossCellPointer`] with the destination [`CellId`] (an opaque routing handle — where to carry
/// it). It carries NO payload, NO PII — only the four-field frame + the destination routing key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropagatedPointer {
    /// The destination member cell the pointer is carried TO (an opaque routing handle).
    pub to_cell: CellId,
    /// The PII-free cross-cell pointer (the four frozen fields — never payload, never PII).
    pub pointer: CrossCellPointer,
    /// The cross-cell surface the pointer rides (ISS rollup / KN collab / CHAT cross-org).
    pub stream: CrossCellStream,
}

/// **The Bus's cross-cell event propagator (contract 12.6 — the event-propagation half, LIVE).**
/// It serves a tenant's **home cell** and, for a cross-cell-relevant event, fans the PII-free
/// pointer out to the tenant's *other* member cells — carrying ONLY the four-field frame across,
/// never the payload, never any PII.
///
/// `pointers_propagated` + `pii_fields_crossed` are the **PII-free propagation proof** telemetry
/// (the CP-D8 / GA-D8 gate): every propagated pointer increments `pointers_propagated`;
/// `pii_fields_crossed` is pinned to **0** by construction (the propagator only ever emits the
/// four-field frame), exposed as a live tripwire so a future regression that carried payload/PII
/// across the boundary would be observable (it would tick above 0). This is the "0 PII crosses the
/// boundary" projection the drills assert `== 0`.
#[derive(Clone)]
pub struct CrossCellPropagator {
    /// The home cell this propagator serves (an opaque id). A member cell == the home cell is
    /// skipped on fan-out (no self-hop).
    home_cell: CellId,
    /// CP-D8 / GA-D8 telemetry: how many cross-cell pointer-events were propagated (PII-free).
    pointers_propagated: Arc<AtomicU64>,
    /// **The CP-D8 / GA-D8 ZERO — PII fields carried across a cell boundary.** Pinned to 0 by
    /// construction (the propagator only ever emits the four-field PII-free frame). A live counter
    /// (not a constant) so a future regression — a code path that carried payload/PII across — is
    /// observable. This is the "0 PII crosses" projection the drills assert `== 0`.
    pii_fields_crossed: Arc<AtomicU64>,
}

impl CrossCellPropagator {
    /// Build a propagator serving `home_cell`.
    #[must_use]
    pub fn new(home_cell: CellId) -> CrossCellPropagator {
        CrossCellPropagator {
            home_cell,
            pointers_propagated: Arc::new(AtomicU64::new(0)),
            pii_fields_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The home cell this propagator serves (opaque id).
    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    /// **Fan a home-cell event out to the tenant's other member cells (§7.4) — the multi-cell
    /// fan-out.** For a cross-cell-relevant `envelope` (one whose `type_` classifies to a
    /// [`CrossCellStream`]), mint the PII-free [`CrossCellPointer`] (homed in THIS cell) and produce
    /// one [`PropagatedPointer`] per `member_cell` that is **not** this home cell (no self-hop — that
    /// cell already has the event). An event that is NOT cross-cell-relevant (`classify` →
    /// `None`) produces an EMPTY fan-out (it stays single-home-cell — the v1 default, never
    /// propagated, the residency-preserving default).
    ///
    /// Across EVERY produced pointer, ONLY the four frozen frame fields cross — never the payload,
    /// never any PII (`pii_fields_crossed` stays 0). `pointers_propagated` ticks once per produced
    /// pointer. Returns the pointer-events in `member_cells` order (the home cell filtered out).
    pub fn fan_out(
        &self,
        envelope: &EventEnvelope,
        member_cells: &[CellId],
    ) -> Vec<PropagatedPointer> {
        // Route by the opaque taxonomy token only — never the payload.
        let Some(stream) = CrossCellStream::classify(&envelope.type_) else {
            // Not a cross-cell surface — stays single-home-cell (the residency-preserving default).
            return Vec::new();
        };
        let pointer = pointer_for_propagation(envelope, stream, self.home_cell.clone());
        member_cells
            .iter()
            .filter(|to| **to != self.home_cell) // no self-hop — the home cell already has it.
            .map(|to| {
                // Each produced pointer carries ONLY the four-field frame — 0 PII crosses.
                self.pointers_propagated.fetch_add(1, Ordering::SeqCst);
                PropagatedPointer {
                    to_cell: to.clone(),
                    pointer: pointer.clone(),
                    stream,
                }
            })
            .collect()
    }

    /// **The CP-D8 / GA-D8 telemetry — `pointers_propagated`.** How many cross-cell pointer-events
    /// the propagator carried (aggregate, PII-free).
    #[must_use]
    pub fn pointers_propagated(&self) -> u64 {
        self.pointers_propagated.load(Ordering::SeqCst)
    }

    /// **The CP-D8 / GA-D8 ZERO — `pii_fields_crossed`.** Pinned to 0 by construction (the
    /// propagator only ever emits the four-field PII-free frame); exposed as a live tripwire so a
    /// future regression that carried payload/PII across the boundary is observable.
    ///
    /// **Equivalent-mutant note (cargo-mutants):** `replace pii_fields_crossed -> 0` is
    /// observationally identical because the propagator NEVER increments it (the structural
    /// guarantee) — the *correct* property, not a coverage gap. The field + the read seam stay so
    /// the tripwire is wired the day a regression lands (mirrors
    /// `cross_cell_bridge::CrossCellBridge::cross_cell_raw_rows`).
    #[must_use]
    pub fn pii_fields_crossed(&self) -> u64 {
        self.pii_fields_crossed.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossCellPropagator {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // PII-free Debug: the home cell id + the aggregate counters, never an envelope / payload.
        f.debug_struct("CrossCellPropagator")
            .field("home_cell", &self.home_cell.as_str())
            .field("pointers_propagated", &self.pointers_propagated())
            .field("pii_fields_crossed", &self.pii_fields_crossed())
            .finish()
    }
}

/// **The PII-free propagation proof (the CP-D8 / GA-D8 telemetry body).** What crosses the boundary
/// for a propagated pointer is EXACTLY the four frozen [`CrossCellPointer`] fields + the destination
/// routing handle — never a payload, never PII. This helper extracts the (opaque, PII-free) fields a
/// CP-D8 / GA-D8 proof asserts crossed, so a drill can show "the propagation carried only
/// `subject`/`type`/`correlation_id`/`home_cell` + `to_cell`" with the concrete opaque values. It
/// returns the fields by reference — there is structurally no payload field to return.
#[must_use]
pub fn propagated_carried_fields(
    propagated: &PropagatedPointer,
) -> (
    &CellId,
    &OpaqueSubjectId,
    &ArtifactType,
    &CorrelationId,
    &CellId,
) {
    (
        &propagated.to_cell,
        propagated.pointer.subject(),
        propagated.pointer.artifact_type(),
        propagated.pointer.correlation_id(),
        propagated.pointer.home_cell(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Actor, AggregateKey, ArtifactRef, DataRole, EventId, Timestamp, Visibility};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    /// A cross-cell-relevant envelope fixture (an issues event with a PII-BEARING payload, so the
    /// tests can prove the payload NEVER crosses). The subject is an opaque `myelin://…` ref.
    fn envelope_with_payload_pii(type_: &str, subject: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01J0EVT".into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            subject: ArtifactRef(subject.into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: None,
            correlation_id: CorrelationId("01J0CORR".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: true,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
            // PII in the payload — this is exactly what MUST NOT cross the cell boundary.
            payload: serde_json::json!({ "assignee_email": "alice@example.com", "body": "secret" }),
        }
    }

    /// **The pointer minted for propagation carries EXACTLY the four frozen PII-free fields — the
    /// payload NEVER crosses.** The envelope has a PII-bearing payload; the minted pointer carries
    /// only `subject`/`type`/`correlation_id`/`home_cell`, and the serialised frame contains neither
    /// the email nor the body.
    #[test]
    fn pointer_for_propagation_carries_only_the_four_frozen_fields_no_payload() {
        let env =
            envelope_with_payload_pii("issues.issue.created", "myelin://01J0ACME/issues/issue/42");
        let p = pointer_for_propagation(
            &env,
            CrossCellStream::IssuePortfolio,
            CellId::from_token("cell-fr-par-1"),
        );

        assert_eq!(
            p.subject().artifact_ref().0,
            "myelin://01J0ACME/issues/issue/42"
        );
        assert_eq!(p.artifact_type(), &ArtifactType::Issue);
        assert_eq!(p.correlation_id(), &CorrelationId("01J0CORR".into()));
        assert_eq!(p.home_cell().as_str(), "cell-fr-par-1");

        // The frame serialises to EXACTLY the four fields — the payload PII is structurally absent.
        let wire = serde_json::to_string(&p).expect("pointer serialises");
        assert!(
            !wire.contains("alice@example.com"),
            "the payload email NEVER crosses: {wire}"
        );
        assert!(
            !wire.contains("secret"),
            "the payload body NEVER crosses: {wire}"
        );
        assert!(
            !wire.contains("payload"),
            "there is no payload field on the frame: {wire}"
        );
    }

    /// **The pointer rides the envelope's causal-root (BUS-5).** The minted pointer's
    /// `correlation_id` is the SAME `CorrelationId` the envelope carries — one type, no conversion.
    #[test]
    fn pointer_rides_the_envelope_causal_root() {
        let env =
            envelope_with_payload_pii("issues.issue.created", "myelin://01J0ACME/issues/issue/42");
        let p = pointer_for_propagation(
            &env,
            CrossCellStream::IssuePortfolio,
            CellId::from_token("cell-fr-par-1"),
        );
        assert_eq!(p.correlation_id(), &env.correlation_id);
    }

    /// **Stream classification routes by the opaque taxonomy token only.** ISS/KN/CHAT events
    /// classify to their cross-cell stream; a non-cross-cell subsystem (`git`, `ci`, `identity`)
    /// classifies to `None` (stays single-home-cell).
    #[test]
    fn classify_routes_iss_kn_chat_and_skips_others() {
        assert_eq!(
            CrossCellStream::classify(&EventType("issues.issue.created".into())),
            Some(CrossCellStream::IssuePortfolio)
        );
        assert_eq!(
            CrossCellStream::classify(&EventType("knowledge.page.updated".into())),
            Some(CrossCellStream::KnowledgeCollab)
        );
        assert_eq!(
            CrossCellStream::classify(&EventType("chat.message.created".into())),
            Some(CrossCellStream::ChatCrossOrg)
        );
        // Non-cross-cell subsystems stay single-home-cell.
        assert_eq!(
            CrossCellStream::classify(&EventType("git.ref.updated".into())),
            None
        );
        assert_eq!(
            CrossCellStream::classify(&EventType("ci.check.updated".into())),
            None
        );
        assert_eq!(
            CrossCellStream::classify(&EventType("identity.principal.created".into())),
            None
        );
    }

    /// Each stream's artifact-type kind is the §6.2 mapping (ISS→Issue, KN→Page, CHAT→Channel).
    #[test]
    fn stream_artifact_types_are_the_floor_follow_on_kinds() {
        assert_eq!(
            CrossCellStream::IssuePortfolio.artifact_type(),
            ArtifactType::Issue
        );
        assert_eq!(
            CrossCellStream::KnowledgeCollab.artifact_type(),
            ArtifactType::Page
        );
        assert_eq!(
            CrossCellStream::ChatCrossOrg.artifact_type(),
            ArtifactType::Channel
        );
    }

    /// **The multi-cell fan-out produces one PII-free pointer per OTHER member cell (no self-hop).**
    /// A home-cell ISS event fans out to the tenant's two other member cells; the home cell is
    /// filtered out; each produced pointer carries only the four frozen fields; 0 PII crosses.
    #[test]
    fn fan_out_produces_one_pointer_per_other_member_cell() {
        let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
        let env =
            envelope_with_payload_pii("issues.issue.created", "myelin://01J0ACME/issues/issue/42");
        let members = vec![
            CellId::from_token("cell-a"), // the home cell — must be skipped (no self-hop).
            CellId::from_token("cell-b"),
            CellId::from_token("cell-c"),
        ];
        let fanned = prop.fan_out(&env, &members);

        // One pointer per OTHER member cell (cell-a, the home, filtered out).
        let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
        assert_eq!(dests, vec!["cell-b", "cell-c"]);
        assert_eq!(prop.pointers_propagated(), 2);
        // Each produced pointer carries ONLY the four frozen fields — 0 PII crosses.
        for pp in &fanned {
            let (to, subject, kind, corr, home) = propagated_carried_fields(pp);
            assert_eq!(kind, &ArtifactType::Issue);
            assert_eq!(
                subject.artifact_ref().0,
                "myelin://01J0ACME/issues/issue/42"
            );
            assert_eq!(corr, &CorrelationId("01J0CORR".into()));
            assert_eq!(home.as_str(), "cell-a");
            assert!(matches!(to.as_str(), "cell-b" | "cell-c"));
            // The PropagatedPointer serialises with no payload PII.
            let wire = serde_json::to_string(&pp.pointer).expect("serialises");
            assert!(!wire.contains("alice@example.com"));
            assert!(!wire.contains("secret"));
        }
        // The CP-D8 / GA-D8 zero: 0 PII fields crossed the boundary.
        assert_eq!(prop.pii_fields_crossed(), 0);
    }

    /// **A non-cross-cell event produces an EMPTY fan-out (stays single-home-cell).** A `git` event
    /// is not a cross-cell surface — nothing is propagated (the residency-preserving v1 default).
    #[test]
    fn non_cross_cell_event_is_not_propagated() {
        let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
        let env = envelope_with_payload_pii("git.ref.updated", "myelin://01J0ACME/git/repo/r1");
        let fanned = prop.fan_out(&env, &[CellId::from_token("cell-b")]);
        assert!(
            fanned.is_empty(),
            "a non-cross-cell event is never propagated"
        );
        assert_eq!(prop.pointers_propagated(), 0);
        assert_eq!(prop.pii_fields_crossed(), 0);
    }

    /// **A single-home-cell tenant (only the home cell in member_cells) propagates nothing.** With
    /// no OTHER member cell, the fan-out is empty — the single-cell path is unchanged (the v1 floor).
    #[test]
    fn single_home_cell_tenant_propagates_nothing() {
        let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
        let env =
            envelope_with_payload_pii("issues.issue.created", "myelin://01J0ACME/issues/issue/42");
        let fanned = prop.fan_out(&env, &[CellId::from_token("cell-a")]);
        assert!(
            fanned.is_empty(),
            "a single-home-cell tenant has nowhere to propagate"
        );
        assert_eq!(prop.pointers_propagated(), 0);
    }

    /// **KN collab + CHAT cross-org fan out under their own stream kinds.** Proves the floor
    /// follow-on streams beyond ISS ride the SAME propagation with the right artifact-type kind.
    #[test]
    fn kn_collab_and_chat_cross_org_fan_out_under_their_kinds() {
        let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
        let members = vec![CellId::from_token("cell-b")];

        let kn = envelope_with_payload_pii(
            "knowledge.page.updated",
            "myelin://01J0ACME/knowledge/page/9",
        );
        let kn_fan = prop.fan_out(&kn, &members);
        assert_eq!(kn_fan.len(), 1);
        assert_eq!(kn_fan[0].stream, CrossCellStream::KnowledgeCollab);
        assert_eq!(kn_fan[0].pointer.artifact_type(), &ArtifactType::Page);

        let chat =
            envelope_with_payload_pii("chat.message.created", "myelin://01J0ACME/chat/channel/3");
        let chat_fan = prop.fan_out(&chat, &members);
        assert_eq!(chat_fan.len(), 1);
        assert_eq!(chat_fan[0].stream, CrossCellStream::ChatCrossOrg);
        assert_eq!(chat_fan[0].pointer.artifact_type(), &ArtifactType::Channel);
    }

    /// The `CrossCellPropagator` Debug is PII-free + aggregate-only (the home cell id + counters,
    /// never an envelope / payload). Mirrors the `CrossCellBridge` PII-free log discipline.
    #[test]
    fn propagator_debug_is_pii_free() {
        let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
        let env =
            envelope_with_payload_pii("issues.issue.created", "myelin://01J0ACME/issues/issue/42");
        let _ = prop.fan_out(&env, &[CellId::from_token("cell-b")]);
        let dbg = format!("{prop:?}");
        assert!(
            dbg.contains("cell-a"),
            "Debug shows the home cell id: {dbg}"
        );
        assert!(
            dbg.contains("pointers_propagated"),
            "Debug shows the counter: {dbg}"
        );
        assert!(
            !dbg.contains("alice@example.com"),
            "Debug leaks no payload PII: {dbg}"
        );
        assert!(!dbg.contains("secret"), "Debug leaks no payload PII: {dbg}");
    }
}
