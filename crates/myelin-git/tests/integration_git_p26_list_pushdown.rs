#![cfg(feature = "integration")]

use myelin_git::list_filter::{lower_over_pr_id, AUTHZ_VISIBLE_TABLE};
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
async fn git_d11_pr_list_setexpr_join_one_query_zero_leak_tenant_scoped_revoke_reflected() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? run `fed test:backend`)");

    let suffix = std::process::id();
    let pr_tbl = format!("pr_p288_{suffix}");
    let av_tbl = format!("authz_visible_p288_{suffix}");

    sqlx::query(&format!(
        "CREATE TABLE {pr_tbl} (\
           tenant_id text NOT NULL, region text NOT NULL, id text NOT NULL, \
           title text NOT NULL, PRIMARY KEY (tenant_id, id))"
    ))
    .execute(&admin)
    .await
    .expect("create the pr table");
    sqlx::query(&format!(
        "CREATE TABLE {av_tbl} (\
           tenant_id text NOT NULL, subject text NOT NULL, relation text NOT NULL, \
           object_id text NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create the authz_visible reverse index table");

    for (tenant, id, title) in [
        ("acme", "pr:1", "visible one"),
        ("acme", "pr:2", "visible two"),
        ("acme", "pr:secret", "CONFIDENTIAL"),
        ("evilcorp", "pr:x", "cross-tenant"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {pr_tbl} (tenant_id, region, id, title) VALUES ($1, 'fr-par', $2, $3)"
        ))
        .bind(tenant)
        .bind(id)
        .bind(title)
        .execute(&admin)
        .await
        .expect("seed a pr");
    }

    let viewer = Principal::stub(
        PrincipalId("p:viewer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    for (subject, object) in [
        ("p:viewer", "pr:1"),
        ("p:viewer", "pr:2"),
        ("p:other", "pr:secret"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {av_tbl} (tenant_id, subject, relation, object_id) \
             VALUES ('acme', $1, 'view', $2)"
        ))
        .bind(subject)
        .bind(object)
        .execute(&admin)
        .await
        .expect("grant view");
    }

    let lowered = lower_over_pr_id(
        &SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: myelin_git::list_filter::pr_id_colref(),
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
        .replace("pr.id", &format!("{pr_tbl}.id"))
        .replace(":subject_0", "'p:viewer'")
        .replace(":rel_for_view", "'view'");
    let predicate = lowered.sql_predicate;

    let list_sql = format!(
        "SELECT {pr_tbl}.id FROM {pr_tbl} {join_clause} \
         WHERE {pr_tbl}.tenant_id = 'acme' AND {pr_tbl}.region = 'fr-par' AND ({predicate}) \
         ORDER BY {pr_tbl}.id LIMIT 50"
    );
    let rows = sqlx::query(&list_sql)
        .fetch_all(&admin)
        .await
        .unwrap_or_else(|e| panic!("the ONE PR-list query runs: {e}\nSQL: {list_sql}"));

    let ids: Vec<String> = rows.iter().map(|r| r.get::<String, _>("id")).collect();
    assert_eq!(
        ids,
        vec!["pr:1".to_string(), "pr:2".to_string()],
        "exactly the 2 visible PRs (0 leak): {ids:?}"
    );
    assert!(
        !ids.iter().any(|i| i == "pr:secret"),
        "0 leak: the confidential PR is ABSENT (the SetExpr JOIN excluded it)"
    );
    assert!(
        !ids.iter().any(|i| i == "pr:x"),
        "0 cross-tenant: the tenant predicate excluded evilcorp's PR"
    );

    let plan = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {list_sql}"))
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN the PR-list query");
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
        "DELETE FROM {av_tbl} WHERE tenant_id = 'acme' AND subject = 'p:viewer' AND relation = 'view' AND object_id = 'pr:1'"
    ))
    .execute(&admin)
    .await
    .expect("revoke view of pr:1");
    let rows_after = sqlx::query(&list_sql)
        .fetch_all(&admin)
        .await
        .expect("re-run the list after revoke");
    let ids_after: Vec<String> = rows_after
        .iter()
        .map(|r| r.get::<String, _>("id"))
        .collect();
    assert_eq!(
        ids_after,
        vec!["pr:2".to_string()],
        "the just-revoked pr:1 drops out (revoke reflected): {ids_after:?}"
    );

    println!(
        "[P-288 INTEGRATION GREEN] GIT-D11 PR-list SetExpr push-down PROVEN against live Postgres: \
         2 visible of 4 PRs → ONE JOIN query over pr.id (0 leak: pr:secret + cross-tenant absent; \
         EXPLAIN shows one join plan, no per-row check); a revoke drops pr:1 from the SAME query."
    );

    sqlx::query(&format!("DROP TABLE {pr_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {av_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
