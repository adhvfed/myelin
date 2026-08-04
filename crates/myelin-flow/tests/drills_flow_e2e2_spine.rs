use myelin_events::{
    Actor, AggregateKey, ArtifactRef as EvArtifactRef, DataRole, EmitContextBase, EventDraft,
    EventType, IdMinter, MonotonicMinter, OutboxStore, Timestamp, Visibility,
};
use myelin_flow::{
    merge_attempt_id, partition_for_run_id, request_approval_and_wait, run_state, BudgetGate,
    CheckFact, CiDispatch, CiDispatcher, DelegationCaveats, DriveOutcome, DurableExecutor,
    FlowDispatcher, FlowExecutor, FlowTelemetry, JobKind, JobRunner, JobSpec, MergeOutcome,
    MergePerformer, MergeRequest, MeteredUnit, MicroUsd, RealCiResultProducer, RetryPolicy,
    RunStore, RunTokenError, RunTokenHandle, RunTokenLease, RunTokenMinter, SignalSpec,
    SignalStore, TimerStore, WaitOutcome, Wallet, WfCtx, WfJournal, WorkflowBody, DECLINE_MARKER,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const REPO: &str = "myelin://acme/git/repo/core";
const FIX_COMMIT: &str = "f1xc0mm17";

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
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
        caused_by: None,
    }
}

fn unit(wholesale: u64, markup: u64) -> MeteredUnit {
    MeteredUnit {
        unit: "agent.step",
        wholesale: MicroUsd(wholesale),
        markup: MicroUsd(markup),
    }
}

#[derive(Default)]
struct RecordingMinter {
    calls: AtomicU64,
    last: Mutex<Option<(String, String, DelegationCaveats, u64)>>,
}
impl RunTokenMinter for RecordingMinter {
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() =
            Some((agent_id.into(), run_id.into(), caveats.clone(), ttl_secs));
        Ok(RunTokenHandle {
            token: format!("tok-{run_id}-{n}"),
            jti: format!("jti-{run_id}-{n}"),
            ttl_secs,
        })
    }
}

#[derive(Default)]
struct CountingMerger {
    merges: AtomicUsize,
}
impl MergePerformer for CountingMerger {
    fn merge(&self, request: &MergeRequest) -> Result<String, myelin_flow::ActivityError> {
        self.merges.fetch_add(1, Ordering::SeqCst);
        Ok(format!("merged-{}", request.speculative_commit_oid))
    }
}

#[derive(Default)]
struct CountingCi {
    calls: AtomicUsize,
}
impl CiDispatcher for CountingCi {
    fn dispatch(&self, _ci: &CiDispatch) -> Result<(), myelin_flow::ActivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct CountingRunner {
    calls: AtomicUsize,
}
impl JobRunner for CountingRunner {
    fn dispatch(&self, _spec: &JobSpec) -> Result<(), myelin_flow::ActivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn triage_body(runner: Arc<CountingRunner>, merged_flag: Arc<AtomicUsize>) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        let _step = ctx
            .metered_schedule_and_run_job(
                JobSpec::new(JobKind::Agent, "agent://acme/job/triage"),
                runner.as_ref(),
                Some(3600),
                MicroUsd(100),
                vec![unit(70, 30)],
            )
            .map_err(|e| format!("{e:?}"))?;

        let _issue = ctx
            .activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
                Ok(vec![ArtifactRef("myelin://acme/issues/issue/T-1".into())])
            })
            .map_err(|e| format!("{e:?}"))?;

        let outcome = request_approval_and_wait(
            ctx,
            "merge-1",
            vec![ArtifactRef("myelin://acme/agent/tool/git.merge".into())],
            Some(7 * 86_400),
            |refs| EventDraft {
                type_: EventType("agent.approval.requested".into()),
                subject: EvArtifactRef("myelin://acme/agent/run/triage-1".into()),
                aggregate: AggregateKey("run:triage-1".into()),
                payload: serde_json::json!({
                    "reason": "approval_requested",
                    "action": "git.merge",
                    "refs": refs.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
                }),
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            },
        )
        .map_err(|e| format!("{e:?}"))?;

        match outcome {
            WaitOutcome::Signalled {
                payload_key_ref, ..
            } if payload_key_ref.as_deref() == Some(DECLINE_MARKER) => {
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
                if !eff.is_empty() {
                    merged_flag.fetch_add(1, Ordering::SeqCst);
                }
                Ok(eff)
            }
            WaitOutcome::TimedOut => Ok(vec![]),
            WaitOutcome::Parked => Ok(vec![]),
        }
    })
}

