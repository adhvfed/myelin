//! # The `consumer_dead_letter` set — a DURABLE consumer DLQ (CT-004d.2 chunk 6 / peer-review #7b)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §5 (rule 5 — terminate poison
//! immediately) + the SUB-D2 idempotent-consumer runtime ([`crate::consumer`]).
//!
//! ## The gap this closes (a debt the #7 H2 fix introduced)
//! The consumer's dead-letter set was `Mutex<Vec<DeadLetter>>` on [`crate::consumer::Consumer`] —
//! **in-memory only**. The H2 panic path ([`crate::consumer::Consumer::deliver`]) rolls back the
//! co-commit tx and `push_dead_letter`s the poison, then the pump ACKS the message (terminal) so the
//! broker cursor advances. But the record lived only in a volatile `Vec`: a restart LOST it — the
//! "stays replayable from the dead-letter set" comment was self-defeating as shipped (the Vec
//! vanishes with the process). The outbox RELAY already had durable dead-lettering; the CONSUMER side
//! did not. This module is the durable seam that closes it, mirroring the [`crate::dedup`]
//! `DurableDedup` pattern EXACTLY (a frozen table + a SYNC trait + an `InMemory`/`Durable` sink enum
//! + a PG backing in `myelin-storage`).
//!
//! ## PII-SAFETY (load-bearing) — references-not-payloads
//! The frozen event architecture is PII-careful (pseudonyms, crypto-shred; `envelope.payload` may
//! carry inline PII). The durable dead-letter table therefore stores **references, not payloads**:
//! the `event_id` (a globally-unique ULID, a telemetry/trace label — never personal data) + a
//! **PII-free `reason`** string + the audit `occurred_at`. It NEVER stores the raw envelope /
//! `payload`. The `reason` is a diagnostic label authored at the [`crate::Reason`] construction sites
//! (`"malformed"`, `"subject … not on consumer whitelist"`, `"unbridgeable schema gap"`, …); the ONE
//! wildcard is the H2 panic path, which interpolates the panic payload (`{detail}`) and could — in
//! principle — echo event content. We therefore **bound** the stored reason ([`bounded_reason`],
//! [`MAX_REASON_LEN`] bytes) so a panic message can never persist an unbounded blob, and we store
//! only the bounded reason + `event_id`, never the envelope. The in-memory arm is unchanged (it holds
//! the full [`DeadLetter`] for the existing surfaced-in-process view); only the DURABLE row is the
//! PII-free projection.
//!
//! ## Fail-direction (never a silent loss of the poison)
//! A DB-unreachable [`DurableDeadLetter::record`] MUST NOT silently drop the poison. It returns
//! `Err(loud)`; [`DeadLetterSink::push`] then logs LOUDLY and falls back to an in-process Vec (the
//! `Durable { fallback }` arm) so the record is still surfaced for THIS process — never a silent
//! drop. This mirrors [`DurableDedup`](crate::DurableDedup)'s fail-direction discipline (degrade, but
//! never lose).

use crate::consumer::DeadLetter;
use crate::ConsumerName;
use std::sync::{Arc, Mutex};

/// The frozen forward-only DDL for the `consumer_dead_letter` set (CT-004d.2 chunk 6 / #7b). Beside
/// the frozen [`crate::CONSUMER_DEDUP_MIGRATION`]; applied in the FOUNDATION migration set (the same
/// place `consumer_dedup` is applied — `myelin_storage::foundation_migrations` + the substrate boot).
///
/// - `(consumer, event_id)` is the **PRIMARY KEY** — a redelivered dead-letter re-inserts
///   `ON CONFLICT DO NOTHING` (idempotent: one row per poisoned `(consumer, event_id)`);
/// - `reason` is the **PII-free** bounded diagnostic (references-not-payloads: never the envelope /
///   `payload`, which may carry inline PII — see the module docs' PII-safety note);
/// - `occurred_at` is when the consumer durably recorded the poison (audit).
///
/// **Forward-only** (the `forward-only-migration` lint, P-S11): an `expand` migration (adds the table
/// only); no destructive down-migration.
pub const CONSUMER_DEAD_LETTER_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS consumer_dead_letter (
    consumer    TEXT        NOT NULL,
    event_id    TEXT        NOT NULL,
    reason      TEXT        NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT consumer_dead_letter_pk PRIMARY KEY (consumer, event_id)
);";

