//! # `executor` — `DurableExecutor` start/describe/cancel + the engine telemetry set (P-FLOW-06 → P-203, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §5.1 (the `DurableExecutor`
//! trait: `start`, `signal`, `describe`, `cancel` — `StartSpec{wf_type, input, budget, idem_key}`;
//! `signal` lands later) + §5.4 (the telemetry contract — the drill-survival signals on the
//! metrics-health port) + §4.6 (the `wf_version` PIN at start). Carried forward from Phase-3 §5.1.
//!
//! **Contract-index cluster:** OWNS the control half of 9.1 (`DurableExecutor{start, describe,
//! cancel}` — the `signal` method is the named M2.3 follow-on **P-FLOW-09**). Wires the contract-1.8
//! engine telemetry set: runnable-run lag + replay rate (from [`crate::engine::FlowTelemetry`]) +
//! activity queue depth + retry + dead-letter (added to [`FlowTelemetry`] here).
//!
//! ## What this prompt (P-FLOW-06) ships — the control-plane surface
//!
//! [`DurableExecutor`] is the engine-agnostic control surface bus automations / Agent / CI / Issues
//! call to **start and steer** a durable run. It sits ON TOP of the P-FLOW-05 replay/lease engine
//! ([`crate::engine`]): `start` inserts a runnable [`RunRow`] the [`FlowDispatcher`] then leases and
//! drives; `describe` reads the run's lifecycle; `cancel` transitions a non-terminal run to
//! `terminated` so the dispatcher never drives it again.
//!
//! - [`DurableExecutor::start`] — `StartSpec{wf_type, input, budget, idem_key} → RunId`. **Idempotent
//!   on `idem_key`** (FROZEN, contract 9.1): a re-start with the SAME key returns the SAME [`RunId`],
//!   never a second run (a redelivered trigger is one workflow, not two). The `input` is stored as a
//!   REFERENCE (`Vec<ArtifactRef>` — references-not-payloads, §3.1), never an inline PII body. The
//!   run is seeded `running` at cursor 0, partitioned by `hash(run_id) % N` (§7.2), `wf_version`
//!   pinned at start (§4.6).
//! - [`DurableExecutor::describe`] — `RunId → RunStatus`: the run's `state` + `cursor` + pinned
//!   `wf_version` + terminality. The control plane reads it to poll a run / correlate a trigger.
//! - [`DurableExecutor::cancel`] — `RunId, reason → ()`: transitions a NON-terminal run to
//!   `terminated` (and releases its lease) so the dispatcher never drives it again. Idempotent on a
//!   terminal run (a cancel of a completed/cancelled run is a no-op — never re-terminates).
//!
//! The engine telemetry set (contract 1.8 / §5.4) is read off [`FlowExecutor::telemetry`]: the
//! runnable-run lag + replay rate (the P-FLOW-05 signals) PLUS the activity queue depth + retry +
//! dead-letter (added to [`FlowTelemetry`] for this prompt). The timer/signal/budget signals are
//! added by their owning prompts (P-FLOW-09/13).
//!
//! ## FLOORS named
//!
//! - **`DurableExecutor::signal`** (the inbound-signal half of 9.1 — HITL approval / `cancel` /
//!   `ci.result` / `job.done`, idempotent on `idem_key`) → the M2.3 follow-on **P-FLOW-09**. This
//!   prompt ships the start/describe/cancel control half ONLY (the §5.1 surface's first/third/fourth
//!   methods).
//! - **The boot-time definition registry** (`wf_definition`, §3.6 — the versioned `wf_type → body`
//!   registry the executor pins `wf_version` from). Modeled here as the in-memory body registry the
//!   caller supplies via [`FlowExecutor::register_definition`]; the persisted `wf_definition` row +
//!   drift-detection by `code_hash` is the named M5 follow-on. `start` pins `wf_version = 1` for a
//!   registered body until then.
//! - **The live OLTP binding** — the executor drives the SAME in-memory [`RunStore`] +
//!   [`crate::wfctx::WfJournal`] the P-FLOW-05 engine models over the substrate's transactional
//!   primitives (dev↔prod is a config swap, never a code change). The live-PG lease/replay apply is
//!   exercised in `tests/integration_flow_replay.rs`; the control-surface insert/read/cancel rides
//!   the same `workflow_run` shape ([`crate::schema::WorkflowRunRow`]).
//! - **The timer/signal-buffer + per-tenant in-flight + causal-depth telemetry signals** (§5.4) are
//!   added by the prompts that ship those surfaces (P-FLOW-09/13/…); this prompt adds the
//!   activity-queue/retry/dead-letter leg + confirms the runnable-lag/replay-rate leg.

