use myelin_events::{
    assert_cell_agnostic, ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer,
    OpaqueSubjectId,
};

fn canonical_pointer() -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
        ArtifactType::Issue,
        CorrelationId("01J0CORR".into()),
        CellId::from_token("cell-fr-par-1"),
    )
}

#[test]
fn provider_emits_the_canonical_cross_cell_frame() {
    let pointer = canonical_pointer();
    let json = serde_json::to_value(&pointer).expect("provider emits canonical frame");
    let obj = json.as_object().expect("frame is a JSON object");

    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["correlation_id", "home_cell", "subject", "type"],
        "the provider emits EXACTLY the four §6.1 fields - no payload/PII/authz state"
    );
    assert!(
        obj.contains_key("type"),
        "the frozen wire field name is `type`"
    );
    assert!(
        !obj.contains_key("r#type"),
        "the Rust keyword never leaks onto the wire"
    );
}

#[test]
fn consumer_reads_back_only_the_four_frozen_fields_and_routes_by_the_opaque_pointer() {
    let provider = canonical_pointer();
    let wire = serde_json::to_string(&provider).expect("provider emits canonical frame");

    let consumer: CrossCellPointer =
        serde_json::from_str(&wire).expect("consumer reads the canonical frame");

    let routed_subject: &ArtifactRef = assert_cell_agnostic(&consumer);
    assert_eq!(routed_subject.0, "myelin://01J0ACME/issues/issue/42");

    assert_eq!(consumer.artifact_type(), &ArtifactType::Issue);
    assert_eq!(consumer.correlation_id(), &CorrelationId("01J0CORR".into()));
    assert_eq!(consumer.home_cell().as_str(), "cell-fr-par-1");

    assert_eq!(
        consumer, provider,
        "the CDC wire shape is conformant both ways"
    );
}
