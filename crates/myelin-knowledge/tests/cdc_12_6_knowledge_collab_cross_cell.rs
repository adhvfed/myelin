//! # CDC pair for contract 12.6 — Knowledge's cross-cell collab CONSUMER half (KN-P30 / P-485, M5)
//!
//! **Contract:** `contract-index.md` row 12.6 (the cross-cell PII-free pointer bridge —
//! `CrossCellPointer{subject (opaque), type, correlation_id, home_cell}`; resolution always cell-local).
//! Knowledge is a CONSUMER of 12.6: its collab layer ([`myelin_knowledge::collab`]) fans a doc op's
//! PII-free pointer out to the tenant's other cells (the `KnowledgeCollab` stream).
//!
//! This CDC pair proves the Knowledge-produced doc-op pointer conforms to the SAME frozen four-field
//! wire shape the control-plane resolution half (`cross_cell_bridge`, P-429) and the Bus's propagation
//! half (`crosscell_propagation`, P-438) agree on — ONE frame, conformant across all three legs
//! (EI-01 §7, never a second frame):
//! - the **provider** (Knowledge's collab fan-out) mints a [`CrossCellDocPointer`] from a doc op and
//!   emits its [`myelin_events::CrossCellPointer`] to the canonical four-field wire shape — the op
//!   payload (which may carry inline PII under a DEK ref) is structurally absent;
//! - the **consumer** (standing in for the control-plane bridge carrying the pointer cell→cell + the
//!   home cell resolving it cell-local) deserialises that exact wire shape and reads back ONLY the four
//!   frozen fields, routing by the opaque pointer — it cannot reach the originating op payload (there is
//!   no field for it).
//!
//! If the collab fan-out ever leaked the op payload / DEK material onto the wire (a fifth field), the
//! provider's emitted shape would not be the four-field frame the consumer agrees on, and this pair
//! would stop agreeing.

use myelin_events::{
    assert_cell_agnostic, ArtifactType, CellId, CorrelationId, CrossCellPointer, CrossCellStream,
};
use myelin_knowledge::collab::{
    as_propagated, CrossCellCollab, CrossCellDocOp, CrossCellDocPointer,
};
use myelin_knowledge::transport::{DocOp, OpId, OpKind};
use myelin_tenancy::TenantId;

/// A doc op with a PII-bearing payload + a DEK ref (the payload + DEK material MUST NOT cross).
fn op_with_pii() -> DocOp {
    let mut op = DocOp::cas(
        OpId::new("client-1", 4),
        "author-opaque",
        OpKind::Insert,
        b"bob@example.com SECRET".to_vec(),
    );
    op.pii_key_ref = Some("dek:page-9:run-1".into());
    op
}

/// PROVIDER: Knowledge's collab fan-out mints a cross-cell doc-op pointer.
fn provider_pointer() -> CrossCellDocPointer {
    let collab = CrossCellCollab::new(CellId::from_token("cell-fr-par-1"));
    let op = op_with_pii();
    let tenant = TenantId("acme".into());
    let dop = CrossCellDocOp {
        tenant: &tenant,
        page_id: "page-9",
        op: &op,
    };
    let fanned = collab.fan_out_doc_op(
        &dop,
        &CorrelationId("op-causal-root".into()),
        &[CellId::from_token("cell-de-1")],
    );
    assert_eq!(collab.cross_cell_pii_crossed(), 0, "0 PII crosses on mint");
    fanned.into_iter().next().expect("one pointer fanned out")
}

/// **The CDC pair — provider emits the four-field frame, consumer reads back ONLY the four fields.**
#[test]
fn cdc_12_6_knowledge_collab_provider_consumer_agree_on_the_four_field_frame() {
    let provider = provider_pointer();

    // PROVIDER emits the pointer frame to its canonical 12.6 wire shape.
    let wire = serde_json::to_string(&provider.pointer).expect("provider emits the frame");

    // The on-wire frame carries EXACTLY the four frozen §6.1 fields — nothing else.
    let json: serde_json::Value = serde_json::from_str(&wire).expect("valid json");
    let mut keys: Vec<&str> = json
        .as_object()
        .expect("frame is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["correlation_id", "home_cell", "subject", "type"]);

    // The op payload + DEK material are structurally absent from the wire.
    assert!(
        !wire.contains("bob@example.com"),
        "no payload PII on the wire: {wire}"
    );
    assert!(
        !wire.contains("SECRET"),
        "no payload body on the wire: {wire}"
    );
    assert!(
        !wire.contains("dek:"),
        "no DEK material on the wire: {wire}"
    );
    assert!(
        !wire.contains("payload"),
        "no payload field on the frame: {wire}"
    );

    // CONSUMER (the control-plane bridge carriage + the home-cell resolver) deserialises the exact
    // wire shape and routes by the OPAQUE pointer — it reads back ONLY the four frozen fields.
    let consumer: CrossCellPointer = serde_json::from_str(&wire).expect("consumer reads the frame");
    assert_eq!(
        consumer, provider.pointer,
        "the CDC wire shape is conformant both ways"
    );
    // The §5 surface routes by the opaque subject (it cannot reach a cell-bound row or the payload).
    let routed = assert_cell_agnostic(&consumer);
    assert_eq!(routed.0, "myelin://acme/knowledge/page/page-9");
    assert_eq!(consumer.artifact_type(), &ArtifactType::Page);
    assert_eq!(
        consumer.correlation_id(),
        &CorrelationId("op-causal-root".into())
    );
    assert_eq!(consumer.home_cell().as_str(), "cell-fr-par-1");
}

/// **The Knowledge fan-out IS the Bus's propagation under the `KnowledgeCollab` stream (EI-01 §7).** The
/// doc-op pointer adapts to the Bus's `PropagatedPointer` shape with the SAME four-field frame — proving
/// there is no second propagator / no second frame.
#[test]
fn cdc_12_6_knowledge_collab_is_the_bus_knowledge_collab_propagation() {
    let provider = provider_pointer();
    let propagated = as_propagated(&provider);
    assert_eq!(propagated.stream, CrossCellStream::KnowledgeCollab);
    assert_eq!(propagated.pointer.artifact_type(), &ArtifactType::Page);
    assert_eq!(propagated.to_cell.as_str(), "cell-de-1");
    // The adapted pointer round-trips the SAME frozen wire shape.
    let wire = serde_json::to_string(&propagated.pointer).expect("serialises");
    let back: CrossCellPointer = serde_json::from_str(&wire).expect("round-trips");
    assert_eq!(back, provider.pointer);
}
