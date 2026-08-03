//! Live PostgreSQL proof for immutable-manifest resolution and DAG replay.
#![cfg(feature = "integration")]

mod common;

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use myelin_ci_controlplane::{
    ci_controlplane_hot_tables, ci_controlplane_migrations, decode_resolved_ci_manifest,
    register_durable_ci_manifest_pipeline, run_ci_manifest_pipeline, BUMP_CHECK_ATTEMPT_SQL,
    CiExecutionProfileV1, CiExecutionRequestV1, CiJobLaunchGrantV1, CiJobSpecStore,
    CiLaunchAuthorityError, CiLaunchAuthorityMaterializer, CiLaunchAuthorityV1,
    CiManifestDurableJobRunner, CiManifestInputResolver, CiManifestLaneV1, CiManifestLimitsV1,
    CiManifestSchedulingV1, CiRunFinalization, CiRunFinalizationOutcome, CiRunFinalizationWrite,
    CiRunFinalizer, CiRunStoreError, CiWorkflowDefinitionPin, PgCiPipelineStarter,
    PreparedRunPlanV2, ResolvedJobV2, ResolvedRunPlanV2, StartQueuedOutcome,
};
use futures::FutureExt;
use myelin_ci_sandbox::TrustTier;
use myelin_config::MyelinConfig;
use myelin_events::{Actor, IdMinter, MonotonicMinter};
use myelin_flow::{
    migrations::migrations as flow_migrations, partition_for_run_id, DurableExecutor,
    PgClaimedDriveInput, PgFlowExecutor, PgFlowWorker, PgInputResolveError, PgResolvedDriveInput,
    PgRunOnceOutcome, PgWorkerError, PgWorkerScope, PgWorkflowInputResolver, RunId, SignalPayload,
    TypedSignalSpec, CI_PIPELINE_WF_TYPE,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{
    provider::foundation_migrations, BlobStore, FsBlobStore, HotTables, PgMigrator,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool};

const TENANT: &str = "manifest_dag";
const REGION: &str = "fr-par";
const CI_RUN_ID: &str = "41000000-0000-8000-8000-000000000001";
const WF_RUN_ID: &str = "42000000-0000-8000-8000-000000000001";
const BODY_HASH: &str = "blake3:ci-manifest-dag-body-v1";

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| MyelinConfig::dev().database_url)
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

async fn pool_on(schema: &str) -> PgPool {
    let schema = schema.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(16)
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
        .expect("connect live PostgreSQL")
}

fn plan() -> ResolvedRunPlanV2 {
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
                command: vec!["/bin/build".into()],
                build: None,
                selected_cargo_vendor: None,
                needs: Vec::new(),
                is_generator: false,
                matrix_key: BTreeMap::new(),
            },
            ResolvedJobV2 {
                stage: "package".into(),
                name: "package".into(),
                image: format!("registry.example/package@sha256:{}", "b".repeat(64)),
                command: vec!["/bin/package".into()],
                build: None,
                selected_cargo_vendor: None,
                needs: vec!["build".into(), "test".into()],
                is_generator: false,
                matrix_key: BTreeMap::new(),
            },
            ResolvedJobV2 {
                stage: "test".into(),
                name: "test".into(),
                image: format!("registry.example/test@sha256:{}", "c".repeat(64)),
                command: vec!["/bin/test".into()],
                build: None,
                selected_cargo_vendor: None,
                needs: Vec::new(),
                is_generator: false,
                matrix_key: BTreeMap::new(),
            },
        ],
    }
}

#[derive(Clone)]
struct TestAuthority;

impl CiLaunchAuthorityMaterializer for TestAuthority {
    fn materialize<'a>(
        &'a self,
        record: &'a myelin_ci_controlplane::ci_run_store::CiRunRecord,
        prepared: &'a PreparedRunPlanV2,
        _definition: &'a CiWorkflowDefinitionPin,
    ) -> Pin<
        Box<dyn Future<Output = Result<CiLaunchAuthorityV1, CiLaunchAuthorityError>> + Send + 'a>,
    > {
        Box::pin(async move {
            Ok(CiLaunchAuthorityV1 {
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
                        token_authority_handle: format!("mint:{}", record.run_id),
                    })
                    .collect(),
                merge_waiter: None,
            })
        })
    }
}

