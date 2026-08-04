//! # `ci_pipeline` — the CI-pipeline-as-workflow substrate + reference fixture (P-FLOW-22 → P-345, M4)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §4.9 (the
//! `SCHEDULE_AND_RUN_JOB` long-park-completed-by-signal idiom — "any long CI stage (CI
//! pipeline-as-workflow, the Phase-3 §11.7 question now answered)") + §2 (the flow-determinism
//! constraint: no clock/RNG/IO outside `WfCtx`) + "Changes vs Phase 3" item 6 (CI-pipeline-as-workflow
//! stage/step granularity ANSWERED via `SCHEDULE_AND_RUN_JOB` + the unified-runner `kind=ci` job spec,
//! X-6) + item 8 (reserve/settle fronts `SCHEDULE_AND_RUN_JOB` too).
//!
//! **Contract-index cluster:** OWNS the CI-pipeline SURFACE expressed over contract 9.2 (the `WfCtx`
//! `SCHEDULE_AND_RUN_JOB` idiom — each long CI stage is one long-park) + 9.4 (the `job.done` wait).
//! CONSUMES contract 11.7 (reserve/settle per stage — via [`WfCtx::metered_schedule_and_run_job`]) +
//! 1.6 (the `flow-determinism` lint, applied to the body) + 5.9 (CI's `CheckStatus`/`ci.result`
//! PRODUCER — AWAITED; CI's real pipeline definitions + the producer are CI's M4 deliverable, NOT this
//! prompt's).
//!
//! ## What this prompt (P-FLOW-22) ships — THE SUBSTRATE + A REFERENCE FIXTURE (NOT CI's pipelines)
//!
//! This is the **durable-execution substrate** CI's pipeline body sits on, plus a **reference
//! `ci.pipeline` workflow fixture** that proves the properties. It is NOT CI's pipeline definitions
//! and NOT the `CheckStatus` producer — those are CI's M4 deliverable (CI-P6/P7). This prompt builds
//! the pattern + the proof.
//!
//! The CI-pipeline-as-workflow PATTERN (§4.9, item 6): a CI pipeline is a deterministic [`WfCtx`]
//! workflow body whose **every long stage is a `SCHEDULE_AND_RUN_JOB` dispatch** (`kind=ci`) into the
//! unified runner. Each stage:
//!
//! 1. **Dispatches** the stage as a `kind=ci` job ([`WfCtx::metered_schedule_and_run_job`], §4.9
//!    step 1) — minting the deterministic `idem_token`, reserving the stage's budget at dispatch (no
//!    balance → the stage is NEVER handed to the runner), and RETURNING (the worker is freed for the
//!    multi-hour build).
//! 2. **Parks** on `job.done` (§4.9 step 2) holding NO runtime while the runner builds; a timeout
//!    timer bounds a vanished runner.
//! 3. **Resumes** on the journaled `job.done` (§4.9 step 3) — consume-exactly-once — settles the
//!    stage's budget (§4.9 step 4), reads the stage VERDICT (pass/fail) from the
//!    references-not-payloads result, and either advances to the next stage (pass) or **fails the
//!    pipeline fast** (fail) / **fails the pipeline** (a vanished-runner timeout).
//!
//! The whole body is FLOW-DETERMINISTIC: it reads NO clock/RNG/IO outside `WfCtx` (the
//! `flow-determinism` lint, contract 1.6, passes on the body — proven by
//! `tests/fixtures/ci_pipeline.flow.{green,red}.rs.txt` + `tests/lint_fixtures.rs`). The only
//! non-determinism it touches (the per-stage clock for the dispatch order, the runner verdict) flows
//! through the journaled `SCHEDULE_AND_RUN_JOB` activity + the journaled `job.done` signal — so a
//! REPLAY is BIT-IDENTICAL and ONLY a journaled `job.done` feeds the body (CI-D9). A killed runner +
//! control plane mid-run REPLAYS + idempotently re-dispatches (the `wf_signal` PK dedups, the activity
//! short-circuits) — effectively-once, 0 lost runs, 0 double-deploys, 0 duplicate publishes (CI-D1).
//!
//! ## references-not-payloads (the stage verdict)
//!
//! The runner reports a stage's verdict in the `job.done` signal's references-not-payloads result
//! (`Vec<ArtifactRef>`): [`stage_verdict_marker`] encodes `pass`/`fail` + the stage name into a single
//! PII-free `ArtifactRef`, and [`read_stage_verdict`] decodes it. No inline PII ever rides a CI-pipeline
//! stage signal — exactly the merge-queue's `ci.result` codec discipline ([`crate::merge_queue`]).
//!
//! ## NAMED FLOORS (recorded, not owned here)
//!
//! - **CI's real pipeline definitions + the `CheckStatus`/`ci.result` PRODUCER** are CI's M4
//!   deliverable (contract 5.9, CI-P6/P7). This prompt builds the substrate they sit on + a reference
//!   fixture; the real `.ci.yml`-derived pipeline shapes are CI's, never this engine's.
//! - **The dispatch into the unified runner is GATED by AG-D4** (the sandbox-escape drill,
//!   Agent-Fabric / CI-owned, `04-sandbox-AG-D4.md`). The [`crate::JobRunner`] is the seam the engine
//!   calls; the production binding (onto `ToolHands::exec`, contract 8.4) lands behind that gate. The
//!   reference fixture drills against a recording runner fixture, never live untrusted code.

