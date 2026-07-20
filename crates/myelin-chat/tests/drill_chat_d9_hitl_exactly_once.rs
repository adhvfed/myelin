//! # CHAT-D9 — the HITL approval-card exactly-once drill (CHAT-P18 → P-413, M4-C6)
//!
//! **Drill (testing-strategy/01 row CHAT-D9):** request an approval, KILL Chat + Workflow mid-wait,
//! APPROVE days later → the gated tool runs EXACTLY ONCE; a double-click is ONE approval; DENY
//! withholds with NO mutation; TIMEOUT auto-denies; the resume runs under a fresh token. CI; the
//! duplicate-apply signal = 0, the pre-approval-mutation signal = 0.
//!
//! **A CHAINED scenario (EI-01 §4):** request → kill → approve later → assert exactly-once. The CHAT
//! face of FLOW-D4 — chat is the card SURFACE; the durable exactly-once correctness across the kill
//! is the ENGINE's (`myelin_flow`), here driven end-to-end with chat's card posting the decision.
//!
//! **Chat owns ONLY the card** (the click gate + the signal post); the durable wait / the kill /
//! restart / the timer are the ENGINE's. This drill drives the REAL `myelin_flow` engine: the gated
//! tool parks on the durable wait, chat's card posts the approval, the engine resumes + runs the tool
//! EXACTLY ONCE across the kill.

use myelin_chat::hitl::{
    build_card_signal, CardClick, CardDecision, CardSignal, ChatApprovalCard, SignalDelivery,
    SignalPort, SignalPostError, DECLINE_MARKER, TIMEOUT_REASON,
};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    MonotonicMinter, OutboxStore, Timestamp, Visibility,
};
use myelin_flow::{
    approval_wait_name, drive_full, request_approval_and_wait, run_state, DriveOutcome,
    DurableExecutor, FlowExecutor, RetryPolicy, RunBudget, RunId as FlowRunId, SignalOutcome,
    SignalSpec, StartSpec, WaitOutcome, WfCtx, WfJournal, WorkflowBody,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RunId as IdRunId};
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