/// The cap (bytes) the durable `reason` is bounded to before persistence. PII-safety: the H2 panic
/// path interpolates the panic payload into the reason, which could echo event content — a bound
/// guarantees a panic message can never persist an unbounded blob. Diagnostic reasons are far shorter
/// than this in practice; the cap is defense-in-depth, not a normal truncation.
pub const MAX_REASON_LEN: usize = 512;

/// Bound a `reason` to [`MAX_REASON_LEN`] bytes at a UTF-8 char boundary (PII-safety: keep the durable
/// reason a bounded diagnostic, never an unbounded blob). A truncated reason is tagged so ops know it
/// was clipped.
pub fn bounded_reason(reason: &str) -> String {
    if reason.len() <= MAX_REASON_LEN {
        return reason.to_string();
    }
    // Find the largest char boundary <= MAX_REASON_LEN so we never split a multi-byte code point.
    let mut end = MAX_REASON_LEN;
    while end > 0 && !reason.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated {} bytes]", &reason[..end], reason.len())
}

/// A PII-free durable dead-letter row (the ops read/snapshot shape). Holds ONLY references: the
/// consumer, the `event_id` (a ULID / trace label), and the bounded PII-free `reason` — never the
/// envelope / payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadLetterRecord {
    /// The durable consumer name that poisoned the event (rule 4 label).
    pub consumer: ConsumerName,
    /// The poisoned event's id (a globally-unique ULID — a telemetry/trace label, never PII).
    pub event_id: crate::EventId,
    /// The PII-free bounded diagnostic reason (references-not-payloads).
    pub reason: String,
}

/// **The durable backing seam for the consumer dead-letter set (CT-004d.2 chunk 6 / #7b).** A real
/// `(consumer, event_id)` PK table over the OLTP pool implements this so a dead-lettered event —
/// especially the H2 panic path — **survives a process restart** (the whole point: the poison is
/// still present after the pump acked it and the broker cursor advanced). The verbs mirror
/// [`DurableDedup`](crate::DurableDedup): an `INSERT … ON CONFLICT DO NOTHING` write ([`record`]) + a
/// read/snapshot for ops ([`dead_letters`]). The trait is SYNC to match the consumer runtime's sync
/// `push_dead_letter` call site; a PG impl bridges to async internally (`block_in_place` +
/// `block_on`, the same bridge `DurableDedup` uses — the production impl is
/// `myelin_storage::events_durable::DurableDeadLetterBacking`).
///
/// **The H2 close (correctness point):** on the panic path the co-commit tx was ROLLED BACK — you
/// cannot reuse it. [`record`] MUST acquire a FRESH pool connection (not the rolled-back co-commit
/// conn) so the poison persists on its own connection. The PG impl runs against `&self.pool`
/// (a fresh connection), never the handler's tx.
///
/// **Fail-direction (never a silent loss):** [`record`] returns `Err` when it cannot reach the DB;
/// [`DeadLetterSink::push`] logs loudly, keeps an in-process operator copy, and returns `Err` so
/// the broker delivery remains unacked — never a silent drop or a cursor advance without durable
/// quarantine.
///
/// [`record`]: DurableDeadLetter::record
/// [`dead_letters`]: DurableDeadLetter::dead_letters
pub trait DurableDeadLetter: Send + Sync {
    /// `INSERT (consumer, event_id, reason) ON CONFLICT (consumer, event_id) DO NOTHING`. Idempotent:
    /// a redelivered dead-letter re-inserts a no-op. Returns `Err(loud)` on a DB error so the sink can
    /// fall back to in-memory (never a silent drop of the poison). `reason` is already bounded /
    /// PII-free by [`DeadLetterSink::push`].
    fn record(
        &self,
        consumer: &ConsumerName,
        event_id: &crate::EventId,
        reason: &str,
    ) -> Result<(), String>;

    /// The `(consumer)`-scoped durable snapshot (ops introspection). PII-free rows.
    fn dead_letters(&self, consumer: &ConsumerName) -> Vec<DeadLetterRecord>;
}

