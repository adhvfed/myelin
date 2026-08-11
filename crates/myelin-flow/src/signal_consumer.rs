use crate::executor::{DurableExecutor, SignalSpec};
use crate::{ExecutorError, RunId};
use myelin_events::{Backoff, EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_refs::ArtifactRef;

pub const SIGNAL_EVENT_TYPE: &str = "flow.signal.delivered";

pub struct FlowSignalConsumer<E: DurableExecutor> {
    executor: E,
    subjects: Vec<SubjectPattern>,
}

impl<E: DurableExecutor> FlowSignalConsumer<E> {
    pub fn new(executor: E, subjects: impl AsRef<[SubjectPattern]>) -> Self {
        Self {
            executor,
            subjects: subjects.as_ref().to_vec(),
        }
    }

    fn deliver(&self, ev: &EventEnvelope) -> Result<crate::SignalOutcome, DeliverError> {
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum DeliverError {
    Malformed(String),
    Delivery(ExecutorError),
}

impl<E: DurableExecutor + Send + Sync> EventHandler for FlowSignalConsumer<E> {
    fn subjects(&self) -> &[SubjectPattern] {
        &self.subjects
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        match self.deliver(ev) {
            Ok(_) => HandleOutcome::Done,
            Err(DeliverError::Malformed(why)) => HandleOutcome::NonRetryable(Reason(why)),
            Err(DeliverError::Delivery(ExecutorError::UnknownRun(r))) => {
                HandleOutcome::NonRetryable(Reason(format!("signal to unknown run {r}")))
            }
            Err(DeliverError::Delivery(_)) => HandleOutcome::Retry(Backoff { seconds: 2 }),
        }
    }
}

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
    fn subjects() -> Vec<SubjectPattern> {
        vec![SubjectPattern("sig.acme.".into())]
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
            budget: Some(RunBudget {
                minor_units: 10_000_000,
            }),
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

    #[test]
    fn a_double_delivery_under_the_same_key_buffers_once() {
        let ex = executor();
        let run = start_a_run(&ex);
        let consumer = FlowSignalConsumer::new(ex.clone(), subjects());

        let first = consumer.handle(
            &signal_event(&run, "job.done", "tok-1", "evt-1"),
            &mut myelin_events::HandlerTx::none(),
        );
        let second = consumer.handle(
            &signal_event(&run, "job.done", "tok-1", "evt-2"),
            &mut myelin_events::HandlerTx::none(),
        );
        assert_eq!(first, HandleOutcome::Done);
        assert_eq!(
            second,
            HandleOutcome::Done,
            "the duplicate is the idempotency working, not an error"
        );
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            1,
            "the wf_signal PK buffered it ONCE (the workflow wakes once) - even past the event_id guard"
        );
        assert_eq!(
            ex.telemetry().signal_buffer_depth(),
            1,
            "the signal-buffer-depth stayed 1 (truthful)"
        );
    }

    #[test]
    fn a_malformed_signal_is_non_retryable_poison() {
        let ex = executor();
        let run = start_a_run(&ex);
        let consumer = FlowSignalConsumer::new(ex.clone(), subjects());

        let mut ev = signal_event(&run, "job.done", "tok-1", "evt-1");
        ev.payload = serde_json::json!({ "idem_key": "tok-1" });
        assert!(
            matches!(
                consumer.handle(&ev, &mut myelin_events::HandlerTx::none()),
                HandleOutcome::NonRetryable(_)
            ),
            "a malformed signal is non-retryable poison (dead-lettered, no silent drop)"
        );
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            0,
            "nothing buffered for a poison event"
        );
    }

    #[test]
    fn a_signal_to_an_unknown_run_is_surfaced() {
        let ex = executor();
        let consumer = FlowSignalConsumer::new(ex.clone(), subjects());
        let ev = signal_event(&RunId("no-such-run".into()), "job.done", "tok-1", "evt-1");
        assert!(
            matches!(
                consumer.handle(&ev, &mut myelin_events::HandlerTx::none()),
                HandleOutcome::NonRetryable(_)
            ),
            "a signal to an unknown run is surfaced (dead-lettered), never silently swallowed"
        );
    }

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