use crate::job::{JobKind, JobOutcome, JobRunner, JobSpec};
use crate::wfctx::{WfCtx, WfError, WfResult};
use myelin_refs::ArtifactRef;
use myelin_storage::reserve_settle::{MeteredUnit, MicroUsd};

/// **The FROZEN `wf_type` the reference CI-pipeline workflow registers under (§4.9, item 6).** A CI
/// pipeline is "ONE workflow per pipeline-run" registered under this definition name; CI's real
/// definitions register the SAME name (CI owns the per-pipeline body shape, M4). Pinned here so the
/// dispatcher + the executor agree on the registered name by construction.
pub const CI_PIPELINE_WF_TYPE: &str = "ci.pipeline";

/// **One stage of a CI pipeline (§4.9, item 6) — references-not-payloads.** A stage is one long
/// `kind=ci` job (a `build` / `test` / `lint` / `deploy` step): a `SCHEDULE_AND_RUN_JOB` long-park.
/// Every field is a PII-free machine token (a stage name, an opaque pipeline-step target ref, a cost
/// in minor-units). The runner (CI) owns the `target`'s grammar; the engine carries it opaquely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiStage {
    /// The stage name (`build` / `test` / `lint` / `deploy` …) — a PII-free routing/label token. It is
    /// the verdict marker's key (the runner echoes it in the `job.done` result so the body attributes
    /// the verdict to the right stage).
    pub name: String,
    /// The opaque step target the unified runner is handed (references-not-payloads): a CI pipeline-step
    /// ref (an `ArtifactRef`-class identifier). CI owns the shape; the engine carries it opaquely.
    pub target: String,
    /// **The stage's cost reserved at dispatch (contract 11.7, §4.9 step 1).** Reserved BEFORE the
    /// stage is handed to the runner — no balance → the stage is NEVER dispatched (the pipeline fails
    /// loud). Settled on the consumed `job.done` (§4.9 step 4). `MicroUsd(0)` runs the stage
    /// un-metered (the loop-cap depth is then the runaway bound, AG-6).
    pub cost: MicroUsd,
    /// **The stage's max-duration SLA in seconds (§4.9 step 2).** Arms the timeout timer that bounds a
    /// vanished runner — a runner that never reports does NOT park the pipeline forever; the timeout
    /// fails the stage (which fails the pipeline). `None` waits indefinitely (only for a stage whose
    /// completion is otherwise guaranteed).
    pub timeout_secs: Option<i64>,
}

