#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    CREATE_CI_DRIVE_MANIFEST_DDL, CREATE_CI_RUN_DDL, CREATE_JOB_QUEUE_DDL,
    CREATE_JOB_QUEUE_INDEXES_DDL, JQ_CLAIMABLE_INDEX,
};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn rename(ddl: &str, base: &str, tbl: &str) -> String {
    ddl.replace(&format!("EXISTS {base} ("), &format!("EXISTS {tbl} ("))
        .replace(&format!("ON {base} ("), &format!("ON {tbl} ("))
        .replace(&format!("('{base}')"), &format!("('{tbl}')"))
}

#[tokio::test]
async fn ci_run_schema_applies_forward_only_with_rls() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? run `fed test:backend`)");
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the app role");

    let suffix = std::process::id();
    let tbl = format!("ci_run_p349_{suffix}");

    let create = rename(CREATE_CI_RUN_DDL, "ci_run", &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the ci_run CREATE TABLE forward-only");

    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{tbl}')"))
        .execute(&admin)
        .await
        .expect("the ci_run table is made tenant-scoped (RLS)");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant the app role");

    for (tenant, run_id) in [
        ("tenantA", "11111111-1111-1111-1111-111111111111"),
        ("tenantB", "22222222-2222-2222-2222-222222222222"),
    ] {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
            .bind(tenant)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(&format!(
            "INSERT INTO {tbl} \
               (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, definition_snapshot, \
                trigger_kind, triggered_by, trust_tier, state, correlation_id) \
             VALUES ($1, 'fr-par', $2::uuid, gen_random_uuid(), gen_random_uuid(), gen_random_uuid(), \
                'blake3:snap', 'push', 'psn:actor-8a2f', 'trusted', 'queued', 'corr-1')"
        ))
        .bind(tenant)
        .bind(run_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    let rows = sqlx::query(&format!("SELECT tenant_id FROM {tbl}"))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "RLS must hide the other tenant's ci_run (no cross-tenant query path)"
    );
    assert_eq!(rows[0].get::<String, _>("tenant_id"), "tenantA");

    {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        let bad = sqlx::query(&format!(
            "INSERT INTO {tbl} \
               (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, definition_snapshot, \
                trigger_kind, trust_tier, state, correlation_id) \
             VALUES ('tenantA', 'fr-par', gen_random_uuid(), gen_random_uuid(), gen_random_uuid(), \
                gen_random_uuid(), 'blake3:snap', 'push', 'WIDE_OPEN', 'queued', 'corr-2')"
        ))
        .execute(&mut *conn)
        .await;
        assert!(
            bad.is_err(),
            "the trust_tier CHECK rejects an out-of-vocabulary value ('WIDE_OPEN') - the frozen \
             three-tier vocabulary is enforced by Postgres"
        );
    }

    assert!(
        !CREATE_CI_RUN_DDL.to_ascii_uppercase().contains("DROP"),
        "the ci_run schema migration is forward-only (no DROP)"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
async fn ci_drive_manifest_is_live_insert_only_replay_authority() {
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin");
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the app role");

    let suffix = std::process::id();
    let run_tbl = format!("ci_run_manifest_p349_{suffix}");
    let manifest_tbl = format!("ci_drive_manifest_p349_{suffix}");
    sqlx::query(&rename(CREATE_CI_RUN_DDL, "ci_run", &run_tbl))
        .execute(&admin)
        .await
        .expect("create manifest parent run table");
    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{run_tbl}')"))
        .execute(&admin)
        .await
        .expect("scope parent run table");

    let create_manifest = CREATE_CI_DRIVE_MANIFEST_DDL
        .replace(
            "EXISTS ci_drive_manifest (",
            &format!("EXISTS {manifest_tbl} ("),
        )
        .replace("REFERENCES ci_run(", &format!("REFERENCES {run_tbl}("))
        .replace(
            "ON ci_drive_manifest FROM myelin_app",
            &format!("ON {manifest_tbl} FROM myelin_app"),
        )
        .replace(
            "BEFORE UPDATE OR DELETE ON ci_drive_manifest",
            &format!("BEFORE UPDATE OR DELETE ON {manifest_tbl}"),
        );
    sqlx::raw_sql(&create_manifest)
        .execute(&admin)
        .await
        .expect("create immutable manifest table and trigger");
    sqlx::query(&format!(
        "SELECT myelin_make_tenant_scoped('{manifest_tbl}')"
    ))
    .execute(&admin)
    .await
    .expect("scope manifest table");

    let tenant = "manifest-tenant";
    let region = "fr-par";
    let ci_run_id = "11111111-1111-1111-1111-111111111111";
    let wf_run_id = "22222222-2222-2222-2222-222222222222";
    let mut admin_conn = admin.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
        .bind(tenant)
        .execute(&mut *admin_conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', $1, false)")
        .bind(region)
        .execute(&mut *admin_conn)
        .await
        .unwrap();
    sqlx::query(&format!(
        "INSERT INTO {run_tbl} (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, \
         definition_snapshot, trigger_kind, trust_tier, state, correlation_id) VALUES \
         ($1, $2, $3::uuid, gen_random_uuid(), gen_random_uuid(), $4::uuid, \
         'myelin://manifest-tenant/ci/artifact/snapshot-blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', \
         'push', 'trusted', 'queued', 'manifest-proof')"
    ))
    .bind(tenant)
    .bind(region)
    .bind(ci_run_id)
    .bind(wf_run_id)
    .execute(&mut *admin_conn)
    .await
    .expect("seed parent CI run");
    drop(admin_conn);

    let mut app_conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
        .bind(tenant)
        .execute(&mut *app_conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', $1, false)")
        .bind(region)
        .execute(&mut *app_conn)
        .await
        .unwrap();
    let digest = format!("blake3:{}", "b".repeat(64));
    sqlx::query(&format!(
        "INSERT INTO {manifest_tbl} (tenant_id, region, wf_run_id, ci_run_id, schema_version, \
         source_snapshot_ref, manifest_digest, manifest_bytes) \
         VALUES ($1, $2, $3::uuid, $4::uuid, 1, 'snapshot-ref', $5, $6)"
    ))
    .bind(tenant)
    .bind(region)
    .bind(wf_run_id)
    .bind(ci_run_id)
    .bind(&digest)
    .bind(br#"{"schema_version":1}"#.as_slice())
    .execute(&mut *app_conn)
    .await
    .expect("the app role may insert immutable replay authority");

    let app_update = sqlx::query(&format!(
        "UPDATE {manifest_tbl} SET manifest_digest = $1 WHERE tenant_id = $2"
    ))
    .bind(&digest)
    .bind(tenant)
    .execute(&mut *app_conn)
    .await;
    assert!(
        app_update.is_err(),
        "the runtime app role has no manifest UPDATE capability"
    );
    drop(app_conn);

    let admin_update = sqlx::query(&format!(
        "UPDATE {manifest_tbl} SET manifest_digest = $1 WHERE tenant_id = $2"
    ))
    .bind(&digest)
    .bind(tenant)
    .execute(&admin)
    .await;
    assert!(admin_update
        .expect_err("the defensive trigger rejects even an owner mutation")
        .to_string()
        .contains("ci_drive_manifest is immutable"));

    sqlx::query(&format!("DROP TABLE {manifest_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {run_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
async fn job_queue_claim_index_applies_concurrently_with_its_predicate() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin");

    let suffix = std::process::id();
    let tbl = format!("job_queue_p349_{suffix}");

    let create = rename(CREATE_JOB_QUEUE_DDL, "job_queue", &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the job_queue CREATE TABLE forward-only");

    for (name, idx_ddl) in CREATE_JOB_QUEUE_INDEXES_DDL {
        let idx = idx_ddl.replace("ON job_queue (", &format!("ON {tbl} ("));
        let idx = idx.replace("jq_", &format!("jq_{suffix}_"));
        sqlx::query(&idx)
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("apply index {name} CONCURRENTLY: {e}"));
    }

    let claim_name = format!("jq_{suffix}_claimable");
    let _ = JQ_CLAIMABLE_INDEX;
    let row =
        sqlx::query("SELECT indexdef FROM pg_indexes WHERE tablename = $1 AND indexname = $2")
            .bind(&tbl)
            .bind(&claim_name)
            .fetch_one(&admin)
            .await
            .expect("the jq_claimable index exists");
    let def: String = row.get("indexdef");
    assert!(
        def.contains("region") && def.contains("lane") && def.contains("enqueued_at"),
        "jq_claimable keys (region, lane, enqueued_at): {def}"
    );
    assert!(
        def.to_lowercase().contains("where (state = 'queued'")
            || def.to_lowercase().contains("where state = 'queued'"),
        "jq_claimable is queued-only (the claim surface): {def}"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
