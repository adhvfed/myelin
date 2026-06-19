//! `NatsJetStreamBus` — the [`BusTransport`](crate::relay::BusTransport) trait backed by a REAL
//! NATS JetStream (Apache-2.0) durable bus via `async-nats`.
//!
//! **Stage 2 / infra.** This is the durable backing for the bus the [`crate::relay`] module's
//! [`crate::relay::InProcessBus`] floor models in process. It implements the EXACT frozen
//! `put/consume/ack/purge` shape behind the existing [`BusTransport`] trait — it does NOT fork
//! or redefine the trait (EI-01 §7 coherence). The in-process fake remains the unit/default
//! transport; this real backing is config-selected and compiled ONLY under `--features
//! integration`.
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
use async_nats::jetstream::{self, Context};
use futures::StreamExt;

use crate::relay::{BusTransport, Delivery, TransportError};
use crate::{ArtifactRef, EventEnvelope, EventId};

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

    /// Map a subject ref onto a concrete JetStream subject under the stream's root. The relay's
    /// `subject` is an opaque [`ArtifactRef`] string; we slot it under `subject_root` so the
    /// stream's `<root>.>` filter captures it (and so a smoke run's subjects are namespaced).
    fn subject_for(&self, subject: &ArtifactRef) -> String {
        // NATS subject tokens are dot-delimited; a ref may contain characters NATS dislikes, so
        // we hash-free sanitize to a single token (the dedup id, not the subject, is what
        // carries identity — the subject only needs to be a stable, filter-matched token).
        let token: String = subject
            .0
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("{}.{}", self.subject_root, token)
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl BusTransport for NatsJetStreamBus {
    fn put(
        &self,
        subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> Result<Delivery, TransportError> {
        let nats_subject = self.subject_for(subject);
        let body = serde_json::to_vec(envelope)
            .map_err(|e| TransportError(format!("serialize envelope: {e}")))?;

        let mut headers = async_nats::HeaderMap::new();
        // Nats-Msg-Id = the stable event_id → broker-side dedup (0 ghost): a re-publish of the
        // same event_id is suppressed and flagged `duplicate` on the ack.
        headers.insert("Nats-Msg-Id", dedup_id.0.as_str());

        let ack = self
            .block(async {
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
            let consumer: PullConsumer = match self
                .js
                .get_stream(&stream_name)
                .await
            {
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
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}