use crate::engine::{run_state, FlowTelemetry, RunRow, RunStore};
use myelin_events::IdMinter;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The number of worker partitions a run is hashed into (`partition = hash(run_id) % N`, §7.2). A
/// per-cell constant; the executor stamps it on the seeded run so the dispatcher's partition-scoped
/// lease scan finds it. A small fixed fan-out at this milestone (the M5 scale prompt tunes it).
pub const PARTITION_COUNT: u32 = 16;

/// **The run BUDGET the workflow owns** (contract 9.1 / §5.1 / §6.2). Integer **minor-units** (never
/// floats, §5.1) — the cost ceiling the run reserves/settles against. Optional on a [`StartSpec`]: a
/// run with no budget runs un-metered (the engine still owns the loop-cap depth, AG-6). The
/// reserve/settle enforcement is the cost-safety prompt's; this is the frozen carrier shape `start`
/// stores on the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunBudget {
    /// the total cost ceiling in integer minor-units (e.g. cents, or token-units) — never a float.
    pub minor_units: i64,
}

/// **The `DurableExecutor::start` spec** (contract 9.1 / §5.1, FROZEN shape). `wf_type` names the
/// registered definition to run; `input` is **references-not-payloads** (`ArtifactRef`s, never a PII
/// body — §3.1); `budget` is the optional owned [`RunBudget`]; `idem_key` makes `start`
/// effectively-once (a redelivered start is one run).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartSpec {
    /// the registered definition name (e.g. `agent.run`, `ci.pipeline`) — the body the engine drives.
    pub wf_type: String,
    /// the run input as **`ArtifactRef`s** (references-not-payloads, §3.1) — the workflow about a PR
    /// carries the PR's ref, never the PR body. Personal data stays in the owning subsystem's
    /// erasable store (erasure-for-free).
    pub input: Vec<ArtifactRef>,
    /// the optional owned budget (integer minor-units, never floats — §5.1).
    pub budget: Option<RunBudget>,
    /// the per-effect idempotency key (FROZEN, contract 9.1): a re-`start` with this key returns the
    /// SAME [`RunId`], never a second run. The caller derives it (e.g. `<rule_id>:<event_id>`).
    pub idem_key: String,
}

/// **The durable run handle** `start` returns (contract 9.1 — `start` returns a durable handle). The
/// ULID-ordered opaque run id the control plane carries to `describe`/`cancel` and the firing audit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub String);

/// **The run status `describe` returns** (contract 9.1 — `describe(RunId) → RunStatus`). The run's
/// lifecycle `state` + replay `cursor` + pinned `wf_version` (§4.6) + the derived `terminal` flag.
/// References-not-payloads: it carries NO input/result body — only the lifecycle the control plane
/// polls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunStatus {
    /// the run this status describes.
    pub run_id: RunId,
    /// the registered definition name the run drives.
    pub wf_type: String,
    /// the ONE lifecycle state (running|waiting|completed|failed|terminated|nondeterministic) — §3.1.
    pub state: String,
    /// the highest applied history seq (the replay cursor floor, §3.1).
    pub cursor: i64,
    /// the definition version PINNED at start (§4.6) so a deploy cannot diverge an in-flight run.
    pub wf_version: i32,
    /// whether the run is TERMINAL (it will never be driven again — `run_state::is_terminal`).
    pub terminal: bool,
}