impl CiStage {
    /// Build a metered stage (`name`/`target`) with a `cost` + a `timeout_secs` SLA.
    pub fn new(
        name: impl Into<String>,
        target: impl Into<String>,
        cost: MicroUsd,
        timeout_secs: Option<i64>,
    ) -> Self {
        Self {
            name: name.into(),
            target: target.into(),
            cost,
            timeout_secs,
        }
    }
}

/// **A CI pipeline = an ORDERED sequence of stages (§4.9, item 6).** The reference fixture's input. CI's
/// real definitions build their OWN `CiPipelineSpec` per `.ci.yml`-derived pipeline (CI's M4 detail) —
/// this is the substrate shape they target. The stages run IN ORDER, fail-fast (a failed stage stops
/// the pipeline; later stages are NOT dispatched — 0 wasted spend on a doomed run).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiPipelineSpec {
    /// The ordered stages — dispatched one after another, each its own `SCHEDULE_AND_RUN_JOB` long-park.
    pub stages: Vec<CiStage>,
}

impl CiPipelineSpec {
    /// Build a pipeline from its ordered stages.
    pub fn new(stages: Vec<CiStage>) -> Self {
        Self { stages }
    }
}

/// **The terminal outcome of a CI-pipeline-as-workflow run (§4.9, item 6).** A pipeline either SUCCEEDS
/// (every stage's `job.done` reported `pass`), FAILS at a named stage (a stage's `job.done` reported
/// `fail` — fail-fast, the later stages were never dispatched), TIMES OUT at a named stage (a vanished
/// runner — the timeout bounded the wait), or PARKS (a stage is dispatched + the runner is running it;
/// the body returns promptly, the dispatcher re-drives it when `job.done` arrives).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineOutcome {
    /// **Every stage passed.** The pipeline is green — all stages' `job.done` reported `pass`, each
    /// consumed exactly once, each settled. Carries the count of completed stages (the proof every
    /// stage ran to completion).
    Succeeded {
        /// the number of stages that completed `pass` (= `stages.len()` on a green run).
        stages_completed: usize,
    },
    /// **A stage FAILED — the pipeline stopped fast at it (§4.9 step 4 error branch).** Carries the
    /// failed stage's name (PII-free). The later stages were NEVER dispatched (0 wasted spend). This is
    /// the pipeline's deterministic error branch, exactly like a failed synchronous activity.
    Failed {
        /// the name of the stage whose `job.done` reported `fail`.
        stage: String,
    },
    /// **A stage's runner VANISHED — the timeout timer fired (§4.9 step 2).** Carries the timed-out
    /// stage's name. A vanished runner does NOT park the pipeline forever; the timeout fails the stage,
    /// which fails the pipeline (the body retries / a higher layer re-runs).
    TimedOut {
        /// the name of the stage whose runner vanished (the timeout fired before `job.done`).
        stage: String,
    },
    /// **A stage is DISPATCHED + the workflow PARKED on its `job.done` (§4.9 step 2).** The run is
    /// `waiting`, holding NO runtime, until the runner reports. The body returns promptly; the dispatcher
    /// re-drives it when the signal arrives (and the body replays the journaled prefix to this stage).
    Parked,
}

/// **The references-not-payloads stage VERDICT marker (§3.4).** The runner reports a stage's pass/fail
/// in the `job.done` signal's `Vec<ArtifactRef>` result. This encodes `(pass|fail, stage_name)` into a
/// single PII-free `ArtifactRef` — exactly the merge-queue's `encode_ci_result` discipline
/// ([`crate::merge_queue`]): a machine token, never an inline PII body. Exposed so a runner fixture /
/// CI's real producer encodes the SAME shape the body decodes.
pub fn stage_verdict_marker(stage: &str, pass: bool) -> ArtifactRef {
    let verdict = if pass { "pass" } else { "fail" };
    ArtifactRef(format!("ci.stage.verdict:{verdict}:{stage}"))
}

