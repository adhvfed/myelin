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

use std::collections::HashMap;
use std::sync::Mutex;

use async_nats::jetstream::consumer::PullConsumer;
use async_nats::jetstream::stream::{
    Config as StreamConfig, DiscardPolicy, RetentionPolicy, StorageType,
};
use async_nats::jetstream::{self, AckKind, Context};
use futures::StreamExt;

use crate::relay::{BrokerDelivery, BusTransport, Delivery, EventPublisher, TransportError};
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
    /// Ack handles for messages delivered by [`BusTransport::consume`] but not yet acked, keyed
    /// by `event_id` so [`BusTransport::ack`] acks EXACTLY the delivered message. Behind a Mutex
    /// because the sync trait has `&self`.
    pending: Mutex<HashMap<String, jetstream::Message>>,
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
                if actual.durable_name.as_deref() != Some(consumer_name.as_str())
                    || actual.deliver_policy != jetstream::consumer::DeliverPolicy::All
                    || actual.ack_policy != jetstream::consumer::AckPolicy::Explicit
                    || actual.ack_wait != config.ack_wait
                    || actual.max_deliver != config.max_deliver
                    || actual.filter_subject != config.filter_subject
                    || actual.max_ack_pending != config.max_ack_pending
                    || actual.max_batch != config.max_batch as i64
                    || actual.max_expires != config.max_expires
                {
                    return Err(TransportError(format!(
                        "durable consumer {consumer_name} configuration drifted from bounded policy"
                    )));
                }
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
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        for msg in msgs {
            let envelope = serde_json::from_slice::<EventEnvelope>(&msg.payload)
                .map_err(|e| TransportError(format!("decode event envelope: {e}")))?;
            let expected_subject = self.subject_for(&envelope)?;
            if msg.subject.as_str() != expected_subject {
                return Err(TransportError(format!(
                    "broker subject {} disagrees with envelope route {}",
                    msg.subject, expected_subject
                )));
            }
            let delivery_attempt = msg
                .info()
                .map_err(|e| TransportError(format!("read JetStream delivery metadata: {e}")))?
                .delivered;
            let delivery_attempt = u64::try_from(delivery_attempt)
                .ok()
                .filter(|attempt| *attempt > 0)
                .ok_or_else(|| TransportError("invalid JetStream delivery attempt count".into()))?;
            pending.insert(envelope.event_id.0.clone(), msg);
            decoded.push(BrokerDelivery {
                envelope,
                delivery_attempt,
            });
        }
        Ok(decoded)
    }

    fn try_ack(&self, event_id: &EventId) -> Result<(), TransportError> {
        let msg = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.remove(&event_id.0)
        }
        .ok_or_else(|| {
            TransportError(format!(
                "no pending JetStream delivery for event {}",
                event_id.0
            ))
        })?;
        self.block(async {
            // Wait for the JetStream acknowledgement-of-ack before reporting success. A plain
            // publish-only ack can be lost if the process exits immediately after this method,
            // causing an already-completed business effect to be redelivered after restart.
            msg.double_ack()
                .await
                .map_err(|e| TransportError(format!("ack event {}: {e}", event_id.0)))
        })
    }

    fn try_retry(&self, event_id: &EventId, delay_secs: u64) -> Result<(), TransportError> {
        let msg = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.remove(&event_id.0)
        }
        .ok_or_else(|| {
            TransportError(format!(
                "no pending JetStream delivery for event {}",
                event_id.0
            ))
        })?;
        let delay = std::time::Duration::from_secs(delay_secs.clamp(1, 300));
        self.block(async {
            msg.ack_with(AckKind::Nak(Some(delay)))
                .await
                .map_err(|e| TransportError(format!("NAK event {}: {e}", event_id.0)))
        })
    }

    fn try_terminate(&self, event_id: &EventId) -> Result<(), TransportError> {
        let msg = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.remove(&event_id.0)
        }
        .ok_or_else(|| {
            TransportError(format!(
                "no pending JetStream delivery for event {}",
                event_id.0
            ))
        })?;
        self.block(async {
            msg.ack_with(AckKind::Term)
                .await
                .map_err(|e| TransportError(format!("TERM event {}: {e}", event_id.0)))
        })
    }
}

#[cfg(test)]
mod publisher_tests {
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
            .map(|delivery| delivery.envelope)
            .collect()
    }

    fn ack(&self, _consumer: &str, event_id: &EventId) {
        // Explicit ack of the delivered message stashed by `consume` (at-least-once → the ack is
        // what makes it not redeliver). Acking an un-consumed / already-acked id is a no-op.
        let _ = self.try_ack(event_id);
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
    }
}

impl crate::relay::EventConsumer for NatsJetStreamBus {
    fn consume(&self, _subject_prefix: &str) -> Result<Vec<BrokerDelivery>, TransportError> {
        self.try_consume()
    }

    fn ack(&self, _consumer: &str, event_id: &EventId) -> Result<(), TransportError> {
        self.try_ack(event_id)
    }

    fn retry(
        &self,
        _consumer: &str,
        event_id: &EventId,
        delay_secs: u64,
    ) -> Result<(), TransportError> {
        self.try_retry(event_id, delay_secs)
    }

    fn terminate(&self, _consumer: &str, event_id: &EventId) -> Result<(), TransportError> {
        self.try_terminate(event_id)
    }
}
