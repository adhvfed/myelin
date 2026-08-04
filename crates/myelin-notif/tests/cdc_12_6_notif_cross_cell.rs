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

#[test]
fn cdc_notif_consumes_12_6_frame_only_four_fields() {
    let provider = sample_pointer();
    let wire = serde_json::to_string(&provider).expect("provider emits the canonical 12.6 frame");

    let json: serde_json::Value = serde_json::from_str(&wire).expect("frame is JSON");
    let obj = json.as_object().expect("frame is a JSON object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["correlation_id", "home_cell", "subject", "type"],
        "the 12.6 frame Notif consumes carries EXACTLY the four PII-free fields"
    );

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
