//! CDC pair for contract 12.6 — the cross-cell PII-free pointer bridge FRAME, Bus side
//! (EB-14 / P-091).
//!
//! The frame's AUTHORITY is `myelin-tenancy` (the §2.9 DAG sink); EB-14 pins it from the Bus side
//! by re-exporting it on the frozen `myelin_events::*` path (EI-01 §7 — never a second
//! definition). This CDC pair proves the Bus's re-export conforms to the SAME frozen wire shape:
//!
//! - the **provider** emits a `CrossCellPointer` to its canonical four-field wire shape through the
//!   `myelin_events` re-export path;
//! - the **consumer** (standing in for the Bus's §5 cross-cell carriage) deserialises that exact
//!   wire shape and can read back ONLY the four frozen fields — it routes by `home_cell`, never by
//!   a cell-bound row, and cannot attach payload/PII/authz state because the type does not let it.
//!
//! If the frame shape drifts (a field add/rename, the `type` wire-name change), this pair stops
//! agreeing. The frame is designed-not-built; the live cell-local resolution BUILD is the M5
//! follow-on EB-25.

use myelin_events::{
    assert_cell_agnostic, ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer,
    OpaqueSubjectId,
};

/// The canonical frozen frame both sides of the CDC pair agree on — built ENTIRELY through the
/// frozen `myelin_events::*` re-export path (the path the Bus's §5 surfaces resolve).
fn canonical_pointer() -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
        ArtifactType::Issue,
        CorrelationId("01J0CORR".into()),
        CellId::from_token("cell-fr-par-1"),
    )
}

/// PROVIDER side of the 12.6 CDC pair: a Bus producer emits the cross-cell bridge frame to its
/// canonical four-field wire shape. The wire form carries EXACTLY the four §6.1 fields and the
/// frozen `type` wire-name — never payload/PII/authz state.
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
        "the provider emits EXACTLY the four §6.1 fields — no payload/PII/authz state"
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

/// CONSUMER side of the 12.6 CDC pair: a Bus §5 cross-cell carriage deserialises the
/// provider-emitted wire shape and reads back ONLY the four frozen fields, routing by the opaque
/// pointer (`assert_cell_agnostic`) — it cannot reach into a cell's rows or attach PII.
#[test]
fn consumer_reads_back_only_the_four_frozen_fields_and_routes_by_the_opaque_pointer() {
    let provider = canonical_pointer();
    let wire = serde_json::to_string(&provider).expect("provider emits canonical frame");

    let consumer: CrossCellPointer =
        serde_json::from_str(&wire).expect("consumer reads the canonical frame");

    // The §5 surface routes by the OPAQUE subject (cell-agnostic), never a cell-bound row.
    let routed_subject: &ArtifactRef = assert_cell_agnostic(&consumer);
    assert_eq!(routed_subject.0, "myelin://01J0ACME/issues/issue/42");

    // It sees exactly the four frozen fields and routes by `home_cell`.
    assert_eq!(consumer.artifact_type(), &ArtifactType::Issue);
    assert_eq!(consumer.correlation_id(), &CorrelationId("01J0CORR".into()));
    assert_eq!(consumer.home_cell().as_str(), "cell-fr-par-1");

    // The pair agrees both ways — the Bus re-export conforms to the frozen frame.
    assert_eq!(
        consumer, provider,
        "the CDC wire shape is conformant both ways"
    );
}