fn merge_queue_body(ci: Arc<CountingCi>, merger: Arc<CountingMerger>) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        let out = ctx
            .run_merge_attempt(
                &fix_pr_request(),
                ci.as_ref(),
                merger.as_ref(),
                Some(3600),
                MicroUsd(50),
                vec![unit(30, 20)],
            )
            .map_err(|e| format!("{e:?}"))?;
        match out {
            MergeOutcome::Merged {
                merged_commit_oid, ..
            } => Ok(vec![ArtifactRef(format!(
                "outcome:merged:{merged_commit_oid}"
            ))]),
            MergeOutcome::Dequeued { reason } => {
                Ok(vec![ArtifactRef(format!("outcome:dequeued:{reason}"))])
            }
            MergeOutcome::TimedOut => Ok(vec![ArtifactRef("outcome:timedout".into())]),
            MergeOutcome::Parked => Ok(vec![]),
        }
    })
}

fn fix_pr_request() -> MergeRequest {
    MergeRequest {
        pr_ref: format!("{REPO}#pr-fix"),
        target_ref: "refs/heads/main".into(),
        speculative_commit_oid: FIX_COMMIT.into(),
        required_contexts: vec!["build".into(), "test".into()],
    }
}

fn required() -> Vec<String> {
    vec!["build".into(), "test".into()]
}

struct Substrate {
    runs: RunStore,
    journal: WfJournal,
    signals: SignalStore,
    outbox: OutboxStore,
    tele: FlowTelemetry,
    timers: TimerStore,
    minter: Arc<dyn IdMinter>,
}

