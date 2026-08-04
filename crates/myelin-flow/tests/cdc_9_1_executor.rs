use myelin_flow::{
    DurableExecutor as FlowDurableExecutor, FlowExecutor, RunBudget, StartSpec, PARTITION_COUNT,
};
use myelin_query::automations::{
    DurableExecutor as QueryDurableExecutor, DurableHandle, ExecutorError as QueryExecutorError,
    InMemoryExecutor, WorkflowRef,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn minter() -> Arc<dyn myelin_events::IdMinter> {
    Arc::new(myelin_events::MonotonicMinter::new())
}

struct FlowExecutorAdapter {
    inner: FlowExecutor,
}

impl QueryDurableExecutor for FlowExecutorAdapter {
    fn start(
        &self,
        workflow_ref: &WorkflowRef,
        input: &serde_json::Value,
        idem_key: &str,
    ) -> Result<DurableHandle, QueryExecutorError> {
        let input_refs: Vec<ArtifactRef> = input
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| ArtifactRef(s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let spec = StartSpec {
            wf_type: workflow_ref.0.clone(),
            input: input_refs,
            budget: Some(RunBudget {
                minor_units: 10_000,
            }),
            idem_key: idem_key.to_string(),
        };
        let run_id = self
            .inner
            .start(spec)
            .map_err(|e| QueryExecutorError(e.to_string()))?;
        Ok(DurableHandle(run_id.0))
    }
}

fn adapter() -> FlowExecutorAdapter {
    let inner = FlowExecutor::new(minter(), tenant(), region());
    inner.register_definition("agent.run");
    FlowExecutorAdapter { inner }
}

#[test]
fn provider_flow_executor_starts_describes_and_cancels() {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    let spec = StartSpec {
        wf_type: "agent.run".into(),
        input: vec![ArtifactRef("myelin://acme/git/pr/PR-1".into())],
        budget: Some(RunBudget { minor_units: 5_000 }),
        idem_key: "rule:evt-1".into(),
    };

    let r1 = ex.start(spec.clone()).expect("start");
    let r2 = ex.start(spec).expect("re-start");
    assert_eq!(
        r1, r2,
        "PROVIDER promise: start is idempotent on idem_key (one run)"
    );

    let status = ex.describe(&r1).expect("describe");
    assert_eq!(status.wf_type, "agent.run");
    assert_eq!(status.state, "running");
    assert!(!status.terminal);

    ex.cancel(&r1, "test").expect("cancel");
    assert_eq!(ex.describe(&r1).expect("describe").state, "terminated");
    assert!(ex.describe(&r1).expect("describe").terminal);
}

#[test]
fn consumer_start_through_the_adapter_is_idempotent_on_idem_key() {
    let adapter = adapter();
    let wf = WorkflowRef("agent.run".into());
    let input = serde_json::json!(["myelin://acme/git/pr/PR-1"]);
    let idem = "rule-7:evt-42";

    let h1 = QueryDurableExecutor::start(&adapter, &wf, &input, idem).expect("start");
    let h2 =
        QueryDurableExecutor::start(&adapter, &wf, &input, idem).expect("re-start (redelivery)");
    assert_eq!(
        h1, h2,
        "CONSUMER reliance: a redelivered firing returns the SAME handle (one durable run, not two)"
    );

    let total: usize = (0..PARTITION_COUNT as i16)
        .map(|p| adapter.inner.runs().runnable_lag(p, i64::MAX))
        .sum();
    assert_eq!(
        total, 1,
        "the provider seeded exactly one run (the redelivery delegated to a no-op)"
    );
}

#[test]
fn the_floor_and_the_real_engine_honour_the_same_consumer_contract() {
    let wf = WorkflowRef("agent.run".into());
    let input = serde_json::json!(["myelin://acme/git/pr/PR-1"]);

    let floor = InMemoryExecutor::new();
    let real = adapter();
    let consumers: Vec<&dyn QueryDurableExecutor> = vec![&floor, &real];
    for ex in consumers {
        let a = ex.start(&wf, &input, "k:1").expect("start a");
        let a_again = ex.start(&wf, &input, "k:1").expect("re-start a");
        assert_eq!(a, a_again, "idempotent on idem_key (both ends)");
        let b = ex.start(&wf, &input, "k:2").expect("start b");
        assert_ne!(a, b, "a distinct idem_key is a distinct handle (both ends)");
    }
}

#[test]
fn unknown_workflow_is_surfaced_through_the_consumer_seam() {
    let adapter = adapter();
    let wf = WorkflowRef("no.such.workflow".into());
    let input = serde_json::json!([]);
    let err = QueryDurableExecutor::start(&adapter, &wf, &input, "k")
        .expect_err("an unknown workflow type is surfaced, never a silent drop");
    assert!(
        err.0.contains("no.such.workflow"),
        "the consumer error names the unknown workflow (observable, retryable)"
    );
}
