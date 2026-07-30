//! Live PostgreSQL proof for the exact-cell CI run starter.
#![cfg(feature = "integration")]

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use common::with_schema_cleanup;
use myelin_ci_controlplane::{
    ci_artifact_ref, ci_job_id_v2, ci_production_runtime_factory_test_support,
    ci_region_run_discovery_test_support, ci_run_ref, ci_run_starter_factory, CiDriveManifestStore,
    CiExecutionProfileV1, CiExecutionRequestV1, CiJobLaunchClaim, CiJobLaunchGrantV1,
    CiJobQueueStore, CiJobSpecStore, CiLaunchAuthorityError, CiLaunchAuthorityMaterializer,
    CiLaunchAuthorityV1, CiManifestLaneV1, CiManifestLimitsV1, CiManifestSchedulingV1,
    CiRunSupersessionError, CiWorkflowDefinitionPin, DurableCiJobLaunchTemplate, DurableEnqueue,
    GrantedCiJobV1, Lane, PgCiPipelineStarter, PgCiRunStarterFactory, PgCiRunStarterPoller,
    PgCiStarterError, PreparedRunPlanV2, ResolvedJobV2, ResolvedRunPlanV2, StartQueuedOutcome,
    ALTER_CI_JOB_ACCOUNTING_ADD_SKIPPED_DDL, ALTER_CI_JOB_SPEC_ADD_STAGE_DDL,
    ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL, ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL,
    ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL, ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
    ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL, BUMP_CHECK_ATTEMPT_SQL, CI_JOB_RUN_LEDGER_INDEX,
    CREATE_CHECK_ATTEMPT_DDL, CREATE_CI_COST_EVENT_DDL, CREATE_CI_DRIVE_MANIFEST_DDL,
    CREATE_CI_JOB_ACCOUNTING_DDL, CREATE_CI_JOB_DDL, CREATE_CI_JOB_RUN_LEDGER_INDEX_DDL,
    CREATE_CI_JOB_SPEC_DDL, CREATE_CI_RUN_CHECK_ATTEMPT_DDL, CREATE_CI_RUN_DDL,
    CREATE_JOB_QUEUE_DDL,
};
use myelin_ci_sandbox::{
    CompletionClaim, EgressPolicy, EnvVar, IdemToken, ImageRef, JobKind, JobSpecTemplate,
    MeterTarget, ResourceLimits, ResourceUsage, SecretRef, TerminalReport, TerminalReporter,
    TrustTier, WorkspaceSpec,
};
use myelin_config::MyelinConfig;
use myelin_events::MonotonicMinter;
use myelin_flow::{
    migrations::migrations as flow_migrations, DurableExecutor, PgFlowExecutor, RunId,
    SignalOutcome, StartSpec, CI_PIPELINE_WF_TYPE,
};
use myelin_refs::ArtifactRef;
use myelin_storage::{
    provider::foundation_migrations, reserve_settle_durable_migrations, BlobError, BlobMeta,
    BlobStore, ContentHash, DurableCostLedger, FsBlobStore, HotTables, PgMigrator,
    SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool};

/// Concurrent `#[tokio::test]` functions in this file each run their own `PgMigrator` sequence
/// against the same live PostgreSQL instance; running two migration sequences at once can hit a
/// genuine advisory-lock deadlock (not just contention) rather than a benign wait, so every test
/// that runs migrations serializes on this guard for its migration-touching span, mirroring
/// `MIGRATION_SCENARIO_LOCK` in `integration_ci_terminal_accounting_atomic.rs`.
static MIGRATION_SCENARIO_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const BODY_HASH: &str = "blake3:ci-pg-body-v1";
const BODY_HASH_V2: &str = "blake3:ci-pg-body-v2";
/// A syntactically valid (40-hex-char, SHA-1-shaped) placeholder commit id. The real production
/// launch authority (`checkout_scope_for_run` in `ci_launch_authority.rs`) requires a full
/// 40-character (SHA-1) or 64-character (SHA-256) lowercase-hex commit object id and refuses
/// anything shorter (e.g. the old `"deadbeef"` placeholder), so every fixture that may reach that
/// real authority must use this instead.
const TEST_COMMIT_OID: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
static AUTHORITY_CALLS: AtomicUsize = AtomicUsize::new(0);

struct PausingBlobStore {
    inner: Arc<FsBlobStore>,
    pause_once: AtomicBool,
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl BlobStore for PausingBlobStore {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> Result<ContentHash, BlobError> {
        self.inner.put(tenant, bytes)
    }

    fn get(&self, tenant: &TenantId, hash: &ContentHash) -> Result<Vec<u8>, BlobError> {
        self.inner.get(tenant, hash)
    }

    fn head(&self, tenant: &TenantId, hash: &ContentHash) -> Result<BlobMeta, BlobError> {
        if !self.pause_once.swap(true, Ordering::SeqCst) {
            self.entered.send(()).expect("announce CAS preflight");
            self.release
                .lock()
                .expect("release lock")
                .recv()
                .expect("release CAS preflight");
        }
        self.inner.head(tenant, hash)
    }

    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> Result<(), BlobError> {
        self.inner.delete(tenant, hash)
    }
}

fn app_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| MyelinConfig::dev().database_url)
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn schema_name() -> String {
    format!("ci_pg_starter_{}", std::process::id())
}

async fn pool_on(url: &str, schema: &str) -> PgPool {
    let schema = schema.to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(12)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                connection
                    .execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("connect to the development PostgreSQL stack")
}

fn plan() -> ResolvedRunPlanV2 {
    let mut matrix = BTreeMap::new();
    matrix.insert("os".into(), "linux".into());
    let test_name = myelin_ci_controlplane::derive_concrete_job_name("test", &matrix);
    ResolvedRunPlanV2 {
        schema_version: 2,
        execution: CiExecutionRequestV1 {
            schema_version: 1,
            profile: CiExecutionProfileV1::LinuxSmallV1,
        },
        jobs: vec![
            ResolvedJobV2 {
                stage: "build".into(),
                name: "build".into(),
                image: format!("registry.example/build@sha256:{}", "a".repeat(64)),
                command: vec!["/bin/build".into(), "--locked".into()],
                needs: Vec::new(),
                is_generator: false,
                matrix_key: BTreeMap::new(),
            },
            ResolvedJobV2 {
                stage: "package".into(),
                name: "package".into(),
                image: format!("registry.example/package@sha256:{}", "b".repeat(64)),
                command: vec!["/bin/package".into()],
                needs: vec!["build".into(), test_name.clone()],
                is_generator: false,
                matrix_key: BTreeMap::new(),
            },
            ResolvedJobV2 {
                stage: "test".into(),
                name: test_name,
                image: format!("registry.example/test@sha256:{}", "c".repeat(64)),
                command: vec!["/bin/test".into()],
                needs: vec!["build".into()],
                is_generator: false,
                matrix_key: matrix,
            },
        ],
    }
}

#[derive(Clone, Debug)]
struct TestLaunchAuthority;

fn test_launch_authority(
    record: &myelin_ci_controlplane::ci_run_store::CiRunRecord,
    prepared: &PreparedRunPlanV2,
) -> CiLaunchAuthorityV1 {
    CiLaunchAuthorityV1 {
        policy_revision: "test-policy-v1".into(),
        jobs: prepared
            .plan()
            .jobs
            .iter()
            .map(|job| CiJobLaunchGrantV1 {
                concrete_name: job.name.clone(),
                env: BTreeMap::new(),
                secret_handles: BTreeMap::new(),
                egress_allow: Vec::new(),
                limits: CiManifestLimitsV1 {
                    cpu_millis: 1_000,
                    mem_bytes: 1_073_741_824,
                    disk_bytes: 2_147_483_648,
                    pids_max: 128,
                    timeout_secs: 600,
                },
                scheduling: CiManifestSchedulingV1 {
                    lane: CiManifestLaneV1::Batch,
                    labels: vec!["linux".into()],
                    concurrency_group: None,
                    fair_key: format!("project:{}", record.project_id),
                },
                reserve_handle: format!("reserve:{}:{}", record.run_id, job.name),
                token_authority_handle: format!("mint:{}:{}", record.run_id, job.name),
            })
            .collect(),
        merge_waiter: None,
    }
}

impl CiLaunchAuthorityMaterializer for TestLaunchAuthority {
    fn materialize<'a>(
        &'a self,
        record: &'a myelin_ci_controlplane::ci_run_store::CiRunRecord,
        prepared: &'a PreparedRunPlanV2,
        _definition: &'a CiWorkflowDefinitionPin,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<CiLaunchAuthorityV1, CiLaunchAuthorityError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            AUTHORITY_CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(test_launch_authority(record, prepared))
        })
    }
}

#[derive(Clone, Debug)]
struct PausingLaunchAuthority {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl CiLaunchAuthorityMaterializer for PausingLaunchAuthority {
    fn materialize<'a>(
        &'a self,
        record: &'a myelin_ci_controlplane::ci_run_store::CiRunRecord,
        prepared: &'a PreparedRunPlanV2,
        _definition: &'a CiWorkflowDefinitionPin,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<CiLaunchAuthorityV1, CiLaunchAuthorityError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(test_launch_authority(record, prepared))
        })
    }
}

fn starter(pool: &PgPool, tenant: &str, blobs: Arc<FsBlobStore>) -> PgCiPipelineStarter {
    starter_with(
        pool,
        tenant,
        blobs,
        CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
    )
}

fn starter_with(
    pool: &PgPool,
    tenant: &str,
    blobs: Arc<dyn BlobStore + Send + Sync>,
    definition: CiWorkflowDefinitionPin,
) -> PgCiPipelineStarter {
    PgCiPipelineStarter::new_with_authority(
        pool.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(MonotonicMinter::new()),
        TenantId(tenant.into()),
        Region("fr-par".into()),
        blobs,
        definition,
        Arc::new(TestLaunchAuthority),
    )
    .expect("valid exact-cell starter")
}

fn starter_without_authority(
    pool: &PgPool,
    tenant: &str,
    blobs: Arc<dyn BlobStore + Send + Sync>,
) -> PgCiPipelineStarter {
    starter_without_authority_with(
        pool,
        tenant,
        blobs,
        CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
    )
}

fn starter_without_authority_with(
    pool: &PgPool,
    tenant: &str,
    blobs: Arc<dyn BlobStore + Send + Sync>,
    definition: CiWorkflowDefinitionPin,
) -> PgCiPipelineStarter {
    PgCiPipelineStarter::new(
        pool.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(MonotonicMinter::new()),
        TenantId(tenant.into()),
        Region("fr-par".into()),
        blobs,
        definition,
    )
    .expect("valid fail-closed production starter")
}

fn starter_with_operational_reservations(
    pool: &PgPool,
    tenant: &str,
    blobs: Arc<dyn BlobStore + Send + Sync>,
    ledger: DurableCostLedger,
) -> PgCiPipelineStarter {
    ci_run_starter_factory(
        pool.clone(),
        Region("fr-par".into()),
        blobs,
        tokio::runtime::Handle::current(),
        ledger,
    )
    .expect("valid production Tier-P starter factory")
    .starter_for(
        TenantId(tenant.into()),
        CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
    )
    .expect("valid exact-cell production starter")
}

// The production composition-root seam (`ci_run_starter_factory`) over the app-role pool + cell region
// + blob CAS — the exact router the service main composes behind the runner activation gate. It mints a
// per-tenant `PgCiPipelineStarter` for an explicit authoritative tenant, never enumerating tenants.
fn factory(pool: &PgPool, blobs: Arc<FsBlobStore>) -> PgCiRunStarterFactory {
    PgCiRunStarterFactory::new_with_authority(
        pool.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(MonotonicMinter::new()),
        Region("fr-par".into()),
        blobs,
        Arc::new(TestLaunchAuthority),
    )
}

fn flow_executor(pool: &PgPool, tenant: &str) -> PgFlowExecutor {
    PgFlowExecutor::new(
        pool.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(MonotonicMinter::new()),
        TenantId(tenant.into()),
        Region("fr-par".into()),
    )
}

async fn insert_run(
    admin: &PgPool,
    blobs: &FsBlobStore,
    tenant: &str,
    run_id: &str,
    wf_run_id: &str,
) {
    let bytes = plan().canonical_bytes().expect("canonical plan");
    let hash = blobs
        .put(&TenantId(tenant.into()), &bytes)
        .expect("put immutable plan");
    let snapshot = format!(
        "myelin://{tenant}/ci/snapshot/{}",
        hash.to_multihash_string()
    );
    sqlx::query(
        "INSERT INTO ci_run (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, \
         repo_ref, commit_oid, cause_event_id, cause_depth, caused_by, definition_snapshot, trigger_kind, trust_tier, state, correlation_id) \
         VALUES ($1, 'fr-par', $2::uuid, '22222222-2222-2222-2222-222222222222'::uuid, \
         '33333333-3333-3333-3333-333333333333'::uuid, $3::uuid, \
         'myelin://' || $1 || '/git/repo/core', $5, \
         'trigger-' || $2, 1, 'session:test', $4, \
         'push', 'trusted', 'queued', 'corr-' || $2)",
    )
    .bind(tenant)
    .bind(run_id)
    .bind(wf_run_id)
    .bind(snapshot)
    .bind(TEST_COMMIT_OID)
    .execute(admin)
    .await
    .expect("insert queued ci_run");
    reserve_test_attempts(admin, tenant, run_id).await;
}

#[allow(clippy::too_many_arguments)]
async fn insert_pr_run(
    admin: &PgPool,
    blobs: &FsBlobStore,
    tenant: &str,
    run_id: &str,
    wf_run_id: &str,
    group: &str,
    generation: Option<i64>,
    persist_plan: bool,
) {
    let snapshot = if persist_plan {
        let bytes = plan().canonical_bytes().expect("canonical plan");
        let hash = blobs
            .put(&TenantId(tenant.into()), &bytes)
            .expect("put immutable PR plan");
        format!(
            "myelin://{tenant}/ci/snapshot/{}",
            hash.to_multihash_string()
        )
    } else {
        format!("myelin://{tenant}/ci/snapshot/blake3:{}", "f".repeat(64))
    };
    sqlx::query(
        "INSERT INTO ci_run (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, \
         repo_ref, commit_oid, cause_event_id, cause_depth, caused_by, definition_snapshot, \
         trigger_kind, concurrency_group, pr_head_generation, trust_tier, state, correlation_id) \
         VALUES ($1, 'fr-par', $2::uuid, '22222222-2222-2222-2222-222222222222'::uuid, \
         '33333333-3333-3333-3333-333333333333'::uuid, $3::uuid, \
         'myelin://' || $1 || '/git/repo/core', $7, \
         'trigger-' || $2, 1, \
         'session:test', $4, 'pull_request', $5, $6, 'trusted', 'queued', 'corr-' || $2)",
    )
    .bind(tenant)
    .bind(run_id)
    .bind(wf_run_id)
    .bind(snapshot)
    .bind(group)
    .bind(generation)
    .bind(TEST_COMMIT_OID)
    .execute(admin)
    .await
    .expect("insert queued PR ci_run");
    reserve_test_attempts(admin, tenant, run_id).await;
}

async fn reserve_test_attempts(admin: &PgPool, tenant: &str, run_id: &str) {
    let run_id = sqlx::types::Uuid::parse_str(run_id).expect("test run UUID");
    let repo_ref = format!("myelin://{tenant}/git/repo/core");
    for context in ["build", "package", "test"] {
        let attempt: i32 = sqlx::query_scalar(BUMP_CHECK_ATTEMPT_SQL)
            .bind(tenant)
            .bind("fr-par")
            .bind(&repo_ref)
            .bind(TEST_COMMIT_OID)
            .bind(context)
            .bind(run_id)
            .fetch_one(admin)
            .await
            .expect("allocate test check attempt");
        sqlx::query(
            "INSERT INTO ci_run_check_attempt \
             (tenant_id,region,run_id,repo_ref,commit_oid,context,run_attempt) \
             VALUES ($1,'fr-par',$2,$3,$6,$4,$5)",
        )
        .bind(tenant)
        .bind(run_id)
        .bind(&repo_ref)
        .bind(context)
        .bind(attempt)
        .bind(TEST_COMMIT_OID)
        .execute(admin)
        .await
        .expect("persist test run-scoped check attempt");
    }
}

struct SeededLaunch {
    claim: CiJobLaunchClaim,
    idem_token: String,
    reserve_handle: String,
}

