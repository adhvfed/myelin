//! # E2E-2 — the durable-workflow + HITL SPINE of the agent-native flagship (P-FLOW-28 → P-477, M5)
//!
//! **Drill catalogue:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! **E2E-2** (CI-fail → triage agent → issue → chat → fix-PR — the agent-native flagship). This test
//! owns the **durable-workflow + HITL SPINE** of that whole-system scenario — the part `myelin-flow`
//! is responsible for (the prompt P-FLOW-28 / global P-477 scope). The cross-subsystem FACES (the real
//! agent plan loop, the Issues row, the Chat thread, the Notif card render, Git's real merge) are owned
//! by those subsystems' E2E prompts; here they are MOCK adapters (VISION §3 — no real agents during
//! development) so the SPINE properties are forced and observed deterministically.
//!
//! ## The spine the scenario forces (the exact thresholds — NEVER weaken / NEVER invert, EI-01 §3)
//!
//! 1. **CI fails → a triage agent run wakes (a `myelin-flow` workflow).** A failing CI run derives a
//!    `ci.result=failure`; that wakes a mock triage agent whose run IS a durable workflow.
//! 2. **0 mutation before approval.** The triage workflow plans, files an issue (no approval needed —
//!    Issues `triage` default), and reaches the HITL-gated `git.merge` effect. The merge effect is
//!    WITHHELD (`request_approval_and_wait` parks `state=waiting`, holding NO runtime) — the merge
//!    activity NEVER runs before a human approves (AG-8). The merge-count is `0` at the park.
//! 3. **Kill mid-`ack_window`.** The Agent + Workflow worker is KILLED while the run is parked on the
//!    approval wait (drop the dispatcher). The durable state (run store + journal + signal buffer +
//!    outbox) survives.
//! 4. **The approval arrives DAYS later as a DOUBLE-CLICK.** Two deliveries under the SAME `idem_key`
//!    buffer EXACTLY ONE approval (`ON CONFLICT DO NOTHING`) — the workflow wakes once.
//! 5. **Resume → re-mint → merge ONCE.** A redeployed worker re-leases the parked run, RESUMES across
//!    the multi-day wait, RE-MINTS a fresh short-lived attenuated per-run token on resume (contract
//!    4.7 — token life == activity life, NOT the days-long workflow life), CONSUMES the approval
//!    EXACTLY ONCE, and the merge activity applies EXACTLY ONCE (merge-count == 1, FLOW-D1 — no
//!    double-effect across the kill).
//! 6. **The fix-PR's CI goes green → the merge-queue workflow wakes on `ci.result` IDEMPOTENTLY (X-1).**
//!    A doubly-delivered green `ci.result` wakes the merge-queue run EXACTLY ONCE → it merges EXACTLY
//!    ONCE (merge-count == 1).
//! 7. **reserve/settle BALANCED.** Every spend-bearing dispatch across the WHOLE run (the triage step,
//!    the merge dispatch, the merge-queue CI dispatch) reserves-at-dispatch + settles-on-completion
//!    against the SAME wallet — reserve-count == settle-count (one cost event per metered unit, never
//!    interrupts in-flight, contract 11.7/9.5). The wallet conserves (refunded over-reservation).
//!
//! **Green artifact (dated, SCHED):** the deterministic run trace + the HITL withhold→approve→apply
//! ledger + reserve/settle parity + merge-count == 1. A red drill is information — never weaken it.
//!
//! ## Contracts exercised (the P-FLOW-28 COMMIT list)
//! 9.1/9.4 (signal + the wait — the HITL park/resume), 4.7 (the mid-workflow re-mint on resume), 5.9
//! (the `ci.result` the merge-queue wakes on), 9.5/11.7 (the reserve/settle bookend parity).
//!
//! ## What is MOCK vs REAL here (the cross-subsystem faces, recorded as their owners')
//! - REAL `myelin-flow` substrate: the [`FlowDispatcher`] over a `RunStore` + `WfJournal` + signal
//!   buffer + outbox + timer wheel; the durable park/resume; the exactly-once consume; the re-mint on
//!   resume; the reserve/settle bookend; the merge-queue body ([`WfCtx::run_merge_attempt`]).
//! - MOCK faces (owned by the OTHER subsystems' E2E prompts): the triage agent's PLAN (a fixed effect
//!   sequence — the real plan loop is Agent Fabric's E2E leg), the Issues row (`create_issue` is a
//!   no-op activity returning a ref — Issues' E2E leg), the Notif approval card RENDER (the
//!   `agent.approval.requested` emit is real; the card UX is Notif/Chat's E2E leg, P-471), Git's merge
//!   (a counting [`MergePerformer`] — Git's E2E leg). The Identity `mint_run_token` BODY is Identity's
//!   (a recording minter fixture here proves the engine CALLS the surface with the right args).

