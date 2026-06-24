//! EB-25 (global P-438, M5) GATE / DRILL — **the Bus's cross-cell EVENT-PROPAGATION half: a
//! cross-cell-relevant event in the home cell fans a PII-free [`CrossCellPointer`] out to the
//! tenant's OTHER member cells — carrying ONLY `subject`/`type`/`correlation_id`/`home_cell`, never
//! the payload, never any PII (0 PII crosses the cell boundary)** — dated green artifact.
//!
//! **The GATE (testing-strategy GA-D8 / CP-D7 / CP-D8; event-bus.md §7.4):** the cross-cell ref
//! carries ONLY the PII-free bridge; **0 PII crosses a cell boundary**. The resolution half
//! (per-viewer cell-local resolve, unauthorised → tombstone) + the per-cell receipt set / 0
//! migration loss live in `myelin-control-plane` (P-429 / P-430, the resolution + DSR/zookie/rebal
//! legs). **This is the Bus's leg:** the *production* of the cross-cell pointer-event the control
//! plane carries — the 0-PII-crosses propagation proof.
//!
//! **The load-bearing zero (EI-01 §2):** a cross-cell PII leak is stop-the-bleeding. The defence is
//! STRUCTURAL: the propagator only ever emits the four-field PII-free [`CrossCellPointer`] frame —
//! the envelope's `payload` (which may carry inline PII) is NEVER read into the pointer (there is no
//! field on the frame for it to go). So `pii_fields_crossed == 0` is by construction, not by a query
//! that "happened" to return nothing.
//!
//! **This drill proves the gate can go RED** (`pii_fields_crossed > 0` reads RED — a gate that
//! cannot go red is not a gate, EI-01 §3) **AND green** (a cross-cell ISS event fans the four-field
//! frame out to the tenant's other cells with the payload PII structurally absent), and emits the
//! result on the SAME `SignalSource` every drill uses (observability is part of the pass).
//!
//! **FLOOR (named, VISION §3):** the cell→cell transport WIRE (the actual carriage of the pointer
//! between cells) is the control plane's `cross_cell_bridge` + the resilient client (P-429 / P-437);
//! this leg produces the pointer-event the wire carries. The `[OPEN — LEGAL]` cross-cell bridge
//! residency proof (counsel sign-off that the four fields are not personal data) ships regardless of
//! ratification — PII-free by construction.

use myelin_events::{Actor, AggregateKey, CorrelationId};
use myelin_events::{
    ArtifactRef, CellId, CrossCellPropagator, CrossCellStream, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

/// A cross-cell-relevant ISS envelope whose PAYLOAD carries PII (an assignee email + a free-text
/// body) — exactly the data that MUST NOT cross the cell boundary. The subject is an opaque
/// `myelin://…` ref.
fn iss_envelope_with_payload_pii() -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J0EVT".into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        subject: ArtifactRef("myelin://01J0ACME/issues/issue/42".into()),
        aggregate: AggregateKey("issue:PROJ-1".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J0CHAIN".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: true,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        payload: serde_json::json!({ "assignee_email": "alice@example.com", "body": "the secret plan" }),
    }
}

