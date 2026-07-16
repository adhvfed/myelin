//! # `signal_consumer` — the inbound-signal consumer wired into the P-FLOW-02 consumer slot (P-FLOW-09, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §3.4 (`wf_signal` — the
//! durably-buffered inbound signal; PK `(tenant, run_id, signal_name, idem_key)`) + §4.3 (the signal
//! round-trip — the DELIVERY side) + §4.9 (the `SCHEDULE_AND_RUN_JOB` long-park: completion arrives
//! HOURS later as a durable signal, idempotent on `idem_token`). Contract 9.1
//! (`DurableExecutor::signal` — idempotent on `idem_key`).
//!
//! ## What this module is — the bus side of `DurableExecutor::signal`
//!
//! [`DurableExecutor::signal`](crate::DurableExecutor::signal) is the engine's DELIVERY surface: a
//! caller hands it a [`SignalSpec`](crate::SignalSpec) and it buffers the signal into `wf_signal`
//! idempotently (`INSERT … ON CONFLICT (tenant, run_id, signal_name, idem_key) DO NOTHING`). The
//! callers are:
//!
//! - a **direct in-process** call (a Chat approval-card posts `approval`; the unified runner's
//!   `job.done` callback; the merge-queue's `ci.result` rollup) — they hold the
//!   [`crate::FlowExecutor`] handle and call `signal` directly; AND
//! - an **inbound bus signal** — a completion that arrives as a bus event (`flow.signal.delivered`,
//!   the §4.3 round-trip). [`FlowSignalConsumer`] is the [`EventHandler`] that consumes those events
//!   and translates them into a `DurableExecutor::signal` call. It is the registration that fills the
//!   P-FLOW-02 empty `consumers` slot for the signal leg.
//!
//! ## Idempotency-by-construction (the P-FLOW-09 gate)
//!
//! The consumer is idempotent at TWO layers, belt-and-braces:
//! 1. the seven-rule runtime's `consumer_dedup` outer guard (rule 1) — a redelivered bus message with
//!    the same `event_id` is absorbed before `handle` runs; AND
//! 2. the `wf_signal` PK INSIDE `signal` — even if a DIFFERENT bus message carries the SAME
//!    `(run, signal_name, idem_key)` (e.g. the runner retried with a fresh `event_id` after a crash),
//!    the ON CONFLICT DO NOTHING buffers it exactly once. The deterministic `idem_token` (§4.9) is
//!    what makes the producer and consumer agree on the key without coordination.
//!
//! A [`SignalOutcome::Duplicate`](crate::SignalOutcome::Duplicate) is NOT an error — it is the
//! idempotency working; the consumer acks it `Done` (the workflow already has its one buffered copy).
//!
//! ## FLOORS named
//!
//! - **The consuming wait** (`wait_for_signal`, which re-leases the parked run + replays + consumes
//!   the buffered signal, flipping `waiting → running`) is the named follow-on **P-FLOW-11**. This
//!   module ships DELIVERY only: a delivered signal lands in `wf_signal` buffered (`consumed_seq =
//!   NULL`); the wait drains it later.
//! - **The per-effect `idem_key`-CONSTRUCTION rule** (single `card_id` vs multi `card_id:<idx>` for
//!   a batch/partial HITL approval, §6.4) is **P-FLOW-10**. This module delivers under WHATEVER
//!   `idem_key` the event carries — the construction rule is the producer's, layered on top.
//! - The signal payload is references-not-payloads (`ArtifactRef`s); a malformed event (no run /
//!   no signal_name / a non-ref payload) is a non-retryable POISON (dead-lettered, rule 5 — never a
//!   silent drop, never a head-of-line stall, EI-02 §4).

