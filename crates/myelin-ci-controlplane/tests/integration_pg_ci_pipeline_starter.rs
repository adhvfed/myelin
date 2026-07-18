//! Live PostgreSQL proof for the exact-cell CI run starter.
#![cfg(feature = "integration")]

use std::collections::BTreeMap;
use std::sync::Arc;

use myelin_ci_controlplane::{
    ci_artifact_ref, ci_run_ref, PgCiPipelineStarter, ResolvedJobV1, ResolvedRunPlanV1,
    StartQueuedOutcome, CREATE_CI_RUN_DDL,
};
use myelin_config::MyelinConfig;
use myelin_events::MonotonicMinter;
use myelin_flow::{
    migrations::migrations as flow_migrations, DurableExecutor, PgFlowExecutor, RunId, StartSpec,
    CI_PIPELINE_WF_TYPE,
};
use myelin_refs::ArtifactRef;
use myelin_storage::{
    provider::foundation_migrations, BlobStore, FsBlobStore, HotTables, PgMigrator,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool};

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

fn plan() -> ResolvedRunPlanV1 {
    ResolvedRunPlanV1 {
        schema_version: 1,
        jobs: vec![ResolvedJobV1 {
            name: "build".into(),
            image: format!("registry.example/build@sha256:{}", "a".repeat(64)),
            command: vec!["/bin/build".into(), "--locked".into()],
            needs: Vec::new(),
            is_generator: false,
            matrix_key: BTreeMap::new(),
        }],
    }
}

fn starter(pool: &PgPool, tenant: &str, blobs: Arc<FsBlobStore>) -> PgCiPipelineStarter {
    PgCiPipelineStarter::new(
        pool.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(MonotonicMinter::new()),
        TenantId(tenant.into()),
        Region("fr-par".into()),
        blobs,
    )
    .expect("valid exact-cell starter")
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
         repo_ref, commit_oid, definition_snapshot, trigger_kind, trust_tier, state, correlation_id) \
         VALUES ($1, 'fr-par', $2::uuid, '22222222-2222-2222-2222-222222222222'::uuid, \
         '33333333-3333-3333-3333-333333333333'::uuid, $3::uuid, 'repo-1', 'deadbeef', $4, \
         'push', 'trusted', 'queued', $2)",
    )
    .bind(tenant)
    .bind(run_id)
    .bind(wf_run_id)
    .bind(snapshot)
    .execute(admin)
    .await
    .expect("insert queued ci_run");
}

async fn assert_atomic_started(
    admin: &PgPool,
    tenant: &str,
    run_id: &str,
    running: bool,
    workflow_exists: bool,
) {
    let pair: (bool, bool) = sqlx::query_as(
        "SELECT state = 'running', EXISTS (SELECT 1 FROM workflow_run w \
         WHERE w.tenant_id = c.tenant_id AND w.region = c.region AND w.run_id = c.wf_run_id::text) \
         FROM ci_run c WHERE tenant_id = $1 AND run_id = $2::uuid",
    )
    .bind(tenant)
    .bind(run_id)
    .fetch_one(admin)
    .await
    .expect("read atomic run pair");
    assert_eq!(pair, (running, workflow_exists));
}

async fn expected_input(admin: &PgPool, tenant: &str, run_id: &str) -> Vec<ArtifactRef> {
    let snapshot: String = sqlx::query_scalar(
        "SELECT definition_snapshot FROM ci_run WHERE tenant_id=$1 AND run_id=$2::uuid",
    )
    .bind(tenant)
    .bind(run_id)
    .fetch_one(admin)
    .await
    .unwrap();
    let address = snapshot
        .strip_prefix(&format!("myelin://{tenant}/ci/snapshot/"))
        .unwrap();
    vec![
        ci_artifact_ref(tenant, &format!("snapshot-{address}")),
        ci_run_ref(tenant, run_id),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exact_cell_starter_is_atomic_concurrent_restart_safe_and_rls_isolated() {
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
    let admin = pool_on(&admin_url(), &schema).await;
    PgMigrator::apply(&admin, &foundation_migrations())
        .await
        .expect("foundation migrations");
    PgMigrator::apply_validated(
        &admin,
        &flow_migrations(),
        &HotTables::declare(["workflow_run"]),
    )
    .await
    .expect("flow migrations");
    admin.execute(CREATE_CI_RUN_DDL).await.expect("ci_run DDL");
    admin
        .execute("SELECT myelin_make_tenant_scoped('ci_run')")
        .await
        .expect("force RLS on ci_run");
    admin
        .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant schema");
    admin
        .execute(format!("GRANT ALL ON ALL TABLES IN SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant tables");
    let app = pool_on(&app_url(), &schema).await;
    let blobs = Arc::new(FsBlobStore::new());

    let register = flow_executor(&admin, "tenant_a");
    tokio::task::block_in_place(|| {
        register
            .register_definition(CI_PIPELINE_WF_TYPE, 1, "blake3:ci-pg-body-v1")
            .expect("register immutable workflow definition");
    });

    // Two concurrent starters see one row. SKIP LOCKED lets one win and the other return idle;
    // there is exactly one workflow and the state transition cannot split from it.
    let run1 = "10000000-0000-0000-0000-000000000001";
    let wf1 = "20000000-0000-0000-0000-000000000001";
    insert_run(&admin, blobs.as_ref(), "tenant_a", run1, wf1).await;
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

    // A new process can recover a legacy split where the workflow exists but ci_run remains queued:
    // the idempotency winner is the pre-minted id and the restart only advances the durable row.
    let run2 = "10000000-0000-0000-0000-000000000002";
    let wf2 = "20000000-0000-0000-0000-000000000002";
    insert_run(&admin, blobs.as_ref(), "tenant_a", run2, wf2).await;
    let restart_seed = flow_executor(&app, "tenant_a");
    let restart_input = expected_input(&admin, "tenant_a", run2).await;
    tokio::task::block_in_place(|| {
        restart_seed
            .start_with_id(
                StartSpec {
                    wf_type: CI_PIPELINE_WF_TYPE.into(),
                    input: restart_input,
                    budget: None,
                    idem_key: format!("ci:{run2}"),
                },
                Some(RunId(wf2.into())),
            )
            .expect("seed pre-existing durable workflow");
    });
    let restarted = starter(&app, "tenant_a", blobs.clone());
    assert!(matches!(
        restarted.run_once().await.expect("restart pass"),
        StartQueuedOutcome::Started { ref wf_run_id, .. } if wf_run_id == wf2
    ));
    assert_atomic_started(&admin, "tenant_a", run2, true, true).await;

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
    let terminal_input = expected_input(&admin, "tenant_terminal", run8).await;
    let terminal = flow_executor(&app, "tenant_terminal");
    tokio::task::block_in_place(|| {
        terminal
            .start_with_id(
                StartSpec {
                    wf_type: CI_PIPELINE_WF_TYPE.into(),
                    input: terminal_input,
                    budget: None,
                    idem_key: format!("ci:{run8}"),
                },
                Some(RunId(wf8.into())),
            )
            .expect("seed exact workflow before terminalization");
    });
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
    assert_atomic_started(&admin, "tenant_terminal", run8, false, true).await;

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

    bare.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .ok();
}