/// chat's [`SignalPort`] over the REAL engine (the production wiring shape).
struct FlowSignalPort {
    ex: FlowExecutor,
}
impl SignalPort for FlowSignalPort {
    fn post_signal(&self, signal: &CardSignal) -> Result<SignalDelivery, SignalPostError> {
        let outcome = self
            .ex
            .signal(SignalSpec {
                run: FlowRunId(signal.run_id.0.clone()),
                signal_name: signal.signal_name.clone(),
                idem_key: signal.idem_key.clone(),
                payload: signal.payload.clone(),
                payload_key_ref: signal.payload_key_ref.clone(),
            })
            .map_err(|e| SignalPostError {
                reason: format!("{e}"),
            })?;
        Ok(match outcome {
            SignalOutcome::Buffered => SignalDelivery::Buffered,
            SignalOutcome::Duplicate => SignalDelivery::Duplicate,
            SignalOutcome::TerminalNoOp => {
                unreachable!("the in-memory FlowExecutor buffers signals to terminal runs")
            }
        })
    }
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

fn approval_request_draft(refs: &[ArtifactRef]) -> EventDraft {
    EventDraft {
        type_: EventType("agent.approval.requested".into()),
        subject: ArtifactRef("myelin://acme/agent/run/R1".into()),
        aggregate: AggregateKey("run:R1".into()),
        payload: serde_json::json!({ "refs": refs.iter().map(|r| r.0.clone()).collect::<Vec<_>>() }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// The gated-tool body (the ENGINE's wait/withhold/resume logic — NOT chat's). It emits the card
/// request + waits; on approve it runs the mutating activity (one effect); on decline/timeout it
/// WITHHOLDS (returns no effect — 0 mutation, AG-8).
fn gated_tool_body() -> Box<WorkflowBody> {
    Box::new(|ctx: &mut WfCtx| {
        let outcome = request_approval_and_wait(
            ctx,
            "card-1",
            vec![ArtifactRef("myelin://acme/agent/tool/merge".into())],
            Some(86_400),
            approval_request_draft,
        )
        .map_err(|e| format!("{e:?}"))?;
        match outcome {
            WaitOutcome::Signalled {
                payload_key_ref, ..
            } if payload_key_ref.as_deref() == Some(DECLINE_MARKER)
                || payload_key_ref.as_deref() == Some(TIMEOUT_REASON) =>
            {
                // DENY / TIMEOUT → WITHHELD: 0 mutation (AG-8). The tool does NOT run.
                Ok(vec![])
            }
            WaitOutcome::Signalled { .. } => {
                // APPROVE → run the tool (one mutating activity → one effect).
                let eff = ctx
                    .activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
                        Ok(vec![ArtifactRef(
                            "myelin://acme/agent/effect/merged".into(),
                        )])
                    })
                    .map_err(|e| format!("{e:?}"))?;
                Ok(eff)
            }
            WaitOutcome::TimedOut => Ok(vec![]),
            WaitOutcome::Parked => Ok(vec![]),
        }
    })
}

fn executor() -> FlowExecutor {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    ex
}

fn start_run(ex: &FlowExecutor) -> FlowRunId {
    ex.start(StartSpec {
        wf_type: "agent.run".into(),
        input: vec![],
        budget: Some(RunBudget { minor_units: 1_000 }),
        idem_key: "k".into(),
    })
    .expect("start")
}

#[allow(clippy::too_many_arguments)]
fn drive(
    ex: &FlowExecutor,
    outbox: &OutboxStore,
    journal: &WfJournal,
    tele: &myelin_flow::FlowTelemetry,
    run_row: &myelin_flow::RunRow,
    body: &WorkflowBody,
    now_secs: i64,
) -> DriveOutcome {
    drive_full(
        ex.runs(),
        outbox,
        journal,
        tele,
        minter(),
        ctx_base(),
        run_row,
        "2026-06-21T00:00:00Z",
        7,
        body,
        1,
        1,
        None,
        Some(ex.signals().clone()),
        now_secs,
        None,
        None,
    )
}

fn card(run: &FlowRunId) -> ChatApprovalCard {
    ChatApprovalCard {
        run_id: IdRunId(run.0.clone()),
        card_id: "card-1".into(),
        effects: vec![myelin_chat::hitl::CardEffect {
            subject: ArtifactRef("myelin://acme/git/pr/88".into()),
            action: "merge".into(),
            risk: "irreversible".into(),
            cost: "$0.40".into(),
            effect_refs: vec![ArtifactRef("myelin://acme/agent/effect/merge-88".into())],
        }],
    }
}

/// **CHAT-D9 core: request → KILL mid-wait → APPROVE days later → the gated tool runs EXACTLY ONCE;
/// a double-click is ONE approval.** Drive 1 emits the card request + parks (the kill is modelled by
/// dropping all in-memory drive state except the durable stores — the engine re-leases). Chat's card
/// posts the approval; a DOUBLE-CLICK re-posts the same key (the engine dedups). Drive 2 (the
/// restart) resumes, consumes the approval ONCE, runs the tool — the card request is NOT re-emitted
/// and the tool runs exactly once.
#[test]
fn chat_d9_request_kill_approve_runs_exactly_once_double_click_is_one() {
    let ex = executor();
    let run = start_run(&ex);
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let run_row =
        myelin_flow::RunRow::new_runnable(tenant(), region(), run.0.clone(), "agent.run", 0);
    ex.runs().put(run_row.clone());
    let body = gated_tool_body();
    let tele = myelin_flow::FlowTelemetry::new();

    // DRIVE 1: emit the card request + park (state=waiting, holds no runtime — the kill window).
    let o1 = drive(
        &ex,
        &outbox,
        &journal,
        &tele,
        &run_row,
        body.as_ref(),
        1_000,
    );
    assert_eq!(
        o1,
        DriveOutcome::Waiting,
        "drive 1 parks on the approval wait"
    );
    let emits_after_request = outbox.committed_count();
    assert_eq!(emits_after_request, 1, "the card request emitted ONCE");
    assert_eq!(
        ex.runs().get(&tenant(), &run.0).unwrap().state,
        run_state::WAITING,
        "the run holds NO runtime while it waits (the multi-day kill window)"
    );

    // --- KILL Chat + Workflow mid-wait (the durable stores survive; the in-memory drive is gone). ---

    // DAYS LATER: a human clicks Approve in Chat → chat's card posts the approval signal. Modelled as
    // chat's SignalPort over the real engine. NOTE: the engine's `request_approval_and_wait` waits on
    // `approval:card-1`; chat posts under that name with the single-effect key `card-1`.
    let port = FlowSignalPort { ex: ex.clone() };
    let approve = CardClick {
        effect_idx: 0,
        decision: CardDecision::Approve,
        decline_reason: String::new(),
    };
    // chat builds the signal under the engine's wait name (single-effect: idem_key == card_id).
    let mut sig = build_card_signal(&card(&run), &approve);
    sig.signal_name = approval_wait_name("card-1");
    let d1 = port.post_signal(&sig).unwrap();
    assert_eq!(
        d1,
        SignalDelivery::Buffered,
        "the approval buffered (first click)"
    );
    // DOUBLE-CLICK: re-post the SAME key → the engine dedups (one approval).
    let d2 = port.post_signal(&sig).unwrap();
    assert_eq!(
        d2,
        SignalDelivery::Duplicate,
        "a double-click is ONE approval (0 double-apply)"
    );

    // DRIVE 2 (the restart re-leases the run after the signal wake): resume, consume ONCE, run the tool.
    ex.runs().wake(&tenant(), &run.0);
    let run_row2 = ex.runs().get(&tenant(), &run.0).unwrap();
    let o2 = drive(
        &ex,
        &outbox,
        &journal,
        &tele,
        &run_row2,
        body.as_ref(),
        200_000,
    );
    match o2 {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![ArtifactRef("myelin://acme/agent/effect/merged".into())],
            "the resume RAN the approved tool EXACTLY ONCE (one effect)"
        ),
        other => panic!("expected Completed, got {other:?}"),
    }
    // the card request was emitted EXACTLY once (NOT re-emitted on the resume — the duplicate-apply
    // signal = 0); the approval was consumed EXACTLY once.
    assert_eq!(
        outbox.committed_count(),
        1,
        "card request emitted exactly once across the kill"
    );
    assert_eq!(
        ex.signals().buffered_depth(),
        0,
        "the approval was consumed EXACTLY once"
    );
}

/// **CHAT-D9: DENY withholds with NO mutation (AG-8; pre-approval-mutation signal = 0).** The run
/// parks; chat's card posts a DECLINE (empty payload + DECLINE_MARKER); the resume WITHHOLDS the tool
/// — the merge activity NEVER runs (0 mutation past the card request).
#[test]
fn chat_d9_deny_withholds_zero_mutation() {
    let ex = executor();
    let run = start_run(&ex);
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let run_row =
        myelin_flow::RunRow::new_runnable(tenant(), region(), run.0.clone(), "agent.run", 0);
    ex.runs().put(run_row.clone());
    let body = gated_tool_body();
    let tele = myelin_flow::FlowTelemetry::new();

    drive(
        &ex,
        &outbox,
        &journal,
        &tele,
        &run_row,
        body.as_ref(),
        1_000,
    );
    let emits_after_park = outbox.committed_count();

    // chat's card posts a DECLINE under the engine's wait name (empty payload + DECLINE_MARKER, AG-8).
    let port = FlowSignalPort { ex: ex.clone() };
    let decline = CardClick {
        effect_idx: 0,
        decision: CardDecision::Decline,
        decline_reason: DECLINE_MARKER.into(),
    };
    let mut sig = build_card_signal(&card(&run), &decline);
    sig.signal_name = approval_wait_name("card-1");
    port.post_signal(&sig).unwrap();

    ex.runs().wake(&tenant(), &run.0);
    let run_row2 = ex.runs().get(&tenant(), &run.0).unwrap();
    let o2 = drive(
        &ex,
        &outbox,
        &journal,
        &tele,
        &run_row2,
        body.as_ref(),
        2_000,
    );
    assert_eq!(
        o2,
        DriveOutcome::Completed(vec![]),
        "a DENY completes with NO effect (withheld)"
    );
    // 0 mutation: the merge effect was NEVER emitted past the card request (AG-8).
    assert_eq!(
        outbox.committed_count(),
        emits_after_park,
        "the declined tool made 0 mutation — the pre-approval-mutation signal = 0 (AG-8)"
    );
}

/// **CHAT-D9: TIMEOUT auto-denies with NO mutation.** The card posts a TIMEOUT auto-deny (the
/// engine's durable timer fired first; chat surfaces it as a Decline with the TIMEOUT marker) → the
/// tool is WITHHELD (0 mutation, AG-8). Chat does NOT own the timer — it renders the auto-deny.
#[test]
fn chat_d9_timeout_auto_denies_zero_mutation() {
    let ex = executor();
    let run = start_run(&ex);
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let run_row =
        myelin_flow::RunRow::new_runnable(tenant(), region(), run.0.clone(), "agent.run", 0);
    ex.runs().put(run_row.clone());
    let body = gated_tool_body();
    let tele = myelin_flow::FlowTelemetry::new();

    drive(
        &ex,
        &outbox,
        &journal,
        &tele,
        &run_row,
        body.as_ref(),
        1_000,
    );
    let emits_after_park = outbox.committed_count();

    // the durable timer fired (the engine's) → chat surfaces the auto-deny as a Decline(TIMEOUT).
    let port = FlowSignalPort { ex: ex.clone() };
    let click = myelin_chat::hitl::auto_deny_on_timeout(0);
    let mut sig = build_card_signal(&card(&run), &click);
    sig.signal_name = approval_wait_name("card-1");
    assert_eq!(sig.payload_key_ref.as_deref(), Some(TIMEOUT_REASON));
    port.post_signal(&sig).unwrap();

    ex.runs().wake(&tenant(), &run.0);
    let run_row2 = ex.runs().get(&tenant(), &run.0).unwrap();
    let o2 = drive(
        &ex,
        &outbox,
        &journal,
        &tele,
        &run_row2,
        body.as_ref(),
        2_000,
    );
    assert_eq!(
        o2,
        DriveOutcome::Completed(vec![]),
        "a TIMEOUT auto-deny withholds the tool"
    );
    assert_eq!(
        outbox.committed_count(),
        emits_after_park,
        "the timed-out tool made 0 mutation (AG-8)"
    );
}