use crate::executor::{DurableExecutor, SignalSpec};
use crate::{ExecutorError, RunId};
use myelin_events::{Backoff, EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_refs::ArtifactRef;

/// The FROZEN inbound-signal event type the consumer whitelists (§4.3). A completion/approval the bus
/// delivers to a parked workflow — the subject is the run's `ArtifactRef`, the payload names the
/// signal + the per-effect idem_key + the references-not-payloads body.
pub const SIGNAL_EVENT_TYPE: &str = "flow.signal.delivered";

/// **The inbound-signal consumer (the bus side of `DurableExecutor::signal`, P-FLOW-09).** An
/// [`EventHandler`] that consumes `flow.signal.delivered` events and DELIVERS them to the durable
/// executor (`signal`), which buffers them into `wf_signal` idempotently. It holds the executor as a
/// trait object (engine-agnostic, §2.9) so the production wiring boxes a [`crate::FlowExecutor`] and
/// a test can box an in-memory double.
///
/// Generic over the executor `E: DurableExecutor` so the call-site sees the concrete type (the
/// monomorphised `signal` call) while the architecture seam stays the trait.
pub struct FlowSignalConsumer<E: DurableExecutor> {
    executor: E,
    /// the `*`-free subject whitelist (rule 3) — `sig.<tenant>.…` patterns, NEVER `*`. The `'static`
    /// slice the trait requires; bound through the sanctioned `consume` at registration.
    subjects: &'static [SubjectPattern],
}

impl<E: DurableExecutor> FlowSignalConsumer<E> {
    /// Build the inbound-signal consumer over `executor` + its `*`-free subject whitelist. The
    /// whitelist is bound through the sanctioned `myelin_events::consume` at registration (rule 3
    /// rejects a `*`/empty subject loudly); this constructor only carries it.
    pub fn new(executor: E, subjects: &'static [SubjectPattern]) -> Self {
        Self { executor, subjects }
    }

    /// Parse + DELIVER one inbound-signal event into a [`SignalSpec`] and hand it to the durable
    /// executor's `signal` (the idempotent buffer). Returns the parse/delivery result so [`handle`]
    /// maps it to the seven-rule outcome.
    ///
    /// **References-not-payloads parse (§3.4):** the event payload is a JSON object
    /// `{ "signal_name": <token>, "idem_key": <key>, "payload": [<ArtifactRef>…],
    /// "payload_key_ref": <opt> }`; the run is the event SUBJECT (an `ArtifactRef` whose tail is the
    /// `RunId`). A missing/empty `signal_name` or `idem_key`, or a `payload` that is not an array of
    /// strings, is a [`DeliverError::Malformed`] poison.
    fn deliver(&self, ev: &EventEnvelope) -> Result<crate::SignalOutcome, DeliverError> {
        // The run id is the tail of the subject ArtifactRef (`myelin://<tenant>/flow/run/<run_id>`).
        let run_id = run_id_of(&ev.subject).ok_or_else(|| {
            DeliverError::Malformed(format!("no run id in subject {}", ev.subject.0))
        })?;

        let obj = ev
            .payload
            .as_object()
            .ok_or_else(|| DeliverError::Malformed("signal payload is not a JSON object".into()))?;
        let signal_name = obj
            .get("signal_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                DeliverError::Malformed("signal payload has no non-empty signal_name".into())
            })?
            .to_string();
        let idem_key = obj
            .get("idem_key")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                DeliverError::Malformed("signal payload has no non-empty idem_key".into())
            })?
            .to_string();
        // references-not-payloads: the body is an array of ArtifactRef strings (never a PII body).
        let payload: Vec<ArtifactRef> = match obj.get("payload") {
            None => Vec::new(),
            Some(serde_json::Value::Array(arr)) => {
                let mut refs = Vec::with_capacity(arr.len());
                for v in arr {
                    let s = v.as_str().ok_or_else(|| {
                        DeliverError::Malformed(
                            "signal payload body is not an array of refs".into(),
                        )
                    })?;
                    refs.push(ArtifactRef(s.to_string()));
                }
                refs
            }
            Some(_) => return Err(DeliverError::Malformed(
                "signal payload body must be an array of ArtifactRefs (references-not-payloads)"
                    .into(),
            )),
        };
        let payload_key_ref = obj
            .get("payload_key_ref")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        self.executor
            .signal(SignalSpec {
                run: RunId(run_id),
                signal_name,
                idem_key,
                payload,
                payload_key_ref,
            })
            .map_err(DeliverError::Delivery)
    }
}

/// Why an inbound-signal delivery failed — drives the seven-rule outcome. A [`Malformed`](DeliverError::Malformed)
/// event is a non-retryable POISON (dead-lettered, never a head-of-line stall); an
/// [`UnknownRun`](DeliverError::Delivery) of [`ExecutorError::UnknownRun`] is ALSO non-retryable (the
/// run does not exist — retrying will never make it appear; surfaced, never silently dropped).
#[derive(Clone, Debug, PartialEq, Eq)]
enum DeliverError {
    /// the event shape is wrong (no run / no signal_name / no idem_key / a non-ref payload) — poison.
    Malformed(String),
    /// `DurableExecutor::signal` surfaced an error (an unknown run).
    Delivery(ExecutorError),
}

