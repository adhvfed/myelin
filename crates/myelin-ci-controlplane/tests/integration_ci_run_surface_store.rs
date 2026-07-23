//! Live CT-005 proof for the durable CI run read authority.
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};

use myelin_ci_controlplane::surfacing_store::{
    CiRunPageRequest, CiRunStateFilter, CiRunSurfaceError,
};
use myelin_ci_controlplane::{
    ci_controlplane_migrations, CiRunStore, CI_RUN_SURFACE_REPO_CREATED_INDEX,
};
use myelin_storage::{with_tenant_tx, PgError};
use sqlx::{Executor, PgPool};

const TENANT: &str = "ci_surface";
const OTHER_TENANT: &str = "ci_surface_other";
const REGION: &str = "eu-north";
const ALPHA: &str = "myelin://ci_surface/git/repo/alpha";
const BETA: &str = "myelin://ci_surface/git/repo/beta";
const HIDDEN: &str = "myelin://ci_surface/git/repo/hidden";
const RUN_1: &str = "71000000-0000-4000-8000-000000000001";
const RUN_2: &str = "71000000-0000-4000-8000-000000000002";
const RUN_3: &str = "71000000-0000-4000-8000-000000000003";
const RUN_HIDDEN: &str = "71000000-0000-4000-8000-000000000004";
const RUN_NEWER: &str = "71000000-0000-4000-8000-000000000005";
const JOB: &str = "72000000-0000-4000-8000-000000000001";

static SCHEMA_SEQ: AtomicU64 = AtomicU64::new(0);

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    std::env::var("DATABASE_MIGRATION_URL").unwrap_or_else(|_| {
        app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
    })
}

fn schema_name() -> String {
    format!(
        "ci_surface_{}_{}",
        std::process::id(),
        SCHEMA_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

async fn pool(url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _| {
            let schema = schema.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("connect to dev Postgres (is the stack up?)")
}

async fn setup_schema(admin: &PgPool, schema: &str) {
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop stale isolated schema");
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated schema");

    for migration in ci_controlplane_migrations().0.iter().filter(|migration| {
        matches!(
            migration.id,
            "ci_0001_ci_run"
                | "ci_0002_ci_job"
                | "ci_0008_log_anchor"
                | "ci_0018d_ci_run_surface_repo_created"
        )
    }) {
        admin
            .execute(migration.ddl)
            .await
            .unwrap_or_else(|error| panic!("apply {}: {error}", migration.id));
    }
    admin
        .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant isolated schema usage");
    admin
        .execute(
            format!("GRANT SELECT, INSERT ON ALL TABLES IN SCHEMA {schema} TO myelin_app").as_str(),
        )
        .await
        .expect("grant read fixture access");
}

async fn insert_run(
    app: &PgPool,
    tenant: &str,
    run: &str,
    repo_ref: &str,
    state: &str,
    created_at: &str,
) {
    let tenant_owned = tenant.to_owned();
    let run_owned = run.to_owned();
    let repo_owned = repo_ref.to_owned();
    let state_owned = state.to_owned();
    let created_owned = created_at.to_owned();
    with_tenant_tx(app, tenant, REGION, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO ci_run (
                   tenant_id, region, run_id, project_id, repo_ref, commit_oid, pipeline_id,
                   wf_run_id, definition_snapshot, trigger_kind, trust_tier, state,
                   cost_settled, correlation_id, created_at, finished_at
                 ) VALUES (
                   $1, $2, $3::uuid, '73000000-0000-4000-8000-000000000001'::uuid, $4,
                   '0123456789abcdef', '74000000-0000-4000-8000-000000000001'::uuid,
                   '75000000-0000-4000-8000-000000000001'::uuid, 'cas:test', 'push',
                   'trusted', $5, $5 IN ('succeeded', 'failed'), $3, $6::timestamptz,
                   CASE WHEN $5 IN ('succeeded', 'failed') THEN $6::timestamptz ELSE NULL END
                 )",
            )
            .bind(tenant_owned)
            .bind(REGION)
            .bind(run_owned)
            .bind(repo_owned)
            .bind(state_owned)
            .bind(created_owned)
            .execute(&mut *conn)
            .await
            .map(|_| ())
            .map_err(|error| PgError::Query(error.to_string()))
        })
    })
    .await
    .expect("insert tenant-scoped CI run");
}