fn manifest_dispatch_for_test(
    tenant: &str,
    ci_run_id: &str,
    wf_run_id: &str,
    job: &GrantedCiJobV1,
    suffix: &str,
) -> (DurableEnqueue, DurableCiJobLaunchTemplate) {
    let idem_token = format!("{wf_run_id}:fenced:{suffix}");
    let template = DurableCiJobLaunchTemplate {
        spec: JobSpecTemplate {
            kind: JobKind::Ci,
            image: ImageRef::pinned(job.image.clone()).unwrap(),
            command: job.command.clone(),
            env: job
                .env
                .iter()
                .map(|(name, value)| EnvVar {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
            secret_refs: job
                .secret_handles
                .iter()
                .map(|(name, handle)| SecretRef {
                    name: name.clone(),
                    handle: handle.clone(),
                })
                .collect(),
            egress: EgressPolicy {
                allow: job.egress_allow.clone(),
            },
            limits: ResourceLimits {
                cpu_millis: job.limits.cpu_millis,
                mem_bytes: job.limits.mem_bytes,
                disk_bytes: job.limits.disk_bytes,
                tmpfs_bytes: job.limits.disk_bytes,
                pids_max: job.limits.pids_max,
                timeout_secs: job.limits.timeout_secs,
            },
            workspace: WorkspaceSpec {
                repo_ref: Some(job.workspace.repo_ref.clone()),
                commit: Some(job.workspace.commit_oid.clone()),
            },
            trust_tier: TrustTier::Trusted,
            meter_to: MeterTarget {
                reserve_id: job.reserve_handle.clone(),
            },
            idem_token: IdemToken(idem_token.clone()),
        },
        ci_run_id: ci_run_id.into(),
        token_authority_handle: job.token_authority_handle.clone(),
    };
    (
        DurableEnqueue {
            tenant_id: tenant.into(),
            region: "fr-par".into(),
            job_id: job.job_id.clone(),
            run_id: wf_run_id.into(),
            lane: match job.scheduling.lane {
                CiManifestLaneV1::Interactive => Lane::Interactive,
                CiManifestLaneV1::Batch => Lane::Batch,
                CiManifestLaneV1::Deploy => Lane::Deploy,
            },
            labels: job.scheduling.labels.clone(),
            trust_tier: TrustTier::Trusted,
            concurrency_group: job.scheduling.concurrency_group.clone(),
            fair_key: job.scheduling.fair_key.clone(),
            idem_token,
            stage: job.name.clone(),
        },
        template,
    )
}

async fn seed_claimed_manifest_job(
    admin: &PgPool,
    tenant: &str,
    ci_run_id: &str,
    wf_run_id: &str,
    queue_state: &str,
) -> SeededLaunch {
    assert!(matches!(queue_state, "leased" | "running"));
    let tenant_id = TenantId(tenant.into());
    let region = Region("fr-par".into());
    let store =
        CiDriveManifestStore::new(admin.clone(), tenant_id.clone(), region.clone()).unwrap();
    let (manifest, _) = store
        .load_by_identity(wf_run_id, ci_run_id)
        .await
        .unwrap()
        .expect("started run has immutable manifest");
    let job = manifest.jobs.first().expect("test plan has jobs");
    let idem_token = format!("{wf_run_id}:race:{}", job.stage);
    let template = DurableCiJobLaunchTemplate {
        spec: JobSpecTemplate {
            kind: JobKind::Ci,
            image: ImageRef::pinned(job.image.clone()).unwrap(),
            command: job.command.clone(),
            env: job
                .env
                .iter()
                .map(|(name, value)| EnvVar {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
            secret_refs: job
                .secret_handles
                .iter()
                .map(|(name, handle)| SecretRef {
                    name: name.clone(),
                    handle: handle.clone(),
                })
                .collect(),
            egress: EgressPolicy {
                allow: job.egress_allow.clone(),
            },
            limits: ResourceLimits {
                cpu_millis: job.limits.cpu_millis,
                mem_bytes: job.limits.mem_bytes,
                disk_bytes: job.limits.disk_bytes,
                tmpfs_bytes: job.limits.disk_bytes,
                pids_max: job.limits.pids_max,
                timeout_secs: job.limits.timeout_secs,
            },
            workspace: WorkspaceSpec {
                repo_ref: Some(job.workspace.repo_ref.clone()),
                commit: Some(job.workspace.commit_oid.clone()),
            },
            trust_tier: TrustTier::Trusted,
            meter_to: MeterTarget {
                reserve_id: job.reserve_handle.clone(),
            },
            idem_token: IdemToken(idem_token.clone()),
        },
        ci_run_id: ci_run_id.into(),
        token_authority_handle: job.token_authority_handle.clone(),
    };
    sqlx::query(
        "INSERT INTO job_queue \
           (tenant_id, region, job_id, run_id, lane, labels, trust_tier, concurrency_group, \
            fair_key, idem_token, stage, state, lease_owner, lease_expires, lease_epoch, \
            claim_nonce, claim_started_at, claim_expires_at) \
         VALUES ($1, 'fr-par', $2::uuid, $3::uuid, 'interactive', ARRAY['linux'], 'trusted', \
                 $4, $1, $5, $6, $7, 'runner-race', statement_timestamp() + interval '10 minutes', \
                 1, $2::uuid, statement_timestamp(), statement_timestamp() + interval '5 minutes')",
    )
    .bind(tenant)
    .bind(&job.job_id)
    .bind(wf_run_id)
    .bind(&job.scheduling.concurrency_group)
    .bind(&idem_token)
    .bind(&job.stage)
    .bind(queue_state)
    .execute(admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ci_job_spec \
           (tenant_id, region, job_id, run_id, idem_token, spec, stage) \
         VALUES ($1, 'fr-par', $2::uuid, $3::uuid, $4, $5, $6)",
    )
    .bind(tenant)
    .bind(&job.job_id)
    .bind(wf_run_id)
    .bind(&idem_token)
    .bind(serde_json::to_value(template).unwrap())
    .bind(&job.stage)
    .execute(admin)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE cost_reservation SET state='inflight' \
         WHERE tenant_id=$1 AND region='fr-par' AND run_id=$2",
    )
    .bind(tenant)
    .bind(&job.reserve_handle)
    .execute(admin)
    .await
    .unwrap();
    let (started, expires): (i64, i64) = sqlx::query_as(
        "SELECT extract(epoch FROM claim_started_at)::bigint, \
                extract(epoch FROM claim_expires_at)::bigint \
         FROM job_queue WHERE tenant_id=$1 AND job_id=$2::uuid",
    )
    .bind(tenant)
    .bind(&job.job_id)
    .fetch_one(admin)
    .await
    .unwrap();
    SeededLaunch {
        claim: CiJobLaunchClaim {
            tenant_id: tenant.into(),
            region: "fr-par".into(),
            wf_run_id: wf_run_id.into(),
            job_id: job.job_id.clone(),
            lease_owner: "runner-race".into(),
            lease_epoch: 1,
            claim_nonce: job.job_id.clone(),
            claim_started_at_epoch_secs: started,
            claim_expires_at_epoch_secs: expires,
        },
        idem_token,
        reserve_handle: job.reserve_handle.clone(),
    }
}

async fn assert_atomic_started(
    admin: &PgPool,
    tenant: &str,
    run_id: &str,
    running: bool,
    workflow_exists: bool,
) {
    let pair: (bool, bool, i64) = sqlx::query_as(
        "SELECT state = 'running', EXISTS (SELECT 1 FROM workflow_run w \
         WHERE w.tenant_id = c.tenant_id AND w.region = c.region AND w.run_id = c.wf_run_id::text), \
         (SELECT count(*) FROM ci_job j WHERE j.tenant_id = c.tenant_id AND j.run_id = c.run_id) \
         FROM ci_run c WHERE tenant_id = $1 AND run_id = $2::uuid",
    )
    .bind(tenant)
    .bind(run_id)
    .fetch_one(admin)
    .await
    .expect("read atomic run pair");
    assert_eq!(
        pair,
        (running, workflow_exists, if running { 3 } else { 0 })
    );
}

async fn assert_manifest_backed_queued(admin: &PgPool, tenant: &str, run_id: &str) {
    let pair: (bool, bool, bool, i64) = sqlx::query_as(
        "SELECT state = 'queued',
                EXISTS (SELECT 1 FROM workflow_run w WHERE w.tenant_id=c.tenant_id
                        AND w.region=c.region AND w.run_id=c.wf_run_id::text),
                EXISTS (SELECT 1 FROM ci_drive_manifest m WHERE m.tenant_id=c.tenant_id
                        AND m.region=c.region AND m.ci_run_id=c.run_id),
                (SELECT count(*) FROM ci_job j WHERE j.tenant_id=c.tenant_id AND j.run_id=c.run_id)
           FROM ci_run c WHERE tenant_id=$1 AND run_id=$2::uuid",
    )
    .bind(tenant)
    .bind(run_id)
    .fetch_one(admin)
    .await
    .unwrap();
    assert_eq!(pair, (true, true, true, 3));
}

async fn attempt_rows(
    admin: &PgPool,
    tenant: &str,
) -> Vec<(String, i32, Option<sqlx::types::Uuid>)> {
    sqlx::query_as(
        "SELECT context, next_attempt, current_run FROM check_attempt \
         WHERE tenant_id=$1 ORDER BY context",
    )
    .bind(tenant)
    .fetch_all(admin)
    .await
    .expect("read exact check-attempt ledger")
}

async fn initial_check_envelopes(
    admin: &PgPool,
    tenant: &str,
    run_id: &str,
) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT envelope FROM outbox \
         WHERE envelope->>'type_' = 'ci.check.updated' \
           AND envelope->>'tenant' = $1 \
           AND envelope->'payload'->>'run' = $2 \
           AND envelope->'payload'->>'state' = 'in_progress' \
         ORDER BY envelope->'payload'->'context'->>'name'",
    )
    .bind(tenant)
    .bind(ci_run_ref(tenant, run_id).0)
    .fetch_all(admin)
    .await
    .expect("read initial check outbox facts")
}

async fn assert_initial_checks(admin: &PgPool, tenant: &str, run_id: &str, attempt: u32) {
    let started_at: String = sqlx::query_scalar(
        "SELECT to_char(created_at AT TIME ZONE 'UTC', \
                        'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') \
         FROM ci_run WHERE tenant_id=$1 AND run_id=$2::uuid",
    )
    .bind(tenant)
    .bind(run_id)
    .fetch_one(admin)
    .await
    .expect("read immutable CI start timestamp");
    let run_ref = ci_run_ref(tenant, run_id).0;
    let rows = initial_check_envelopes(admin, tenant, run_id).await;
    assert_eq!(rows.len(), 3, "one initial fact per authored context");
    let expected_contexts = ["build", "package", "test"];
    for (envelope, context) in rows.iter().zip(expected_contexts) {
        let payload = &envelope["payload"];
        assert_eq!(envelope["tenant"], tenant);
        assert_eq!(envelope["region"], "fr-par");
        assert_eq!(envelope["causation_id"], format!("trigger-{run_id}"));
        assert_eq!(envelope["correlation_id"], format!("corr-{run_id}"));
        assert_eq!(envelope["caused_by"], "session:test");
        assert_eq!(envelope["depth"], 2);
        let occurred_at = envelope["occurred_at"]
            .as_str()
            .expect("check occurrence timestamp is a string");
        let recorded_at = envelope["recorded_at"]
            .as_str()
            .expect("check recording timestamp is a string");
        assert!(occurred_at.contains('T') && occurred_at.ends_with('Z'));
        assert_eq!(recorded_at, occurred_at);
        assert!(
            occurred_at >= started_at.as_str(),
            "the actual emission clock cannot predate the immutable run start"
        );
        assert_eq!(payload["tenant"], tenant);
        assert_eq!(payload["repo"], format!("myelin://{tenant}/git/repo/core"));
        assert_eq!(payload["commit_oid"], TEST_COMMIT_OID);
        assert_eq!(
            payload["context"],
            serde_json::json!({
                "provider": "ci",
                "name": context,
            })
        );
        assert_eq!(payload["state"], "in_progress");
        assert_eq!(payload["required"], true);
        assert_eq!(payload["run"], run_ref);
        assert_eq!(payload["run_attempt"], attempt);
        assert_eq!(payload["trust_tier"], "trusted");
        assert_eq!(payload["details_ref"], run_ref);
        assert_eq!(payload["started_at"], started_at);
        assert!(payload["completed_at"].is_null());
        assert_eq!(payload["cost_settled"], false);
        assert!(envelope["event_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("ci-check-start-")));
    }
}

async fn cancelled_check_envelopes(
    admin: &PgPool,
    tenant: &str,
    run_id: &str,
) -> Vec<serde_json::Value> {
    let run_ref = ci_run_ref(tenant, run_id).0;
    sqlx::query_scalar(
        "SELECT envelope FROM outbox \
         WHERE envelope->>'type_' = 'ci.check.updated' \
           AND envelope->>'tenant' = $1 \
           AND envelope->'payload'->>'run' = $2 \
           AND envelope->'payload'->>'state' = 'cancelled' \
         ORDER BY (envelope->'payload'->>'cost_settled')::boolean, \
                  envelope->'payload'->'context'->>'name'",
    )
    .bind(tenant)
    .bind(run_ref)
    .fetch_all(admin)
    .await
    .expect("read cancelled check outbox facts")
}

async fn assert_cancelled_facts(
    admin: &PgPool,
    tenant: &str,
    run_id: &str,
    expected_cost_postures: &[bool],
) {
    let run_ref = ci_run_ref(tenant, run_id).0;
    let terminal: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT envelope FROM outbox \
         WHERE envelope->>'type_' = 'ci.run.cancelled' \
           AND envelope->>'tenant' = $1 \
           AND envelope->'payload'->>'run' = $2",
    )
    .bind(tenant)
    .bind(&run_ref)
    .fetch_all(admin)
    .await
    .expect("read cancelled run fact");
    assert_eq!(terminal.len(), 1, "one terminal run-cancellation fact");
    assert_eq!(
        terminal[0]["payload"]["reason"],
        "superseded-by-newer-pr-head"
    );
    assert_eq!(terminal[0]["causation_id"], format!("trigger-{run_id}"));
    assert_eq!(terminal[0]["correlation_id"], format!("corr-{run_id}"));
    let checks = cancelled_check_envelopes(admin, tenant, run_id).await;
    let expected_attempts: BTreeMap<String, i64> = initial_check_envelopes(admin, tenant, run_id)
        .await
        .into_iter()
        .map(|envelope| {
            (
                envelope["payload"]["context"]["name"]
                    .as_str()
                    .expect("initial context name")
                    .to_owned(),
                envelope["payload"]["run_attempt"]
                    .as_i64()
                    .expect("initial run attempt"),
            )
        })
        .collect();
    assert_eq!(
        checks.len(),
        expected_cost_postures.len() * 3,
        "one cancellation fact per context and requested cost posture"
    );
    for expected_cost_settled in expected_cost_postures {
        let posture: Vec<&serde_json::Value> = checks
            .iter()
            .filter(|envelope| {
                envelope["payload"]["cost_settled"].as_bool() == Some(*expected_cost_settled)
            })
            .collect();
        assert_eq!(posture.len(), 3);
        for (envelope, context) in posture.iter().zip(["build", "package", "test"]) {
            let payload = &envelope["payload"];
            assert_eq!(payload["context"]["name"], context);
            assert_eq!(payload["state"], "cancelled");
            assert_eq!(
                payload["run_attempt"].as_i64(),
                expected_attempts.get(context).copied()
            );
            assert!(payload["completed_at"].is_string());
            assert_eq!(payload["summary"]["template_key"], "ci.check.cancelled");
            assert_eq!(envelope["causation_id"], format!("trigger-{run_id}"));
            assert!(envelope["event_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("ci-supersession-")));
        }
    }
}

type CiJobLedgerRow = (
    sqlx::types::Uuid,
    String,
    String,
    Vec<sqlx::types::Uuid>,
    Option<serde_json::Value>,
    String,
    String,
    i32,
    Option<serde_json::Value>,
);

async fn assert_exact_jobs(admin: &PgPool, tenant: &str, run_id: &str) {
    let manifest_digest: String = sqlx::query_scalar(
        "SELECT manifest_digest FROM ci_drive_manifest WHERE tenant_id=$1 AND ci_run_id=$2::uuid",
    )
    .bind(tenant)
    .bind(run_id)
    .fetch_one(admin)
    .await
    .unwrap();
    let run_uuid = sqlx::types::Uuid::parse_str(run_id).unwrap();
    let expected_plan = plan();
    let ids = expected_plan
        .jobs
        .iter()
        .map(|job| {
            (
                job.name.clone(),
                ci_job_id_v2(
                    &TenantId(tenant.into()),
                    run_uuid,
                    &job.stage,
                    &job.name,
                    &job.matrix_identity(),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let rows: Vec<CiJobLedgerRow> = sqlx::query_as(
        "SELECT job_id, stage, name, needs, matrix_key, spec_ref, state, attempt, result_summary \
         FROM ci_job WHERE tenant_id=$1 AND run_id=$2::uuid ORDER BY name",
    )
    .bind(tenant)
    .bind(run_id)
    .fetch_all(admin)
    .await
    .expect("read canonical ci_job ledger");
    assert_eq!(rows.len(), expected_plan.jobs.len());
    for (row, job) in rows.iter().zip(&expected_plan.jobs) {
        assert_eq!(row.0, ids[&job.name]);
        assert_eq!(row.1, job.stage, "V2 preserves authored stage identity");
        assert_eq!(row.2, job.name);
        assert_eq!(
            row.3,
            job.needs.iter().map(|name| ids[name]).collect::<Vec<_>>()
        );
        let expected_matrix =
            (!job.matrix_key.is_empty()).then(|| serde_json::to_value(&job.matrix_key).unwrap());
        assert_eq!(row.4, expected_matrix);
        assert_eq!(
            row.5,
            ci_artifact_ref(tenant, &format!("drive-manifest-{manifest_digest}")).0,
            "spec_ref binds the immutable drive manifest, not customer plan input"
        );
        assert_eq!(row.6, "queued");
        assert_eq!(row.7, 1);
        assert_eq!(row.8, None);
    }
}

async fn visible_job_count(app: &PgPool, tenant: &str, region: &str) -> i64 {
    let mut transaction = app.begin().await.expect("begin RLS probe");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), \
                set_config('myelin.region', $2, true)",
    )
    .bind(tenant)
    .bind(region)
    .execute(&mut *transaction)
    .await
    .expect("scope RLS probe");
    let count = sqlx::query_scalar("SELECT count(*) FROM ci_job")
        .fetch_one(&mut *transaction)
        .await
        .expect("count visible ci_job rows");
    transaction.rollback().await.expect("rollback RLS probe");
    count
}

async fn assert_run_ledger_index_is_used(admin: &PgPool, tenant: &str, run_id: &str) {
    let indexdef: String = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes \
         WHERE schemaname = current_schema() AND tablename = 'ci_job' AND indexname = $1",
    )
    .bind(CI_JOB_RUN_LEDGER_INDEX)
    .fetch_one(admin)
    .await
    .expect("ci_job run-ledger index exists");
    assert!(
        indexdef.contains("(tenant_id, region, run_id)"),
        "exact-cell ledger index has the frozen key order: {indexdef}"
    );

    // Disable only sequential scans inside this probe. PostgreSQL may choose either an Index Scan or
    // Bitmap Index Scan; both prove the index can serve the exact tenant+region+run predicate without
    // making a brittle cost/cardinality assertion.
    let mut transaction = admin.begin().await.expect("begin planner probe");
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *transaction)
        .await
        .expect("disable sequential scans for deterministic planner proof");
    let plan: Vec<String> = sqlx::query_scalar(
        "EXPLAIN (COSTS OFF) SELECT job_id FROM ci_job \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid FOR UPDATE",
    )
    .bind(tenant)
    .bind("fr-par")
    .bind(run_id)
    .fetch_all(&mut *transaction)
    .await
    .expect("explain exact ci_job ledger lookup");
    assert!(
        plan.iter()
            .any(|line| line.contains(CI_JOB_RUN_LEDGER_INDEX)),
        "planner uses {CI_JOB_RUN_LEDGER_INDEX} for tenant+region+run lookup: {plan:?}"
    );
    transaction
        .rollback()
        .await
        .expect("rollback planner probe");
}

async fn expected_input(admin: &PgPool, tenant: &str, run_id: &str) -> Vec<ArtifactRef> {
    let digest: String = sqlx::query_scalar(
        "SELECT manifest_digest FROM ci_drive_manifest WHERE tenant_id=$1 AND ci_run_id=$2::uuid",
    )
    .bind(tenant)
    .bind(run_id)
    .fetch_one(admin)
    .await
    .unwrap();
    vec![
        ci_artifact_ref(tenant, &format!("drive-manifest-{digest}")),
        ci_run_ref(tenant, run_id),
    ]
}

async fn seed_exact_workflow(
    app: &PgPool,
    admin: &PgPool,
    blobs: Arc<FsBlobStore>,
    tenant: &str,
    run_id: &str,
    wf_run_id: &str,
) {
    starter(app, tenant, blobs)
        .run_once()
        .await
        .expect("seed complete manifest-backed workflow through the real starter");
    let input = expected_input(admin, tenant, run_id).await;
    let actual: serde_json::Value =
        sqlx::query_scalar("SELECT input FROM workflow_run WHERE tenant_id=$1 AND run_id=$2")
            .bind(tenant)
            .bind(wf_run_id)
            .fetch_one(admin)
            .await
            .unwrap();
    assert_eq!(actual, serde_json::to_value(input).unwrap());
    sqlx::query("UPDATE ci_run SET state='queued' WHERE tenant_id=$1 AND run_id=$2::uuid")
        .bind(tenant)
        .bind(run_id)
        .execute(admin)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exact_cell_starter_is_atomic_concurrent_restart_safe_and_rls_isolated() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    let schema = schema_name();
    let bare = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .expect("connect schema setup");
    bare.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop stale schema");
    bare.execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let cleanup_bare = bare.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_bare, &schema_for_cleanup, move || async move {
    let admin = pool_on(&admin_url(), &schema).await;
    PgMigrator::apply(&admin, &foundation_migrations())
        .await
        .expect("foundation migrations");
    PgMigrator::apply(&admin, &reserve_settle_durable_migrations())
        .await
        .expect("durable reservation migrations");
    PgMigrator::apply_validated(
        &admin,
        &flow_migrations(),
        &HotTables::declare(["workflow_run"]),
    )
    .await
    .expect("flow migrations");
    admin.execute(CREATE_CI_RUN_DDL).await.expect("ci_run DDL");
    admin
        .execute(ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL)
        .await
        .expect("ci_run causal provenance migration");
    admin
        .execute(ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL)
        .await
        .expect("ci_run concurrency identity migration");
    admin
        .execute(ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL)
        .await
        .expect("ci_run PR ordering authority migration");
    sqlx::raw_sql(CREATE_CI_DRIVE_MANIFEST_DDL)
        .execute(&admin)
        .await
        .expect("ci_drive_manifest DDL");
    admin.execute(CREATE_CI_JOB_DDL).await.expect("ci_job DDL");
    admin
        .execute(CREATE_JOB_QUEUE_DDL)
        .await
        .expect("job_queue DDL");
    admin
        .execute("CREATE UNIQUE INDEX jq_idem ON job_queue (tenant_id, idem_token)")
        .await
        .expect("job_queue dispatch idempotency index");
    for ddl in [
        ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
        ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL,
        ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
        ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL,
    ] {
        admin
            .execute(ddl)
            .await
            .expect("job_queue launch-authority migration");
    }
    admin
        .execute(CREATE_CI_JOB_SPEC_DDL)
        .await
        .expect("ci_job_spec DDL");
    admin
        .execute(ALTER_CI_JOB_SPEC_ADD_STAGE_DDL)
        .await
        .expect("ci_job_spec stage migration");
    admin
        .execute(CREATE_CI_COST_EVENT_DDL)
        .await
        .expect("ci_cost_event DDL");
    sqlx::raw_sql(CREATE_CI_JOB_ACCOUNTING_DDL)
        .execute(&admin)
        .await
        .expect("ci_job_accounting DDL");
    admin
        .execute(ALTER_CI_JOB_ACCOUNTING_ADD_SKIPPED_DDL)
        .await
        .expect("ci_job_accounting skipped migration");
    admin
        .execute(CREATE_CHECK_ATTEMPT_DDL)
        .await
        .expect("check_attempt DDL");
    sqlx::raw_sql(CREATE_CI_RUN_CHECK_ATTEMPT_DDL)
        .execute(&admin)
        .await
        .expect("ci_run_check_attempt DDL");
    admin
        .execute(CREATE_CI_JOB_RUN_LEDGER_INDEX_DDL)
        .await
        .expect("ci_job run-ledger concurrent index DDL");
    admin
        .execute("SELECT myelin_make_tenant_scoped('ci_run')")
        .await
        .expect("force RLS on ci_run");
    admin
        .execute("SELECT myelin_make_tenant_scoped('ci_drive_manifest')")
        .await
        .expect("force RLS on ci_drive_manifest");
    admin
        .execute("SELECT myelin_make_tenant_scoped('ci_job')")
        .await
        .expect("force RLS on ci_job");
    admin
        .execute("SELECT myelin_make_tenant_scoped('check_attempt')")
        .await
        .expect("force RLS on check_attempt");
    admin
        .execute("SELECT myelin_make_tenant_scoped('ci_run_check_attempt')")
        .await
        .expect("force RLS on ci_run_check_attempt");
    for table in [
        "job_queue",
        "ci_job_spec",
        "ci_cost_event",
        "ci_job_accounting",
    ] {
        admin
            .execute(format!("SELECT myelin_make_tenant_scoped('{table}')").as_str())
            .await
            .expect("force RLS on supersession table");
    }
    admin
        .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant schema");
    admin
        .execute(format!("GRANT ALL ON ALL TABLES IN SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant tables");
    admin
        .execute("REVOKE UPDATE, DELETE ON ci_drive_manifest FROM myelin_app")
        .await
        .expect("manifest remains insert-only after broad test setup grant");
    let app = pool_on(&app_url(), &schema).await;
    let blobs = Arc::new(FsBlobStore::new());
    let mut ledger_config = MyelinConfig::dev();
    ledger_config.database_url = admin_url();
    ledger_config.region = "fr-par".into();
    let supersession_ledger =
        DurableCostLedger::new(SubstrateProvider::connect(ledger_config, 1).await.unwrap());

    let register = flow_executor(&admin, "tenant_a");
    tokio::task::block_in_place(|| {
        register
            .register_definition(CI_PIPELINE_WF_TYPE, 1, BODY_HASH)
            .expect("register immutable workflow definition");
    });

    // A shutdown arriving during one real starter transaction lets that in-flight start commit, then
    // prevents the next queued run in the same nominal 64-item pass from acquiring authority.
    let shutdown_run_a = "10000000-0000-0000-0000-0000000000c1";
    let shutdown_wf_a = "20000000-0000-0000-0000-0000000000c1";
    let shutdown_run_b = "10000000-0000-0000-0000-0000000000c2";
    let shutdown_wf_b = "20000000-0000-0000-0000-0000000000c2";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_shutdown_a",
        shutdown_run_a,
        shutdown_wf_a,
    )
    .await;
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_shutdown_b",
        shutdown_run_b,
        shutdown_wf_b,
    )
    .await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let shutdown_poller = PgCiRunStarterPoller::new(
        ci_region_run_discovery_test_support(admin.clone()),
        PgCiRunStarterFactory::new_with_authority(
            app.clone(),
            tokio::runtime::Handle::current(),
            Arc::new(MonotonicMinter::new()),
            Region("fr-par".into()),
            blobs.clone(),
            Arc::new(PausingLaunchAuthority {
                entered: entered.clone(),
                release: release.clone(),
            }),
        ),
        CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let shutdown_task = tokio::spawn(async move {
        shutdown_poller
            .run_until_shutdown(shutdown_rx, Duration::from_millis(1), 64)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("the first real starter entered launch authority");
    shutdown_tx.send(true).unwrap();
    release.notify_one();
    shutdown_task
        .await
        .unwrap()
        .expect("starter lane drains one in-flight transaction");
    let shutdown_states: Vec<String> = sqlx::query_scalar(
        "SELECT state FROM ci_run
         WHERE run_id IN ($1::uuid, $2::uuid)
         ORDER BY run_id",
    )
    .bind(shutdown_run_a)
    .bind(shutdown_run_b)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(
        shutdown_states,
        vec!["running".to_string(), "queued".to_string()],
        "shutdown must stop before the second concrete starter unit"
    );
    sqlx::query("DELETE FROM ci_run WHERE run_id = $1::uuid")
        .bind(shutdown_run_b)
        .execute(&admin)
        .await
        .expect("remove only the deliberately unstarted shutdown fixture");

    // The region-wide poller routes only the discovered authoritative tenant into the exact-cell
    // starter. Two pollers may discover the same row, but the starter's exact queued-row lock admits
    // one authority call and one atomic start; the loser returns Idle.
    let poller_run = "10000000-0000-0000-0000-0000000000e0";
    let poller_wf = "20000000-0000-0000-0000-0000000000e0";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_poller",
        poller_run,
        poller_wf,
    )
    .await;
    AUTHORITY_CALLS.store(0, Ordering::SeqCst);
    let poller = PgCiRunStarterPoller::new(
        ci_region_run_discovery_test_support(admin.clone()),
        factory(&app, blobs.clone()),
        CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
    );
    let (poll_a, poll_b) = tokio::join!(poller.run_once(), poller.run_once());
    let poll_outcomes = [poll_a.expect("poller A"), poll_b.expect("poller B")];
    assert_eq!(
        poll_outcomes
            .iter()
            .filter(|outcome| matches!(outcome, StartQueuedOutcome::Started { .. }))
            .count(),
        1
    );
    assert_eq!(
        poll_outcomes
            .iter()
            .filter(|outcome| matches!(outcome, StartQueuedOutcome::Idle))
            .count(),
        1
    );
    assert_atomic_started(&admin, "tenant_poller", poller_run, true, true).await;
    assert_eq!(
        AUTHORITY_CALLS.load(Ordering::SeqCst),
        1,
        "racing discovery passes cannot duplicate launch authority"
    );

    // Production composition has no implicit runtime policy. Reserve-time attempt authority already
    // exists; a fresh V2 run is refused before any manifest, job, workflow, or lifecycle mutation
    // when no launch-authority adapter is wired.
    let denied_run = "10000000-0000-0000-0000-0000000000d0";
    let denied_wf = "20000000-0000-0000-0000-0000000000d0";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_no_authority",
        denied_run,
        denied_wf,
    )
    .await;
    let denied = starter_without_authority(&app, "tenant_no_authority", blobs.clone())
        .run_once()
        .await
        .expect_err("fresh V2 launch without explicit authority must fail closed");
    assert!(denied
        .to_string()
        .contains("no policy-aware launch-authority"));
    assert_atomic_started(&admin, "tenant_no_authority", denied_run, false, false).await;
    let denied_side_effects: (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT count(*) FROM ci_drive_manifest WHERE tenant_id='tenant_no_authority'),
           (SELECT count(*) FROM check_attempt WHERE tenant_id='tenant_no_authority'),
           (SELECT count(*) FROM ci_run_check_attempt WHERE tenant_id='tenant_no_authority')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(denied_side_effects, (0, 3, 3));
    assert!(
        initial_check_envelopes(&admin, "tenant_no_authority", denied_run)
            .await
            .is_empty()
    );

    // The real Tier-P authority is dispatched through `materialize_in_tx`: reservations are written
    // before the manifest, but a later manifest failure rolls both back with the starter transaction.
    // Calling the authority's standalone path here would commit three orphan reservations and fail
    // this proof.
    let reservation_tenant = "tenant_atomic_reservation";
    let reservation_run = "10000000-0000-0000-0000-0000000000a7";
    let reservation_wf = "20000000-0000-0000-0000-0000000000a7";
    insert_run(
        &admin,
        blobs.as_ref(),
        reservation_tenant,
        reservation_run,
        reservation_wf,
    )
    .await;
    admin
        .execute(
            "CREATE FUNCTION fail_atomic_reservation_manifest() RETURNS trigger \
             LANGUAGE plpgsql AS $$ BEGIN \
               IF NEW.tenant_id = 'tenant_atomic_reservation' \
               THEN RAISE EXCEPTION 'injected post-reservation manifest failure'; END IF; \
               RETURN NEW; END $$",
        )
        .await
        .unwrap();
    admin
        .execute(
            "CREATE TRIGGER fail_atomic_reservation_manifest \
             BEFORE INSERT ON ci_drive_manifest FOR EACH ROW \
             EXECUTE FUNCTION fail_atomic_reservation_manifest()",
        )
        .await
        .unwrap();
    let atomic_starter = starter_with_operational_reservations(
        &app,
        reservation_tenant,
        blobs.clone(),
        supersession_ledger.clone(),
    );
    let atomic_error = atomic_starter
        .run_once()
        .await
        .expect_err("post-reservation manifest failure must abort the complete start");
    assert!(atomic_error
        .to_string()
        .contains("injected post-reservation manifest failure"));
    assert_atomic_started(&admin, reservation_tenant, reservation_run, false, false).await;
    let atomic_side_effects: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT count(*) FROM cost_reservation WHERE tenant_id=$1),
           (SELECT count(*) FROM ci_drive_manifest WHERE tenant_id=$1),
           (SELECT count(*) FROM check_attempt WHERE tenant_id=$1),
           (SELECT count(*) FROM ci_run_check_attempt WHERE tenant_id=$1)",
    )
    .bind(reservation_tenant)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        atomic_side_effects,
        (0, 0, 3, 3),
        "failed start preserves reserve-time attempts but commits no capacity reservation or manifest"
    );
    admin
        .execute("DROP TRIGGER fail_atomic_reservation_manifest ON ci_drive_manifest")
        .await
        .unwrap();
    admin
        .execute("DROP FUNCTION fail_atomic_reservation_manifest()")
        .await
        .unwrap();
    assert!(matches!(
        atomic_starter
            .run_once()
            .await
            .expect("retry after the injected crash starts normally"),
        StartQueuedOutcome::Started { .. }
    ));
    let committed_reservations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cost_reservation WHERE tenant_id=$1 AND state='reserved'",
    )
    .bind(reservation_tenant)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(committed_reservations, 3);

    // Producer generations, not arrival time, own PR supersession. Starting generation 2
    // co-commits its own workflow/reservations with cancellation of the already-running generation
    // 1 run. Because no old job crossed launch, all three reservations settle at zero and the
    // cancelled run is immediately cost-closed.
    let pr_tenant = "tenant_pr_supersession";
    let pr_group = "pr:core:42";
    let old_run = "10000000-0000-0000-0000-000000000141";
    let old_wf = "20000000-0000-0000-0000-000000000141";
    let new_run = "10000000-0000-0000-0000-000000000142";
    let new_wf = "20000000-0000-0000-0000-000000000142";
    let pr_factory = ci_run_starter_factory(
        app.clone(),
        Region("fr-par".into()),
        blobs.clone(),
        tokio::runtime::Handle::current(),
        supersession_ledger.clone(),
    )
    .unwrap();
    let pr_starter = pr_factory
        .starter_for(
            TenantId(pr_tenant.into()),
            CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
        )
        .unwrap();
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        old_run,
        old_wf,
        pr_group,
        Some(1),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { run_id, .. } if run_id == old_run
    ));
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        new_run,
        new_wf,
        pr_group,
        Some(2),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { run_id, .. } if run_id == new_run
    ));
    let old_lifecycle: (String, bool, String, Option<String>) = sqlx::query_as(
        "SELECT r.state, r.cost_settled, w.state, w.cancel_reason \
         FROM ci_run r JOIN workflow_run w \
           ON w.tenant_id=r.tenant_id AND w.region=r.region AND w.run_id=r.wf_run_id::text \
         WHERE r.tenant_id=$1 AND r.run_id=$2::uuid",
    )
    .bind(pr_tenant)
    .bind(old_run)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        old_lifecycle,
        (
            "cancelled".into(),
            true,
            "terminated".into(),
            Some("superseded-by-newer-pr-head".into())
        )
    );
    let old_accounting: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
           (SELECT count(*) FROM ci_job_accounting \
             WHERE tenant_id=$1 AND ci_run_id=$2::uuid AND skipped), \
           (SELECT count(*) FROM ci_cost_event \
             WHERE tenant_id=$1 AND run_id=$2::uuid), \
           (SELECT count(*) FROM cost_reservation \
             WHERE tenant_id=$1 AND run_id LIKE ('ci-reserve:v1:' || $2 || ':%') \
               AND state='settled'), \
           (SELECT count(*) FROM workflow_run \
             WHERE tenant_id=$1 AND run_id=$3 AND state IN ('running','waiting'))",
    )
    .bind(pr_tenant)
    .bind(old_run)
    .bind(new_wf)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(old_accounting, (3, 6, 3, 1));
    assert_cancelled_facts(&admin, pr_tenant, old_run, &[true]).await;

    // A delayed generation-1 event is consumed without consulting its missing CAS plan. The
    // already-running generation 2 remains untouched, proving arrival order and timestamps do not
    // become accidental authority.
    let delayed_run = "10000000-0000-0000-0000-000000000140";
    let delayed_wf = "20000000-0000-0000-0000-000000000140";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        delayed_run,
        delayed_wf,
        pr_group,
        Some(1),
        false,
    )
    .await;
    assert_eq!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Superseded {
            run_id: delayed_run.into()
        }
    );
    let delayed_shape: (String, bool, i64, i64) = sqlx::query_as(
        "SELECT state, cost_settled, \
           (SELECT count(*) FROM workflow_run WHERE run_id=$2), \
           (SELECT count(*) FROM ci_drive_manifest WHERE ci_run_id=$1::uuid) \
         FROM ci_run WHERE tenant_id=$3 AND run_id=$1::uuid",
    )
    .bind(delayed_run)
    .bind(delayed_wf)
    .bind(pr_tenant)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(delayed_shape, ("cancelled".into(), true, 0, 0));
    assert_cancelled_facts(&admin, pr_tenant, delayed_run, &[]).await;
    let current_state: String =
        sqlx::query_scalar("SELECT state FROM ci_run WHERE tenant_id=$1 AND run_id=$2::uuid")
            .bind(pr_tenant)
            .bind(new_run)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(current_state, "running");

    // Terminal rows remain the durable generation high-water mark. A late lower positive row and
    // a rolling-upgrade NULL row are stale even after the newest generation has left the active
    // set; completion never erases producer ordering authority.
    let terminal_watermark_group = "pr:core:48";
    let terminal_watermark_run = "10000000-0000-0000-0000-000000000201";
    let terminal_watermark_wf = "20000000-0000-0000-0000-000000000201";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        terminal_watermark_run,
        terminal_watermark_wf,
        terminal_watermark_group,
        Some(9),
        false,
    )
    .await;
    sqlx::query(
        "UPDATE ci_run SET state='succeeded', cost_settled=true, \
           finished_at=clock_timestamp() \
         WHERE tenant_id=$1 AND run_id=$2::uuid",
    )
    .bind(pr_tenant)
    .bind(terminal_watermark_run)
    .execute(&admin)
    .await
    .unwrap();
    for (run, wf, generation) in [
        (
            "10000000-0000-0000-0000-000000000202",
            "20000000-0000-0000-0000-000000000202",
            Some(8),
        ),
        (
            "10000000-0000-0000-0000-000000000203",
            "20000000-0000-0000-0000-000000000203",
            None,
        ),
    ] {
        insert_pr_run(
            &admin,
            blobs.as_ref(),
            pr_tenant,
            run,
            wf,
            terminal_watermark_group,
            generation,
            false,
        )
        .await;
        assert_eq!(
            pr_starter.run_once().await.unwrap(),
            StartQueuedOutcome::Superseded { run_id: run.into() },
            "retained terminal generation is permanent high-water authority"
        );
    }

    // Rolling-upgrade NULL generations are legacy-oldest only relative to a positive generation.
    // Two legacy rows do not invent an order and therefore both start; a positive successor then
    // cancels both.
    let legacy_group = "pr:core:43";
    let legacy_a = "10000000-0000-0000-0000-000000000151";
    let legacy_a_wf = "20000000-0000-0000-0000-000000000151";
    let legacy_b = "10000000-0000-0000-0000-000000000152";
    let legacy_b_wf = "20000000-0000-0000-0000-000000000152";
    for (run, wf) in [(legacy_a, legacy_a_wf), (legacy_b, legacy_b_wf)] {
        insert_pr_run(
            &admin,
            blobs.as_ref(),
            pr_tenant,
            run,
            wf,
            legacy_group,
            None,
            true,
        )
        .await;
        assert!(matches!(
            pr_starter.run_once().await.unwrap(),
            StartQueuedOutcome::Started { run_id, .. } if run_id == run
        ));
    }
    let legacy_running: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ci_run WHERE tenant_id=$1 AND concurrency_group=$2 \
           AND state='running' AND pr_head_generation IS NULL",
    )
    .bind(pr_tenant)
    .bind(legacy_group)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(legacy_running, 2);
    let positive_run = "10000000-0000-0000-0000-000000000153";
    let positive_wf = "20000000-0000-0000-0000-000000000153";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        positive_run,
        positive_wf,
        legacy_group,
        Some(2),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { run_id, .. } if run_id == positive_run
    ));
    let legacy_cancelled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ci_run WHERE tenant_id=$1 AND concurrency_group=$2 \
           AND state='cancelled' AND cost_settled",
    )
    .bind(pr_tenant)
    .bind(legacy_group)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(legacy_cancelled, 2);

    // Cancellation winning the final-launch race terminalizes the exact leased generation and
    // zero-settles even an already-inflight reservation. The later launch CAS is refused.
    let cancel_wins_group = "pr:core:44";
    let cancel_wins_old = "10000000-0000-0000-0000-000000000161";
    let cancel_wins_old_wf = "20000000-0000-0000-0000-000000000161";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        cancel_wins_old,
        cancel_wins_old_wf,
        cancel_wins_group,
        Some(1),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    let cancelled_claim = seed_claimed_manifest_job(
        &admin,
        pr_tenant,
        cancel_wins_old,
        cancel_wins_old_wf,
        "leased",
    )
    .await;
    let cancel_wins_new = "10000000-0000-0000-0000-000000000162";
    let cancel_wins_new_wf = "20000000-0000-0000-0000-000000000162";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        cancel_wins_new,
        cancel_wins_new_wf,
        cancel_wins_group,
        Some(2),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    assert!(
        !CiJobQueueStore::with_pg(app.clone())
            .authorize_launch(&cancelled_claim.claim)
            .await
            .unwrap(),
        "cancellation and final launch serialize on the same queue row"
    );
    let cancelled_race_shape: (String, bool, String, i64) = sqlx::query_as(
        "SELECT r.state, r.cost_settled, q.state, \
           (SELECT count(*) FROM ci_job_accounting a \
             WHERE a.tenant_id=r.tenant_id AND a.ci_run_id=r.run_id) \
         FROM ci_run r JOIN job_queue q \
           ON q.tenant_id=r.tenant_id AND q.run_id=r.wf_run_id \
         WHERE r.tenant_id=$1 AND r.run_id=$2::uuid",
    )
    .bind(pr_tenant)
    .bind(cancel_wins_old)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        cancelled_race_shape,
        ("cancelled".into(), true, "terminal".into(), 3)
    );
    assert_cancelled_facts(&admin, pr_tenant, cancel_wins_old, &[true]).await;

    // Production manifest dispatch and supersession share the exact Flow→queue fence. A dispatch
    // that commits first is observed and terminalized; after cancellation commits, a stale body
    // cannot insert another manifest row behind the queue snapshot.
    let late_dispatch_group = "pr:core:49";
    let late_dispatch_old = "10000000-0000-0000-0000-000000000211";
    let late_dispatch_old_wf = "20000000-0000-0000-0000-000000000211";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        late_dispatch_old,
        late_dispatch_old_wf,
        late_dispatch_group,
        Some(1),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    let manifest_store = CiDriveManifestStore::new(
        admin.clone(),
        TenantId(pr_tenant.into()),
        Region("fr-par".into()),
    )
    .unwrap();
    let (late_manifest, _) = manifest_store
        .load_by_identity(late_dispatch_old_wf, late_dispatch_old)
        .await
        .unwrap()
        .unwrap();
    let dispatched_job = late_manifest.jobs.first().unwrap();
    let stale_job = late_manifest.jobs.get(1).unwrap();
    let dispatch_store = CiJobSpecStore::with_pg(app.clone());
    let (dispatch_enqueue, dispatch_template) = manifest_dispatch_for_test(
        pr_tenant,
        late_dispatch_old,
        late_dispatch_old_wf,
        dispatched_job,
        "wins",
    );
    dispatch_store
        .co_persist_active_flow_dispatch(
            &dispatch_enqueue,
            &dispatch_template,
            &dispatched_job.name,
        )
        .await
        .expect("active Flow permits the production manifest dispatch");
    let late_dispatch_new = "10000000-0000-0000-0000-000000000212";
    let late_dispatch_new_wf = "20000000-0000-0000-0000-000000000212";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        late_dispatch_new,
        late_dispatch_new_wf,
        late_dispatch_group,
        Some(2),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { run_id, .. } if run_id == late_dispatch_new
    ));
    let late_dispatch_shape: (String, String, bool, i64) = sqlx::query_as(
        "SELECT r.state, q.state, r.cost_settled, \
           (SELECT count(*) FROM ci_job_accounting a \
             WHERE a.tenant_id=r.tenant_id AND a.ci_run_id=r.run_id AND a.skipped) \
         FROM ci_run r JOIN job_queue q \
           ON q.tenant_id=r.tenant_id AND q.run_id=r.wf_run_id \
         WHERE r.tenant_id=$1 AND r.run_id=$2::uuid AND q.job_id=$3::uuid",
    )
    .bind(pr_tenant)
    .bind(late_dispatch_old)
    .bind(&dispatched_job.job_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        late_dispatch_shape,
        ("cancelled".into(), "terminal".into(), true, 3)
    );
    let (stale_enqueue, stale_template) = manifest_dispatch_for_test(
        pr_tenant,
        late_dispatch_old,
        late_dispatch_old_wf,
        stale_job,
        "loses",
    );
    assert!(dispatch_store
        .co_persist_active_flow_dispatch(&stale_enqueue, &stale_template, &stale_job.name)
        .await
        .expect_err("terminated Flow must fence stale production dispatch")
        .to_string()
        .contains("owning Flow run is not active"));
    let stale_rows: (i64, i64) = sqlx::query_as(
        "SELECT \
           (SELECT count(*) FROM job_queue WHERE tenant_id=$1 AND job_id=$2::uuid), \
           (SELECT count(*) FROM ci_job_spec WHERE tenant_id=$1 AND job_id=$2::uuid)",
    )
    .bind(pr_tenant)
    .bind(&stale_job.job_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        stale_rows,
        (0, 0),
        "stale body writes neither queue nor executable spec"
    );

    // Recreate the dangerous finalizer-winning interleaving: hold the old ci_run row, let
    // supersession terminate/lock Flow and reach the product CAS, then publish a terminal product
    // result first. The canceller must roll back its Flow termination and the replacement start,
    // never accept a terminal product row paired with a newly terminated workflow.
    let finalizer_group = "pr:core:50";
    let finalizer_old = "10000000-0000-0000-0000-000000000221";
    let finalizer_old_wf = "20000000-0000-0000-0000-000000000221";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        finalizer_old,
        finalizer_old_wf,
        finalizer_group,
        Some(1),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    let mut finalizer_tx = admin.begin().await.unwrap();
    sqlx::query(
        "SELECT state FROM ci_run \
         WHERE tenant_id=$1 AND run_id=$2::uuid FOR UPDATE",
    )
    .bind(pr_tenant)
    .bind(finalizer_old)
    .fetch_one(&mut *finalizer_tx)
    .await
    .unwrap();
    let finalizer_new = "10000000-0000-0000-0000-000000000222";
    let finalizer_new_wf = "20000000-0000-0000-0000-000000000222";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        finalizer_new,
        finalizer_new_wf,
        finalizer_group,
        Some(2),
        true,
    )
    .await;
    let racing_starter = pr_starter.clone();
    let racing_supersession = tokio::spawn(async move { racing_starter.run_once().await });
    let mut flow_locked = false;
    for _ in 0..100 {
        let probe = sqlx::query(
            "SELECT state FROM workflow_run \
             WHERE tenant_id=$1 AND region='fr-par' AND run_id=$2 FOR UPDATE NOWAIT",
        )
        .bind(pr_tenant)
        .bind(finalizer_old_wf)
        .fetch_one(&admin)
        .await;
        if probe
            .as_ref()
            .err()
            .and_then(sqlx::Error::as_database_error)
            .and_then(|error| error.code())
            .as_deref()
            == Some("55P03")
        {
            flow_locked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        flow_locked,
        "supersession must own the Flow row before reaching the blocked ci_run CAS"
    );
    sqlx::query(
        "UPDATE ci_run SET state='succeeded', cost_settled=true, \
           finished_at=clock_timestamp() \
         WHERE tenant_id=$1 AND run_id=$2::uuid",
    )
    .bind(pr_tenant)
    .bind(finalizer_old)
    .execute(&mut *finalizer_tx)
    .await
    .unwrap();
    finalizer_tx.commit().await.unwrap();
    assert!(racing_supersession
        .await
        .unwrap()
        .expect_err("Flow/product terminal disagreement must abort supersession")
        .to_string()
        .contains("Flow and CI run terminal transitions disagreed"));
    let finalizer_race_shape: (String, String, String, i64, i64) = sqlx::query_as(
        "SELECT old.state, w.state, new.state, \
           (SELECT count(*) FROM workflow_run WHERE run_id=$4), \
           (SELECT count(*) FROM cost_reservation \
             WHERE run_id LIKE ('ci-reserve:v1:' || $3::text || ':%')) \
         FROM ci_run old \
         JOIN workflow_run w ON w.tenant_id=old.tenant_id AND w.run_id=old.wf_run_id::text \
         JOIN ci_run new ON new.tenant_id=old.tenant_id \
         WHERE old.tenant_id=$1 AND old.run_id=$2::uuid AND new.run_id=$3::uuid",
    )
    .bind(pr_tenant)
    .bind(finalizer_old)
    .bind(finalizer_new)
    .bind(finalizer_new_wf)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        finalizer_race_shape,
        ("succeeded".into(), "running".into(), "queued".into(), 0, 0)
    );
    sqlx::query(
        "UPDATE ci_run SET state='cancelled', cost_settled=true, finished_at=clock_timestamp() \
         WHERE tenant_id=$1 AND run_id=$2::uuid",
    )
    .bind(pr_tenant)
    .bind(finalizer_new)
    .execute(&admin)
    .await
    .unwrap();

    let runtime = ci_production_runtime_factory_test_support(
        app.clone(),
        Region("fr-par".into()),
        supersession_ledger.clone(),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    let reporter = runtime.reporter_router().unwrap();

    // A job that completed before the newer head is immutable settled history, not cancellation
    // work. Supersession verifies its queue receipt, pricing mode, and settled Storage reservation,
    // then zero-settles only the two undispatched jobs.
    let completed_group = "pr:core:46";
    let completed_old = "10000000-0000-0000-0000-000000000181";
    let completed_old_wf = "20000000-0000-0000-0000-000000000181";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        completed_old,
        completed_old_wf,
        completed_group,
        Some(1),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    let completed_claim = seed_claimed_manifest_job(
        &admin,
        pr_tenant,
        completed_old,
        completed_old_wf,
        "running",
    )
    .await;
    let completed = CompletionClaim {
        tenant: TenantId(pr_tenant.into()),
        run: RunId(completed_old_wf.into()),
        job_id: completed_claim.claim.job_id.clone(),
        idem_token: completed_claim.idem_token.clone(),
        lease_owner: completed_claim.claim.lease_owner.clone(),
        lease_epoch: completed_claim.claim.lease_epoch,
        claim_nonce: completed_claim.claim.claim_nonce.clone(),
    };
    let completed_report = TerminalReport {
        passed: true,
        timed_out: false,
        usage: ResourceUsage {
            cpu_seconds: 3,
            mem_byte_seconds: 536_870_912,
        },
        result_refs: vec![ArtifactRef(format!(
            "myelin://{pr_tenant}/ci/artifact/completed-result"
        ))],
    };
    assert_ne!(
        reporter.report_done(&completed, &completed_report).unwrap(),
        SignalOutcome::TerminalNoOp
    );
    let completed_new = "10000000-0000-0000-0000-000000000182";
    let completed_new_wf = "20000000-0000-0000-0000-000000000182";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        completed_new,
        completed_new_wf,
        completed_group,
        Some(2),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    let completed_shape: (String, bool, String, i64, i64, i64) = sqlx::query_as(
        "SELECT r.state, r.cost_settled, q.state, \
           (SELECT count(*) FROM ci_job_accounting a \
             WHERE a.tenant_id=r.tenant_id AND a.ci_run_id=r.run_id), \
           (SELECT count(*) FROM ci_cost_event c \
             WHERE c.tenant_id=r.tenant_id AND c.run_id=r.run_id), \
           (SELECT count(*) FROM cost_reservation c \
             WHERE c.tenant_id=r.tenant_id \
               AND c.run_id LIKE ('ci-reserve:v1:' || r.run_id::text || ':%') \
               AND c.state='settled') \
         FROM ci_run r JOIN job_queue q \
           ON q.tenant_id=r.tenant_id AND q.run_id=r.wf_run_id \
         WHERE r.tenant_id=$1 AND r.run_id=$2::uuid",
    )
    .bind(pr_tenant)
    .bind(completed_old)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        completed_shape,
        ("cancelled".into(), true, "terminal".into(), 3, 6, 3)
    );
    assert_cancelled_facts(&admin, pr_tenant, completed_old, &[true]).await;

    // Matching IDs, receipt text, and pricing token are insufficient accounting authority. Forge a
    // mutually matching queue/accounting receipt and an exact Storage settlement, but diverge the
    // CI usage projection. Supersession must verify all monetary truth and abort the replacement.
    let corrupt_group = "pr:core:47";
    let corrupt_old = "10000000-0000-0000-0000-000000000191";
    let corrupt_old_wf = "20000000-0000-0000-0000-000000000191";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        corrupt_old,
        corrupt_old_wf,
        corrupt_group,
        Some(1),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    let corrupt_claim =
        seed_claimed_manifest_job(&admin, pr_tenant, corrupt_old, corrupt_old_wf, "running").await;
    sqlx::query(
        "UPDATE job_queue SET state='terminal', \
           completion_receipt='v3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff' \
         WHERE tenant_id=$1 AND job_id=$2::uuid",
    )
    .bind(pr_tenant)
    .bind(&corrupt_claim.claim.job_id)
    .execute(&admin)
    .await
    .unwrap();
    let corrupt_reserved: i64 = sqlx::query_scalar(
        "SELECT reserved FROM cost_reservation \
         WHERE tenant_id=$1 AND region='fr-par' AND run_id=$2",
    )
    .bind(pr_tenant)
    .bind(&corrupt_claim.reserve_handle)
    .fetch_one(&admin)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE cost_reservation SET state='settled' \
         WHERE tenant_id=$1 AND region='fr-par' AND run_id=$2",
    )
    .bind(pr_tenant)
    .bind(&corrupt_claim.reserve_handle)
    .execute(&admin)
    .await
    .unwrap();
    for (ord, unit) in [(0_i32, "cpu_seconds"), (1_i32, "mem_gb_seconds")] {
        sqlx::query(
            "INSERT INTO cost_event \
               (tenant_id, region, run_id, ord, unit, wholesale, markup) \
             VALUES ($1, 'fr-par', $2, $3, $4, 0, 0)",
        )
        .bind(pr_tenant)
        .bind(&corrupt_claim.reserve_handle)
        .bind(ord)
        .bind(unit)
        .execute(&admin)
        .await
        .unwrap();
    }
    for (cost_id, meter, amount) in [
        ("30000000-0000-0000-0000-000000000191", "cpu_seconds", 1_i64),
        (
            "30000000-0000-0000-0000-000000000192",
            "mem_gb_seconds",
            0_i64,
        ),
    ] {
        sqlx::query(
            "INSERT INTO ci_cost_event \
               (tenant_id, region, cost_id, run_id, job_id, meter, amount, \
                wholesale_minor_units, markup_minor_units, kind) \
             VALUES ($1, 'fr-par', $2::uuid, $3::uuid, $4::uuid, $5, $6, 0, 0, 'ci')",
        )
        .bind(pr_tenant)
        .bind(cost_id)
        .bind(corrupt_old)
        .bind(&corrupt_claim.claim.job_id)
        .bind(meter)
        .bind(amount)
        .execute(&admin)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO ci_job_accounting \
           (tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle, passed, timed_out, \
            skipped, cpu_seconds, mem_byte_seconds, pricing_revision, billed_minor_units, \
            refunded_minor_units, completion_receipt) \
         VALUES ($1, 'fr-par', $2::uuid, $3::uuid, $4::uuid, $5, true, false, false, \
                 0, 0, 'tier-p-operational:v1', 0, $6, \
                 'v3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff')",
    )
    .bind(pr_tenant)
    .bind(&corrupt_claim.claim.job_id)
    .bind(corrupt_old_wf)
    .bind(corrupt_old)
    .bind(&corrupt_claim.reserve_handle)
    .bind(corrupt_reserved)
    .execute(&admin)
    .await
    .unwrap();
    let corrupt_new = "10000000-0000-0000-0000-000000000192";
    let corrupt_new_wf = "20000000-0000-0000-0000-000000000192";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        corrupt_new,
        corrupt_new_wf,
        corrupt_group,
        Some(2),
        true,
    )
    .await;
    assert!(pr_starter
        .run_once()
        .await
        .expect_err("divergent monetary projection must abort supersession")
        .to_string()
        .contains("accounting was refused"));
    let corrupt_rollback: (String, String, i64, i64, i64) = sqlx::query_as(
        "SELECT old.state, new.state, \
           (SELECT count(*) FROM ci_drive_manifest WHERE ci_run_id=$3::uuid), \
           (SELECT count(*) FROM workflow_run WHERE run_id=$4), \
           (SELECT count(*) FROM cost_reservation \
             WHERE run_id LIKE ('ci-reserve:v1:' || $3::text || ':%')) \
         FROM ci_run old JOIN ci_run new ON new.tenant_id=old.tenant_id \
         WHERE old.tenant_id=$1 AND old.run_id=$2::uuid AND new.run_id=$3::uuid",
    )
    .bind(pr_tenant)
    .bind(corrupt_old)
    .bind(corrupt_new)
    .bind(corrupt_new_wf)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        corrupt_rollback,
        ("running".into(), "queued".into(), 0, 0, 0)
    );
    sqlx::query(
        "UPDATE ci_run SET state='cancelled', cost_settled=true, finished_at=clock_timestamp() \
         WHERE tenant_id=$1 AND run_id=$2::uuid",
    )
    .bind(pr_tenant)
    .bind(corrupt_new)
    .execute(&admin)
    .await
    .unwrap();

    // A no-queue skipped receipt is accepted only when its deterministic supersession receipt and
    // full-refund monetary facts agree. Even exact zero-unit ledgers/projections cannot bless an
    // invented completion receipt.
    let skipped_forge_group = "pr:core:51";
    let skipped_forge_old = "10000000-0000-0000-0000-000000000231";
    let skipped_forge_old_wf = "20000000-0000-0000-0000-000000000231";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        skipped_forge_old,
        skipped_forge_old_wf,
        skipped_forge_group,
        Some(1),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    let (skipped_manifest, _) = manifest_store
        .load_by_identity(skipped_forge_old_wf, skipped_forge_old)
        .await
        .unwrap()
        .unwrap();
    let skipped_job = skipped_manifest.jobs.first().unwrap();
    let skipped_reserved: i64 = sqlx::query_scalar(
        "SELECT reserved FROM cost_reservation \
         WHERE tenant_id=$1 AND region='fr-par' AND run_id=$2",
    )
    .bind(pr_tenant)
    .bind(&skipped_job.reserve_handle)
    .fetch_one(&admin)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE cost_reservation SET state='settled' \
         WHERE tenant_id=$1 AND region='fr-par' AND run_id=$2",
    )
    .bind(pr_tenant)
    .bind(&skipped_job.reserve_handle)
    .execute(&admin)
    .await
    .unwrap();
    for (ord, unit) in [(0_i32, "cpu_seconds"), (1_i32, "mem_gb_seconds")] {
        sqlx::query(
            "INSERT INTO cost_event \
               (tenant_id, region, run_id, ord, unit, wholesale, markup) \
             VALUES ($1, 'fr-par', $2, $3, $4, 0, 0)",
        )
        .bind(pr_tenant)
        .bind(&skipped_job.reserve_handle)
        .bind(ord)
        .bind(unit)
        .execute(&admin)
        .await
        .unwrap();
    }
    for (cost_id, meter) in [
        ("30000000-0000-0000-0000-000000000231", "cpu_seconds"),
        ("30000000-0000-0000-0000-000000000232", "mem_gb_seconds"),
    ] {
        sqlx::query(
            "INSERT INTO ci_cost_event \
               (tenant_id, region, cost_id, run_id, job_id, meter, amount, \
                wholesale_minor_units, markup_minor_units, kind) \
             VALUES ($1, 'fr-par', $2::uuid, $3::uuid, $4::uuid, $5, 0, 0, 0, 'ci')",
        )
        .bind(pr_tenant)
        .bind(cost_id)
        .bind(skipped_forge_old)
        .bind(&skipped_job.job_id)
        .bind(meter)
        .execute(&admin)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO ci_job_accounting \
           (tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle, passed, timed_out, \
            skipped, cpu_seconds, mem_byte_seconds, pricing_revision, billed_minor_units, \
            refunded_minor_units, completion_receipt) \
         VALUES ($1, 'fr-par', $2::uuid, $3::uuid, $4::uuid, $5, false, false, true, \
                 0, 0, 'tier-p-operational:v1', 0, $6, \
                 'v3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee')",
    )
    .bind(pr_tenant)
    .bind(&skipped_job.job_id)
    .bind(skipped_forge_old_wf)
    .bind(skipped_forge_old)
    .bind(&skipped_job.reserve_handle)
    .bind(skipped_reserved)
    .execute(&admin)
    .await
    .unwrap();
    let skipped_forge_new = "10000000-0000-0000-0000-000000000232";
    let skipped_forge_new_wf = "20000000-0000-0000-0000-000000000232";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        skipped_forge_new,
        skipped_forge_new_wf,
        skipped_forge_group,
        Some(2),
        true,
    )
    .await;
    assert!(pr_starter
        .run_once()
        .await
        .expect_err("invented skipped receipt must abort supersession")
        .to_string()
        .contains("accounting receipt disagrees with queue lifecycle"));
    let skipped_forge_rollback: (String, String, i64) = sqlx::query_as(
        "SELECT old.state, new.state, \
           (SELECT count(*) FROM workflow_run WHERE run_id=$3) \
         FROM ci_run old JOIN ci_run new ON new.tenant_id=old.tenant_id \
         WHERE old.tenant_id=$1 AND old.run_id=$2::uuid AND new.run_id=$4::uuid",
    )
    .bind(pr_tenant)
    .bind(skipped_forge_old)
    .bind(skipped_forge_new_wf)
    .bind(skipped_forge_new)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        skipped_forge_rollback,
        ("running".into(), "queued".into(), 0)
    );
    sqlx::query(
        "UPDATE ci_run SET state='cancelled', cost_settled=true, finished_at=clock_timestamp() \
         WHERE tenant_id=$1 AND run_id=$2::uuid",
    )
    .bind(pr_tenant)
    .bind(skipped_forge_new)
    .execute(&admin)
    .await
    .unwrap();

    // If final launch wins first, supersession never zero-settles that running generation. It
    // terminates Flow and closes every other job, leaving cost_settled=false until the real terminal
    // report accounts actual usage. The late report is an acknowledged terminal no-op at Flow while
    // atomically closing the cancelled ci_run; exact replay changes nothing.
    let launch_wins_group = "pr:core:45";
    let launch_wins_old = "10000000-0000-0000-0000-000000000171";
    let launch_wins_old_wf = "20000000-0000-0000-0000-000000000171";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        launch_wins_old,
        launch_wins_old_wf,
        launch_wins_group,
        Some(1),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    let launched_claim = seed_claimed_manifest_job(
        &admin,
        pr_tenant,
        launch_wins_old,
        launch_wins_old_wf,
        "running",
    )
    .await;
    let launch_wins_new = "10000000-0000-0000-0000-000000000172";
    let launch_wins_new_wf = "20000000-0000-0000-0000-000000000172";
    insert_pr_run(
        &admin,
        blobs.as_ref(),
        pr_tenant,
        launch_wins_new,
        launch_wins_new_wf,
        launch_wins_group,
        Some(2),
        true,
    )
    .await;
    assert!(matches!(
        pr_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    let before_late_report: (String, bool, String, i64, i64) = sqlx::query_as(
        "SELECT r.state, r.cost_settled, q.state, \
           (SELECT count(*) FROM ci_job_accounting a \
             WHERE a.tenant_id=r.tenant_id AND a.ci_run_id=r.run_id), \
           (SELECT count(*) FROM cost_reservation c \
             WHERE c.tenant_id=r.tenant_id \
               AND c.run_id LIKE ('ci-reserve:v1:' || r.run_id::text || ':%') \
               AND c.state='inflight') \
         FROM ci_run r JOIN job_queue q \
           ON q.tenant_id=r.tenant_id AND q.run_id=r.wf_run_id \
         WHERE r.tenant_id=$1 AND r.run_id=$2::uuid",
    )
    .bind(pr_tenant)
    .bind(launch_wins_old)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        before_late_report,
        ("cancelled".into(), false, "running".into(), 2, 1)
    );
    assert_cancelled_facts(&admin, pr_tenant, launch_wins_old, &[false]).await;
    let completion = CompletionClaim {
        tenant: TenantId(pr_tenant.into()),
        run: RunId(launch_wins_old_wf.into()),
        job_id: launched_claim.claim.job_id.clone(),
        idem_token: launched_claim.idem_token.clone(),
        lease_owner: launched_claim.claim.lease_owner.clone(),
        lease_epoch: launched_claim.claim.lease_epoch,
        claim_nonce: launched_claim.claim.claim_nonce.clone(),
    };
    let report = TerminalReport {
        passed: true,
        timed_out: false,
        usage: ResourceUsage {
            cpu_seconds: 7,
            mem_byte_seconds: 1_073_741_824,
        },
        result_refs: vec![ArtifactRef(format!(
            "myelin://{pr_tenant}/ci/artifact/late-result"
        ))],
    };
    assert_eq!(
        reporter.report_done(&completion, &report).unwrap(),
        SignalOutcome::TerminalNoOp
    );
    assert_eq!(
        reporter.report_done(&completion, &report).unwrap(),
        SignalOutcome::TerminalNoOp,
        "acknowledgement-loss replay is exact"
    );
    let after_late_report: (bool, String, i64, i64, i64) = sqlx::query_as(
        "SELECT r.cost_settled, q.state, \
           (SELECT count(*) FROM ci_job_accounting a \
             WHERE a.tenant_id=r.tenant_id AND a.ci_run_id=r.run_id), \
           (SELECT count(*) FROM ci_cost_event c \
             WHERE c.tenant_id=r.tenant_id AND c.run_id=r.run_id), \
           (SELECT count(*) FROM wf_signal s \
             WHERE s.tenant_id=r.tenant_id AND s.run_id=r.wf_run_id::text) \
         FROM ci_run r JOIN job_queue q \
           ON q.tenant_id=r.tenant_id AND q.run_id=r.wf_run_id \
         WHERE r.tenant_id=$1 AND r.run_id=$2::uuid",
    )
    .bind(pr_tenant)
    .bind(launch_wins_old)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(after_late_report, (true, "terminal".into(), 3, 6, 0));
    assert_cancelled_facts(&admin, pr_tenant, launch_wins_old, &[false, true]).await;

    // Two concurrent starters see one row. SKIP LOCKED lets one win and the other return idle;
    // there is exactly one workflow and the state transition cannot split from it.
    let run1 = "10000000-0000-0000-0000-000000000001";
    let wf1 = "20000000-0000-0000-0000-000000000001";
    insert_run(&admin, blobs.as_ref(), "tenant_a", run1, wf1).await;
    AUTHORITY_CALLS.store(0, Ordering::SeqCst);
    let first = starter(&app, "tenant_a", blobs.clone());
    let second = starter(&app, "tenant_a", blobs.clone());
    let (a, b) = tokio::join!(first.run_once(), second.run_once());
    let outcomes = [a.expect("first pass"), b.expect("second pass")];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, StartQueuedOutcome::Started { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, StartQueuedOutcome::Idle))
            .count(),
        1
    );
    assert_atomic_started(&admin, "tenant_a", run1, true, true).await;
    assert_exact_jobs(&admin, "tenant_a", run1).await;
    assert_run_ledger_index_is_used(&admin, "tenant_a", run1).await;
    assert_eq!(visible_job_count(&app, "tenant_a", "fr-par").await, 3);
    assert_eq!(visible_job_count(&app, "tenant_empty", "fr-par").await, 0);
    assert_eq!(visible_job_count(&app, "tenant_a", "de-fra").await, 0);
    let first_attempt_rows = attempt_rows(&admin, "tenant_a").await;
    assert_eq!(
        first_attempt_rows,
        vec![
            (
                "build".into(),
                2,
                Some(sqlx::types::Uuid::parse_str(run1).unwrap()),
            ),
            (
                "package".into(),
                2,
                Some(sqlx::types::Uuid::parse_str(run1).unwrap()),
            ),
            (
                "test".into(),
                2,
                Some(sqlx::types::Uuid::parse_str(run1).unwrap()),
            ),
        ],
        "concurrent first launch allocates each authored context exactly once"
    );
    let run1_manifest_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ci_drive_manifest WHERE tenant_id='tenant_a' AND ci_run_id=$1::uuid",
    )
    .bind(run1)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(run1_manifest_count, 1);
    assert_initial_checks(&admin, "tenant_a", run1, 1).await;
    assert_eq!(
        AUTHORITY_CALLS.load(Ordering::SeqCst),
        1,
        "only the exact queued-row lock winner may invoke launch authority"
    );

    // Reserve two runs before either starter executes. Their immutable attempts stay A=1/B=2;
    // starting the older run never reallocates it above the newer queued fact.
    let prequeued_tenant = "tenant_prequeued_attempts";
    let prequeued_a = "11000000-0000-0000-0000-000000000001";
    let prequeued_b = "11000000-0000-0000-0000-000000000002";
    insert_run(
        &admin,
        blobs.as_ref(),
        prequeued_tenant,
        prequeued_a,
        "21000000-0000-0000-0000-000000000001",
    )
    .await;
    insert_run(
        &admin,
        blobs.as_ref(),
        prequeued_tenant,
        prequeued_b,
        "21000000-0000-0000-0000-000000000002",
    )
    .await;
    starter(&app, prequeued_tenant, blobs.clone())
        .run_once()
        .await
        .expect("start older prequeued run");
    assert_initial_checks(&admin, prequeued_tenant, prequeued_a, 1).await;
    starter(&app, prequeued_tenant, blobs.clone())
        .run_once()
        .await
        .expect("start newer prequeued run");
    assert_initial_checks(&admin, prequeued_tenant, prequeued_b, 2).await;

    let mut newer_queued =
        initial_check_envelopes(&admin, prequeued_tenant, prequeued_b).await[0]["payload"].clone();
    newer_queued["state"] = serde_json::json!("queued");
    let newer_queued: myelin_git::check_status::CheckStatus =
        serde_json::from_value(newer_queued).expect("decode newer queued fact");
    let mut older_terminal =
        initial_check_envelopes(&admin, prequeued_tenant, prequeued_a).await[0]["payload"].clone();
    older_terminal["state"] = serde_json::json!("success");
    older_terminal["cost_settled"] = serde_json::json!(true);
    older_terminal["completed_at"] = serde_json::json!("2026-07-23T00:00:01Z");
    let older_terminal: myelin_git::check_status::CheckStatus =
        serde_json::from_value(older_terminal).expect("decode older terminal fact");
    let mut projected = myelin_git::check_status::CheckStatusProjection::new();
    projected.apply(&newer_queued);
    projected.apply(&older_terminal);
    let current = projected
        .current(&newer_queued.key())
        .expect("newer queued attempt remains current");
    assert_eq!(current.run_attempt, 2);
    assert_eq!(
        current.state,
        myelin_git::check_status::CheckState::Queued,
        "an older terminal fact cannot supersede the newer queued rerun"
    );

    // ── CT-004: the per-tenant starter COMPOSITION SEAM (`PgCiRunStarterFactory`) against the real
    // migrated schema — the exact router the service main composes at the root (dormant behind the
    // runner activation gate). (a) A factory-minted starter CONSTRUCTS against the live schema and starts
    // its own tenant's queued run atomically. (b) Per-tenant scoping SURVIVES the seam: a starter minted
    // for tenant A never discovers or starts tenant B's queued run; a starter minted for B starts B's.
    // Exercised here while v1 is still the sole active definition (before the fresh-old-pin scenario
    // registers v2 below).
    let starters = factory(&app, blobs.clone());
    assert_eq!(starters.region(), &Region("fr-par".into()));

    let run_fa = "10000000-0000-0000-0000-0000000000fa";
    let wf_fa = "20000000-0000-0000-0000-0000000000fa";
    insert_run(&admin, blobs.as_ref(), "tenant_factory_a", run_fa, wf_fa).await;
    let a_started = starters
        .starter_for(
            TenantId("tenant_factory_a".into()),
            CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
        )
        .expect("factory mints an exact-cell starter against the migrated schema")
        .run_once()
        .await
        .expect("factory-minted starter drives run_once");
    assert!(matches!(a_started, StartQueuedOutcome::Started { .. }));
    assert_atomic_started(&admin, "tenant_factory_a", run_fa, true, true).await;

    // (b) tenant B has a queued run; the A-minted starter must never see or start it.
    let run_fb = "10000000-0000-0000-0000-0000000000fb";
    let wf_fb = "20000000-0000-0000-0000-0000000000fb";
    insert_run(&admin, blobs.as_ref(), "tenant_factory_b", run_fb, wf_fb).await;
    let a_starter = starters
        .starter_for(
            TenantId("tenant_factory_a".into()),
            CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
        )
        .expect("mint the tenant A starter");
    assert_eq!(
        a_starter.run_once().await.unwrap(),
        StartQueuedOutcome::Idle
    );
    assert_atomic_started(&admin, "tenant_factory_b", run_fb, false, false).await;

    // The B-minted starter starts B's run — proving the router binds each record to its own cell.
    let b_started = starters
        .starter_for(
            TenantId("tenant_factory_b".into()),
            CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
        )
        .expect("mint the tenant B starter")
        .run_once()
        .await
        .expect("tenant B starter drives run_once");
    assert!(matches!(b_started, StartQueuedOutcome::Started { .. }));
    assert_atomic_started(&admin, "tenant_factory_b", run_fb, true, true).await;

    // Exact legacy replay is a byte-for-byte no-op on the complete DAG ledger. Re-open only the
    // lifecycle split that this starter repairs; the existing workflow and all three jobs remain.
    sqlx::query("UPDATE ci_run SET state='queued' WHERE tenant_id='tenant_a' AND run_id=$1::uuid")
        .bind(run1)
        .execute(&admin)
        .await
        .unwrap();
    assert!(matches!(
        starter(&app, "tenant_a", blobs.clone())
            .run_once()
            .await
            .expect("exact ci_job replay"),
        StartQueuedOutcome::Started { .. }
    ));
    assert_atomic_started(&admin, "tenant_a", run1, true, true).await;
    assert_exact_jobs(&admin, "tenant_a", run1).await;
    assert_initial_checks(&admin, "tenant_a", run1, 1).await;

    // A divergent immutable field is refused before the queued split can commit. Restore the
    // adversarial edit after the probe, never through the starter.
    for (mutation, restore) in [(
        "UPDATE ci_job SET spec_ref='myelin://tenant_a/ci/snapshot/blake3:forged' \
             WHERE tenant_id='tenant_a' AND run_id=$1::uuid AND name='package'",
        "UPDATE ci_job SET spec_ref='myelin://tenant_a/ci/artifact/drive-manifest-' || \
             (SELECT manifest_digest FROM ci_drive_manifest \
              WHERE tenant_id='tenant_a' AND ci_run_id=$1::uuid) \
             WHERE tenant_id='tenant_a' AND run_id=$1::uuid AND name='package'",
    )] {
        sqlx::query(
            "UPDATE ci_run SET state='queued' WHERE tenant_id='tenant_a' AND run_id=$1::uuid",
        )
        .bind(run1)
        .execute(&admin)
        .await
        .unwrap();
        sqlx::query(mutation)
            .bind(run1)
            .execute(&admin)
            .await
            .unwrap();
        let error = starter(&app, "tenant_a", blobs.clone())
            .run_once()
            .await
            .expect_err("divergent ci_job ledger must fail closed");
        assert!(error.to_string().contains("ci_job"));
        let state: String = sqlx::query_scalar(
            "SELECT state FROM ci_run WHERE tenant_id='tenant_a' AND run_id=$1::uuid",
        )
        .bind(run1)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(state, "queued");
        sqlx::query(restore)
            .bind(run1)
            .execute(&admin)
            .await
            .unwrap();
    }

    // Replay verifies immutable job authority but never rewinds legitimate lifecycle progress.
    sqlx::query("UPDATE ci_run SET state='queued' WHERE tenant_id='tenant_a' AND run_id=$1::uuid")
        .bind(run1)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE ci_job SET state='running', attempt=2 \
         WHERE tenant_id='tenant_a' AND run_id=$1::uuid AND name='package'",
    )
    .bind(run1)
    .execute(&admin)
    .await
    .unwrap();
    assert!(matches!(
        starter_without_authority(&app, "tenant_a", blobs.clone())
            .run_once()
            .await
            .expect("replay preserves advanced job lifecycle"),
        StartQueuedOutcome::Started { .. }
    ));
    let advanced: (String, i32) = sqlx::query_as(
        "SELECT state, attempt FROM ci_job \
         WHERE tenant_id='tenant_a' AND run_id=$1::uuid AND name='package'",
    )
    .bind(run1)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(advanced, ("running".into(), 2));
    sqlx::query(
        "UPDATE ci_job SET state='queued', attempt=1 \
         WHERE tenant_id='tenant_a' AND run_id=$1::uuid AND name='package'",
    )
    .bind(run1)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("UPDATE ci_run SET state='queued' WHERE tenant_id='tenant_a' AND run_id=$1::uuid")
        .bind(run1)
        .execute(&admin)
        .await
        .unwrap();

    // The run-id half of the exact SELECT catches an unexpected extra ledger row even though its
    // job id is not one of the derived ids.
    let extra_id = "80000000-0000-8000-8000-000000000001";
    sqlx::query(
        "INSERT INTO ci_job (tenant_id, region, job_id, run_id, stage, name, needs, matrix_key, \
         spec_ref, state, attempt, result_summary) \
         SELECT tenant_id, region, $2::uuid, run_id, 'extra', 'extra', '{}', NULL, \
                definition_snapshot, 'queued', 1, NULL \
         FROM ci_run WHERE tenant_id='tenant_a' AND run_id=$1::uuid",
    )
    .bind(run1)
    .bind(extra_id)
    .execute(&admin)
    .await
    .unwrap();
    assert!(starter(&app, "tenant_a", blobs.clone())
        .run_once()
        .await
        .expect_err("extra run ledger row must fail closed")
        .to_string()
        .contains("has 4 rows"));
    sqlx::query("DELETE FROM ci_job WHERE tenant_id='tenant_a' AND job_id=$1::uuid")
        .bind(extra_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE ci_run SET state='running' WHERE tenant_id='tenant_a' AND run_id=$1::uuid")
        .bind(run1)
        .execute(&admin)
        .await
        .unwrap();
    assert_exact_jobs(&admin, "tenant_a", run1).await;

    // The expected-id half catches a truncated-id collision owned by another run. The two otherwise
    // fresh victim jobs inserted before verification roll back with the failed start.
    let collision_run = "18000000-0000-0000-0000-000000000001";
    let collision_wf = "28000000-0000-0000-0000-000000000001";
    let owner_run = "18000000-0000-0000-0000-000000000002";
    let owner_wf = "28000000-0000-0000-0000-000000000002";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_job_collision",
        collision_run,
        collision_wf,
    )
    .await;
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_job_collision",
        owner_run,
        owner_wf,
    )
    .await;
    let build = &plan().jobs[0];
    let colliding_id = ci_job_id_v2(
        &TenantId("tenant_job_collision".into()),
        sqlx::types::Uuid::parse_str(collision_run).unwrap(),
        &build.stage,
        &build.name,
        &build.matrix_identity(),
    );
    sqlx::query(
        "INSERT INTO ci_job (tenant_id, region, job_id, run_id, stage, name, needs, matrix_key, \
         spec_ref, state, attempt, result_summary) \
         SELECT tenant_id, region, $2, run_id, 'foreign', 'foreign', '{}', NULL, \
                definition_snapshot, 'queued', 1, NULL \
         FROM ci_run WHERE tenant_id='tenant_job_collision' AND run_id=$1::uuid",
    )
    .bind(owner_run)
    .bind(colliding_id)
    .execute(&admin)
    .await
    .unwrap();
    let error = starter(&app, "tenant_job_collision", blobs.clone())
        .run_once()
        .await
        .expect_err("cross-run deterministic id owner must fail closed");
    assert!(error.to_string().contains("diverges"));
    assert_atomic_started(&admin, "tenant_job_collision", collision_run, false, false).await;
    let owner_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ci_job WHERE tenant_id='tenant_job_collision' AND run_id=$1::uuid",
    )
    .bind(owner_run)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        owner_jobs, 1,
        "foreign owner survives the rolled-back victim start"
    );

    // A reconstructed starter repairs a manifest-backed lifecycle split without reallocating
    // attempts or reconstructing authority from mutable inputs.
    let run2 = "10000000-0000-0000-0000-000000000002";
    let wf2 = "20000000-0000-0000-0000-000000000002";
    insert_run(&admin, blobs.as_ref(), "tenant_a", run2, wf2).await;
    starter(&app, "tenant_a", blobs.clone())
        .run_once()
        .await
        .expect("create the complete manifest-backed start");
    assert_initial_checks(&admin, "tenant_a", run2, 2).await;
    sqlx::query("UPDATE ci_run SET state='queued' WHERE tenant_id='tenant_a' AND run_id=$1::uuid")
        .bind(run2)
        .execute(&admin)
        .await
        .unwrap();
    let attempts_after_run2 = attempt_rows(&admin, "tenant_a").await;
    assert!(attempts_after_run2.iter().all(|(_, next, current)| {
        *next == 3 && *current == Some(sqlx::types::Uuid::parse_str(run2).unwrap())
    }));
    let source_hash = ContentHash::blake3(&plan().canonical_bytes().unwrap());
    blobs
        .delete(&TenantId("tenant_a".into()), &source_hash)
        .expect("remove source CAS after manifest commit");
    let restarted = starter_without_authority_with(
        &app,
        "tenant_a",
        blobs.clone(),
        CiWorkflowDefinitionPin::new(99, "blake3:not-the-frozen-pin").unwrap(),
    );
    assert!(matches!(
        restarted
            .run_once()
            .await
            .expect("manifest replay ignores unavailable CAS and mutable composition pin"),
        StartQueuedOutcome::Started { ref wf_run_id, .. } if wf_run_id == wf2
    ));
    assert_atomic_started(&admin, "tenant_a", run2, true, true).await;
    assert_exact_jobs(&admin, "tenant_a", run2).await;

    // Retry an older frozen manifest after a newer run has superseded all three contexts. Replay
    // neither consults today's unavailable authority nor moves the monotonic attempt ledger back to
    // the old run (or forward again).
    sqlx::query("UPDATE ci_run SET state='queued' WHERE tenant_id='tenant_a' AND run_id=$1::uuid")
        .bind(run1)
        .execute(&admin)
        .await
        .unwrap();
    assert!(matches!(
        starter_without_authority(&app, "tenant_a", blobs.clone())
            .run_once()
            .await
            .expect("old immutable manifest replays without current policy"),
        StartQueuedOutcome::Started { ref wf_run_id, .. } if wf_run_id == wf1
    ));
    assert_eq!(
        attempt_rows(&admin, "tenant_a").await,
        attempts_after_run2,
        "exact replay never reallocates or supersedes check attempts"
    );
    assert_exact_jobs(&admin, "tenant_a", run1).await;

    // A failure after workflow insertion (the trigger rejects the lifecycle CAS) rolls the workflow
    // back too. No queued->running row can exist without its workflow.
    let run3 = "10000000-0000-0000-0000-000000000003";
    let wf3 = "20000000-0000-0000-0000-000000000003";
    insert_run(&admin, blobs.as_ref(), "tenant_rollback", run3, wf3).await;
    admin
        .execute(
            "CREATE FUNCTION reject_starter_probe() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN IF NEW.run_id = '10000000-0000-0000-0000-000000000003'::uuid \
             AND NEW.state = 'running' THEN RAISE EXCEPTION 'probe rollback'; END IF; RETURN NEW; END $$",
        )
        .await
        .unwrap();
    admin
        .execute("CREATE TRIGGER reject_starter_probe BEFORE UPDATE ON ci_run FOR EACH ROW EXECUTE FUNCTION reject_starter_probe()")
        .await
        .unwrap();
    assert!(starter(&app, "tenant_rollback", blobs.clone())
        .run_once()
        .await
        .is_err());
    assert_atomic_started(&admin, "tenant_rollback", run3, false, false).await;
    let rolled_back_side_effects: (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT count(*) FROM ci_drive_manifest WHERE tenant_id='tenant_rollback'),
           (SELECT count(*) FROM check_attempt WHERE tenant_id='tenant_rollback'),
           (SELECT count(*) FROM ci_run_check_attempt WHERE tenant_id='tenant_rollback')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        rolled_back_side_effects,
        (0, 3, 3),
        "failed start preserves reserve-time attempt authority but rolls back starter state"
    );
    assert!(initial_check_envelopes(&admin, "tenant_rollback", run3)
        .await
        .is_empty());

    // A manifest and workflow are an atomic replay pair. An orphan manifest is corruption and must
    // not invoke current authority or synthesize a replacement workflow.
    let orphan_run = "10000000-0000-0000-0000-0000000000a1";
    let orphan_wf = "20000000-0000-0000-0000-0000000000a1";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_orphan_manifest",
        orphan_run,
        orphan_wf,
    )
    .await;
    starter(&app, "tenant_orphan_manifest", blobs.clone())
        .run_once()
        .await
        .unwrap();
    sqlx::query("DELETE FROM workflow_run WHERE tenant_id='tenant_orphan_manifest' AND run_id=$1")
        .bind(orphan_wf)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE ci_run SET state='queued' \
         WHERE tenant_id='tenant_orphan_manifest' AND run_id=$1::uuid",
    )
    .bind(orphan_run)
    .execute(&admin)
    .await
    .unwrap();
    let calls_before_orphan = AUTHORITY_CALLS.load(Ordering::SeqCst);
    assert!(
        starter_without_authority(&app, "tenant_orphan_manifest", blobs.clone())
            .run_once()
            .await
            .expect_err("manifest without workflow must fail closed")
            .to_string()
            .contains("without its atomically-started workflow")
    );
    assert_eq!(AUTHORITY_CALLS.load(Ordering::SeqCst), calls_before_orphan);

    // Frozen attempts are checked against the immutable per-run ledger. Removing one context makes
    // replay fail before any workflow/job/lifecycle mutation.
    let attempt_run = "10000000-0000-0000-0000-0000000000a2";
    let attempt_wf = "20000000-0000-0000-0000-0000000000a2";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_attempt_tamper",
        attempt_run,
        attempt_wf,
    )
    .await;
    starter(&app, "tenant_attempt_tamper", blobs.clone())
        .run_once()
        .await
        .unwrap();
    sqlx::query(
        "UPDATE ci_run SET state='queued' \
         WHERE tenant_id='tenant_attempt_tamper' AND run_id=$1::uuid",
    )
    .bind(attempt_run)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "DELETE FROM ci_run_check_attempt \
         WHERE tenant_id='tenant_attempt_tamper' AND context='test'",
    )
    .execute(&admin)
    .await
    .unwrap();
    assert!(
        starter_without_authority(&app, "tenant_attempt_tamper", blobs.clone())
            .run_once()
            .await
            .expect_err("missing attempt allocation must fail closed")
            .to_string()
            .contains("has no run-scoped allocation ledger")
    );
    assert_manifest_backed_queued(&admin, "tenant_attempt_tamper", attempt_run).await;

    // Replay requires the complete immutable ci_job ledger and never repairs a missing row.
    let missing_job_run = "10000000-0000-0000-0000-0000000000a3";
    let missing_job_wf = "20000000-0000-0000-0000-0000000000a3";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_missing_job",
        missing_job_run,
        missing_job_wf,
    )
    .await;
    starter(&app, "tenant_missing_job", blobs.clone())
        .run_once()
        .await
        .unwrap();
    sqlx::query(
        "UPDATE ci_run SET state='queued' \
         WHERE tenant_id='tenant_missing_job' AND run_id=$1::uuid",
    )
    .bind(missing_job_run)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "DELETE FROM ci_job WHERE tenant_id='tenant_missing_job' AND run_id=$1::uuid AND name='package'",
    )
    .bind(missing_job_run)
    .execute(&admin)
    .await
    .unwrap();
    assert!(
        starter_without_authority(&app, "tenant_missing_job", blobs.clone())
            .run_once()
            .await
            .expect_err("missing replay job must not be repaired")
            .to_string()
            .contains("has 2 rows")
    );
    let still_missing: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ci_job WHERE tenant_id='tenant_missing_job' AND run_id=$1::uuid",
    )
    .bind(missing_job_run)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(still_missing, 2);

    // A pre-minted run-id collision cannot clobber the foreign workflow and leaves ci_run queued.
    let run4 = "10000000-0000-0000-0000-000000000004";
    let wf4 = "20000000-0000-0000-0000-000000000004";
    insert_run(&admin, blobs.as_ref(), "tenant_id_collision", run4, wf4).await;
    let collision = flow_executor(&app, "tenant_id_collision");
    tokio::task::block_in_place(|| {
        collision
            .start_with_id(
                StartSpec {
                    wf_type: CI_PIPELINE_WF_TYPE.into(),
                    input: Vec::new(),
                    budget: None,
                    idem_key: "foreign-owner".into(),
                },
                Some(RunId(wf4.into())),
            )
            .expect("seed colliding workflow id");
    });
    assert!(starter(&app, "tenant_id_collision", blobs.clone())
        .run_once()
        .await
        .is_err());
    assert_atomic_started(&admin, "tenant_id_collision", run4, false, true).await;

    // An idempotency-key collision resolving to a different run id is also refused explicitly.
    let run5 = "10000000-0000-0000-0000-000000000005";
    let wf5 = "20000000-0000-0000-0000-000000000005";
    let other_wf5 = "29999999-0000-0000-0000-000000000005";
    insert_run(&admin, blobs.as_ref(), "tenant_key_collision", run5, wf5).await;
    let key_collision = flow_executor(&app, "tenant_key_collision");
    tokio::task::block_in_place(|| {
        key_collision
            .start_with_id(
                StartSpec {
                    wf_type: CI_PIPELINE_WF_TYPE.into(),
                    input: Vec::new(),
                    budget: None,
                    idem_key: format!("ci:{run5}"),
                },
                Some(RunId(other_wf5.into())),
            )
            .expect("seed divergent idempotency owner");
    });
    assert!(starter(&app, "tenant_key_collision", blobs.clone())
        .run_once()
        .await
        .is_err());
    assert_atomic_started(&admin, "tenant_key_collision", run5, false, false).await;

    // Even when both idempotency keys resolve to the expected pre-minted ID, a divergent stored
    // input cannot be blessed as this CI run. Verification locks and compares the exact row.
    let run7 = "10000000-0000-0000-0000-000000000007";
    let wf7 = "20000000-0000-0000-0000-000000000007";
    insert_run(&admin, blobs.as_ref(), "tenant_divergent", run7, wf7).await;
    let divergent = flow_executor(&app, "tenant_divergent");
    tokio::task::block_in_place(|| {
        divergent
            .start_with_id(
                StartSpec {
                    wf_type: CI_PIPELINE_WF_TYPE.into(),
                    input: Vec::new(),
                    budget: None,
                    idem_key: format!("ci:{run7}"),
                },
                Some(RunId(wf7.into())),
            )
            .expect("seed same-id same-key divergent workflow");
    });
    assert!(starter(&app, "tenant_divergent", blobs.clone())
        .run_once()
        .await
        .is_err());
    assert_atomic_started(&admin, "tenant_divergent", run7, false, true).await;

    // An exact historical workflow that is already terminal is not resurrected or linked to a
    // still-queued CI row.
    let run8 = "10000000-0000-0000-0000-000000000008";
    let wf8 = "20000000-0000-0000-0000-000000000008";
    insert_run(&admin, blobs.as_ref(), "tenant_terminal", run8, wf8).await;
    seed_exact_workflow(&app, &admin, blobs.clone(), "tenant_terminal", run8, wf8).await;
    sqlx::query(
        "UPDATE workflow_run SET state='completed' \
         WHERE tenant_id='tenant_terminal' AND region='fr-par' AND run_id=$1",
    )
    .bind(wf8)
    .execute(&admin)
    .await
    .unwrap();
    assert!(starter(&app, "tenant_terminal", blobs.clone())
        .run_once()
        .await
        .is_err());
    assert_manifest_backed_queued(&admin, "tenant_terminal", run8).await;

    // Hold ID/key/input fixed and mutate every other starter-owned immutable workflow column in an
    // isolated tenant. Each row is genuinely claimed and rejected by the post-start identity proof.
    let immutable_mutations = [
        (
            "tenant_wf_version",
            "11000000-0000-0000-0000-000000000001",
            "21000000-0000-0000-0000-000000000001",
            "UPDATE workflow_run SET wf_version=2 WHERE tenant_id=$1 AND run_id=$2",
        ),
        (
            "tenant_budget",
            "11000000-0000-0000-0000-000000000002",
            "21000000-0000-0000-0000-000000000002",
            "UPDATE workflow_run SET budget='{\"minor_units\":1}'::jsonb WHERE tenant_id=$1 AND run_id=$2",
        ),
        (
            "tenant_correlation",
            "11000000-0000-0000-0000-000000000003",
            "21000000-0000-0000-0000-000000000003",
            "UPDATE workflow_run SET correlation_id='foreign-correlation' WHERE tenant_id=$1 AND run_id=$2",
        ),
        (
            "tenant_causation",
            "11000000-0000-0000-0000-000000000004",
            "21000000-0000-0000-0000-000000000004",
            "UPDATE workflow_run SET causation_id='foreign-cause' WHERE tenant_id=$1 AND run_id=$2",
        ),
        (
            "tenant_caused_by",
            "11000000-0000-0000-0000-000000000005",
            "21000000-0000-0000-0000-000000000005",
            "UPDATE workflow_run SET caused_by='foreign-actor' WHERE tenant_id=$1 AND run_id=$2",
        ),
        (
            "tenant_depth",
            "11000000-0000-0000-0000-000000000006",
            "21000000-0000-0000-0000-000000000006",
            "UPDATE workflow_run SET depth=1 WHERE tenant_id=$1 AND run_id=$2",
        ),
        (
            "tenant_partition",
            "11000000-0000-0000-0000-000000000007",
            "21000000-0000-0000-0000-000000000007",
            "UPDATE workflow_run SET partition=((partition+1)%64)::smallint WHERE tenant_id=$1 AND run_id=$2",
        ),
    ];
    for (tenant, run_id, wf_run_id, mutation) in immutable_mutations {
        insert_run(&admin, blobs.as_ref(), tenant, run_id, wf_run_id).await;
        seed_exact_workflow(&app, &admin, blobs.clone(), tenant, run_id, wf_run_id).await;
        sqlx::query(mutation)
            .bind(tenant)
            .bind(wf_run_id)
            .execute(&admin)
            .await
            .expect("mutate one immutable workflow field");
        let error = starter(&app, tenant, blobs.clone())
            .run_once()
            .await
            .expect_err("divergent immutable workflow identity must be refused");
        assert!(error.to_string().contains("diverges"));
        assert_manifest_backed_queued(&admin, tenant, run_id).await;
    }

    // The typed code pin is load-bearing even when the version number matches.
    let run_hash = "12000000-0000-0000-0000-000000000001";
    let wf_hash = "22000000-0000-0000-0000-000000000001";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_bad_code_pin",
        run_hash,
        wf_hash,
    )
    .await;
    assert!(starter_with(
        &app,
        "tenant_bad_code_pin",
        blobs.clone(),
        CiWorkflowDefinitionPin::new(1, "blake3:not-the-body").unwrap(),
    )
    .run_once()
    .await
    .is_err());
    assert_atomic_started(&admin, "tenant_bad_code_pin", run_hash, false, false).await;

    // Queued lifecycle contradictions are refused before any workflow is created.
    for (tenant, run_id, wf_run_id, mutation) in [
        (
            "tenant_cost_settled",
            "12000000-0000-0000-0000-000000000002",
            "22000000-0000-0000-0000-000000000002",
            "UPDATE ci_run SET cost_settled=true WHERE tenant_id=$1 AND run_id=$2::uuid",
        ),
        (
            "tenant_finished",
            "12000000-0000-0000-0000-000000000003",
            "22000000-0000-0000-0000-000000000003",
            "UPDATE ci_run SET finished_at=now() WHERE tenant_id=$1 AND run_id=$2::uuid",
        ),
    ] {
        insert_run(&admin, blobs.as_ref(), tenant, run_id, wf_run_id).await;
        sqlx::query(mutation)
            .bind(tenant)
            .bind(run_id)
            .execute(&admin)
            .await
            .unwrap();
        assert!(starter(&app, tenant, blobs.clone())
            .run_once()
            .await
            .is_err());
        assert_atomic_started(&admin, tenant, run_id, false, false).await;
    }

    // CAS preflight holds no row lock: a concurrent authority mutation can commit, and the exact
    // re-lock compares the complete row and refuses it before workflow creation.
    let run_preflight = "12000000-0000-0000-0000-000000000004";
    let wf_preflight = "22000000-0000-0000-0000-000000000004";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_preflight_mutation",
        run_preflight,
        wf_preflight,
    )
    .await;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let pausing: Arc<dyn BlobStore + Send + Sync> = Arc::new(PausingBlobStore {
        inner: blobs.clone(),
        pause_once: AtomicBool::new(false),
        entered: entered_tx,
        release: Mutex::new(release_rx),
    });
    let preflight_starter = starter_with(
        &app,
        "tenant_preflight_mutation",
        pausing,
        CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
    );
    let runtime = tokio::runtime::Handle::current();
    let preflight_thread =
        std::thread::spawn(move || runtime.block_on(preflight_starter.run_once()));
    entered_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("starter reaches unlocked CAS preflight");
    sqlx::query(
        "UPDATE ci_run SET commit_oid='changed-during-preflight' \
         WHERE tenant_id='tenant_preflight_mutation' AND run_id=$1::uuid",
    )
    .bind(run_preflight)
    .execute(&admin)
    .await
    .expect("mutation is not blocked by object-store preflight");
    release_tx.send(()).unwrap();
    assert!(preflight_thread.join().unwrap().is_err());
    assert_atomic_started(
        &admin,
        "tenant_preflight_mutation",
        run_preflight,
        false,
        false,
    )
    .await;

    // Likewise, a winner can complete while another starter is in CAS preflight. The loser re-locks
    // the exact candidate, observes it is no longer queued, and returns Idle without a duplicate.
    let run_winner = "12000000-0000-0000-0000-000000000005";
    let wf_winner = "22000000-0000-0000-0000-000000000005";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_preflight_winner",
        run_winner,
        wf_winner,
    )
    .await;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let pausing: Arc<dyn BlobStore + Send + Sync> = Arc::new(PausingBlobStore {
        inner: blobs.clone(),
        pause_once: AtomicBool::new(false),
        entered: entered_tx,
        release: Mutex::new(release_rx),
    });
    let losing_starter = starter_with(
        &app,
        "tenant_preflight_winner",
        pausing,
        CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
    );
    let runtime = tokio::runtime::Handle::current();
    let losing_thread = std::thread::spawn(move || runtime.block_on(losing_starter.run_once()));
    entered_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("loser reaches unlocked CAS preflight");
    assert!(matches!(
        starter(&app, "tenant_preflight_winner", blobs.clone())
            .run_once()
            .await
            .unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    release_tx.send(()).unwrap();
    assert_eq!(
        losing_thread.join().unwrap().unwrap(),
        StartQueuedOutcome::Idle
    );
    assert_atomic_started(&admin, "tenant_preflight_winner", run_winner, true, true).await;

    // Existing pinned v1 runs may recover while v1 is draining; fresh intake remains closed.
    let run_draining = "12000000-0000-0000-0000-000000000006";
    let wf_draining = "22000000-0000-0000-0000-000000000006";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_draining_replay",
        run_draining,
        wf_draining,
    )
    .await;
    seed_exact_workflow(
        &app,
        &admin,
        blobs.clone(),
        "tenant_draining_replay",
        run_draining,
        wf_draining,
    )
    .await;
    sqlx::query("UPDATE wf_definition SET status='draining' WHERE wf_type=$1 AND version=1")
        .bind(CI_PIPELINE_WF_TYPE)
        .execute(&admin)
        .await
        .unwrap();
    assert!(matches!(
        starter(&app, "tenant_draining_replay", blobs.clone())
            .run_once()
            .await
            .unwrap(),
        StartQueuedOutcome::Started { .. }
    ));
    sqlx::query("UPDATE wf_definition SET status='active' WHERE wf_type=$1 AND version=1")
        .bind(CI_PIPELINE_WF_TYPE)
        .execute(&admin)
        .await
        .unwrap();

    // Invalid/absent CAS is refused before a workflow row exists and before lifecycle mutation.
    let run6 = "10000000-0000-0000-0000-000000000006";
    let wf6 = "20000000-0000-0000-0000-000000000006";
    insert_run(&admin, blobs.as_ref(), "tenant_cas", run6, wf6).await;
    sqlx::query(
        "UPDATE ci_run SET definition_snapshot=$1 WHERE tenant_id='tenant_cas' AND run_id=$2::uuid",
    )
    .bind(format!(
        "myelin://tenant_cas/ci/snapshot/blake3:{}",
        "f".repeat(64)
    ))
    .bind(run6)
    .execute(&admin)
    .await
    .unwrap();
    assert!(starter(&app, "tenant_cas", blobs.clone())
        .run_once()
        .await
        .is_err());
    assert_atomic_started(&admin, "tenant_cas", run6, false, false).await;

    // Genuine app-role RLS plus the exact tenant predicate: an A starter cannot discover or start B.
    let run_b = "10000000-0000-0000-0000-0000000000bb";
    let wf_b = "20000000-0000-0000-0000-0000000000bb";
    insert_run(&admin, blobs.as_ref(), "tenant_b", run_b, wf_b).await;
    let isolated = starter(&app, "tenant_empty", blobs.clone());
    assert_eq!(isolated.run_once().await.unwrap(), StartQueuedOutcome::Idle);
    assert_atomic_started(&admin, "tenant_b", run_b, false, false).await;

    // Region is an independent residency boundary, not merely part of tenant RLS. A fr-par starter
    // cannot claim the same tenant's de-fra row.
    let run_region = "10000000-0000-0000-0000-0000000000cc";
    let wf_region = "20000000-0000-0000-0000-0000000000cc";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_region",
        run_region,
        wf_region,
    )
    .await;
    sqlx::query(
        "UPDATE ci_run SET region='de-fra' \
         WHERE tenant_id='tenant_region' AND run_id=$1::uuid",
    )
    .bind(run_region)
    .execute(&admin)
    .await
    .unwrap();
    assert_eq!(
        starter(&app, "tenant_region", blobs.clone())
            .run_once()
            .await
            .unwrap(),
        StartQueuedOutcome::Idle
    );
    assert_atomic_started(&admin, "tenant_region", run_region, false, false).await;

    // Fresh intake cannot silently bind an older pinned body when a newer active definition is the
    // deterministic registry selection. The queued row remains untouched and no workflow appears.
    let register_v2 = flow_executor(&admin, "tenant_fresh_old_pin");
    tokio::task::block_in_place(|| {
        register_v2
            .register_definition(CI_PIPELINE_WF_TYPE, 2, BODY_HASH_V2)
            .expect("register newer active workflow definition");
    });
    let run_old_pin = "12000000-0000-0000-0000-000000000007";
    let wf_old_pin = "22000000-0000-0000-0000-000000000007";
    insert_run(
        &admin,
        blobs.as_ref(),
        "tenant_fresh_old_pin",
        run_old_pin,
        wf_old_pin,
    )
    .await;
    let error = starter_with(
        &app,
        "tenant_fresh_old_pin",
        blobs.clone(),
        CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
    )
    .run_once()
    .await
    .expect_err("fresh v1 pin must not bypass active v2 selection");
    assert!(error
        .to_string()
        .contains("active workflow selection does not equal pinned version 1"));
    assert_atomic_started(&admin, "tenant_fresh_old_pin", run_old_pin, false, false).await;
    })
    .await;
}

