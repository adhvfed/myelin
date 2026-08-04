use myelin_events::{
    assert_cell_agnostic, ArtifactType, CellId, CorrelationId, CrossCellPointer, CrossCellStream,
};
use myelin_knowledge::collab::{
    as_propagated, CrossCellCollab, CrossCellDocOp, CrossCellDocPointer,
};
use myelin_knowledge::transport::{DocOp, OpId, OpKind};
use myelin_tenancy::TenantId;

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

#[test]
fn cdc_12_6_knowledge_collab_provider_consumer_agree_on_the_four_field_frame() {
    let provider = provider_pointer();

    let wire = serde_json::to_string(&provider.pointer).expect("provider emits the frame");

    let json: serde_json::Value = serde_json::from_str(&wire).expect("valid json");
    let mut keys: Vec<&str> = json
        .as_object()
        .expect("frame is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["correlation_id", "home_cell", "subject", "type"]);

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

    let consumer: CrossCellPointer = serde_json::from_str(&wire).expect("consumer reads the frame");
    assert_eq!(
        consumer, provider.pointer,
        "the CDC wire shape is conformant both ways"
    );
    let routed = assert_cell_agnostic(&consumer);
    assert_eq!(routed.0, "myelin://acme/knowledge/page/page-9");
    assert_eq!(consumer.artifact_type(), &ArtifactType::Page);
    assert_eq!(
        consumer.correlation_id(),
        &CorrelationId("op-causal-root".into())
    );
    assert_eq!(consumer.home_cell().as_str(), "cell-fr-par-1");
}

#[test]
fn cdc_12_6_knowledge_collab_is_the_bus_knowledge_collab_propagation() {
    let provider = provider_pointer();
    let propagated = as_propagated(&provider);
    assert_eq!(propagated.stream, CrossCellStream::KnowledgeCollab);
    assert_eq!(propagated.pointer.artifact_type(), &ArtifactType::Page);
    assert_eq!(propagated.to_cell.as_str(), "cell-de-1");
    let wire = serde_json::to_string(&propagated.pointer).expect("serialises");
    let back: CrossCellPointer = serde_json::from_str(&wire).expect("round-trips");
    assert_eq!(back, provider.pointer);
}
