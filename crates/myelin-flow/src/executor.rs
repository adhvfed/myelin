use crate::engine::{run_state, FlowTelemetry, RunRow, RunStore, SignalRow, SignalStore};
use myelin_events::IdMinter;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const PARTITION_COUNT: u32 = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunBudget {
    pub minor_units: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartSpec {
    pub wf_type: String,
    pub input: Vec<ArtifactRef>,
    pub budget: Option<RunBudget>,
    pub idem_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalSpec {
    pub run: RunId,
    pub signal_name: String,
    pub idem_key: String,
    pub payload: Vec<ArtifactRef>,
    pub payload_key_ref: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalOutcome {
    Buffered,
    Duplicate,
    TerminalNoOp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalPayload {
    ScopedRefs(Vec<ArtifactRef>),
    CiJobDone {
        stage: String,
        passed: bool,
        result_refs: Vec<ArtifactRef>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedSignalSpec {
    pub run: RunId,
    pub signal_name: String,
    pub idem_key: String,
    pub payload: SignalPayload,
    pub payload_key_ref: Option<String>,
}

pub(crate) const MAX_CI_STAGE_TOKEN_BYTES: usize = 128;

pub(crate) fn valid_ci_stage_token(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    value.len() <= MAX_CI_STAGE_TOKEN_BYTES
        && first.is_ascii_alphanumeric()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

pub(crate) fn validate_signal_refs(
    refs: &[ArtifactRef],
    tenant: &TenantId,
) -> Result<(), ExecutorError> {
    for artifact in refs {
        let parsed = myelin_refs::parse_scoped(&artifact.0).map_err(|error| {
            ExecutorError::InvalidInput(format!("malformed ArtifactRef: {error}"))
        })?;
        if parsed.tenant != *tenant {
            return Err(ExecutorError::InvalidInput(
                "ArtifactRef tenant does not match the verified executor tenant".into(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn canonicalize_signal_payload(
    signal_name: &str,
    payload: SignalPayload,
    tenant: &TenantId,
) -> Result<Vec<ArtifactRef>, ExecutorError> {
    match payload {
        SignalPayload::ScopedRefs(refs) => {
            validate_signal_refs(&refs, tenant)?;
            Ok(refs)
        }
        SignalPayload::CiJobDone {
            stage,
            passed,
            result_refs,
        } => {
            if signal_name != crate::job::JOB_DONE_SIGNAL {
                return Err(ExecutorError::InvalidInput(format!(
                    "a CiJobDone payload requires signal_name `{}`, not `{signal_name}`",
                    crate::job::JOB_DONE_SIGNAL
                )));
            }
            if !valid_ci_stage_token(&stage) {
                return Err(ExecutorError::InvalidInput(
                    "CiJobDone stage is not a bounded machine token ([A-Za-z0-9_.-], ≤128 bytes)"
                        .into(),
                ));
            }
            validate_signal_refs(&result_refs, tenant)?;
            let mut canonical = Vec::with_capacity(result_refs.len() + 1);
            canonical.push(crate::ci_pipeline::stage_verdict_marker(&stage, passed));
            canonical.extend(result_refs);
            Ok(canonical)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunStatus {
    pub run_id: RunId,
    pub wf_type: String,
    pub state: String,
    pub cursor: i64,
    pub wf_version: i32,
    pub terminal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutorError {
    UnknownWorkflow(String),
    UnknownRun(String),
    RunIdConflict(String),
    DefinitionDrift(String),
    InvalidInput(String),
    Storage(String),
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutorError::UnknownWorkflow(t) => write!(f, "unknown workflow type: {t}"),
            ExecutorError::UnknownRun(r) => write!(f, "unknown run: {r}"),
            ExecutorError::RunIdConflict(r) => {
                write!(f, "provided run_id collides with an existing run: {r}")
            }
            ExecutorError::DefinitionDrift(d) => write!(f, "workflow definition drift: {d}"),
            ExecutorError::InvalidInput(e) => write!(f, "invalid workflow control input: {e}"),
            ExecutorError::Storage(e) => write!(f, "durable workflow store failed: {e}"),
        }
    }
}

impl std::error::Error for ExecutorError {}

pub trait DurableExecutor {
    fn start(&self, spec: StartSpec) -> Result<RunId, ExecutorError> {
        self.start_with_id(spec, None)
    }

    fn start_with_id(&self, spec: StartSpec, run_id: Option<RunId>)
        -> Result<RunId, ExecutorError>;

    fn signal(&self, spec: SignalSpec) -> Result<SignalOutcome, ExecutorError>;

    fn signal_typed(&self, spec: TypedSignalSpec) -> Result<SignalOutcome, ExecutorError> {
        match spec.payload {
            SignalPayload::ScopedRefs(payload) => self.signal(SignalSpec {
                run: spec.run,
                signal_name: spec.signal_name,
                idem_key: spec.idem_key,
                payload,
                payload_key_ref: spec.payload_key_ref,
            }),
            SignalPayload::CiJobDone { .. } => Err(ExecutorError::InvalidInput(
                "this executor does not support typed CiJobDone delivery".into(),
            )),
        }
    }

    fn describe(&self, run: &RunId) -> Result<RunStatus, ExecutorError>;

    fn cancel(&self, run: &RunId, reason: &str) -> Result<(), ExecutorError>;
}

#[derive(Clone, Debug)]
struct StartedRun {
    run_id: RunId,
    wf_type: String,
    input: Vec<ArtifactRef>,
    budget: Option<RunBudget>,
    wf_version: i32,
    cancel_reason: Option<String>,
}

#[derive(Clone)]
pub struct FlowExecutor {
    runs: RunStore,
    signals: SignalStore,
    telemetry: FlowTelemetry,
    minter: Arc<dyn IdMinter>,
    tenant: TenantId,
    region: Region,
    definitions: Arc<Mutex<HashMap<String, i32>>>,
    started: Arc<Mutex<StartedState>>,
}

#[derive(Default)]
struct StartedState {
    by_idem: HashMap<String, StartedRun>,
    run_to_idem: HashMap<String, String>,
}

impl FlowExecutor {
    pub fn new(minter: Arc<dyn IdMinter>, tenant: TenantId, region: Region) -> Self {
        Self {
            runs: RunStore::new(),
            signals: SignalStore::new(),
            telemetry: FlowTelemetry::new(),
            minter,
            tenant,
            region,
            definitions: Arc::new(Mutex::new(HashMap::new())),
            started: Arc::new(Mutex::new(StartedState::default())),
        }
    }

    pub fn register_definition(&self, wf_type: impl Into<String>) {
        self.definitions.lock().unwrap().insert(wf_type.into(), 1);
    }

    pub fn dispatcher_handles(&self) -> (RunStore, FlowTelemetry) {
        (self.runs.clone(), self.telemetry.clone())
    }

    pub fn telemetry(&self) -> &FlowTelemetry {
        &self.telemetry
    }

    pub fn runs(&self) -> &RunStore {
        &self.runs
    }

    pub fn signals(&self) -> &SignalStore {
        &self.signals
    }

    fn refresh_signal_buffer_depth(&self) {
        self.telemetry
            .set_signal_buffer_depth(self.signals.buffered_depth());
    }

    pub fn run_input(&self, run: &RunId) -> Option<Vec<ArtifactRef>> {
        let started = self.started.lock().unwrap();
        let idem = started.run_to_idem.get(&run.0)?;
        started.by_idem.get(idem).map(|r| r.input.clone())
    }

    pub fn run_budget(&self, run: &RunId) -> Option<RunBudget> {
        let started = self.started.lock().unwrap();
        let idem = started.run_to_idem.get(&run.0)?;
        started.by_idem.get(idem).and_then(|r| r.budget.clone())
    }

    fn partition_for(run_id: &str) -> i16 {
        partition_for_run_id(run_id)
    }

    fn deliver(
        &self,
        run: &RunId,
        signal_name: String,
        idem_key: String,
        payload: Vec<ArtifactRef>,
        payload_key_ref: Option<String>,
    ) -> Result<SignalOutcome, ExecutorError> {
        {
            let started = self.started.lock().unwrap();
            if !started.run_to_idem.contains_key(&run.0) {
                return Err(ExecutorError::UnknownRun(run.0.clone()));
            }
        }

        let buffered = self.signals.deliver(SignalRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            run_id: run.0.clone(),
            signal_name,
            idem_key,
            payload,
            payload_key_ref,
            received_unix_ms: 0,
            consumed_seq: None,
        });

        self.refresh_signal_buffer_depth();

        Ok(if buffered {
            SignalOutcome::Buffered
        } else {
            SignalOutcome::Duplicate
        })
    }

    fn refresh_runnable_lag(&self) {
        let mut total = 0u64;
        for p in 0..PARTITION_COUNT as i16 {
            total += self.runs.runnable_lag(p, i64::MAX) as u64;
        }
        self.telemetry.set_runnable_lag(total);
    }
}

fn stable_partition_hash(run_id: &str) -> u64 {
    use std::hash::Hasher;

    let mut hasher = siphasher::sip::SipHasher13::new_with_keys(0, 0);
    hasher.write(run_id.as_bytes());
    hasher.write_u8(0xff);
    hasher.finish()
}

pub(crate) fn partition_for_shards(run_id: &str, shards: u32) -> i16 {
    (stable_partition_hash(run_id) % u64::from(shards.max(1))) as i16
}

pub fn partition_for_run_id(run_id: &str) -> i16 {
    partition_for_shards(run_id, PARTITION_COUNT)
}

impl DurableExecutor for FlowExecutor {
    fn start_with_id(
        &self,
        spec: StartSpec,
        run_id: Option<RunId>,
    ) -> Result<RunId, ExecutorError> {
        let wf_version = *self
            .definitions
            .lock()
            .unwrap()
            .get(&spec.wf_type)
            .ok_or_else(|| ExecutorError::UnknownWorkflow(spec.wf_type.clone()))?;

        let mut started = self.started.lock().unwrap();
        if let Some(existing) = started.by_idem.get(&spec.idem_key) {
            return Ok(existing.run_id.clone());
        }

        let run_id = match run_id {
            Some(id) => {
                if started.run_to_idem.contains_key(&id.0)
                    || self.runs.get(&self.tenant, &id.0).is_some()
                {
                    return Err(ExecutorError::RunIdConflict(id.0));
                }
                id
            }
            None => RunId(self.minter.mint().0),
        };
        let partition = Self::partition_for(&run_id.0);
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

    fn signal(&self, spec: SignalSpec) -> Result<SignalOutcome, ExecutorError> {
        self.deliver(
            &spec.run,
            spec.signal_name,
            spec.idem_key,
            spec.payload,
            spec.payload_key_ref,
        )
    }

    fn signal_typed(&self, spec: TypedSignalSpec) -> Result<SignalOutcome, ExecutorError> {
        let payload = canonicalize_signal_payload(&spec.signal_name, spec.payload, &self.tenant)?;
        self.deliver(
            &spec.run,
            spec.signal_name,
            spec.idem_key,
            payload,
            spec.payload_key_ref,
        )
    }

    fn describe(&self, run: &RunId) -> Result<RunStatus, ExecutorError> {
        let started = self.started.lock().unwrap();
        let idem = started
            .run_to_idem
            .get(&run.0)
            .ok_or_else(|| ExecutorError::UnknownRun(run.0.clone()))?;
        let record = started
            .by_idem
            .get(idem)
            .expect("run_to_idem points at a record");
        let wf_version = record.wf_version;
        let wf_type = record.wf_type.clone();
        drop(started);

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
        if run_state::is_terminal(&row.state) {
            return Ok(());
        }
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
    use myelin_events::{Actor, EmitContextBase, MonotonicMinter, OutboxStore, Timestamp};
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
            budget: Some(RunBudget {
                minor_units: 100_000_000,
            }),
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

    #[test]
    fn durable_partition_hash_matches_frozen_known_answers() {
        let vectors = [
            ("", 0x3040_6ea5_23c5_3def_u64, 15_i16, 47_u64),
            ("a", 0x719b_50b9_a4f0_e9f3, 3, 51),
            ("run-rolled-back", 0x04ea_b0d1_24f5_7a61, 1, 33),
            (
                "00000000-0000-0000-0000-000000000000",
                0xc104_447f_85ee_a341,
                1,
                1,
            ),
            (
                "0190f8b0-7c00-7f3d-8000-000000000001",
                0x9e9d_af38_43b9_f61c,
                12,
                28,
            ),
            ("wf:evt-42", 0xfe07_9a5f_b671_7967, 7, 39),
            ("myelin://acme/flow/run/123", 0x5ca0_2bf3_f373_7ab2, 2, 50),
            ("éclair", 0x31e1_0df2_2833_fc5b, 11, 27),
            ("emoji-🧠", 0x4cbd_d88e_c2d0_0148, 8, 8),
            ("nul\0inside", 0xacc6_c10a_e1da_2b31, 1, 49),
        ];

        for (run_id, expected_hash, expected_partition, expected_mod_64) in vectors {
            let actual_hash = stable_partition_hash(run_id);
            assert_eq!(
                actual_hash, expected_hash,
                "full digest drifted for {run_id:?}"
            );
            assert_eq!(
                partition_for_run_id(run_id),
                expected_partition,
                "durable modulo-{PARTITION_COUNT} mapping drifted for {run_id:?}"
            );
            assert_eq!(
                partition_for_shards(run_id, 64) as u64,
                expected_mod_64,
                "modulo-64 compatibility drifted for {run_id:?}"
            );
        }
    }

    #[test]
    fn frozen_partition_hash_has_broad_parity_with_the_legacy_mapping() {
        use std::hash::{Hash, Hasher};

        let fixed = [
            "",
            "a",
            "run-rolled-back",
            "00000000-0000-0000-0000-000000000000",
            "0190f8b0-7c00-7f3d-8000-000000000001",
            "wf:evt-42",
            "myelin://acme/flow/run/123",
            "éclair",
            "emoji-🧠",
            "nul\0inside",
        ];
        let generated = (0_u64..20_000).flat_map(|index| {
            [
                format!("run-{index}"),
                format!("{index:016x}-{index:020}-tenant/acme"),
                format!("workflow:🧠:{index}:\0:{:x}", index.rotate_left(29)),
            ]
        });

        for run_id in fixed
            .iter()
            .map(|value| (*value).to_owned())
            .chain(generated)
        {
            let mut legacy = std::collections::hash_map::DefaultHasher::new();
            run_id.hash(&mut legacy);
            assert_eq!(
                stable_partition_hash(&run_id),
                legacy.finish(),
                "frozen hash diverged from the historical mapping for {run_id:?}"
            );
        }
    }

    #[test]
    fn start_is_idempotent_on_idem_key() {
        let ex = executor();
        let r1 = ex.start(spec("rule:evt-1")).expect("start");
        let r2 = ex
            .start(spec("rule:evt-1"))
            .expect("re-start same idem_key");
        assert_eq!(
            r1, r2,
            "the re-start returns the SAME run id (effectively-once)"
        );
        let total: usize = (0..PARTITION_COUNT as i16)
            .map(|p| ex.runs().runnable_lag(p, i64::MAX))
            .sum();
        assert_eq!(
            total, 1,
            "the second start was a no-op - exactly one run seeded"
        );

        let r3 = ex.start(spec("rule:evt-2")).expect("distinct start");
        assert_ne!(r1, r3, "a distinct idem_key is a distinct run");
    }

    #[test]
    fn start_with_id_uses_the_provided_run_id() {
        let ex = executor();
        let provided = RunId("wf:evt-42".into());
        let run = ex
            .start_with_id(spec("rule:evt-42"), Some(provided.clone()))
            .expect("start with a provided id");
        assert_eq!(
            run, provided,
            "the created run carries the CALLER-PROVIDED id, not a minted one"
        );
        assert_eq!(
            ex.describe(&provided)
                .expect("describe the provided-id run")
                .run_id,
            provided
        );
    }

    #[test]
    fn start_with_id_idem_key_wins_on_redelivery() {
        let ex = executor();
        let provided = RunId("wf:evt-7".into());
        let r1 = ex
            .start_with_id(spec("rule:evt-7"), Some(provided.clone()))
            .expect("first start");
        let r2 = ex
            .start_with_id(spec("rule:evt-7"), Some(provided.clone()))
            .expect("redelivery: same idem_key + same provided id returns the existing run");
        assert_eq!(r1, provided);
        assert_eq!(
            r2, provided,
            "the redelivery returned the EXISTING run (idem_key wins)"
        );
        let total: usize = (0..PARTITION_COUNT as i16)
            .map(|p| ex.runs().runnable_lag(p, i64::MAX))
            .sum();
        assert_eq!(
            total, 1,
            "the redelivery was a no-op - exactly one run seeded"
        );
    }

    #[test]
    fn start_with_id_collision_on_different_idem_key_fails_closed() {
        let ex = executor();
        let provided = RunId("wf:evt-clash".into());
        let first = ex
            .start_with_id(spec("rule:A"), Some(provided.clone()))
            .expect("first run claims the id");
        let err = ex
            .start_with_id(spec("rule:B"), Some(provided.clone()))
            .expect_err("a colliding provided id under a different idem_key is surfaced");
        assert_eq!(err, ExecutorError::RunIdConflict("wf:evt-clash".into()));
        assert_eq!(
            ex.describe(&first).expect("the first run is intact").run_id,
            provided
        );
        let total: usize = (0..PARTITION_COUNT as i16)
            .map(|p| ex.runs().runnable_lag(p, i64::MAX))
            .sum();
        assert_eq!(total, 1, "the fail-closed collision seeded no second run");
    }

    #[test]
    fn start_with_none_mints_as_before() {
        let ex = executor();
        let minted = ex
            .start_with_id(spec("rule:mint"), None)
            .expect("None mints an id");
        let legacy = ex
            .start(spec("rule:mint-2"))
            .expect("legacy start still mints");
        assert_ne!(minted, legacy, "each minted run has a distinct id");
        assert!(!minted.0.is_empty() && !legacy.0.is_empty());
        assert_eq!(ex.describe(&minted).unwrap().state, run_state::RUNNING);
        assert_eq!(ex.describe(&legacy).unwrap().state, run_state::RUNNING);
    }

    #[test]
    fn start_of_unknown_workflow_is_an_error() {
        let ex = executor();
        let mut s = spec("k");
        s.wf_type = "no.such.workflow".into();
        let err = ex.start(s).expect_err("unknown workflow type is surfaced");
        assert_eq!(
            err,
            ExecutorError::UnknownWorkflow("no.such.workflow".into())
        );
    }

    #[test]
    fn describe_returns_run_status() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");
        let status = ex.describe(&run).expect("describe");
        assert_eq!(status.run_id, run);
        assert_eq!(status.wf_type, "agent.run");
        assert_eq!(status.state, run_state::RUNNING, "a fresh run is running");
        assert_eq!(status.cursor, 0, "a fresh run is at cursor 0");
        assert_eq!(
            status.wf_version, 1,
            "the wf_version pinned at start (§4.6)"
        );
        assert!(!status.terminal, "a running run is not terminal");

        let err = ex.describe(&RunId("nope".into())).expect_err("unknown run");
        assert_eq!(err, ExecutorError::UnknownRun("nope".into()));
    }

    #[test]
    fn describe_reflects_completion_after_the_dispatcher_drives() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");
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
        assert!(
            matches!(outcome, Some(DriveOutcome::Completed(_))),
            "the dispatcher drove the run"
        );

        let status = ex.describe(&run).expect("describe");
        assert_eq!(
            status.state,
            run_state::COMPLETED,
            "the run describes as completed"
        );
        assert!(status.terminal, "a completed run is terminal");
    }

    #[test]
    fn deploy_version_bump_halts_in_flight_run_as_nondeterministic() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start a v1-pinned run");
        assert_eq!(
            ex.describe(&run).unwrap().wf_version,
            1,
            "the run pinned to v1 at start"
        );

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
            "worker-2",
            30,
        );
        disp.register_versioned("agent.run", 2, one_activity_body());

        let outcome = disp.tick(1000, "2026-06-21T00:00:00Z", 7);
        assert!(
            matches!(outcome, Some(DriveOutcome::Nondeterministic(_))),
            "the version mismatch halts the run, got {outcome:?}"
        );
        let status = ex.describe(&run).expect("describe after the halt");
        assert_eq!(
            status.state,
            run_state::NONDETERMINISTIC,
            "the run is dead-lettered as nondeterministic"
        );
        assert!(
            status.terminal,
            "a nondeterministic run is terminal - never re-driven"
        );
        assert_eq!(
            ex.telemetry().nondeterministic_halt_count(),
            1,
            "the nondeterministic-halt count incremented (the FLOW-D2 green artifact, surfaced on the metrics port)"
        );
    }

    #[test]
    fn cancel_terminates_a_running_run() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");
        ex.cancel(&run, "user requested").expect("cancel");
        let status = ex.describe(&run).expect("describe after cancel");
        assert_eq!(status.state, run_state::TERMINATED, "the run is terminated");
        assert!(status.terminal, "a cancelled run is terminal");
        let total: usize = (0..PARTITION_COUNT as i16)
            .map(|p| ex.runs().runnable_lag(p, i64::MAX))
            .sum();
        assert_eq!(total, 0, "a cancelled run is no longer runnable");
    }

    #[test]
    fn cancel_is_idempotent_and_surfaces_unknown() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");
        ex.cancel(&run, "first").expect("cancel");
        ex.cancel(&run, "second")
            .expect("a second cancel is a no-op (idempotent)");
        assert_eq!(
            ex.describe(&run).unwrap().state,
            run_state::TERMINATED,
            "the run stays terminated (the second cancel did not change it)"
        );
        let err = ex
            .cancel(&RunId("nope".into()), "x")
            .expect_err("unknown run");
        assert_eq!(err, ExecutorError::UnknownRun("nope".into()));
    }

    #[test]
    fn the_named_telemetry_signals_are_readable() {
        let ex = executor();
        assert_eq!(ex.telemetry().runnable_run_lag(), 0);
        assert_eq!(ex.telemetry().replay_rate_bps(), 0);
        assert_eq!(ex.telemetry().activity_queue_depth(), 0);
        assert_eq!(ex.telemetry().activity_retry_count(), 0);
        assert_eq!(ex.telemetry().dead_letter_count(), 0);

        ex.start(spec("a")).expect("start a");
        ex.start(spec("b")).expect("start b");
        assert_eq!(
            ex.telemetry().runnable_run_lag(),
            2,
            "two runnable runs are queued (the lag signal)"
        );

        ex.telemetry().set_activity_queue_depth(3);
        ex.telemetry().record_activity_retry();
        ex.telemetry().record_dead_letter();
        assert_eq!(
            ex.telemetry().activity_queue_depth(),
            3,
            "activity-queue-depth readable"
        );
        assert_eq!(
            ex.telemetry().activity_retry_count(),
            1,
            "activity-retry readable"
        );
        assert_eq!(
            ex.telemetry().dead_letter_count(),
            1,
            "dead-letter readable"
        );
    }

    fn signal_spec(run: &RunId, name: &str, idem: &str) -> SignalSpec {
        SignalSpec {
            run: run.clone(),
            signal_name: name.into(),
            idem_key: idem.into(),
            payload: vec![ArtifactRef("myelin://acme/agent/result/r0".into())],
            payload_key_ref: None,
        }
    }

    #[test]
    fn signal_double_delivery_buffers_once() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");

        let first = ex
            .signal(signal_spec(&run, "job.done", "tok-1"))
            .expect("first delivery");
        let second = ex
            .signal(signal_spec(&run, "job.done", "tok-1"))
            .expect("re-delivery");
        assert_eq!(
            first,
            SignalOutcome::Buffered,
            "the first delivery buffered the signal"
        );
        assert_eq!(
            second,
            SignalOutcome::Duplicate,
            "the re-delivery is a no-op (ON CONFLICT DO NOTHING)"
        );
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            1,
            "the wf_signal PK buffered the signal EXACTLY ONCE (the workflow wakes once, §4.9)"
        );
        assert_eq!(
            ex.telemetry().signal_buffer_depth(),
            1,
            "signal-buffer-depth = 1 (a double-delivery is one)"
        );
    }

    #[test]
    fn signals_differing_in_idem_key_both_insert() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");
        ex.signal(signal_spec(&run, "approval", "card-7:0"))
            .expect("effect 0");
        ex.signal(signal_spec(&run, "approval", "card-7:1"))
            .expect("effect 1");
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            2,
            "two distinct per-effect keys buffer two rows (the multi-effect anchor, §6.4)"
        );
        assert_eq!(
            ex.telemetry().signal_buffer_depth(),
            2,
            "signal-buffer-depth = 2 (two distinct keys)"
        );
    }

    #[test]
    fn signals_differing_in_signal_name_both_insert() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");
        ex.signal(signal_spec(&run, "approval", "tok-1"))
            .expect("approval");
        ex.signal(signal_spec(&run, "cancel", "tok-1"))
            .expect("cancel");
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            2,
            "distinct signal_names are distinct rows"
        );
    }

    #[test]
    fn signal_payload_stores_as_a_reference_not_pii() {
        let ex = executor();
        let run = ex.start(spec("k")).expect("start");
        let mut sp = signal_spec(&run, "approval", "tok-1");
        sp.payload_key_ref = Some("kms://acme/epoch-1/content".into());
        ex.signal(sp).expect("deliver");
        let row = ex
            .signals()
            .get(&tenant(), &run.0, "approval", "tok-1")
            .expect("the buffered signal");
        assert_eq!(
            row.payload,
            vec![ArtifactRef("myelin://acme/agent/result/r0".into())]
        );
        assert_eq!(
            row.payload_key_ref.as_deref(),
            Some("kms://acme/epoch-1/content"),
            "the rare inline-PII payload names a crypto-shred key ref, never an inline body"
        );
        assert_eq!(
            row.consumed_seq, None,
            "a freshly-delivered signal is buffered, unconsumed (the wait is P-FLOW-11)"
        );
    }

    #[test]
    fn signal_to_unknown_run_is_surfaced() {
        let ex = executor();
        let err = ex
            .signal(signal_spec(&RunId("nope".into()), "job.done", "tok-1"))
            .expect_err("a signal to an unknown run is surfaced");
        assert_eq!(err, ExecutorError::UnknownRun("nope".into()));
        assert_eq!(
            ex.telemetry().signal_buffer_depth(),
            0,
            "nothing buffered for a phantom run"
        );
    }

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
        let input = ex.run_input(&run).expect("the started run's input");
        assert_eq!(input, vec![ArtifactRef("myelin://acme/git/pr/PR-9".into())]);
        assert_eq!(
            ex.run_budget(&run),
            None,
            "no budget → un-metered (the engine owns the loop-cap depth)"
        );
    }
}
