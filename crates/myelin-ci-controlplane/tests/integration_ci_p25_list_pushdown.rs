#![cfg(feature = "integration")]

use myelin_ci_controlplane::{ci_run_id_colref, lower_over_run_id, AUTHZ_VISIBLE_TABLE};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_tenancy::TenantId;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn ci_run_list_setexpr_join_one_query_zero_leak_tenant_scoped_revoke_reflected() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? run `fed test:backend`)");

    let suffix = std::process::id();
    let run_tbl = format!("ci_run_p368_{suffix}");
    let av_tbl = format!("authz_visible_p368_{suffix}");

    sqlx::query(&format!(
        "CREATE TABLE {run_tbl} (\
           tenant_id text NOT NULL, region text NOT NULL, run_id text NOT NULL, \
           pipeline text NOT NULL, PRIMARY KEY (tenant_id, run_id))"
    ))
    .execute(&admin)
    .await
    .expect("create the ci_run table");
    sqlx::query(&format!(
        "CREATE TABLE {av_tbl} (\
           tenant_id text NOT NULL, subject text NOT NULL, relation text NOT NULL, \
           object_id text NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create the authz_visible reverse index table");

    for (tenant, run_id, pipeline) in [
        ("acme", "run:1", "ci"),
        ("acme", "run:2", "ci"),
        ("acme", "run:secret", "deploy-prod"),
        ("evilcorp", "run:x", "ci"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {run_tbl} (tenant_id, region, run_id, pipeline) VALUES ($1, 'fr-par', $2, $3)"
        ))
        .bind(tenant)
        .bind(run_id)
        .bind(pipeline)
        .execute(&admin)
        .await
        .expect("seed a run");
    }

    let viewer = Principal::stub(
        PrincipalId("p:viewer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    for (subject, object) in [
        ("p:viewer", "run:1"),
        ("p:viewer", "run:2"),
        ("p:other", "run:secret"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {av_tbl} (tenant_id, subject, relation, object_id) \
             VALUES ('acme', $1, 'read', $2)"
        ))
        .bind(subject)
        .bind(object)
        .execute(&admin)
        .await
        .expect("grant read");
    }

    let lowered = lower_over_run_id(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: ci_run_id_colref(),
        },
        &viewer,
    );
    assert_eq!(
        lowered.joins.len(),
        1,
        "the InRelation lowers to ONE JOIN (no N+1)"
    );
    let join_clause = lowered.joins[0]
        .clause
        .replace(AUTHZ_VISIBLE_TABLE, &av_tbl)
        .replace("ci_run.run_id", &format!("{run_tbl}.run_id"))
        .replace(":subject_0", "'p:viewer'")
        .replace(":rel_for_read", "'read'");
    let predicate = lowered.sql_predicate;

    let list_sql = format!(
        "SELECT {run_tbl}.run_id FROM {run_tbl} {join_clause} \
         WHERE {run_tbl}.tenant_id = 'acme' AND {run_tbl}.region = 'fr-par' AND ({predicate}) \
         ORDER BY {run_tbl}.run_id LIMIT 50"
    );
    let rows = sqlx::query(&list_sql)
        .fetch_all(&admin)
        .await
        .unwrap_or_else(|e| panic!("the ONE run-list query runs: {e}\nSQL: {list_sql}"));

    let ids: Vec<String> = rows.iter().map(|r| r.get::<String, _>("run_id")).collect();
    assert_eq!(
        ids,
        vec!["run:1".to_string(), "run:2".to_string()],
        "exactly the 2 visible runs (0 leak): {ids:?}"
    );
    assert!(
        !ids.iter().any(|i| i == "run:secret"),
        "0 leak: the confidential run is ABSENT (the SetExpr JOIN excluded it)"
    );
    assert!(
        !ids.iter().any(|i| i == "run:x"),
        "0 cross-tenant: the tenant predicate excluded evilcorp's run"
    );

    let plan = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {list_sql}"))
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN the run-list query");
    let plan_text: String = plan
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan_text.to_lowercase().contains("join")
            || plan_text.to_lowercase().contains("nested loop"),
        "the read is ONE join query (no per-row check loop): {plan_text}"
    );

    sqlx::query(&format!(
        "DELETE FROM {av_tbl} WHERE tenant_id = 'acme' AND subject = 'p:viewer' AND relation = 'read' AND object_id = 'run:1'"
    ))
    .execute(&admin)
    .await
    .expect("revoke read of run:1");
    let rows_after = sqlx::query(&list_sql)
        .fetch_all(&admin)
        .await
        .expect("re-run the list after revoke");
    let ids_after: Vec<String> = rows_after
        .iter()
        .map(|r| r.get::<String, _>("run_id"))
        .collect();
    assert_eq!(
        ids_after,
        vec!["run:2".to_string()],
        "the just-revoked run:1 drops out (revoke reflected): {ids_after:?}"
    );

    sqlx::query(&format!("DROP TABLE {run_tbl}"))
        .execute(&admin)
        .await
        .ok();
    sqlx::query(&format!("DROP TABLE {av_tbl}"))
        .execute(&admin)
        .await
        .ok();
}