/// **The consumer dead-letter SINK (CT-004d.2 chunk 6 / #7b — the MR-023 backend-enum pattern,
/// mirroring [`crate::DedupLedger`]'s `DedupBackend`).** `InMemory` is the always-compiled DEFAULT
/// (backward-compatible: the existing unit/drill suites keep their surfaced-in-process `Vec` behavior
/// unchanged); `Durable` is the opt-in production seam the events `serve()` composition root
/// (`myelin_storage::events_serve`) wires so a service that opts in gets a **restart-surviving**
/// consumer DLQ.
///
/// Unlike `DedupBackend::Memory` (a test-support-gated double), the `InMemory` arm here is the
/// PRODUCTION DEFAULT and stays always-compiled — a `Consumer` built without a durable backing still
/// surfaces poison in-process exactly as before. (The `no-in-memory-durable-store` scanner does not
/// fire: [`crate::consumer::Consumer`] is not a durable role-suffix store, and this enum is a sink,
/// not a system-of-record ledger — the durable arm IS the restart-surviving system of record.)
pub enum DeadLetterSink {
    /// The in-process surfaced dead-letter list (rule 5) — the unchanged default behavior.
    InMemory(Mutex<Vec<DeadLetter>>),
    /// The durable PG-backed seam (opt-in). Carries a `fallback` Vec used ONLY when a `record` cannot
    /// reach the DB (the fail-direction: loud log + in-process fallback, never a silent drop).
    Durable {
        /// The restart-surviving `(consumer, event_id)` table backing.
        backing: Arc<dyn DurableDeadLetter>,
        /// Fail-direction fallback: poison the durable `record` could not persist (DB unreachable) is
        /// pushed HERE so it is still surfaced in-process — never silently lost.
        fallback: Mutex<Vec<DeadLetter>>,
    },
}

impl Default for DeadLetterSink {
    /// The default sink is the always-compiled in-memory list (the backward-compatible default).
    fn default() -> Self {
        DeadLetterSink::InMemory(Mutex::new(Vec::new()))
    }
}

impl DeadLetterSink {
    /// A fresh, empty IN-MEMORY sink (the default — the existing surfaced-in-process behavior).
    pub fn in_memory() -> Self {
        DeadLetterSink::default()
    }

    /// **Bind the sink to a DURABLE backing** so a dead-lettered event survives a process restart
    /// (CT-004d.2 chunk 6 / #7b). The events `serve()` composition root
    /// (`myelin_storage::events_serve::EventsRuntime`) constructs this with the PG-backed
    /// `consumer_dead_letter` table; [`DeadLetterSink::in_memory`] stays the default.
    pub fn durable(backing: Arc<dyn DurableDeadLetter>) -> Self {
        DeadLetterSink::Durable {
            backing,
            fallback: Mutex::new(Vec::new()),
        }
    }