struct AcceptFinalizer;

impl CiRunFinalizer for AcceptFinalizer {
    fn finalize(
        &self,
        finalization: &CiRunFinalization,
    ) -> Result<CiRunFinalizationOutcome, CiRunStoreError> {
        Ok(CiRunFinalizationOutcome {
            write: CiRunFinalizationWrite::Finalized,
            completed_at: finalization.completed_at.clone(),
        })
    }
}

#[derive(Clone)]
struct RetryOnceResolver {
    inner: CiManifestInputResolver,
    attempts: Arc<AtomicUsize>,
}

impl PgWorkflowInputResolver for RetryOnceResolver {
    fn resolve(
        &self,
        input: PgClaimedDriveInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PgInputResolveError>> + Send + '_>> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Box::pin(async {
                Err(PgInputResolveError::Retry(
                    "injected manifest-store outage".into(),
                ))
            })
        } else {
            self.inner.resolve(input)
        }
    }
}

fn definition() -> CiWorkflowDefinitionPin {
    CiWorkflowDefinitionPin::new(1, BODY_HASH).unwrap()
}

/// Seed `ci_run_check_attempt` for every manifest check context, mirroring the allocation
/// `CiRunStore::co_commit_reserve` performs at real dispatch/reserve time (see
/// `ci_run_store::allocate_reserve_check_attempts`). Production `PgCiPipelineStarter::run_once`
/// only ever READS this table (`pg_pipeline_starter::load_reserved_check_attempts`) — it never
/// allocates it itself, by design, because the runtime role has no `UPDATE` grant on the table and
/// the allocation is meant to happen exactly once, at dispatch reserve time, before the run is even
/// queued. This test drives `ci_run` into existence with a raw `INSERT` instead of going through the
/// dispatch consumer's `co_commit_reserve`, so it must perform this same reservation itself or the
/// starter correctly refuses to fabricate a manifest with no run-scoped attempt authority.
async fn reserve_test_check_attempts(pool: &PgPool, run_id: &str, repo_ref: &str, commit_oid: &str) {
    let run_id = sqlx::types::Uuid::parse_str(run_id).expect("test run UUID");
    for context in ["build", "package", "test"] {
        let attempt: i32 = sqlx::query_scalar(BUMP_CHECK_ATTEMPT_SQL)
            .bind(TENANT)
            .bind(REGION)
            .bind(repo_ref)
            .bind(commit_oid)
            .bind(context)
            .bind(run_id)
            .fetch_one(pool)
            .await
            .expect("allocate test check attempt");
        sqlx::query(
            "INSERT INTO ci_run_check_attempt \
             (tenant_id,region,run_id,repo_ref,commit_oid,context,run_attempt) \
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(run_id)
        .bind(repo_ref)
        .bind(commit_oid)
        .bind(context)
        .bind(attempt)
        .execute(pool)
        .await
        .expect("persist test run-scoped check attempt");
    }
}

fn worker(pool: &PgPool, name: &str) -> PgFlowWorker {
    let scope = PgWorkerScope::new(
        TenantId(TENANT.into()),
        Region(REGION.into()),
        partition_for_run_id(WF_RUN_ID),
        name,
        60,
        Actor(Principal::new(
            TenantId(TENANT.into()),
            Region(REGION.into()),
            PrincipalId("ci-controlplane".into()),
            PrincipalKind::Service,
            DataRole::Processor,
            PrincipalStatus::Active,
        )),
        1,
    )
    .unwrap();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    PgFlowWorker::new(
        pool.clone(),
        tokio::runtime::Handle::current(),
        minter,
        scope,
    )
}

fn resolver(pool: &PgPool) -> CiManifestInputResolver {
    CiManifestInputResolver::new(
        pool.clone(),
        TenantId(TENANT.into()),
        Region(REGION.into()),
        definition(),
    )
    .unwrap()
}

fn signal(pool: &PgPool, token: &str, name: &str, received_order: usize) {
    let executor = PgFlowExecutor::new(
        pool.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(MonotonicMinter::new()),
        TenantId(TENANT.into()),
        Region(REGION.into()),
    );
    tokio::task::block_in_place(|| {
        executor.signal_typed(TypedSignalSpec {
            run: RunId(WF_RUN_ID.into()),
            signal_name: myelin_flow::JOB_DONE_SIGNAL.into(),
            idem_key: token.into(),
            payload: SignalPayload::CiJobDone {
                stage: name.into(),
                passed: true,
                result_refs: Vec::new(),
            },
            payload_key_ref: None,
        })
    })
    .unwrap_or_else(|error| panic!("deliver completion {received_order}: {error:?}"));
}

async fn future_drive_clock(pool: &PgPool) -> (i64, String) {
    sqlx::query_as(
        "SELECT extract(epoch FROM instant)::bigint, \
                to_char(instant AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') \
         FROM (SELECT clock_timestamp() + interval '60 seconds' AS instant) clock",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn starter_manifest_drives_dag_across_worker_restarts_and_loader_retry() {
    let schema = format!("ci_manifest_dag_{}", std::process::id());
    let bare = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .unwrap();
    bare.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bare.execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    let pool = pool_on(&schema).await;
    // BUG FIX (investigation, 2026-07-25): this test's ONLY cleanup used to be the
    // `pool.close(); bare.execute(DROP SCHEMA ...)` pair at the very end of the happy path — so a
    // panicking assertion or `.unwrap()` anywhere in between (this test hit exactly that, via a
    // genuinely separate `pg_pipeline_starter.rs` "manifest check context has no run-scoped
    // allocation ledger" bug) left `ci_manifest_dag_<pid>` behind forever, the same class of leak
    // found and fixed across 21 other files today. Wrapping the body in catch_unwind + unconditional
    // cleanup + resume_unwind (mirrors `myelin-ci-sandbox`/`myelin-storage`'s sibling fixes) makes
    // cleanup run whether this test passes, fails an assertion, or panics. `pool.close()` shuts down
    // the WHOLE pool (not just this handle) for every clone, so it must run AFTER the wrapped body,
    // never inside it.
    let result = std::panic::AssertUnwindSafe(async {
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

    let register = PgFlowExecutor::new(
        pool.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(MonotonicMinter::new()),
        TenantId(TENANT.into()),
        Region(REGION.into()),
    );
    tokio::task::block_in_place(|| {
        register
            .register_definition(CI_PIPELINE_WF_TYPE, 1, BODY_HASH)
            .unwrap()
    });

    let blobs = Arc::new(FsBlobStore::new());
    let plan_bytes = plan().canonical_bytes().unwrap();
    let snapshot = blobs.put(&TenantId(TENANT.into()), &plan_bytes).unwrap();
    sqlx::query(
        "INSERT INTO ci_run (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, \
         repo_ref, commit_oid, cause_event_id, cause_depth, caused_by, definition_snapshot, \
         trigger_kind, trust_tier, state, correlation_id) \
         VALUES ($1,$2,$3::uuid,'43000000-0000-8000-8000-000000000001'::uuid, \
         '44000000-0000-8000-8000-000000000001'::uuid,$4::uuid, \
         'myelin://manifest_dag/git/repo/core','deadbeef00deadbeef00deadbeef00deadbeef00','trigger-manifest-dag',1, \
         'session:test',$5,'push','trusted','queued','corr-manifest-dag')",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(CI_RUN_ID)
    .bind(WF_RUN_ID)
    .bind(format!(
        "myelin://{TENANT}/ci/snapshot/{}",
        snapshot.to_multihash_string()
    ))
    .execute(&pool)
    .await
    .unwrap();
    reserve_test_check_attempts(
        &pool,
        CI_RUN_ID,
        "myelin://manifest_dag/git/repo/core",
        "deadbeef00deadbeef00deadbeef00deadbeef00",
    )
    .await;
    let starter = PgCiPipelineStarter::new_with_authority(
        pool.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(MonotonicMinter::new()),
        TenantId(TENANT.into()),
        Region(REGION.into()),
        blobs,
        definition(),
        Arc::new(TestAuthority),
    )
    .unwrap();
    assert!(matches!(
        starter.run_once().await.unwrap(),
        StartQueuedOutcome::Started { .. }
    ));

    let durable_store = CiJobSpecStore::with_pg(pool.clone());
    let finalizer: Arc<dyn CiRunFinalizer> = Arc::new(AcceptFinalizer);
    let mut first = worker(&pool, "worker-retry");
    let flaky = RetryOnceResolver {
        inner: resolver(&pool),
        attempts: Arc::new(AtomicUsize::new(0)),
    };
    let retry_store = durable_store.clone();
    let retry_finalizer = Arc::clone(&finalizer);
    let retry_rt = tokio::runtime::Handle::current();
    first
        .register_definition_with_input_resolver(
            CI_PIPELINE_WF_TYPE,
            1,
            BODY_HASH,
            flaky,
            move |input: &PgResolvedDriveInput, ctx| {
                let manifest = decode_resolved_ci_manifest(input)?;
                let runner = CiManifestDurableJobRunner::new(
                    Arc::new(manifest.clone()),
                    retry_store.clone(),
                    retry_rt.clone(),
                )?;
                run_ci_manifest_pipeline(ctx, &manifest, &runner, retry_finalizer.as_ref())
                    .map_err(|error| format!("manifest CI workflow failed: {error:?}"))?;
                Ok(vec![myelin_refs::ArtifactRef(manifest.run_ref)])
            },
        )
        .unwrap();
    let (first_now, first_iso) = future_drive_clock(&pool).await;
    assert!(matches!(
        first.run_once(first_now, &first_iso).await,
        Err(PgWorkerError::InputUnavailable(ref detail))
            if detail == "injected manifest-store outage"
    ));
    let unchanged: (String, i64, Option<String>, i64) = sqlx::query_as(
        "SELECT state, cursor, lease_owner, \
         (SELECT count(*) FROM wf_history WHERE tenant_id=$1 AND region=$2 AND run_id=$3) \
         FROM workflow_run WHERE tenant_id=$1 AND region=$2 AND run_id=$3",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(WF_RUN_ID)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unchanged, ("running".into(), 0, None, 0));
    drop(first);

    let mut roots_worker = worker(&pool, "worker-roots");
    register_durable_ci_manifest_pipeline(
        &mut roots_worker,
        resolver(&pool),
        durable_store.clone(),
        Arc::clone(&finalizer),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    let (roots_now, roots_iso) = future_drive_clock(&pool).await;
    assert!(matches!(
        roots_worker.run_once(roots_now, &roots_iso).await.unwrap(),
        PgRunOnceOutcome::Driven { .. }
    ));
    let ids: BTreeMap<String, String> = sqlx::query_as::<_, (String, String)>(
        "SELECT name, job_id::text FROM ci_job WHERE tenant_id=$1 AND run_id=$2::uuid",
    )
    .bind(TENANT)
    .bind(CI_RUN_ID)
    .fetch_all(&pool)
    .await
    .unwrap()
    .into_iter()
    .collect();
    let root_dispatches =
        sqlx::query_as::<_, (String, String, String, String, Vec<String>, String, String)>(
            "SELECT stage, job_id::text, idem_token, lane, labels, trust_tier, fair_key \
         FROM job_queue \
         WHERE tenant_id=$1 AND region=$2 ORDER BY stage",
        )
        .bind(TENANT)
        .bind(REGION)
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(root_dispatches.len(), 2);
    assert_eq!(
        root_dispatches
            .iter()
            .map(|(_, job_id, ..)| job_id.clone())
            .collect::<std::collections::BTreeSet<_>>(),
        [ids["build"].clone(), ids["test"].clone()]
            .into_iter()
            .collect()
    );
    for (_, _, _, lane, labels, trust_tier, fair_key) in &root_dispatches {
        assert_eq!(lane, "batch");
        assert_eq!(labels, &["linux"]);
        assert_eq!(trust_tier, "trusted");
        assert_eq!(fair_key, "project:43000000-0000-8000-8000-000000000001");
    }
    let by_stage = root_dispatches
        .into_iter()
        .map(|(stage, job_id, idem_token, ..)| (stage, (job_id, idem_token)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(by_stage["build"].0, ids["build"]);
    assert_eq!(by_stage["test"].0, ids["test"]);
    for stage in ["build", "test"] {
        let (job_id, idem_token) = &by_stage[stage];
        let launch = durable_store
            .get_launch_template(TENANT, REGION, job_id)
            .await
            .unwrap();
        let spec = &launch.spec;
        assert_eq!(spec.idem_token.0, *idem_token);
        assert_eq!(spec.trust_tier, TrustTier::Trusted);
        assert_eq!(launch.token_authority_handle, format!("mint:{CI_RUN_ID}"));
        assert_eq!(
            spec.meter_to.reserve_id,
            format!("reserve:{CI_RUN_ID}:{stage}")
        );
        assert_eq!(
            spec.workspace.repo_ref.as_deref(),
            Some("myelin://manifest_dag/git/repo/core")
        );
        assert_eq!(spec.workspace.commit.as_deref(), Some("deadbeef00deadbeef00deadbeef00deadbeef00"));
        assert!(spec.env.is_empty());
        assert!(spec.secret_refs.is_empty());
        assert!(spec.egress.allow.is_empty());
        assert_eq!(spec.limits.cpu_millis, 1_000);
        assert_eq!(spec.limits.mem_bytes, 1_073_741_824);
        assert_eq!(spec.limits.disk_bytes, 2_147_483_648);
        assert_eq!(spec.limits.pids_max, 128);
        assert_eq!(spec.limits.timeout_secs, 600);
    }
    signal(&pool, &by_stage["test"].1, "test", 1);
    signal(&pool, &by_stage["build"].1, "build", 2);
    drop(roots_worker);

    let mut dependent_worker = worker(&pool, "worker-dependent");
    register_durable_ci_manifest_pipeline(
        &mut dependent_worker,
        resolver(&pool),
        durable_store.clone(),
        Arc::clone(&finalizer),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    let (dependent_now, dependent_iso) = future_drive_clock(&pool).await;
    dependent_worker
        .run_once(dependent_now, &dependent_iso)
        .await
        .unwrap();
    let package: (String, String) = sqlx::query_as(
        "SELECT job_id::text, idem_token FROM job_queue \
         WHERE tenant_id=$1 AND region=$2 AND stage='package'",
    )
    .bind(TENANT)
    .bind(REGION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(package.0, ids["package"]);
    assert_eq!(
        durable_store
            .get_launch_template(TENANT, REGION, &package.0)
            .await
            .unwrap()
            .spec
            .command,
        vec!["/bin/package"]
    );
    signal(&pool, &package.1, "package", 3);
    drop(dependent_worker);

    let mut terminal_worker = worker(&pool, "worker-terminal");
    register_durable_ci_manifest_pipeline(
        &mut terminal_worker,
        resolver(&pool),
        durable_store.clone(),
        Arc::clone(&finalizer),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    let (terminal_now, terminal_iso) = future_drive_clock(&pool).await;
    terminal_worker
        .run_once(terminal_now, &terminal_iso)
        .await
        .unwrap();
    let dispatch_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM job_queue WHERE tenant_id=$1 AND region=$2")
            .bind(TENANT)
            .bind(REGION)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(dispatch_count, 3, "reconstruction never redispatches");

    let workflow_state: String = sqlx::query_scalar(
        "SELECT state FROM workflow_run WHERE tenant_id=$1 AND region=$2 AND run_id=$3",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(WF_RUN_ID)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(workflow_state, "completed");
    let terminal_checks: Vec<(String, i64, String, bool)> = sqlx::query_as(
        "SELECT envelope->'payload'->'context'->>'name', \
                (envelope->'payload'->>'run_attempt')::bigint, \
                envelope->'payload'->>'state', \
                (envelope->'payload'->>'cost_settled')::boolean \
         FROM outbox WHERE envelope->>'type_'='ci.check.updated' \
           AND envelope->'payload'->>'run'=$1 \
           AND envelope->'payload'->>'state'='success' \
         ORDER BY 1",
    )
    .bind(format!("myelin://{TENANT}/ci/run/{CI_RUN_ID}"))
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        terminal_checks,
        vec![
            ("build".into(), 1, "success".into(), true),
            ("package".into(), 1, "success".into(), true),
            ("test".into(), 1, "success".into(), true),
        ]
    );

    })
    .catch_unwind()
    .await;

    pool.close().await;
    if let Err(error) = bare
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
    {
        eprintln!(
            "starter_manifest_drives_dag_across_worker_restarts_and_loader_retry: DROP SCHEMA \
             {schema} CASCADE failed (schema may have leaked): {error}"
        );
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