impl<E: DurableExecutor + Send + Sync> EventHandler for FlowSignalConsumer<E> {
    /// The `*`-free `sig.<tenant>.…` subject whitelist (rule 3) — NEVER `*` (BUS-3). Bound through
    /// the sanctioned `consume` at registration, which rejects a `*`/empty subject loudly.
    fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
    }

    /// Deliver the inbound signal idempotently (contract 9.1, P-FLOW-09). Idempotent on `event_id`
    /// (the runtime's `consumer_dedup` outer guard, rule 1) AND on `(tenant, run_id, signal_name,
    /// idem_key)` (the `wf_signal` PK inside `signal`) — belt and braces. A [`SignalOutcome::Buffered`]
    /// or [`SignalOutcome::Duplicate`](crate::SignalOutcome) is BOTH `Done` (a duplicate is the
    /// idempotency working, not an error). A malformed event, or a signal to an unknown run, is a
    /// non-retryable POISON (dead-lettered immediately, rule 5 — never a head-of-line stall, never a
    /// silent drop, EI-02 §4).
    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        match self.deliver(ev) {
            // Buffered OR Duplicate: the workflow has its one buffered copy — ack Done either way.
            Ok(_) => HandleOutcome::Done,
            // A malformed event terminates immediately (poison, rule 5) — it never blocks the subject
            // behind it (head-of-line isolation), never a silent drop.
            Err(DeliverError::Malformed(why)) => HandleOutcome::NonRetryable(Reason(why)),
            // An unknown run is non-retryable: retrying will not make a phantom run appear. Surfaced
            // (dead-lettered) so the mis-routed signal is observable, never silently swallowed.
            Err(DeliverError::Delivery(ExecutorError::UnknownRun(r))) => {
                HandleOutcome::NonRetryable(Reason(format!("signal to unknown run {r}")))
            }
            // An UnknownWorkflow cannot arise from `signal` (it only validates the run), but the
            // exhaustive match keeps the surface honest: treat any other surfaced delivery error as a
            // transient retry (0 lost — the runtime redelivers).
            Err(DeliverError::Delivery(_)) => HandleOutcome::Retry(Backoff { seconds: 2 }),
        }
    }
}