/// **The `DurableExecutor` errors** (the 9.1 control-surface failures — surfaced, never swallowed,
/// EI-02 §4). A start of an unknown `wf_type`, or a describe/cancel of an unknown run, is an
/// observable error the caller can retry/alert on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutorError {
    /// `start` named a `wf_type` with no registered definition (the body the engine would drive does
    /// not exist) — surfaced so the misconfiguration is observable, never a silent dropped run.
    UnknownWorkflow(String),
    /// `describe`/`cancel` named a `RunId` the executor has no record of.
    UnknownRun(String),
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutorError::UnknownWorkflow(t) => write!(f, "unknown workflow type: {t}"),
            ExecutorError::UnknownRun(r) => write!(f, "unknown run: {r}"),
        }
    }
}

impl std::error::Error for ExecutorError {}

/// **The engine-agnostic durable-execution control surface** (contract 9.1, §5.1 — the escape-hatch
/// seam, §2). The control half this prompt owns: `start` / `describe` / `cancel`. The `signal`
/// method (the inbound-signal half) is the named follow-on **P-FLOW-09**, added to this trait there.
///
/// This is the surface bus automations (`action.kind = workflow`), Agent Fabric (a HITL-gated run),
/// CI (a pipeline), and Issues (an SLA workflow) call to start + steer a durable run. It is
/// engine-AGNOSTIC: the consumer depends on this trait, never on a concrete engine (the §2.9
/// DAG-respecting seam).
pub trait DurableExecutor {
    /// **Start (or no-op-return-the-existing) a durable run** for `spec`, idempotent on
    /// `spec.idem_key` (FROZEN, contract 9.1). A re-`start` with the same key returns the SAME
    /// [`RunId`] — a redelivered trigger is ONE run, not two. The `input` is stored references-not-
    /// payloads. Returns [`ExecutorError::UnknownWorkflow`] if `spec.wf_type` has no registered
    /// definition (surfaced, never a silent dropped run).
    fn start(&self, spec: StartSpec) -> Result<RunId, ExecutorError>;

    /// **Describe a run** — its lifecycle `state` + `cursor` + pinned `wf_version` + terminality
    /// (contract 9.1). Returns [`ExecutorError::UnknownRun`] for an unknown handle.
    fn describe(&self, run: &RunId) -> Result<RunStatus, ExecutorError>;

    /// **Cancel a run** — transition a NON-terminal run to `terminated` (releasing its lease) so the
    /// dispatcher never drives it again (contract 9.1). Idempotent on a terminal run (a cancel of a
    /// completed/cancelled run is a no-op return, never a re-terminate). `reason` is recorded for
    /// audit (a machine reason, no PII). Returns [`ExecutorError::UnknownRun`] for an unknown handle.
    fn cancel(&self, run: &RunId, reason: &str) -> Result<(), ExecutorError>;
}

/// One started run's control-plane record (the idempotency anchor + the references-not-payloads
/// `StartSpec` carriage). Keyed by `idem_key` (so a re-start is a no-op return) AND reachable by
/// `RunId` (so describe/cancel resolve). Carries NO PII — `input` is `ArtifactRef`s.
#[derive(Clone, Debug)]
struct StartedRun {
    run_id: RunId,
    wf_type: String,
    input: Vec<ArtifactRef>,
    budget: Option<RunBudget>,
    wf_version: i32,
    /// the last-recorded cancel reason (audit; None until cancelled) — a machine reason, no PII.
    cancel_reason: Option<String>,
}

