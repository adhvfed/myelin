use crate::{ArtifactRef, CorrelationId};

pub use myelin_tenancy::{ArtifactType, CellId, CrossCellPointer, OpaqueSubjectId};

#[must_use]
pub fn assert_cell_agnostic(pointer: &CrossCellPointer) -> &ArtifactRef {
    let _home: &CellId = pointer.home_cell();
    pointer.subject().artifact_ref()
}

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

    fn sample_pointer() -> CrossCellPointer {
        CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
            ArtifactType::Issue,
            CorrelationId("01J0CORR".into()),
            CellId::from_token("cell-fr-par-1"),
        )
    }

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
            "the pinned frame carries EXACTLY the four §6.1 fields - no payload/PII/authz state"
        );
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

        assert_eq!(
            back.subject().artifact_ref().0,
            "myelin://01J0ACME/issues/issue/42"
        );
        assert_eq!(back.artifact_type(), &ArtifactType::Issue);
        assert_eq!(back.correlation_id(), &CorrelationId("01J0CORR".into()));
        assert_eq!(back.home_cell().as_str(), "cell-fr-par-1");
    }

    #[test]
    fn section_5_surfaces_are_cell_agnostic_they_take_the_opaque_subject() {
        let p = sample_pointer();
        let subject: &ArtifactRef = assert_cell_agnostic(&p);
        assert_eq!(subject.0, "myelin://01J0ACME/issues/issue/42");
        assert_eq!(pointer_correlation(&p), &CorrelationId("01J0CORR".into()));
    }

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
        assert_eq!(pointer_correlation(&p), &env.correlation_id);
    }

    #[test]
    fn cdc_12_6_bus_provider_emits_and_consumer_reads_only_four_fields() {
        let provider = sample_pointer();
        let wire = serde_json::to_string(&provider).expect("provider emits canonical frame");

        let consumer: CrossCellPointer =
            serde_json::from_str(&wire).expect("consumer reads the canonical frame");
        let routed_subject = assert_cell_agnostic(&consumer);
        assert_eq!(routed_subject.0, "myelin://01J0ACME/issues/issue/42");
        assert_eq!(consumer.home_cell().as_str(), "cell-fr-par-1");
        assert_eq!(
            consumer, provider,
            "the CDC wire shape is conformant both ways"
        );
    }

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
