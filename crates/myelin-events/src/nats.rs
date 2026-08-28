use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use async_nats::jetstream::consumer::PullConsumer;
use async_nats::jetstream::stream::{
    Config as StreamConfig, DiscardPolicy, RetentionPolicy, StorageType,
};
use async_nats::jetstream::{self, AckKind, Context};
use futures::StreamExt;

use crate::relay::{
    BrokerDelivery, BrokerDeliveryBody, BrokerDeliveryRef, BusTransport, Delivery,
    DeliveryPoisonKind, DeliveryToken, EventPublisher, TransportError,
};
use crate::{ArtifactRef, EventEnvelope, EventId};

const SERVER_SHUTDOWN_PULL_ERROR: &str =
    "error while processing messages from the stream: 409, Some(\"Server Shutdown\")";

fn is_server_shutdown(error: &(dyn std::error::Error + Send + Sync)) -> bool {
    // async-nats 0.47 erases unexpected pull status messages into an I/O error.
    // Match its complete rendering so unrelated 409 responses remain observable.
    error.to_string() == SERVER_SHUTDOWN_PULL_ERROR
}

#[derive(Clone, PartialEq, Eq)]
pub struct JetStreamPublisherConfig {
    pub nats_url: String,
    pub stream_name: String,
    pub subject_root: String,
    pub max_age: std::time::Duration,
    pub max_bytes: i64,
    pub max_messages: i64,
    pub replicas: usize,
    pub duplicate_window: std::time::Duration,
    pub publish_ack_timeout: std::time::Duration,
}

impl core::fmt::Debug for JetStreamPublisherConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("JetStreamPublisherConfig")
            .field("nats_url", &"<redacted>")
            .field("stream_name", &self.stream_name)
            .field("subject_root", &self.subject_root)
            .field("max_age", &self.max_age)
            .field("max_bytes", &self.max_bytes)
            .field("max_messages", &self.max_messages)
            .field("replicas", &self.replicas)
            .field("duplicate_window", &self.duplicate_window)
            .field("publish_ack_timeout", &self.publish_ack_timeout)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct JetStreamConsumerConfig {
    pub nats_url: String,
    pub stream_name: String,
    pub subject_root: String,
    pub filter_subject: String,
    pub consumer_name: String,
    pub ack_wait: std::time::Duration,
    pub max_deliver: i64,
    pub max_ack_pending: i64,
    pub max_batch: usize,
    pub max_expires: std::time::Duration,
}

impl core::fmt::Debug for JetStreamConsumerConfig {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("JetStreamConsumerConfig")
            .field("nats_url", &"<redacted>")
            .field("stream_name", &self.stream_name)
            .field("subject_root", &self.subject_root)
            .field("filter_subject", &self.filter_subject)
            .field("consumer_name", &self.consumer_name)
            .field("ack_wait", &self.ack_wait)
            .field("max_deliver", &self.max_deliver)
            .field("max_ack_pending", &self.max_ack_pending)
            .field("max_batch", &self.max_batch)
            .field("max_expires", &self.max_expires)
            .finish()
    }
}

impl JetStreamConsumerConfig {
    pub fn bounded(
        nats_url: impl Into<String>,
        stream_name: impl Into<String>,
        subject_root: impl Into<String>,
        filter_subject: impl Into<String>,
        consumer_name: impl Into<String>,
    ) -> Self {
        Self {
            nats_url: nats_url.into(),
            stream_name: stream_name.into(),
            subject_root: subject_root.into(),
            filter_subject: filter_subject.into(),
            consumer_name: consumer_name.into(),
            ack_wait: std::time::Duration::from_secs(30),
            max_deliver: -1,
            max_ack_pending: 256,
            max_batch: 256,
            max_expires: std::time::Duration::from_secs(1),
        }
    }

    pub fn with_admission(mut self, admission: crate::DurableWorkerAdmission) -> Self {
        self.max_ack_pending = i64::from(admission.max_ack_pending().get());
        self.max_batch = admission.max_batch() as usize;
        self
    }

    fn validate(&self) -> Result<(), TransportError> {
        if self.nats_url.trim().is_empty()
            || self.stream_name.trim().is_empty()
            || self.subject_root.trim().is_empty()
            || self.consumer_name.trim().is_empty()
        {
            return Err(TransportError(
                "JetStream consumer endpoint, stream, root, and durable name are required".into(),
            ));
        }
        if self.filter_subject != self.subject_root
            && !self
                .filter_subject
                .starts_with(&format!("{}.", self.subject_root))
        {
            return Err(TransportError(format!(
                "consumer filter {} escapes subject root {}",
                self.filter_subject, self.subject_root
            )));
        }
        if self.ack_wait.is_zero()
            || (self.max_deliver != -1 && self.max_deliver <= 0)
            || self.max_ack_pending <= 0
            || self.max_batch == 0
            || self.max_batch > self.max_ack_pending as usize
            || self.max_expires.is_zero()
        {
            return Err(TransportError(
                "JetStream consumer bounds must be finite and positive (batch <= ack pending)"
                    .into(),
            ));
        }
        Ok(())
    }

    fn pull_config(&self) -> jetstream::consumer::pull::Config {
        jetstream::consumer::pull::Config {
            durable_name: Some(self.consumer_name.clone()),
            name: Some(self.consumer_name.clone()),
            deliver_policy: jetstream::consumer::DeliverPolicy::All,
            ack_policy: jetstream::consumer::AckPolicy::Explicit,
            ack_wait: self.ack_wait,
            max_deliver: self.max_deliver,
            filter_subject: self.filter_subject.clone(),
            replay_policy: jetstream::consumer::ReplayPolicy::Instant,
            max_waiting: 8,
            max_ack_pending: self.max_ack_pending,
            max_batch: self.max_batch as i64,
            max_bytes: 4 * 1024 * 1024,
            max_expires: self.max_expires,
            ..Default::default()
        }
    }
}