/// **The `myelin-flow` `DurableExecutor` implementation — the control-plane over the replay/lease
/// engine.** Holds the [`RunStore`] the [`crate::engine::FlowDispatcher`] leases from (so a `start`
/// seeds a runnable run the dispatcher then drives), the [`FlowTelemetry`] the metrics-health port
/// reads (contract 1.8), and the per-`idem_key` started-run record (the idempotency anchor). A
/// cloneable handle (shared `Arc<Mutex<…>>` state) so the control surface and the worker loop share
/// one run store.
///
/// **Wiring:** build the executor + the dispatcher over the SAME [`RunStore`] + [`FlowTelemetry`]
/// ([`FlowExecutor::new`] / [`FlowExecutor::dispatcher_handles`]); `start` inserts a runnable run,
/// the dispatcher's `tick` leases + drives it, `describe` reads its settled lifecycle.
#[derive(Clone)]
pub struct FlowExecutor {
    runs: RunStore,
    telemetry: FlowTelemetry,
    minter: Arc<dyn IdMinter>,
    tenant: TenantId,
    region: Region,
    /// the registered definition names (the `wf_type → wf_version` registry §3.6; the body itself is
    /// registered on the dispatcher — here we track which types are startable + their pinned
    /// version). A `start` of an unregistered type is [`ExecutorError::UnknownWorkflow`].
    definitions: Arc<Mutex<HashMap<String, i32>>>,
    /// `idem_key → StartedRun` — the idempotency anchor (a re-start is a no-op return of the existing
    /// run) + `run_id → idem_key` so describe/cancel resolve by handle.
    started: Arc<Mutex<StartedState>>,
}

#[derive(Default)]
struct StartedState {
    by_idem: HashMap<String, StartedRun>,
    run_to_idem: HashMap<String, String>,
}

impl FlowExecutor {
    /// Build an executor over a fresh run store + telemetry for `(tenant, region)`. Use
    /// [`FlowExecutor::dispatcher_handles`] to share its run store + telemetry with the dispatcher
    /// that drives the runs `start` seeds.
    pub fn new(minter: Arc<dyn IdMinter>, tenant: TenantId, region: Region) -> Self {
        Self {
            runs: RunStore::new(),
            telemetry: FlowTelemetry::new(),
            minter,
            tenant,
            region,
            definitions: Arc::new(Mutex::new(HashMap::new())),
            started: Arc::new(Mutex::new(StartedState::default())),
        }
    }

    /// Register a definition the executor may `start` (the §3.6 registry; the deterministic body is
    /// registered on the dispatcher via [`crate::engine::FlowDispatcher::register`]). Pins
    /// `wf_version = 1` (the persisted versioned registry + `code_hash` drift detection is the named
    /// M5 floor). A `start` of an unregistered `wf_type` is [`ExecutorError::UnknownWorkflow`].
    pub fn register_definition(&self, wf_type: impl Into<String>) {
        self.definitions.lock().unwrap().insert(wf_type.into(), 1);
    }

    /// The shared [`RunStore`] + [`FlowTelemetry`] to build a [`crate::engine::FlowDispatcher`] over
    /// — so the dispatcher leases + drives EXACTLY the runs this executor's `start` seeds and writes
    /// the same telemetry the control surface reads.
    pub fn dispatcher_handles(&self) -> (RunStore, FlowTelemetry) {
        (self.runs.clone(), self.telemetry.clone())
    }

    /// The telemetry handle the metrics-health port reads (the contract-1.8 engine signal set).
    pub fn telemetry(&self) -> &FlowTelemetry {
        &self.telemetry
    }

    /// The run store the executor seeds + reads (so a test/dispatcher shares it).
    pub fn runs(&self) -> &RunStore {
        &self.runs
    }

    /// The references-not-payloads `input` a run was started with (`ArtifactRef`s, never a PII body
    /// — §3.1). The engine carries it to the run's first activity (the body resolves the refs); the
    /// cost-safety + signal prompts read it. `None` for an unknown run.
    pub fn run_input(&self, run: &RunId) -> Option<Vec<ArtifactRef>> {
        let started = self.started.lock().unwrap();
        let idem = started.run_to_idem.get(&run.0)?;
        started.by_idem.get(idem).map(|r| r.input.clone())
    }

    /// The owned [`RunBudget`] a run was started with (integer minor-units, §5.1) — the ceiling the
    /// cost-safety prompt reserves/settles against. `None` for an un-metered run OR an unknown run.
    pub fn run_budget(&self, run: &RunId) -> Option<RunBudget> {
        let started = self.started.lock().unwrap();
        let idem = started.run_to_idem.get(&run.0)?;
        started.by_idem.get(idem).and_then(|r| r.budget.clone())
    }

