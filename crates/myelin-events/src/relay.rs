#[cfg(any(test, feature = "test-support"))]
use crate::outbox::OutboxRow;
use crate::outbox::OutboxStore;
use crate::{ArtifactRef, EventEnvelope, EventId, TenantId, Timestamp};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub trait BusTransport: Send + Sync {
    fn put(
        &self,
        subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> std::result::Result<Delivery, TransportError>;

    fn consume(&self, subject_prefix: &str) -> Vec<EventEnvelope>;

    fn ack(&self, consumer: &str, event_id: &EventId);

    fn purge(&self);
}

pub trait EventPublisher: Send + Sync {
    fn publish(
        &self,
        subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> std::result::Result<Delivery, TransportError>;
}

pub trait EventConsumer: Send + Sync {
    fn durable_name(&self) -> &str;

    fn pre_intake_readiness(
        &self,
    ) -> std::result::Result<Option<IntakeDependency>, IntakeDependency> {
        Ok(None)
    }

    fn consume(
        &self,
        subject_prefix: &str,
    ) -> std::result::Result<Vec<BrokerDelivery>, TransportError>;

    fn flush_settlements(&self) -> std::result::Result<(), TransportError>;

    fn ack(&self, token: DeliveryToken) -> std::result::Result<(), TransportError>;

    fn retry(
        &self,
        token: DeliveryToken,
        delay_secs: u64,
    ) -> std::result::Result<(), TransportError>;

    fn terminate(&self, token: DeliveryToken) -> std::result::Result<(), TransportError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IntakeDependency {
    Blob,
}

impl IntakeDependency {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Blob => "blob",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeliveryToken(u64);

impl DeliveryToken {
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 {
            None
        } else {
            Some(Self(value))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerDeliveryRef {
    pub stream: String,
    pub stream_sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryPoisonKind {
    MalformedEnvelope,
    SubjectMismatch,
}

impl DeliveryPoisonKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::MalformedEnvelope => "malformed_envelope",
            Self::SubjectMismatch => "subject_mismatch",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrokerDeliveryBody {
    Event(Box<EventEnvelope>),
    Poison(DeliveryPoisonKind),
    TransientMetadataFault,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerDelivery {
    pub token: DeliveryToken,
    pub broker_ref: Option<BrokerDeliveryRef>,
    pub body: BrokerDeliveryBody,
    pub delivery_attempt: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryQuarantineReason {
    MalformedEnvelope,
    SubjectMismatch,
    NoRegisteredConsumer,
}

impl DeliveryQuarantineReason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::MalformedEnvelope => "malformed_envelope",
            Self::SubjectMismatch => "subject_mismatch",
            Self::NoRegisteredConsumer => "no_registered_consumer",
        }
    }
}

impl From<DeliveryPoisonKind> for DeliveryQuarantineReason {
    fn from(value: DeliveryPoisonKind) -> Self {
        match value {
            DeliveryPoisonKind::MalformedEnvelope => Self::MalformedEnvelope,
            DeliveryPoisonKind::SubjectMismatch => Self::SubjectMismatch,
        }
    }
}

pub trait DurableDeliveryQuarantine: Send + Sync {
    fn record(
        &self,
        consumer: &str,
        broker_ref: &BrokerDeliveryRef,
        reason: DeliveryQuarantineReason,
        delivery_attempt: u64,
    ) -> Result<(), String>;
}

pub const CONSUMER_DELIVERY_QUARANTINE_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS consumer_delivery_quarantine (
    consumer        TEXT NOT NULL,
    stream          TEXT NOT NULL,
    stream_sequence BIGINT NOT NULL,
    reason_code     TEXT NOT NULL,
    delivery_attempt BIGINT NOT NULL,
    quarantined_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer, stream, stream_sequence),
    CONSTRAINT consumer_delivery_quarantine_sequence_positive CHECK (stream_sequence > 0),
    CONSTRAINT consumer_delivery_quarantine_attempt_positive CHECK (delivery_attempt > 0),
    CONSTRAINT consumer_delivery_quarantine_reason_fixed CHECK (
        reason_code IN ('malformed_envelope', 'subject_mismatch', 'no_registered_consumer')
    )
);"#;

impl<T: BusTransport + ?Sized> EventPublisher for T {
    fn publish(
        &self,
        subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> std::result::Result<Delivery, TransportError> {
        self.put(subject, envelope, dedup_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Accepted,
    Deduplicated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportError(pub String);

#[derive(Clone, Default)]
pub struct InProcessBus {
    inner: Arc<Mutex<BusInner>>,
}

#[derive(Default)]
struct BusInner {
    delivered: Vec<(ArtifactRef, EventEnvelope)>,
    accepted_ids: HashSet<EventId>,
    acks: HashMap<String, EventId>,
    fail_with: Option<TransportError>,
    fail_next: u32,
}

impl InProcessBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sever(&self) {
        self.lock().fail_with = Some(TransportError(
            "broker unreachable (severed by drill)".into(),
        ));
    }

    pub fn heal(&self) {
        let mut inner = self.lock();
        inner.fail_with = None;
        inner.fail_next = 0;
    }

    pub fn fail_next(&self, n: u32) {
        self.lock().fail_next = n;
    }

    pub fn delivered_count(&self) -> usize {
        self.lock().delivered.len()
    }

    pub fn delivered_ids(&self) -> HashSet<EventId> {
        self.lock()
            .delivered
            .iter()
            .map(|(_, e)| e.event_id.clone())
            .collect()
    }

    pub fn ack_of(&self, consumer: &str) -> Option<EventId> {
        self.lock().acks.get(consumer).cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BusInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl BusTransport for InProcessBus {
    fn put(
        &self,
        subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> std::result::Result<Delivery, TransportError> {
        let mut inner = self.lock();
        if let Some(err) = &inner.fail_with {
            return Err(err.clone());
        }
        if inner.fail_next > 0 {
            inner.fail_next -= 1;
            return Err(TransportError("broker transient outage (fail_next)".into()));
        }
        if inner.accepted_ids.contains(dedup_id) {
            return Ok(Delivery::Deduplicated);
        }
        inner.accepted_ids.insert(dedup_id.clone());
        inner.delivered.push((subject.clone(), envelope.clone()));
        Ok(Delivery::Accepted)
    }

    fn consume(&self, subject_prefix: &str) -> Vec<EventEnvelope> {
        self.lock()
            .delivered
            .iter()
            .filter(|(subject, _)| subject.0.starts_with(subject_prefix))
            .map(|(_, e)| e.clone())
            .collect()
    }

    fn ack(&self, consumer: &str, event_id: &EventId) {
        self.lock()
            .acks
            .insert(consumer.to_string(), event_id.clone());
    }

    fn purge(&self) {
        let mut inner = self.lock();
        inner.delivered.clear();
        inner.accepted_ids.clear();
        inner.acks.clear();
    }
}

impl BusTransport for Box<dyn BusTransport> {
    fn put(
        &self,
        subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> std::result::Result<Delivery, TransportError> {
        (**self).put(subject, envelope, dedup_id)
    }

    fn consume(&self, subject_prefix: &str) -> Vec<EventEnvelope> {
        (**self).consume(subject_prefix)
    }

    fn ack(&self, consumer: &str, event_id: &EventId) {
        (**self).ack(consumer, event_id)
    }

    fn purge(&self) {
        (**self).purge()
    }
}

pub const MAX_PUBLISH_ATTEMPTS: u32 = 5;

pub const DEFAULT_DRAIN_BATCH: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadLetterAlert {
    pub dlq_subject: String,
    pub event_id: EventId,
    pub tenant: TenantId,
    pub subsystem: String,
    pub attempts: u32,
}

pub fn dlq_subject(tenant: &TenantId, subsystem: &str) -> String {
    format!("dlq.{}.{}", tenant.0, subsystem)
}

#[cfg(any(test, feature = "test-support"))]
fn subsystem_of(envelope: &EventEnvelope) -> String {
    envelope
        .type_
        .0
        .split('.')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DrainReport {
    pub published: usize,
    pub deduplicated: usize,
    pub failed: usize,
    pub dead_lettered: usize,
    pub drain_errors: usize,
}

impl DrainReport {
    fn durable_failure() -> Self {
        Self {
            drain_errors: 1,
            ..Self::default()
        }
    }

    fn add_drain_errors(&mut self, errors: usize) {
        self.drain_errors += errors;
    }
}

pub struct Relay<T: BusTransport> {
    store: OutboxStore,
    transport: T,
    #[cfg_attr(not(any(test, feature = "test-support")), allow(dead_code))]
    clock: Box<dyn Fn() -> Timestamp + Send + Sync>,
    dead_letter_alerts: Arc<Mutex<Vec<DeadLetterAlert>>>,
}

impl<T: BusTransport> Relay<T> {
    pub fn new(
        store: OutboxStore,
        transport: T,
        clock: impl Fn() -> Timestamp + Send + Sync + 'static,
    ) -> Self {
        Relay {
            store,
            transport,
            clock: Box::new(clock),
            dead_letter_alerts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn dead_letter_alerts(&self) -> Vec<DeadLetterAlert> {
        self.dead_letter_alerts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn gc_published(&self, published_before: &Timestamp) -> usize {
        #[cfg(any(test, feature = "test-support"))]
        {
            self.store.gc_published_before(published_before)
        }
        #[cfg(not(any(test, feature = "test-support")))]
        {
            let _ = published_before;
            0
        }
    }

    pub fn drain_once(&self) -> DrainReport {
        if let Some(backing) = self.store.durable_backing() {
            return match backing.drain_once(&self.transport, DEFAULT_DRAIN_BATCH) {
                Ok(report) => report,
                Err(e) => {
                    eprintln!(
                        "[outbox-relay] LOUD: durable drain_once FAILED - rows stay claimable \
                         (0 lost), depth/age signals remain the alarm: {}",
                        e.0
                    );
                    DrainReport::durable_failure()
                }
            };
        }
        #[cfg(any(test, feature = "test-support"))]
        {
            let mut report = DrainReport::default();
            let claimed = self.store.claim_unsent();
            for row in claimed {
                match self
                    .transport
                    .put(&row.subject, &row.envelope, &row.event_id)
                {
                    Ok(Delivery::Accepted) => {
                        self.store.mark_published(&row.event_id, (self.clock)());
                        report.published += 1;
                    }
                    Ok(Delivery::Deduplicated) => {
                        self.store.mark_published(&row.event_id, (self.clock)());
                        report.deduplicated += 1;
                    }
                    Err(_) => {
                        let attempts = self.store.fail_attempt(&row.event_id);
                        if attempts >= MAX_PUBLISH_ATTEMPTS {
                            self.store.dead_letter(&row.event_id);
                            let subsystem = subsystem_of(&row.envelope);
                            self.dead_letter_alerts
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(DeadLetterAlert {
                                    dlq_subject: dlq_subject(&row.envelope.tenant, &subsystem),
                                    event_id: row.event_id.clone(),
                                    tenant: row.envelope.tenant.clone(),
                                    subsystem,
                                    attempts,
                                });
                            report.dead_lettered += 1;
                        } else {
                            report.failed += 1;
                        }
                    }
                }
            }
            report
        }
        #[cfg(not(any(test, feature = "test-support")))]
        unreachable!(
            "a production OutboxStore is Durable-only (the Memory arm is test-support-gated)"
        )
    }

    pub fn drain_to_empty(&self) -> DrainReport {
        let mut total = DrainReport::default();
        loop {
            if self.store.outbox_depth() == 0 {
                break;
            }
            let r = self.drain_once();
            let made_progress = r.published > 0 || r.deduplicated > 0;
            total.published += r.published;
            total.deduplicated += r.deduplicated;
            total.failed += r.failed;
            total.add_drain_errors(r.drain_errors);
            if !made_progress {
                break;
            }
        }
        total
    }
}

#[cfg(any(test, feature = "test-support"))]
impl OutboxStore {
    pub(crate) fn claim_unsent(&self) -> Vec<OutboxRow> {
        let Some(mut inner) = self.mem() else {
            return Vec::new();
        };
        let mut to_claim: Vec<OutboxRow> = inner
            .order
            .iter()
            .filter_map(|id| inner.rows.get(id).cloned())
            .filter(|r| r.published_at.is_none() && !inner.claimed.contains(&r.event_id))
            .collect();
        to_claim.sort_by_key(|r| (r.aggregate.0.clone(), r.seq));
        for r in &to_claim {
            inner.claimed.insert(r.event_id.clone());
        }
        to_claim
    }

    pub(crate) fn mark_published(&self, id: &EventId, at: Timestamp) {
        let Some(mut inner) = self.mem() else {
            return;
        };
        if let Some(row) = inner.rows.get_mut(id) {
            row.published_at = Some(at);
        }
        inner.claimed.remove(id);
    }

    pub(crate) fn fail_attempt(&self, id: &EventId) -> u32 {
        let Some(mut inner) = self.mem() else {
            return 0;
        };
        let attempts = if let Some(row) = inner.rows.get_mut(id) {
            row.attempts += 1;
            row.attempts
        } else {
            0
        };
        inner.claimed.remove(id);
        attempts
    }

    pub(crate) fn dead_letter(&self, id: &EventId) {
        let Some(mut inner) = self.mem() else {
            return;
        };
        if let Some(row) = inner.rows.remove(id) {
            inner.order.retain(|x| x != id);
            inner.dead_letters.push(row);
        }
        inner.claimed.remove(id);
    }

    pub(crate) fn gc_published_before(&self, cutoff: &Timestamp) -> usize {
        let Some(mut inner) = self.mem() else {
            return 0;
        };
        let reap: Vec<EventId> = inner
            .rows
            .values()
            .filter(|r| r.published_at.as_ref().is_some_and(|p| p.0 < cutoff.0))
            .map(|r| r.event_id.clone())
            .collect();
        for id in &reap {
            inner.rows.remove(id);
            inner.order.retain(|x| x != id);
        }
        reap.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumer_delivery_quarantine_migration_is_fixed_code_and_payload_free() {
        let ddl = CONSUMER_DELIVERY_QUARANTINE_MIGRATION;
        assert!(ddl.contains("PRIMARY KEY (consumer, stream, stream_sequence)"));
        for code in [
            "malformed_envelope",
            "subject_mismatch",
            "no_registered_consumer",
        ] {
            assert!(ddl.contains(code));
        }
        for forbidden in [
            "payload ",
            "raw_payload",
            "raw_subject",
            "tenant ",
            "payload_hash",
        ] {
            assert!(
                !ddl.contains(forbidden),
                "forbidden quarantine column {forbidden}"
            );
        }
    }
    use crate::outbox::{EmitContextBase, IdMinter, MonotonicMinter};
    use crate::{
        Actor, AggregateKey, CausedBy, DataRole, EventDraft, EventType, OutboxTx, Region, TenantId,
        Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn clock() -> Timestamp {
        Timestamp("2026-06-19T00:00:02Z".into())
    }

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(principal()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn draft(type_: &str, aggregate: &str) -> EventDraft {
        EventDraft {
            type_: EventType(type_.into()),
            subject: crate::ArtifactRef(format!("myelin://acme/issues/issue/{aggregate}")),
            aggregate: AggregateKey(aggregate.into()),
            payload: serde_json::json!({ "ref": aggregate }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    #[test]
    fn transport_classification_codes_are_stable() {
        assert_eq!(IntakeDependency::Blob.name(), "blob");
        assert_eq!(
            DeliveryPoisonKind::MalformedEnvelope.code(),
            "malformed_envelope"
        );
        assert_eq!(
            DeliveryPoisonKind::SubjectMismatch.code(),
            "subject_mismatch"
        );
        assert_eq!(
            DeliveryQuarantineReason::MalformedEnvelope.code(),
            "malformed_envelope"
        );
        assert_eq!(
            DeliveryQuarantineReason::SubjectMismatch.code(),
            "subject_mismatch"
        );
        assert_eq!(
            DeliveryQuarantineReason::NoRegisteredConsumer.code(),
            "no_registered_consumer"
        );
    }

    #[test]
    fn durable_drain_errors_are_reported_and_accumulated() {
        let failure = DrainReport::durable_failure();
        assert_eq!(failure.drain_errors, 1);
        assert_eq!(failure.published, 0);
        assert_eq!(failure.deduplicated, 0);
        assert_eq!(failure.failed, 0);
        assert_eq!(failure.dead_lettered, 0);

        let mut total = DrainReport {
            drain_errors: 3,
            ..DrainReport::default()
        };
        total.add_drain_errors(4);
        assert_eq!(total.drain_errors, 7);
    }

    fn commit_n(
        store: &OutboxStore,
        minter: Arc<dyn IdMinter>,
        n: usize,
        aggregate: &str,
    ) -> Vec<EventId> {
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("state");
        let mut ids = Vec::new();
        for i in 0..n {
            ids.push(
                tx.emit(draft(&format!("issues.issue.e{i}"), aggregate), None)
                    .unwrap(),
            );
        }
        tx.commit().unwrap();
        ids
    }

    #[test]
    fn relay_drains_committed_rows_exactly_once_depth_to_zero() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 3, "issue:PROJ-1");
        assert_eq!(store.outbox_depth(), 3);

        let bus = InProcessBus::new();
        let relay = Relay::new(store.clone(), bus.clone(), clock);
        let report = relay.drain_to_empty();

        assert_eq!(report.published, 3, "every committed event published once");
        assert_eq!(report.deduplicated, 0);
        assert_eq!(store.outbox_depth(), 0, "outbox-depth drains to 0 (SUB-D1)");
        assert_eq!(bus.delivered_count(), 3);
        assert_eq!(bus.delivered_ids(), ids.into_iter().collect());
    }

    #[test]
    fn sub_d1_kill_between_commit_and_publish_zero_ghost_zero_lost() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 4, "issue:PROJ-1");

        let bus = InProcessBus::new();
        let relay = Relay::new(store.clone(), bus.clone(), clock);

        bus.sever();
        relay.drain_to_empty();
        assert_eq!(
            store.outbox_depth(),
            4,
            "severed broker → events parked, not lost"
        );
        assert_eq!(bus.delivered_count(), 0, "nothing delivered while severed");

        bus.heal();
        let report = relay.drain_to_empty();
        assert_eq!(report.published, 4);
        assert_eq!(
            store.outbox_depth(),
            0,
            "outbox-depth drains after heal (SUB-D1)"
        );
        assert_eq!(
            bus.delivered_count(),
            4,
            "exactly-once: 4 committed → 4 delivered"
        );
        assert_eq!(
            bus.delivered_ids(),
            ids.into_iter().collect(),
            "0 ghost / 0 lost"
        );
    }

    #[test]
    fn relay_reclaim_after_crash_is_deduplicated_zero_ghost() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 1, "issue:PROJ-1");
        let id = ids[0].clone();

        let bus = InProcessBus::new();
        let row = store.row(&id).unwrap();
        assert_eq!(
            bus.put(&row.subject, &row.envelope, &row.event_id).unwrap(),
            Delivery::Accepted
        );
        assert_eq!(
            store.outbox_depth(),
            1,
            "row still unsent (crash before mark)"
        );

        let relay = Relay::new(store.clone(), bus.clone(), clock);
        let report = relay.drain_once();
        assert_eq!(
            report.deduplicated, 1,
            "the re-claim was deduplicated (0 ghost)"
        );
        assert_eq!(report.published, 0);
        assert_eq!(
            bus.delivered_count(),
            1,
            "still exactly one delivery (no ghost)"
        );
        assert_eq!(
            store.outbox_depth(),
            0,
            "the row is marked sent after the dedup"
        );
    }

    #[test]
    fn skip_locked_two_workers_never_double_claim() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 5, "issue:PROJ-1");

        let a = store.claim_unsent();
        assert_eq!(a.len(), 5);
        let b = store.claim_unsent();
        assert!(b.is_empty(), "a second worker skips already-claimed rows");

        for r in &a {
            store.fail_attempt(&r.event_id);
        }
        let b2 = store.claim_unsent();
        assert_eq!(b2.len(), 5, "released claims are re-claimable");
    }

    #[test]
    fn bounded_retries_then_dead_letter() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 1, "issue:PROJ-1");

        let bus = InProcessBus::new();
        bus.sever();
        let relay = Relay::new(store.clone(), bus.clone(), clock);

        for _ in 0..MAX_PUBLISH_ATTEMPTS {
            relay.drain_once();
        }
        assert_eq!(store.outbox_depth(), 0, "the row left the unsent set");
        assert_eq!(
            store.dead_letter_count(),
            1,
            "it is dead-lettered, not silently lost"
        );
        assert_eq!(bus.delivered_count(), 0);
    }

    #[test]
    fn dead_letter_removes_only_the_dead_row_survivors_still_deliver() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 3, "issue:PROJ-1");
        assert_eq!(store.outbox_depth(), 3);

        store.dead_letter(&ids[1]);
        assert_eq!(store.dead_letter_count(), 1);
        assert_eq!(
            store.outbox_depth(),
            2,
            "only the dead row left the live set"
        );
        assert!(
            store.row(&ids[1]).is_none(),
            "the dead row is gone from live rows"
        );
        assert!(store.row(&ids[0]).is_some(), "survivor 0 still live");
        assert!(store.row(&ids[2]).is_some(), "survivor 2 still live");

        let bus = InProcessBus::new();
        Relay::new(store.clone(), bus.clone(), clock).drain_to_empty();
        assert_eq!(bus.delivered_count(), 2);
        let delivered = bus.delivered_ids();
        assert!(delivered.contains(&ids[0]) && delivered.contains(&ids[2]));
        assert!(
            !delivered.contains(&ids[1]),
            "the dead-lettered row is not delivered"
        );
    }

    #[test]
    fn drain_once_report_counts_this_pass_exactly() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 3, "issue:PROJ-1");
        let relay = Relay::new(store.clone(), InProcessBus::new(), clock);

        let r = relay.drain_once();
        assert_eq!(r.published, 3, "this pass published exactly 3");
        assert_eq!(r.deduplicated, 0);
        assert_eq!(r.failed, 0);
        assert_eq!(r.dead_lettered, 0);
        let r2 = relay.drain_once();
        assert_eq!(
            r2,
            DrainReport::default(),
            "an empty drain pass reports all-zero"
        );
    }

    #[test]
    fn drain_once_reports_failed_while_severed_under_bound() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 2, "issue:PROJ-1");
        let bus = InProcessBus::new();
        bus.sever();
        let relay = Relay::new(store.clone(), bus, clock);

        let r = relay.drain_once();
        assert_eq!(r.failed, 2, "both rows failed this pass (broker severed)");
        assert_eq!(r.published, 0);
        assert_eq!(r.dead_lettered, 0);
        assert_eq!(store.outbox_depth(), 2);
    }

    #[test]
    fn repeated_drain_failed_then_dead_letters_and_surfaces() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 1, "issue:PROJ-1");
        let bus = InProcessBus::new();
        bus.sever();
        let relay = Relay::new(store.clone(), bus, clock);

        let mut failed_total = 0usize;
        let mut dead_total = 0usize;
        for pass in 1..=MAX_PUBLISH_ATTEMPTS {
            let r = relay.drain_once();
            failed_total += r.failed;
            dead_total += r.dead_lettered;
            if pass < MAX_PUBLISH_ATTEMPTS {
                assert_eq!(r.failed, 1, "under the bound, each pass reports one failed");
                assert_eq!(r.dead_lettered, 0);
            } else {
                assert_eq!(r.dead_lettered, 1, "the bound-th pass dead-letters");
                assert_eq!(r.failed, 0);
            }
        }
        assert_eq!(failed_total, (MAX_PUBLISH_ATTEMPTS - 1) as usize);
        assert_eq!(dead_total, 1);
        assert_eq!(store.dead_letter_count(), 1);
        let dl = store.dead_letters();
        assert_eq!(dl.len(), 1);
        assert_eq!(
            dl[0].attempts, MAX_PUBLISH_ATTEMPTS,
            "attempts hit the bound"
        );
    }

    #[test]
    fn drain_to_empty_loops_and_accumulates_published_across_passes() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 5, "issue:PROJ-1");
        let bus = InProcessBus::new();
        bus.fail_next(2);
        let relay = Relay::new(store.clone(), bus.clone(), clock);

        let total = relay.drain_to_empty();
        assert_eq!(
            total.published, 5,
            "published accumulates across passes (3 + 2)"
        );
        assert_eq!(total.failed, 2, "the 2 transient failures are reported");
        assert_eq!(
            store.outbox_depth(),
            0,
            "the outbox fully drained over 2 passes"
        );
        assert_eq!(
            bus.delivered_count(),
            5,
            "exactly 5 delivered, none ghosted/lost"
        );
    }

    #[test]
    fn drain_to_empty_counts_deduplicated_as_progress() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 3, "issue:PROJ-1");
        let bus = InProcessBus::new();
        for id in &ids {
            let row = store.row(id).unwrap();
            bus.put(&row.subject, &row.envelope, &row.event_id).unwrap();
        }
        assert_eq!(
            store.outbox_depth(),
            3,
            "rows delivered but not yet marked sent"
        );

        let relay = Relay::new(store.clone(), bus.clone(), clock);
        let total = relay.drain_to_empty();
        assert_eq!(
            total.deduplicated, 3,
            "all 3 re-claims were deduplicated (0 ghost)"
        );
        assert_eq!(
            total.published, 0,
            "nothing newly published (already delivered)"
        );
        assert_eq!(store.outbox_depth(), 0, "depth drains via the dedup path");
        assert_eq!(
            bus.delivered_count(),
            3,
            "still exactly 3 delivered (no double-delivery)"
        );
    }

    #[test]
    fn drain_to_empty_stops_on_no_progress_when_severed() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 3, "issue:PROJ-1");
        let bus = InProcessBus::new();
        bus.sever();
        let relay = Relay::new(store.clone(), bus, clock);

        let total = relay.drain_to_empty();
        assert_eq!(total.published, 0);
        assert_eq!(total.failed, 3, "one failed per claimed row, then it stops");
        assert_eq!(
            store.outbox_depth(),
            3,
            "0 lost: the rows stay parked while severed"
        );
    }

    #[test]
    fn drain_to_empty_loops_on_dedup_progress_across_passes() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 5, "issue:PROJ-1");
        let bus = InProcessBus::new();
        for id in &ids {
            let row = store.row(id).unwrap();
            bus.put(&row.subject, &row.envelope, &row.event_id).unwrap();
        }
        assert_eq!(store.outbox_depth(), 5, "delivered but not yet marked sent");
        bus.fail_next(2);

        let relay = Relay::new(store.clone(), bus.clone(), clock);
        let total = relay.drain_to_empty();
        assert_eq!(
            total.deduplicated, 5,
            "all 5 dedups accumulate ACROSS the two passes (the dedup progress term keeps looping)"
        );
        assert_eq!(
            total.published, 0,
            "nothing newly published (already delivered)"
        );
        assert_eq!(total.failed, 2, "the 2 transient failures are reported");
        assert_eq!(
            store.outbox_depth(),
            0,
            "the outbox fully drained over 2 passes via the dedup path (0 lost)"
        );
        assert_eq!(
            bus.delivered_count(),
            5,
            "still exactly 5 delivered (no ghost)"
        );
    }

    #[test]
    fn transport_ack_and_purge_behave() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 2, "issue:PROJ-1");
        let bus = InProcessBus::new();
        Relay::new(store.clone(), bus.clone(), clock).drain_to_empty();
        assert_eq!(bus.delivered_count(), 2);

        bus.ack("indexer", &ids[1]);
        assert_eq!(
            bus.ack_of("indexer").as_ref(),
            Some(&ids[1]),
            "ack recorded the high-water"
        );
        assert_eq!(
            bus.ack_of("nobody"),
            None,
            "an un-acked consumer reads None"
        );
        bus.purge();
        assert_eq!(bus.delivered_count(), 0, "purge cleared the delivered log");
        let row = store.row(&ids[0]).unwrap();
        assert_eq!(
            bus.put(&row.subject, &row.envelope, &row.event_id).unwrap(),
            Delivery::Accepted,
            "after purge the dedup state is gone - the id is accepted fresh"
        );
    }

    #[test]
    fn cdc_2_3_consumer_reads_the_relayed_envelope() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 2, "issue:PROJ-1");

        let bus = InProcessBus::new();
        let relay = Relay::new(store.clone(), bus.clone(), clock);
        relay.drain_to_empty();

        let consumed = bus.consume("myelin://acme/issues/issue/");
        assert_eq!(consumed.len(), 2);
        assert_eq!(consumed[0].event_id, ids[0]);
        assert_eq!(consumed[1].event_id, ids[1]);
        let provider_row = store.row(&ids[0]).unwrap();
        assert_eq!(
            consumed[0], provider_row.envelope,
            "consumer sees the provider's wire shape"
        );

        bus.ack("indexer", &ids[1]);
    }

    #[test]
    fn eb04_dlq_subject_is_tenant_and_subsystem() {
        assert_eq!(
            dlq_subject(&TenantId("acme".into()), "issues"),
            "dlq.acme.issues"
        );
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 1, "issue:PROJ-1");
        let row = store.row(&ids[0]).unwrap();
        assert_eq!(
            subsystem_of(&row.envelope),
            "issues",
            "type 'issues.issue.eN' → 'issues'"
        );
    }

    #[test]
    fn eb04_dead_letter_raises_dlq_alert_surfaced_not_silent() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 1, "issue:PROJ-1");

        let bus = InProcessBus::new();
        bus.sever();
        let relay = Relay::new(store.clone(), bus, clock);

        for _ in 0..(MAX_PUBLISH_ATTEMPTS - 1) {
            relay.drain_once();
        }
        assert!(
            relay.dead_letter_alerts().is_empty(),
            "no alert before the retry bound"
        );
        relay.drain_once();

        assert_eq!(
            store.dead_letter_count(),
            1,
            "the row is dead-lettered (count signal)"
        );
        let alerts = relay.dead_letter_alerts();
        assert_eq!(
            alerts.len(),
            1,
            "exactly one dead-letter Signal alert raised"
        );
        assert_eq!(
            alerts[0].dlq_subject, "dlq.acme.issues",
            "routed to dlq.<tenant>.<subsystem>"
        );
        assert_eq!(
            alerts[0].event_id, ids[0],
            "the alert carries the offending event_id"
        );
        assert_eq!(alerts[0].tenant, TenantId("acme".into()));
        assert_eq!(alerts[0].subsystem, "issues");
        assert_eq!(
            alerts[0].attempts, MAX_PUBLISH_ATTEMPTS,
            "the bound was exhausted"
        );
    }

    #[test]
    fn eb04_gc_reaps_only_old_published_rows_never_unsent() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 3, "issue:PROJ-1");

        store.mark_published(&ids[0], Timestamp("2026-06-17T00:00:00Z".into()));
        store.mark_published(&ids[1], Timestamp("2026-06-19T11:59:00Z".into()));
        assert_eq!(store.outbox_depth(), 1, "row 2 is still unsent");

        let relay = Relay::new(store.clone(), InProcessBus::new(), clock);
        let reaped = relay.gc_published(&Timestamp("2026-06-18T12:00:00Z".into()));

        assert_eq!(reaped, 1, "only the >24h-old published row is GC'd");
        assert!(
            store.row(&ids[0]).is_none(),
            "the old published row was reaped"
        );
        assert!(
            store.row(&ids[1]).is_some(),
            "the recent published row is retained"
        );
        assert!(
            store.row(&ids[2]).is_some(),
            "the UNSENT row is NEVER reaped (0 lost)"
        );
        assert_eq!(store.outbox_depth(), 1, "GC did not touch the unsent set");
    }

    #[test]
    fn eb04_gc_never_reaps_a_severed_outbox() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 4, "issue:PROJ-1");
        let relay = Relay::new(store.clone(), InProcessBus::new(), clock);
        let reaped = relay.gc_published(&Timestamp("2099-01-01T00:00:00Z".into()));
        assert_eq!(reaped, 0, "an unsent outbox loses nothing to GC");
        assert_eq!(
            store.outbox_depth(),
            4,
            "all four rows still parked + deliverable"
        );
    }

    #[test]
    fn eb04_gc_is_strict_before_cutoff_a_row_at_the_cutoff_is_retained() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 2, "issue:PROJ-1");
        let cutoff = Timestamp("2026-06-18T12:00:00Z".into());

        store.mark_published(&ids[0], Timestamp("2026-06-18T11:59:59Z".into()));
        store.mark_published(&ids[1], cutoff.clone());

        let relay = Relay::new(store.clone(), InProcessBus::new(), clock);
        let reaped = relay.gc_published(&cutoff);
        assert_eq!(
            reaped, 1,
            "only the row strictly BEFORE the cutoff is reaped"
        );
        assert!(
            store.row(&ids[0]).is_none(),
            "the row before the cutoff was reaped"
        );
        assert!(
            store.row(&ids[1]).is_some(),
            "the row AT the cutoff is retained (strict `<`, not `<=`)"
        );
    }

    #[test]
    fn boxed_bustransport_forwards_ack_and_purge_to_the_inner_bus() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 2, "issue:PROJ-1");
        let inner = InProcessBus::new();
        let boxed: Box<dyn BusTransport> = Box::new(inner.clone());
        Relay::new(store.clone(), boxed, clock).drain_to_empty();
        assert_eq!(
            inner.delivered_count(),
            2,
            "drained through the boxed transport"
        );

        let boxed_consumer: Box<dyn BusTransport> = Box::new(inner.clone());
        let consumed = boxed_consumer.consume("myelin://acme/issues/issue/");
        assert_eq!(
            consumed.len(),
            2,
            "the boxed `consume` forward returns the inner bus's 2 envelopes (not vec![])"
        );

        let boxed2: Box<dyn BusTransport> = Box::new(inner.clone());
        boxed2.ack("indexer", &ids[1]);
        assert_eq!(
            inner.ack_of("indexer").as_ref(),
            Some(&ids[1]),
            "the boxed `ack` forward landed on the inner bus"
        );

        boxed2.purge();
        assert_eq!(
            inner.delivered_count(),
            0,
            "the boxed `purge` forward cleared the inner bus"
        );
    }

    #[test]
    fn cdc_bustransport_put_consume_conformance() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 2, "issue:PROJ-1");
        let r0 = store.row(&ids[0]).unwrap();
        let r1 = store.row(&ids[1]).unwrap();

        let bus = InProcessBus::new();
        assert_eq!(
            bus.put(&r0.subject, &r0.envelope, &r0.event_id).unwrap(),
            Delivery::Accepted
        );
        assert_eq!(
            bus.put(&r1.subject, &r1.envelope, &r1.event_id).unwrap(),
            Delivery::Accepted
        );
        assert_eq!(
            bus.put(&r0.subject, &r0.envelope, &r0.event_id).unwrap(),
            Delivery::Deduplicated,
            "the same dedup_id is suppressed (Nats-Msg-Id broker dedup → 0 ghost)"
        );

        let consumed = bus.consume("myelin://acme/issues/issue/");
        assert_eq!(
            consumed.len(),
            2,
            "exactly two distinct events delivered (dedup suppressed the 3rd put)"
        );
        assert_eq!(
            consumed[0], r0.envelope,
            "consumer sees the provider's wire envelope #0"
        );
        assert_eq!(
            consumed[1], r1.envelope,
            "consumer sees the provider's wire envelope #1, in order"
        );
    }

    #[test]
    fn eb04_sub_d1_bus_d4_reconfirm_zero_ghost_zero_lost_after_refinements() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 5, "issue:PROJ-1");
        let committed: HashSet<EventId> = ids.iter().cloned().collect();

        let bus = InProcessBus::new();
        let relay = Relay::new(store.clone(), bus.clone(), clock);

        bus.sever();
        relay.drain_to_empty();
        assert_eq!(
            store.outbox_depth(),
            5,
            "0 lost while severed (events parked)"
        );
        assert_eq!(bus.delivered_count(), 0, "0 ghost while severed");
        assert!(
            relay.dead_letter_alerts().is_empty(),
            "no premature dead-letter"
        );

        bus.heal();
        relay.drain_to_empty();
        assert_eq!(store.outbox_depth(), 0, "outbox-depth drains (SUB-D1)");
        assert_eq!(
            store.dead_letter_count(),
            0,
            "0 dead-letters on the no-loss path"
        );
        assert_eq!(
            bus.delivered_count(),
            5,
            "exactly-once: 5 committed → 5 delivered"
        );
        assert_eq!(
            bus.delivered_ids(),
            committed,
            "delivered set == committed set (0 ghost/0 lost)"
        );

        let reaped = relay.gc_published(&Timestamp("2099-01-01T00:00:00Z".into()));
        assert_eq!(reaped, 5, "all 5 published rows aged out of the 24h window");
        assert_eq!(store.outbox_depth(), 0, "still 0 unsent after GC");
        assert_eq!(
            bus.delivered_count(),
            5,
            "GC did not lose or duplicate any delivery"
        );
        assert_eq!(
            bus.delivered_ids(),
            committed,
            "delivered set STILL == committed set after GC"
        );
        println!("[2026-06-19] PASS  gate=EB-04  SUB-D1/BUS-D4 reconfirm  ghost=0 lost=0  (after DLQ-alert + 24h-GC refinements)");
    }
}