impl JetStreamPublisherConfig {
    pub fn validate(&self) -> Result<(), TransportError> {
        if self.nats_url.trim().is_empty() {
            return Err(TransportError("NATS URL must not be empty".into()));
        }
        if self.stream_name.is_empty()
            || self
                .stream_name
                .chars()
                .any(|c| c.is_whitespace() || c == '.')
        {
            return Err(TransportError(
                "JetStream stream name must be non-empty and contain no whitespace or dots".into(),
            ));
        }
        if !valid_subject_root(&self.subject_root) {
            return Err(TransportError(
                "JetStream subject root must contain non-empty literal dot-separated tokens".into(),
            ));
        }
        if self.max_age.is_zero() {
            return Err(TransportError("JetStream max_age must be positive".into()));
        }
        if self.max_bytes <= 0 {
            return Err(TransportError(
                "JetStream max_bytes must be a finite positive bound".into(),
            ));
        }
        if self.max_messages <= 0 {
            return Err(TransportError(
                "JetStream max_messages must be a finite positive bound".into(),
            ));
        }
        if !(1..=5).contains(&self.replicas) {
            return Err(TransportError(
                "JetStream replicas must be between 1 and 5".into(),
            ));
        }
        if self.duplicate_window.is_zero() || self.duplicate_window > self.max_age {
            return Err(TransportError(
                "JetStream duplicate_window must be positive and no greater than max_age".into(),
            ));
        }
        if self.publish_ack_timeout.is_zero()
            || self.publish_ack_timeout > std::time::Duration::from_secs(60)
        {
            return Err(TransportError(
                "JetStream publish acknowledgement timeout must be positive and at most 60 seconds"
                    .into(),
            ));
        }
        Ok(())
    }

    fn stream_config(&self) -> StreamConfig {
        StreamConfig {
            name: self.stream_name.clone(),
            subjects: vec![format!("{}.>", self.subject_root)],
            retention: RetentionPolicy::Limits,
            storage: StorageType::File,
            max_age: self.max_age,
            max_bytes: self.max_bytes,
            max_messages: self.max_messages,
            num_replicas: self.replicas,
            duplicate_window: self.duplicate_window,
            discard: DiscardPolicy::Old,
            ..Default::default()
        }
    }
}

