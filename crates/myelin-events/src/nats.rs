//! `NatsJetStreamBus` — the [`BusTransport`](crate::relay::BusTransport) trait backed by a REAL
//! NATS JetStream (Apache-2.0) durable bus via `async-nats`.
//!
//! **Stage 2 / infra.** This is the durable backing for the bus the [`crate::relay`] module's
//! [`crate::relay::InProcessBus`] floor models in process. It implements the EXACT frozen
//! `put/consume/ack/purge` shape behind the existing [`BusTransport`] trait — it does NOT fork
//! or redefine the trait (EI-01 §7 coherence). The in-process fake remains the unit/default
//! transport; this real backing is config-selected under production's `nats` feature (also
//! included by the live-test `integration` feature).
//!
//! ## The JetStream wiring (durable stream + durable PULL consumer + ack)
//! - A durable **stream** captures the subject set, with `MsgId`-based **dedup** so a second
//!   `put` carrying an `event_id` the stream already accepted is suppressed (the
//!   `Nats-Msg-Id = event_id` broker-side dedup → 0 ghost, exactly the property the
//!   [`BusTransport::put`] contract names).
//! - A durable **PULL consumer** (an explicit-ack consumer) is what [`BusTransport::consume`]
//!   fetches a batch from; the delivered messages' ack handles are stashed keyed by `event_id`
//!   so [`BusTransport::ack`] can ack EXACTLY the delivered message (explicit ack, at-least-once
//!   with consumer-side dedup the durable consumer provides).
//!
//! ## How a sync trait drives the async client
//! [`BusTransport`] is sync (it matches the in-process floor). `async-nats` is async, so
//! `NatsJetStreamBus` holds a `tokio::runtime::Handle` and drives each op with `block_in_place`
//! + `block_on` — the same bridge the storage S3/Valkey backings use.

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

/// Explicit capacity and durability policy for the shared production event stream.
///
/// There are deliberately no unlimited defaults: callers must choose finite byte and message
/// bounds, a retention age, a deduplication window, and a replica count appropriate for the NATS
/// cluster they operate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JetStreamPublisherConfig {
    pub nats_url: String,
    pub stream_name: String,
    pub subject_root: String,
    pub max_age: std::time::Duration,
    pub max_bytes: i64,
    pub max_messages: i64,
    pub replicas: usize,
    pub duplicate_window: std::time::Duration,
}

/// Explicit bounded-capacity policy for a durable JetStream pull consumer.
#[derive(Clone, Debug, PartialEq, Eq)]
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

impl JetStreamConsumerConfig {
    /// The bounded production defaults. A caller still supplies the exact server-side filter.
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
            // Unlimited redelivery is intentional until broker delivery-attempt metadata is
            // carried into the durable application DLQ transaction. A finite broker-only cap
            // would strand the final unacked Retry with only a JetStream advisory.
            max_deliver: -1,
            max_ack_pending: 256,
            max_batch: 256,
            max_expires: std::time::Duration::from_secs(1),
        }
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
    /// Refuse incomplete, unlimited, or internally inconsistent production stream settings.
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
    let mut drift = Vec::new();
    if actual.storage != StorageType::File {
        drift.push(format!("storage={:?} (expected File)", actual.storage));
    }
    if actual.retention != RetentionPolicy::Limits {
        drift.push(format!(
            "retention={:?} (expected Limits)",
            actual.retention
        ));
    }
    if actual.discard != DiscardPolicy::Old {
        drift.push(format!("discard={:?} (expected Old)", actual.discard));
    }
    if actual.max_age != expected.max_age {
        drift.push(format!(
            "max_age={:?} (expected {:?})",
            actual.max_age, expected.max_age
        ));
    }
    if actual.max_bytes != expected.max_bytes {
        drift.push(format!(
            "max_bytes={} (expected {})",
            actual.max_bytes, expected.max_bytes
        ));
    }
    if actual.max_messages != expected.max_messages {
        drift.push(format!(
            "max_messages={} (expected {})",
            actual.max_messages, expected.max_messages
        ));
    }
    if actual.num_replicas != expected.num_replicas {
        drift.push(format!(
            "replicas={} (expected {})",
            actual.num_replicas, expected.num_replicas
        ));
    }
    if actual.subjects != expected.subjects {
        drift.push(format!(
            "subjects={:?} (expected {:?})",
            actual.subjects, expected.subjects
        ));
    }
    if actual.duplicate_window != expected.duplicate_window {
        drift.push(format!(
            "duplicate_window={:?} (expected {:?})",
            actual.duplicate_window, expected.duplicate_window
        ));
    }

    if drift.is_empty() {
        Ok(())
    } else {
        Err(TransportError(format!(
            "existing JetStream stream configuration is incompatible: {}",
            drift.join(", ")
        )))
    }
}