    /// The deterministic partition for `run_id` (`hash(run_id) % N`, §7.2) — the worker shard the
    /// dispatcher's partition-scoped lease scan claims it from.
    fn partition_for(run_id: &str) -> i16 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        run_id.hash(&mut h);
        (h.finish() % PARTITION_COUNT as u64) as i16
    }

    /// Refresh the runnable-run-lag gauge across all partitions (the §1.8 signal) — called after a
    /// start/cancel so the metrics-health port reads a current lag. `i64::MAX` reads the lag
    /// ignoring lease expiry (every unleased runnable run counts).
    fn refresh_runnable_lag(&self) {
        let mut total = 0u64;
        for p in 0..PARTITION_COUNT as i16 {
            total += self.runs.runnable_lag(p, i64::MAX) as u64;
        }
        self.telemetry.set_runnable_lag(total);
    }
}

impl DurableExecutor for FlowExecutor {
    fn start(&self, spec: StartSpec) -> Result<RunId, ExecutorError> {
        // The registered definition the engine would drive must exist — surfaced, never a silent run.
        let wf_version = *self
            .definitions
            .lock()
            .unwrap()
            .get(&spec.wf_type)
            .ok_or_else(|| ExecutorError::UnknownWorkflow(spec.wf_type.clone()))?;

        let mut started = self.started.lock().unwrap();
        // IDEMPOTENT on idem_key (FROZEN, contract 9.1): a re-start with the same key returns the
        // SAME run — a redelivered trigger is ONE workflow, not two. The wf_type/input are NOT
        // re-validated against the existing record (the first start won the race; this is a no-op
        // return of its handle — exactly the effectively-once semantic).
        if let Some(existing) = started.by_idem.get(&spec.idem_key) {
            return Ok(existing.run_id.clone());
        }

        // Mint the ULID-ordered run id, seed a runnable RunRow (state=running, cursor 0, unleased)
        // the dispatcher then leases + drives, and record the references-not-payloads StartSpec.
        let run_id = RunId(self.minter.mint().0);
        let partition = Self::partition_for(&run_id.0);
        // Seed the run with its PINNED wf_version (§4.6) so the divergence guard can detect a deploy
        // that bumps the definition while the run is in flight (P-FLOW-07).
        self.runs.put(RunRow::new_runnable_versioned(
            self.tenant.clone(),
            self.region.clone(),
            run_id.0.clone(),
            spec.wf_type.clone(),
            wf_version,
            partition,
        ));
        let record = StartedRun {
            run_id: run_id.clone(),
            wf_type: spec.wf_type.clone(),
            input: spec.input.clone(),
            budget: spec.budget.clone(),
            wf_version,
            cancel_reason: None,
        };
        started.by_idem.insert(spec.idem_key.clone(), record);
        started
            .run_to_idem
            .insert(run_id.0.clone(), spec.idem_key.clone());
        drop(started);

        self.refresh_runnable_lag();
        Ok(run_id)
    }

    fn describe(&self, run: &RunId) -> Result<RunStatus, ExecutorError> {
        let started = self.started.lock().unwrap();
        let idem = started
            .run_to_idem
            .get(&run.0)
            .ok_or_else(|| ExecutorError::UnknownRun(run.0.clone()))?;
        let record = started.by_idem.get(idem).expect("run_to_idem points at a record");
        let wf_version = record.wf_version;
        let wf_type = record.wf_type.clone();
        drop(started);

        // The lifecycle is the run store's truth (the dispatcher drives state/cursor as it leases).
        let row = self
            .runs
            .get(&self.tenant, &run.0)
            .ok_or_else(|| ExecutorError::UnknownRun(run.0.clone()))?;
        Ok(RunStatus {
            run_id: run.clone(),
            wf_type,
            state: row.state.clone(),
            cursor: row.cursor,
            wf_version,
            terminal: run_state::is_terminal(&row.state),
        })
    }

