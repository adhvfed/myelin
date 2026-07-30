#![cfg(feature = "integration")]

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::with_schema_cleanup;
use myelin_ci_controlplane::{
    ci_artifact_ref, ci_controlplane_hot_tables, ci_controlplane_migrations, ci_job_queue_store,
    ci_job_spec_store, ci_manifest_pipeline_definition, ci_production_runtime_factory_test_support,
    ci_region_queue_store_test_support, ci_region_run_discovery_test_support, ci_run_ref,
    ci_run_store_factory, ci_runner_hooks, ci_runner_identity_authorities, durable_spec_resolver,
    CiDriveManifestStore, CiDriveManifestV1, CiJobAccountingPricer, CiJobAccountingStore,
    CiJobAccountingWriteVersion, CiJobPricingError, CiJobRuntimeAuthorityRequest,
    CiJobTokenRequest, CiManifestLaneV1, CiManifestLimitsV1, CiManifestSchedulingV1,
    CiManifestTrustTierV1, CiManifestWorkspaceV1, CiPipelineReporter, CiRunFinalization,
    CiRunFinalizationJob, CiRunFinalizationWrite, CiRunFinalizer, CiRunInsert, CiRunStoreError,
    CiRunTerminalState, DurableCiJobAccounting, DurableCiRunFinalizer, DurableEnqueue,
    DurableLeaseAdapter, GrantedCiJobV1, JobQueueReaper, Lane, ManifestBoundCiJobTokenAuthority,
    PgCiRunSupersession, PricedCiJobUsage, CI_MANIFEST_PIPELINE_VERSION, CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
    LINUX_SMALL_V1_RUNNER_LABELS, TIER_P_OPERATIONAL_PRICING_REVISION,
};
use myelin_ci_sandbox::asset_registry::GvisorAssetRegistry;
use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, resolved_gvisor_rootfs, CompletionClaim,
    CompletionSettlementOwner, CountingFirehose, ImageRef, JobKind, PreparationPhase,
    PreparationTerminalDisposition, ResourceUsage, RetryableAttemptCause, RetryableAttemptFailure,
    RetryableAttemptOutcome, RunnerAgent, TerminalReport, TerminalReporter, TrustTier,
    WorkspaceSpec, LINUX_SMALL_V1_ROOTFS_SHA256,
};
use myelin_config::MyelinConfig;
use myelin_events::{IdMinter, MonotonicMinter};
use myelin_flow::{
    migrations::migrations as flow_migrations, partition_for_run_id, DurableExecutor, MinorUnits,
    PgFlowExecutor, RunId, SignalOutcome, StartSpec, CI_PIPELINE_WF_TYPE,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{
    cell_root_durable_migrations, identity_durable_migrations, provider::foundation_migrations,
    reserve_settle_durable_migrations, DurableCostLedger, HotTables, PgMigrator, SealKey,
    SubstrateProvider, TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool, Row};

const OPERATIONAL_RESERVE_HANDLE: &str =
    "ci-reserve:v1:22222222-2222-8222-8222-222222222222:batch:33333333-3333-8333-8333-333333333333:item";
static MIGRATION_SCENARIO_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The real, already-founder-pipeline-pinned `linux-small-v1` image. CT-007 gate 2/4 made
/// `spec.image` the real launch authority: only the "build" job (`/bin/false`) in this file's
/// manifest ever actually reaches `GvisorBackend::launch` (the "package"/`skipped_job` never
/// launches — its own name says so), so only that job's image must be genuinely verifiable.
fn linux_small_v1_image() -> ImageRef {
    ImageRef::pinned(format!(
        "myelin.local/linux-small-v1-rootfs@sha256:{LINUX_SMALL_V1_ROOTFS_SHA256}"
    ))
    .unwrap()
}

fn test_registry() -> std::sync::Arc<GvisorAssetRegistry> {
    std::sync::Arc::new(
        GvisorAssetRegistry::from_bindings(vec![
            myelin_ci_sandbox::asset_registry::RootfsAssetBinding {
                image: linux_small_v1_image(),
                rootfs: resolved_gvisor_rootfs(),
            },
        ])
        .expect("the base linux-small-v1 rootfs binding verifies"),
    )
}

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn scoped_url(url: &str, schema: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}options=-csearch_path%3D{schema}%2Cpublic")
}

async fn isolated_pool(schema: &str) -> PgPool {
    let schema = schema.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                connection
                    .execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&admin_url())
        .await
        .expect("connect to live development PostgreSQL")
}

#[derive(Clone)]
struct TestPricer {
    valid: bool,
}

impl CiJobAccountingPricer for TestPricer {
    fn price(&self, usage: ResourceUsage) -> Result<PricedCiJobUsage, CiJobPricingError> {
        let memory_gb_seconds = usage.mem_byte_seconds.div_ceil(1_073_741_824);
        Ok(PricedCiJobUsage {
            pricing_revision: TIER_P_OPERATIONAL_PRICING_REVISION.into(),
            memory_gb_seconds,
            cpu_wholesale: MinorUnits(usage.cpu_seconds),
            cpu_markup: MinorUnits::ZERO,
            memory_wholesale: MinorUnits(memory_gb_seconds + u64::from(!self.valid)),
            memory_markup: MinorUnits::ZERO,
        })
    }
}

fn manifest(
    tenant: &str,
    region: &str,
    wf_run: &str,
    ci_run: &str,
    job: &str,
    skipped_job: &str,
    workflow_code_hash: &str,
) -> CiDriveManifestV1 {
    let digest = |byte: char| format!("blake3:{}", byte.to_string().repeat(64));
    let mut manifest = CiDriveManifestV1 {
        schema_version: 1,
        tenant_id: tenant.into(),
        region: region.into(),
        wf_run_id: wf_run.into(),
        ci_run_id: ci_run.into(),
        source_snapshot_ref: format!("myelin://{tenant}/ci/artifact/snapshot-{}", digest('a')),
        source_plan_schema_version: 2,
        launch_request_digest: digest('b'),
        workflow_type: CI_PIPELINE_WF_TYPE.into(),
        workflow_definition_version: CI_MANIFEST_PIPELINE_VERSION,
        workflow_code_hash: workflow_code_hash.into(),
        authority_policy_revision: "ci-policy:2026-07-21".into(),
        repo_ref: format!("myelin://{tenant}/git/repo/core"),
        commit_oid: "deadbeef00deadbeef00deadbeef00deadbeef00".into(),
        run_ref: format!("myelin://{tenant}/ci/run/{ci_run}"),
        started_at: "2026-07-21T12:34:56.000000Z".into(),
        trust_tier: CiManifestTrustTierV1::Trusted,
        check_attempts: BTreeMap::from([("build".into(), 1), ("package".into(), 1)]),
        merge_waiter: None,
        jobs: vec![
            GrantedCiJobV1 {
                job_id: job.into(),
                stage: "build".into(),
                name: "build".into(),
                check_context: "build".into(),
                needs: Vec::new(),
                matrix_key: BTreeMap::new(),
                image: linux_small_v1_image().reference,
                command: vec!["/bin/false".into()],
                env: BTreeMap::new(),
                secret_handles: BTreeMap::new(),
                egress_allow: Vec::new(),
                limits: CiManifestLimitsV1 {
                    cpu_millis: 1_000,
                    mem_bytes: 1_073_741_824,
                    disk_bytes: 1_073_741_824,
                    pids_max: 64,
                    timeout_secs: 60,
                },
                workspace: CiManifestWorkspaceV1 {
                    repo_ref: format!("myelin://{tenant}/git/repo/core"),
                    commit_oid: "deadbeef00deadbeef00deadbeef00deadbeef00".into(),
                    read_only_root: true,
                    tmpfs_scratch: true,
                },
                scheduling: CiManifestSchedulingV1 {
                    lane: CiManifestLaneV1::Interactive,
                    labels: vec!["linux".into()],
                    concurrency_group: None,
                    fair_key: tenant.into(),
                },
                reserve_handle: OPERATIONAL_RESERVE_HANDLE.into(),
                token_authority_handle: "token-authority:live".into(),
                continue_on_error: false,
            },
            GrantedCiJobV1 {
                job_id: skipped_job.into(),
                stage: "package".into(),
                name: "package".into(),
                check_context: "package".into(),
                needs: vec![job.into()],
                matrix_key: BTreeMap::new(),
                image: format!("registry.example/package@sha256:{}", "e".repeat(64)),
                command: vec!["/bin/package".into()],
                env: BTreeMap::new(),
                secret_handles: BTreeMap::new(),
                egress_allow: Vec::new(),
                limits: CiManifestLimitsV1 {
                    cpu_millis: 1_000,
                    mem_bytes: 1_073_741_824,
                    disk_bytes: 1_073_741_824,
                    pids_max: 64,
                    timeout_secs: 60,
                },
                workspace: CiManifestWorkspaceV1 {
                    repo_ref: format!("myelin://{tenant}/git/repo/core"),
                    commit_oid: "deadbeef00deadbeef00deadbeef00deadbeef00".into(),
                    read_only_root: true,
                    tmpfs_scratch: true,
                },
                scheduling: CiManifestSchedulingV1 {
                    lane: CiManifestLaneV1::Interactive,
                    labels: vec!["linux".into()],
                    concurrency_group: None,
                    fair_key: tenant.into(),
                },
                reserve_handle: "reserve:skipped-live".into(),
                token_authority_handle: "token-authority:skipped".into(),
                continue_on_error: false,
            },
        ],
    };
    let executable = &manifest.jobs[0];
    manifest.jobs[0].token_authority_handle =
        ManifestBoundCiJobTokenAuthority::handle_for(&CiJobRuntimeAuthorityRequest {
            tenant_id: tenant.into(),
            region: region.into(),
            ci_run_id: ci_run.into(),
            wf_run_id: wf_run.into(),
            project_id: "55555555-5555-8555-8555-555555555555".into(),
            job_id: executable.job_id.clone(),
            stage: executable.stage.clone(),
            concrete_name: executable.name.clone(),
            trigger_kind: "push".into(),
            trust_tier: "trusted".into(),
            source_snapshot_digest: digest('a'),
            workflow_definition_version: CI_MANIFEST_PIPELINE_VERSION,
            workflow_code_hash: workflow_code_hash.into(),
            policy_revision: "ci-policy:2026-07-21".into(),
            limits: executable.limits.clone(),
            checkout: derive_checkout_authorization_scope(
                JobKind::Ci,
                &WorkspaceSpec {
                    repo_ref: Some(executable.workspace.repo_ref.clone()),
                    commit: Some(executable.workspace.commit_oid.clone()),
                },
            )
            .unwrap(),
        });
    manifest
}

async fn counts(pool: &PgPool, job: &str, wf_run: &str) -> (i64, i64, i64, String) {
    let accounting: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ci_job_accounting WHERE job_id = $1::uuid")
            .bind(job)
            .fetch_one(pool)
            .await
            .unwrap();
    let projection: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ci_cost_event WHERE job_id = $1::uuid")
            .bind(job)
            .fetch_one(pool)
            .await
            .unwrap();
    let signals: i64 = sqlx::query_scalar("SELECT count(*) FROM wf_signal WHERE run_id = $1")
        .bind(wf_run)
        .fetch_one(pool)
        .await
        .unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1::uuid")
        .bind(job)
        .fetch_one(pool)
        .await
        .unwrap();
    (accounting, projection, signals, state)
}