/// Refuse any semantic drift in the durable pull consumer. Every field emitted by
/// [`JetStreamConsumerConfig::pull_config`] is pinned, including the safety-relevant defaults:
/// a pull (not push) consumer, no rate/sample/backoff overrides, disk-backed inherited storage,
/// and no alternate filter set.
fn validate_consumer_config(
    actual: &jetstream::consumer::Config,
    expected: &jetstream::consumer::pull::Config,
) -> Result<(), TransportError> {
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
        .map_err(|_| TransportError("invalid event routing subject".into()))?;
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
            Ok(expected) if actual_subject == expected => BrokerDeliveryBody::Event(envelope),
            _ => BrokerDeliveryBody::Poison(DeliveryPoisonKind::SubjectMismatch),
        },
    }
}

fn allocate_delivery_token(sequence: &AtomicU64) -> DeliveryToken {
    DeliveryToken(sequence.fetch_add(1, Ordering::Relaxed).wrapping_add(1))
}

fn retain_on_settlement_failure<T>(
    pending: &Mutex<HashMap<DeliveryToken, T>>,
    token: DeliveryToken,
    retained: T,
    result: &Result<(), TransportError>,
) {
    if result.is_err() {
        pending.lock().unwrap_or_else(|e| e.into_inner()).insert(token, retained);
    }
}

/// A capability-minimal production adapter for the elected shared-outbox relay.
///
/// Construction provisions or validates the shared stream and creates no consumer. Its public
/// surface implements only [`EventPublisher`], so the relay cannot consume, acknowledge, or purge
/// production history.
pub struct NatsJetStreamPublisher {
    js: Context,
    subject_root: String,
    rt: tokio::runtime::Handle,
}

impl NatsJetStreamPublisher {
    pub fn connect(
        config: JetStreamPublisherConfig,
        rt: tokio::runtime::Handle,
    ) -> Result<Self, TransportError> {
        config.validate()?;
        let expected = config.stream_config();
        let subject_root = config.subject_root.clone();

        let js = tokio::task::block_in_place(|| {
            rt.block_on(async {
                let client = async_nats::connect(&config.nats_url)
                    .await
                    .map_err(|e| TransportError(format!("nats connect: {e}")))?;
                let js = jetstream::new(client);
                let stream = js
                    .get_or_create_stream(expected.clone())
                    .await
                    .map_err(|e| TransportError(format!("create/get stream: {e}")))?;
                validate_stream_config(&stream.cached_info().config, &expected)?;
                Ok::<Context, TransportError>(js)
            })
        })?;

        Ok(Self {
            js,
            subject_root,
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
            Err(TransportError("invalid event routing subject".into()))
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
            classify_delivery_body(
                "root", &valid_subject, &serde_json::to_vec(&first).unwrap(),
            ),
            classify_delivery_body(
                "root", "root.evt.other.route", &serde_json::to_vec(&first).unwrap(),
            ),
            classify_delivery_body(
                "root", &valid_subject, &serde_json::to_vec(&second).unwrap(),
            ),
        ];
        assert!(matches!(bodies[0], BrokerDeliveryBody::Poison(DeliveryPoisonKind::MalformedEnvelope)));
        assert!(matches!(bodies[1], BrokerDeliveryBody::Event(_)));
        assert!(matches!(bodies[2], BrokerDeliveryBody::Poison(DeliveryPoisonKind::SubjectMismatch)));
        assert!(matches!(bodies[3], BrokerDeliveryBody::Event(_)));
    }

    #[test]
    fn duplicate_event_ids_still_receive_distinct_opaque_settlement_tokens() {
        let sequence = AtomicU64::new(0);
        let first = allocate_delivery_token(&sequence);
        let second = allocate_delivery_token(&sequence);
        assert_ne!(first, second);
        let pending = HashMap::from([(first, "handle-a"), (second, "handle-b")]);
        assert_eq!(pending.len(), 2, "duplicate payload identity cannot collide handles");
    }

    #[test]
    fn settlement_failure_reinserts_the_exact_token_handle() {
        let token = DeliveryToken(7);
        let pending = Mutex::new(HashMap::new());
        let failure = Err(TransportError("fixed settlement failure".into()));
        retain_on_settlement_failure(&pending, token, "raw-handle", &failure);
        assert_eq!(pending.lock().unwrap().get(&token), Some(&"raw-handle"));
    }
}

/// The [`BusTransport`] backed by a real NATS JetStream stream + durable PULL consumer.
pub struct NatsJetStreamBus {
    js: Context,
    stream_name: String,
    subject_root: String,
    consumer_name: String,
    max_batch: usize,
    max_expires: std::time::Duration,
    rt: tokio::runtime::Handle,
    /// Raw handles are keyed only by opaque process-local delivery token. Event ids are payload
    /// data and cannot identify malformed messages or distinguish duplicate-id deliveries.
    pending: Mutex<HashMap<DeliveryToken, jetstream::Message>>,
    next_delivery_token: AtomicU64,
    /// Compatibility lookup for the legacy `BusTransport` surface. The production
    /// [`crate::relay::EventConsumer`] path settles directly by token.
    legacy_tokens: Mutex<HashMap<String, VecDeque<DeliveryToken>>>,
}

impl NatsJetStreamBus {
    /// Connect to NATS at `nats_url`, ensure a durable JetStream stream named `stream_name`
    /// capturing `subject_root.>` exists (with `MsgId` dedup), and ensure a durable PULL
    /// consumer named `consumer_name` exists. `rt` is the runtime handle the sync trait methods
    /// drive the async client on. Idempotent: re-connecting reuses the existing stream/consumer.
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