/// A queued PR run superseded by a newer generation is normally cancelled cleanly, exactly like the
/// `delayed_run` scenario above: nothing has attached launch, workflow, or accounting authority to it,
/// so `cancel_stale_queued_on_conn` finds it untouched and closes it out. Here a `ci-reserve:v2:`-shaped
/// `cost_reservation` row is attached to the stale run before the starter observes it. That must trip
/// the same corruption guard the zero-attachment case never reaches: `cancel_stale_queued_on_conn`
/// refuses the cancellation with `CorruptState` (surfaced through `run_once()` as
/// `PgCiStarterError::Supersession`) instead of returning the clean `StartQueuedOutcome::Superseded`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_v2_reservation_prevents_stale_queued_cancellation() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    let schema = format!("ci_pg_starter_v2res_{}", std::process::id());
    let bare = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .expect("connect schema setup");
    bare.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop stale schema");
    bare.execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let cleanup_bare = bare.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_bare, &schema_for_cleanup, move || async move {
        let admin = pool_on(&admin_url(), &schema).await;
        PgMigrator::apply(&admin, &foundation_migrations())
            .await
            .expect("foundation migrations");
        PgMigrator::apply(&admin, &reserve_settle_durable_migrations())
            .await
            .expect("durable reservation migrations");
        PgMigrator::apply_validated(
            &admin,
            &flow_migrations(),
            &HotTables::declare(["workflow_run"]),
        )
        .await
        .expect("flow migrations");
        admin.execute(CREATE_CI_RUN_DDL).await.expect("ci_run DDL");
        admin
            .execute(ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL)
            .await
            .expect("ci_run causal provenance migration");
        admin
            .execute(ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL)
            .await
            .expect("ci_run concurrency identity migration");
        admin
            .execute(ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL)
            .await
            .expect("ci_run PR ordering authority migration");
        sqlx::raw_sql(CREATE_CI_DRIVE_MANIFEST_DDL)
            .execute(&admin)
            .await
            .expect("ci_drive_manifest DDL");
        admin.execute(CREATE_CI_JOB_DDL).await.expect("ci_job DDL");
        admin
            .execute(CREATE_JOB_QUEUE_DDL)
            .await
            .expect("job_queue DDL");
        admin
            .execute("CREATE UNIQUE INDEX jq_idem ON job_queue (tenant_id, idem_token)")
            .await
            .expect("job_queue dispatch idempotency index");
        for ddl in [
            ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
            ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL,
            ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
            ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL,
        ] {
            admin
                .execute(ddl)
                .await
                .expect("job_queue launch-authority migration");
        }
        admin
            .execute(CREATE_CI_JOB_SPEC_DDL)
            .await
            .expect("ci_job_spec DDL");
        admin
            .execute(ALTER_CI_JOB_SPEC_ADD_STAGE_DDL)
            .await
            .expect("ci_job_spec stage migration");
        admin
            .execute(CREATE_CI_COST_EVENT_DDL)
            .await
            .expect("ci_cost_event DDL");
        sqlx::raw_sql(CREATE_CI_JOB_ACCOUNTING_DDL)
            .execute(&admin)
            .await
            .expect("ci_job_accounting DDL");
        admin
            .execute(ALTER_CI_JOB_ACCOUNTING_ADD_SKIPPED_DDL)
            .await
            .expect("ci_job_accounting skipped migration");
        admin
            .execute(CREATE_CHECK_ATTEMPT_DDL)
            .await
            .expect("check_attempt DDL");
        sqlx::raw_sql(CREATE_CI_RUN_CHECK_ATTEMPT_DDL)
            .execute(&admin)
            .await
            .expect("ci_run_check_attempt DDL");
        admin
            .execute(CREATE_CI_JOB_RUN_LEDGER_INDEX_DDL)
            .await
            .expect("ci_job run-ledger concurrent index DDL");
        admin
            .execute("SELECT myelin_make_tenant_scoped('ci_run')")
            .await
            .expect("force RLS on ci_run");
        admin
            .execute("SELECT myelin_make_tenant_scoped('ci_drive_manifest')")
            .await
            .expect("force RLS on ci_drive_manifest");
        admin
            .execute("SELECT myelin_make_tenant_scoped('ci_job')")
            .await
            .expect("force RLS on ci_job");
        admin
            .execute("SELECT myelin_make_tenant_scoped('check_attempt')")
            .await
            .expect("force RLS on check_attempt");
        admin
            .execute("SELECT myelin_make_tenant_scoped('ci_run_check_attempt')")
            .await
            .expect("force RLS on ci_run_check_attempt");
        for table in [
            "job_queue",
            "ci_job_spec",
            "ci_cost_event",
            "ci_job_accounting",
        ] {
            admin
                .execute(format!("SELECT myelin_make_tenant_scoped('{table}')").as_str())
                .await
                .expect("force RLS on supersession table");
        }
        admin
            .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
            .await
            .expect("grant schema");
        admin
            .execute(format!("GRANT ALL ON ALL TABLES IN SCHEMA {schema} TO myelin_app").as_str())
            .await
            .expect("grant tables");
        admin
            .execute("REVOKE UPDATE, DELETE ON ci_drive_manifest FROM myelin_app")
            .await
            .expect("manifest remains insert-only after broad test setup grant");
        let app = pool_on(&app_url(), &schema).await;
        let blobs = Arc::new(FsBlobStore::new());
        let mut ledger_config = MyelinConfig::dev();
        ledger_config.database_url = admin_url();
        ledger_config.region = "fr-par".into();
        let supersession_ledger =
            DurableCostLedger::new(SubstrateProvider::connect(ledger_config, 1).await.unwrap());

        let register = flow_executor(&admin, "tenant_a");
        tokio::task::block_in_place(|| {
            register
                .register_definition(CI_PIPELINE_WF_TYPE, 1, BODY_HASH)
                .expect("register immutable workflow definition");
        });

        // The same PR-supersession fixture as `delayed_run` above: a queued generation-1 run becomes
        // stale once a newer generation-2 head is running.
        let pr_tenant = "tenant_pr_v2_reservation_guard";
        let pr_group = "pr:core:99";
        let old_run = "10000000-0000-0000-0000-000000000901";
        let old_wf = "20000000-0000-0000-0000-000000000901";
        let new_run = "10000000-0000-0000-0000-000000000902";
        let new_wf = "20000000-0000-0000-0000-000000000902";
        let pr_factory = ci_run_starter_factory(
            app.clone(),
            Region("fr-par".into()),
            blobs.clone(),
            tokio::runtime::Handle::current(),
            supersession_ledger.clone(),
        )
        .unwrap();
        let pr_starter = pr_factory
            .starter_for(
                TenantId(pr_tenant.into()),
                CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap(),
            )
            .unwrap();
        insert_pr_run(
            &admin,
            blobs.as_ref(),
            pr_tenant,
            old_run,
            old_wf,
            pr_group,
            Some(1),
            true,
        )
        .await;
        assert!(matches!(
            pr_starter.run_once().await.unwrap(),
            StartQueuedOutcome::Started { run_id, .. } if run_id == old_run
        ));
        insert_pr_run(
            &admin,
            blobs.as_ref(),
            pr_tenant,
            new_run,
            new_wf,
            pr_group,
            Some(2),
            true,
        )
        .await;
        assert!(matches!(
            pr_starter.run_once().await.unwrap(),
            StartQueuedOutcome::Started { run_id, .. } if run_id == new_run
        ));

        // A stale generation-1 arrival, same shape as `delayed_run`, would normally be cancelled cleanly
        // because nothing has attached launch/workflow/accounting authority to it. Attach a v2 operational
        // reservation to it before the starter observes it: `cancel_stale_queued_on_conn`'s "nothing
        // attached" guard must now refuse the cancellation instead of closing the run out.
        let stale_run = "10000000-0000-0000-0000-000000000900";
        let stale_wf = "20000000-0000-0000-0000-000000000900";
        insert_pr_run(
            &admin,
            blobs.as_ref(),
            pr_tenant,
            stale_run,
            stale_wf,
            pr_group,
            Some(1),
            false,
        )
        .await;
        sqlx::query(
            "INSERT INTO cost_reservation (tenant_id, region, run_id, reserved, state) \
         VALUES ($1, 'fr-par', $2, 1, 'reserved')",
        )
        .bind(pr_tenant)
        .bind(format!(
            "ci-reserve:v2:{stale_run}:budget-v1:a1:batch:job:{}",
            "d".repeat(64)
        ))
        .execute(&admin)
        .await
        .expect("attach a v2 operational reservation to the stale queued run");
        let error = pr_starter
            .run_once()
            .await
            .expect_err("an attached v2 reservation must block stale-queued cancellation");
        assert!(
            matches!(
                error,
                PgCiStarterError::Supersession(CiRunSupersessionError::CorruptState(_))
            ),
            "unexpected error shape: {error:?}"
        );
        let stale_state: String =
            sqlx::query_scalar("SELECT state FROM ci_run WHERE tenant_id=$1 AND run_id=$2::uuid")
                .bind(pr_tenant)
                .bind(stale_run)
                .fetch_one(&admin)
                .await
                .unwrap();
        assert_eq!(
            stale_state, "queued",
            "the refused cancellation must leave the stale run's lifecycle untouched"
        );
        let reservation_still_present: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM cost_reservation \
          WHERE tenant_id=$1 AND run_id LIKE ('ci-reserve:v2:' || $2 || ':%'))",
        )
        .bind(pr_tenant)
        .bind(stale_run)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(
            reservation_still_present,
            "the refused cancellation must not have touched the attached reservation"
        );
    })
    .await;
}
