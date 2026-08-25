use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind as SandboxJobKind, JobSpec as SandboxJobSpec,
    MeterTarget, ResourceLimits, RunTokenCredential, TrustTier, WorkspaceSpec,
};
use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    DriveOutcome, DurableExecutor, ExecutorError, FlowDispatcher, FlowExecutor, FlowTelemetry,
    JobSpec as FlowJobSpec, PgFlowExecutor, RunId, StartSpec, TimerStore, WfCtx, WfJournal,
    WorkflowBody, CI_PIPELINE_WF_TYPE, PARTITION_COUNT,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::ci_pipeline::{run_ci_pipeline_body, PipelineRun, RunVerdict};
use crate::ci_run_store::CiRunRecord;
use crate::job_queue_store::{trust_from_token, CiJobQueueStore, JobQueueStoreError};
use crate::job_schedule::JobScheduleTerms;
use crate::job_spec_store::CiJobSpecStore;
use crate::scheduler::Lane;

use super::{CiPipelineReporter, DurableJobRunner, StageSpecBuilder};

#[derive(Clone)]
struct RunPlan {
    pipeline: PipelineRun,
    terms: JobScheduleTerms,
}

pub struct CiPipelineDriver {
    executor: FlowExecutor,
    pg_executor: PgFlowExecutor,
    tenant: TenantId,
    region: String,
    journal: WfJournal,
    outbox: OutboxStore,
    telemetry: FlowTelemetry,
    timers: TimerStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    spec_store: CiJobSpecStore,
    rt: tokio::runtime::Handle,
    build_spec: StageSpecBuilder,
    plans: Arc<Mutex<HashMap<String, RunPlan>>>,
    started: Arc<Mutex<Vec<String>>>,
}

impl CiPipelineDriver {
    pub fn new(
        tenant: TenantId,
        region: impl Into<String>,
        spec_store: CiJobSpecStore,
        rt: tokio::runtime::Handle,
        build_spec: StageSpecBuilder,
        outbox: OutboxStore,
    ) -> CiPipelineDriver {
        let region = region.into();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let executor = FlowExecutor::new(minter.clone(), tenant.clone(), Region(region.clone()));
        let pg_executor = PgFlowExecutor::new(
            spec_store.pool().clone(),
            rt.clone(),
            minter.clone(),
            tenant.clone(),
            Region(region.clone()),
        );
        executor.register_definition(CI_PIPELINE_WF_TYPE);
        CiPipelineDriver {
            executor,
            pg_executor,
            tenant: tenant.clone(),
            region: region.clone(),
            journal: WfJournal::new(),
            outbox,
            telemetry: FlowTelemetry::new(),
            timers: TimerStore::new(),
            minter,
            ctx_base: service_ctx_base(&tenant, &region),
            spec_store,
            rt,
            build_spec,
            plans: Arc::new(Mutex::new(HashMap::new())),
            started: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn executor(&self) -> FlowExecutor {
        self.executor.clone()
    }

    pub fn reporter(&self) -> CiPipelineReporter {
        CiPipelineReporter::new(
            self.pg_executor.clone(),
            self.spec_store.clone(),
            CiJobQueueStore::with_pg(self.spec_store.pool().clone()),
            self.rt.clone(),
            self.tenant.clone(),
            self.region.clone(),
        )
        .with_test_executor(self.executor.clone())
    }

    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    pub fn start_run(
        &self,
        record: &CiRunRecord,
        pipeline: PipelineRun,
        labels: Vec<String>,
    ) -> Result<RunId, StartRunError> {
        validate_driver_tenant(&self.tenant, record)?;
        let trust_tier = trust_from_token(&record.trust_tier).map_err(StartRunError::TrustTier)?;
        let terms = JobScheduleTerms {
            tenant_id: record.tenant_id.clone(),
            region: record.region.clone(),
            run_id: record.wf_run_id.clone(),
            lane: Lane::Interactive,
            labels,
            trust_tier,
            concurrency_group: None,
            fair_key: record.tenant_id.clone(),
        };
        self.plans
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(record.wf_run_id.clone(), RunPlan { pipeline, terms });
        {
            let mut started = self
                .started
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !started.contains(&record.wf_run_id) {
                started.push(record.wf_run_id.clone());
            }
        }
        self.pg_executor
            .register_definition(CI_PIPELINE_WF_TYPE, 1, "blake3:ci-pipeline-driver-v1")
            .map_err(StartRunError::Start)?;
        let durable = self
            .pg_executor
            .start_with_id(
                StartSpec {
                    wf_type: CI_PIPELINE_WF_TYPE.into(),
                    input: vec![],
                    budget: None,
                    idem_key: format!("ci:{}", record.run_id),
                },
                Some(RunId(record.wf_run_id.clone())),
            )
            .map_err(StartRunError::Start)?;
        let memory = self
            .executor
            .start_with_id(
                StartSpec {
                    wf_type: CI_PIPELINE_WF_TYPE.into(),
                    input: vec![],
                    budget: None,
                    idem_key: format!("ci:{}", record.run_id),
                },
                Some(RunId(record.wf_run_id.clone())),
            )
            .map_err(StartRunError::Start)?;
        if durable != memory {
            return Err(StartRunError::Start(ExecutorError::RunIdConflict(
                record.wf_run_id.clone(),
            )));
        }
        Ok(durable)
    }

    fn body(&self) -> Box<WorkflowBody> {
        let plans = self.plans.clone();
        let spec_store = self.spec_store.clone();
        let rt = self.rt.clone();
        let build_spec = self.build_spec.clone();
        Box::new(move |ctx: &mut WfCtx| {
            let run_id = ctx.run_id().to_string();
            let plan = plans
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&run_id)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "no PipelineRun registered for ci.pipeline run `{run_id}` - the starter must \
                         register the plan before start_with_id (CT-004d.2 chunk 3)"
                    )
                })?;
            let runner = DurableJobRunner::new(
                spec_store.clone(),
                rt.clone(),
                plan.terms.clone(),
                build_spec.clone(),
                &plan.pipeline.stages,
            );
            let verdict = run_ci_pipeline_body(ctx, &plan.pipeline, &runner)
                .map_err(|error| format!("{error:?}"))?;
            Ok(match verdict {
                RunVerdict::Succeeded { stages_completed } => {
                    vec![ArtifactRef(format!("outcome:succeeded:{stages_completed}"))]
                }
                RunVerdict::Failed { stage } => {
                    vec![ArtifactRef(format!("outcome:failed:{stage}"))]
                }
                RunVerdict::Rejected { stage } => {
                    vec![ArtifactRef(format!("outcome:rejected:{stage}"))]
                }
                RunVerdict::Parked => vec![],
            })
        })
    }

    fn dispatcher(&self, partition: i16) -> FlowDispatcher {
        let mut dispatcher = FlowDispatcher::new(
            self.executor.runs().clone(),
            self.outbox.clone(),
            self.journal.clone(),
            self.telemetry.clone(),
            self.minter.clone(),
            self.ctx_base.clone(),
            partition,
            "ci-pipeline-driver",
            30,
        )
        .with_signals(self.executor.signals().clone())
        .with_timers(self.timers.clone());
        dispatcher.register(CI_PIPELINE_WF_TYPE, self.body());
        dispatcher
    }

    pub fn drive_once(&self, now: i64, now_clock: &str) -> Vec<DriveOutcome> {
        for run_id in self
            .started
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
        {
            self.executor.runs().wake(&self.tenant, run_id);
        }
        let mut outcomes = Vec::new();
        for partition in 0..PARTITION_COUNT as i16 {
            let dispatcher = self.dispatcher(partition);
            if let Some(outcome) = dispatcher.tick(now, now_clock, 7) {
                outcomes.push(outcome);
            }
        }
        outcomes
    }

    pub fn is_terminal(&self, run: &RunId) -> Option<bool> {
        self.executor
            .describe(run)
            .ok()
            .map(|status| status.terminal)
    }

    pub fn run_state(&self, run: &RunId) -> Option<String> {
        self.executor.describe(run).ok().map(|status| status.state)
    }

    pub fn region(&self) -> &str {
        &self.region
    }
}