    fn cancel(&self, run: &RunId, reason: &str) -> Result<(), ExecutorError> {
        let mut started = self.started.lock().unwrap();
        let idem = started
            .run_to_idem
            .get(&run.0)
            .cloned()
            .ok_or_else(|| ExecutorError::UnknownRun(run.0.clone()))?;

        let row = self
            .runs
            .get(&self.tenant, &run.0)
            .ok_or_else(|| ExecutorError::UnknownRun(run.0.clone()))?;
        // Idempotent on a terminal run: a cancel of a completed/failed/already-cancelled run is a
        // no-op return — never re-terminate, never resurrect a completed run.
        if run_state::is_terminal(&row.state) {
            return Ok(());
        }
        // Transition the non-terminal run to `terminated` and release its lease so the dispatcher
        // never drives it again. The cursor is unchanged (the journal is the source of truth; a
        // cancel does not rewrite history).
        self.runs
            .terminate(&self.tenant, &run.0, run_state::TERMINATED);
        if let Some(rec) = started.by_idem.get_mut(&idem) {
            rec.cancel_reason = Some(reason.to_string());
        }
        drop(started);

        self.refresh_runnable_lag();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{DriveOutcome, FlowDispatcher, WorkflowBody};
    use crate::wfctx::{RetryPolicy, WfCtx, WfJournal};
    use myelin_events::{
        Actor, EmitContextBase, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

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

    fn spec(idem: &str) -> StartSpec {
        StartSpec {
            wf_type: "agent.run".into(),
            input: vec![ArtifactRef("myelin://acme/git/pr/PR-1".into())],
            budget: Some(RunBudget { minor_units: 10_000 }),
            idem_key: idem.into(),
        }
    }

    fn executor() -> FlowExecutor {
        let ex = FlowExecutor::new(minter(), tenant(), region());
        ex.register_definition("agent.run");
        ex
    }

    fn one_activity_body() -> Box<WorkflowBody> {
        Box::new(|ctx: &mut WfCtx| {
            ctx.activity(RetryPolicy::default_policy(), |_i, _a| {
                Ok(vec![ArtifactRef("myelin://acme/agent/effect/e0".into())])
            })
            .map_err(|e| format!("{e:?}"))?;
            Ok(vec![])
        })
    }

    /// **`start` is IDEMPOTENT on `idem_key` (FROZEN, contract 9.1).** A re-start with the SAME key
    /// returns the SAME [`RunId`] and seeds NO second run — a redelivered trigger is one workflow.
    #[test]
    fn start_is_idempotent_on_idem_key() {
        let ex = executor();
        let r1 = ex.start(spec("rule:evt-1")).expect("start");
        let r2 = ex.start(spec("rule:evt-1")).expect("re-start same idem_key");
        assert_eq!(r1, r2, "the re-start returns the SAME run id (effectively-once)");
        // exactly ONE runnable run was seeded (not two) across all partitions.
        let total: usize = (0..PARTITION_COUNT as i16)
            .map(|p| ex.runs().runnable_lag(p, i64::MAX))
            .sum();
        assert_eq!(total, 1, "the second start was a no-op — exactly one run seeded");

        // a DIFFERENT idem_key starts a distinct run.
        let r3 = ex.start(spec("rule:evt-2")).expect("distinct start");
        assert_ne!(r1, r3, "a distinct idem_key is a distinct run");
    }

    /// **`start` of an unregistered `wf_type` is surfaced (never a silent dropped run, EI-02 §4).**
    #[test]
    fn start_of_unknown_workflow_is_an_error() {
        let ex = executor();
        let mut s = spec("k");
        s.wf_type = "no.such.workflow".into();
        let err = ex.start(s).expect_err("unknown workflow type is surfaced");
        assert_eq!(err, ExecutorError::UnknownWorkflow("no.such.workflow".into()));
    }

    /// **`describe` returns the run's `RunStatus` — lifecycle + cursor + pinned version (contract
    /// 9.1).** A freshly-started run describes as `running` at cursor 0, non-terminal, version 1.
    #[test]
    fn describe_returns_run_status() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");
        let status = ex.describe(&run).expect("describe");
        assert_eq!(status.run_id, run);
        assert_eq!(status.wf_type, "agent.run");
        assert_eq!(status.state, run_state::RUNNING, "a fresh run is running");
        assert_eq!(status.cursor, 0, "a fresh run is at cursor 0");
        assert_eq!(status.wf_version, 1, "the wf_version pinned at start (§4.6)");
        assert!(!status.terminal, "a running run is not terminal");

        // describe of an unknown run is surfaced.
        let err = ex.describe(&RunId("nope".into())).expect_err("unknown run");
        assert_eq!(err, ExecutorError::UnknownRun("nope".into()));
    }

    /// **`describe` reflects the dispatcher driving the run to completion (start → drive → describe).**
    /// A started run, once the dispatcher leases + drives it, describes as `completed` + terminal.
    #[test]
    fn describe_reflects_completion_after_the_dispatcher_drives() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");
        // the run is hashed into some partition; build the dispatcher on THAT partition, sharing the
        // executor's run store + telemetry, so its tick leases + drives exactly this run.
        let part = FlowExecutor::partition_for(&run.0);
        let (runs, tele) = ex.dispatcher_handles();
        let mut disp = FlowDispatcher::new(
            runs,
            OutboxStore::new(),
            WfJournal::new(),
            tele,
            minter(),
            ctx_base(),
            part,
            "worker-1",
            30,
        );
        disp.register("agent.run", one_activity_body());

        let outcome = disp.tick(1000, "2026-06-21T00:00:00Z", 7);
        assert!(matches!(outcome, Some(DriveOutcome::Completed(_))), "the dispatcher drove the run");

        let status = ex.describe(&run).expect("describe");
        assert_eq!(status.state, run_state::COMPLETED, "the run describes as completed");
        assert!(status.terminal, "a completed run is terminal");
    }