/// Extract the `RunId` tail from a run `ArtifactRef` (`myelin://<tenant>/flow/run/<run_id>`). Returns
/// `None` if the ref has no `/`-delimited tail (a malformed subject → a poison event).
fn run_id_of(subject: &ArtifactRef) -> Option<String> {
    subject
        .0
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FlowExecutor, RunBudget, SignalOutcome, StartSpec};
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, IdMinter,
        MonotonicMinter, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::Arc;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }
    fn subjects() -> &'static [SubjectPattern] {
        Box::leak(vec![SubjectPattern("sig.acme.".into())].into_boxed_slice())
    }

    fn executor() -> FlowExecutor {
        let ex = FlowExecutor::new(minter(), tenant(), region());
        ex.register_definition("agent.run");
        ex
    }

    fn start_a_run(ex: &FlowExecutor) -> RunId {
        ex.start(StartSpec {
            wf_type: "agent.run".into(),
            input: vec![],
            budget: Some(RunBudget { minor_units: 1_000 }),
            idem_key: "k".into(),
        })
        .expect("start")
    }

    fn signal_event(run: &RunId, signal_name: &str, idem_key: &str, ev_id: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(ev_id.into()),
            type_: EventType(SIGNAL_EVENT_TYPE.into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            subject: ArtifactRef(format!("myelin://acme/flow/run/{}", run.0)),
            aggregate: AggregateKey(format!("flow/run/{}", run.0)),
            causation_id: None,
            correlation_id: CorrelationId("c".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            payload: serde_json::json!({
                "signal_name": signal_name,
                "idem_key": idem_key,
                "payload": ["myelin://acme/agent/result/r0"],
            }),
        }
    }

    /// **The consumer delivers an inbound signal into `wf_signal` (the bus side of `signal`).** A
    /// `flow.signal.delivered` event for a known run buffers the signal — the executor's signal store
    /// holds exactly one buffered row.
    #[test]
    fn consumer_delivers_an_inbound_signal_into_wf_signal() {
        let ex = executor();
        let run = start_a_run(&ex);
        let consumer = FlowSignalConsumer::new(ex.clone(), subjects());

        let ev = signal_event(&run, "job.done", "tok-1", "evt-1");
        assert_eq!(
            consumer.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done,
            "a good signal acks Done"
        );
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            1,
            "one buffered row"
        );
        assert_eq!(
            ex.telemetry().signal_buffer_depth(),
            1,
            "signal-buffer-depth reads 1"
        );
    }

    /// **A DOUBLE-delivered signal (different bus `event_id`, SAME per-effect key) buffers ONCE — the
    /// `wf_signal` PK is the dedup even past the runtime's `event_id` guard.** This is the P-FLOW-09
    /// gate at the consumer: at-least-once bus delivery + a runner that retried with a fresh event_id
    /// still wakes the workflow once.
    #[test]
    fn a_double_delivery_under_the_same_key_buffers_once() {
        let ex = executor();
        let run = start_a_run(&ex);
        let consumer = FlowSignalConsumer::new(ex.clone(), subjects());

        // two DISTINCT bus events (distinct event_id) carrying the SAME (run, signal_name, idem_key).
        let first = consumer.handle(&signal_event(&run, "job.done", "tok-1", "evt-1"), &mut myelin_events::HandlerTx::none());
        let second = consumer.handle(&signal_event(&run, "job.done", "tok-1", "evt-2"), &mut myelin_events::HandlerTx::none());
        assert_eq!(first, HandleOutcome::Done);
        assert_eq!(
            second,
            HandleOutcome::Done,
            "the duplicate is the idempotency working, not an error"
        );
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            1,
            "the wf_signal PK buffered it ONCE (the workflow wakes once) — even past the event_id guard"
        );
        assert_eq!(
            ex.telemetry().signal_buffer_depth(),
            1,
            "the signal-buffer-depth stayed 1 (truthful)"
        );
    }

    /// **A malformed signal event is a non-retryable POISON (dead-lettered, never a head-of-line
    /// stall, EI-02 §4).** A missing `signal_name` terminates immediately — it does not block the
    /// subject behind it.
    #[test]
    fn a_malformed_signal_is_non_retryable_poison() {
        let ex = executor();
        let run = start_a_run(&ex);
        let consumer = FlowSignalConsumer::new(ex.clone(), subjects());

        let mut ev = signal_event(&run, "job.done", "tok-1", "evt-1");
        ev.payload = serde_json::json!({ "idem_key": "tok-1" }); // no signal_name → poison.
        assert!(
            matches!(consumer.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::NonRetryable(_)),
            "a malformed signal is non-retryable poison (dead-lettered, no silent drop)"
        );
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            0,
            "nothing buffered for a poison event"
        );
    }

    /// **A signal to an UNKNOWN run is surfaced (dead-lettered, never a silent drop, EI-02 §4).** The
    /// `DurableExecutor::signal` `UnknownRun` error maps to a non-retryable poison — retrying will not
    /// make a phantom run appear.
    #[test]
    fn a_signal_to_an_unknown_run_is_surfaced() {
        let ex = executor();
        let consumer = FlowSignalConsumer::new(ex.clone(), subjects());
        let ev = signal_event(&RunId("no-such-run".into()), "job.done", "tok-1", "evt-1");
        assert!(
            matches!(consumer.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::NonRetryable(_)),
            "a signal to an unknown run is surfaced (dead-lettered), never silently swallowed"
        );
    }

    /// **The delivery returns `Buffered` then `Duplicate` (the outcome the control plane reads).** The
    /// direct `signal` call surfaces WHICH delivery was the first — the bus consumer acks both Done.
    #[test]
    fn delivery_reports_buffered_then_duplicate() {
        let ex = executor();
        let run = start_a_run(&ex);
        let consumer = FlowSignalConsumer::new(ex.clone(), subjects());
        let first = consumer
            .deliver(&signal_event(&run, "approval", "card-7", "e1"))
            .unwrap();
        let second = consumer
            .deliver(&signal_event(&run, "approval", "card-7", "e2"))
            .unwrap();
        assert_eq!(
            first,
            SignalOutcome::Buffered,
            "the first delivery buffered"
        );
        assert_eq!(
            second,
            SignalOutcome::Duplicate,
            "the re-delivery is a no-op duplicate"
        );
    }
}
