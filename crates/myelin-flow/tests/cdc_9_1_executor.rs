//! # The CDC pair for the `DurableExecutor` control surface — contract 9.1 (start/describe/cancel)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 9.1
//! (`DurableExecutor{start, signal, describe, cancel}` — engine-agnostic; `start` returns a durable
//! handle; idempotent on `idem_key`; the `signal` method is the named follow-on P-FLOW-09). Owning
//! architecture: `durable-workflow.md` §5.1 (the `DurableExecutor` trait + `StartSpec`), §5.3 (who
//! consumes it: bus automations `action.kind = workflow`, Agent, CI, Issues).
//!
//! ## What this pair pins (the PROVIDER ↔ CONSUMER agreement of 9.1's control half)
//!
//! **9.1 PROVIDER (the `myelin-flow` [`myelin_flow::FlowExecutor`]) — what the engine guarantees:**
//! - `start(StartSpec{wf_type, input, budget, idem_key}) → RunId`, **idempotent on `idem_key`** (a
//!   redelivered start is ONE run); `input` is references-not-payloads.
//! - `describe(RunId) → RunStatus` (lifecycle + cursor + pinned version + terminality);
//! - `cancel(RunId, reason)` transitions a non-terminal run to `terminated`.
//!
//! **9.1 CONSUMER (the Automation engine, [`myelin_query::DurableExecutor`]) — what it relies on:**
//! - `action.kind = workflow` is a SINGLE `start(workflow_ref, input, idem_key) → DurableHandle`
//!   call (the engine NEVER reinvents the durable loop, ADR-09); a redelivered trigger that fires
//!   the same rule produces the SAME `idem_key`, so the executor returns the SAME handle (a
//!   double-delivery is one workflow run, not two).
//!
//! The two ends have DIFFERENT trait shapes by design: the consumer's seam
//! ([`myelin_query::DurableExecutor`]) is the MINIMAL `start` the Automation engine needs; the
//! provider's surface ([`myelin_flow::DurableExecutor`]) is the FULL §5.1 control surface. This pair
//! proves they RECONCILE: an adapter that delegates the consumer's `start` to the provider's
//! [`FlowExecutor::start`] is idempotent-on-`idem_key` exactly as the consumer relies on — the
//! delegation, not a reinvention. This is the §2.9 DAG-respecting seam (`myelin-query` is UPSTREAM
//! of `myelin-flow`; the production code depends on the trait, never the concrete engine; the CDC
//! test depends on BOTH to pin the agreement).

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

/// **The PROVIDER↔CONSUMER ADAPTER** — implements the CONSUMER's seam
/// ([`myelin_query::DurableExecutor`]) by DELEGATING to the PROVIDER ([`myelin_flow::FlowExecutor`]).
/// This is the real wiring at the §2.9 boundary: the Automation engine's `action.kind = workflow`
/// dispatch holds a `Box<dyn myelin_query::DurableExecutor>`; in production that box is THIS adapter
/// over `myelin-flow`. The adapter translates the consumer's minimal `start(workflow_ref, input,
/// idem_key)` into the provider's `StartSpec` and maps the `RunId` back to a `DurableHandle`.
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
        // The consumer's opaque `workflow_ref` is the provider's registered `wf_type`; the
        // references-not-payloads input is the JSON array of `ArtifactRef`s (the engine carries
        // refs, never bodies). Translate, delegate, map the RunId → the consumer's DurableHandle.
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
            budget: Some(RunBudget { minor_units: 10_000 }),
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

/// **PROVIDER side of 9.1: the `myelin-flow` executor starts a run idempotently on `idem_key` +
/// describes/cancels it.** The provider's full §5.1 control surface — the agreement the consumer's
/// `start` reconciles against.
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

    // start is idempotent on idem_key (a re-start returns the SAME run).
    let r1 = ex.start(spec.clone()).expect("start");
    let r2 = ex.start(spec).expect("re-start");
    assert_eq!(r1, r2, "PROVIDER promise: start is idempotent on idem_key (one run)");

    // describe returns the run's status.
    let status = ex.describe(&r1).expect("describe");
    assert_eq!(status.wf_type, "agent.run");
    assert_eq!(status.state, "running");
    assert!(!status.terminal);

    // cancel transitions it to terminated.
    ex.cancel(&r1, "test").expect("cancel");
    assert_eq!(ex.describe(&r1).expect("describe").state, "terminated");
    assert!(ex.describe(&r1).expect("describe").terminal);
}

/// **CONSUMER side of 9.1: the Automation engine's `start` seam, satisfied by the PROVIDER through
/// the adapter, is idempotent-on-`idem_key` EXACTLY as the consumer relies on.** A redelivered
/// trigger that fires the same rule (the SAME `<rule_id>:<event_id>` idem_key) returns the SAME
/// [`DurableHandle`] — the delegation is effectively-once, never a second run.
#[test]
fn consumer_start_through_the_adapter_is_idempotent_on_idem_key() {
    let adapter = adapter();
    let wf = WorkflowRef("agent.run".into());
    let input = serde_json::json!(["myelin://acme/git/pr/PR-1"]);
    let idem = "rule-7:evt-42";

    let h1 = QueryDurableExecutor::start(&adapter, &wf, &input, idem).expect("start");
    let h2 = QueryDurableExecutor::start(&adapter, &wf, &input, idem).expect("re-start (redelivery)");
    assert_eq!(
        h1, h2,
        "CONSUMER reliance: a redelivered firing returns the SAME handle (one durable run, not two)"
    );

    // exactly ONE run was seeded on the provider (the redelivery was a no-op).
    let total: usize = (0..PARTITION_COUNT as i16)
        .map(|p| adapter.inner.runs().runnable_lag(p, i64::MAX))
        .sum();
    assert_eq!(total, 1, "the provider seeded exactly one run (the redelivery delegated to a no-op)");
}

/// **The two seam shapes RECONCILE — the consumer's `InMemoryExecutor` floor and the provider-backed
/// adapter satisfy the SAME consumer contract (start idempotent, distinct handles for distinct
/// keys).** This pins that swapping the floor for the real engine is a config change, never a code
/// change at the call-site: both honour the consumer's frozen `start` semantics.
#[test]
fn the_floor_and_the_real_engine_honour_the_same_consumer_contract() {
    let wf = WorkflowRef("agent.run".into());
    let input = serde_json::json!(["myelin://acme/git/pr/PR-1"]);

    // run the SAME assertion suite against BOTH the consumer's in-memory floor AND the real
    // myelin-flow-backed adapter, via the consumer trait object (the call-site sees only the trait).
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

/// **A `start` of an unknown workflow is SURFACED through the adapter (never a silent dropped run,
/// EI-02 §4).** The provider's [`ExecutorError::UnknownWorkflow`] maps to the consumer's
/// [`QueryExecutorError`] — the reflex whose workflow could not start is observable.
#[test]
fn unknown_workflow_is_surfaced_through_the_consumer_seam() {
    let adapter = adapter(); // only "agent.run" is registered.
    let wf = WorkflowRef("no.such.workflow".into());
    let input = serde_json::json!([]);
    let err = QueryDurableExecutor::start(&adapter, &wf, &input, "k")
        .expect_err("an unknown workflow type is surfaced, never a silent drop");
    assert!(
        err.0.contains("no.such.workflow"),
        "the consumer error names the unknown workflow (observable, retryable)"
    );
}