    /// **FLOW-D2 end-to-end (P-FLOW-07, §4.6): a deploy that bumps the definition version HALTS an
    /// in-flight run as `nondeterministic`.** A run starts pinned to `wf_version = 1`; the worker is
    /// then redeployed running version 2 of `agent.run` (registered via `register_versioned`). When
    /// the dispatcher leases + drives the v1-pinned run, the version-divergence guard halts it as
    /// `nondeterministic` and dead-letters it — `describe` reports the terminal state and the
    /// nondeterministic-halt telemetry increments. This is the control-plane face of the FLOW-D2 drill.
    #[test]
    fn deploy_version_bump_halts_in_flight_run_as_nondeterministic() {
        let ex = executor(); // registers agent.run at pinned version 1.
        let run = ex.start(spec("k")).expect("start a v1-pinned run");
        assert_eq!(ex.describe(&run).unwrap().wf_version, 1, "the run pinned to v1 at start");

        // the worker is redeployed running VERSION 2 of agent.run (a new body shape).
        let part = FlowExecutor::partition_for(&run.0);
        let (runs, tele) = ex.dispatcher_handles();
        let mut disp = FlowDispatcher::new(
            runs, OutboxStore::new(), WfJournal::new(), tele, minter(), ctx_base(), part, "worker-2", 30,
        );
        disp.register_versioned("agent.run", 2, one_activity_body());

        // the dispatcher leases + drives the v1-pinned run with the v2 engine → the version guard halts.
        let outcome = disp.tick(1000, "2026-06-21T00:00:00Z", 7);
        assert!(
            matches!(outcome, Some(DriveOutcome::Nondeterministic(_))),
            "the version mismatch halts the run, got {outcome:?}"
        );
        let status = ex.describe(&run).expect("describe after the halt");
        assert_eq!(status.state, run_state::NONDETERMINISTIC, "the run is dead-lettered as nondeterministic");
        assert!(status.terminal, "a nondeterministic run is terminal — never re-driven");
        assert_eq!(
            ex.telemetry().nondeterministic_halt_count(),
            1,
            "the nondeterministic-halt count incremented (the FLOW-D2 green artifact, surfaced on the metrics port)"
        );
    }