#[derive(Debug)]
pub enum StartRunError {
    TenantMismatch {
        driver_tenant: String,
        record_tenant: String,
    },
    TrustTier(JobQueueStoreError),
    Start(ExecutorError),
}

impl std::fmt::Display for StartRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartRunError::TenantMismatch {
                driver_tenant,
                record_tenant,
            } => write!(
                f,
                "ci.pipeline start refused: driver tenant `{driver_tenant}` does not match durable ci_run tenant `{record_tenant}`"
            ),
            StartRunError::TrustTier(error) => {
                write!(f, "ci.pipeline start refused: corrupt trust_tier token: {error}")
            }
            StartRunError::Start(error) => write!(f, "ci.pipeline start_with_id failed: {error}"),
        }
    }
}

impl std::error::Error for StartRunError {}

pub(super) fn validate_driver_tenant(
    driver_tenant: &TenantId,
    record: &CiRunRecord,
) -> Result<(), StartRunError> {
    if driver_tenant.0 == record.tenant_id {
        Ok(())
    } else {
        Err(StartRunError::TenantMismatch {
            driver_tenant: driver_tenant.0.clone(),
            record_tenant: record.tenant_id.clone(),
        })
    }
}

fn service_ctx_base(tenant: &TenantId, region: &str) -> EmitContextBase {
    EmitContextBase {
        tenant: tenant.clone(),
        region: Region(region.to_string()),
        actor: Actor(Principal::stub(
            PrincipalId("ci-controlplane".into()),
            PrincipalKind::Service,
            tenant.clone(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-07-17T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-17T00:00:00Z".into()),
        caused_by: None,
    }
}

pub fn fixed_command_spec_builder(
    image: &str,
    command: Vec<String>,
    timeout_secs: u32,
) -> Result<StageSpecBuilder, String> {
    let image = ImageRef::pinned(image).map_err(|error| error.to_string())?;
    Ok(Arc::new(move |_flow_spec: &FlowJobSpec| {
        SandboxJobSpec::new(
            SandboxJobKind::Ci,
            image.clone(),
            command.clone(),
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1 << 30,
                tmpfs_bytes: 1 << 30,
                pids_max: 128,
                timeout_secs,
            },
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            RunTokenCredential::new("ci-pipeline-driver-bearer", "ci-pipeline-driver-jti", 300)
                .expect("static driver credential is valid"),
            MeterTarget {
                reserve_id: "ci-pipeline-driver-reserve".into(),
            },
            IdemToken(String::new()),
        )
        .map_err(|error| error.to_string())
    }))
}