    /// **Write a poison through the sink (rule 5).** In-memory: append the full [`DeadLetter`]
    /// (unchanged behavior). Durable: record the PII-free `(consumer, event_id, bounded_reason)` on a
    /// FRESH pool connection (the H2 panic path's co-commit tx was rolled back — the durable backing
    /// acquires its own connection). Fail-direction: on a DB error, log LOUDLY, retain the record
    /// in the process-local fallback for operator visibility, and return `Err`. The consumer MUST
    /// then treat the delivery as retryable so the broker cursor cannot advance past a poison whose
    /// durable quarantine record did not commit.
    pub fn push(&self, consumer: &ConsumerName, dead: DeadLetter) -> Result<(), String> {
        match self {
            DeadLetterSink::InMemory(v) => {
                v.lock().unwrap_or_else(|e| e.into_inner()).push(dead);
                Ok(())
            }
            DeadLetterSink::Durable { backing, fallback } => {
                // PII-safety: store references-not-payloads — the event_id + a BOUNDED PII-free
                // reason, never the envelope/payload.
                let reason = bounded_reason(&dead.reason.0);
                match backing.record(consumer, &dead.envelope.event_id, &reason) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        // Preserve a surfaced process-local copy, but do not report terminal
                        // success: the caller must withhold the broker ack until durable storage
                        // recovers and this write succeeds.
                        eprintln!(
                            "[consumer-dlq] LOUD: durable dead-letter record FAILED for \
                             consumer={} event_id={} — retaining an in-memory operator copy and \
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

    /// The in-process SURFACED dead-letters — the existing [`crate::consumer::Consumer::dead_letters`]
    /// view. On the in-memory sink this is the full list; on the durable sink it is the fail-fallback
    /// list ONLY (the durable rows are read via [`DeadLetterSink::durable_dead_letters`], mirroring
    /// [`crate::DedupLedger::len`] returning the in-process view on the durable backend). It carries
    /// the full [`DeadLetter`] (with envelope) because it is the in-process surface, not the PII-free
    /// durable projection.
    pub fn surfaced(&self) -> Vec<DeadLetter> {
        match self {
            DeadLetterSink::InMemory(v) => v.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            DeadLetterSink::Durable { fallback, .. } => {
                fallback.lock().unwrap_or_else(|e| e.into_inner()).clone()
            }
        }
    }

    /// The DURABLE `(consumer)`-scoped snapshot for OPS (PII-free rows). Empty on the in-memory sink
    /// (there is no durable table); on the durable sink it queries the restart-surviving table.
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

    /// The in-memory sink arm behaves EXACTLY as the old bare `Mutex<Vec<DeadLetter>>`: `push`
    /// appends the full DeadLetter, `surfaced` returns them in order, `durable_dead_letters` is empty
    /// (there is no durable table). Backward compatibility for the existing suites.
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

    /// A mock [`DurableDeadLetter`] that records `(consumer, event_id, reason)` idempotently (an
    /// `ON CONFLICT DO NOTHING` model) so the sink's durable arm is exercised without a live PG.
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
            // ON CONFLICT (consumer, event_id) DO NOTHING — a re-record of the same pair is a no-op.
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

    /// The durable sink arm records PII-free rows and is idempotent on a re-record (the `ON CONFLICT
    /// DO NOTHING` property): a redelivered dead-letter re-inserts a no-op, so exactly one row per
    /// `(consumer, event_id)`. The stored row carries only `event_id` + `reason` — never the envelope.
    #[test]
    fn durable_sink_records_pii_free_and_is_idempotent() {
        let mock = Arc::new(MockDurable::default());
        let sink = DeadLetterSink::durable(mock.clone());
        let c = consumer("indexer");

        sink.push(&c, dead("01J-a", "malformed")).unwrap();
        // A REDELIVERED dead-letter (same consumer+event_id) re-records — idempotent no-op row-wise.
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
        // PII-safety: the durable projection carries no envelope/payload — only references.
        // (Structurally guaranteed by DeadLetterRecord's fields: consumer, event_id, reason.)
        assert!(
            sink.surfaced().is_empty(),
            "no fail-fallback occurred → the in-process surface is empty on the durable arm"
        );
    }

    /// **Fail-direction:** a DB-unreachable `record` does NOT silently drop the poison — it logs,
    /// retains an operator copy, and returns `Err` so callers withhold the broker ack.
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
            "the poison fell back to in-memory — NOT silently dropped"
        );
        assert_eq!(fallback[0].envelope.event_id, EventId("01J-poison".into()));
    }

    /// PII-safety of the bound: a `reason` longer than [`MAX_REASON_LEN`] is truncated at a UTF-8
    /// boundary and tagged, so a panic message that echoes event content cannot persist unbounded.
    #[test]
    fn bounded_reason_caps_a_long_reason_at_a_char_boundary() {
        let short = "malformed";
        assert_eq!(bounded_reason(short), short, "a short reason is unchanged");

        // A multi-byte payload longer than the cap: bounded, tagged, and a valid UTF-8 string.
        let long = "é".repeat(MAX_REASON_LEN); // 2 bytes each → well over the byte cap.
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

    /// The frozen `consumer_dead_letter` migration shape: the `(consumer, event_id)` PK + the columns
    /// are present; forward-only (no destructive DROP); and — PII-safety — it has NO payload/envelope
    /// column (references-not-payloads).
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
        // PII-safety: references-not-payloads — no raw envelope/payload column.
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
