#![cfg(feature = "integration")]

use std::collections::BTreeMap;
use std::sync::Arc;

use myelin_ci_controlplane::{
    ci_artifact_ref, ci_controlplane_hot_tables, ci_controlplane_migrations, ci_job_queue_store,
    ci_job_spec_store, ci_manifest_pipeline_definition, ci_production_runtime_factory_test_support,
    ci_region_run_discovery_test_support, ci_run_ref, ci_run_store_factory, CiDriveManifestStore,
    CiDriveManifestV1, CiJobAccountingPricer, CiJobAccountingStore, CiJobPricingError,
    CiManifestLaneV1, CiManifestLimitsV1, CiManifestSchedulingV1, CiManifestTrustTierV1,
    CiManifestWorkspaceV1, CiPipelineReporter, CiRunFinalization, CiRunFinalizationJob,
    CiRunFinalizationWrite, CiRunFinalizer, CiRunInsert, CiRunStoreError, CiRunTerminalState,
    DurableCiJobAccounting, DurableCiRunFinalizer, GrantedCiJobV1, PricedCiJobUsage,
    TIER_P_OPERATIONAL_PRICING_REVISION,
};
use myelin_ci_sandbox::{
    CompletionClaim, CompletionSettlementOwner, ResourceUsage, TerminalReport, TerminalReporter,
};
use myelin_config::MyelinConfig;
use myelin_events::{IdMinter, MonotonicMinter};
use myelin_flow::{
    migrations::migrations as flow_migrations, partition_for_run_id, DurableExecutor, MinorUnits,
    PgFlowExecutor, RunId, SignalOutcome, StartSpec, CI_PIPELINE_WF_TYPE,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{
    provider::foundation_migrations, reserve_settle_durable_migrations, DurableCostLedger,
    HotTables, PgMigrator, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool, Row};

const OPERATIONAL_RESERVE_HANDLE: &str =
    "ci-reserve:v1:22222222-2222-8222-8222-222222222222:batch:33333333-3333-8333-8333-333333333333:item";

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
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
    CiDriveManifestV1 {
        schema_version: 1,
        tenant_id: tenant.into(),
        region: region.into(),
        wf_run_id: wf_run.into(),
        ci_run_id: ci_run.into(),
        source_snapshot_ref: format!("myelin://{tenant}/ci/artifact/snapshot-{}", digest('a')),
        source_plan_schema_version: 2,
        launch_request_digest: digest('b'),
        workflow_type: CI_PIPELINE_WF_TYPE.into(),
        workflow_definition_version: 1,
        workflow_code_hash: workflow_code_hash.into(),
        authority_policy_revision: "ci-policy:2026-07-21".into(),
        repo_ref: format!("myelin://{tenant}/git/repo/core"),
        commit_oid: "deadbeef".into(),
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
                image: format!("registry.example/build@sha256:{}", "d".repeat(64)),
                command: vec!["/bin/true".into()],
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
                    commit_oid: "deadbeef".into(),
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
                    commit_oid: "deadbeef".into(),
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
    }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reporter_co_commits_accounting_claim_and_signal_and_rolls_back_failure() {
    let schema = format!("ci_accounting_{}", std::process::id());
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
    let pool = isolated_pool(&schema).await;
    PgMigrator::apply(&pool, &foundation_migrations())
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
    PgMigrator::apply_validated(
        &pool,
        &ci_controlplane_migrations(),
        &ci_controlplane_hot_tables(),
    )
    .await
    .unwrap();

    let tenant = TenantId::from_token("accounting-tenant");
    let region = Region::new("fr-par");
    let wf_run = "11111111-1111-8111-8111-111111111111";
    let ci_run = "22222222-2222-8222-8222-222222222222";
    let job = "33333333-3333-8333-8333-333333333333";
    let skipped_job = "77777777-7777-8777-8777-777777777777";
    let nonce = "44444444-4444-8444-8444-444444444444";
    let owner = "runner-live";

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
            trust_tier: "trusted".into(),
            state: "queued".into(),
            correlation_id: "accounting-live".into(),
            cause_event_id: None,
            cause_depth: 0,
            caused_by: None,
            repo_ref: Some(format!("myelin://{}/git/repo/core", tenant.0)),
            commit_oid: Some("deadbeef".into()),
            triggered_by: None,
        })
        .await
        .unwrap();
    sqlx::query("UPDATE ci_run SET state = 'running' WHERE tenant_id = $1 AND run_id = $2::uuid")
        .bind(&tenant.0)
        .bind(ci_run)
        .execute(&pool)
        .await
        .unwrap();
    let manifest_store =
        CiDriveManifestStore::new(pool.clone(), tenant.clone(), region.clone()).unwrap();
    let production_definition = ci_manifest_pipeline_definition();
    let manifest_digest = manifest_store
        .insert(&manifest(
            &tenant.0,
            &region.0,
            wf_run,
            ci_run,
            job,
            skipped_job,
            production_definition.code_hash(),
        ))
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO cost_reservation (tenant_id, region, run_id, reserved, state)
         VALUES ($1, $2, $3, 100, 'inflight'),
                ($1, $2, 'reserve:skipped-live', 40, 'reserved')",
    )
    .bind(&tenant.0)
    .bind(&region.0)
    .bind(OPERATIONAL_RESERVE_HANDLE)
    .execute(&pool)
    .await
    .unwrap();

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
    config.database_url = admin_url();
    config.region = region.0.clone();
    let provider = SubstrateProvider::connect(config, 1).await.unwrap();
    let ledger = DurableCostLedger::new(provider);
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
                        ci_artifact_ref(&tenant.0, &format!("drive-manifest-{manifest_digest}")),
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
    sqlx::query(
        "UPDATE job_queue
            SET lease_owner = $3,
                lease_expires = now() + interval '1 hour',
                state = 'leased',
                lease_epoch = 1,
                claim_nonce = $4::uuid
          WHERE tenant_id = $1 AND job_id = $2::uuid",
    )
    .bind(&tenant.0)
    .bind(job)
    .bind(owner)
    .bind(nonce)
    .execute(&pool)
    .await
    .unwrap();

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
                myelin_ci_controlplane::CiCostEventStore::with_pg(pool.clone(), region.clone()),
                CiJobAccountingStore::with_pg(pool.clone(), region.clone()),
                Arc::new(TestPricer { valid }),
            ),
        )
    };
    let claim = CompletionClaim {
        tenant: tenant.clone(),
        run: RunId(wf_run.into()),
        job_id: job.into(),
        idem_token: idem.clone(),
        lease_owner: owner.into(),
        lease_epoch: 1,
        claim_nonce: nonce.into(),
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
                reserve_handle: OPERATIONAL_RESERVE_HANDLE.into(),
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
        reporter(false).completion_settlement_owner(),
        CompletionSettlementOwner::TerminalReporter,
        "the durable accounting transaction is the sole successful-completion settlement owner"
    );
    assert!(reporter(false).report_done(&claim, &report).is_err());
    assert_eq!(
        counts(&pool, job, wf_run).await,
        (0, 0, 0, "leased".into()),
        "a post-claim pricing refusal rolls the entire transaction back"
    );
    let reservation_state: String =
        sqlx::query_scalar("SELECT state FROM cost_reservation WHERE run_id = $1")
            .bind(OPERATIONAL_RESERVE_HANDLE)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reservation_state, "inflight");

    assert_eq!(
        production_reporter.report_done(&claim, &report).unwrap(),
        SignalOutcome::Buffered
    );
    assert_eq!(
        counts(&pool, job, wf_run).await,
        (1, 2, 1, "terminal".into())
    );
    let accounting = sqlx::query(
        "SELECT cpu_seconds, mem_byte_seconds, pricing_revision, billed_minor_units,
                refunded_minor_units, completion_receipt
         FROM ci_job_accounting WHERE job_id = $1::uuid",
    )
    .bind(job)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(accounting.get::<i64, _>("cpu_seconds"), 7);
    assert_eq!(
        accounting.get::<i64, _>("mem_byte_seconds"),
        2 * 1_073_741_824
    );
    assert_eq!(
        accounting.get::<String, _>("pricing_revision"),
        TIER_P_OPERATIONAL_PRICING_REVISION
    );
    assert_eq!(accounting.get::<i64, _>("billed_minor_units"), 9);
    assert_eq!(accounting.get::<i64, _>("refunded_minor_units"), 91);
    assert!(accounting
        .get::<String, _>("completion_receipt")
        .starts_with("v3:"));
    let storage_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cost_event WHERE run_id = $1")
            .bind(OPERATIONAL_RESERVE_HANDLE)
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
    assert!(
        !matches!(workflow_terminal.as_str(), "running" | "waiting"),
        "the replayed Flow drive closes the finalizer-to-Flow-commit crash window"
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
    let acknowledgement_loss_replay = durable_finalizer.finalize(&divergent_finalization).unwrap();
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
            reporter(false).report_done(&claim, &report).unwrap(),
            SignalOutcome::Duplicate | SignalOutcome::TerminalNoOp
        ),
        "an exact replay reuses persisted pricing instead of consulting the now-invalid pricer"
    );
    assert_eq!(
        counts(&pool, job, wf_run).await,
        (1, 2, 1, "terminal".into())
    );

    let mut divergent = report;
    divergent.usage.cpu_seconds += 1;
    assert!(reporter(true).report_done(&claim, &divergent).is_err());
    assert_eq!(
        counts(&pool, job, wf_run).await,
        (1, 2, 1, "terminal".into())
    );

    drop(pool);
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
}