/// **Decode a stage VERDICT from the `job.done` references-not-payloads result.** Reads the first
/// [`stage_verdict_marker`] off the result refs and returns `(stage_name, pass)`. A result that carries
/// no verdict marker (a runner protocol violation) returns `None` — the body surfaces it as a LOUD error
/// (EI-01 §2: never a silent wrong verdict). The decode is deterministic (a pure function of the
/// journaled signal), so replay reads back the SAME verdict.
pub fn read_stage_verdict(result: &[ArtifactRef]) -> Option<(String, bool)> {
    for r in result {
        if let Some(rest) = r.0.strip_prefix("ci.stage.verdict:") {
            let (verdict, stage) = rest.split_once(':')?;
            return match verdict {
                "pass" => Some((stage.to_string(), true)),
                "fail" => Some((stage.to_string(), false)),
                _ => None,
            };
        }
    }
    None
}

impl WfCtx {
    /// **`run_ci_pipeline(spec, runner)` (the CI-pipeline-as-workflow PATTERN, §4.9 item 6) — the
    /// reference fixture body the CI M4 build targets.** A deterministic [`WfCtx`] workflow body whose
    /// every long stage is a `SCHEDULE_AND_RUN_JOB` (`kind=ci`) long-park with reserve/settle per stage.
    ///
    /// **Per stage, IN ORDER (fail-fast):**
    /// 1. **Dispatch + park + settle** via [`WfCtx::metered_schedule_and_run_job`] (§4.9): reserve the
    ///    stage's `cost` at dispatch (no balance → the stage is NEVER handed to the runner, the pipeline
    ///    fails loud), dispatch the `kind=ci` job, park on `job.done` holding NO runtime (a timeout timer
    ///    bounds a vanished runner), and settle the stage on the consumed `job.done`.
    /// 2. **Read the verdict** ([`read_stage_verdict`]) off the references-not-payloads result:
    ///    - `pass` → ADVANCE to the next stage;
    ///    - `fail` → STOP fast → [`PipelineOutcome::Failed`] (the later stages are NEVER dispatched — 0
    ///      wasted spend on a doomed run);
    ///    - a `JobOutcome::TimedOut` (a vanished runner) → [`PipelineOutcome::TimedOut`];
    ///    - a `JobOutcome::Parked` (the stage is running) → [`PipelineOutcome::Parked`] (the body returns
    ///      promptly; the dispatcher re-drives it when `job.done` arrives).
    ///
    /// **Determinism (the CI-D9 property).** The body reads NO clock/RNG/IO outside `WfCtx`: the only
    /// non-determinism is the runner's verdict, which flows through the journaled
    /// `SCHEDULE_AND_RUN_JOB` activity + the journaled `job.done` signal. A REPLAY is BIT-IDENTICAL (the
    /// `idem_token` re-derives identically, the dispatch short-circuits, the `job.done` consume
    /// short-circuits) and ONLY a journaled `job.done` feeds the body.
    ///
    /// **Crash recovery (the CI-D1 property).** A killed runner + control plane mid-run replays the
    /// journaled prefix (0 re-dispatch — the activity short-circuits) and idempotently re-dispatches the
    /// un-journaled stage (the runner dedups on the deterministic `idem_token`); a double-delivered
    /// `job.done` wakes the run ONCE (the `wf_signal` PK dedup). Effectively-once: 0 lost runs, 0
    /// double-deploys, 0 duplicate publishes.
    ///
    /// **NAMED FLOORS (recorded, not owned here):** CI's real pipeline definitions + the
    /// `CheckStatus`/`ci.result` producer are CI's M4 deliverable (contract 5.9); the dispatch into
    /// `runner` is GATED by AG-D4 (no untrusted code runs until the sandbox-escape gate is green).
    pub fn run_ci_pipeline<R>(
        &mut self,
        spec: &CiPipelineSpec,
        runner: &R,
    ) -> WfResult<PipelineOutcome>
    where
        R: JobRunner,
    {
        let mut stages_completed = 0usize;
        for stage in &spec.stages {
            // ── DISPATCH + PARK + SETTLE one stage as a `kind=ci` SCHEDULE_AND_RUN_JOB long-park (§4.9).
            // The metered idiom reserves the stage's cost at dispatch (no balance → the stage is NEVER
            // dispatched), dispatches the job, parks holding no runtime, and settles on the consumed
            // job.done. The stage's name + cost ride the reserve as a single metered unit (the per-stage
            // meter the §4.9 item-8 reserve/settle bookend fronts). The dispatch into `runner` is the
            // contract-8.4 ToolHands::exec seam — GATED by AG-D4.
            let units = vec![MeteredUnit {
                unit: "ci.stage",
                wholesale: stage.cost,
                markup: MicroUsd(0),
            }];
            let outcome = self.metered_schedule_and_run_job(
                JobSpec::new(JobKind::Ci, stage.target.clone()),
                runner,
                stage.timeout_secs,
                stage.cost,
                units,
            )?;

            match outcome {
                JobOutcome::Completed { result, .. } => {
                    // The runner echoed the stage VERDICT in the references-not-payloads result. A
                    // missing verdict marker is a runner protocol violation → a LOUD error (never a
                    // silent wrong verdict, EI-01 §2). On replay the journaled job.done carries the same
                    // result, so this decode is replay-stable (the BIT-IDENTICAL property).
                    let (verdict_stage, pass) = read_stage_verdict(&result).ok_or_else(|| {
                        WfError::CoCommit(format!(
                            "ci.pipeline stage `{}` job.done carried no verdict marker (the \
                                 runner did not report pass/fail, §4.9)",
                            stage.name
                        ))
                    })?;
                    // The runner MUST attribute the verdict to the dispatched stage — a mismatch is a
                    // protocol violation (the runner reported the wrong stage's verdict). Loud, never
                    // silent.
                    if verdict_stage != stage.name {
                        return Err(WfError::CoCommit(format!(
                            "ci.pipeline stage `{}` job.done reported a verdict for stage \
                             `{verdict_stage}` (the runner mis-attributed the verdict, §4.9)",
                            stage.name
                        )));
                    }
                    if !pass {
                        // FAIL-FAST: the stage failed → STOP. The later stages are NEVER dispatched (0
                        // wasted spend). This is the pipeline's deterministic error branch.
                        return Ok(PipelineOutcome::Failed {
                            stage: stage.name.clone(),
                        });
                    }
                    // PASS: advance to the next stage.
                    stages_completed += 1;
                }
                JobOutcome::TimedOut => {
                    // A vanished runner: the timeout fired before job.done. Fail the stage → fail the
                    // pipeline (never park forever).
                    return Ok(PipelineOutcome::TimedOut {
                        stage: stage.name.clone(),
                    });
                }
                JobOutcome::Parked => {
                    // The stage is dispatched + the runner is running it: the run parked (holds no
                    // runtime). Return promptly — the dispatcher re-drives when job.done arrives, and the
                    // body replays the journaled prefix back to THIS stage.
                    return Ok(PipelineOutcome::Parked);
                }
            }
        }
        // Every stage passed → the pipeline is green.
        Ok(PipelineOutcome::Succeeded { stages_completed })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{SignalRow, SignalStore};
    use crate::{BudgetGate, Wallet, WfJournal};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
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
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }
    fn minter() -> std::sync::Arc<dyn IdMinter> {
        std::sync::Arc::new(MonotonicMinter::new())
    }