async fn insert_detail(app: &PgPool) {
    with_tenant_tx(app, TENANT, REGION, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO ci_job (
                   tenant_id, region, job_id, run_id, stage, name, needs, matrix_key,
                   spec_ref, state, attempt, result_summary
                 ) VALUES (
                   $1, $2, $3::uuid, $4::uuid, 'build', 'test', '{}', '{\"os\":\"linux\"}',
                   'cas:spec', 'failed', 2, '{\"exit_code\":1}'
                 )",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(JOB)
            .bind(RUN_2)
            .execute(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
            sqlx::query(
                "INSERT INTO log_anchor (
                   tenant_id, region, run_id, job_id, step_id, byte_start, byte_end, status
                 ) VALUES ($1, $2, $3::uuid, $4::uuid, 'compile', 12, 48, 'failed')",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(RUN_2)
            .bind(JOB)
            .execute(&mut *conn)
            .await
            .map(|_| ())
            .map_err(|error| PgError::Query(error.to_string()))
        })
    })
    .await
    .expect("insert tenant-scoped CI detail");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_list_and_detail_are_visibility_scoped_keyset_and_rls_safe() {
    let schema = schema_name();
    let admin = pool(&admin_url(), &schema).await;
    setup_schema(&admin, &schema).await;
    let app = pool(&app_url(), &schema).await;
    let cursor_key = myelin_storage::SealKey::from_bytes([0x61; 32])
        .derive_service_key("myelin test ci run surface cursor v1");
    let store = CiRunStore::with_pg_surface_cursor_key(app.clone(), cursor_key);

    insert_run(
        &app,
        TENANT,
        RUN_1,
        ALPHA,
        "succeeded",
        "2026-07-24T10:00:00Z",
    )
    .await;
    insert_run(&app, TENANT, RUN_2, BETA, "failed", "2026-07-24T11:00:00Z").await;
    insert_run(
        &app,
        TENANT,
        RUN_3,
        ALPHA,
        "running",
        "2026-07-24T12:00:00Z",
    )
    .await;
    insert_run(
        &app,
        TENANT,
        RUN_HIDDEN,
        HIDDEN,
        "failed",
        "2026-07-24T13:00:00Z",
    )
    .await;
    insert_run(
        &app,
        OTHER_TENANT,
        RUN_1,
        "myelin://ci_surface_other/git/repo/alpha",
        "failed",
        "2026-07-24T14:00:00Z",
    )
    .await;
    insert_detail(&app).await;

    let visible = vec![BETA.to_owned(), ALPHA.to_owned(), ALPHA.to_owned()];
    let first = store
        .list_surface_runs(
            TENANT,
            REGION,
            &visible,
            CiRunPageRequest::new(CiRunStateFilter::All, 2, None).unwrap(),
        )
        .await
        .expect("first visible page");
    assert_eq!(
        first
            .items
            .iter()
            .map(|run| run.run_id.as_str())
            .collect::<Vec<_>>(),
        [RUN_3, RUN_2]
    );
    let cursor = first.next_cursor.expect("one older visible run remains");

    insert_run(
        &app,
        TENANT,
        RUN_NEWER,
        ALPHA,
        "running",
        "2026-07-24T15:00:00Z",
    )
    .await;
    let second = store
        .list_surface_runs(
            TENANT,
            REGION,
            &visible,
            CiRunPageRequest::new(CiRunStateFilter::All, 2, Some(cursor.clone())).unwrap(),
        )
        .await
        .expect("keyset continuation");
    assert_eq!(
        second
            .items
            .iter()
            .map(|run| run.run_id.as_str())
            .collect::<Vec<_>>(),
        [RUN_1]
    );
    assert!(second.next_cursor.is_none());
    assert!(
        first.items.iter().all(|run| run.run_id != RUN_HIDDEN)
            && second.items.iter().all(|run| run.run_id != RUN_HIDDEN),
        "a non-visible parent repository never enters pagination"
    );

    let stale = store
        .list_surface_runs(
            TENANT,
            REGION,
            &[ALPHA.to_owned()],
            CiRunPageRequest::new(CiRunStateFilter::All, 2, Some(cursor.clone())).unwrap(),
        )
        .await
        .expect_err("visibility-set changes stale the cursor");
    assert_eq!(stale, CiRunSurfaceError::CursorStale);
    let stale = store
        .list_surface_runs(
            TENANT,
            REGION,
            &visible,
            CiRunPageRequest::new(CiRunStateFilter::Failed, 2, Some(cursor)).unwrap(),
        )
        .await
        .expect_err("state-filter changes stale the cursor");
    assert_eq!(stale, CiRunSurfaceError::CursorStale);

    let detail = store
        .get_surface_run(TENANT, REGION, RUN_2, BETA)
        .await
        .expect("detail read")
        .expect("visible tenant run exists");
    assert_eq!(detail.run.repo_ref, BETA);
    assert_eq!(detail.jobs.len(), 1);
    assert_eq!(detail.jobs[0].name, "test");
    assert_eq!(detail.jobs[0].needs, Vec::<String>::new());
    assert_eq!(detail.steps.len(), 1);
    assert_eq!(detail.steps[0].step_id, "compile");
    assert_eq!(detail.steps[0].byte_end, Some(48));

    assert!(
        store
            .get_surface_run(TENANT, REGION, RUN_HIDDEN, HIDDEN)
            .await
            .expect("same-tenant hidden row is an internal store fact")
            .is_some(),
        "Edge, not the transport-agnostic store, owns parent-repository authorization"
    );
    assert!(
        store
            .get_surface_run(OTHER_TENANT, REGION, RUN_2, BETA)
            .await
            .expect("cross-tenant read returns no row")
            .is_none(),
        "explicit scope and FORCE RLS hide another tenant's run"
    );

    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let raced_store = store
        .clone()
        .with_surface_detail_test_barrier(barrier.clone());
    let snapshot = raced_store.get_surface_run(TENANT, REGION, RUN_2, BETA);
    let move_parent = async {
        barrier.wait().await;
        sqlx::query(
            "UPDATE ci_run
                SET repo_ref = $1
              WHERE tenant_id = $2 AND region = $3 AND run_id = $4::uuid",
        )
        .bind(HIDDEN)
        .bind(TENANT)
        .bind(REGION)
        .bind(RUN_2)
        .execute(&admin)
        .await
        .expect("move the parent during detail materialization");
        barrier.wait().await;
    };
    let (snapshot, ()) = tokio::join!(snapshot, move_parent);
    let snapshot = snapshot
        .expect("repeatable-read detail snapshot")
        .expect("authorized parent existed at the snapshot instant");
    assert_eq!(snapshot.run.repo_ref, BETA);
    assert_eq!(snapshot.jobs.len(), 1);
    assert_eq!(snapshot.steps.len(), 1);
    assert!(
        store
            .get_surface_run(TENANT, REGION, RUN_2, BETA)
            .await
            .expect("repo-bound detail read")
            .is_none(),
        "the returned detail is atomically bound to the exact authorized parent repository"
    );

    let index_ready: bool = sqlx::query_scalar(
        "SELECT i.indisready AND i.indisvalid
           FROM pg_index i
           JOIN pg_class c ON c.oid = i.indexrelid
          WHERE c.relname = $1",
    )
    .bind(CI_RUN_SURFACE_REPO_CREATED_INDEX)
    .fetch_one(&admin)
    .await
    .expect("inspect run-list index");
    assert!(index_ready);

    app.close().await;
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop isolated schema");
    admin.close().await;
}
