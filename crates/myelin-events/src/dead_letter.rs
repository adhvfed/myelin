use crate::consumer::DeadLetter;
use crate::ConsumerName;
use std::sync::{Arc, Mutex};

pub const CONSUMER_DEAD_LETTER_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS consumer_dead_letter (
    consumer    TEXT        NOT NULL,
    event_id    TEXT        NOT NULL,
    reason      TEXT        NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT consumer_dead_letter_pk PRIMARY KEY (consumer, event_id)
);";

pub const MAX_REASON_LEN: usize = 512;

pub fn bounded_reason(reason: &str) -> String {
    if reason.len() <= MAX_REASON_LEN {
        return reason.to_string();
    }
    let mut end = MAX_REASON_LEN;
    while end > 0 && !reason.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated {} bytes]", &reason[..end], reason.len())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadLetterRecord {
    pub consumer: ConsumerName,
    pub event_id: crate::EventId,
    pub reason: String,
}

pub trait DurableDeadLetter: Send + Sync {
    fn record(
        &self,
        consumer: &ConsumerName,
        event_id: &crate::EventId,
        reason: &str,
    ) -> Result<(), String>;

    fn dead_letters(&self, consumer: &ConsumerName) -> Vec<DeadLetterRecord>;
}

pub enum DeadLetterSink {
    InMemory(Mutex<Vec<DeadLetter>>),
    Durable {
        backing: Arc<dyn DurableDeadLetter>,
        fallback: Mutex<Vec<DeadLetter>>,
    },
}

impl Default for DeadLetterSink {
    fn default() -> Self {
        DeadLetterSink::InMemory(Mutex::new(Vec::new()))
    }
}

impl DeadLetterSink {
    pub fn in_memory() -> Self {
        DeadLetterSink::default()
    }

    pub fn durable(backing: Arc<dyn DurableDeadLetter>) -> Self {
        DeadLetterSink::Durable {
            backing,
            fallback: Mutex::new(Vec::new()),
        }
    }

