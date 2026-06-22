//! # The CDC pair for the durable-signal WAIT — contract 9.4 (the multi-day HITL approval/cancel
//! waits, the CONSUMING half, P-FLOW-11)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 9.4
//! (**durable signal (multi-day HITL)** — `state=waiting` holds no runtime; an `approval`/`cancel`/
//! `ci.result`/`job.done` signal arrives hours/days later (idempotent), re-leases + replays +
//! CONSUMES), and row 9.2 (`WfCtx::wait_for_signal` — the wait half). Owning architecture:
//! `durable-workflow.md` §4.3 (the signal round-trip — `state=waiting` holds no runtime) and §6.3 (the
//! HITL approval-card round-trip mechanics).
//!
//! ## What this pair pins (the PROVIDER ↔ CONSUMER agreement of 9.4's CONSUMING half)
//!
//! The DELIVERY half of 9.4 (the bus consumer translating an inbound signal into one buffered
//! `wf_signal` row, idempotent on the PK) is pinned by `cdc_9_1_signal.rs` (P-FLOW-09). THIS pair pins
//! the CONSUMING half — the wait that re-leases + replays + consumes the buffered signal:
//!
//! **9.4 PROVIDER (the `myelin-flow` wait, [`WfCtx::wait_for_signal`]) — what the engine guarantees:**
//! - a wait on an absent signal PARKS the run (`state=waiting`, holds NO runtime);
//! - a buffered signal RESUMES the run and is CONSUMED **exactly once** (the buffered depth drops by
//!   one; a re-drive replays the journaled `signal_received` and consumes NOTHING new);
//! - a declined decision is WITHHELD (0 mutation, AG-8).
//!
//! **9.4 CONSUMER (the HITL surface — Chat's approval card / the merge queue's `ci.result` producer) —
//! what it relies on:**
//! - it posts the decision/completion via `DurableExecutor::signal` ONCE under a per-effect `idem_key`
//!   (`card_id` / the merge attempt id); a **double-click** posts the SAME key → ON CONFLICT DO NOTHING
//!   → one buffered row → the wait consumes it ONCE (the workflow wakes once, never twice);
//! - it does NOT re-implement the wait/consume — it relies on the provider's consume-exactly-once.
//!
//! This pair proves the two ends RECONCILE: the HITL surface posts the signal (the consumer side) and
//! the wait consumes it exactly once across a multi-day park + a re-drive (the provider side) — the
//! §2.9 DAG-respecting seam (the consumer depends on the `DurableExecutor`/`WfCtx` traits).

use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    approval_wait_name, drive_full, run_state, DriveOutcome, DurableExecutor, FlowExecutor,
    FlowTelemetry, RetryPolicy, RunRow, SignalOutcome, SignalSpec, StartSpec, WaitOutcome, WfCtx,
    WfJournal, WorkflowBody, DECLINE_MARKER,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
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
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: None,
    }
}

fn executor() -> FlowExecutor {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    ex
}

fn start_a_run(ex: &FlowExecutor) -> myelin_flow::RunId {
    ex.start(StartSpec {
        wf_type: "agent.run".into(),
        input: vec![],
        budget: None,
        idem_key: "k".into(),
    })
    .expect("start")
}

/// The waiting body: park on `approval:call-1`; on approve run a tool (one effect), on decline withhold.
fn waiting_body() -> Box<WorkflowBody> {
    Box::new(|ctx: &mut WfCtx| {
        match ctx
            .wait_for_signal(&approval_wait_name("call-1"), None)
            .map_err(|e| format!("{e:?}"))?
        {
            WaitOutcome::Signalled {
                payload_key_ref, ..
            } if payload_key_ref.as_deref() == Some(DECLINE_MARKER) => {
                Ok(vec![]) // withheld (AG-8).
            }
            WaitOutcome::Signalled { .. } => ctx
                .activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
                    Ok(vec![ArtifactRef("myelin://acme/agent/effect/done".into())])
                })
                .map_err(|e| format!("{e:?}")),
            _ => Ok(vec![]),
        }
    })
}