    /// Connect only the bounded durable pull consumer to an already elected/shared stream.
    /// Production consumer services use this constructor so they never create or mutate stream
    /// publisher state. Existing consumer configuration is validated and drift fails boot.
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
            next_delivery_token: AtomicU64::new(0),
            legacy_tokens: Mutex::new(HashMap::new()),
        })
    }

    /// Map an event onto a concrete JetStream subject under the stream's root. EB-12: the routing +
    /// ordering key is now the §2.2 STRUCTURED subject
    /// `evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>` ([`StreamSubject`]),
    /// derived from the envelope — so the broker subject encodes the `(tenant, subsystem)` routing
    /// split (the blast-radius unit) and the per-aggregate ordering partition, not an opaque token.
    /// We keep the transport's `subject_root` as the stream-capture + consume-filter namespace and
    /// slot the structured subject beneath it: `<subject_root>.evt.<tenant>.…`. So the stream's
    /// `<root>.>` filter still captures every event and a `consume(subject_root)` still matches,
    /// while the subject carries the real §2.2 key.
    ///
    /// A malformed envelope is rejected before a broker operation. Durable elected relays
    /// quarantine it in PostgreSQL; transports must never invent a fallback routing namespace.
    fn subject_for(&self, envelope: &EventEnvelope) -> Result<String, TransportError> {
        event_subject(&self.subject_root, envelope)
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }

    fn try_consume(&self) -> Result<Vec<BrokerDelivery>, TransportError> {
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
                out.push(next.map_err(|e| TransportError(format!("pull message: {e}")))?);
            }
            Ok::<_, TransportError>(out)
        })?;

        let mut decoded = Vec::with_capacity(msgs.len());
        for msg in msgs {
            let token = allocate_delivery_token(&self.next_delivery_token);
            let classify = msg.clone();
            // Insert the raw handle before any fallible metadata/decode/routing operation.
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(token, msg);

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
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&token)
            .ok_or_else(|| TransportError("no pending JetStream delivery for token".into()))
    }

    fn try_ack(&self, token: DeliveryToken) -> Result<(), TransportError> {
        let msg = self.take_pending(token)?;
        let retained = msg.clone();
        let result = self.block(async {
            // Wait for the JetStream acknowledgement-of-ack before reporting success. A plain
            // publish-only ack can be lost if the process exits immediately after this method.
            msg.double_ack().await.map_err(|_| TransportError("broker ACK failed".into()))
        });
        retain_on_settlement_failure(&self.pending, token, retained, &result);
        result
    }

    fn try_retry(&self, token: DeliveryToken, delay_secs: u64) -> Result<(), TransportError> {
        let msg = self.take_pending(token)?;
        let retained = msg.clone();
        let delay = std::time::Duration::from_secs(delay_secs.clamp(1, 300));
        let result = self.block(async {
            msg.ack_with(AckKind::Nak(Some(delay))).await
                .map_err(|_| TransportError("broker NAK failed".into()))
        });
        retain_on_settlement_failure(&self.pending, token, retained, &result);
        result
    }

    fn try_terminate(&self, token: DeliveryToken) -> Result<(), TransportError> {
        let msg = self.take_pending(token)?;
        let retained = msg.clone();
        let result = self.block(async {
            msg.ack_with(AckKind::Term).await
                .map_err(|_| TransportError("broker TERM failed".into()))
        });
        retain_on_settlement_failure(&self.pending, token, retained, &result);
        result
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
    fn refuses_unbounded_or_drifted_stream_configuration() {
        let mut cfg = config();
        cfg.max_bytes = -1;
        assert!(cfg.validate().is_err());

        let expected = config().stream_config();
        let mut actual = expected.clone();
        actual.num_replicas = 1;
        let err = validate_stream_config(&actual, &expected).expect_err("replica drift");
        assert!(err.0.contains("replicas=1"));

        let mut actual = expected.clone();
        actual.discard = DiscardPolicy::New;
        let err = validate_stream_config(&actual, &expected).expect_err("discard drift");
        assert!(err.0.contains("discard=New"));
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
    fn every_pinned_pull_consumer_field_refuses_semantic_drift() {
        let expected = JetStreamConsumerConfig::bounded(
            "nats://127.0.0.1:4222",
            "MYELIN_EVENTS",
            "myelin.events",
            "myelin.events.evt.*.git.>",
            "ci-dispatch-trigger",
        ).pull_config();
        let baseline = expected.clone().into_consumer_config();
        validate_consumer_config(&baseline, &expected).expect("the exact policy matches");

        let assert_drift = |actual: jetstream::consumer::Config, field: &str| {
            let error = validate_consumer_config(&actual, &expected).expect_err(field);
            assert!(error.0.contains(field), "{field} drift was not named: {error:?}");
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
        // Nats-Msg-Id = the stable event_id → broker-side dedup (0 ghost): a re-publish of the
        // same event_id is suppressed and flagged `duplicate` on the ack.
        headers.insert("Nats-Msg-Id", dedup_id.0.as_str());

        let ack = self.block(async {
            self.js
                .publish_with_headers(nats_subject, headers, body.into())
                .await
                .map_err(|e| TransportError(format!("publish: {e}")))?
                .await
                .map_err(|e| TransportError(format!("publish ack: {e}")))
        })?;

        // `duplicate` true ⇒ the broker already had this dedup_id ⇒ Deduplicated; else Accepted.
        if ack.duplicate {
            Ok(Delivery::Deduplicated)
        } else {
            Ok(Delivery::Accepted)
        }
    }

    fn consume(&self, _subject_prefix: &str) -> Vec<EventEnvelope> {
        // Fetch a batch from the durable PULL consumer. The delivered messages' ack handles are
        // stashed keyed by event_id so `ack` can ack exactly the delivered message (explicit
        // ack). Returns the decoded envelopes in delivery order. A pull with no messages within
        // the short expiry returns empty (a clean "nothing to consume right now").
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
                    Some(envelope)
                }
                BrokerDeliveryBody::Poison(_) | BrokerDeliveryBody::TransientMetadataFault => {
                    // Legacy consumers have no quarantine seam. NAK independently instead of
                    // silently leaking the raw handle or discarding valid siblings.
                    let _ = self.try_retry(delivery.token, 1);
                    None
                }
            })
            .collect()
    }

    fn ack(&self, _consumer: &str, event_id: &EventId) {
        // Explicit ack of the delivered message stashed by `consume` (at-least-once → the ack is
        // what makes it not redeliver). Acking an un-consumed / already-acked id is a no-op.
        if let Ok(token) = self.legacy_token(event_id) {
            if self.try_ack(token).is_err() {
                self.legacy_tokens
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(event_id.0.clone())
                    .or_default()
                    .push_front(token);
            }
        }
    }

    fn purge(&self) {
        // Purge the stream's accepted/dedup state (test/GC convenience — the frozen shape's
        // fourth method). Best-effort: a transport error is swallowed (purge is not on a
        // correctness path).
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

    fn ack(&self, token: DeliveryToken) -> Result<(), TransportError> {
        self.try_ack(token)
    }

    fn retry(
        &self,
        token: DeliveryToken,
        delay_secs: u64,
    ) -> Result<(), TransportError> {
        self.try_retry(token, delay_secs)
    }

    fn terminate(&self, token: DeliveryToken) -> Result<(), TransportError> {
        self.try_terminate(token)
    }
}