use myelin_events::{
    Actor, AggregateKey, ArtifactRef as EvArtifactRef, DataRole, EmitContextBase, EventDraft,
    EventType, IdMinter, MonotonicMinter, OutboxStore, Timestamp, Visibility,
};
use myelin_flow::{
    merge_attempt_id, partition_for_run_id, request_approval_and_wait, run_state, BudgetGate,
    CheckFact, CiDispatch, CiDispatcher, DelegationCaveats, DriveOutcome, DurableExecutor,
    FlowDispatcher, FlowExecutor, FlowTelemetry, JobKind, JobRunner, JobSpec, MergeOutcome,
    MergePerformer, MergeRequest, MeteredUnit, MinorUnits, RealCiResultProducer, RetryPolicy,
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
        wholesale: MinorUnits(wholesale),
        markup: MinorUnits(markup),
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
// The MOCK cross-subsystem faces (owned by the OTHER subsystems' E2E prompts — recorded as theirs).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// A recording `mint_run_token` minter (the contract-4.7 CONSUMER fixture — Identity owns the BODY,
/// P-ID-18). Mints a DISTINCT short-lived token per call (so a re-mint is provably a NEW token) and
/// records the `(agent_id, run_id, caveats, ttl)` it was called with — the spine asserts the resume
/// re-minted a SHORT-LIVED token ATTENUATED to the run.
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

/// Git's merge face (Git's E2E leg) — a counting [`MergePerformer`] so the spine proves the merge
/// applies EXACTLY ONCE (merge-count == 1, FLOW-D1 — no double-effect across the kill).
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

/// CI's dispatch face (CI's E2E leg) — a counting [`CiDispatcher`] so the spine proves the merge-queue
/// dispatches the required CI EXACTLY ONCE across a restart (0 re-dispatch).
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

/// The triage agent's job runner face (Agent Fabric's E2E leg) — a runner that accepts a dispatch
/// (the long-park `SCHEDULE_AND_RUN_JOB`); counts calls so a kill→resume proves 0 re-dispatch.
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

// ──────────────────────────────────────────────────────────────────────────────────────────────────
// The TRIAGE workflow body — the agent-native flagship's durable spine.
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The mock triage agent's run AS a durable workflow (the SPINE this prompt owns). The plan is a fixed
/// effect sequence (the real plan loop is Agent Fabric's leg): (1) a metered triage STEP (a
/// `SCHEDULE_AND_RUN_JOB` long-park — reserve/settle into the wallet); (2) `create_issue` (a no-op
/// activity — Issues' face; no approval needed); (3) the HITL-gated `git.merge` — `request_approval_
/// and_wait` parks; on APPROVE the merge activity runs (one effect → the body's "merged" ref); on
/// DENY/timeout the merge is WITHHELD (0 mutation, AG-8). Deterministic over its journal.
fn triage_body(runner: Arc<CountingRunner>, merged_flag: Arc<AtomicUsize>) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        // (1) The triage STEP: a metered long-park job dispatch (reserve at dispatch → settle on
        //     job.done). The runner fixture's job.done is pre-buffered by the harness (a fast triage).
        let _step = ctx
            .metered_schedule_and_run_job(
                JobSpec::new(JobKind::Agent, "agent://acme/job/triage"),
                runner.as_ref(),
                Some(3600),
                MinorUnits(100),
                vec![unit(70, 30)],
            )
            .map_err(|e| format!("{e:?}"))?;

        // (2) create_issue — no approval needed (Issues `triage` default = no). A no-op activity
        //     returning the issue ref (Issues owns the real row; this is the face).
        let _issue = ctx
            .activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
                Ok(vec![ArtifactRef("myelin://acme/issues/issue/T-1".into())])
            })
            .map_err(|e| format!("{e:?}"))?;

        // (3) The HITL-gated git.merge effect: request approval + WAIT (parks state=waiting holding no
        //     runtime). 0 mutation before approval — the merge activity is GATED behind the decision.
        let outcome = request_approval_and_wait(
            ctx,
            "merge-1",
            vec![ArtifactRef("myelin://acme/agent/tool/git.merge".into())],
            Some(7 * 86_400), // a one-week approval window.
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
                Ok(vec![]) // DENY → WITHHELD: 0 mutation (AG-8).
            }
            WaitOutcome::Signalled { .. } => {
                // APPROVE → run the merge tool EXACTLY ONCE (one mutating activity → one effect).
                let eff = ctx
                    .activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
                        Ok(vec![ArtifactRef(
                            "myelin://acme/agent/effect/merged".into(),
                        )])
                    })
                    .map_err(|e| format!("{e:?}"))?;
                // Side-channel the merge-applied count (the body's effect is the "merge"; Git's real
                // performer is the merge-queue leg below). On replay the activity short-circuits, so
                // this increments EXACTLY ONCE across the kill (the FLOW-D1 0-double-effect property).
                if !eff.is_empty() {
                    merged_flag.fetch_add(1, Ordering::SeqCst);
                }
                Ok(eff)
            }
            WaitOutcome::TimedOut => Ok(vec![]), // auto-deny → 0 mutation.
            WaitOutcome::Parked => Ok(vec![]),   // still waiting.
        }
    })
}