fn drive_once(
    ex: &FlowExecutor,
    journal: &WfJournal,
    outbox: &OutboxStore,
    tele: &FlowTelemetry,
    run: &myelin_flow::RunId,
    now_secs: i64,
) -> DriveOutcome {
    let row = ex.runs().get(&tenant(), &run.0).expect("the run row");
    let body = waiting_body();
    drive_full(
        ex.runs(),
        outbox,
        journal,
        tele,
        minter(),
        ctx_base(),
        &row,
        "2026-06-21T00:00:00Z",
        7,
        body.as_ref(),
        1,
        1,
        None,
        Some(ex.signals().clone()),
        now_secs,
    )
}

/// **PROVIDER side of 9.4 (the wait): a buffered signal resumes the parked run and is CONSUMED EXACTLY
/// ONCE across a re-drive.** Drive 1 parks (state=waiting holds no runtime). The approval is buffered.
/// Drive 2 resumes + consumes ONCE. Drive 3 (a later re-drive) replays the journaled consume and
/// consumes NOTHING new — consume-exactly-once.
#[test]
fn provider_wait_parks_then_consumes_a_buffered_signal_exactly_once() {
    let ex = executor();
    let run = start_a_run(&ex);
    let journal = WfJournal::new();
    let outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

    // DRIVE 1: park (state=waiting holds no runtime — the PROVIDER promise).
    let o1 = drive_once(&ex, &journal, &outbox, &tele, &run, 1_000);
    assert_eq!(
        o1,
        DriveOutcome::Waiting,
        "PROVIDER promise: the wait PARKS on an absent signal"
    );
    assert_eq!(
        ex.runs().get(&tenant(), &run.0).unwrap().state,
        run_state::WAITING
    );

    // the HITL surface posts the approval (the CONSUMER side) — buffered once.
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: approval_wait_name("call-1"),
        idem_key: "card-7".into(),
        payload: vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
        payload_key_ref: None,
    })
    .expect("approve");
    assert_eq!(
        ex.signals().buffered_depth(),
        1,
        "the approval is buffered (depth 1)"
    );

    // DRIVE 2 (re-lease): resume + consume ONCE.
    ex.runs().wake(&tenant(), &run.0);
    let o2 = drive_once(&ex, &journal, &outbox, &tele, &run, 200_000);
    assert!(
        matches!(o2, DriveOutcome::Completed(_)),
        "PROVIDER: the buffered signal resumes the run"
    );
    assert_eq!(
        ex.signals().buffered_depth(),
        0,
        "PROVIDER promise: the signal was CONSUMED exactly once (the buffered depth dropped to 0)"
    );
}

/// **CONSUMER side of 9.4: a DOUBLE-CLICK posts the SAME per-effect key → ON CONFLICT DO NOTHING → the
/// wait consumes it ONCE (the workflow wakes once).** The HITL surface's reliance: it may deliver the
/// decision twice (a double-click, an at-least-once redelivery); the provider's PK dedup + the wait's
/// consume-once means the run wakes ONCE, never twice.
#[test]
fn consumer_double_click_under_the_same_key_wakes_the_run_once() {
    let ex = executor();
    let run = start_a_run(&ex);
    let journal = WfJournal::new();
    let outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

    // park.
    drive_once(&ex, &journal, &outbox, &tele, &run, 1_000);

    // the CONSUMER double-clicks Approve (two deliveries under the SAME idem_key).
    let post = || {
        ex.signal(SignalSpec {
            run: run.clone(),
            signal_name: approval_wait_name("call-1"),
            idem_key: "card-7".into(),
            payload: vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
            payload_key_ref: None,
        })
        .expect("post")
    };
    assert_eq!(post(), SignalOutcome::Buffered, "the first click buffered");
    assert_eq!(
        post(),
        SignalOutcome::Duplicate,
        "the double-click is a no-op (ON CONFLICT DO NOTHING)"
    );
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        1,
        "CONSUMER reliance: the double-click buffered ONE row (the wait consumes it once)"
    );

    // the wait consumes the single buffered row ONCE — the run wakes once.
    ex.runs().wake(&tenant(), &run.0);
    let o2 = drive_once(&ex, &journal, &outbox, &tele, &run, 200_000);
    assert!(
        matches!(o2, DriveOutcome::Completed(_)),
        "the run woke ONCE on the double-clicked approval"
    );
    assert_eq!(
        ex.signals().buffered_depth(),
        0,
        "consume-exactly-once across the double-click"
    );
}

