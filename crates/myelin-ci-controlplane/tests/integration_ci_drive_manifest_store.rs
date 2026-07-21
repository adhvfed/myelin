//! Live PostgreSQL proof for the canonical, insert-only CI drive-manifest store.
#![cfg(feature = "integration")]

use std::collections::BTreeMap;

use myelin_ci_controlplane::{
    CiDriveManifestStore, CiDriveManifestV1, CiManifestLaneV1, CiManifestLimitsV1,
    CiManifestSchedulingV1, CiManifestTrustTierV1, CiManifestWorkspaceV1, GrantedCiJobV1,
    ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL, CI_PIPELINE_WF_TYPE, CREATE_CI_DRIVE_MANIFEST_DDL,
    CREATE_CI_RUN_DDL,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn unique_schema() -> String {
    format!(
        "ci_drive_manifest_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

async fn pinned_pool(url: &str, schema: &str) -> PgPool {
    let schema = schema.to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
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
        .expect("connect to live PostgreSQL")
}

fn digest(byte: char) -> String {
    format!("blake3:{}", byte.to_string().repeat(64))
}

fn manifest() -> CiDriveManifestV1 {
    let repo_ref = "myelin://manifest-live/git/repo/core".to_string();
    CiDriveManifestV1 {
        schema_version: 1,
        tenant_id: "manifest-live".into(),
        region: "fr-par".into(),
        wf_run_id: "11111111-1111-8111-8111-111111111111".into(),
        ci_run_id: "22222222-2222-8222-8222-222222222222".into(),
        source_snapshot_ref: format!(
            "myelin://manifest-live/ci/artifact/snapshot-{}",
            digest('a')
        ),
        source_plan_schema_version: 2,
        launch_request_digest: digest('b'),
        workflow_type: CI_PIPELINE_WF_TYPE.into(),
        workflow_definition_version: 3,
        workflow_code_hash: digest('c'),
        authority_policy_revision: "ci-policy-live-v1".into(),
        repo_ref: repo_ref.clone(),
        commit_oid: "deadbeef".into(),
        run_ref: "myelin://manifest-live/ci/run/22222222-2222-8222-8222-222222222222".into(),
        started_at: "2026-07-21T12:34:56.000000Z".into(),
        trust_tier: CiManifestTrustTierV1::Trusted,
        check_attempts: BTreeMap::from([("build".into(), 9)]),
        merge_waiter: None,
        jobs: vec![GrantedCiJobV1 {
            job_id: "33333333-3333-8333-8333-333333333333".into(),
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
                disk_bytes: 2_147_483_648,
                pids_max: 128,
                timeout_secs: 600,
            },
            workspace: CiManifestWorkspaceV1 {
                repo_ref,
                commit_oid: "deadbeef".into(),
                read_only_root: true,
                tmpfs_scratch: true,
            },
            scheduling: CiManifestSchedulingV1 {
                lane: CiManifestLaneV1::Batch,
                labels: vec!["linux".into()],
                concurrency_group: None,
                fair_key: "project:core".into(),
            },
            reserve_handle: "reserve:live-run".into(),
            token_authority_handle: "mint:live-run".into(),
            continue_on_error: false,
        }],
    }
}

#[tokio::test]
async fn store_replays_exact_bytes_and_refuses_divergent_authority() {
    let bare_admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .expect("connect to dev PostgreSQL as migration role");
    let schema = unique_schema();
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema} AUTHORIZATION myelin_admin;
         GRANT USAGE ON SCHEMA {schema} TO myelin_app;
         ALTER DEFAULT PRIVILEGES FOR ROLE myelin_admin IN SCHEMA {schema}
           GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO myelin_app;"
    ))
    .execute(&bare_admin)
    .await
    .unwrap();

    let admin = pinned_pool(&admin_url(), &schema).await;
    let app = pinned_pool(&app_url(), &schema).await;
    sqlx::raw_sql(&format!(
        "{CREATE_CI_RUN_DDL};
         {ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL};
         SELECT myelin_make_tenant_scoped('ci_run');
         {CREATE_CI_DRIVE_MANIFEST_DDL};
         SELECT myelin_make_tenant_scoped('ci_drive_manifest');"
    ))
    .execute(&admin)
    .await
    .expect("apply the production run and manifest migrations");

    let expected = manifest();
    let mut parent = admin.begin().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, true)")
        .bind(&expected.tenant_id)
        .execute(&mut *parent)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', $1, true)")
        .bind(&expected.region)
        .execute(&mut *parent)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO ci_run (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id,
         repo_ref, commit_oid, definition_snapshot, trigger_kind, trust_tier, state, correlation_id)
         VALUES ($1, $2, $3::uuid, gen_random_uuid(), gen_random_uuid(), $4::uuid,
                 $5, $6, $7, 'push', 'trusted', 'queued', 'manifest-live')",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.ci_run_id)
    .bind(&expected.wf_run_id)
    .bind(&expected.repo_ref)
    .bind(&expected.commit_oid)
    .bind(&expected.source_snapshot_ref)
    .execute(&mut *parent)
    .await
    .unwrap();
    parent.commit().await.unwrap();

    let store = CiDriveManifestStore::new(
        app.clone(),
        TenantId(expected.tenant_id.clone()),
        Region(expected.region.clone()),
    )
    .unwrap();
    let first_digest = store.insert(&expected).await.unwrap();
    assert_eq!(store.insert(&expected).await.unwrap(), first_digest);
    assert_eq!(
        store
            .load_expected(&expected.wf_run_id, &expected.ci_run_id, &first_digest)
            .await
            .unwrap(),
        expected
    );

    let mut divergent = expected.clone();
    divergent.authority_policy_revision = "ci-policy-live-v2".into();
    assert!(matches!(
        store.insert(&divergent).await,
        Err(myelin_ci_controlplane::CiDriveManifestError::IdentityMismatch)
    ));
    assert!(store
        .load_expected(&expected.wf_run_id, &expected.ci_run_id, &digest('f'))
        .await
        .is_err());

    admin.close().await;
    app.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&bare_admin)
        .await
        .unwrap();
    bare_admin.close().await;
}