async fn drive_clock(pool: &PgPool) -> (i64, String) {
    sqlx::query_as(
        "SELECT extract(epoch FROM instant)::bigint,
                to_char(instant AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')
         FROM (SELECT clock_timestamp() AS instant) clock",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

const JOURNALED_PRELAUNCH_USAGE: ResourceUsage = ResourceUsage {
    cpu_seconds: 8,
    mem_byte_seconds: 5 * 1_073_741_824,
};

fn expected_operational_settlement(usage: ResourceUsage) -> (i64, i64) {
    let priced = usage
        .cpu_seconds
        .checked_add(usage.mem_byte_seconds.div_ceil(1_073_741_824))
        .unwrap();
    let billed = i64::try_from(priced.min(100)).unwrap();
    (billed, 100 - billed)
}

async fn seed_prelaunch_usage_mix(pool: &PgPool, seed: JournalSeed<'_>) {
    sqlx::query(
        "INSERT INTO ci_job_parent_attempt (
           tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle, lease_owner,
           lease_epoch, claim_nonce, claim_started_at_epoch_secs, claim_expires_at_epoch_secs,
           budget_revision, max_parent_attempts
         )
         SELECT tenant_id, region, job_id, run_id, $5::uuid, $6, lease_owner, lease_epoch,
                claim_nonce, EXTRACT(EPOCH FROM claim_started_at)::bigint,
                EXTRACT(EPOCH FROM claim_expires_at)::bigint, 1, 5
         FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid AND run_id = $4::uuid
         ON CONFLICT DO NOTHING",
    )
    .bind(seed.tenant)
    .bind(seed.region)
    .bind(seed.job_id)
    .bind(seed.wf_run_id)
    .bind(seed.ci_run_id)
    .bind(seed.reserve_handle)
    .execute(pool)
    .await
    .expect("seed the exact durable parent attempt");
    if !seed.include_phases {
        return;
    }
    sqlx::query(
        "INSERT INTO ci_job_prelaunch_usage (
           tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status,
           ceiling_cpu_seconds, ceiling_mem_byte_seconds, exact_cpu_seconds,
           exact_mem_byte_seconds, started_at, resolved_at, seal_after
         )
         SELECT tenant_id, region, job_id, lease_epoch, claim_nonce,
                'checkout_transport', 'measured', 120, 120000000000, 3, $4::numeric,
                statement_timestamp(), statement_timestamp(), statement_timestamp() + interval '1 hour'
         FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(seed.tenant)
    .bind(seed.region)
    .bind(seed.job_id)
    .bind((2 * 1_073_741_824_u64).to_string())
    .execute(pool)
    .await
    .expect("seed measured checkout transport usage");
    sqlx::query(
        "INSERT INTO ci_job_prelaunch_usage (
           tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status,
           ceiling_cpu_seconds, ceiling_mem_byte_seconds, exact_cpu_seconds,
           exact_mem_byte_seconds, started_at, resolved_at, seal_after
         )
         SELECT tenant_id, region, job_id, lease_epoch, claim_nonce,
                'checkout_materialization', 'sealed_ceiling', 5, $4::numeric, NULL, NULL,
                statement_timestamp(), statement_timestamp(), statement_timestamp() + interval '1 hour'
         FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(seed.tenant)
    .bind(seed.region)
    .bind(seed.job_id)
    .bind((3 * 1_073_741_824_u64).to_string())
    .execute(pool)
    .await
    .expect("seed sealed checkout materialization usage");
}

struct JournalSeed<'a> {
    tenant: &'a str,
    region: &'a str,
    job_id: &'a str,
    wf_run_id: &'a str,
    ci_run_id: &'a str,
    reserve_handle: &'a str,
    include_phases: bool,
}

async fn run_reporter_scenario(
    supersession_wins_first: bool,
    runner_abandoned: bool,
    retry_then_supersession: bool,
    journal_prelaunch_usage: bool,
    verify_existing_via_supersession: bool,
    preparation_scenario: Option<&'static str>,
) {
    let suffix = if let Some(preparation_scenario) = preparation_scenario {
        preparation_scenario
    } else if runner_abandoned {
        "cancel_abandoned"
    } else if supersession_wins_first {
        "cancel_reported"
    } else if retry_then_supersession {
        "retry_cancelled"
    } else {
        "retry_first"
    };
    let schema = format!("ci_accounting_{}_{}", std::process::id(), suffix);
    let bootstrap = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .expect("connect to create isolated schema");
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    // A cleanup-dedicated clone of `bootstrap` (a cheap `Arc` handle clone, same underlying pool):
    // `with_schema_cleanup` unconditionally drops `schema` through it once this scenario's body
    // (success, an early return, an assertion failure, or a panic) finishes, so the schema never
    // outlives this call regardless of outcome. The body below still runs its own explicit
    // `bootstrap.execute("DROP SCHEMA ...")` at each success/early-return exit exactly as before —
    // those become harmless no-ops (`IF EXISTS`) once this wrapper's own drop has already run, or
    // simply drop it first on the ordinary paths; only a PANIC before reaching one of them now also
    // gets cleaned up, which previously leaked the schema.
    let cleanup_bootstrap = bootstrap.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(
        &cleanup_bootstrap,
        &schema_for_cleanup,
        move || async move {
    let pool = isolated_pool(&schema).await;
    PgMigrator::apply(&pool, &foundation_migrations())
        .await
        .unwrap();
    PgMigrator::apply(&pool, &identity_durable_migrations())
        .await
        .unwrap();
    PgMigrator::apply(&pool, &cell_root_durable_migrations())
        .await
        .unwrap();
    PgMigrator::apply_validated(
        &pool,
        &flow_migrations(),
        &HotTables::declare(["workflow_run"]),
    )
    .await
    .unwrap();
    PgMigrator::apply(&pool, &reserve_settle_durable_migrations())
        .await
        .unwrap();
    // This drill uses the admin-backed test scheduler adapter, not the dedicated production role.
    // The shared helper holds CI_PRIVILEGE_FIXTURE_LOCK across the apply and then strips EVERY
    // scheduler grant from this schema — replacing a hand-listed REVOKE that had silently fallen
    // behind the migration set (it never covered ci_job_parent_attempt, ci_job_prelaunch_usage,
    // ci_job, or workflow_run.wf_version).
    common::with_fixture_migration_lock(&admin_url(), &pool, &schema, || async {
        PgMigrator::apply_validated(
            &pool,
            &ci_controlplane_migrations(),
            &ci_controlplane_hot_tables(),
        )
        .await
        .unwrap();
    })
    .await;

    let tenant = TenantId::from_token("accounting-tenant");
    let region = Region::new("fr-par");
    let wf_run = "11111111-1111-8111-8111-111111111111";
    let ci_run = "22222222-2222-8222-8222-222222222222";
    let job = "33333333-3333-8333-8333-333333333333";
    let skipped_job = "77777777-7777-8777-8777-777777777777";
    let owner = "runner-live";
            let journal_prelaunch_usage = journal_prelaunch_usage || preparation_scenario.is_some();
            let reserve_handle =
                if journal_prelaunch_usage && preparation_scenario != Some("preparation_v1") {
        format!("ci-reserve:v2:{ci_run}:budget-v1:a5:batch:{job}:usage")
    } else {
        OPERATIONAL_RESERVE_HANDLE.to_owned()
    };
    let journal_usage = if journal_prelaunch_usage {
        JOURNALED_PRELAUNCH_USAGE
    } else {
        ResourceUsage {
            cpu_seconds: 0,
            mem_byte_seconds: 0,
        }
    };

    let ci_runs = ci_run_store_factory(pool.clone());
    ci_runs
        .insert_ci_run(&CiRunInsert {
            tenant_id: tenant.0.clone(),
            region: region.0.clone(),
            run_id: ci_run.into(),
            project_id: "55555555-5555-8555-8555-555555555555".into(),
            pipeline_id: "66666666-6666-8666-8666-666666666666".into(),
            wf_run_id: wf_run.into(),
            definition_snapshot: format!("blake3:{}", "a".repeat(64)),
            trigger_kind: "push".into(),
            concurrency_group: None,
            pr_head_generation: None,
            trust_tier: "trusted".into(),
            state: "queued".into(),
            correlation_id: "accounting-live".into(),
            cause_event_id: Some("trigger-accounting-live".into()),
            cause_depth: 0,
            caused_by: None,
            repo_ref: Some(format!("myelin://{}/git/repo/core", tenant.0)),
            commit_oid: Some("deadbeef00deadbeef00deadbeef00deadbeef00".into()),
            triggered_by: None,
        })
        .await
        .unwrap();
            sqlx::query(
                "UPDATE ci_run SET state = 'running' WHERE tenant_id = $1 AND run_id = $2::uuid",
            )
        .bind(&tenant.0)
        .bind(ci_run)
        .execute(&pool)
        .await
        .unwrap();
    let manifest_store =
        CiDriveManifestStore::new(pool.clone(), tenant.clone(), region.clone()).unwrap();
    let production_definition = ci_manifest_pipeline_definition();
    let mut drive_manifest = manifest(
        &tenant.0,
        &region.0,
        wf_run,
        ci_run,
        job,
        skipped_job,
        production_definition.code_hash(),
    );
    drive_manifest.jobs[0].reserve_handle = reserve_handle.clone();
            if supersession_wins_first
                || retry_then_supersession
                || verify_existing_via_supersession
            {
        drive_manifest.jobs.truncate(1);
        drive_manifest.check_attempts.remove("package");
    }
    let manifest_digest = manifest_store.insert(&drive_manifest).await.unwrap();
    for manifest_job in &drive_manifest.jobs {
        sqlx::query(
            "INSERT INTO ci_job \
             (tenant_id,region,job_id,run_id,stage,name,needs,spec_ref,state,attempt) \
             VALUES ($1,$2,$3::uuid,$4::uuid,$5,$6,'{}'::uuid[],$7,'queued',1)",
        )
        .bind(&tenant.0)
        .bind(&region.0)
        .bind(&manifest_job.job_id)
        .bind(ci_run)
        .bind(&manifest_job.stage)
        .bind(&manifest_job.name)
        .bind(format!(
            "myelin://{}/ci/job-spec/{}",
            tenant.0, manifest_job.job_id
        ))
        .execute(&pool)
        .await
        .expect("seed the starter-owned CI job surface used by the production reporter");
    }

    sqlx::query(
        "INSERT INTO cost_reservation (tenant_id, region, run_id, reserved, state)
         VALUES ($1, $2, $3, 100, 'inflight')",
    )
    .bind(&tenant.0)
    .bind(&region.0)
    .bind(&reserve_handle)
    .execute(&pool)
    .await
    .unwrap();
            if !supersession_wins_first
                && !retry_then_supersession
                && !verify_existing_via_supersession
            {
        sqlx::query(
            "INSERT INTO cost_reservation (tenant_id, region, run_id, reserved, state)
             VALUES ($1, $2, 'reserve:skipped-live', 40, 'reserved')",
        )
        .bind(&tenant.0)
        .bind(&region.0)
        .execute(&pool)
        .await
        .unwrap();
    }

    let principal = Principal::new(
        tenant.clone(),
        region.clone(),
        PrincipalId("accounting-reporter".into()),
        PrincipalKind::Service,
        DataRole::Processor,
        PrincipalStatus::Active,
    );
    let scope = TenantScope::from_verified_token(&principal, region.clone());
    let mut config = MyelinConfig::dev();
    config.database_url = scoped_url(&admin_url(), &schema);
    config.region = region.0.clone();
    let provider = SubstrateProvider::connect(config, 4).await.unwrap();
    let ledger = DurableCostLedger::new(provider.clone());
    let production_runtime = ci_production_runtime_factory_test_support(
        pool.clone(),
        region.clone(),
        ledger.clone(),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    let _definition_registration = production_runtime
        .worker_for(
            tenant.clone(),
            partition_for_run_id(wf_run),
            "ci-flow-definition-proof",
        )
        .unwrap();
    let mut production_poller = production_runtime
        .workflow_poller(
            ci_region_run_discovery_test_support(pool.clone()),
            "ci-flow-accounting-proof",
        )
        .unwrap();
    let production_reporter = production_runtime.reporter_router().unwrap();
    let registered_code_hash: String = sqlx::query_scalar(
        "SELECT code_hash FROM wf_definition WHERE wf_type = $1 AND version = $2",
    )
    .bind(CI_PIPELINE_WF_TYPE)
    .bind(production_runtime.definition().version())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        registered_code_hash,
        production_runtime.definition().code_hash()
    );
    assert_eq!(
        production_reporter.completion_settlement_owner(),
        CompletionSettlementOwner::TerminalReporter
    );

    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let pg_executor = PgFlowExecutor::new(
        pool.clone(),
        tokio::runtime::Handle::current(),
        minter,
        tenant.clone(),
        region.clone(),
    );
    tokio::task::block_in_place(|| {
        pg_executor
            .start_with_id(
                StartSpec {
                    wf_type: CI_PIPELINE_WF_TYPE.into(),
                    input: vec![
                                ci_artifact_ref(
                                    &tenant.0,
                                    &format!("drive-manifest-{manifest_digest}"),
                                ),
                        ci_run_ref(&tenant.0, ci_run),
                    ],
                    budget: None,
                    idem_key: "accounting-live".into(),
                },
                Some(RunId(wf_run.into())),
            )
            .unwrap();
    });
    let (first_secs, first_stamp) = drive_clock(&pool).await;
    let first_drive = production_poller
        .run_once(8, 8, first_secs, &first_stamp)
        .await
        .unwrap();
    assert_eq!(first_drive.scopes, 1);
    assert_eq!(first_drive.driven, 1);
    let (idem, queued_state): (String, String) = sqlx::query_as(
        "SELECT idem_token, state FROM job_queue
         WHERE tenant_id = $1 AND job_id = $2::uuid",
    )
    .bind(&tenant.0)
    .bind(job)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(queued_state, "queued");
    let region_store = ci_region_queue_store_test_support(pool.clone());
    let identity = ci_runner_identity_authorities(
        provider.clone(),
        "ci-accounting-live-cell",
        &SealKey::from_bytes([0x4c; 32]),
        tokio::runtime::Handle::current(),
    )
    .await
    .expect("compose the production claim-time Identity authorities");
    let resolver = durable_spec_resolver(
        ci_job_spec_store(pool.clone()),
        region.0.clone(),
        tokio::runtime::Handle::current(),
        identity.token_issuer().clone(),
    );
    let runner_labels: Vec<String> = LINUX_SMALL_V1_RUNNER_LABELS
        .iter()
        .map(|label| (*label).to_owned())
        .collect();
    let first_lease = region_store
        .claim(
            &region.0,
            &runner_labels,
            &[TrustTier::Trusted],
            owner,
            CI_RUNNER_EXECUTION_LEASE_TTL_SECS as u64,
        )
        .await
        .unwrap()
        .expect("the first production generation claims the queued manifest job");
    assert_eq!(first_lease.lease_epoch, 1);
            if journal_prelaunch_usage && preparation_scenario.is_none() {
        seed_prelaunch_usage_mix(
            &pool,
            JournalSeed {
                tenant: &tenant.0,
                region: &region.0,
                job_id: job,
                wf_run_id: wf_run,
                ci_run_id: ci_run,
                reserve_handle: &reserve_handle,
                include_phases: true,
            },
        )
        .await;
    }
            let first_spec = resolver(&first_lease)
                .expect("the first production generation mints its Identity token");
    let first_hooks = ci_runner_hooks(
        provider.clone(),
        identity.launch_authorizer(),
        tokio::runtime::Handle::current(),
    );
            assert_eq!(first_hooks.reserve(&first_spec).unwrap().0, reserve_handle);

    let reporter = |valid| {
        CiPipelineReporter::new_accounted(
            pg_executor.clone(),
            ci_job_spec_store(pool.clone()),
            ci_job_queue_store(pool.clone()),
            tokio::runtime::Handle::current(),
            DurableCiJobAccounting::new(
                scope.clone(),
                manifest_store.clone(),
                ledger.clone(),
                        myelin_ci_controlplane::CiCostEventStore::with_pg(
                            pool.clone(),
                            region.clone(),
                        ),
                CiJobAccountingStore::with_pg(pool.clone(), region.clone()),
                Arc::new(TestPricer { valid }),
            ),
        )
    };
            let preparation_reporter = || {
                CiPipelineReporter::new_accounted(
                    pg_executor.clone(),
                    ci_job_spec_store(pool.clone()),
                    ci_job_queue_store(pool.clone()),
                    tokio::runtime::Handle::current(),
                    DurableCiJobAccounting::new(
                        scope.clone(),
                        manifest_store.clone(),
                        ledger.clone(),
                        myelin_ci_controlplane::CiCostEventStore::with_pg(
                            pool.clone(),
                            region.clone(),
                        ),
                        CiJobAccountingStore::with_pg_and_write_version(
                            pool.clone(),
                            region.clone(),
                            CiJobAccountingWriteVersion::V4,
                        ),
                        Arc::new(TestPricer { valid: true }),
                    ),
                )
            };
    let claim = CompletionClaim {
        tenant: tenant.clone(),
        run: RunId(wf_run.into()),
        job_id: job.into(),
        idem_token: idem.clone(),
        lease_owner: owner.into(),
        lease_epoch: first_lease.lease_epoch,
        claim_nonce: first_lease.claim_nonce.clone(),
    };
    let report = TerminalReport {
        passed: false,
        timed_out: false,
        usage: ResourceUsage {
            cpu_seconds: 7,
            mem_byte_seconds: 2 * 1_073_741_824,
        },
        result_refs: Vec::new(),
    };
            if let Some(preparation_scenario) = preparation_scenario {
                let preparation_claim = CiJobTokenRequest {
                    tenant_id: tenant.0.clone(),
                    region: region.0.clone(),
                    wf_run_id: wf_run.into(),
                    ci_run_id: ci_run.into(),
                    job_id: job.into(),
                    token_authority_handle: drive_manifest.jobs[0].token_authority_handle.clone(),
                    idem_token: idem.clone(),
                    lease_owner: owner.into(),
                    lease_epoch: first_lease.lease_epoch,
                    claim_nonce: first_lease.claim_nonce.clone(),
                    claim_started_at_epoch_secs: first_lease.claim_started_at_epoch_secs,
                    claim_expires_at_epoch_secs: first_lease.claim_expires_at_epoch_secs,
                };
                assert!(
                    reporter(true)
                        .report_preparation_terminal(
                            &preparation_claim,
                            PreparationTerminalDisposition::Failed {
                                phase: PreparationPhase::CheckoutTransport,
                            },
                        )
                        .is_err(),
                    "the production-v3 accounting gate refuses preparation writes"
                );
                assert!(
                    preparation_reporter()
                        .report_preparation_terminal(
                            &preparation_claim,
                            PreparationTerminalDisposition::Failed {
                                phase: PreparationPhase::CheckoutTransport,
                            },
                        )
                        .is_err(),
                    "no parent attempt can authorize preparation completion"
                );
                seed_prelaunch_usage_mix(
                    &pool,
                    JournalSeed {
                        tenant: &tenant.0,
                        region: &region.0,
                        job_id: job,
                        wf_run_id: wf_run,
                        ci_run_id: ci_run,
                        reserve_handle: &reserve_handle,
                        include_phases: false,
                    },
                )
                .await;
                let refusal_disposition = PreparationTerminalDisposition::Failed {
                    phase: PreparationPhase::CheckoutTransport,
                };
                assert!(preparation_reporter()
                    .report_preparation_terminal(
                        &preparation_claim,
                        PreparationTerminalDisposition::Failed {
                            phase: PreparationPhase::CheckoutTransport,
                        },
                    )
                    .is_err());
                sqlx::query(
                    "INSERT INTO ci_job_prelaunch_usage (
               tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status,
               ceiling_cpu_seconds, ceiling_mem_byte_seconds, started_at, seal_after
             )
             SELECT tenant_id, region, job_id, lease_epoch, claim_nonce,
                    'checkout_transport', 'started', 120, 120000000000,
                    statement_timestamp(), statement_timestamp() + interval '1 hour'
             FROM job_queue WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
                )
                .bind(&tenant.0)
                .bind(&region.0)
                .bind(job)
                .execute(&pool)
                .await
                .unwrap();
                assert!(preparation_reporter()
                    .report_preparation_terminal(
                        &preparation_claim,
                        PreparationTerminalDisposition::Failed {
                            phase: PreparationPhase::CheckoutTransport,
                        },
                    )
                    .is_err());
                sqlx::query(
                    "UPDATE ci_job_prelaunch_usage
             SET status = 'measured', exact_cpu_seconds = 3,
                 exact_mem_byte_seconds = $4::numeric, resolved_at = statement_timestamp()
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
               AND phase = 'checkout_transport'",
                )
                .bind(&tenant.0)
                .bind(&region.0)
                .bind(job)
                .bind(if preparation_scenario == "preparation_overflow" {
                    (i64::MAX as u64 + 1).to_string()
                } else {
                    (2 * 1_073_741_824_u64).to_string()
                })
                .execute(&pool)
                .await
                .unwrap();
                assert!(preparation_reporter()
                    .report_preparation_terminal(
                        &preparation_claim,
                        PreparationTerminalDisposition::Failed {
                            phase: PreparationPhase::CheckoutMaterialization,
                        },
                    )
                    .is_err());
                assert!(preparation_reporter()
                    .report_preparation_terminal(
                        &preparation_claim,
                        PreparationTerminalDisposition::AttemptsExhausted,
                    )
                    .is_err());
                sqlx::query(
                    "INSERT INTO ci_job_prelaunch_usage (
               tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status,
               ceiling_cpu_seconds, ceiling_mem_byte_seconds, started_at, resolved_at, seal_after
             )
             SELECT tenant_id, region, job_id, lease_epoch, claim_nonce,
                    'checkout_materialization', 'sealed_ceiling', 5, $4::numeric,
                    statement_timestamp(), statement_timestamp(),
                    statement_timestamp() + interval '1 hour'
             FROM job_queue WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
                )
                .bind(&tenant.0)
                .bind(&region.0)
                .bind(job)
                .bind((3 * 1_073_741_824_u64).to_string())
                .execute(&pool)
                .await
                .unwrap();

                if preparation_scenario == "preparation_v1" {
                    assert!(
                        preparation_reporter()
                            .report_preparation_terminal(&preparation_claim, refusal_disposition)
                            .is_err(),
                        "legacy v1 reservations cannot enter capped preparation completion"
                    );
                    assert_eq!(counts(&pool, job, wf_run).await, (0, 0, 0, "leased".into()));
                    drop(pool);
                    bootstrap
                        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
                        .await
                        .unwrap();
                    return;
                }

                if preparation_scenario == "preparation_overflow" {
                    let error = preparation_reporter()
                        .report_preparation_terminal(&preparation_claim, refusal_disposition)
                        .expect_err("usage outside the accounting bigint range must refuse");
                    assert!(error.to_string().contains("usage"));
                    assert_eq!(counts(&pool, job, wf_run).await, (0, 0, 0, "leased".into()));
                    drop(pool);
                    bootstrap
                        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
                        .await
                        .unwrap();
                    return;
                }

                let mut divergent_claims = Vec::new();
                let mut divergent = preparation_claim.clone();
                divergent.tenant_id = "different-tenant".into();
                divergent_claims.push(divergent);
                let mut divergent = preparation_claim.clone();
                divergent.region = "different-region".into();
                divergent_claims.push(divergent);
                let mut divergent = preparation_claim.clone();
                divergent.wf_run_id = "aaaaaaaa-1111-4111-8111-111111111111".into();
                divergent_claims.push(divergent);
                let mut divergent = preparation_claim.clone();
                divergent.ci_run_id = "aaaaaaaa-2222-4222-8222-222222222222".into();
                divergent_claims.push(divergent);
                let mut divergent = preparation_claim.clone();
                divergent.idem_token.push_str("-different");
                divergent_claims.push(divergent);
                let mut divergent = preparation_claim.clone();
                divergent.token_authority_handle.push_str("-different");
                divergent_claims.push(divergent);
                let mut divergent = preparation_claim.clone();
                divergent.lease_owner.push_str("-different");
                divergent_claims.push(divergent);
                let mut divergent = preparation_claim.clone();
                divergent.lease_epoch += 1;
                divergent_claims.push(divergent);
                let mut divergent = preparation_claim.clone();
                divergent.claim_nonce = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into();
                divergent_claims.push(divergent);
                let mut divergent = preparation_claim.clone();
                divergent.claim_started_at_epoch_secs += 1;
                divergent_claims.push(divergent);
                let mut divergent = preparation_claim.clone();
                divergent.claim_expires_at_epoch_secs += 1;
                divergent_claims.push(divergent);
                for divergent in divergent_claims {
                    assert!(
                        preparation_reporter()
                            .report_preparation_terminal(&divergent, refusal_disposition)
                            .is_err(),
                        "every exact-generation identity divergence is refused"
                    );
                }
                let original_spec: serde_json::Value =
                    sqlx::query_scalar("SELECT spec FROM ci_job_spec WHERE job_id = $1::uuid")
                        .bind(job)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                sqlx::query(
                    "UPDATE ci_job_spec
             SET spec = jsonb_set(spec, '{spec,meter_to,reserve_id}', '\"different-reserve\"')
             WHERE job_id = $1::uuid",
                )
                .bind(job)
                .execute(&pool)
                .await
                .unwrap();
                assert!(
                    preparation_reporter()
                        .report_preparation_terminal(&preparation_claim, refusal_disposition)
                        .is_err(),
                    "a dispatched meter target diverging from the manifest/parent is refused"
                );
                sqlx::query("UPDATE ci_job_spec SET spec = $2 WHERE job_id = $1::uuid")
                    .bind(job)
                    .bind(original_spec.clone())
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query(
                    "UPDATE ci_job_spec
             SET spec = jsonb_set(
                 jsonb_set(spec, '{spec,workspace,repo_ref}', 'null'),
                 '{spec,workspace,commit}', 'null'
             )
             WHERE job_id = $1::uuid",
                )
                .bind(job)
                .execute(&pool)
                .await
                .unwrap();
                assert!(
                    preparation_reporter()
                        .report_preparation_terminal(&preparation_claim, refusal_disposition)
                        .is_err(),
                    "a compute-only durable job cannot claim a preparation disposition"
                );
                sqlx::query("UPDATE ci_job_spec SET spec = $2 WHERE job_id = $1::uuid")
                    .bind(job)
                    .bind(original_spec)
                    .execute(&pool)
                    .await
                    .unwrap();

                if preparation_scenario == "preparation_launch_first" {
                    first_hooks
                        .acquire_launch_permit(&first_spec)
                        .unwrap()
                        .commit_and_release()
                        .unwrap();
                    assert!(preparation_reporter()
                        .report_preparation_terminal(&preparation_claim, refusal_disposition)
                        .is_err());
                    assert_eq!(
                        reporter(true).report_done(&claim, &report).unwrap(),
                        SignalOutcome::Buffered,
                        "after launch wins, only the ordinary running completion can terminalize"
                    );
                    drop(pool);
                    bootstrap
                        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
                        .await
                        .unwrap();
                    return;
                }

                if preparation_scenario == "preparation_exhausted" {
                    for epoch in 2_i64..=5 {
                        let nonce = format!("bbbbbbbb-bbbb-4bbb-8bbb-{epoch:012}");
                        sqlx::query(
                            "INSERT INTO ci_job_parent_attempt (
                       tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle,
                       lease_owner, lease_epoch, claim_nonce, claim_started_at_epoch_secs,
                       claim_expires_at_epoch_secs, budget_revision, max_parent_attempts
                     ) VALUES (
                       $1, $2, $3::uuid, $4::uuid, $5::uuid, $6, $7, $8, $9::uuid,
                       $10, $11, 1, 5
                     )",
                        )
                        .bind(&tenant.0)
                        .bind(&region.0)
                        .bind(job)
                        .bind(wf_run)
                        .bind(ci_run)
                        .bind(&reserve_handle)
                        .bind(format!("prior-runner-{epoch}"))
                        .bind(epoch)
                        .bind(nonce)
                        .bind(preparation_claim.claim_started_at_epoch_secs - epoch)
                        .bind(preparation_claim.claim_expires_at_epoch_secs - epoch)
                        .execute(&pool)
                        .await
                        .unwrap();
                    }
                    sqlx::query(
                        "UPDATE job_queue q
                 SET lease_owner = p.lease_owner, lease_epoch = p.lease_epoch,
                     claim_nonce = p.claim_nonce,
                     claim_started_at = to_timestamp(p.claim_started_at_epoch_secs),
                     claim_expires_at = to_timestamp(p.claim_expires_at_epoch_secs),
                     lease_expires = to_timestamp(p.claim_expires_at_epoch_secs)
                 FROM ci_job_parent_attempt p
                 WHERE q.tenant_id = p.tenant_id AND q.region = p.region
                   AND q.job_id = p.job_id AND p.lease_epoch = 5
                   AND q.job_id = $1::uuid",
                    )
                    .bind(job)
                    .execute(&pool)
                    .await
                    .unwrap();
                    let exhausted_claim = sqlx::query(
                        "SELECT lease_owner, lease_epoch, claim_nonce::text AS claim_nonce,
                        EXTRACT(EPOCH FROM claim_started_at)::bigint AS claim_started,
                        EXTRACT(EPOCH FROM claim_expires_at)::bigint AS claim_expires
                 FROM job_queue WHERE job_id = $1::uuid",
                    )
                    .bind(job)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                    let mut exhausted_claim_request = preparation_claim.clone();
                    exhausted_claim_request.lease_owner = exhausted_claim.get("lease_owner");
                    exhausted_claim_request.lease_epoch = exhausted_claim.get("lease_epoch");
                    exhausted_claim_request.claim_nonce = exhausted_claim.get("claim_nonce");
                    exhausted_claim_request.claim_started_at_epoch_secs =
                        exhausted_claim.get("claim_started");
                    exhausted_claim_request.claim_expires_at_epoch_secs =
                        exhausted_claim.get("claim_expires");
                    assert_eq!(
                        preparation_reporter()
                            .report_preparation_terminal(
                                &exhausted_claim_request,
                                PreparationTerminalDisposition::AttemptsExhausted,
                            )
                            .unwrap(),
                        SignalOutcome::Buffered
                    );
                    let disposition: String = sqlx::query_scalar(
                "SELECT terminal_disposition FROM ci_job_accounting WHERE job_id = $1::uuid",
            )
            .bind(job)
            .fetch_one(&pool)
            .await
            .unwrap();
                    assert_eq!(disposition, "preparation_attempts_exhausted");
                    drop(pool);
                    bootstrap
                        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
                        .await
                        .unwrap();
                    return;
                }

                if preparation_scenario == "preparation_duplicate" {
                    let barrier = Arc::new(std::sync::Barrier::new(3));
                    let mut tasks = Vec::new();
                    for _ in 0..2 {
                        let barrier = barrier.clone();
                        let reporter = preparation_reporter();
                        let claim = preparation_claim.clone();
                        tasks.push(tokio::task::spawn_blocking(move || {
                            barrier.wait();
                            reporter.report_preparation_terminal(&claim, refusal_disposition)
                        }));
                    }
                    barrier.wait();
                    let left = tasks.remove(0).await.unwrap().unwrap();
                    let right = tasks.remove(0).await.unwrap().unwrap();
                    assert!(
                        matches!(
                            (left, right),
                            (SignalOutcome::Buffered, SignalOutcome::Duplicate)
                                | (SignalOutcome::Duplicate, SignalOutcome::Buffered)
                        ),
                        "concurrent identical deliveries produce one signal and one exact replay"
                    );
                    assert_eq!(
                        counts(&pool, job, wf_run).await,
                        (1, 2, 1, "terminal".into())
                    );
                    drop(pool);
                    bootstrap
                        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
                        .await
                        .unwrap();
                    return;
                }

                if preparation_scenario == "preparation_cancel_race" {
                    sqlx::query(
                        "UPDATE workflow_run
                 SET state = 'terminated', cancel_reason = 'preparation-race'
                 WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                    )
                    .bind(&tenant.0)
                    .bind(&region.0)
                    .bind(wf_run)
                    .execute(&pool)
                    .await
                    .unwrap();
                    sqlx::query(
                        "UPDATE ci_run
                 SET state = 'cancelled', finished_at = statement_timestamp()
                 WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid",
                    )
                    .bind(&tenant.0)
                    .bind(&region.0)
                    .bind(ci_run)
                    .execute(&pool)
                    .await
                    .unwrap();
                    let supersession = PgCiRunSupersession::new(
                        pool.clone(),
                        ledger.clone(),
                        tenant.clone(),
                        region.clone(),
                        tokio::runtime::Handle::current(),
                    )
                    .unwrap();
                    let barrier = Arc::new(std::sync::Barrier::new(3));
                    let cancellation_barrier = barrier.clone();
                    let cancellation = tokio::spawn(async move {
                        cancellation_barrier.wait();
                        supersession.cancel_running_for_test(ci_run, wf_run).await
                    });
                    let completion_barrier = barrier.clone();
                    let completion_reporter = preparation_reporter();
                    let completion_claim = preparation_claim.clone();
                    let completion = tokio::task::spawn_blocking(move || {
                        completion_barrier.wait();
                        completion_reporter
                            .report_preparation_terminal(&completion_claim, refusal_disposition)
                    });
                    barrier.wait();
                    let cancellation_won = cancellation.await.unwrap().is_ok();
                    let preparation_won = completion.await.unwrap().is_ok();
                    assert_ne!(
                        cancellation_won, preparation_won,
                        "cancellation and preparation completion admit one terminal owner"
                    );
                    let shape = counts(&pool, job, wf_run).await;
                    assert_eq!((shape.0, shape.1, shape.3.as_str()), (1, 2, "terminal"));
                    drop(pool);
                    bootstrap
                        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
                        .await
                        .unwrap();
                    return;
                }

                if preparation_scenario == "preparation_rollback" {
                    sqlx::raw_sql(
                        "CREATE FUNCTION fail_preparation_accounting_receipt() RETURNS trigger
                   LANGUAGE plpgsql AS $$
                   BEGIN
                     RAISE EXCEPTION 'injected preparation receipt failure';
                   END $$;
                 CREATE TRIGGER fail_preparation_accounting_receipt
                   BEFORE INSERT ON ci_job_accounting
                   FOR EACH ROW EXECUTE FUNCTION fail_preparation_accounting_receipt();",
                    )
                    .execute(&pool)
                    .await
                    .unwrap();
                    assert!(preparation_reporter()
                        .report_preparation_terminal(
                            &preparation_claim,
                            PreparationTerminalDisposition::Failed {
                                phase: PreparationPhase::CheckoutTransport,
                            },
                        )
                        .is_err());
                    assert_eq!(
                        counts(&pool, job, wf_run).await,
                        (0, 0, 0, "leased".into()),
                        "a post-CAS receipt failure rolls every preparation effect back"
                    );
                    let reservation: (String, i64) = sqlx::query_as(
                        "SELECT state, (SELECT count(*) FROM cost_event WHERE run_id = $1)
                 FROM cost_reservation WHERE run_id = $1",
                    )
                    .bind(&reserve_handle)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                    assert_eq!(reservation, ("inflight".into(), 0));
                    drop(pool);
                    bootstrap
                        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
                        .await
                        .unwrap();
                    return;
                }

                if preparation_scenario == "preparation_race" {
                    let barrier = Arc::new(std::sync::Barrier::new(3));
                    let launch_barrier = barrier.clone();
                    let report_barrier = barrier.clone();
                    let launch = tokio::task::spawn_blocking(move || {
                        launch_barrier.wait();
                        first_hooks
                            .acquire_launch_permit(&first_spec)
                            .and_then(|permit| {
                                permit.commit_and_release().map_err(|error| {
                                    myelin_ci_sandbox::HookError(format!("{error:?}"))
                                })
                            })
                    });
                    let racing_reporter = preparation_reporter();
                    let racing_claim = preparation_claim.clone();
                    let completion = tokio::task::spawn_blocking(move || {
                        report_barrier.wait();
                        racing_reporter.report_preparation_terminal(
                            &racing_claim,
                            PreparationTerminalDisposition::Failed {
                                phase: PreparationPhase::CheckoutTransport,
                            },
                        )
                    });
                    barrier.wait();
                    let launch_won = launch.await.unwrap().is_ok();
                    let preparation_won = completion.await.unwrap().is_ok();
                    assert_ne!(
                        launch_won, preparation_won,
                        "real concurrent PostgreSQL updates admit exactly one owner"
                    );
                    let state: String =
                        sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1::uuid")
                            .bind(job)
                            .fetch_one(&pool)
                            .await
                            .unwrap();
                    assert_eq!(state, if launch_won { "running" } else { "terminal" });
                    let accounting_count: i64 =
                        sqlx::query_scalar("SELECT count(*) FROM ci_job_accounting")
                            .fetch_one(&pool)
                            .await
                            .unwrap();
                    assert_eq!(accounting_count, i64::from(preparation_won));
                    drop(pool);
                    bootstrap
                        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
                        .await
                        .unwrap();
                    return;
                }

                let exact = preparation_reporter()
                    .report_preparation_terminal(
                        &preparation_claim,
                        PreparationTerminalDisposition::Failed {
                            phase: PreparationPhase::CheckoutTransport,
                        },
                    )
                    .expect("the exact leased preparation generation terminalizes");
                assert_eq!(exact, SignalOutcome::Buffered);
                assert_eq!(
                    preparation_reporter()
                        .report_preparation_terminal(
                            &preparation_claim,
                            PreparationTerminalDisposition::Failed {
                                phase: PreparationPhase::CheckoutTransport,
                            },
                        )
                        .unwrap(),
                    SignalOutcome::Duplicate
                );
        assert!(preparation_reporter()
            .report_preparation_terminal(
                &preparation_claim,
                PreparationTerminalDisposition::TimedOut {
                    phase: PreparationPhase::CheckoutTransport,
                },
            )
            .is_err());
        assert!(preparation_reporter()
            .report_preparation_terminal(
                &preparation_claim,
                PreparationTerminalDisposition::Failed {
                    phase: PreparationPhase::CheckoutMaterialization,
                },
            )
            .is_err());
                assert!(first_hooks.acquire_launch_permit(&first_spec).is_err());
                assert!(reporter(true).report_done(&claim, &report).is_err());
                assert_eq!(
                    counts(&pool, job, wf_run).await,
                    (1, 2, 1, "terminal".into())
                );
                let accounting: (i64, i64, bool, bool, String, i64) = sqlx::query_as(
                    "SELECT cpu_seconds, mem_byte_seconds, passed, timed_out, terminal_disposition,
                    (SELECT count(*) FROM cost_event WHERE run_id = $2)
             FROM ci_job_accounting WHERE job_id = $1::uuid",
                )
                .bind(job)
                .bind(&reserve_handle)
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(
                    accounting,
                    (
                        i64::try_from(JOURNALED_PRELAUNCH_USAGE.cpu_seconds).unwrap(),
                        i64::try_from(JOURNALED_PRELAUNCH_USAGE.mem_byte_seconds).unwrap(),
                        false,
                        false,
                        "checkout_transport_failed".into(),
                        2,
                    ),
                    "prelaunch usage is priced exactly once with zero workload usage"
                );
                let summary: serde_json::Value =
                    sqlx::query_scalar("SELECT result_summary FROM ci_job WHERE job_id = $1::uuid")
                        .bind(job)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                assert_eq!(
                    summary,
                    serde_json::json!({
                        "passed": false,
                        "timed_out": false,
                        "disposition": "checkout_transport_failed",
                        "workload_started": false,
                    })
                );
                drop(pool);
                bootstrap
                    .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
                    .await
                    .unwrap();
                return;
            }
    assert!(
        reporter(true).report_done(&claim, &report).is_err(),
        "a merely leased generation cannot report work that never crossed the launch CAS"
    );
    assert_eq!(
        counts(&pool, job, wf_run).await,
        (0, 0, 0, "leased".into()),
        "leased completion refusal has zero queue, signal, or accounting effects"
    );
    first_hooks
        .acquire_launch_permit(&first_spec)
        .unwrap()
        .commit_and_release()
        .expect("the first generation crosses the exact production launch CAS");
    let first_state: String =
        sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1::uuid")
            .bind(job)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(first_state, "running");

    let retryable = RetryableAttemptFailure {
        cause: RetryableAttemptCause::OutputPersistence,
        usage: report.usage,
    };
    if supersession_wins_first {
        sqlx::query(
            "UPDATE workflow_run
             SET state = 'terminated', cancel_reason = 'superseded-by-newer-pr-head'
             WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
        )
        .bind(&tenant.0)
        .bind(&region.0)
        .bind(wf_run)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE ci_run
             SET state = 'cancelled', cost_settled = false, finished_at = clock_timestamp(),
                 cause_event_id = 'trigger-cancel', correlation_id = 'cancel-correlation'
             WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid",
        )
        .bind(&tenant.0)
        .bind(&region.0)
        .bind(ci_run)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE job_queue SET lease_expires = statement_timestamp() - interval '1 second'
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(&tenant.0)
        .bind(&region.0)
        .bind(job)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            region_store.reap(&region.0).await.unwrap(),
            0,
            "the generic dead-runner reaper never resurrects a terminated workflow"
        );
        assert!(
            region_store
                .claim(
                    &region.0,
                    &runner_labels,
                    &[TrustTier::Trusted],
                    "obsolete-before-report",
                    CI_RUNNER_EXECUTION_LEASE_TTL_SECS as u64,
                )
                .await
                .unwrap()
                .is_none(),
            "a cancelled run is unclaimable even before late accounting closes"
        );
        if runner_abandoned {
            let poison_tenant = "aaa-poison";
            let poison_wf = "01111111-1111-8111-8111-111111111111";
            let poison_ci = "02222222-2222-8222-8222-222222222222";
            let poison_job = "03333333-3333-8333-8333-333333333333";
            sqlx::query(
                "INSERT INTO workflow_run (
                   tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
                   depth, partition, idem_key
                 ) VALUES (
                   $1, $2, $3, 'ci.pipeline', 1, '[]'::jsonb, 'terminated', $3, 0, 0,
                   'poison-cancelled-recovery'
                 )",
            )
            .bind(poison_tenant)
            .bind(&region.0)
            .bind(poison_wf)
            .execute(&pool)
            .await
            .unwrap();
            ci_runs
                .insert_ci_run(&CiRunInsert {
                    tenant_id: poison_tenant.into(),
                    region: region.0.clone(),
                    run_id: poison_ci.into(),
                    project_id: "05555555-5555-8555-8555-555555555555".into(),
                    pipeline_id: "06666666-6666-8666-8666-666666666666".into(),
                    wf_run_id: poison_wf.into(),
                    definition_snapshot: format!("blake3:{}", "f".repeat(64)),
                    trigger_kind: "push".into(),
                    concurrency_group: None,
                    pr_head_generation: None,
                    trust_tier: "trusted".into(),
                    state: "queued".into(),
                    correlation_id: "poison-cancelled-recovery".into(),
                    cause_event_id: Some("trigger-poison-cancelled-recovery".into()),
                    cause_depth: 0,
                    caused_by: None,
                    repo_ref: Some(format!("myelin://{poison_tenant}/git/repo/core")),
                    commit_oid: Some("badc0de".into()),
                    triggered_by: None,
                })
                .await
                .unwrap();
            sqlx::query(
                "UPDATE ci_run
                 SET state = 'cancelled', finished_at = clock_timestamp()
                 WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid",
            )
            .bind(poison_tenant)
            .bind(&region.0)
            .bind(poison_ci)
            .execute(&pool)
            .await
            .unwrap();
            ci_job_queue_store(pool.clone())
                .enqueue(&DurableEnqueue {
                    tenant_id: poison_tenant.into(),
                    region: region.0.clone(),
                    job_id: poison_job.into(),
                    run_id: poison_wf.into(),
                    lane: Lane::Interactive,
                    labels: runner_labels.clone(),
                    trust_tier: TrustTier::Trusted,
                    concurrency_group: None,
                    fair_key: poison_tenant.into(),
                    idem_token: "poison-cancelled-recovery".into(),
                    stage: "build".into(),
                    claim_window_secs: CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
                })
                .await
                .unwrap();
            sqlx::query(
                "UPDATE job_queue
                 SET state = 'running', lease_owner = 'dead-poison-runner',
                     lease_expires = statement_timestamp() - interval '1 second',
                     lease_epoch = 1, claim_nonce = $4::uuid,
                     claim_started_at = statement_timestamp() - interval '2 seconds',
                     claim_expires_at = statement_timestamp() - interval '1 second'
                 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
            )
            .bind(poison_tenant)
            .bind(&region.0)
            .bind(poison_job)
            .bind("04444444-4444-8444-8444-444444444444")
            .execute(&pool)
            .await
            .unwrap();

            let reaper = JobQueueReaper::new(
                region_store.clone(),
                region.0.clone(),
                std::time::Duration::from_secs(1),
            )
            .with_cancelled_accounting(pool.clone(), ledger.clone());
                    let recovery_error = reaper.reap_once().await.expect_err(
                        "the poison candidate is surfaced after the healthy candidate runs",
                    );
            let rendered = recovery_error.to_string();
            assert!(
                rendered.contains("1 cancelled recovery candidate(s) failed")
                    && rendered.contains("after 1 row(s) were recovered"),
                "the bounded sweep reports the isolated poison failure and committed progress: \
                 {rendered}"
            );
            assert_eq!(
                counts(&pool, job, wf_run).await,
                (1, 2, 0, "terminal".into())
            );
            let poison_state: String = sqlx::query_scalar(
                "SELECT state FROM job_queue WHERE tenant_id = $1 AND job_id = $2::uuid",
            )
            .bind(poison_tenant)
            .bind(poison_job)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                poison_state, "running",
                "the corrupt first candidate rolls back without blocking the later tenant"
            );
            let accounting: (bool, i64, i64, i64, i64) = sqlx::query_as(
                "SELECT skipped, cpu_seconds, mem_byte_seconds,
                        billed_minor_units, refunded_minor_units
                 FROM ci_job_accounting WHERE job_id = $1::uuid",
            )
            .bind(job)
            .fetch_one(&pool)
            .await
            .unwrap();
            let usage = ResourceUsage {
                cpu_seconds: 60 + journal_usage.cpu_seconds,
                mem_byte_seconds: 60 * 1_073_741_824 + journal_usage.mem_byte_seconds,
            };
            let (billed, refunded) = expected_operational_settlement(usage);
            assert_eq!(
                accounting,
                (
                    false,
                    i64::try_from(usage.cpu_seconds).unwrap(),
                    i64::try_from(usage.mem_byte_seconds).unwrap(),
                    billed,
                    refunded,
                ),
                "unknown crash usage is conservatively closed at immutable manifest ceilings"
            );
            let run_closed: bool = sqlx::query_scalar(
                "SELECT cost_settled FROM ci_run WHERE tenant_id = $1 AND run_id = $2::uuid",
            )
            .bind(&tenant.0)
            .bind(ci_run)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(run_closed);
            drop(pool);
            bootstrap
                .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
                .await
                .unwrap();
            return;
        }
        assert_eq!(
            production_reporter
                .report_retryable_attempt(&claim, &retryable)
                .unwrap(),
            RetryableAttemptOutcome::Cancelled,
            "a supersession that wins the Flow lock terminalizes and settles the measured attempt"
        );
        assert_eq!(
            counts(&pool, job, wf_run).await,
            (1, 2, 0, "terminal".into()),
            "the late measured attempt emits no job.done and cannot become claimable again"
        );
        let accounting: (bool, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT skipped, cpu_seconds, mem_byte_seconds,
                    billed_minor_units, refunded_minor_units
             FROM ci_job_accounting WHERE job_id = $1::uuid",
        )
        .bind(job)
        .fetch_one(&pool)
        .await
        .unwrap();
        let usage = ResourceUsage {
            cpu_seconds: retryable.usage.cpu_seconds + journal_usage.cpu_seconds,
                    mem_byte_seconds: retryable.usage.mem_byte_seconds
                        + journal_usage.mem_byte_seconds,
        };
        let (billed, refunded) = expected_operational_settlement(usage);
        assert_eq!(
            accounting,
            (
                false,
                i64::try_from(usage.cpu_seconds).unwrap(),
                i64::try_from(usage.mem_byte_seconds).unwrap(),
                billed,
                refunded,
            )
        );
        let run_closed: bool = sqlx::query_scalar(
            "SELECT cost_settled FROM ci_run WHERE tenant_id = $1 AND run_id = $2::uuid",
        )
        .bind(&tenant.0)
        .bind(ci_run)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(run_closed, "the cancelled one-job run is cost-closed");
        assert!(
            region_store
                .claim(
                    &region.0,
                    &runner_labels,
                    &[TrustTier::Trusted],
                    "obsolete-probe",
                    CI_RUNNER_EXECUTION_LEASE_TTL_SECS as u64,
                )
                .await
                .unwrap()
                .is_none(),
            "superseded measured work is never runnable again"
        );
        assert_eq!(
            production_reporter
                .report_retryable_attempt(&claim, &retryable)
                .unwrap(),
            RetryableAttemptOutcome::ExactReplay
        );
        assert_eq!(
            counts(&pool, job, wf_run).await,
            (1, 2, 0, "terminal".into())
        );
        drop(pool);
        bootstrap
            .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
            .await
            .unwrap();
        return;
    }
    assert_eq!(
        production_reporter
            .report_retryable_attempt(&claim, &retryable)
            .unwrap(),
        RetryableAttemptOutcome::Requeued,
        "a measured log-durability failure accrues usage and requeues the exact generation"
    );
    assert_eq!(
        production_reporter
            .report_retryable_attempt(&claim, &retryable)
            .unwrap(),
        RetryableAttemptOutcome::ExactReplay,
        "acknowledgement loss cannot double-accrue a failed attempt"
    );
    let mut divergent_retry = retryable;
    divergent_retry.usage.cpu_seconds += 1;
    assert!(
        production_reporter
            .report_retryable_attempt(&claim, &divergent_retry)
            .is_err(),
        "the same generation cannot replay with divergent usage"
    );
    assert_eq!(
        counts(&pool, job, wf_run).await,
        (0, 0, 0, "queued".into()),
        "retryable attempt accounting emits no job.done and no terminal money event"
    );
    let accrued_count: i64 = sqlx::query_scalar(
        "SELECT (retry_attempts->>'attempts')::bigint
         FROM job_queue WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(&tenant.0)
    .bind(&region.0)
    .bind(job)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(accrued_count, 1);
    if retry_then_supersession {
        let supersession = PgCiRunSupersession::new(
            pool.clone(),
            ledger.clone(),
            tenant.clone(),
            region.clone(),
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        supersession
            .cancel_running_for_test(ci_run, wf_run)
            .await
            .expect("the real supersession transaction cancels the retry-queued run");
        assert_eq!(
            counts(&pool, job, wf_run).await,
            (1, 2, 0, "terminal".into()),
            "real supersession settles accrued usage without job.done"
        );
        let accounting: (bool, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT skipped, cpu_seconds, mem_byte_seconds,
                    billed_minor_units, refunded_minor_units
             FROM ci_job_accounting WHERE job_id = $1::uuid",
        )
        .bind(job)
        .fetch_one(&pool)
        .await
        .unwrap();
        let usage = ResourceUsage {
            cpu_seconds: retryable.usage.cpu_seconds + journal_usage.cpu_seconds,
                    mem_byte_seconds: retryable.usage.mem_byte_seconds
                        + journal_usage.mem_byte_seconds,
        };
        let (billed, refunded) = expected_operational_settlement(usage);
        assert_eq!(
            accounting,
            (
                false,
                i64::try_from(usage.cpu_seconds).unwrap(),
                i64::try_from(usage.mem_byte_seconds).unwrap(),
                billed,
                refunded,
            )
        );
        assert!(region_store
            .claim(
                &region.0,
                &runner_labels,
                &[TrustTier::Trusted],
                "obsolete-retry-probe",
                CI_RUNNER_EXECUTION_LEASE_TTL_SECS as u64,
            )
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            production_reporter
                .report_retryable_attempt(&claim, &retryable)
                .unwrap(),
            RetryableAttemptOutcome::ExactReplay
        );
        drop(pool);
        bootstrap
            .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
            .await
            .unwrap();
        return;
    }

    let second_lease = region_store
        .claim(
            &region.0,
            &runner_labels,
            &[TrustTier::Trusted],
            owner,
            CI_RUNNER_EXECUTION_LEASE_TTL_SECS as u64,
        )
        .await
        .unwrap()
        .expect("the retryable failure is immediately claimable as a fresh generation");
    assert_eq!(second_lease.lease_epoch, 2);
    if journal_prelaunch_usage {
        seed_prelaunch_usage_mix(
            &pool,
            JournalSeed {
                tenant: &tenant.0,
                region: &region.0,
                job_id: job,
                wf_run_id: wf_run,
                ci_run_id: ci_run,
                reserve_handle: &reserve_handle,
                include_phases: false,
            },
        )
        .await;
    }
            let second_spec =
                resolver(&second_lease).expect("the retry generation mints fresh authority");
    let second_hooks = ci_runner_hooks(
        provider.clone(),
        identity.launch_authorizer(),
        tokio::runtime::Handle::current(),
    );
    assert_eq!(
        second_hooks.reserve(&second_spec).unwrap().0,
        reserve_handle,
        "the in-flight operational reservation spans retry generations"
    );
    second_hooks
        .acquire_launch_permit(&second_spec)
        .unwrap()
        .commit_and_release()
        .expect("the retry generation crosses the exact production launch CAS");
    let claim = CompletionClaim {
        tenant: tenant.clone(),
        run: RunId(wf_run.into()),
        job_id: job.into(),
        idem_token: idem.clone(),
        lease_owner: owner.into(),
        lease_epoch: second_lease.lease_epoch,
        claim_nonce: second_lease.claim_nonce.clone(),
    };

    if journal_prelaunch_usage {
        assert_eq!(
            production_reporter.report_done(&claim, &report).unwrap(),
            SignalOutcome::Buffered,
            "normal completion settles journaled prelaunch, prior retry, and current workload once",
        );
        let expected = ResourceUsage {
            cpu_seconds: JOURNALED_PRELAUNCH_USAGE.cpu_seconds
                + retryable.usage.cpu_seconds
                + report.usage.cpu_seconds,
            mem_byte_seconds: JOURNALED_PRELAUNCH_USAGE.mem_byte_seconds
                + retryable.usage.mem_byte_seconds
                + report.usage.mem_byte_seconds,
        };
        let accounted: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT cpu_seconds, mem_byte_seconds, billed_minor_units, refunded_minor_units
             FROM ci_job_accounting WHERE job_id = $1::uuid",
        )
        .bind(job)
        .fetch_one(&pool)
        .await
        .unwrap();
        let (billed, refunded) = expected_operational_settlement(expected);
        assert_eq!(
            accounted,
            (
                i64::try_from(expected.cpu_seconds).unwrap(),
                i64::try_from(expected.mem_byte_seconds).unwrap(),
                billed,
                refunded,
            ),
            "the measured and sealed phases are included exactly once",
        );
        assert_eq!(
            production_reporter.report_done(&claim, &report).unwrap(),
            SignalOutcome::Duplicate,
            "existing accounting replay re-resolves the journal without adding it twice",
        );
        let replayed: (i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT cpu_seconds, mem_byte_seconds, billed_minor_units, refunded_minor_units,
                    (SELECT count(*) FROM cost_event WHERE run_id = $2)
             FROM ci_job_accounting WHERE job_id = $1::uuid",
        )
        .bind(job)
        .bind(&reserve_handle)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            replayed,
            (
                i64::try_from(expected.cpu_seconds).unwrap(),
                i64::try_from(expected.mem_byte_seconds).unwrap(),
                billed,
                refunded,
                2,
            ),
            "replay preserves one immutable receipt and one pair of cost rows",
        );
        if verify_existing_via_supersession {
            let supersession = PgCiRunSupersession::new(
                pool.clone(),
                ledger.clone(),
                tenant.clone(),
                region.clone(),
                tokio::runtime::Handle::current(),
            )
            .unwrap();
            supersession
                .cancel_running_for_test(ci_run, wf_run)
                .await
                .expect(
                    "supersession re-verifies existing accounting against the durable prelaunch \
                     journal",
                );
            let after_supersession: (i64, i64, i64, i64, i64) = sqlx::query_as(
                "SELECT cpu_seconds, mem_byte_seconds, billed_minor_units,
                        refunded_minor_units,
                        (SELECT count(*) FROM cost_event WHERE run_id = $2)
                 FROM ci_job_accounting WHERE job_id = $1::uuid",
            )
            .bind(job)
            .bind(&reserve_handle)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                after_supersession, replayed,
                "existing-accounting verification neither omits nor double-adds prelaunch usage",
            );
        }
        drop(pool);
        bootstrap
            .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
            .await
            .unwrap();
        return;
    }

    let finalization = CiRunFinalization {
        tenant_id: tenant.0.clone(),
        region: region.0.clone(),
        run_id: ci_run.into(),
        wf_run_id: wf_run.into(),
        terminal_state: CiRunTerminalState::Failed,
        completed_at: "2026-07-21T13:00:00Z".into(),
        jobs: vec![
            CiRunFinalizationJob {
                job_id: job.into(),
                reserve_handle: reserve_handle.clone(),
                flow_timed_out: false,
                dispatched: true,
            },
            CiRunFinalizationJob {
                job_id: skipped_job.into(),
                reserve_handle: "reserve:skipped-live".into(),
                flow_timed_out: false,
                dispatched: false,
            },
        ],
    };

    let mut cross_tenant_finalization = finalization.clone();
    cross_tenant_finalization.tenant_id = "different-tenant".into();
    assert_eq!(
        ci_runs
            .finalize_ci_run(&scope, &cross_tenant_finalization)
            .await
            .unwrap_err(),
        CiRunStoreError::InvalidFinalization("tenant or region scope")
    );

    assert_eq!(
        ci_runs
            .finalize_ci_run(&scope, &finalization)
            .await
            .unwrap_err(),
        CiRunStoreError::IncompleteTerminalAccounting,
        "the run cannot become terminal before its immutable accounting receipt exists"
    );

    assert_eq!(
        reporter(true).completion_settlement_owner(),
        CompletionSettlementOwner::TerminalReporter,
        "the durable accounting transaction is the sole successful-completion settlement owner"
    );
    sqlx::raw_sql(
        "CREATE FUNCTION fail_accounting_receipt_after_ledger() RETURNS trigger
           LANGUAGE plpgsql AS $$
           BEGIN
             IF NEW.job_id = '33333333-3333-8333-8333-333333333333'::uuid THEN
               RAISE EXCEPTION 'injected post-ledger accounting receipt failure';
             END IF;
             RETURN NEW;
           END $$;
         CREATE TRIGGER fail_accounting_receipt_after_ledger
           BEFORE INSERT ON ci_job_accounting
           FOR EACH ROW EXECUTE FUNCTION fail_accounting_receipt_after_ledger();",
    )
    .execute(&pool)
    .await
    .expect("install a failure after Storage settlement and CI projection writes");
    let post_ledger_error = reporter(true)
        .report_done(&claim, &report)
        .expect_err("the receipt fault must abort the complete terminal transaction");
    assert!(post_ledger_error
        .to_string()
        .contains("terminal CI accounting receipt refused"));
    assert_eq!(
        counts(&pool, job, wf_run).await,
        (0, 0, 0, "running".into()),
        "a post-ledger receipt failure rolls queue, signal, projection, and receipt back"
    );
    let reservation_shape: (String, i64) = sqlx::query_as(
        "SELECT state,
                (SELECT count(*) FROM cost_event WHERE run_id = $1)
           FROM cost_reservation WHERE run_id = $1",
    )
    .bind(&reserve_handle)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        reservation_shape,
        ("inflight".into(), 0),
        "Storage's reservation and events roll back with the later receipt failure"
    );
    sqlx::raw_sql(
        "DROP TRIGGER fail_accounting_receipt_after_ledger ON ci_job_accounting;
         DROP FUNCTION fail_accounting_receipt_after_ledger();",
    )
    .execute(&pool)
    .await
    .expect("remove the injected terminal-accounting fault");

    // The failed terminal transaction leaves the exact running generation live. Model runner death,
    // recover it through the advisory-lock-aware reaper, then drive the complete production claim →
    // Identity mint → fenced gVisor launch → exact-tenant accounting reporter path. This joins the
    // previously separate execution and settlement proofs at their real composition seams.
    sqlx::query(
        "UPDATE job_queue SET lease_expires = statement_timestamp() - interval '1 second'
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(&tenant.0)
    .bind(&region.0)
    .bind(job)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        region_store.reap(&region.0).await.unwrap(),
        1,
        "the failed running generation is recovered after its launch ownership was released"
    );
    let stale_shape = counts(&pool, job, wf_run).await;
    assert!(production_reporter.report_done(&claim, &report).is_err());
    assert_eq!(
        counts(&pool, job, wf_run).await,
        stale_shape,
        "the reaped epoch-2 completion cannot mutate accounting or signal state"
    );
    let leases = DurableLeaseAdapter::new(
        region_store,
        ci_job_queue_store(pool.clone()),
        region.0.clone(),
        tokio::runtime::Handle::current(),
        resolver,
    );
    let hooks = ci_runner_hooks(
        provider.clone(),
        identity.launch_authorizer(),
        tokio::runtime::Handle::current(),
    );
    let backend = GvisorBackend::new(test_registry());
    let firehose = CountingFirehose::new();
    let recovered_worker = "runner-recovered";
    let agent = RunnerAgent::new(
        recovered_worker,
        LINUX_SMALL_V1_RUNNER_LABELS
            .iter()
            .map(|label| (*label).to_owned())
            .collect(),
        vec![TrustTier::Trusted],
        region.clone(),
        CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
        leases,
        &backend,
        &firehose,
        &production_reporter,
        hooks,
    );
    let recovered = agent
        .run_one(first_secs + 1)
        .expect("the recovered production runner executes and atomically reports terminal");
    assert_eq!(
        recovered.signal_outcome,
        SignalOutcome::Buffered,
        "the real runner buffers the one durable job.done signal"
    );
    assert!(
        !recovered.report.passed && !recovered.report.timed_out,
        "the real /bin/false guest result, not a caller verdict, drives failure"
    );
    assert_eq!(recovered.lease_epoch, 3);
    assert_eq!(firehose.jobs_finished(), 1);
    let recovered_claim = CompletionClaim {
        tenant: tenant.clone(),
        run: RunId(wf_run.into()),
        job_id: job.into(),
        idem_token: idem.clone(),
        lease_owner: recovered_worker.into(),
        lease_epoch: recovered.lease_epoch,
        claim_nonce: recovered.claim_nonce.clone(),
    };
    assert_eq!(
        agent
            .report_done_again(&recovered_claim, &recovered.report)
            .unwrap(),
        SignalOutcome::Duplicate,
        "an acknowledgement-loss retry reuses the exact completion receipt and signal"
    );
    assert_eq!(
        counts(&pool, job, wf_run).await,
        (1, 2, 1, "terminal".into())
    );
    let accounting = sqlx::query(
        "SELECT cpu_seconds, mem_byte_seconds, pricing_revision, billed_minor_units,
                refunded_minor_units, completion_receipt, terminal_disposition,
                completion_receipt_v4
         FROM ci_job_accounting WHERE job_id = $1::uuid",
    )
    .bind(job)
    .fetch_one(&pool)
    .await
    .unwrap();
    let accounted_cpu = accounting.get::<i64, _>("cpu_seconds");
    let accounted_memory = accounting.get::<i64, _>("mem_byte_seconds");
    assert_eq!(
        accounted_cpu,
                i64::try_from(recovered.report.usage.cpu_seconds + retryable.usage.cpu_seconds)
                    .unwrap()
    );
    assert_eq!(
        accounted_memory,
                i64::try_from(
                    recovered.report.usage.mem_byte_seconds + retryable.usage.mem_byte_seconds
                )
            .unwrap()
    );
    assert_eq!(
        accounting.get::<String, _>("pricing_revision"),
        TIER_P_OPERATIONAL_PRICING_REVISION
    );
            let expected_billed =
                accounted_cpu + (accounted_memory + 1_073_741_823) / 1_073_741_824;
    assert_eq!(
        accounting.get::<i64, _>("billed_minor_units"),
        expected_billed
    );
    assert_eq!(
        accounting.get::<i64, _>("refunded_minor_units"),
        100 - expected_billed
    );
    assert!(accounting
        .get::<String, _>("completion_receipt")
        .starts_with("v3:"));
    assert_eq!(
        accounting.get::<Option<String>, _>("terminal_disposition"),
        None,
        "production reporter and supersession owners remain pinned to v3 until fleet convergence"
    );
    assert_eq!(
        accounting.get::<Option<String>, _>("completion_receipt_v4"),
        None
    );
    let storage_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cost_event WHERE run_id = $1")
            .bind(&reserve_handle)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(storage_events, 2);

    let durable_finalizer = DurableCiRunFinalizer::new(
        ci_runs.clone(),
        ledger.clone(),
        CiJobAccountingStore::with_pg(pool.clone(), region.clone()),
        manifest_store.clone(),
        scope.clone(),
        tokio::runtime::Handle::current(),
    );
    let mut wrong_terminal = finalization.clone();
    wrong_terminal.terminal_state = CiRunTerminalState::Succeeded;
    assert_eq!(
        durable_finalizer.finalize(&wrong_terminal).unwrap_err(),
        CiRunStoreError::TerminalVerdictDivergence
    );
    let skipped_after_rollback: (i64, String) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM ci_job_accounting WHERE job_id=$1::uuid), state
         FROM cost_reservation WHERE run_id='reserve:skipped-live'",
    )
    .bind(skipped_job)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(skipped_after_rollback, (0, "reserved".into()));

    // Model a process death after the external finalizer transaction commits but before Flow can
    // commit the drive. Recovery must route from workflow_run, not the now-terminal ci_run.
    let precommitted = durable_finalizer.finalize(&finalization).unwrap();
    assert_eq!(precommitted.write, CiRunFinalizationWrite::Finalized);
    let split_brain_window: (String, String) = sqlx::query_as(
        "SELECT ci.state, wf.state
           FROM ci_run ci
           JOIN workflow_run wf
             ON wf.tenant_id = ci.tenant_id
            AND wf.region = ci.region
            AND wf.run_id = ci.wf_run_id::text
          WHERE ci.run_id = $1::uuid",
    )
    .bind(ci_run)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(split_brain_window, ("failed".into(), "running".into()));

    let (second_secs, second_stamp) = drive_clock(&pool).await;
    let second_drive = production_poller
        .run_once(8, 8, second_secs, &second_stamp)
        .await
        .unwrap();
    assert_eq!(second_drive.scopes, 1);
    assert_eq!(second_drive.driven, 1);
    let workflow_terminal: String =
        sqlx::query_scalar("SELECT state FROM workflow_run WHERE run_id = $1")
            .bind(wf_run)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        workflow_terminal, "completed",
        "the replayed Flow drive must complete normally, never merely leave the active states"
    );
    let run_terminal: (String, bool, bool) = sqlx::query_as(
        "SELECT state, cost_settled, finished_at IS NOT NULL
         FROM ci_run WHERE run_id = $1::uuid",
    )
    .bind(ci_run)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(run_terminal, ("failed".into(), true, true));
    let skipped_accounting: (bool, bool, bool, i64, i64, String) = sqlx::query_as(
        "SELECT passed, timed_out, skipped, billed_minor_units, refunded_minor_units,
                pricing_revision
         FROM ci_job_accounting WHERE job_id=$1::uuid",
    )
    .bind(skipped_job)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        skipped_accounting,
        (false, false, true, 0, 40, "ci-skipped:v1".into())
    );
    let skipped_state: String = sqlx::query_scalar(
        "SELECT state FROM cost_reservation WHERE run_id='reserve:skipped-live'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(skipped_state, "cancelled");

    let replay = durable_finalizer.finalize(&finalization).unwrap();
    assert_eq!(replay.write, CiRunFinalizationWrite::ExactReplay);
    let mut divergent_finalization = finalization.clone();
    divergent_finalization.completed_at = "2026-07-21T13:00:01Z".into();
            let acknowledgement_loss_replay =
                durable_finalizer.finalize(&divergent_finalization).unwrap();
    assert_eq!(
        acknowledgement_loss_replay.write,
        CiRunFinalizationWrite::ExactReplay
    );
    assert_eq!(
        acknowledgement_loss_replay.completed_at,
        replay.completed_at
    );

    assert!(
        matches!(
            reporter(false)
                .report_done(&recovered_claim, &recovered.report)
                .unwrap(),
            SignalOutcome::Duplicate | SignalOutcome::TerminalNoOp
        ),
        "an exact replay reuses persisted pricing instead of consulting the now-invalid pricer"
    );
    assert_eq!(
        counts(&pool, job, wf_run).await,
        (1, 2, 1, "terminal".into())
    );

    let mut divergent = recovered.report.clone();
    divergent.usage.cpu_seconds += 1;
    assert!(reporter(true)
        .report_done(&recovered_claim, &divergent)
        .is_err());
    assert_eq!(
        counts(&pool, job, wf_run).await,
        (1, 2, 1, "terminal".into())
    );

    drop(pool);
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_and_supersession_are_safe_in_both_commit_orders() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(false, false, false, false, false, None).await;
    run_reporter_scenario(false, false, true, false, false, None).await;
    run_reporter_scenario(true, false, false, false, false, None).await;
    run_reporter_scenario(true, true, false, false, false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn normal_completion_and_existing_accounting_replay_include_prelaunch_usage_once() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(false, false, false, true, false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_terminal_retry_includes_prelaunch_usage_once() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(true, false, false, true, false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_supersession_includes_prelaunch_usage_once() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(false, false, true, true, false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abandoned_launched_reconciliation_includes_prelaunch_usage_once() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(true, true, false, true, false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_accounting_replay_verifies_prelaunch_usage_without_resettling() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(false, false, false, true, true, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preparation_terminal_is_exact_replay_safe_and_one_shot_accounted() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(
        false,
        false,
        false,
        true,
        false,
        Some("preparation_terminal"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preparation_terminal_and_launch_cas_race_with_exactly_one_winner() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(false, false, false, true, false, Some("preparation_race")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preparation_post_cas_failure_rolls_back_every_durable_effect() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(
        false,
        false,
        false,
        true,
        false,
        Some("preparation_rollback"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workload_launch_winning_first_excludes_preparation_completion() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(
        false,
        false,
        false,
        true,
        false,
        Some("preparation_launch_first"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_identical_preparation_reports_settle_once() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(
        false,
        false,
        false,
        true,
        false,
        Some("preparation_duplicate"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attempts_exhausted_requires_and_accepts_the_exact_durable_cap() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(
        false,
        false,
        false,
        true,
        false,
        Some("preparation_exhausted"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preparation_usage_outside_accounting_range_refuses_and_rolls_back() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(
        false,
        false,
        false,
        true,
        false,
        Some("preparation_overflow"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preparation_completion_refuses_legacy_v1_reservations() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(false, false, false, false, false, Some("preparation_v1")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_and_preparation_completion_race_with_one_settlement_owner() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    run_reporter_scenario(
        true,
        false,
        false,
        true,
        false,
        Some("preparation_cancel_race"),
    )
    .await;
}

/// CT-007 slice 5b.3-4a.1b: `settle_cancelled_job` must recognize a `ci-reserve:v2:...` handle
/// exactly like the existing `ci-reserve:v1:...` handle and reach real Storage settlement, not the
/// `CiRunSupersessionError::Settlement` refusal a pre-slice handle shape would hit. This never
/// launches the job (no `job_queue` row is seeded at all), so `cancel_running_on_conn` takes its
/// `None` queue-lifecycle arm and settles a full refund against the untouched `inflight` reservation
/// — the same shape the `retry_then_supersession` v1 scenario above proves, minus the retry/launch
/// machinery that scenario needs for its OWN assertions but this one does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_job_with_a_v2_reserve_handle_settles_like_v1() {
    let _migration_guard = MIGRATION_SCENARIO_LOCK.lock().await;
    let schema = format!("ci_accounting_{}_v2_settle", std::process::id());
    let bootstrap = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .expect("connect to create isolated schema");
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    let cleanup_bootstrap = bootstrap.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(
        &cleanup_bootstrap,
        &schema_for_cleanup,
        move || async move {
        let pool = isolated_pool(&schema).await;
        PgMigrator::apply(&pool, &foundation_migrations())
            .await
            .unwrap();
        PgMigrator::apply(&pool, &identity_durable_migrations())
            .await
            .unwrap();
        PgMigrator::apply(&pool, &cell_root_durable_migrations())
            .await
            .unwrap();
        PgMigrator::apply_validated(
            &pool,
            &flow_migrations(),
            &HotTables::declare(["workflow_run"]),
        )
        .await
        .unwrap();
        PgMigrator::apply(&pool, &reserve_settle_durable_migrations())
            .await
            .unwrap();
        common::with_fixture_migration_lock(&admin_url(), &pool, &schema, || async {
            PgMigrator::apply_validated(
                &pool,
                &ci_controlplane_migrations(),
                &ci_controlplane_hot_tables(),
            )
            .await
            .unwrap();
        })
        .await;

        let tenant = TenantId::from_token("accounting-v2-tenant");
        let region = Region::new("fr-par");
        let wf_run = "41111111-1111-8111-8111-111111111111";
        let ci_run = "42222222-2222-8222-8222-222222222222";
        let job = "43333333-3333-8333-8333-333333333333";
        let skipped_job = "47777777-7777-8777-8777-777777777777";
        let v2_reserve_handle = format!("ci-reserve:v2:{ci_run}:batch:{job}:item");

        let ci_runs = ci_run_store_factory(pool.clone());
        ci_runs
            .insert_ci_run(&CiRunInsert {
                tenant_id: tenant.0.clone(),
                region: region.0.clone(),
                run_id: ci_run.into(),
                project_id: "55555555-5555-8555-8555-555555555555".into(),
                pipeline_id: "66666666-6666-8666-8666-666666666666".into(),
                wf_run_id: wf_run.into(),
                definition_snapshot: format!("blake3:{}", "a".repeat(64)),
                trigger_kind: "push".into(),
                concurrency_group: None,
                pr_head_generation: None,
                trust_tier: "trusted".into(),
                state: "queued".into(),
                correlation_id: "accounting-v2-live".into(),
                cause_event_id: Some("trigger-accounting-v2-live".into()),
                cause_depth: 0,
                caused_by: None,
                repo_ref: Some(format!("myelin://{}/git/repo/core", tenant.0)),
                commit_oid: Some("deadbeef00deadbeef00deadbeef00deadbeef00".into()),
                triggered_by: None,
            })
            .await
            .unwrap();
            sqlx::query(
                "UPDATE ci_run SET state = 'running' WHERE tenant_id = $1 AND run_id = $2::uuid",
            )
            .bind(&tenant.0)
            .bind(ci_run)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO workflow_run (
               tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
               depth, partition, idem_key
             ) VALUES ($1, $2, $3, 'ci.pipeline', 1, '[]'::jsonb, 'running', $3, 0, 0, \
               'accounting-v2-live')",
        )
        .bind(&tenant.0)
        .bind(&region.0)
        .bind(wf_run)
        .execute(&pool)
        .await
        .unwrap();

        let manifest_store =
            CiDriveManifestStore::new(pool.clone(), tenant.clone(), region.clone()).unwrap();
        let production_definition = ci_manifest_pipeline_definition();
        let mut drive_manifest = manifest(
            &tenant.0,
            &region.0,
            wf_run,
            ci_run,
            job,
            skipped_job,
            production_definition.code_hash(),
        );
        drive_manifest.jobs.truncate(1);
        drive_manifest.check_attempts.remove("package");
        drive_manifest.jobs[0].reserve_handle = v2_reserve_handle.clone();
        manifest_store.insert(&drive_manifest).await.unwrap();

        sqlx::query(
            "INSERT INTO cost_reservation (tenant_id, region, run_id, reserved, state)
             VALUES ($1, $2, $3, 100, 'inflight')",
        )
        .bind(&tenant.0)
        .bind(&region.0)
        .bind(&v2_reserve_handle)
        .execute(&pool)
        .await
        .unwrap();

        let mut config = MyelinConfig::dev();
        config.database_url = scoped_url(&admin_url(), &schema);
        config.region = region.0.clone();
        let provider = SubstrateProvider::connect(config, 4).await.unwrap();
        let ledger = DurableCostLedger::new(provider.clone());

        let supersession = PgCiRunSupersession::new(
            pool.clone(),
            ledger.clone(),
            tenant.clone(),
            region.clone(),
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        supersession
            .cancel_running_for_test(ci_run, wf_run)
            .await
            .expect(
                "settle_cancelled_job must accept a v2-shaped reserve handle and reach real \
                 settlement, not the pre-slice Settlement refusal",
            );

        let accounting: (bool, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT skipped, cpu_seconds, mem_byte_seconds, billed_minor_units, refunded_minor_units
             FROM ci_job_accounting WHERE job_id = $1::uuid",
        )
        .bind(job)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            accounting,
            (true, 0, 0, 0, 100),
            "the never-launched v2-reserved job settles a full refund, same as the v1 shape"
        );
        let reservation_state: String =
            sqlx::query_scalar("SELECT state FROM cost_reservation WHERE run_id = $1")
                .bind(&v2_reserve_handle)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(reservation_state, "settled");
        let cost_events: i64 =
            sqlx::query_scalar("SELECT count(*) FROM cost_event WHERE run_id = $1")
                .bind(&v2_reserve_handle)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(cost_events, 2, "both metered units settle, exactly like v1");

        drop(pool);
        bootstrap
            .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
            .await
            .unwrap();
        },
    )
    .await;
}