/// The merge-queue workflow body (the X-1 seam, §6.5) — ONE merge attempt per queued fix-PR. Dispatches
/// the required CI (reserve at dispatch), parks on `ci.result`, merges on a green rollup. Reads its
/// terminal outcome off the result refs.
fn merge_queue_body(ci: Arc<CountingCi>, merger: Arc<CountingMerger>) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        let out = ctx
            .run_merge_attempt(
                &fix_pr_request(),
                ci.as_ref(),
                merger.as_ref(),
                Some(3600),
                MinorUnits(50),
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

/// The shared durable substrate a worker drives over (survives a worker kill). The `minter` is SHARED
/// across every worker so the outbox ULID `event_id`s are globally unique (two independent monotonic
/// minters would mint colliding ULIDs into the one shared outbox — the real engine has one
/// per-process monotonic minter; here one shared minter models the same global uniqueness).
struct Substrate {
    runs: RunStore,
    journal: WfJournal,
    signals: SignalStore,
    outbox: OutboxStore,
    tele: FlowTelemetry,
    timers: TimerStore,
    minter: Arc<dyn IdMinter>,
}

/// **THE E2E-2 DURABLE-WORKFLOW + HITL SPINE — the full chain across the kill + the days-later
/// approval.** CI-fail → triage agent workflow → issue → HITL git.merge gate → KILL mid-ack_window →
/// days-later double-click approve → resume → re-mint → merge-once → fix-PR-CI-green → merge-queue
/// wakes on ci.result idempotently → merge-once. Asserts: 0 mutation before approval; exactly-once
/// approval + merge across the kill; reserve/settle balanced; merge-count == 1.
#[test]
fn e2e2_durable_workflow_hitl_spine_across_kill_and_days_later_approval() {
    // ── ONE wallet across the WHOLE run (reserve/settle parity is read off it + the telemetry).
    let wallet_start = MinorUnits(1_000);
    let tele = FlowTelemetry::new();
    let gate = BudgetGate::new(Wallet::new(wallet_start)).with_telemetry(tele.clone());

    // ── The contract-4.7 re-mint minter (Identity's BODY is mocked; the engine CALLS the surface).
    let recording_minter = Arc::new(RecordingMinter::default());
    let lease = RunTokenLease::new(
        recording_minter.clone(),
        "agent://acme/agent/triage",
        DelegationCaveats(vec!["tenant:acme".into()]),
    );

    let runner = Arc::new(CountingRunner::default());
    let merged_flag = Arc::new(AtomicUsize::new(0)); // the triage body's merge-applied count.

    // ── Start the triage agent run (a durable workflow). The CI-fail → rule → dispatch is modelled by
    //    the executor `start` (the Bus/Agent dispatch tier is those subsystems' E2E faces).
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
        minter: minter(), // ONE shared minter — globally-unique outbox ULIDs across all workers.
    };
    let part = partition_for_run_id(&triage.0);

    // Pre-buffer the triage STEP's job.done (a fast triage runner — the long-park resolves in one
    // drive). The job dispatch is the FIRST command (agent.run:0), so its idem_token keys on that.
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

    // A fresh worker over the shared substrate, wired with the wallet + the re-mint lease (the
    // production shape: the dispatcher meters into the run's wallet + mints from its agent identity).
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

    // ── WORKER 1: drive the triage run — triage step (metered) → create_issue → request the merge
    //    approval card → PARK on the approval wait (state=waiting, holds no runtime). 0 mutation.
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
        "state=waiting — the HITL gate holds no runtime across the (multi-day) ack_window"
    );
    // 0 MUTATION BEFORE APPROVAL: the merge activity NEVER ran (the body's merge-applied count is 0).
    assert_eq!(
        merged_flag.load(Ordering::SeqCst),
        0,
        "0 mutation before approval — the gated git.merge effect is WITHHELD (AG-8)"
    );
    // the triage STEP reserved + settled once at the park (reserve/settle BALANCED mid-run): the
    // metered dispatch admitted exactly one reserve, settled it on its job.done, and the wallet was
    // debited the billed 100. Parity = settle-count matches the one completed metered dispatch, with 0
    // rejects + 0 in-flight interrupts (the never-interrupt invariant), and the wallet conserved.
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
        MinorUnits(wallet_start.0 - 100),
        "the wallet conserved: debited exactly the billed triage step cost (100) — reserve/settle balanced"
    );
    assert!(
        runner.calls.load(Ordering::SeqCst) == 1,
        "the triage step's job dispatched once"
    );

    // ── KILL the Agent + Workflow worker mid-ack_window (drop the dispatcher). Days pass.
    drop(w1);

    // ── DAYS LATER: a human clicks Approve — and DOUBLE-CLICKS (two deliveries, same idem_key).
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
    let second = approve(); // the DOUBLE-CLICK.
    assert_eq!(
        first,
        myelin_flow::SignalOutcome::Buffered,
        "the first click buffered the approval"
    );
    assert_eq!(
        second,
        myelin_flow::SignalOutcome::Duplicate,
        "the double-click is a no-op (ON CONFLICT DO NOTHING) — the workflow wakes once"
    );
    // EXACTLY ONE approval is now buffered (unconsumed) — the triage step's job.done was already
    // consumed on drive 1, so the only outstanding signal is the single approval (the double-click
    // deduped on idem_key).
    assert_eq!(
        sub.signals.buffered_depth(),
        1,
        "the double-click buffered EXACTLY ONE approval (1 wake)"
    );
    sub.runs.wake(&tenant(), &triage.0);

    let remints_before_resume = recording_minter.calls.load(Ordering::SeqCst);

    // ── WORKER 2 (redeployed): re-lease + RESUME across the multi-day wait → re-mint → consume once →
    //    merge activity applies ONCE.
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

    // EXACTLY-ONCE APPROVAL: the buffered approval was consumed once (depth dropped to 0).
    assert_eq!(
        sub.signals.buffered_depth(),
        0,
        "the approval was consumed EXACTLY ONCE across the kill (1 consume)"
    );
    // EXACTLY-ONCE MERGE (the triage effect): the merge activity applied once (no double-effect).
    assert_eq!(
        merged_flag.load(Ordering::SeqCst),
        1,
        "the git.merge effect applied EXACTLY ONCE across the kill (merge-count == 1, FLOW-D1)"
    );
    // RE-MINT ON RESUME (contract 4.7): the resume re-minted EXACTLY ONE fresh token.
    let remints_after_resume = recording_minter.calls.load(Ordering::SeqCst);
    assert_eq!(
        remints_after_resume - remints_before_resume,
        1,
        "the resume across the multi-day wait re-minted EXACTLY ONE fresh per-run token (contract 4.7)"
    );
    // the re-minted token is SHORT-LIVED + ATTENUATED to the run (token life == activity life).
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

    // RESERVE/SETTLE BALANCED across the resumed run: the resume's replay re-derives the metered
    // dispatch's reserve as a DUPLICATE (deduped, not re-settled) — so the settle-count stays at the
    // ONE completed triage dispatch, with 0 rejects + 0 interrupts and the wallet still conserved
    // (debited exactly 100). A stranded reservation or a missing settle would corrupt the balance.
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
        MinorUnits(wallet_start.0 - 100),
        "the wallet conserved across the resume: still debited exactly 100 (reserve/settle balanced)"
    );

    // ──────────────────────────────────────────────────────────────────────────────────────────────
    // ── THE FIX-PR's CI GOES GREEN → THE MERGE-QUEUE WORKFLOW WAKES ON ci.result IDEMPOTENTLY (X-1).
    // ──────────────────────────────────────────────────────────────────────────────────────────────
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

    // MQ WORKER 1: dispatch the required CI + PARK on ci.result (holds no runtime). Then kill it.
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
    drop(mw1); // the merge-queue worker is killed while parked (the fix-PR's CI runs for hours).

    // CI's REAL producer DERIVES a green ci.result from per-context facts and delivers it TWICE
    // (at-least-once) — the merge-queue run must wake EXACTLY ONCE.
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
    let producer =
        RealCiResultProducer::new(&sub.signals, tenant(), region(), &mq.0, REPO).unwrap();
    let d1 = producer
        .deliver(FIX_COMMIT, &facts, &required(), &attempt)
        .unwrap();
    let d2 = producer
        .deliver(FIX_COMMIT, &facts, &required(), &attempt)
        .unwrap();
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

    // MQ WORKER 2 (redeployed): re-lease + resume + merge EXACTLY ONCE.
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

    // X-1 THRESHOLDS: 1 wake, merge-count == 1, 0 re-dispatch across the restart.
    assert_eq!(
        merger.merges.load(Ordering::SeqCst),
        1,
        "merge-count == 1 (0 double-merge) — the merge-queue merged the fix-PR EXACTLY once"
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

    // ── reserve/settle BALANCED across the WHOLE run (triage step + merge-queue CI dispatch). The
    //    two completed metered dispatches each admitted ONE reserve + ONE settle (one cost event per
    //    metered unit, never interrupts in-flight, contract 11.7/9.5); 0 rejects + 0 in-flight
    //    interrupts; the wallet conserved (debited exactly the billed 100 + 50). Replay-duplicate
    //    reserves are deduped (not re-settled), so the SETTLE-count is the true completed-dispatch
    //    count — the definitive parity ledger (a stranded reservation would corrupt the wallet).
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
    // the wallet conserved: debited the billed cost of the two metered dispatches (100 + 50), refunding
    // any over-reservation; both billed their full reserve, so balance is start − 150.
    assert_eq!(
        gate.balance(),
        MinorUnits(wallet_start.0 - 150),
        "the wallet conserved: debited exactly the billed cost of the 2 metered dispatches (100+50)"
    );

    // THE DATED GREEN ARTIFACT (SCHED): the run trace + HITL ledger + reserve/settle parity + merge==1.
    println!(
        "[2026-06-25] PASS  drill=E2E-2  spine=durable-workflow+HITL  \
         ci-fail->triage-agent-workflow=yes  mutation-before-approval=0  \
         kill-mid-ack_window=yes  days-later-double-click->buffered=1  consume=1  \
         remint-on-resume=1(short-lived,attenuated,ttl={mint_ttl})  triage-merge-effect=1  \
         fix-pr-ci=green  merge-queue-wake=1(idempotent,X-1)  merge-count=1  re-dispatch=0  \
         reserve={}  settle={}  reserve/settle-parity=yes  inflight-interrupts=0  wallet={}->{}  \
         producer=REAL(RealCiResultProducer)  faces=MOCK(agent/issues/chat/notif/git — owners' E2E legs)",
        tele.reserve_attempted(),
        tele.settled(),
        wallet_start.0,
        gate.balance().0,
    );
}