    /// **`cancel` transitions a non-terminal run to `terminated` (contract 9.1).** A cancel of a
    /// running run lands it `terminated` + releases its lease; `describe` then reports terminal.
    #[test]
    fn cancel_terminates_a_running_run() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");
        ex.cancel(&run, "user requested").expect("cancel");
        let status = ex.describe(&run).expect("describe after cancel");
        assert_eq!(status.state, run_state::TERMINATED, "the run is terminated");
        assert!(status.terminal, "a cancelled run is terminal");
        // the run is no longer runnable (the dispatcher will never lease it).
        let total: usize = (0..PARTITION_COUNT as i16)
            .map(|p| ex.runs().runnable_lag(p, i64::MAX))
            .sum();
        assert_eq!(total, 0, "a cancelled run is no longer runnable");
    }

    /// **`cancel` is idempotent on a terminal run (a no-op, never a re-terminate).** A second cancel,
    /// and a cancel of an unknown run (surfaced), pin the idempotency + the error path.
    #[test]
    fn cancel_is_idempotent_and_surfaces_unknown() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");
        ex.cancel(&run, "first").expect("cancel");
        ex.cancel(&run, "second").expect("a second cancel is a no-op (idempotent)");
        assert_eq!(
            ex.describe(&run).unwrap().state,
            run_state::TERMINATED,
            "the run stays terminated (the second cancel did not change it)"
        );
        let err = ex.cancel(&RunId("nope".into()), "x").expect_err("unknown run");
        assert_eq!(err, ExecutorError::UnknownRun("nope".into()));
    }

    /// **The four named contract-1.8 telemetry signals are assert-readable off the executor's
    /// telemetry handle (§5.4).** runnable-run lag (the start seeds runnable work), replay rate,
    /// activity queue depth, retry + dead-letter — all readable on the metrics-health port.
    #[test]
    fn the_named_telemetry_signals_are_readable() {
        let ex = executor();
        // before any start: lag 0, replay rate 0 (nothing driven), 0 retry/dead-letter.
        assert_eq!(ex.telemetry().runnable_run_lag(), 0);
        assert_eq!(ex.telemetry().replay_rate_bps(), 0);
        assert_eq!(ex.telemetry().activity_queue_depth(), 0);
        assert_eq!(ex.telemetry().activity_retry_count(), 0);
        assert_eq!(ex.telemetry().dead_letter_count(), 0);

        // start two runs → the runnable-run-lag gauge reads 2 (the §1.8 signal is set on start).
        ex.start(spec("a")).expect("start a");
        ex.start(spec("b")).expect("start b");
        assert_eq!(ex.telemetry().runnable_run_lag(), 2, "two runnable runs are queued (the lag signal)");

        // the activity-queue/retry/dead-letter signals are settable + readable (the §1.8 leg this
        // prompt adds; the engine sets them as it schedules/retries/dead-letters activities).
        ex.telemetry().set_activity_queue_depth(3);
        ex.telemetry().record_activity_retry();
        ex.telemetry().record_dead_letter();
        assert_eq!(ex.telemetry().activity_queue_depth(), 3, "activity-queue-depth readable");
        assert_eq!(ex.telemetry().activity_retry_count(), 1, "activity-retry readable");
        assert_eq!(ex.telemetry().dead_letter_count(), 1, "dead-letter readable");
    }

    /// **The references-not-payloads `input` is carried as `ArtifactRef`s (the §3.1 invariant).** A
    /// `start` stores the run input as refs (never a PII body) — the type-level erasure-for-free
    /// posture. (No `describe` field exposes the input — it is the engine's private carriage.)
    #[test]
    fn start_stores_input_as_references_not_payloads() {
        let ex = executor();
        let s = StartSpec {
            wf_type: "agent.run".into(),
            input: vec![ArtifactRef("myelin://acme/git/pr/PR-9".into())],
            budget: None,
            idem_key: "k".into(),
        };
        let run = ex.start(s).expect("start");
        // input is ArtifactRefs (references-not-payloads) — read through the public accessor.
        let input = ex.run_input(&run).expect("the started run's input");
        assert_eq!(input, vec![ArtifactRef("myelin://acme/git/pr/PR-9".into())]);
        assert_eq!(
            ex.run_budget(&run),
            None,
            "no budget → un-metered (the engine owns the loop-cap depth)"
        );
    }
}
