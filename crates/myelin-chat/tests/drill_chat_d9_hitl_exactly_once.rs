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
    SignalSpec, StartSpec, TimerStore, WaitOutcome, WfCtx, WfJournal, WorkflowBody,
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
                Ok(vec![])
            }
            WaitOutcome::Signalled { .. } => {
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

struct ChatHitlStory {
    executor: FlowExecutor,
    run: FlowRunId,
    outbox: OutboxStore,
    journal: WfJournal,
    timers: TimerStore,
    telemetry: myelin_flow::FlowTelemetry,
    body: Box<WorkflowBody>,
}

impl ChatHitlStory {
    const APPROVAL_REQUESTED_AT: i64 = 1_000;
    const APPROVAL_DEADLINE: i64 = Self::APPROVAL_REQUESTED_AT + 86_400;

    fn begin() -> Self {
        let executor = executor();
        let run = start_run(&executor);
        executor.runs().put(myelin_flow::RunRow::new_runnable(
            tenant(),
            region(),
            run.0.clone(),
            "agent.run",
            0,
        ));
        Self {
            executor,
            run,
            outbox: OutboxStore::new(),
            journal: WfJournal::new(),
            timers: TimerStore::new(),
            telemetry: myelin_flow::FlowTelemetry::new(),
            body: gated_tool_body(),
        }
    }

    fn drive(&self, now_secs: i64) -> DriveOutcome {
        let run = self
            .executor
            .runs()
            .get(&tenant(), &self.run.0)
            .expect("the durable workflow run exists");
        drive_full(
            self.executor.runs(),
            &self.outbox,
            &self.journal,
            &self.telemetry,
            minter(),
            ctx_base(),
            &run,
            "2026-06-21T00:00:00Z",
            7,
            self.body.as_ref(),
            1,
            1,
            Some(self.timers.clone()),
            Some(self.executor.signals().clone()),
            now_secs,
            None,
            None,
        )
    }

    fn request_approval(&self) -> usize {
        assert_eq!(
            self.drive(Self::APPROVAL_REQUESTED_AT),
            DriveOutcome::Waiting,
            "the first drive parks on the approval wait",
        );
        assert_eq!(
            self.executor
                .runs()
                .get(&tenant(), &self.run.0)
                .expect("the waiting run remains durable")
                .state,
            run_state::WAITING,
            "the run holds no runtime while a person decides",
        );
        let timers = self.timers.rows_for_run(&tenant(), &region(), &self.run.0);
        assert_eq!(timers.len(), 1, "the approval owns one durable timeout");
        assert_eq!(
            timers[0].fire_at,
            Self::APPROVAL_DEADLINE,
            "the timeout survives a multi-day worker restart",
        );
        let emitted = self.outbox.committed_count();
        assert_eq!(emitted, 1, "the card request emitted exactly once");
        emitted
    }

    fn signal(&self, click: &CardClick) -> CardSignal {
        let mut signal = build_card_signal(&card(&self.run), click);
        signal.signal_name = approval_wait_name("card-1");
        signal
    }

    fn post(&self, signal: &CardSignal) -> SignalDelivery {
        FlowSignalPort {
            ex: self.executor.clone(),
        }
        .post_signal(signal)
        .expect("the Chat decision reaches the workflow signal store")
    }

    fn resume(&self, now_secs: i64) -> DriveOutcome {
        self.executor.runs().wake(&tenant(), &self.run.0);
        self.drive(now_secs)
    }
}

#[test]
fn chat_d9_request_kill_approve_runs_exactly_once_double_click_is_one() {
    let story = ChatHitlStory::begin();
    story.request_approval();
    let approve = CardClick {
        effect_idx: 0,
        decision: CardDecision::Approve,
        decline_reason: String::new(),
    };
    let signal = story.signal(&approve);
    let d1 = story.post(&signal);
    assert_eq!(
        d1,
        SignalDelivery::Buffered,
        "the approval buffered (first click)"
    );
    let d2 = story.post(&signal);
    assert_eq!(
        d2,
        SignalDelivery::Duplicate,
        "a double-click is ONE approval (0 double-apply)"
    );

    match story.resume(200_000) {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![ArtifactRef("myelin://acme/agent/effect/merged".into())],
            "the resume RAN the approved tool EXACTLY ONCE (one effect)"
        ),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert_eq!(
        story.outbox.committed_count(),
        1,
        "card request emitted exactly once across the kill"
    );
    assert_eq!(
        story.executor.signals().buffered_depth(),
        0,
        "the approval was consumed EXACTLY once"
    );
}

#[test]
fn chat_d9_deny_withholds_zero_mutation() {
    let story = ChatHitlStory::begin();
    let emits_after_park = story.request_approval();
    let decline = CardClick {
        effect_idx: 0,
        decision: CardDecision::Decline,
        decline_reason: DECLINE_MARKER.into(),
    };
    story.post(&story.signal(&decline));

    assert_eq!(
        story.resume(2_000),
        DriveOutcome::Completed(vec![]),
        "a DENY completes with NO effect (withheld)"
    );
    assert_eq!(
        story.outbox.committed_count(),
        emits_after_park,
        "the declined tool made 0 mutation - the pre-approval-mutation signal = 0 (AG-8)"
    );
}

#[test]
fn chat_d9_timeout_auto_denies_zero_mutation() {
    let story = ChatHitlStory::begin();
    let emits_after_park = story.request_approval();
    let click = myelin_chat::hitl::auto_deny_on_timeout(0);
    let signal = story.signal(&click);
    assert_eq!(signal.payload_key_ref.as_deref(), Some(TIMEOUT_REASON));
    story.post(&signal);

    assert_eq!(
        story.resume(2_000),
        DriveOutcome::Completed(vec![]),
        "a TIMEOUT auto-deny withholds the tool"
    );
    assert_eq!(
        story.outbox.committed_count(),
        emits_after_park,
        "the timed-out tool made 0 mutation (AG-8)"
    );
}