/// **THE EB-25 DRILL (dated green artifact): a cross-cell ISS event fans the PII-free pointer out to
/// the tenant's OTHER member cells — carrying ONLY the four frozen fields, 0 PII across the
/// boundary.**
#[test]
fn eb25_cross_cell_propagation_carries_zero_pii() {
    // The tenant is homed in cell-a with member cells b + c (the multi-element member_cells set
    // P-CP-20 / P-430 now returns). The ISS event occurs in the home cell with a PII-bearing payload.
    let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
    let env = iss_envelope_with_payload_pii();
    let member_cells = vec![
        CellId::from_token("cell-a"), // the home cell — skipped (no self-hop)
        CellId::from_token("cell-b"),
        CellId::from_token("cell-c"),
    ];

    // ── GREEN leg: the cross-cell event fans out to the tenant's OTHER member cells. ──
    let fanned = prop.fan_out(&env, &member_cells);
    let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
    assert_eq!(
        dests,
        vec!["cell-b", "cell-c"],
        "the cross-cell ISS event fans out to the tenant's OTHER cells (the home cell skipped)"
    );

    // ── Each produced pointer carries ONLY the four frozen §6.1 fields — the payload PII is
    //    STRUCTURALLY absent (the propagation proof). ──
    for pp in &fanned {
        assert_eq!(pp.stream, CrossCellStream::IssuePortfolio);
        assert_eq!(
            pp.pointer.subject().artifact_ref().0,
            "myelin://01J0ACME/issues/issue/42"
        );
        assert_eq!(
            pp.pointer.correlation_id(),
            &CorrelationId("01J0CHAIN".into())
        );
        assert_eq!(pp.pointer.home_cell().as_str(), "cell-a");
        // The pointer serialises with NO payload PII — there is no field for it.
        let wire = serde_json::to_string(&pp.pointer).expect("pointer serialises");
        assert!(
            !wire.contains("alice@example.com"),
            "the payload email NEVER crosses the boundary: {wire}"
        );
        assert!(
            !wire.contains("the secret plan"),
            "the payload body NEVER crosses the boundary: {wire}"
        );
    }

    // ── The EB-25 ZERO: 0 PII fields crossed the boundary across the whole fan-out. ──
    let pii_crossed = prop.pii_fields_crossed();
    assert_eq!(
        pii_crossed, 0,
        "0 PII crosses the cell boundary (the EB-25 zero)"
    );
    assert_eq!(
        prop.pointers_propagated(),
        2,
        "two cross-cell pointer-events propagated (one per other member cell)"
    );

    // ── Emit the gate result on the SAME SignalSource every drill uses (observability is part of the
    //    pass, EI-01 §3): the cross-cell PII-crossing count == 0 (the headline EB-25 zero; the
    //    CrossTenantCount projection is the cross-cell/cross-tenant leak counter). ──
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, pii_crossed as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-438 EB-25 GA-D8/CP-D8 GREEN 2026-06-24] Bus cross-cell EVENT-PROPAGATION half LIVE: a \
         cross-cell ISS event in the home cell (cell-a) with a PII-bearing payload fanned a PII-free \
         CrossCellPointer out to the tenant's OTHER member cells (cell-b, cell-c) — each carried ONLY \
         subject/type/correlation_id/home_cell (the four frozen fields); the payload PII \
         (assignee_email/body) was STRUCTURALLY absent. pointers_propagated={}, PII fields across the \
         boundary={} (the EB-25 zero). The resolution half (per-viewer cell-local resolve, \
         unauthorised → tombstone) + per-cell receipts + 0 migration loss green in \
         myelin-control-plane (P-429 CP-D8 / P-430 GA-D8/CP-D7). FLOOR: the cell→cell transport wire is \
         the control-plane bridge + resilient client; the [OPEN — LEGAL] bridge-residency proof ships \
         regardless of ratification (PII-free by construction).",
        prop.pointers_propagated(),
        pii_crossed,
    );
}

/// **The gate is NOT vacuous: a PII field crossing the boundary would read RED.** Proves the EB-25
/// zero is a real tripwire — if a (hypothetical) regression carried one PII field across, the
/// `CrossTenantCount > 0` predicate would fail. The structural defence pins the real value to 0;
/// this asserts the assertion itself is load-bearing (EI-01 §3, a gate that cannot go red is not a
/// gate).
#[test]
fn eb25_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    // A hypothetical regression that carried ONE PII field across the boundary.
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a PII field crossing the cell boundary MUST read RED — the EB-25 zero is a real tripwire"
    );
}

/// **CP-D7 cross-reference (0 migration loss):** a tenant's home cell migrating cell→cell does not
/// lose the cross-cell pointer-event — the propagation is derived purely from the (durable) envelope
/// plus the (control-plane) member_cells, so a re-derivation after a migration is byte-identical.
/// This asserts the propagation is a PURE function of the envelope + the cell set (no hidden state),
/// so it survives the cell→cell migration the control plane drills as CP-D7 (P-430).
#[test]
fn eb25_propagation_is_pure_so_survives_migration_zero_loss() {
    let env = iss_envelope_with_payload_pii();
    let members = vec![CellId::from_token("cell-a"), CellId::from_token("cell-b")];

    let before = CrossCellPropagator::new(CellId::from_token("cell-a"));
    let fan_before = before.fan_out(&env, &members);

    // After a (cell→cell) migration the control plane re-drives propagation from the SAME durable
    // envelope + cell set — a fresh propagator yields the byte-identical pointer-events (0 loss).
    let after = CrossCellPropagator::new(CellId::from_token("cell-a"));
    let fan_after = after.fan_out(&env, &members);

    assert_eq!(
        fan_before, fan_after,
        "propagation is a pure function of (envelope, member_cells) — a re-drive after migration is \
         byte-identical (CP-D7 0 loss)"
    );
}