#[test]
fn e2e2_durable_workflow_hitl_spine_across_kill_and_days_later_approval() {
    let wallet_start = MicroUsd(1_000);
    let tele = FlowTelemetry::new();
    let gate = BudgetGate::new(Wallet::new(wallet_start)).with_telemetry(tele.clone());

    let recording_minter = Arc::new(RecordingMinter::default());
    let lease = RunTokenLease::new(
        recording_minter.clone(),
        "agent://acme/agent/triage",
        DelegationCaveats(vec!["tenant:acme".into()]),
    );

    let runner = Arc::new(CountingRunner::default());
    let merged_flag = Arc::new(AtomicUsize::new(0));

    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    let triage = ex
        .start(myelin_flow::StartSpec {
            wf_type: "agent.run".into(),
            input: vec![],
            budget: None,
            idem_key: "rule:ci.result=failure:run-1".into(),
        })
        .expect("CI-fail wakes the triage agent run");

    let sub = Substrate {
        runs: ex.runs().clone(),
        journal: WfJournal::new(),
        signals: ex.signals().clone(),
        outbox: OutboxStore::new(),
        tele: tele.clone(),
        timers: TimerStore::new(),
        minter: minter(),
    };
    let part = partition_for_run_id(&triage.0);

    let step_token = myelin_flow::job_idem_token(&triage.0, "agent.run:0");
    sub.signals.deliver(myelin_flow::SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: triage.0.clone(),
        signal_name: myelin_flow::JOB_DONE_SIGNAL.into(),
        idem_key: step_token,
        payload: vec![ArtifactRef("myelin://acme/agent/triage/done".into())],
        payload_key_ref: None,
        received_unix_ms: 0,
        consumed_seq: None,
    });

    let fresh_triage_worker = |worker: &str| -> FlowDispatcher {
        let mut disp = FlowDispatcher::new(
            sub.runs.clone(),
            sub.outbox.clone(),
            sub.journal.clone(),
            sub.tele.clone(),
            sub.minter.clone(),
            ctx_base(),
            part,
            worker,
            30,
        )
        .with_signals(sub.signals.clone())
        .with_timers(sub.timers.clone())
        .with_budget(gate.clone())
        .with_run_identity(lease.clone());
        disp.register(
            "agent.run",
            triage_body(runner.clone(), merged_flag.clone()),
        );
        disp
    };

    let w1 = fresh_triage_worker("agent-worker-1");
    let o1 = w1
        .tick(1_000, "2026-06-25T00:00:00Z", 7)
        .expect("worker-1 drives the triage run");
    assert_eq!(
        o1,
        DriveOutcome::Waiting,
        "the triage run PARKED on the git.merge approval wait (0 mutation before approval)"
    );
    assert_eq!(
        sub.runs.get(&tenant(), &triage.0).unwrap().state,
        run_state::WAITING,
        "state=waiting - the HITL gate holds no runtime across the (multi-day) ack_window"
    );
    assert_eq!(
        merged_flag.load(Ordering::SeqCst),
        0,
        "0 mutation before approval - the gated git.merge effect is WITHHELD (AG-8)"
    );
    assert_eq!(
        tele.settled(),
        1,
        "the triage step settled ONCE at the park (1 completed dispatch)"
    );
    assert_eq!(
        tele.reserve_rejected(),
        0,
        "0 reserve rejects (the wallet funded the dispatch)"
    );
    assert_eq!(
        gate.inflight_interrupt_count(),
        0,
        "0 in-flight interrupts at the park (never-interrupt-in-flight, contract 11.7)"
    );
    assert_eq!(
        gate.balance(),
        MicroUsd(wallet_start.0 - 100),
        "the wallet conserved: debited exactly the billed triage step cost (100) - reserve/settle balanced"
    );
    assert!(
        runner.calls.load(Ordering::SeqCst) == 1,
        "the triage step's job dispatched once"
    );

    drop(w1);

    let approve = || {
        ex.signal(SignalSpec {
            run: triage.clone(),
            signal_name: myelin_flow::approval_wait_name("merge-1"),
            idem_key: "card-merge-1".into(),
            payload: vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
            payload_key_ref: None,
        })
        .expect("approve")
    };
    let first = approve();
    let second = approve();
    assert_eq!(
        first,
        myelin_flow::SignalOutcome::Buffered,
        "the first click buffered the approval"
    );
    assert_eq!(
        second,
        myelin_flow::SignalOutcome::Duplicate,
        "the double-click is a no-op (ON CONFLICT DO NOTHING) - the workflow wakes once"
    );
    assert_eq!(
        sub.signals.buffered_depth(),
        1,
        "the double-click buffered EXACTLY ONE approval (1 wake)"
    );
    sub.runs.wake(&tenant(), &triage.0);

    let remints_before_resume = recording_minter.calls.load(Ordering::SeqCst);

    let w2 = fresh_triage_worker("agent-worker-2");
    let o2 = w2
        .tick(7 * 86_400 + 2_000, "2026-07-02T00:00:00Z", 7)
        .expect("worker-2 resumes the triage run");
    match o2 {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![ArtifactRef("myelin://acme/agent/effect/merged".into())],
            "the resumed run RAN the approved git.merge tool (the approve branch)"
        ),
        other => panic!("expected the triage run to resume + complete, got {other:?}"),
    }

    assert_eq!(
        sub.signals.buffered_depth(),
        0,
        "the approval was consumed EXACTLY ONCE across the kill (1 consume)"
    );
    assert_eq!(
        merged_flag.load(Ordering::SeqCst),
        1,
        "the git.merge effect applied EXACTLY ONCE across the kill (merge-count == 1, FLOW-D1)"
    );
    let remints_after_resume = recording_minter.calls.load(Ordering::SeqCst);
    assert_eq!(
        remints_after_resume - remints_before_resume,
        1,
        "the resume across the multi-day wait re-minted EXACTLY ONE fresh per-run token (contract 4.7)"
    );
    let (mint_agent, mint_run, mint_caveats, mint_ttl) = recording_minter
        .last
        .lock()
        .unwrap()
        .clone()
        .expect("a re-mint was recorded on resume");
    assert_eq!(
        mint_agent, "agent://acme/agent/triage",
        "minted for the run's agent"
    );
    assert_eq!(mint_run, triage.0, "minted for THIS run");
    assert!(
        mint_caveats.0.contains(&format!("run:{}", triage.0)),
        "the re-minted token is ATTENUATED per-run (scoped to THIS run): {mint_caveats:?}"
    );
    assert!(
        mint_ttl > 0 && mint_ttl <= 3600,
        "the re-minted token is SHORT-LIVED (token life == activity life, not the days-long workflow life): ttl={mint_ttl}"
    );

    assert_eq!(
        tele.settled(),
        1,
        "still ONE settle across the resume (the replay re-settles nothing)"
    );
    assert_eq!(
        tele.reserve_rejected(),
        0,
        "0 reserve rejects across the resume"
    );
    assert_eq!(
        gate.balance(),
        MicroUsd(wallet_start.0 - 100),
        "the wallet conserved across the resume: still debited exactly 100 (reserve/settle balanced)"
    );

    let ci = Arc::new(CountingCi::default());
    let merger = Arc::new(CountingMerger::default());

    ex.register_definition("merge.queue");
    let mq = ex
        .start(myelin_flow::StartSpec {
            wf_type: "merge.queue".into(),
            input: vec![],
            budget: None,
            idem_key: "queue:main:pr-fix".into(),
        })
        .expect("the fix-PR is queued for merge");
    let mq_part = partition_for_run_id(&mq.0);

    let fresh_mq_worker = |worker: &str| -> FlowDispatcher {
        let mut disp = FlowDispatcher::new(
            sub.runs.clone(),
            sub.outbox.clone(),
            sub.journal.clone(),
            sub.tele.clone(),
            sub.minter.clone(),
            ctx_base(),
            mq_part,
            worker,
            30,
        )
        .with_signals(sub.signals.clone())
        .with_timers(sub.timers.clone())
        .with_budget(gate.clone());
        disp.register("merge.queue", merge_queue_body(ci.clone(), merger.clone()));
        disp
    };

    let mw1 = fresh_mq_worker("mq-worker-1");
    assert_eq!(
        mw1.tick(8 * 86_400, "2026-07-03T00:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting,
        "the merge-queue run PARKED on the fix-PR's ci.result wait"
    );
    assert_eq!(
        ci.calls.load(Ordering::SeqCst),
        1,
        "the required CI dispatched once"
    );
    drop(mw1);

    let facts = vec![
        CheckFact {
            context: "build".into(),
            run_attempt: 1,
            success: true,
            seq: 1,
        },
        CheckFact {
            context: "test".into(),
            run_attempt: 1,
            success: true,
            seq: 2,
        },
    ];
    let attempt = merge_attempt_id(&mq.0, "merge.queue:0");
    let producer = RealCiResultProducer::new(&sub.signals, tenant(), region(), &mq.0, REPO);
    let d1 = producer.deliver(FIX_COMMIT, &facts, &required(), &attempt);
    let d2 = producer.deliver(FIX_COMMIT, &facts, &required(), &attempt);
    assert!(d1, "the first green ci.result delivery is new");
    assert!(
        !d2,
        "the at-least-once double-delivery deduped on merge_attempt_id (1 wake)"
    );
    assert_eq!(
        sub.signals.count_for_run(&tenant(), &mq.0),
        1,
        "ONE buffered ci.result for the merge-queue run (wakes once)"
    );
    sub.runs.wake(&tenant(), &mq.0);

    let mw2 = fresh_mq_worker("mq-worker-2");
    match mw2
        .tick(8 * 86_400 + 7_200, "2026-07-03T02:00:00Z", 7)
        .expect("resume")
    {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![ArtifactRef(format!("outcome:merged:merged-{FIX_COMMIT}"))],
            "the resumed merge-queue run MERGED on the green rollup"
        ),
        other => panic!("expected the merge-queue run to merge + complete, got {other:?}"),
    }

    assert_eq!(
        merger.merges.load(Ordering::SeqCst),
        1,
        "merge-count == 1 (0 double-merge) - the merge-queue merged the fix-PR EXACTLY once"
    );
    assert_eq!(
        ci.calls.load(Ordering::SeqCst),
        1,
        "0 re-dispatch of the required CI across the merge-queue worker restart"
    );
    assert_eq!(
        sub.runs.get(&tenant(), &mq.0).unwrap().state,
        run_state::COMPLETED
    );

    assert_eq!(
        tele.settled(),
        2,
        "reserve/settle PARITY: exactly 2 settles for the 2 completed metered dispatches (triage step + merge CI)"
    );
    assert_eq!(
        tele.reserve_rejected(),
        0,
        "0 reserve rejects across the WHOLE spine"
    );
    assert_eq!(
        gate.inflight_interrupt_count(),
        0,
        "0 in-flight interrupts across the whole run (never-interrupt-in-flight, contract 11.7)"
    );
    assert_eq!(
        gate.balance(),
        MicroUsd(wallet_start.0 - 150),
        "the wallet conserved: debited exactly the billed cost of the 2 metered dispatches (100+50)"
    );

    println!(
        "[2026-06-25] PASS  drill=E2E-2  spine=durable-workflow+HITL  \
         ci-fail->triage-agent-workflow=yes  mutation-before-approval=0  \
         kill-mid-ack_window=yes  days-later-double-click->buffered=1  consume=1  \
         remint-on-resume=1(short-lived,attenuated,ttl={mint_ttl})  triage-merge-effect=1  \
         fix-pr-ci=green  merge-queue-wake=1(idempotent,X-1)  merge-count=1  re-dispatch=0  \
         reserve={}  settle={}  reserve/settle-parity=yes  inflight-interrupts=0  wallet={}->{}  \
         producer=REAL(RealCiResultProducer)  faces=MOCK(agent/issues/chat/notif/git - owners' E2E legs)",
        tele.reserve_attempted(),
        tele.settled(),
        wallet_start.0,
        gate.balance().0,
    );
}

