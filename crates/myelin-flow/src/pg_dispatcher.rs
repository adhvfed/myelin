use crate::engine::{run_state, DriveOutcome, SignalRow, SignalStore};
use crate::executor::{ExecutorError, PARTITION_COUNT};
use crate::pg_drive_store::{
    ActivityAttemptWrite, CommitOutcome, DriveCommit, DriveLease, DriveSnapshot, DriveStoreError,
    HistoryWrite, PgFlowDriveStore, SignalKey, TimerArm,
};
use crate::schema::{WfActivityAttemptRow, WfHistoryRow};
use crate::{PgFlowExecutor, TimerStore, WfCtx};
use myelin_events::{Actor, CausedBy, EmitContextBase, IdMinter, Timestamp, Ulid};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use sqlx::PgPool;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const MAX_TOKEN_BYTES: usize = 512;
const MAX_BATCH: usize = 1024;
pub const MAX_PG_RESOLVED_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const OPERATIONAL_PROBE_WF_TYPE: &str = "myelin.flow.operational-probe";

#[derive(Clone, Debug, PartialEq)]
pub struct PgClaimedDriveInput {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub wf_type: String,
    pub wf_version: i32,
    pub input: Vec<ArtifactRef>,
    pub budget: Option<serde_json::Value>,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub caused_by: Option<String>,
    pub depth: i32,
    pub partition: i16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PgResolvedDriveInput {
    pub claimed: PgClaimedDriveInput,
    pub material: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PgInputResolveError {
    Retry(String),
    Permanent(String),
}

pub trait PgWorkflowInputResolver: Send + Sync {
    fn resolve(
        &self,
        input: PgClaimedDriveInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PgInputResolveError>> + Send + '_>>;
}

impl<F, Fut> PgWorkflowInputResolver for F
where
    F: Fn(PgClaimedDriveInput) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Vec<u8>, PgInputResolveError>> + Send + 'static,
{
    fn resolve(
        &self,
        input: PgClaimedDriveInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PgInputResolveError>> + Send + '_>> {
        Box::pin((self)(input))
    }
}

impl From<&DriveLease> for PgClaimedDriveInput {
    fn from(lease: &DriveLease) -> Self {
        Self {
            tenant: lease.tenant.clone(),
            region: lease.region.clone(),
            run_id: lease.run_id.clone(),
            wf_type: lease.wf_type.clone(),
            wf_version: lease.wf_version,
            input: lease.input.clone(),
            budget: lease.budget.clone(),
            correlation_id: lease.correlation_id.clone(),
            causation_id: lease.causation_id.clone(),
            caused_by: lease.caused_by.clone(),
            depth: lease.depth,
            partition: lease.partition,
        }
    }
}

pub type PgWorkflowBody = dyn Fn(&PgClaimedDriveInput, &mut WfCtx) -> Result<Vec<ArtifactRef>, String>
    + Send
    + Sync
    + 'static;

pub type PgResolvedWorkflowBody = dyn Fn(&PgResolvedDriveInput, &mut WfCtx) -> Result<Vec<ArtifactRef>, String>
    + Send
    + Sync
    + 'static;

#[derive(Clone)]
struct RegisteredBody {
    body: Arc<PgResolvedWorkflowBody>,
    resolver: Option<Arc<dyn PgWorkflowInputResolver>>,
}

#[derive(Clone)]
pub struct PgWorkerScope {
    pub tenant: TenantId,
    pub region: Region,
    pub partition: i16,
    pub worker: String,
    pub lease_ttl_secs: i64,
    pub actor: Actor,
    pub schema_ver: u32,
}

impl PgWorkerScope {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tenant: TenantId,
        region: Region,
        partition: i16,
        worker: impl Into<String>,
        lease_ttl_secs: i64,
        actor: Actor,
        schema_ver: u32,
    ) -> Result<Self, PgWorkerError> {
        bounded("tenant", &tenant.0)?;
        bounded("region", &region.0)?;
        let worker = worker.into();
        bounded("worker", &worker)?;
        if !(0..PARTITION_COUNT as i16).contains(&partition) {
            return Err(PgWorkerError::InvalidConfig(format!(
                "partition must be between 0 and {}",
                PARTITION_COUNT - 1
            )));
        }
        if !(1..=300).contains(&lease_ttl_secs) {
            return Err(PgWorkerError::InvalidConfig(
                "lease TTL must be between 1 and 300 seconds".into(),
            ));
        }
        if actor.0.tenant != tenant || actor.0.region != region {
            return Err(PgWorkerError::InvalidConfig(
                "worker actor must be pinned to the same tenant and region".into(),
            ));
        }
        if schema_ver == 0 {
            return Err(PgWorkerError::InvalidConfig(
                "outbox schema version must be positive".into(),
            ));
        }
        Ok(Self {
            tenant,
            region,
            partition,
            worker,
            lease_ttl_secs,
            actor,
            schema_ver,
        })
    }

    pub fn from_env() -> Result<Self, PgWorkerError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self, PgWorkerError> {
        fn required(
            lookup: &mut impl FnMut(&str) -> Option<String>,
            name: &str,
        ) -> Result<String, PgWorkerError> {
            let value = lookup(name).ok_or_else(|| {
                PgWorkerError::InvalidConfig(format!(
                    "required environment variable {name} is missing"
                ))
            })?;
            bounded(name, &value)?;
            Ok(value)
        }

        let tenant = TenantId(required(&mut lookup, "MYELIN_FLOW_TENANT")?);
        let region = Region(required(&mut lookup, "MYELIN_FLOW_REGION")?);
        let partition = required(&mut lookup, "MYELIN_FLOW_PARTITION")?
            .parse::<i16>()
            .map_err(|_| {
                PgWorkerError::InvalidConfig("MYELIN_FLOW_PARTITION must be an integer".into())
            })?;
        let worker = required(&mut lookup, "MYELIN_FLOW_WORKER")?;
        let lease_ttl_secs = required(&mut lookup, "MYELIN_FLOW_LEASE_TTL_SECS")?
            .parse::<i64>()
            .map_err(|_| {
                PgWorkerError::InvalidConfig("MYELIN_FLOW_LEASE_TTL_SECS must be an integer".into())
            })?;
        let schema_ver = lookup("MYELIN_FLOW_SCHEMA_VER")
            .unwrap_or_else(|| "1".into())
            .parse::<u32>()
            .map_err(|_| {
                PgWorkerError::InvalidConfig(
                    "MYELIN_FLOW_SCHEMA_VER must be a positive integer".into(),
                )
            })?;
        let actor = Actor(myelin_identity::Principal::new(
            tenant.clone(),
            region.clone(),
            myelin_identity::PrincipalId(format!("svc:myelin-flow/{worker}")),
            myelin_identity::PrincipalKind::Service,
            myelin_identity::DataRole::Processor,
            myelin_identity::PrincipalStatus::Active,
        ));
        Self::new(
            tenant,
            region,
            partition,
            worker,
            lease_ttl_secs,
            actor,
            schema_ver,
        )
    }
}

fn bounded(label: &str, value: &str) -> Result<(), PgWorkerError> {
    if value.trim().is_empty() || value.len() > MAX_TOKEN_BYTES {
        return Err(PgWorkerError::InvalidConfig(format!(
            "{label} must be non-empty and at most {MAX_TOKEN_BYTES} bytes"
        )));
    }
    Ok(())
}

pub fn configured_production_definitions() -> Result<Vec<(String, i32)>, PgWorkerError> {
    configured_definitions_from(std::env::var("MYELIN_FLOW_DEFINITIONS").ok())
}

fn configured_definitions_from(value: Option<String>) -> Result<Vec<(String, i32)>, PgWorkerError> {
    let value = value.ok_or_else(|| {
        PgWorkerError::InvalidConfig(
            "required environment variable MYELIN_FLOW_DEFINITIONS is missing".into(),
        )
    })?;
    let mut definitions = Vec::new();
    for token in value
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let supported = format!("{OPERATIONAL_PROBE_WF_TYPE}@1");
        if token != supported {
            return Err(PgWorkerError::InvalidConfig(format!(
                "unsupported compiled workflow definition `{token}`; this binary currently supports only `{supported}`; ci.pipeline/merge bodies require their subsystem adapters"
            )));
        }
        if !definitions.contains(&(OPERATIONAL_PROBE_WF_TYPE.to_string(), 1)) {
            definitions.push((OPERATIONAL_PROBE_WF_TYPE.to_string(), 1));
        }
    }
    if definitions.is_empty() {
        return Err(PgWorkerError::InvalidConfig(
            "MYELIN_FLOW_DEFINITIONS must name at least one compiled definition".into(),
        ));
    }
    Ok(definitions)
}

#[derive(Debug)]
pub enum PgWorkerError {
    InvalidConfig(String),
    Definition(ExecutorError),
    Store(DriveStoreError),
    InputUnavailable(String),
    MissingDefinition { wf_type: String, version: i32 },
    Staging(String),
}

impl std::fmt::Display for PgWorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PgWorkerError {}

impl From<DriveStoreError> for PgWorkerError {
    fn from(value: DriveStoreError) -> Self {
        Self::Store(value)
    }
}

impl From<ExecutorError> for PgWorkerError {
    fn from(value: ExecutorError) -> Self {
        Self::Definition(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PgRunOnceOutcome {
    Idle,
    Driven {
        run_id: String,
        outcome: DriveOutcome,
        commit: CommitOutcome,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PgDriveBatch {
    pub driven: usize,
    pub saturated: bool,
}

const REPAIR_PROBE_CADENCE: u64 = 64;

pub struct PgFlowWorker {
    store: PgFlowDriveStore,
    executor: PgFlowExecutor,
    scope: PgWorkerScope,
    bodies: HashMap<(String, i32), RegisteredBody>,
    repair_probe: AtomicU64,
}

impl PgFlowWorker {
    pub fn new(
        pool: PgPool,
        rt: tokio::runtime::Handle,
        run_id_minter: Arc<dyn IdMinter>,
        scope: PgWorkerScope,
    ) -> Self {
        let store = PgFlowDriveStore::new(pool.clone(), scope.tenant.clone(), scope.region.clone());
        let executor = PgFlowExecutor::new(
            pool,
            rt,
            run_id_minter,
            scope.tenant.clone(),
            scope.region.clone(),
        );
        Self {
            store,
            executor,
            scope,
            bodies: HashMap::new(),
            repair_probe: AtomicU64::new(0),
        }
    }

    pub fn executor(&self) -> &PgFlowExecutor {
        &self.executor
    }

    pub fn register_definition<F>(
        &mut self,
        wf_type: &str,
        version: i32,
        code_hash: &str,
        legacy_body: F,
    ) -> Result<(), PgWorkerError>
    where
        F: Fn(&PgClaimedDriveInput, &mut WfCtx) -> Result<Vec<ArtifactRef>, String>
            + Send
            + Sync
            + 'static,
    {
        let body =
            move |input: &PgResolvedDriveInput, ctx: &mut WfCtx| legacy_body(&input.claimed, ctx);
        self.register_body(wf_type, version, code_hash, None, body)
    }

    pub fn register_definition_with_input_resolver<R, F>(
        &mut self,
        wf_type: &str,
        version: i32,
        code_hash: &str,
        resolver: R,
        body: F,
    ) -> Result<(), PgWorkerError>
    where
        R: PgWorkflowInputResolver + 'static,
        F: Fn(&PgResolvedDriveInput, &mut WfCtx) -> Result<Vec<ArtifactRef>, String>
            + Send
            + Sync
            + 'static,
    {
        self.register_body(wf_type, version, code_hash, Some(Arc::new(resolver)), body)
    }

    fn register_body<F>(
        &mut self,
        wf_type: &str,
        version: i32,
        code_hash: &str,
        resolver: Option<Arc<dyn PgWorkflowInputResolver>>,
        body: F,
    ) -> Result<(), PgWorkerError>
    where
        F: Fn(&PgResolvedDriveInput, &mut WfCtx) -> Result<Vec<ArtifactRef>, String>
            + Send
            + Sync
            + 'static,
    {
        self.executor
            .register_definition(wf_type, version, code_hash)?;
        let key = (wf_type.to_owned(), version);
        if self.bodies.contains_key(&key) {
            return Err(PgWorkerError::InvalidConfig(format!(
                "workflow body {wf_type}@{version} is already registered"
            )));
        }
        self.bodies.insert(
            key,
            RegisteredBody {
                body: Arc::new(body),
                resolver,
            },
        );
        Ok(())
    }

    pub async fn run_once(
        &self,
        now_unix_secs: i64,
        now_rfc3339: &str,
    ) -> Result<PgRunOnceOutcome, PgWorkerError> {
        bounded("drive clock", now_rfc3339)?;
        if self.bodies.is_empty() {
            return Err(PgWorkerError::InvalidConfig(
                "worker has no explicitly registered workflow definitions".into(),
            ));
        }
        let mut definitions: Vec<_> = self.bodies.keys().cloned().collect();
        definitions.sort();
        let mut claimed = None;

        let probe = self.repair_probe.fetch_add(1, Ordering::Relaxed) + 1;
        if probe.is_multiple_of(REPAIR_PROBE_CADENCE) {
            for (wf_type, version) in &definitions {
                claimed = self
                    .store
                    .claim_stranded_signal_wait(
                        self.scope.partition,
                        wf_type,
                        *version,
                        &self.scope.worker,
                        self.scope.lease_ttl_secs,
                    )
                    .await?;
                if claimed.is_some() {
                    break;
                }
            }
        }

        if claimed.is_none() {
            for (wf_type, version) in &definitions {
                claimed = self
                    .store
                    .claim_runnable_definition(
                        self.scope.partition,
                        wf_type,
                        *version,
                        &self.scope.worker,
                        self.scope.lease_ttl_secs,
                    )
                    .await?;
                if claimed.is_some() {
                    break;
                }
            }
        }
        if claimed.is_none() {
            for (wf_type, version) in &definitions {
                claimed = self
                    .store
                    .claim_stranded_signal_wait(
                        self.scope.partition,
                        wf_type,
                        *version,
                        &self.scope.worker,
                        self.scope.lease_ttl_secs,
                    )
                    .await?;
                if claimed.is_some() {
                    break;
                }
            }
        }
        let Some(lease) = claimed else {
            return Ok(PgRunOnceOutcome::Idle);
        };

        let result = self.drive_claimed(&lease, now_unix_secs, now_rfc3339).await;
        if result.is_err() {
            let _ = self.store.release_lease(&lease).await;
        }
        result
    }

    async fn drive_claimed(
        &self,
        lease: &DriveLease,
        now_unix_secs: i64,
        now_rfc3339: &str,
    ) -> Result<PgRunOnceOutcome, PgWorkerError> {
        let registered = self
            .bodies
            .get(&(lease.wf_type.clone(), lease.wf_version))
            .ok_or_else(|| PgWorkerError::MissingDefinition {
                wf_type: lease.wf_type.clone(),
                version: lease.wf_version,
            })?
            .clone();

        self.store
            .renew_lease(lease, self.scope.lease_ttl_secs)
            .await?;
        let DriveSnapshot {
            run: claimed_run,
            history: loaded_history,
            pending_signals,
        } = self.store.load_drive(lease).await?;
        let signals = SignalStore::new();
        for pending in pending_signals {
            signals.deliver(SignalRow {
                tenant: lease.tenant.clone(),
                region: lease.region.clone(),
                run_id: lease.run_id.clone(),
                signal_name: pending.signal_name,
                idem_key: pending.idem_key,
                payload: pending.payload,
                payload_key_ref: pending.payload_key_ref,
                received_unix_ms: pending.received_unix_ms,
                consumed_seq: None,
            });
        }
        let history = loaded_history
            .into_iter()
            .map(|row| WfHistoryRow {
                tenant: lease.tenant.clone(),
                region: lease.region.clone(),
                run_id: lease.run_id.clone(),
                seq: row.seq,
                kind: row.kind,
                command_id: row.command_id,
                result: row.result,
                result_key_ref: row.result_key_ref,
            })
            .collect();
        let emit_clock = Timestamp(lease.created_at_rfc3339.clone());
        let ctx_base = EmitContextBase {
            tenant: lease.tenant.clone(),
            region: lease.region.clone(),
            actor: self.scope.actor.clone(),
            schema_ver: self.scope.schema_ver,
            occurred_at: emit_clock.clone(),
            recorded_at: emit_clock,
            caused_by: lease.caused_by.clone().map(CausedBy),
        };
        let timers = TimerStore::new();
        let drive_input = PgClaimedDriveInput::from(&claimed_run);
        let resolved_material = if let Some(resolver) = registered.resolver {
            match self
                .resolve_input_under_lease(lease, resolver, drive_input.clone())
                .await?
            {
                Ok(material) => Ok(material),
                Err(PgInputResolveError::Permanent(detail)) => Err(detail),
                Err(PgInputResolveError::Retry(detail)) => {
                    return Err(PgWorkerError::InputUnavailable(detail));
                }
            }
        } else {
            Ok(Vec::new())
        };

        if let Err(reason) = &resolved_material {
            let outcome = DriveOutcome::Nondeterministic(format!(
                "immutable workflow input permanently refused: {reason}"
            ));
            self.store
                .renew_lease(lease, self.scope.lease_ttl_secs)
                .await?;
            let commit = build_commit(lease, &outcome, None)?;
            let commit_outcome = self.store.commit_drive(lease, commit).await?;
            return Ok(PgRunOnceOutcome::Driven {
                run_id: lease.run_id.clone(),
                outcome,
                commit: commit_outcome,
            });
        }
        let resolved_input = PgResolvedDriveInput {
            claimed: drive_input,
            material: resolved_material.expect("permanent resolution failure returned above"),
        };
        let mut ctx = WfCtx::resume_staged_versioned(
            Arc::new(DriveIdMinter::new(&lease.run_id)),
            ctx_base,
            lease.run_id.clone(),
            lease.wf_type.clone(),
            now_rfc3339,
            stable_seed(&lease.run_id),
            history,
            lease.wf_version,
            lease.wf_version,
        )
        .with_timers(timers, lease.partition, now_unix_secs)
        .with_signals(signals);

        let body = registered.body;
        let mut body_task = tokio::task::spawn_blocking(move || {
            let result = body(&resolved_input, &mut ctx);
            (ctx, result)
        });
        let renew_every = Duration::from_secs((self.scope.lease_ttl_secs as u64 / 3).max(1));
        let mut heartbeat =
            tokio::time::interval_at(tokio::time::Instant::now() + renew_every, renew_every);
        let mut heartbeat_failure = None;
        let (ctx, body_result) = loop {
            tokio::select! {
                result = &mut body_task => {
                    break result.map_err(|error| {
                        PgWorkerError::Staging(format!("workflow body task failed: {error}"))
                    })?;
                }
                _ = heartbeat.tick(), if heartbeat_failure.is_none() => {
                    if let Err(error) = self.store.renew_lease(lease, self.scope.lease_ttl_secs).await {
                        heartbeat_failure = Some(error);
                    }
                }
            }
        };
        if let Some(error) = heartbeat_failure {
            return Err(PgWorkerError::Store(error));
        }
        let divergence = ctx.divergence().map(ToOwned::to_owned);
        let parked = ctx.parked();
        let outcome = if let Some(reason) = divergence {
            DriveOutcome::Nondeterministic(reason)
        } else {
            match body_result {
                Ok(_) if parked => DriveOutcome::Waiting,
                Ok(result) => DriveOutcome::Completed(result),
                Err(error) => DriveOutcome::Failed(error),
            }
        };
        let staged = if matches!(outcome, DriveOutcome::Nondeterministic(_)) {
            None
        } else {
            Some(
                ctx.into_staged_drive()
                    .map_err(|error| PgWorkerError::Staging(format!("{error:?}")))?,
            )
        };

        self.store
            .renew_lease(lease, self.scope.lease_ttl_secs)
            .await?;
        let commit = build_commit(lease, &outcome, staged)?;
        let commit_outcome = self.store.commit_drive(lease, commit).await?;
        Ok(PgRunOnceOutcome::Driven {
            run_id: lease.run_id.clone(),
            outcome,
            commit: commit_outcome,
        })
    }

    async fn resolve_input_under_lease(
        &self,
        lease: &DriveLease,
        resolver: Arc<dyn PgWorkflowInputResolver>,
        input: PgClaimedDriveInput,
    ) -> Result<Result<Vec<u8>, PgInputResolveError>, PgWorkerError> {
        let resolution = resolver.resolve(input);
        tokio::pin!(resolution);
        let renew_every = Duration::from_secs((self.scope.lease_ttl_secs as u64 / 3).max(1));
        let mut heartbeat =
            tokio::time::interval_at(tokio::time::Instant::now() + renew_every, renew_every);
        loop {
            tokio::select! {
                result = &mut resolution => {
                    return Ok(result.and_then(|material| {
                        if material.len() > MAX_PG_RESOLVED_INPUT_BYTES {
                            Err(PgInputResolveError::Permanent(format!(
                                "resolved input is {} bytes; maximum is {MAX_PG_RESOLVED_INPUT_BYTES}",
                                material.len()
                            )))
                        } else {
                            Ok(material)
                        }
                    }));
                }
                _ = heartbeat.tick() => {
                    self.store
                        .renew_lease(lease, self.scope.lease_ttl_secs)
                        .await?;
                }
            }
        }
    }

    pub async fn run_until_idle(
        &self,
        max_runs: usize,
        now_unix_secs: i64,
        now_rfc3339: &str,
    ) -> Result<PgDriveBatch, PgWorkerError> {
        self.run_until_idle_inner(max_runs, now_unix_secs, now_rfc3339, None)
            .await
    }

    pub async fn run_until_idle_or_shutdown(
        &self,
        max_runs: usize,
        now_unix_secs: i64,
        now_rfc3339: &str,
        shutdown: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<PgDriveBatch, PgWorkerError> {
        self.run_until_idle_inner(max_runs, now_unix_secs, now_rfc3339, Some(shutdown))
            .await
    }

    async fn run_until_idle_inner(
        &self,
        max_runs: usize,
        now_unix_secs: i64,
        now_rfc3339: &str,
        shutdown: Option<&tokio::sync::watch::Receiver<bool>>,
    ) -> Result<PgDriveBatch, PgWorkerError> {
        if !(1..=MAX_BATCH).contains(&max_runs) {
            return Err(PgWorkerError::InvalidConfig(format!(
                "max_runs must be between 1 and {MAX_BATCH}"
            )));
        }
        let mut driven = 0;
        while driven < max_runs {
            if shutdown.is_some_and(|receiver| *receiver.borrow()) {
                return Ok(PgDriveBatch {
                    driven,
                    saturated: true,
                });
            }
            match self.run_once(now_unix_secs, now_rfc3339).await? {
                PgRunOnceOutcome::Idle => {
                    return Ok(PgDriveBatch {
                        driven,
                        saturated: false,
                    })
                }
                PgRunOnceOutcome::Driven { .. } => driven += 1,
            }
        }
        Ok(PgDriveBatch {
            driven,
            saturated: true,
        })
    }

    pub async fn run_until_shutdown(
        &self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
        poll_interval: Duration,
        max_batch: usize,
    ) -> Result<(), PgWorkerError> {
        if poll_interval.is_zero() || max_batch == 0 || max_batch > MAX_BATCH {
            return Err(PgWorkerError::InvalidConfig(
                "runtime poll interval and batch bound must be positive and bounded".into(),
            ));
        }
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            let (secs, stamp) = system_clock()?;
            let _ = self
                .store
                .fire_due_timer(self.scope.partition, secs)
                .await?;
            self.run_until_idle_or_shutdown(max_batch, secs, &stamp, &shutdown)
                .await?;
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { return Ok(()); }
                }
                _ = tokio::time::sleep(poll_interval) => {}
            }
        }
    }
}

fn build_commit(
    lease: &DriveLease,
    outcome: &DriveOutcome,
    staged: Option<crate::StagedWfDrive>,
) -> Result<DriveCommit, PgWorkerError> {
    let staged = staged.unwrap_or_else(|| crate::StagedWfDrive {
        history: Vec::new(),
        attempts: Vec::new(),
        timers: Vec::new(),
        outbox: Vec::new(),
        consumed_signals: Vec::new(),
        disarmed_timer_ids: Vec::new(),
        park: None,
    });
    let signal_keys: HashMap<_, _> = staged
        .consumed_signals
        .into_iter()
        .map(|signal| {
            (
                signal.command_id,
                SignalKey {
                    signal_name: signal.signal_name,
                    idem_key: signal.idem_key,
                },
            )
        })
        .collect();
    let history = staged
        .history
        .into_iter()
        .map(|row| HistoryWrite {
            consume_signal: signal_keys.get(&row.command_id).cloned(),
            seq: row.seq,
            kind: row.kind,
            command_id: row.command_id,
            result: row.result,
            result_key_ref: row.result_key_ref,
        })
        .collect();
    let attempts = staged.attempts.into_iter().map(map_attempt).collect();
    let timers = staged
        .timers
        .into_iter()
        .map(|row| TimerArm {
            timer_id: row.timer_id,
            command_id: row.command_id,
            fire_at_unix_secs: row.fire_at,
            partition: row.partition,
        })
        .collect();
    let next_state = match outcome {
        DriveOutcome::Completed(_) => run_state::COMPLETED,
        DriveOutcome::Failed(_) => run_state::FAILED,
        DriveOutcome::Waiting => run_state::WAITING,
        DriveOutcome::Nondeterministic(_) => run_state::NONDETERMINISTIC,
    };
    let park = match outcome {
        DriveOutcome::Waiting => staged.park,
        _ => None,
    };
    Ok(DriveCommit {
        drive_id: format!(
            "{}/cursor-{}/epoch-{}",
            lease.run_id, lease.cursor, lease.lease_epoch
        ),
        expected_cursor: lease.cursor,
        next_state: next_state.into(),
        history,
        attempts,
        timers,
        timer_disarms: staged.disarmed_timer_ids,
        outbox: staged.outbox,
        park,
    })
}

fn map_attempt(row: WfActivityAttemptRow) -> ActivityAttemptWrite {
    ActivityAttemptWrite {
        command_id: row.command_id,
        attempt: row.attempt,
        idem_token: row.idem_token,
        state: row.state,
        error: row.error,
        started_unix_ms: None,
        ended_unix_ms: None,
    }
}

fn stable_seed(run_id: &str) -> u64 {
    let hash = blake3::hash(run_id.as_bytes());
    u64::from_be_bytes(hash.as_bytes()[..8].try_into().expect("eight hash bytes"))
}

struct DriveIdMinter {
    prefix: u64,
    ordinal: AtomicU64,
}

impl DriveIdMinter {
    fn new(run_id: &str) -> Self {
        Self {
            prefix: stable_seed(run_id),
            ordinal: AtomicU64::new(0),
        }
    }

    fn render(mut value: u128) -> String {
        const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let mut bytes = [0u8; 26];
        for slot in bytes.iter_mut().rev() {
            *slot = CROCKFORD[(value & 0x1f) as usize];
            value >>= 5;
        }
        String::from_utf8(bytes.to_vec()).expect("Crockford alphabet is ASCII")
    }
}

impl IdMinter for DriveIdMinter {
    fn mint(&self) -> Ulid {
        let ordinal = self.ordinal.fetch_add(1, Ordering::SeqCst);
        Ulid(Self::render(
            (u128::from(self.prefix) << 64) | u128::from(ordinal),
        ))
    }
}

fn system_clock() -> Result<(i64, String), PgWorkerError> {
    let now = chrono::Utc::now();
    Ok((
        now.timestamp(),
        now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{AggregateKey, DataRole, EventDraft, EventType, Visibility};
    use myelin_identity::{
        DataRole as IdentityDataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
    };

    fn actor() -> Actor {
        Actor(Principal::new(
            TenantId("acme".into()),
            Region("eu-north".into()),
            PrincipalId("svc:flow".into()),
            PrincipalKind::Service,
            IdentityDataRole::Processor,
            PrincipalStatus::Active,
        ))
    }

    #[test]
    fn scope_refuses_empty_or_cross_region_configuration() {
        assert!(PgWorkerScope::new(
            TenantId(String::new()),
            Region("eu-north".into()),
            0,
            "worker",
            30,
            actor(),
            1,
        )
        .is_err());
        assert!(PgWorkerScope::new(
            TenantId("acme".into()),
            Region("wrong".into()),
            0,
            "worker",
            30,
            actor(),
            1,
        )
        .is_err());
    }

    #[test]
    fn production_scope_refuses_missing_or_empty_required_values() {
        assert!(PgWorkerScope::from_lookup(|_| None).is_err());
        let values = HashMap::from([
            ("MYELIN_FLOW_TENANT", "acme"),
            ("MYELIN_FLOW_REGION", "eu-north"),
            ("MYELIN_FLOW_PARTITION", "0"),
            ("MYELIN_FLOW_WORKER", ""),
            ("MYELIN_FLOW_LEASE_TTL_SECS", "30"),
        ]);
        assert!(PgWorkerScope::from_lookup(|name| values.get(name).map(|v| (*v).into())).is_err());
    }

    #[test]
    fn production_definition_allowlist_is_explicit_and_rejects_product_stubs() {
        assert!(configured_definitions_from(None).is_err());
        assert!(configured_definitions_from(Some(String::new())).is_err());
        assert!(configured_definitions_from(Some("ci.pipeline@1".into())).is_err());
        assert_eq!(
            configured_definitions_from(Some(format!("{OPERATIONAL_PROBE_WF_TYPE}@1"))).unwrap(),
            [(OPERATIONAL_PROBE_WF_TYPE.to_string(), 1)]
        );
    }

    #[test]
    fn claimed_drive_input_projects_pinned_body_data_without_lease_authority() {
        let lease = DriveLease {
            tenant: TenantId("acme".into()),
            region: Region("eu-north".into()),
            run_id: "run-1".into(),
            wf_type: "ci.pipeline".into(),
            wf_version: 3,
            input: vec![ArtifactRef("myelin://acme/ci/run/1".into())],
            budget: Some(serde_json::json!({"minor_units": 42})),
            correlation_id: "corr-1".into(),
            causation_id: Some("cause-1".into()),
            caused_by: Some("event-1".into()),
            depth: 2,
            partition: 7,
            cursor: 4,
            lease_owner: "worker-secret".into(),
            lease_epoch: 9,
            lease_expires_unix_ms: 123_000,
            created_at_rfc3339: "2026-07-18T00:00:00Z".into(),
        };

        let input = PgClaimedDriveInput::from(&lease);
        assert_eq!(input.run_id, "run-1");
        assert_eq!(input.wf_version, 3);
        assert_eq!(input.input, lease.input);
        assert_eq!(input.budget, lease.budget);
        assert_eq!(input.correlation_id, "corr-1");
        assert_eq!(input.partition, 7);
    }

    #[test]
    fn drive_event_ids_and_command_ids_are_stable_across_replay() {
        fn stage() -> (String, String) {
            let mut ctx = WfCtx::resume_staged_versioned(
                Arc::new(DriveIdMinter::new("run-1")),
                EmitContextBase {
                    tenant: TenantId("acme".into()),
                    region: Region("eu-north".into()),
                    actor: actor(),
                    schema_ver: 1,
                    occurred_at: Timestamp("2026-07-18T00:00:00Z".into()),
                    recorded_at: Timestamp("2026-07-18T00:00:00Z".into()),
                    caused_by: None,
                },
                "run-1",
                "wf.test",
                "2026-07-18T00:00:00Z",
                stable_seed("run-1"),
                Vec::new(),
                1,
                1,
            );
            ctx.now();
            ctx.emit(
                EventDraft {
                    type_: EventType("flow.test.completed".into()),
                    subject: ArtifactRef("myelin://acme/flow/run/run-1".into()),
                    aggregate: AggregateKey("flow:run-1".into()),
                    payload: serde_json::json!({"run_ref":"myelin://acme/flow/run/run-1"}),
                    data_role: DataRole::Processor,
                    visibility: Visibility::Internal,
                    contains_personal_data: false,
                    pii_key_ref: None,
                },
                None,
            )
            .unwrap();
            let staged = ctx.into_staged_drive().unwrap();
            (
                staged.outbox[0].event_id.0.clone(),
                staged.history[0].command_id.clone(),
            )
        }
        assert_eq!(stage(), stage());
    }
}
