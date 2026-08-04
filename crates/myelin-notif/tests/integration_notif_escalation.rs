#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_notif::migrations::{rls_scope_sql, ESCALATION_RUN_DDL};

#[tokio::test]
async fn notif_escalation_run_round_trips_state_check_and_rls_denies_cross_tenant() {
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

    let run_tbl = format!("notif_escalation_run_p192_{}", std::process::id());
    let run_ddl = ESCALATION_RUN_DDL.replacen("notif_escalation_run", &run_tbl, 1);

    sqlx::query(&format!("DROP TABLE IF EXISTS {run_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&run_ddl)
        .execute(&admin)
        .await
        .expect("the notif_escalation_run DDL applies");
    sqlx::query(&rls_scope_sql(&run_tbl))
        .execute(&admin)
        .await
        .expect("RLS policy installs");
    sqlx::query(&format!("GRANT ALL ON {run_tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .unwrap();

    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();

    let insert = format!(
        "INSERT INTO {run_tbl} \
         (tenant_id, region, run_id, policy_id, trigger_event, workflow_ref, current_step, state, dek_ref) \
         VALUES ('tenantA', 'fr-par', 'esc-run-1', 'sla-chain', 'myelin://tenantA/issues/issue/7', \
                 'wf://flow/esc-run-1', 0, 'active', 'kms://tenantA/0/tenant')"
    );
    sqlx::query(&insert)
        .execute(&mut *conn)
        .await
        .expect("the escalation_run handle inserts");

    let row = sqlx::query(&format!(
        "SELECT current_step, state FROM {run_tbl} WHERE run_id='esc-run-1'"
    ))
    .fetch_one(&mut *conn)
    .await
    .expect("the durable handle reads back");
    assert_eq!(
        row.get::<i32, _>("current_step"),
        0,
        "the resume cursor starts at step 0"
    );
    assert_eq!(
        row.get::<String, _>("state"),
        "active",
        "the run is active (chain walking)"
    );

    let ack = format!(
        "UPDATE {run_tbl} SET state='acked', acked_by='psn:alice', acked_at=now() WHERE run_id='esc-run-1'"
    );
    sqlx::query(&ack)
        .execute(&mut *conn)
        .await
        .expect("the ack transitions the run");
    let acked = sqlx::query(&format!(
        "SELECT state, acked_by FROM {run_tbl} WHERE run_id='esc-run-1'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        acked.get::<String, _>("state"),
        "acked",
        "the run HALTED on the ack"
    );
    assert_eq!(
        acked.get::<String, _>("acked_by"),
        "psn:alice",
        "the acker is recorded on the handle"
    );

    let bad = format!(
        "INSERT INTO {run_tbl} \
         (tenant_id, region, run_id, policy_id, trigger_event, workflow_ref, state, dek_ref) \
         VALUES ('tenantA', 'fr-par', 'esc-bad', 'p', 'e', 'w', 'paused', 'k')"
    );
    let err = sqlx::query(&bad).execute(&mut *conn).await;
    assert!(
        err.is_err(),
        "an off-grammar state ('paused') is rejected by the CHECK constraint"
    );
    drop(conn);

    let mut conn_b = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantB', false)")
        .execute(&mut *conn_b)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn_b)
        .await
        .unwrap();
    let cross: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {run_tbl}"))
        .fetch_one(&mut *conn_b)
        .await
        .unwrap()
        .get("n");
    assert_eq!(
        cross, 0,
        "RLS denies cross-tenant: tenant B reads 0 of tenant A's escalation runs"
    );

    sqlx::query(&format!("DROP TABLE IF EXISTS {run_tbl}"))
        .execute(&admin)
        .await
        .ok();
}