#[test]
fn e2e2_spine_days_later_decline_withholds_the_merge_zero_mutation() {
    let tele = FlowTelemetry::new();
    let gate = BudgetGate::new(Wallet::new(MicroUsd(1_000))).with_telemetry(tele.clone());
    let recording_minter = Arc::new(RecordingMinter::default());
    let lease = RunTokenLease::new(
        recording_minter.clone(),
        "agent://acme/agent/triage",
        DelegationCaveats(vec!["tenant:acme".into()]),
    );
    let runner = Arc::new(CountingRunner::default());
    let merged_flag = Arc::new(AtomicUsize::new(0));

    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    let triage = ex
        .start(myelin_flow::StartSpec {
            wf_type: "agent.run".into(),
            input: vec![],
            budget: None,
            idem_key: "rule:ci.result=failure:run-2".into(),
        })
        .expect("start");

    let sub = Substrate {
        runs: ex.runs().clone(),
        journal: WfJournal::new(),
        signals: ex.signals().clone(),
        outbox: OutboxStore::new(),
        tele: tele.clone(),
        timers: TimerStore::new(),
        minter: minter(),
    };
    let part = partition_for_run_id(&triage.0);

    let step_token = myelin_flow::job_idem_token(&triage.0, "agent.run:0");
    sub.signals.deliver(myelin_flow::SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: triage.0.clone(),
        signal_name: myelin_flow::JOB_DONE_SIGNAL.into(),
        idem_key: step_token,
        payload: vec![ArtifactRef("myelin://acme/agent/triage/done".into())],
        payload_key_ref: None,
        received_unix_ms: 0,
        consumed_seq: None,
    });

    let fresh = |worker: &str| -> FlowDispatcher {
        let mut disp = FlowDispatcher::new(
            sub.runs.clone(),
            sub.outbox.clone(),
            sub.journal.clone(),
            sub.tele.clone(),
            sub.minter.clone(),
            ctx_base(),
            part,
            worker,
            30,
        )
        .with_signals(sub.signals.clone())
        .with_timers(sub.timers.clone())
        .with_budget(gate.clone())
        .with_run_identity(lease.clone());
        disp.register(
            "agent.run",
            triage_body(runner.clone(), merged_flag.clone()),
        );
        disp
    };

    let w1 = fresh("agent-worker-1");
    assert_eq!(
        w1.tick(1_000, "2026-06-25T00:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting,
        "parked on the merge approval"
    );
    let emits_after_park = sub.outbox.committed_count();
    drop(w1);

    let deny = || {
        ex.signal(SignalSpec {
            run: triage.clone(),
            signal_name: myelin_flow::approval_wait_name("merge-1"),
            idem_key: "card-merge-1".into(),
            payload: vec![],
            payload_key_ref: Some(DECLINE_MARKER.into()),
        })
        .expect("deny")
    };
    deny();
    deny();
    assert_eq!(
        sub.signals.buffered_depth(),
        1,
        "the double-click buffered one decline (the triage step's job.done already consumed)"
    );
    sub.runs.wake(&tenant(), &triage.0);

    let w2 = fresh("agent-worker-2");
    assert_eq!(
        w2.tick(7 * 86_400 + 2_000, "2026-07-02T00:00:00Z", 7)
            .unwrap(),
        DriveOutcome::Completed(vec![]),
        "a DECLINE completes the run with NO effect (the git.merge was WITHHELD)"
    );

    assert_eq!(
        merged_flag.load(Ordering::SeqCst),
        0,
        "the declined git.merge made 0 MUTATION (AG-8)"
    );
    assert_eq!(
        sub.outbox.committed_count(),
        emits_after_park,
        "0 emit past the card request - the withheld merge mutated nothing"
    );
    assert_eq!(
        sub.signals.buffered_depth(),
        0,
        "the decline was consumed EXACTLY once"
    );
    assert_eq!(
        tele.settled(),
        1,
        "reserve/settle BALANCED on the decline leg: ONE settle (the triage step); the withheld merge made no dispatch"
    );
    assert_eq!(
        tele.reserve_rejected(),
        0,
        "0 reserve rejects on the decline leg"
    );
    assert_eq!(
        gate.balance(),
        MicroUsd(1_000 - 100),
        "the wallet conserved on the decline leg: debited only the triage step (100)"
    );
    assert_eq!(
        recording_minter.calls.load(Ordering::SeqCst),
        1,
        "the resume re-minted once even on the decline leg (unconditional on resume, §6.2)"
    );

    println!(
        "[2026-06-25] PASS  drill=E2E-2  spine=durable-workflow+HITL  leg=DECLINE  \
         days-later-double-click->buffered=1  consume=1  git.merge=WITHHELD(0 mutation, AG-8)  \
         remint-on-resume=1  reserve/settle-parity=yes"
    );
}
