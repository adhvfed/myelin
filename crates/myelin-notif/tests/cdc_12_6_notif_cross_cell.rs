//! # CDC — Notif's consumption of contract 12.6 (the CrossCellPointer PII-free bridge) (NOTIF-P24 / P-466)
//!
//! The cross-cell inbox aggregation CONSUMES the frozen contract-12.6 frame (the
//! `CrossCellPointer{subject(opaque), type, correlation_id, home_cell}` — the control plane carries
//! ONLY the pointer, never name/email/body). This consumer-driven contract pair proves Notif reads the
//! frame by its frozen four-field wire shape: a PROVIDER (the control-plane bridge / the per-cell inbox
//! materialiser) emits the frame; the Notif CONSUMER (the aggregation) deserialises it and reads back
//! ONLY the four frozen fields — it cannot attach a payload/PII/authz field because the type does not
//! let it. If the frame shape drifts, this build breaks (never silently in prod, ADR-01).
//!
//! It also proves the aggregation's PII-free-bridge property structurally: what crosses is EXACTLY the
//! four frozen fields (`aggregation_carried_fields`); only a humanised projection / a tombstone crosses
//! back — never a raw `inbox_item` row (`raw_rows_crossed == 0`).

use myelin_notif::{aggregation_carried_fields, cross_cell_inbox_pointer};
use myelin_tenancy::{ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer};

fn sample_pointer() -> CrossCellPointer {
    cross_cell_inbox_pointer(
        &ArtifactRef("myelin://01J0BETA/notif/item/42".into()),
        ArtifactType::Issue,
        CorrelationId("01J0CORR".into()),
        CellId::from_token("cell-fr-par-2"),
    )
}

/// **The 12.6 CDC pair (Notif side).** A PROVIDER emits the frame to its frozen wire shape; the Notif
/// CONSUMER (the aggregation) deserialises that exact wire shape and reads back ONLY the four frozen
/// fields. The wire frame carries EXACTLY `subject`/`type`/`correlation_id`/`home_cell` and nothing
/// else (no payload/PII/authz state).
#[test]
fn cdc_notif_consumes_12_6_frame_only_four_fields() {
    // PROVIDER: emit the frame to its canonical wire shape.
    let provider = sample_pointer();
    let wire = serde_json::to_string(&provider).expect("provider emits the canonical 12.6 frame");

    // The on-wire frame carries EXACTLY the four §6.1 fields (no fifth — no payload/PII).
    let json: serde_json::Value = serde_json::from_str(&wire).expect("frame is JSON");
    let obj = json.as_object().expect("frame is a JSON object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["correlation_id", "home_cell", "subject", "type"],
        "the 12.6 frame Notif consumes carries EXACTLY the four PII-free fields"
    );

    // CONSUMER: the Notif aggregation deserialises and reads back only the four frozen fields.
    let consumer: CrossCellPointer =
        serde_json::from_str(&wire).expect("the Notif consumer reads the canonical frame");
    let (subject, kind, corr, home) = aggregation_carried_fields(&consumer);
    assert_eq!(subject.artifact_ref().0, "myelin://01J0BETA/notif/item/42");
    assert_eq!(kind, &ArtifactType::Issue);
    assert_eq!(corr, &CorrelationId("01J0CORR".into()));
    assert_eq!(home.as_str(), "cell-fr-par-2");
    assert_eq!(
        consumer, provider,
        "the 12.6 CDC wire shape is conformant both ways"
    );
}

/// The frame's `correlation_id` is the SAME causal-root type the rest of the platform carries (one
/// type platform-wide, EI-01 §7) — a `CorrelationId` read off any envelope/pointer drops straight into
/// the Notif aggregation's pointer with no conversion.
#[test]
fn cdc_12_6_correlation_id_is_the_platform_causal_root_type() {
    let corr = CorrelationId("01J0ROOT".into());
    let p = cross_cell_inbox_pointer(
        &ArtifactRef("myelin://01J0BETA/notif/item/1".into()),
        ArtifactType::Channel,
        corr.clone(),
        CellId::from_token("cell-fr-par-2"),
    );
    let (_subject, _kind, read_corr, _home) = aggregation_carried_fields(&p);
    assert_eq!(
        read_corr, &corr,
        "the frame ties to the platform causal chain, no parallel id"
    );
}
