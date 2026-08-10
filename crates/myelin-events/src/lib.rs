pub mod check_seam;
pub mod consumer;
pub mod crosscell;
pub mod crosscell_propagation;
pub mod dead_letter;
pub mod dedup;
pub mod envelope;
pub mod firehose;
pub mod harness;
pub mod holder;
pub mod retention;
#[cfg(feature = "nats")]
pub mod nats;
pub mod outbox;
pub mod partition;
pub mod reerase;
pub mod reindex;
pub mod relay;
pub mod residency;
pub mod taxonomy;
pub mod telemetry;
pub mod upcast;

pub use check_seam::{
    check_aggregate, check_subject, check_updated_draft, ci_result_draft, ci_result_subject,
    rollup_ci_result, CheckSeamError, CheckSeamOrder, CiOverall, CiResult, CiResultWaitSubstrate,
    OrderedCheck, WakeOutcome,
};
pub use consumer::{
    consume, install_payload_free_panic_hook, Consumer, ConsumerName, ConsumerSpec, DeadLetter,
    Delivered, Message, PerTenantInflight, PrefetchBound, SubscribeError, Subscription,
};
pub use crosscell::{
    assert_cell_agnostic, pointer_correlation, ArtifactType, CellId, CrossCellPointer,
    OpaqueSubjectId,
};
pub use crosscell_propagation::{
    pointer_for_propagation, propagated_carried_fields, CrossCellPropagator, CrossCellStream,
    PropagatedPointer,
};
pub use dead_letter::{
    bounded_reason, DeadLetterRecord, DeadLetterSink, DurableDeadLetter,
    CONSUMER_DEAD_LETTER_MIGRATION, MAX_REASON_LEN,
};
pub use dedup::{CoCommitError, CoCommitTx, DedupLedger, DurableDedup, CONSUMER_DEDUP_MIGRATION};
pub use firehose::{
    Firehose, FirehoseError, FirehoseScope, Frame, FrameDraft, FramePayload, RetentionWindow,
    ScopeKind, SubStream, Subscription as FirehoseSubscription, DEFAULT_INFLIGHT_CAP,
};
pub use harness::{
    HarnessError, PayloadShape, RegisteredToken, SubsystemTokenList, TokenListHarness,
};
pub use outbox::{
    DurableOutboxBacking, EmitContextBase, IdMinter, MonotonicMinter, OutboxRow, OutboxStore,
    OutboxTransaction, Ulid, UlidMinter, OUTBOX_MIGRATION, OUTBOX_PUBLISHER_GRANTS_MIGRATION,
    OUTBOX_PUBLISHER_GRANT_SCOPE_MIGRATION, OUTBOX_QUARANTINE_MIGRATION,
};
pub use partition::{
    stream_name_for, PartitionKey, StreamSubject, SubjectComponent, SubjectComponentError,
    SubjectError, MAX_ENCODED_COMPONENT_BYTES, MAX_STREAM_SUBJECT_BYTES, MAX_SUBJECT_TOKEN_BYTES,
    SUBJECT_ROOT,
};
pub use relay::{
    dlq_subject, BrokerDelivery, BrokerDeliveryBody, BrokerDeliveryRef, BusTransport,
    DeadLetterAlert, Delivery, DeliveryPoisonKind, DeliveryQuarantineReason, DeliveryToken,
    DrainReport, DurableDeliveryQuarantine, EventConsumer, InProcessBus, Relay, TransportError,
    CONSUMER_DELIVERY_QUARANTINE_MIGRATION, MAX_PUBLISH_ATTEMPTS,
};
pub use residency::{BusRegionReport, BusResidencySignal, BusStreamResidency, ResidencyError};
pub use retention::{RetentionTuning, StreamClass};
pub use taxonomy::{
    resolve_automation_subject_type, validate as validate_event_type, AutomationSubjectTypeError,
    TaxonomyError, ARTIFACT_TYPE_TOKENS, AUTOMATION_SUBJECT_TYPE_TOKENS, SEED_EVENT_NAMES,
    SUBSYSTEM_TOKENS,
};
pub use telemetry::{
    BusObservations, BusSignal, BusSignals, MetricLabel, MetricRecorder, MetricSample, MetricsSink,
};
pub use upcast::{RegisterError, UpcastError, UpcasterRegistry};

pub use holder::{
    degrade_on_tombstone, BusEventLog, BusHolder, EraseReceipt, ExportedEvent, InMemoryShredder,
    InlinePiiShredder, LocateReport, LocatedEvent, ShredError, BUS_ERASED_TYPE, ERASED_EVENT_NAME,
};
pub use reerase::{BusErasureLedger, DurableBusErasure, ErasedSubject, ReErasureReceipt};