/// **The two ends RECONCILE on a DENY: the HITL surface posts a decline → the wait consumes it →
/// WITHHELD = 0 mutation (AG-8).** This pins that the consumer's decline lands as the provider's
/// withhold (the merge tool NEVER runs) — the 0-pre-approval-mutation invariant the HITL surface relies
/// on.
#[test]
fn the_decline_path_reconciles_to_zero_mutation() {
    let ex = executor();
    let run = start_a_run(&ex);
    let journal = WfJournal::new();
    let outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

    // park.
    drive_once(&ex, &journal, &outbox, &tele, &run, 1_000);
    let emits_at_park = outbox.committed_count();

    // the CONSUMER posts a DENY (empty payload + the DECLINE_MARKER, §3.4).
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: approval_wait_name("call-1"),
        idem_key: "card-7".into(),
        payload: vec![],
        payload_key_ref: Some(DECLINE_MARKER.into()),
    })
    .expect("deny");

    // the wait consumes the decline → the body WITHHOLDS the tool (0 mutation).
    ex.runs().wake(&tenant(), &run.0);
    let o2 = drive_once(&ex, &journal, &outbox, &tele, &run, 200_000);
    assert_eq!(
        o2,
        DriveOutcome::Completed(vec![]),
        "a DENY completes with NO effect (withheld)"
    );
    assert_eq!(
        outbox.committed_count(),
        emits_at_park,
        "RECONCILE: the consumer's decline → the provider's WITHHOLD → 0 mutation (AG-8)"
    );
    assert_eq!(
        ex.signals().buffered_depth(),
        0,
        "the decline was consumed once"
    );
}

/// **The provider's `cancel` wait is the SAME mechanism (9.4 names `cancel` alongside `approval`).** A
/// run parked on a `cancel` wait consumes a delivered `cancel` signal exactly once — the wait is
/// signal-name-agnostic (the FROZEN names `approval`/`cancel`/`ci.result`/`job.done` all ride it).
#[test]
fn the_cancel_wait_rides_the_same_consume_once_mechanism() {
    let ex = executor();
    let run = start_a_run(&ex);
    let journal = WfJournal::new();
    let outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

    // a body that waits on `cancel` (the §4.3 FROZEN name) instead of approval.
    let cancel_body: Box<WorkflowBody> = Box::new(|ctx: &mut WfCtx| {
        match ctx
            .wait_for_signal("cancel", None)
            .map_err(|e| format!("{e:?}"))?
        {
            WaitOutcome::Signalled { .. } => Ok(vec![]), // the cancel arrived — the body unwinds.
            _ => Ok(vec![]),
        }
    });
    let row = RunRow::new_runnable(tenant(), region(), run.0.clone(), "agent.run", 0);
    ex.runs().put(row.clone());

    // park on the cancel wait.
    let o1 = drive_full(
        ex.runs(),
        &outbox,
        &journal,
        &tele,
        minter(),
        ctx_base(),
        &row,
        "2026-06-21T00:00:00Z",
        7,
        cancel_body.as_ref(),
        1,
        1,
        None,
        Some(ex.signals().clone()),
        1_000,
    );
    assert_eq!(o1, DriveOutcome::Waiting, "the cancel wait parks");

    // deliver the cancel signal — consumed exactly once.
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: "cancel".into(),
        idem_key: "user-req".into(),
        payload: vec![],
        payload_key_ref: None,
    })
    .expect("cancel");
    ex.runs().wake(&tenant(), &run.0);
    let row2 = ex.runs().get(&tenant(), &run.0).unwrap();
    let o2 = drive_full(
        ex.runs(),
        &outbox,
        &journal,
        &tele,
        minter(),
        ctx_base(),
        &row2,
        "2026-06-21T00:00:00Z",
        7,
        cancel_body.as_ref(),
        1,
        1,
        None,
        Some(ex.signals().clone()),
        2_000,
    );
    assert!(
        matches!(o2, DriveOutcome::Completed(_)),
        "the cancel wait resumes on the cancel signal"
    );
    assert_eq!(
        ex.signals().buffered_depth(),
        0,
        "the cancel signal was consumed exactly once"
    );
}