fn valid_subject_root(root: &str) -> bool {
    !root.is_empty()
        && root.split('.').all(|token| {
            !token.is_empty()
                && token
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

fn validate_stream_config(
    actual: &StreamConfig,
    expected: &StreamConfig,
) -> Result<(), TransportError> {
    let mut drifted = false;
    if actual.storage != StorageType::File {
        drifted = true;
    }
    if actual.retention != RetentionPolicy::Limits {
        drifted = true;
    }
    if actual.discard != DiscardPolicy::Old {
        drifted = true;
    }
    if actual.max_age != expected.max_age {
        drifted = true;
    }
    if actual.max_bytes != expected.max_bytes {
        drifted = true;
    }
    if actual.max_messages != expected.max_messages {
        drifted = true;
    }
    if actual.num_replicas != expected.num_replicas {
        drifted = true;
    }
    if actual.subjects != expected.subjects {
        drifted = true;
    }
    if actual.duplicate_window != expected.duplicate_window {
        drifted = true;
    }

    if drifted {
        Err(TransportError(
            "existing JetStream stream configuration is incompatible".into(),
        ))
    } else {
        Ok(())
    }
}

fn consumer_config_drift(
    actual: &jetstream::consumer::Config,
    expected: &jetstream::consumer::pull::Config,
) -> Vec<&'static str> {
    let mut drift = Vec::new();
    macro_rules! pin {
        ($field:ident) => {
            if actual.$field != expected.$field {
                drift.push(stringify!($field));
            }
        };
    }

    pin!(durable_name);
    pin!(name);
    pin!(description);
    pin!(deliver_policy);
    pin!(ack_policy);
    pin!(ack_wait);
    pin!(max_deliver);
    pin!(filter_subject);
    pin!(filter_subjects);
    pin!(replay_policy);
    pin!(rate_limit);
    pin!(sample_frequency);
    pin!(max_waiting);
    pin!(max_ack_pending);
    pin!(headers_only);
    pin!(max_batch);
    pin!(max_bytes);
    pin!(max_expires);
    pin!(inactive_threshold);
    pin!(num_replicas);
    pin!(memory_storage);
    pin!(metadata);
    pin!(backoff);
    if actual.deliver_subject.is_some() {
        drift.push("deliver_subject");
    }
    if actual.deliver_group.is_some() {
        drift.push("deliver_group");
    }
    if actual.flow_control {
        drift.push("flow_control");
    }
    if !actual.idle_heartbeat.is_zero() {
        drift.push("idle_heartbeat");
    }

    drift
}

fn admission_limits_are_the_only_consumer_drift(drift: &[&str]) -> bool {
    !drift.is_empty()
        && drift
            .iter()
            .all(|field| matches!(*field, "max_ack_pending" | "max_batch"))
}

fn validate_consumer_config(
    actual: &jetstream::consumer::Config,
    expected: &jetstream::consumer::pull::Config,
) -> Result<(), TransportError> {
    let drift = consumer_config_drift(actual, expected);
    if drift.is_empty() {
        Ok(())
    } else {
        Err(TransportError(format!(
            "durable consumer configuration drifted from bounded policy: {}",
            drift.join(", ")
        )))
    }
}

fn event_subject(subject_root: &str, envelope: &EventEnvelope) -> Result<String, TransportError> {
    let structured = crate::partition::StreamSubject::of(envelope)
        .map_err(|error| TransportError(format!("invalid event routing subject: {error}")))?;
    let subject = format!("{subject_root}.{}", structured.to_subject());
    if subject.len() > crate::partition::MAX_STREAM_SUBJECT_BYTES {
        return Err(TransportError(
            "event routing subject exceeds byte limit".into(),
        ));
    }
    Ok(subject)
}

fn classify_delivery_body(
    subject_root: &str,
    actual_subject: &str,
    payload: &[u8],
) -> BrokerDeliveryBody {
    match serde_json::from_slice::<EventEnvelope>(payload) {
        Err(_) => BrokerDeliveryBody::Poison(DeliveryPoisonKind::MalformedEnvelope),
        Ok(envelope) => match event_subject(subject_root, &envelope) {
            Ok(expected) if actual_subject == expected => {
                BrokerDeliveryBody::Event(Box::new(envelope))
            }
            _ => BrokerDeliveryBody::Poison(DeliveryPoisonKind::SubjectMismatch),
        },
    }
}

fn allocate_delivery_token(sequence: &AtomicU64) -> Result<DeliveryToken, TransportError> {
    let previous = sequence
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map_err(|_| TransportError("delivery token space exhausted".into()))?;
    DeliveryToken::new(previous + 1)
        .ok_or_else(|| TransportError("delivery token allocation failed".into()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettlementIntent {
    Ack,
    Retry(u64),
    Terminate,
}

struct QueuedSettlement<T> {
    handle: T,
    intent: SettlementIntent,
}

fn queue_on_settlement_failure<T>(
    retries: &Mutex<HashMap<DeliveryToken, QueuedSettlement<T>>>,
    token: DeliveryToken,
    retained: T,
    intent: SettlementIntent,
    result: &Result<(), TransportError>,
) {
    if result.is_err() {
        retries.lock().unwrap_or_else(|e| e.into_inner()).insert(
            token,
            QueuedSettlement {
                handle: retained,
                intent,
            },
        );
    }
}

fn drain_queued_settlements<T: Clone>(
    retries: &Mutex<HashMap<DeliveryToken, QueuedSettlement<T>>>,
    mut settle: impl FnMut(T, SettlementIntent) -> Result<(), TransportError>,
) -> Result<(), TransportError> {
    loop {
        let next = {
            let retries = retries.lock().unwrap_or_else(|e| e.into_inner());
            retries
                .iter()
                .next()
                .map(|(token, queued)| (*token, queued.handle.clone(), queued.intent))
        };
        let Some((token, handle, intent)) = next else {
            return Ok(());
        };
        if settle(handle, intent).is_err() {
            return Err(TransportError(
                "broker settlement retry remains unresolved".into(),
            ));
        }
        retries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&token);
    }
}

pub struct JetStreamProvisioner;

impl JetStreamProvisioner {
    pub fn ensure(
        config: JetStreamPublisherConfig,
        rt: tokio::runtime::Handle,
    ) -> Result<(), TransportError> {
        config.validate()?;
        let expected = config.stream_config();
        tokio::task::block_in_place(|| {
            rt.block_on(async {
                let client = async_nats::connect(&config.nats_url)
                    .await
                    .map_err(|_| TransportError("NATS provisioning connection failed".into()))?;
                let js = jetstream::new(client);
                let stream = js
                    .get_or_create_stream(expected.clone())
                    .await
                    .map_err(|_| TransportError("JetStream provisioning request failed".into()))?;
                validate_stream_config(&stream.cached_info().config, &expected)
            })
        })
    }
}

async fn publish_with_timeout<F, T>(
    timeout: std::time::Duration,
    future: F,
) -> Result<T, TransportError>
where
    F: std::future::Future<Output = Result<T, TransportError>>,
{
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| TransportError("JetStream publish acknowledgement timed out".into()))?
}

pub struct NatsJetStreamPublisher {
    js: Context,
    subject_root: String,
    publish_ack_timeout: std::time::Duration,
    rt: tokio::runtime::Handle,
}

impl NatsJetStreamPublisher {
    pub fn connect(
        config: JetStreamPublisherConfig,
        rt: tokio::runtime::Handle,
    ) -> Result<Self, TransportError> {
        JetStreamProvisioner::ensure(config.clone(), rt.clone())?;
        Self::connect_existing(config, rt)
    }

    pub fn connect_existing(
        config: JetStreamPublisherConfig,
        rt: tokio::runtime::Handle,
    ) -> Result<Self, TransportError> {
        config.validate()?;
        let subject_root = config.subject_root.clone();
        let publish_ack_timeout = config.publish_ack_timeout;

        let js = tokio::task::block_in_place(|| {
            rt.block_on(async {
                let client = async_nats::connect(&config.nats_url)
                    .await
                    .map_err(|_| TransportError("NATS publisher connection failed".into()))?;
                Ok::<Context, TransportError>(jetstream::new(client))
            })
        })?;

        Ok(Self {
            js,
            subject_root,
            publish_ack_timeout,
            rt,
        })
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl EventPublisher for NatsJetStreamPublisher {
    fn publish(
        &self,
        _subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> Result<Delivery, TransportError> {
        let nats_subject = event_subject(&self.subject_root, envelope)?;
        let body = serde_json::to_vec(envelope)
            .map_err(|_| TransportError("event envelope serialization failed".into()))?;
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Nats-Msg-Id", dedup_id.0.as_str());
        let ack = self.block(publish_with_timeout(self.publish_ack_timeout, async {
            self.js
                .publish_with_headers(nats_subject, headers, body.into())
                .await
                .map_err(|_| TransportError("JetStream publish request failed".into()))?
                .await
                .map_err(|_| TransportError("JetStream publish acknowledgement failed".into()))
        }))?;
        Ok(if ack.duplicate {
            Delivery::Deduplicated
        } else {
            Delivery::Accepted
        })
    }
}

#[cfg(test)]
mod routing_tests {
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    use super::*;
    use crate::{
        Actor, AggregateKey, CorrelationId, DataRole, EventType, Region, TenantId, Timestamp,
        Visibility,
    };

    fn envelope() -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01JROUTINGTEST".into()),
            type_: EventType("issue.issue.created".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("no-osl".into()),
            actor: Actor(Principal::stub(
                PrincipalId("relay-test".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            subject: ArtifactRef("myelin://acme/issue/issue/ONE".into()),
            aggregate: AggregateKey("issue:ONE".into()),
            causation_id: None,
            correlation_id: CorrelationId("01JROUTINGTEST".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-07-18T00:00:00Z".into()),
            recorded_at: Timestamp("2026-07-18T00:00:00Z".into()),
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn invalid_subject_is_rejected_before_any_transport_operation() {
        let mut invalid = envelope();
        invalid.aggregate = AggregateKey("issue:*".into());
        assert_eq!(
            event_subject("myelin.no-osl", &invalid),
            Err(TransportError(
                "invalid event routing subject: aggregate_id token `*` is empty or contains a \
                 subject delimiter, wildcard, or whitespace"
                    .into(),
            ))
        );
    }

    #[test]
    fn subject_root_is_included_in_the_total_wire_bound() {
        let error = event_subject(&"r".repeat(1000), &envelope()).expect_err("oversized wire");
        assert_eq!(
            error,
            TransportError("event routing subject exceeds byte limit".into())
        );
    }

    #[test]
    fn mixed_malformed_valid_route_mismatch_valid_items_classify_independently() {
        let first = envelope();
        let mut second = envelope();
        second.event_id = EventId("01JROUTINGTEST2".into());
        let valid_subject = event_subject("root", &first).unwrap();
        let bodies = [
            classify_delivery_body("root", &valid_subject, b"ATTACKER_SENTINEL not json"),
            classify_delivery_body("root", &valid_subject, &serde_json::to_vec(&first).unwrap()),
            classify_delivery_body(
                "root",
                "root.evt.other.route",
                &serde_json::to_vec(&first).unwrap(),
            ),
            classify_delivery_body(
                "root",
                &valid_subject,
                &serde_json::to_vec(&second).unwrap(),
            ),
        ];
        assert!(matches!(
            bodies[0],
            BrokerDeliveryBody::Poison(DeliveryPoisonKind::MalformedEnvelope)
        ));
        assert!(matches!(bodies[1], BrokerDeliveryBody::Event(_)));
        assert!(matches!(
            bodies[2],
            BrokerDeliveryBody::Poison(DeliveryPoisonKind::SubjectMismatch)
        ));
        assert!(matches!(bodies[3], BrokerDeliveryBody::Event(_)));
    }

    #[test]
    fn duplicate_event_ids_still_receive_distinct_opaque_settlement_tokens() {
        let sequence = AtomicU64::new(0);
        let first = allocate_delivery_token(&sequence).unwrap();
        let second = allocate_delivery_token(&sequence).unwrap();
        assert_ne!(first, second);
        let pending = HashMap::from([(first, "handle-a"), (second, "handle-b")]);
        assert_eq!(
            pending.len(),
            2,
            "duplicate payload identity cannot collide handles"
        );
    }

    #[test]
    fn delivery_token_exhaustion_fails_closed_instead_of_wrapping() {
        let sequence = AtomicU64::new(u64::MAX - 1);
        assert!(allocate_delivery_token(&sequence).is_ok());
        assert_eq!(
            allocate_delivery_token(&sequence),
            Err(TransportError("delivery token space exhausted".into()))
        );
    }

    #[test]
    fn failed_settlement_is_retained_exactly_and_gates_until_retry_succeeds() {
        let token = DeliveryToken::new(7).unwrap();
        let retries = Mutex::new(HashMap::new());
        let failure = Err(TransportError("fixed settlement failure".into()));
        queue_on_settlement_failure(
            &retries,
            token,
            "raw-handle",
            SettlementIntent::Retry(9),
            &failure,
        );

        let first = drain_queued_settlements(&retries, |handle, intent| {
            assert_eq!(handle, "raw-handle");
            assert_eq!(intent, SettlementIntent::Retry(9));
            Err(TransportError("still down".into()))
        });
        assert_eq!(
            first,
            Err(TransportError(
                "broker settlement retry remains unresolved".into()
            ))
        );
        assert_eq!(
            retries.lock().unwrap().len(),
            1,
            "failed retry remains gated"
        );

        drain_queued_settlements(&retries, |handle, intent| {
            assert_eq!(handle, "raw-handle");
            assert_eq!(intent, SettlementIntent::Retry(9));
            Ok(())
        })
        .unwrap();
        assert!(retries.lock().unwrap().is_empty());
    }

    #[test]
    fn successful_settlement_never_enters_the_retry_queue() {
        let retries = Mutex::new(HashMap::new());
        queue_on_settlement_failure(
            &retries,
            DeliveryToken::new(3).unwrap(),
            "settled-handle",
            SettlementIntent::Ack,
            &Ok(()),
        );
        assert!(retries.lock().unwrap().is_empty());
    }
}

pub struct NatsJetStreamBus {
    js: Context,
    stream_name: String,
    subject_root: String,
    consumer_name: String,
    max_batch: usize,
    max_expires: std::time::Duration,
    rt: tokio::runtime::Handle,
    pending: Mutex<HashMap<DeliveryToken, jetstream::Message>>,
    settlement_retries: Mutex<HashMap<DeliveryToken, QueuedSettlement<jetstream::Message>>>,
    intake_gate: Mutex<()>,
    next_delivery_token: AtomicU64,
    legacy_tokens: Mutex<HashMap<String, VecDeque<DeliveryToken>>>,
}

impl NatsJetStreamBus {
    pub fn connect(
        nats_url: &str,
        stream_name: &str,
        subject_root: &str,
        consumer_name: &str,
        rt: tokio::runtime::Handle,
    ) -> Result<NatsJetStreamBus, TransportError> {
        let filter = format!("{subject_root}.>");
        tokio::task::block_in_place(|| {
            rt.block_on(async {
                let client = async_nats::connect(nats_url)
                    .await
                    .map_err(|e| TransportError(format!("nats connect: {e}")))?;
                let js = jetstream::new(client);
                js.get_or_create_stream(jetstream::stream::Config {
                    name: stream_name.to_string(),
                    subjects: vec![filter.clone()],
                    retention: RetentionPolicy::Limits,
                    storage: StorageType::File,
                    max_age: std::time::Duration::from_secs(90 * 24 * 60 * 60),
                    max_bytes: 64 * 1024 * 1024,
                    max_messages: 100_000,
                    num_replicas: 1,
                    duplicate_window: std::time::Duration::from_secs(120),
                    ..Default::default()
                })
                .await
                .map_err(|e| TransportError(format!("create stream: {e}")))?;
                Ok::<(), TransportError>(())
            })
        })?;

        Self::connect_consumer(
            JetStreamConsumerConfig::bounded(
                nats_url,
                stream_name,
                subject_root,
                filter,
                consumer_name,
            ),
            rt,
        )
    }

    pub fn connect_consumer(
        config: JetStreamConsumerConfig,
        rt: tokio::runtime::Handle,
    ) -> Result<NatsJetStreamBus, TransportError> {
        config.validate()?;
        let expected = config.pull_config();
        let nats_url = config.nats_url.clone();
        let stream_name = config.stream_name.clone();
        let consumer_name = config.consumer_name.clone();
        let js = tokio::task::block_in_place(|| {
            rt.block_on(async {
                let client = async_nats::connect(&nats_url)
                    .await
                    .map_err(|e| TransportError(format!("nats connect: {e}")))?;
                let js = jetstream::new(client);
                let stream = js.get_stream(&stream_name).await.map_err(|e| {
                    TransportError(format!("get elected stream {stream_name}: {e}"))
                })?;
                let mut consumer = stream
                    .get_or_create_consumer(&consumer_name, expected.clone())
                    .await
                    .map_err(|e| TransportError(format!("create consumer {consumer_name}: {e}")))?;
                let actual = &consumer
                    .info()
                    .await
                    .map_err(|e| TransportError(format!("inspect consumer {consumer_name}: {e}")))?
                    .config;
                let drift = consumer_config_drift(actual, &expected);
                if admission_limits_are_the_only_consumer_drift(&drift) {
                    consumer = stream
                        .update_consumer(expected.clone())
                        .await
                        .map_err(|e| {
                            TransportError(format!(
                                "reconcile consumer {consumer_name} admission limits: {e}"
                            ))
                        })?;
                }
                let actual = &consumer
                    .info()
                    .await
                    .map_err(|e| TransportError(format!("inspect consumer {consumer_name}: {e}")))?
                    .config;
                validate_consumer_config(actual, &expected).map_err(|error| {
                    TransportError(format!("durable consumer {consumer_name}: {}", error.0))
                })?;
                Ok::<Context, TransportError>(js)
            })
        })?;

        Ok(NatsJetStreamBus {
            js,
            stream_name: config.stream_name,
            subject_root: config.subject_root,
            consumer_name: config.consumer_name,
            max_batch: config.max_batch,
            max_expires: config.max_expires,
            rt,
            pending: Mutex::new(HashMap::new()),
            settlement_retries: Mutex::new(HashMap::new()),
            intake_gate: Mutex::new(()),
            next_delivery_token: AtomicU64::new(0),
            legacy_tokens: Mutex::new(HashMap::new()),
        })
    }

    fn subject_for(&self, envelope: &EventEnvelope) -> Result<String, TransportError> {
        event_subject(&self.subject_root, envelope)
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }

    fn try_consume(&self) -> Result<Vec<BrokerDelivery>, TransportError> {
        let _intake = self.intake_gate.lock().unwrap_or_else(|e| e.into_inner());
        self.drain_settlement_retries()?;
        let consumer_name = self.consumer_name.clone();
        let stream_name = self.stream_name.clone();
        let msgs: Vec<jetstream::Message> = self.block(async {
            let stream = self
                .js
                .get_stream(&stream_name)
                .await
                .map_err(|e| TransportError(format!("get stream: {e}")))?;
            let consumer: PullConsumer = stream
                .get_consumer(&consumer_name)
                .await
                .map_err(|e| TransportError(format!("get consumer {consumer_name}: {e}")))?;
            let mut batch = consumer
                .batch()
                .max_messages(self.max_batch)
                .expires(self.max_expires)
                .messages()
                .await
                .map_err(|e| TransportError(format!("pull batch: {e}")))?;
            let mut out = Vec::new();
            while let Some(next) = batch.next().await {
                match next {
                    Ok(message) => out.push(message),
                    Err(error) if is_server_shutdown(error.as_ref()) => break,
                    Err(error) => {
                        return Err(TransportError(format!("pull message: {error}")));
                    }
                }
            }
            Ok::<_, TransportError>(out)
        })?;

        let mut decoded = Vec::with_capacity(msgs.len());
        for msg in msgs {
            let token = allocate_delivery_token(&self.next_delivery_token)?;
            let classify = msg.clone();
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(token, msg);

            let metadata = classify.info().ok().and_then(|info| {
                let delivery_attempt = u64::try_from(info.delivered).ok().filter(|n| *n > 0)?;
                (info.stream_sequence > 0).then(|| {
                    (
                        BrokerDeliveryRef {
                            stream: info.stream.to_string(),
                            stream_sequence: info.stream_sequence,
                        },
                        delivery_attempt,
                    )
                })
            });
            let Some((broker_ref, delivery_attempt)) = metadata else {
                decoded.push(BrokerDelivery {
                    token,
                    broker_ref: None,
                    body: BrokerDeliveryBody::TransientMetadataFault,
                    delivery_attempt: None,
                });
                continue;
            };
            let body = classify_delivery_body(
                &self.subject_root,
                classify.subject.as_str(),
                &classify.payload,
            );
            decoded.push(BrokerDelivery {
                token,
                broker_ref: Some(broker_ref),
                body,
                delivery_attempt: Some(delivery_attempt),
            });
        }
        Ok(decoded)
    }

    fn take_pending(&self, token: DeliveryToken) -> Result<jetstream::Message, TransportError> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&token)
            .ok_or_else(|| TransportError("no pending JetStream delivery for token".into()))
    }

    fn settle_message(
        &self,
        message: jetstream::Message,
        intent: SettlementIntent,
    ) -> Result<(), TransportError> {
        match intent {
            SettlementIntent::Ack => self.block(async {
                message
                    .double_ack()
                    .await
                    .map_err(|_| TransportError("broker ACK failed".into()))
            }),
            SettlementIntent::Retry(delay_secs) => {
                let delay = std::time::Duration::from_secs(delay_secs.clamp(1, 300));
                self.block(async {
                    message
                        .ack_with(AckKind::Nak(Some(delay)))
                        .await
                        .map_err(|_| TransportError("broker NAK failed".into()))
                })
            }
            SettlementIntent::Terminate => self.block(async {
                message
                    .ack_with(AckKind::Term)
                    .await
                    .map_err(|_| TransportError("broker TERM failed".into()))
            }),
        }
    }

    fn drain_settlement_retries(&self) -> Result<(), TransportError> {
        drain_queued_settlements(&self.settlement_retries, |message, intent| {
            self.settle_message(message, intent)
        })
    }

    fn try_settle(
        &self,
        token: DeliveryToken,
        intent: SettlementIntent,
    ) -> Result<(), TransportError> {
        let _intake = self.intake_gate.lock().unwrap_or_else(|e| e.into_inner());
        let message = self.take_pending(token)?;
        let retained = message.clone();
        let result = self.settle_message(message, intent);
        queue_on_settlement_failure(&self.settlement_retries, token, retained, intent, &result);
        result
    }

    fn try_ack(&self, token: DeliveryToken) -> Result<(), TransportError> {
        self.try_settle(token, SettlementIntent::Ack)
    }

    fn try_retry(&self, token: DeliveryToken, delay_secs: u64) -> Result<(), TransportError> {
        self.try_settle(token, SettlementIntent::Retry(delay_secs.clamp(1, 300)))
    }

    fn try_terminate(&self, token: DeliveryToken) -> Result<(), TransportError> {
        self.try_settle(token, SettlementIntent::Terminate)
    }

    fn legacy_token(&self, event_id: &EventId) -> Result<DeliveryToken, TransportError> {
        let msg = {
            let mut legacy = self.legacy_tokens.lock().unwrap_or_else(|e| e.into_inner());
            legacy.get_mut(&event_id.0).and_then(VecDeque::pop_front)
        };
        msg.ok_or_else(|| TransportError("no legacy delivery token for event".into()))
    }
}

#[cfg(test)]
mod publisher_tests {
    use async_nats::jetstream::consumer::IntoConsumerConfig;

    use super::*;

    fn nats_error(message: &str) -> async_nats::Error {
        std::io::Error::other(message).into()
    }

    fn config() -> JetStreamPublisherConfig {
        JetStreamPublisherConfig {
            nats_url: "nats://127.0.0.1:4222".into(),
            stream_name: "MYELIN_EVENTS".into(),
            subject_root: "myelin.events".into(),
            max_age: std::time::Duration::from_secs(90 * 24 * 60 * 60),
            max_bytes: 64 * 1024 * 1024,
            max_messages: 100_000,
            replicas: 3,
            duplicate_window: std::time::Duration::from_secs(120),
            publish_ack_timeout: std::time::Duration::from_secs(2),
        }
    }

    #[test]
    fn only_server_shutdown_is_a_graceful_pull_ending() {
        let shutdown = nats_error(SERVER_SHUTDOWN_PULL_ERROR);
        assert!(is_server_shutdown(shutdown.as_ref()));

        for error in [
            "error while processing messages from the stream: 409, Some(\"Consumer Deleted\")",
            "error while processing messages from the stream: 503, Some(\"Server Shutdown\")",
            "connection reset by peer",
        ] {
            let error = nats_error(error);
            assert!(
                !is_server_shutdown(error.as_ref()),
                "unrelated broker failure was hidden: {error}"
            );
        }
    }

    #[test]
    fn production_stream_config_is_finite_file_backed_limits_retention() {
        let cfg = config();
        cfg.validate().expect("valid explicit config");
        let stream = cfg.stream_config();
        assert_eq!(stream.storage, StorageType::File);
        assert_eq!(stream.retention, RetentionPolicy::Limits);
        assert_eq!(stream.discard, DiscardPolicy::Old);
        assert_eq!(stream.max_age, cfg.max_age);
        assert_eq!(stream.max_bytes, cfg.max_bytes);
        assert_eq!(stream.max_messages, cfg.max_messages);
        assert_eq!(stream.num_replicas, cfg.replicas);
        assert_eq!(stream.subjects, vec!["myelin.events.>"]);
        assert_eq!(stream.duplicate_window, cfg.duplicate_window);
    }

    #[test]
    fn config_debug_redacts_nats_credentials() {
        let secret = "unique-nats-password";
        let mut publisher = config();
        publisher.nats_url = format!("nats://operator:{secret}@127.0.0.1:4222");
        let consumer = JetStreamConsumerConfig::bounded(
            publisher.nats_url.clone(),
            "MYELIN_EVENTS",
            "myelin.events",
            "myelin.events.git",
            "git-consumer",
        );

        for rendered in [format!("{publisher:?}"), format!("{consumer:?}")] {
            assert!(rendered.contains("<redacted>"));
            assert!(!rendered.contains(secret));
        }
    }

    #[test]
    fn refuses_unbounded_or_drifted_stream_configuration() {
        let mut cfg = config();
        cfg.max_bytes = -1;
        assert!(cfg.validate().is_err());

        let expected = config().stream_config();
        let mut actual = expected.clone();
        actual.num_replicas = 1;
        let err = validate_stream_config(&actual, &expected).expect_err("replica drift");
        assert_eq!(
            err.0,
            "existing JetStream stream configuration is incompatible"
        );

        let mut actual = expected.clone();
        actual.discard = DiscardPolicy::New;
        let err = validate_stream_config(&actual, &expected).expect_err("discard drift");
        assert_eq!(
            err.0,
            "existing JetStream stream configuration is incompatible"
        );
    }

    #[tokio::test]
    async fn unresponsive_publish_ack_times_out_with_a_fixed_redacted_error() {
        let error = publish_with_timeout(
            std::time::Duration::from_millis(1),
            std::future::pending::<Result<(), TransportError>>(),
        )
        .await
        .expect_err("pending acknowledgement must time out");
        assert_eq!(error.0, "JetStream publish acknowledgement timed out");
    }

    #[test]
    fn publisher_config_debug_and_validation_errors_redact_authority() {
        let sentinel = "NATS_AUTHORITY_SENTINEL";
        let mut cfg = config();
        cfg.nats_url = format!("nats://publisher:{sentinel}@broker.invalid:4222");
        let debug = format!("{cfg:?}");
        assert!(!debug.contains(sentinel));
        assert!(!debug.contains("nats://"));

        cfg.stream_name = "invalid.name".into();
        let error = cfg.validate().expect_err("invalid stream name");
        assert!(!error.0.contains(sentinel));
        assert!(!error.0.contains("broker.invalid"));
    }

    #[test]
    fn pull_consumer_policy_is_explicit_bounded_and_root_scoped() {
        let config = JetStreamConsumerConfig::bounded(
            "nats://127.0.0.1:4222",
            "MYELIN_EVENTS",
            "myelin.events",
            "myelin.events.evt.*.git.>",
            "ci-dispatch-trigger",
        );
        config.validate().unwrap();
        let pull = config.pull_config();
        assert_eq!(pull.durable_name.as_deref(), Some("ci-dispatch-trigger"));
        assert_eq!(pull.ack_policy, jetstream::consumer::AckPolicy::Explicit);
        assert_eq!(pull.deliver_policy, jetstream::consumer::DeliverPolicy::All);
        assert_eq!(pull.filter_subject, "myelin.events.evt.*.git.>");
        assert!(pull.ack_wait > std::time::Duration::ZERO);
        assert_eq!(pull.max_deliver, -1, "never strand a final unacked retry");
        assert!(pull.max_ack_pending > 0);
        assert!(pull.max_batch > 0);
        assert!(pull.max_expires > std::time::Duration::ZERO);

        let mut escaped = config;
        escaped.filter_subject = "other.events.>".into();
        assert!(escaped.validate().is_err());
    }

    #[test]
    fn durable_worker_admission_controls_the_actual_pull_window() {
        let admission = crate::DurableWorkerAdmission::new(96, 32, 24).unwrap();
        let config = JetStreamConsumerConfig::bounded(
            "nats://127.0.0.1:4222",
            "MYELIN_EVENTS",
            "myelin.events",
            "myelin.events.evt.*.refs.>",
            "refs-edge-builder-intake",
        )
        .with_admission(admission);

        config.validate().unwrap();
        assert_eq!(config.max_ack_pending, 96);
        assert_eq!(config.max_batch, 32);
        let pull = config.pull_config();
        assert_eq!(pull.max_ack_pending, 96);
        assert_eq!(pull.max_batch, 32);
    }

    #[test]
    fn every_pinned_pull_consumer_field_refuses_semantic_drift() {
        let expected = JetStreamConsumerConfig::bounded(
            "nats://127.0.0.1:4222",
            "MYELIN_EVENTS",
            "myelin.events",
            "myelin.events.evt.*.git.>",
            "ci-dispatch-trigger",
        )
        .pull_config();
        let baseline = expected.clone().into_consumer_config();
        validate_consumer_config(&baseline, &expected).expect("the exact policy matches");

        let assert_drift = |actual: jetstream::consumer::Config, field: &str| {
            let error = validate_consumer_config(&actual, &expected).expect_err(field);
            assert!(
                error.0.contains(field),
                "{field} drift was not named: {error:?}"
            );
        };
        macro_rules! drift {
            ($field:ident, $value:expr) => {{
                let mut actual = baseline.clone();
                actual.$field = $value;
                assert_drift(actual, stringify!($field));
            }};
        }

        drift!(durable_name, Some("other".into()));
        drift!(name, Some("other".into()));
        drift!(description, Some("changed".into()));
        drift!(deliver_subject, Some("push.target".into()));
        drift!(deliver_group, Some("workers".into()));
        drift!(deliver_policy, jetstream::consumer::DeliverPolicy::Last);
        drift!(ack_policy, jetstream::consumer::AckPolicy::None);
        drift!(ack_wait, std::time::Duration::from_secs(31));
        drift!(max_deliver, 4);
        drift!(filter_subject, "myelin.events.>".into());
        drift!(filter_subjects, vec!["myelin.events.>".into()]);
        drift!(replay_policy, jetstream::consumer::ReplayPolicy::Original);
        drift!(rate_limit, 1);
        drift!(sample_frequency, 1);
        drift!(max_waiting, 9);
        drift!(max_ack_pending, 257);
        drift!(headers_only, true);
        drift!(flow_control, true);
        drift!(idle_heartbeat, std::time::Duration::from_secs(1));
        drift!(max_batch, 257);
        drift!(max_bytes, 4 * 1024 * 1024 + 1);
        drift!(max_expires, std::time::Duration::from_secs(2));
        drift!(inactive_threshold, std::time::Duration::from_secs(1));
        drift!(num_replicas, 1);
        drift!(memory_storage, true);
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("owner".into(), "other".into());
        drift!(metadata, metadata);
        drift!(backoff, vec![std::time::Duration::from_secs(1)]);
    }

    #[test]
    fn only_bounded_admission_limits_are_safe_to_reconcile() {
        let expected = JetStreamConsumerConfig::bounded(
            "nats://127.0.0.1:4222",
            "MYELIN_EVENTS",
            "myelin.events",
            "myelin.events.evt.*.notif.>",
            "notif-signal-router",
        )
        .with_admission(crate::DurableWorkerAdmission::new(24, 8, 6).unwrap())
        .pull_config();
        let mut actual = expected.clone().into_consumer_config();
        actual.max_ack_pending = 256;
        actual.max_batch = 256;

        let drift = consumer_config_drift(&actual, &expected);
        assert_eq!(drift, ["max_ack_pending", "max_batch"]);
        assert!(admission_limits_are_the_only_consumer_drift(&drift));

        actual.ack_policy = jetstream::consumer::AckPolicy::None;
        let drift = consumer_config_drift(&actual, &expected);
        assert!(!admission_limits_are_the_only_consumer_drift(&drift));
    }
}

impl BusTransport for NatsJetStreamBus {
    fn put(
        &self,
        _subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> Result<Delivery, TransportError> {
        let nats_subject = self.subject_for(envelope)?;
        let body = serde_json::to_vec(envelope)
            .map_err(|e| TransportError(format!("serialize envelope: {e}")))?;

        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Nats-Msg-Id", dedup_id.0.as_str());

        let ack = self.block(async {
            self.js
                .publish_with_headers(nats_subject, headers, body.into())
                .await
                .map_err(|e| TransportError(format!("publish: {e}")))?
                .await
                .map_err(|e| TransportError(format!("publish ack: {e}")))
        })?;

        if ack.duplicate {
            Ok(Delivery::Deduplicated)
        } else {
            Ok(Delivery::Accepted)
        }
    }

    fn consume(&self, _subject_prefix: &str) -> Vec<EventEnvelope> {
        self.try_consume()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|delivery| match delivery.body {
                BrokerDeliveryBody::Event(envelope) => {
                    self.legacy_tokens
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .entry(envelope.event_id.0.clone())
                        .or_default()
                        .push_back(delivery.token);
                    Some(*envelope)
                }
                BrokerDeliveryBody::Poison(_) | BrokerDeliveryBody::TransientMetadataFault => {
                    let _ = self.try_retry(delivery.token, 1);
                    None
                }
            })
            .collect()
    }

    fn ack(&self, _consumer: &str, event_id: &EventId) {
        if let Ok(token) = self.legacy_token(event_id) {
            let _ = self.try_ack(token);
        }
    }

    fn purge(&self) {
        let stream_name = self.stream_name.clone();
        self.block(async {
            if let Ok(stream) = self.js.get_stream(&stream_name).await {
                let _ = stream.purge().await;
            }
        });
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.settlement_retries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.legacy_tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl crate::relay::EventConsumer for NatsJetStreamBus {
    fn durable_name(&self) -> &str {
        &self.consumer_name
    }

    fn consume(&self, _subject_prefix: &str) -> Result<Vec<BrokerDelivery>, TransportError> {
        self.try_consume()
    }

    fn flush_settlements(&self) -> Result<(), TransportError> {
        let _intake = self.intake_gate.lock().unwrap_or_else(|e| e.into_inner());
        self.drain_settlement_retries()
    }

    fn ack(&self, token: DeliveryToken) -> Result<(), TransportError> {
        self.try_ack(token)
    }

    fn retry(&self, token: DeliveryToken, delay_secs: u64) -> Result<(), TransportError> {
        self.try_retry(token, delay_secs)
    }

    fn terminate(&self, token: DeliveryToken) -> Result<(), TransportError> {
        self.try_terminate(token)
    }
}