    /// A CI runner fixture (the contract-8.4 `ToolHands::exec` consumer side, §4.9). Records each
    /// dispatched stage spec (so a test asserts the deterministic `idem_token` + the `kind=ci` routing)
    /// and counts dispatches (so a replay's 0-re-dispatch is provable).
    #[derive(Default)]
    struct RecordingCiRunner {
        dispatched: Mutex<Vec<JobSpec>>,
        calls: AtomicUsize,
    }
    impl JobRunner for RecordingCiRunner {
        fn dispatch(&self, spec: &JobSpec) -> Result<(), crate::ActivityError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                spec.kind,
                JobKind::Ci,
                "a CI pipeline dispatches kind=ci jobs"
            );
            self.dispatched.lock().unwrap().push(spec.clone());
            Ok(())
        }
    }

    fn pipeline() -> CiPipelineSpec {
        CiPipelineSpec::new(vec![
            CiStage::new(
                "build",
                "pipeline://acme/ci/pr-7#build",
                MicroUsd(10),
                Some(3600),
            ),
            CiStage::new(
                "test",
                "pipeline://acme/ci/pr-7#test",
                MicroUsd(20),
                Some(3600),
            ),
            CiStage::new(
                "lint",
                "pipeline://acme/ci/pr-7#lint",
                MicroUsd(5),
                Some(600),
            ),
        ])
    }

    /// Begin a metered `ci.pipeline` WfCtx with a wallet, signal buffer + timer wheel (the full surface
    /// a stage long-park needs).
    fn begin_metered(
        outbox: &OutboxStore,
        journal: WfJournal,
        signals: SignalStore,
        timers: crate::TimerStore,
        balance: MicroUsd,
        now_secs: i64,
    ) -> WfCtx {
        WfCtx::begin(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            CI_PIPELINE_WF_TYPE,
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_signals(signals)
        .with_timers(timers, 0, now_secs)
        .with_budget(BudgetGate::new(Wallet::new(balance)))
    }

    /// Deliver a stage's `job.done` (the runner's completion) carrying the verdict marker, keyed by the
    /// deterministic dispatch `idem_token`.
    fn deliver_stage_done(signals: &SignalStore, idem_token: &str, stage: &str, pass: bool) {
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: crate::JOB_DONE_SIGNAL.into(),
            idem_key: idem_token.into(),
            payload: vec![stage_verdict_marker(stage, pass)],
            payload_key_ref: None,
            received_unix_ms: 0,
            consumed_seq: None,
        });
    }

    /// The dispatch `idem_token` for the Nth stage (the Nth `SCHEDULE_AND_RUN_JOB` long-park). Each
    /// stage consumes TWO command positions: the dispatch activity (`2*idx`) + the `wait_for_signal`
    /// (`2*idx + 1`). The dispatch idem_token is keyed on the DISPATCH position (`2*idx`), so the runner
    /// echoes `R1/ci.pipeline:<2*idx>/job` for stage `idx`.
    fn stage_token(stage_idx: usize) -> String {
        crate::job_idem_token("R1", &format!("{CI_PIPELINE_WF_TYPE}:{}", stage_idx * 2))
    }

    /// **The reference fixture body PARKS at the first stage (holds no runtime) — no `job.done` yet.**
    /// The CI-pipeline body dispatches stage `build` (one `kind=ci` job), reserves its budget, and parks
    /// on `job.done` — it does NOT block on the multi-hour build.
    #[test]
    fn pipeline_parks_at_the_first_stage_holding_no_runtime() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        let mut ctx = begin_metered(&outbox, journal, signals, timers, MicroUsd(1000), 1000);
        let out = ctx
            .run_ci_pipeline(&pipeline(), &runner)
            .expect("dispatch the first stage + park");

        assert_eq!(
            out,
            PipelineOutcome::Parked,
            "parks on the build stage's job.done"
        );
        assert!(
            ctx.parked_on_signal(),
            "the run holds no runtime (state=waiting)"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            1,
            "ONE stage dispatched"
        );
        let dispatched = runner.dispatched.lock().unwrap();
        assert_eq!(dispatched[0].kind, JobKind::Ci, "kind=ci");
        assert_eq!(
            dispatched[0].idem_token,
            stage_token(0),
            "the deterministic dispatch idem_token (the build stage)"
        );
    }

    /// **Every stage passes → the pipeline SUCCEEDS, each stage's budget reserved + settled (§4.9).**
    /// With all three stages' `job.done` buffered (`pass`), one drive runs the whole pipeline:
    /// dispatch+park+settle each stage in order; the wallet is debited the three reserves and refunded
    /// the over-reservation (settle of 0 units → full refund of each stage's reserve).
    #[test]
    fn all_stages_pass_pipeline_succeeds_each_stage_metered() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        // all three stages' job.done buffered green (the runner already finished each).
        deliver_stage_done(&signals, &stage_token(0), "build", true);
        deliver_stage_done(&signals, &stage_token(1), "test", true);
        deliver_stage_done(&signals, &stage_token(2), "lint", true);

        let mut ctx = begin_metered(&outbox, journal, signals, timers, MicroUsd(1000), 1000);
        let out = ctx
            .run_ci_pipeline(&pipeline(), &runner)
            .expect("the whole pipeline runs green");

        assert_eq!(
            out,
            PipelineOutcome::Succeeded {
                stages_completed: 3
            },
            "all three stages passed"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            3,
            "THREE stages dispatched, one each"
        );
        assert_eq!(
            ctx.consumed_signals().len(),
            3,
            "THREE job.done consumed, one per stage"
        );
    }

    /// **A failing stage STOPS the pipeline fast — the later stages are NEVER dispatched (0 wasted
    /// spend, §4.9 error branch).** The `test` stage reports `fail`; `lint` is never dispatched.
    #[test]
    fn a_failing_stage_stops_the_pipeline_fast_later_stages_never_dispatched() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        deliver_stage_done(&signals, &stage_token(0), "build", true);
        deliver_stage_done(&signals, &stage_token(1), "test", false); // test FAILS
                                                                      // NOTE: lint's job.done is NOT delivered — it must never be dispatched.

        let mut ctx = begin_metered(&outbox, journal, signals, timers, MicroUsd(1000), 1000);
        let out = ctx
            .run_ci_pipeline(&pipeline(), &runner)
            .expect("the pipeline fails fast at test");

        assert_eq!(
            out,
            PipelineOutcome::Failed {
                stage: "test".into()
            },
            "the pipeline failed at the test stage"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            2,
            "ONLY build + test dispatched — lint was NEVER dispatched (0 wasted spend)"
        );
    }

    /// **A vanished runner's timeout fails the stage → the pipeline times out (§4.9 step 2).** The build
    /// stage is dispatched but the runner never reports. Drive 1 parks with a timeout; drive 2 (past the
    /// deadline) STILL has no job.done → the pipeline returns [`PipelineOutcome::TimedOut`].
    #[test]
    fn a_vanished_runner_times_the_stage_out_and_fails_the_pipeline() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        // DRIVE 1 at clock=1000 with the build stage's 3600s SLA → dispatch + park.
        let mut c1 = begin_metered(
            &outbox,
            journal.clone(),
            signals.clone(),
            timers.clone(),
            MicroUsd(1000),
            1000,
        );
        let out1 = c1
            .run_ci_pipeline(&pipeline(), &runner)
            .expect("dispatch + park");
        assert_eq!(out1, PipelineOutcome::Parked, "parked on build's job.done");
        c1.commit()
            .expect("co-commit the dispatch + the timeout-timer");
        let history = journal.history_for(&tenant(), "R1");

        // DRIVE 2 at clock=10000 (past the 1000+3600 deadline), STILL no job.done → TimedOut.
        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            CI_PIPELINE_WF_TYPE,
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals)
        .with_timers(timers, 0, 10_000)
        .with_budget(BudgetGate::new(Wallet::new(MicroUsd(1000))));
        let out2 = c2
            .run_ci_pipeline(&pipeline(), &runner)
            .expect("the timeout drive");
        assert_eq!(
            out2,
            PipelineOutcome::TimedOut {
                stage: "build".into()
            },
            "the build runner vanished → the pipeline timed out"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            1,
            "the build stage dispatched ONCE — the replay did not re-dispatch it"
        );
    }

    /// **A double-delivered stage `job.done` wakes the pipeline ONCE (the `wf_signal` PK dedup, §4.9).**
    /// The runner delivers the build stage's `job.done` TWICE (at-least-once); the buffer holds ONE row;
    /// the pipeline consumes it EXACTLY once. 1 wake per stage → 0 double-deploy.
    #[test]
    fn a_double_delivered_stage_done_wakes_the_pipeline_once() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        // a SINGLE-stage pipeline (build only) so the double-delivery is the whole run.
        let single = CiPipelineSpec::new(vec![CiStage::new(
            "build",
            "pipeline://acme/ci/pr-7#build",
            MicroUsd(10),
            Some(3600),
        )]);

        deliver_stage_done(&signals, &stage_token(0), "build", true);
        deliver_stage_done(&signals, &stage_token(0), "build", true); // DOUBLE delivery
        assert_eq!(
            signals.buffered_depth(),
            1,
            "the double delivery deduped to ONE row"
        );

        let mut ctx = begin_metered(
            &outbox,
            journal,
            signals.clone(),
            timers,
            MicroUsd(1000),
            1000,
        );
        let out = ctx
            .run_ci_pipeline(&single, &runner)
            .expect("the pipeline completes");
        assert_eq!(
            out,
            PipelineOutcome::Succeeded {
                stages_completed: 1
            }
        );
        assert_eq!(
            ctx.consumed_signals().len(),
            1,
            "ONE wake (the double-delivery deduped)"
        );
        assert_eq!(signals.buffered_depth(), 0, "the one row consumed once");
    }

    /// **A runner that reports the WRONG stage's verdict is a LOUD error (§4.9, EI-01 §2).** The runner
    /// must attribute the verdict to the dispatched stage; a mis-attribution surfaces a CoCommit error,
    /// never a silent advance on the wrong stage's verdict.
    #[test]
    fn a_mis_attributed_stage_verdict_is_a_loud_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        // the runner echoed a verdict for "the-wrong-stage" under the build token.
        deliver_stage_done(&signals, &stage_token(0), "the-wrong-stage", true);

        let mut ctx = begin_metered(&outbox, journal, signals, timers, MicroUsd(1000), 1000);
        let err = ctx
            .run_ci_pipeline(&pipeline(), &runner)
            .expect_err("a mis-attributed verdict is loud");
        assert!(
            matches!(err, WfError::CoCommit(ref m) if m.contains("mis-attributed the verdict")),
            "the mis-attribution is a loud CoCommit error, got {err:?}"
        );
    }

    /// **No balance → a stage is NEVER dispatched (reserve fronts the dispatch, §4.9 step 1).** A wallet
    /// that cannot afford the build stage's cost refuses the reserve; the runner is NEVER called; the
    /// pipeline fails loud (0 dispatch on an exhausted wallet).
    #[test]
    fn no_balance_means_the_stage_is_never_dispatched() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        // wallet has 5 minor-units; the build stage costs 10 → refused at reserve.
        let mut ctx = begin_metered(&outbox, journal, signals, timers, MicroUsd(5), 1000);
        let err = ctx
            .run_ci_pipeline(&pipeline(), &runner)
            .expect_err("an exhausted wallet refuses the dispatch");
        assert!(
            matches!(err, WfError::CoCommit(ref m) if m.contains("never dispatched")),
            "the refused reserve is loud, got {err:?}"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            0,
            "the runner was NEVER called (no balance → no dispatch)"
        );
    }

    /// **The verdict codec round-trips (references-not-payloads).** [`stage_verdict_marker`] /
    /// [`read_stage_verdict`] are inverse; a result with no marker decodes to `None` (the loud-error
    /// trigger).
    #[test]
    fn stage_verdict_codec_round_trips() {
        assert_eq!(
            read_stage_verdict(&[stage_verdict_marker("build", true)]),
            Some(("build".to_string(), true))
        );
        assert_eq!(
            read_stage_verdict(&[stage_verdict_marker("test", false)]),
            Some(("test".to_string(), false))
        );
        assert_eq!(
            read_stage_verdict(&[ArtifactRef("not-a-verdict".into())]),
            None,
            "a non-verdict result decodes to None (the loud-error trigger)"
        );
    }
}
