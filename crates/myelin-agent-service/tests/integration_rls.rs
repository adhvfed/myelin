#![cfg(feature = "integration")]

use myelin_agent_service::migrations::{rls_scope_sql, RUN_DDL};
use myelin_config::MyelinConfig;

#[tokio::test]
async fn agent_run_rls_denies_cross_tenant_reads() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("connect to dev Postgres as the app role (is the stack up?)");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(
            &cfg.database_url
                .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"),
        )
        .await
        .expect("connect as admin");

    let tbl = format!("agent_run_rls_probe_{}", std::process::id());
    let create = RUN_DDL.replacen("agent_run", &tbl, 2);

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("the run-table DDL applies");
    sqlx::query(&rls_scope_sql(&tbl))
        .execute(&admin)
        .await
        .expect("myelin_make_tenant_scoped installs the (tenant_id, region) RLS policy");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .unwrap();

    for (run_id, t) in [(1i64, "tenantA"), (2i64, "tenantB")] {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
            .bind(t)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(&format!(
            "INSERT INTO {tbl} \
               (tenant_id, region, run_id, agent_principal, on_behalf_of, binding_id, \
                trigger_event, correlation_id, causation_id, depth, runtime_ref, state, \
                reservation_id, budget, trace_ref) \
             VALUES ($1, 'fr-par', $2, 'psn:agent', 'psn:human', 0, 'evt', 'corr', 'cause', 0, \
                     'skeleton', 'running', 'rsv', 0, NULL)"
        ))
        .bind(t)
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
        "RLS must hide the other tenant's run - 0 cross-tenant rows"
    );
    assert_eq!(rows[0].get::<String, _>("tenant_id"), "tenantA");

    let cross: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {tbl} WHERE tenant_id = 'tenantB'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        cross, 0,
        "a tenant-A session must read 0 cross-tenant (tenantB) rows"
    );

    let mut conn_b = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantB', false)")
        .execute(&mut *conn_b)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn_b)
        .await
        .unwrap();
    let rows_b = sqlx::query(&format!("SELECT tenant_id FROM {tbl}"))
        .fetch_all(&mut *conn_b)
        .await
        .unwrap();
    assert_eq!(rows_b.len(), 1, "tenant B sees only its own run");
    assert_eq!(rows_b[0].get::<String, _>("tenant_id"), "tenantB");

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