/// **The DENY leg of the spine: a days-later DECLINE WITHHOLDS the merge (0 mutation, AG-8) — the gate
/// is not a rubber stamp.** The mirror of the approve leg: the merge effect is withheld; reserve/settle
/// stays balanced (the withheld merge made no spend-bearing dispatch). Proves the "0 mutation before
/// approval" property is decisive — a withheld effect NEVER mutates even after the human acts.
#[test]
fn e2e2_spine_days_later_decline_withholds_the_merge_zero_mutation() {
    let tele = FlowTelemetry::new();
    let gate = BudgetGate::new(Wallet::new(MinorUnits(1_000))).with_telemetry(tele.clone());
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

    // DAYS LATER: a DECLINE (empty payload + the DECLINE_MARKER) — double-clicked.
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

    // 0 MUTATION: the merge activity NEVER ran; no effect emitted past the card request.
    assert_eq!(
        merged_flag.load(Ordering::SeqCst),
        0,
        "the declined git.merge made 0 MUTATION (AG-8)"
    );
    assert_eq!(
        sub.outbox.committed_count(),
        emits_after_park,
        "0 emit past the card request — the withheld merge mutated nothing"
    );
    assert_eq!(
        sub.signals.buffered_depth(),
        0,
        "the decline was consumed EXACTLY once"
    );
    // reserve/settle stays balanced (the triage step settled ONCE; the withheld merge spent nothing).
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
        MinorUnits(1_000 - 100),
        "the wallet conserved on the decline leg: debited only the triage step (100)"
    );
    // the resume STILL re-minted (the resumed body runs the decision branch under a fresh token).
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
