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
use async_nats::jetstream::{self, Context};
use futures::StreamExt;

use crate::relay::{BusTransport, Delivery, EventPublisher, TransportError};
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
        let subject_root = subject_root.to_string();
        let stream_name = stream_name.to_string();
        let consumer_name = consumer_name.to_string();
        let filter = format!("{subject_root}.>");

        let js = tokio::task::block_in_place(|| {
            rt.block_on(async {
                let client = async_nats::connect(nats_url)
                    .await
                    .map_err(|e| TransportError(format!("nats connect: {e}")))?;
                let js = jetstream::new(client);

                // The durable stream with MsgId dedup (the Nats-Msg-Id = event_id 0-ghost
                // property). A 2-minute dedup window is ample for a relay re-claim after a crash.
                js.get_or_create_stream(jetstream::stream::Config {
                    name: stream_name.clone(),
                    subjects: vec![filter.clone()],
                    duplicate_window: std::time::Duration::from_secs(120),
                    ..Default::default()
                })
                .await
                .map_err(|e| TransportError(format!("create stream: {e}")))?;

                // The durable PULL consumer with explicit ack (so consume+ack is real, not
                // auto-ack). AckExplicit is the default; pinning it is loud + intentional.
                let stream = js
                    .get_stream(&stream_name)
                    .await
                    .map_err(|e| TransportError(format!("get stream: {e}")))?;
                stream
                    .get_or_create_consumer(
                        &consumer_name,
                        jetstream::consumer::pull::Config {
                            durable_name: Some(consumer_name.clone()),
                            ack_policy: jetstream::consumer::AckPolicy::Explicit,
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(|e| TransportError(format!("create consumer: {e}")))?;

                Ok::<Context, TransportError>(js)
            })
        })?;

        Ok(NatsJetStreamBus {
            js,
            stream_name,
            subject_root,
            rt,
            pending: Mutex::new(HashMap::new()),
        })
    }

    /// The durable consumer this bus reads through (named after the stream + `_pull`).
    fn consumer_name(&self) -> String {
        format!("{}_pull", self.stream_name)
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
        let consumer_name = self.consumer_name();
        let stream_name = self.stream_name.clone();
        let msgs: Vec<jetstream::Message> = self.block(async {
            let consumer: PullConsumer = match self.js.get_stream(&stream_name).await {
                Ok(s) => match s.get_consumer(&consumer_name).await {
                    Ok(c) => c,
                    Err(_) => return Vec::new(),
                },
                Err(_) => return Vec::new(),
            };
            let mut batch = match consumer
                .batch()
                .max_messages(256)
                .expires(std::time::Duration::from_millis(500))
                .messages()
                .await
            {
                Ok(b) => b,
                Err(_) => return Vec::new(),
            };
            let mut out = Vec::new();
            while let Some(Ok(msg)) = batch.next().await {
                out.push(msg);
            }
            out
        });

        let mut envelopes = Vec::with_capacity(msgs.len());
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        for msg in msgs {
            if let Ok(env) = serde_json::from_slice::<EventEnvelope>(&msg.payload) {
                pending.insert(env.event_id.0.clone(), msg);
                envelopes.push(env);
            }
        }
        envelopes
    }

    fn ack(&self, _consumer: &str, event_id: &EventId) {
        // Explicit ack of the delivered message stashed by `consume` (at-least-once → the ack is
        // what makes it not redeliver). Acking an un-consumed / already-acked id is a no-op.
        let msg = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.remove(&event_id.0)
        };
        if let Some(msg) = msg {
            let _ = self.block(async { msg.ack().await });
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
    }
}