    pub fn push(&self, consumer: &ConsumerName, dead: DeadLetter) -> Result<(), String> {
        match self {
            DeadLetterSink::InMemory(v) => {
                v.lock().unwrap_or_else(|e| e.into_inner()).push(dead);
                Ok(())
            }
            DeadLetterSink::Durable { backing, fallback } => {
                let reason = bounded_reason(&dead.reason.0);
                match backing.record(consumer, &dead.envelope.event_id, &reason) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        eprintln!(
                            "[consumer-dlq] LOUD: durable dead-letter record FAILED for \
                             consumer={} event_id={} - retaining an in-memory operator copy and \
                             WITHHOLDING broker ack: {e}",
                            consumer.0, dead.envelope.event_id.0
                        );
                        fallback
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(dead);
                        Err(e)
                    }
                }
            }
        }
    }

    pub fn surfaced(&self) -> Vec<DeadLetter> {
        match self {
            DeadLetterSink::InMemory(v) => v.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            DeadLetterSink::Durable { fallback, .. } => {
                fallback.lock().unwrap_or_else(|e| e.into_inner()).clone()
            }
        }
    }

    pub fn durable_dead_letters(&self, consumer: &ConsumerName) -> Vec<DeadLetterRecord> {
        match self {
            DeadLetterSink::InMemory(_) => Vec::new(),
            DeadLetterSink::Durable { backing, .. } => backing.dead_letters(consumer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consumer::DeadLetter;
    use crate::{
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId,
        EventType, Reason, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn envelope(id: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType("issues.issue.created".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-07-17T00:00:00Z".into()),
            recorded_at: Timestamp("2026-07-17T00:00:01Z".into()),
            payload: serde_json::json!({ "ref": "x" }),
        }
    }

    fn dead(id: &str, reason: &str) -> DeadLetter {
        DeadLetter {
            envelope: envelope(id),
            reason: Reason(reason.into()),
        }
    }

    fn consumer(name: &str) -> ConsumerName {
        ConsumerName(name.into())
    }

    #[test]
    fn in_memory_sink_appends_and_surfaces_full_dead_letters() {
        let sink = DeadLetterSink::in_memory();
        let c = consumer("indexer");
        assert!(sink.surfaced().is_empty(), "fresh sink is empty");

        sink.push(&c, dead("01J-a", "malformed")).unwrap();
        sink.push(&c, dead("01J-b", "poison")).unwrap();
        let surfaced = sink.surfaced();
        assert_eq!(surfaced.len(), 2, "both poison records surfaced in-process");
        assert_eq!(surfaced[0].envelope.event_id, EventId("01J-a".into()));
        assert_eq!(surfaced[0].reason, Reason("malformed".into()));
        assert_eq!(surfaced[1].envelope.event_id, EventId("01J-b".into()));
        assert!(
            sink.durable_dead_letters(&c).is_empty(),
            "the in-memory sink has NO durable table"
        );
    }

    #[derive(Default)]
    struct MockDurable {
        rows: Mutex<Vec<DeadLetterRecord>>,
        record_calls: AtomicU32,
        fail: bool,
    }
    impl DurableDeadLetter for MockDurable {
        fn record(
            &self,
            consumer: &ConsumerName,
            event_id: &crate::EventId,
            reason: &str,
        ) -> Result<(), String> {
            self.record_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err("DB unreachable (mock)".into());
            }
            let mut rows = self.rows.lock().unwrap();
            let present = rows
                .iter()
                .any(|r| r.consumer == *consumer && r.event_id == *event_id);
            if !present {
                rows.push(DeadLetterRecord {
                    consumer: consumer.clone(),
                    event_id: event_id.clone(),
                    reason: reason.to_string(),
                });
            }
            Ok(())
        }
        fn dead_letters(&self, consumer: &ConsumerName) -> Vec<DeadLetterRecord> {
            self.rows
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.consumer == *consumer)
                .cloned()
                .collect()
        }
    }

    #[test]
    fn durable_sink_records_pii_free_and_is_idempotent() {
        let mock = Arc::new(MockDurable::default());
        let sink = DeadLetterSink::durable(mock.clone());
        let c = consumer("indexer");

        sink.push(&c, dead("01J-a", "malformed")).unwrap();
        sink.push(&c, dead("01J-a", "malformed")).unwrap();
        assert_eq!(
            mock.record_calls.load(Ordering::SeqCst),
            2,
            "record was called twice (both deliveries)"
        );
        let rows = sink.durable_dead_letters(&c);
        assert_eq!(rows.len(), 1, "ON CONFLICT DO NOTHING → exactly ONE row");
        assert_eq!(rows[0].event_id, EventId("01J-a".into()));
        assert_eq!(rows[0].reason, "malformed");
        assert!(
            sink.surfaced().is_empty(),
            "no fail-fallback occurred → the in-process surface is empty on the durable arm"
        );
    }

    #[test]
    fn durable_record_failure_falls_back_to_in_memory_never_silently_dropped() {
        let mock = Arc::new(MockDurable {
            fail: true,
            ..Default::default()
        });
        let sink = DeadLetterSink::durable(mock.clone());
        let c = consumer("indexer");

        assert_eq!(
            sink.push(&c, dead("01J-poison", "handler PANICKED")),
            Err("DB unreachable (mock)".into()),
            "durable failure must be non-terminal to the caller"
        );
        assert_eq!(mock.record_calls.load(Ordering::SeqCst), 1, "record tried");
        assert!(
            mock.dead_letters(&c).is_empty(),
            "the durable table got NOTHING (DB was unreachable)"
        );
        let fallback = sink.surfaced();
        assert_eq!(
            fallback.len(),
            1,
            "the poison fell back to in-memory - NOT silently dropped"
        );
        assert_eq!(fallback[0].envelope.event_id, EventId("01J-poison".into()));
    }

    #[test]
    fn bounded_reason_caps_a_long_reason_at_a_char_boundary() {
        let short = "malformed";
        assert_eq!(bounded_reason(short), short, "a short reason is unchanged");

        let long = "é".repeat(MAX_REASON_LEN);
        let bounded = bounded_reason(&long);
        assert!(
            bounded.len() <= MAX_REASON_LEN + 40,
            "the bounded reason is capped (+ the truncation tag)"
        );
        assert!(bounded.contains("truncated"), "clipping is tagged for ops");
        assert!(
            std::str::from_utf8(bounded.as_bytes()).is_ok(),
            "never split a multi-byte code point"
        );
    }

    #[test]
    fn migration_is_the_frozen_pii_free_shape() {
        assert!(CONSUMER_DEAD_LETTER_MIGRATION
            .contains("CREATE TABLE IF NOT EXISTS consumer_dead_letter"));
        assert!(CONSUMER_DEAD_LETTER_MIGRATION.contains("PRIMARY KEY (consumer, event_id)"));
        for col in ["consumer", "event_id", "reason", "occurred_at"] {
            assert!(
                CONSUMER_DEAD_LETTER_MIGRATION.contains(col),
                "missing column {col}"
            );
        }
        assert!(
            !CONSUMER_DEAD_LETTER_MIGRATION.contains("payload"),
            "PII-safety: the durable dead-letter table stores NO raw payload"
        );
        assert!(
            !CONSUMER_DEAD_LETTER_MIGRATION.contains("envelope"),
            "PII-safety: the durable dead-letter table stores NO raw envelope"
        );
        assert!(
            !CONSUMER_DEAD_LETTER_MIGRATION.contains("DROP TABLE"),
            "forward-only: no destructive down"
        );
    }
}
