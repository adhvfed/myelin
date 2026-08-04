use myelin_identity::Principal;
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};

pub use myelin_tenancy::ArtifactRef;

pub use myelin_tenancy::CorrelationId;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CausedBy(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventType(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AggregateKey(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor(pub Principal);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataRole {
    Controller,
    Processor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Internal,
    Private,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiiKeyRef(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Timestamp(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_id: EventId,
    pub type_: EventType,
    pub schema_ver: u32,
    pub tenant: TenantId,
    pub region: Region,
    pub actor: Actor,
    pub subject: ArtifactRef,
    pub aggregate: AggregateKey,
    pub causation_id: Option<EventId>,
    pub correlation_id: CorrelationId,
    pub caused_by: Option<CausedBy>,
    pub depth: u32,
    pub contains_personal_data: bool,
    pub data_role: DataRole,
    pub visibility: Visibility,
    pub pii_key_ref: Option<PiiKeyRef>,
    pub occurred_at: Timestamp,
    pub recorded_at: Timestamp,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventDraft {
    pub type_: EventType,
    pub subject: ArtifactRef,
    pub aggregate: AggregateKey,
    pub payload: serde_json::Value,
    pub data_role: DataRole,
    pub visibility: Visibility,
    pub contains_personal_data: bool,
    pub pii_key_ref: Option<PiiKeyRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmitContext {
    pub event_id: EventId,
    pub tenant: TenantId,
    pub region: Region,
    pub actor: Actor,
    pub schema_ver: u32,
    pub occurred_at: Timestamp,
    pub recorded_at: Timestamp,
    pub caused_by: Option<CausedBy>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedEventCause {
    pub event_id: EventId,
    pub correlation_id: CorrelationId,
    pub caused_by: Option<CausedBy>,
    pub depth: u32,
}

impl PersistedEventCause {
    pub fn from_envelope(envelope: &EventEnvelope) -> PersistedEventCause {
        PersistedEventCause {
            event_id: envelope.event_id.clone(),
            correlation_id: envelope.correlation_id.clone(),
            caused_by: envelope.caused_by.clone(),
            depth: envelope.depth,
        }
    }
}

pub fn derive_envelope(
    draft: EventDraft,
    ctx: EmitContext,
    cause: Option<&EventEnvelope>,
) -> EventEnvelope {
    let cause = cause.map(PersistedEventCause::from_envelope);
    derive_envelope_from_persisted_cause(draft, ctx, cause.as_ref())
}

pub fn derive_envelope_from_persisted_cause(
    draft: EventDraft,
    ctx: EmitContext,
    cause: Option<&PersistedEventCause>,
) -> EventEnvelope {
    let (causation_id, correlation_id, depth, caused_by) = match cause {
        None => (
            None,
            CorrelationId(ctx.event_id.0.clone()),
            0,
            ctx.caused_by.clone(),
        ),
        Some(parent) => (
            Some(parent.event_id.clone()),
            parent.correlation_id.clone(),
            parent.depth.saturating_add(1),
            parent.caused_by.clone(),
        ),
    };

    EventEnvelope {
        event_id: ctx.event_id,
        type_: draft.type_,
        schema_ver: ctx.schema_ver,
        tenant: ctx.tenant,
        region: ctx.region,
        actor: ctx.actor,
        subject: draft.subject,
        aggregate: draft.aggregate,
        causation_id,
        correlation_id,
        caused_by,
        depth,
        contains_personal_data: draft.contains_personal_data,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: draft.pii_key_ref,
        occurred_at: ctx.occurred_at,
        recorded_at: ctx.recorded_at,
        payload: draft.payload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn sample_principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn anchor_envelope() -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01J0".into()),
            type_: EventType("issues.issue.created".into()),
            schema_ver: 1u32,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(sample_principal()),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: Some(EventId("01J-parent".into())),
            correlation_id: CorrelationId("root".into()),
            caused_by: Some(CausedBy("session:abc".into())),
            depth: 4u32,
            contains_personal_data: true,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: Some(PiiKeyRef("kms://acme/3/subject:u42".into())),
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            payload: serde_json::json!({ "ref": "myelin://acme/issues/issue/PROJ-1" }),
        }
    }

    fn ctx_for(event_id: EventId, caused_by: Option<CausedBy>) -> EmitContext {
        EmitContext {
            event_id,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(sample_principal()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            caused_by,
        }
    }

    fn draft_for(type_: &str) -> EventDraft {
        EventDraft {
            type_: EventType(type_.into()),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            payload: serde_json::json!({ "ref": "myelin://acme/issues/issue/PROJ-1" }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    #[test]
    fn eb01_full_field_round_trip_and_depth_derivation_is_lossless() {
        let env = anchor_envelope();
        assert!(
            env.causation_id.is_some(),
            "fixture exercises the immediate-parent leg"
        );
        assert!(
            env.caused_by.is_some(),
            "fixture exercises the human-action ref"
        );
        assert!(
            env.pii_key_ref.is_some(),
            "fixture exercises a populated pii_key_ref"
        );
        assert_ne!(env.depth, 0, "fixture exercises a non-root depth");

        let json = serde_json::to_string(&env).expect("envelope serialises");
        let back: EventEnvelope = serde_json::from_str(&json).expect("envelope deserialises");
        assert_eq!(
            back, env,
            "every field round-trips lossless (the X-5 anchor is well-defined)"
        );
        assert_eq!(back.causation_id, env.causation_id);
        assert_eq!(back.correlation_id, env.correlation_id);
        assert_eq!(back.caused_by, env.caused_by);
        assert_eq!(back.depth, env.depth);
        assert_eq!(back.pii_key_ref, env.pii_key_ref);

        let parent = derive_envelope(
            draft_for("issues.issue.created"),
            ctx_for(
                EventId("01J-root".into()),
                Some(CausedBy("session:abc".into())),
            ),
            None,
        );
        assert_eq!(parent.depth, 0, "a root is at depth 0");
        let child = derive_envelope(
            draft_for("refs.edge.created"),
            ctx_for(EventId("01J-child".into()), None),
            Some(&parent),
        );
        assert_eq!(
            child.depth,
            parent.depth + 1,
            "child depth = parent depth + 1 (BUS-5)"
        );
        assert_eq!(
            child.causation_id,
            Some(parent.event_id.clone()),
            "causation_id is the immediate parent (nested-not-flat)"
        );
        assert_eq!(
            child.correlation_id, parent.correlation_id,
            "correlation_id (the root) carries through unchanged"
        );

        let cjson = serde_json::to_string(&child).expect("child serialises");
        let cback: EventEnvelope = serde_json::from_str(&cjson).expect("child deserialises");
        assert_eq!(
            cback, child,
            "a derived (caused) envelope round-trips lossless too"
        );
    }

    #[test]
    fn emit_root_carries_its_own_correlation_at_depth_zero() {
        let id = EventId("01J-root".into());
        let ctx = ctx_for(id.clone(), Some(CausedBy("session:abc".into())));
        let env = derive_envelope(draft_for("issues.issue.created"), ctx, None);

        assert_eq!(
            env.event_id, id,
            "the minted id is carried onto the envelope"
        );
        assert_eq!(env.causation_id, None, "a root has no immediate parent");
        assert_eq!(
            env.correlation_id,
            CorrelationId("01J-root".into()),
            "a root carries its OWN id as the correlation/root (BUS-5)"
        );
        assert_eq!(env.depth, 0, "a root is at causal depth 0");
        assert_eq!(
            env.caused_by,
            Some(CausedBy("session:abc".into())),
            "the root defines the human-action ref for the chain"
        );
    }

    #[test]
    fn emit_caused_derives_provenance_from_the_parent() {
        let parent = derive_envelope(
            draft_for("issues.issue.created"),
            ctx_for(
                EventId("01J-root".into()),
                Some(CausedBy("session:abc".into())),
            ),
            None,
        );

        let child = derive_envelope(
            draft_for("refs.edge.created"),
            ctx_for(
                EventId("01J-child".into()),
                Some(CausedBy("session:WRONG".into())),
            ),
            Some(&parent),
        );

        assert_eq!(
            child.causation_id,
            Some(EventId("01J-root".into())),
            "causation_id = the IMMEDIATE parent's event id"
        );
        assert_eq!(
            child.correlation_id,
            CorrelationId("01J-root".into()),
            "correlation_id = the parent's ROOT, carried through unchanged"
        );
        assert_eq!(child.depth, 1, "depth = parent.depth + 1");
        assert_eq!(
            child.caused_by,
            Some(CausedBy("session:abc".into())),
            "the originating human action is INHERITED from the parent, not re-seeded \
             from the child's own context (a deep chain still attributes to the human)"
        );
    }

    #[test]
    fn persisted_cause_derivation_is_identical_to_full_parent_derivation() {
        let parent = anchor_envelope();
        let persisted = PersistedEventCause::from_envelope(&parent);
        let draft = draft_for("ci.check.updated");
        let ctx = ctx_for(EventId("01J-child-from-state".into()), None);

        let from_envelope = derive_envelope(draft.clone(), ctx.clone(), Some(&parent));
        let from_persisted = derive_envelope_from_persisted_cause(draft, ctx, Some(&persisted));

        assert_eq!(
            from_persisted, from_envelope,
            "durable causal provenance must derive the byte-identical child envelope"
        );
    }

    #[test]
    fn emit_deep_chain_keeps_root_and_increments_depth_monotonically() {
        let root = derive_envelope(
            draft_for("issues.issue.created"),
            ctx_for(EventId("01J-0".into()), Some(CausedBy("human:h1".into()))),
            None,
        );

        let mut prev = root.clone();
        for i in 1..=10u32 {
            let next = derive_envelope(
                draft_for("refs.edge.created"),
                ctx_for(
                    EventId(format!("01J-{i}")),
                    Some(CausedBy("human:DECOY".into())),
                ),
                Some(&prev),
            );
            assert_eq!(next.depth, i, "depth increments by exactly 1 per hop");
            assert!(next.depth > prev.depth, "depth is monotonically increasing");
            assert_eq!(
                next.correlation_id, root.correlation_id,
                "the causal root carries through the entire chain"
            );
            assert_eq!(next.causation_id, Some(prev.event_id.clone()));
            assert_eq!(next.caused_by, Some(CausedBy("human:h1".into())));
            prev = next;
        }
    }

    #[test]
    fn emit_depth_saturates_never_wraps() {
        let mut maxed = derive_envelope(
            draft_for("issues.issue.created"),
            ctx_for(EventId("01J-deep".into()), None),
            None,
        );
        maxed.depth = u32::MAX;

        let child = derive_envelope(
            draft_for("refs.edge.created"),
            ctx_for(EventId("01J-deeper".into()), None),
            Some(&maxed),
        );
        assert_eq!(
            child.depth,
            u32::MAX,
            "depth saturates at u32::MAX, never wraps to 0"
        );
    }

    #[test]
    fn emit_passes_caller_authored_fields_through_unchanged() {
        let draft = draft_for("issues.issue.created");
        let expected_type = draft.type_.clone();
        let expected_subject = draft.subject.clone();
        let expected_payload = draft.payload.clone();

        let env = derive_envelope(draft, ctx_for(EventId("01J".into()), None), None);

        assert_eq!(env.type_, expected_type);
        assert_eq!(env.subject, expected_subject);
        assert_eq!(env.payload, expected_payload);
        assert_eq!(env.data_role, DataRole::Controller);
        assert_eq!(env.visibility, Visibility::Internal);
        assert_eq!(env.tenant, TenantId("acme".into()));
        assert_eq!(env.region, Region("eu-west".into()));
        assert_eq!(env.schema_ver, 1);
    }

    #[test]
    fn surface_event_envelope_field_shape_is_frozen() {
        let env = anchor_envelope();

        let _: &EventId = &env.event_id;
        let _: &EventType = &env.type_;
        let _: &u32 = &env.schema_ver;
        let _: &TenantId = &env.tenant;
        let _: &Region = &env.region;
        let _: &Actor = &env.actor;
        let _: &ArtifactRef = &env.subject;
        let _: &AggregateKey = &env.aggregate;
        let _: &Option<EventId> = &env.causation_id;
        let _: &CorrelationId = &env.correlation_id;
        let _: &Option<CausedBy> = &env.caused_by;
        let _: &u32 = &env.depth;
        let _: &bool = &env.contains_personal_data;
        let _: &DataRole = &env.data_role;
        let _: &Visibility = &env.visibility;
        let _: &Option<PiiKeyRef> = &env.pii_key_ref;
        let _: &Timestamp = &env.occurred_at;
        let _: &Timestamp = &env.recorded_at;
        let _: &serde_json::Value = &env.payload;

        for ts in [&env.occurred_at.0, &env.recorded_at.0] {
            assert!(
                ts.contains('T'),
                "timestamp must be RFC-3339 (date T time): {ts}"
            );
            assert!(
                ts.ends_with('Z'),
                "timestamp must be UTC (Z-suffixed): {ts}"
            );
        }
        assert_eq!(env.depth, 4u32);
        assert_eq!(env.causation_id, Some(EventId("01J-parent".into())));
        assert_eq!(env.correlation_id, CorrelationId("root".into()));
        let pkr = &env.pii_key_ref.as_ref().expect("anchor sets pii_key_ref").0;
        assert!(
            pkr.starts_with("kms://"),
            "pii_key_ref must be a kms:// URN: {pkr}"
        );
        let rest = pkr.strip_prefix("kms://").unwrap();
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        assert_eq!(parts.len(), 3, "kms://<tenant>/<dek-epoch>/<class>: {pkr}");
        assert_eq!(parts[0], "acme", "tenant segment");
        assert!(
            parts[1].parse::<u64>().is_ok(),
            "dek-epoch is an integer: {}",
            parts[1]
        );
        let class = parts[2];
        assert!(
            class == "tenant"
                || class == "blob"
                || class
                    .strip_prefix("subject:")
                    .is_some_and(|id| !id.is_empty()),
            "class ∈ {{tenant, subject:<id>, blob}}: {class}"
        );
        assert!(env.payload.is_object());
    }

    #[test]
    fn cdc_2_1_envelope_wire_shape_is_the_anchor() {
        let env = anchor_envelope();
        let json = serde_json::to_value(&env).expect("envelope serializes");
        let obj = json.as_object().expect("envelope is a JSON object");

        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected = [
            "event_id",
            "type_",
            "schema_ver",
            "tenant",
            "region",
            "actor",
            "subject",
            "aggregate",
            "causation_id",
            "correlation_id",
            "caused_by",
            "depth",
            "contains_personal_data",
            "data_role",
            "visibility",
            "pii_key_ref",
            "occurred_at",
            "recorded_at",
            "payload",
        ];
        expected.sort_unstable();
        assert_eq!(
            keys, expected,
            "the 2.1 envelope wire key set is frozen (X-5 anchor)"
        );

        assert!(
            obj["schema_ver"].is_u64(),
            "schema_ver is an integer on the wire"
        );
        assert!(obj["depth"].is_u64(), "depth is an integer on the wire");
        assert_eq!(
            obj["occurred_at"],
            serde_json::json!("2026-06-19T00:00:00Z")
        );
        assert_eq!(
            obj["recorded_at"],
            serde_json::json!("2026-06-19T00:00:01Z")
        );
        assert_eq!(
            obj["pii_key_ref"],
            serde_json::json!("kms://acme/3/subject:u42")
        );
        assert!(
            obj["payload"].is_object(),
            "payload carries references, not a PII body"
        );

        let back: EventEnvelope = serde_json::from_value(json).expect("envelope round-trips");
        assert_eq!(
            back, env,
            "the wire shape round-trips to the anchor (no lossy field)"
        );
    }
}