pub use reindex::{
    reindex, snapshot_event_id, DerivedStore, ReferenceReindexSource, ReindexError, ReindexReceipt,
    ReindexSource, SnapshotDraft, SnapshotScope, SNAPSHOT_EVENT_NAME,
};

use serde::{Deserialize, Serialize};

pub use envelope::{
    derive_envelope, derive_envelope_from_persisted_cause, Actor, AggregateKey, ArtifactRef,
    CausedBy, CorrelationId, DataRole, EmitContext, EventDraft, EventEnvelope, EventId, EventType,
    PersistedEventCause, PiiKeyRef, Timestamp, Visibility,
};

pub use myelin_tenancy::{Region, TenantId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxError(pub String);

pub type Result<T> = core::result::Result<T, OutboxError>;

pub trait OutboxTx {
    fn emit(&mut self, draft: EventDraft, cause: Option<&EventEnvelope>) -> Result<EventId>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectPattern(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backoff {
    pub seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandleOutcome {
    Done,
    NonRetryable(Reason),
    Retry(Backoff),
    DependencyUnavailable {
        dependency: relay::IntakeDependency,
        backoff: Backoff,
    },
}

pub struct HandlerTx<'a> {
    conn: Option<&'a mut dyn core::any::Any>,
}

impl<'a> HandlerTx<'a> {
    pub fn with_connection(conn: &'a mut dyn core::any::Any) -> HandlerTx<'a> {
        HandlerTx { conn: Some(conn) }
    }

    pub fn none() -> HandlerTx<'static> {
        HandlerTx { conn: None }
    }

    pub fn connection<T: core::any::Any>(&mut self) -> Option<&mut T> {
        self.conn.as_deref_mut().and_then(|c| c.downcast_mut::<T>())
    }

    pub fn is_durable(&self) -> bool {
        self.conn.is_some()
    }
}

pub trait EventHandler {
    fn subjects(&self) -> &'static [SubjectPattern];
    fn handle(&self, ev: &EventEnvelope, tx: &mut HandlerTx<'_>) -> HandleOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn sample_principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
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
    fn cdc_2_2_emit_is_the_only_path_and_derives_causality() {
        struct Tx {
            next: u32,
        }
        impl OutboxTx for Tx {
            fn emit(
                &mut self,
                draft: EventDraft,
                cause: Option<&EventEnvelope>,
            ) -> Result<EventId> {
                let id = EventId(format!("01J-{}", self.next));
                self.next += 1;
                let env =
                    derive_envelope(draft, ctx_for(id, Some(CausedBy("human:h".into()))), cause);
                Ok(env.event_id)
            }
        }

        let mut tx = Tx { next: 0 };
        let root_id = tx
            .emit(draft_for("issues.issue.created"), None)
            .expect("root emits");
        assert_eq!(root_id, EventId("01J-0".into()));

        let root_env = derive_envelope(
            draft_for("issues.issue.created"),
            ctx_for(EventId("01J-0".into()), Some(CausedBy("human:h".into()))),
            None,
        );
        let child_id = tx
            .emit(draft_for("refs.edge.created"), Some(&root_env))
            .expect("caused emits");
        assert_eq!(child_id, EventId("01J-1".into()));

        let _obj: &mut dyn OutboxTx = &mut tx;
    }

    #[test]
    fn outbox_has_only_emit_no_publish_now() {
        struct Stub;
        impl OutboxTx for Stub {
            fn emit(
                &mut self,
                draft: EventDraft,
                cause: Option<&EventEnvelope>,
            ) -> Result<EventId> {
                let ctx = ctx_for(EventId("01J-stub".into()), None);
                let env = derive_envelope(draft, ctx, cause);
                Ok(env.event_id)
            }
        }
        let _s = Stub;
    }

    #[test]
    fn event_handler_template_shape_is_frozen() {
        struct Idx;
        static SUBJECTS: &[SubjectPattern] = &[];
        impl EventHandler for Idx {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, _ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
                HandleOutcome::Done
            }
        }
        let h = Idx;
        assert!(h.subjects().is_empty());
        assert_eq!(
            h.handle(&sample_envelope(), &mut HandlerTx::none()),
            HandleOutcome::Done
        );
    }

    fn sample_envelope() -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01J0".into()),
            type_: EventType("t.a.e".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(sample_principal()),
            subject: ArtifactRef("myelin://acme/t/a/1".into()),
            aggregate: AggregateKey("a:1".into()),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Processor,
            visibility: Visibility::Private,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
            payload: serde_json::Value::Null,
        }
    }
}
