#![cfg(feature = "integration")]

use myelin_identity::{Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_issues::planner::{issue_id_colref, lower_over_issue_id, AUTHZ_VISIBLE_TABLE};
use myelin_tenancy::TenantId;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn view_set_expr() -> SetExpr {
    let in_rel = |r: &str| SetExpr::InRelation {
        relation: RelName(r.into()),
        via_column: issue_id_colref(),
    };
    SetExpr::Union(vec![
        SetExpr::Difference(Box::new(in_rel("read")), Box::new(in_rel("confidential"))),
        in_rel("confidential_grant"),
    ])
}

#[tokio::test]
async fn iss_d3_board_setexpr_join_one_query_zero_leak_confidential_and_cross_tenant() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? run `fed test:backend`)");

    let suffix = std::process::id();
    let issue_tbl = format!("issue_p379_{suffix}");
    let av_tbl = format!("authz_visible_p379_{suffix}");

    sqlx::query(&format!(
        "CREATE TABLE {issue_tbl} (\
           tenant_id text NOT NULL, region text NOT NULL, id text NOT NULL, \
           rank text NOT NULL, title text NOT NULL, PRIMARY KEY (tenant_id, id))"
    ))
    .execute(&admin)
    .await
    .expect("create the issue table");
    sqlx::query(&format!(
        "CREATE TABLE {av_tbl} (\
           tenant_id text NOT NULL, subject text NOT NULL, relation text NOT NULL, \
           object_id text NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create the authz_visible reverse index table");

    for (tenant, id, rank, title) in [
        ("acme", "ENG-1", "U", "open one"),
        ("acme", "ENG-2", "V", "granted confidential"),
        ("acme", "ENG-3", "W", "CONFIDENTIAL no grant"),
        ("evilcorp", "ENG-X", "U", "cross-tenant"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {issue_tbl} (tenant_id, region, id, rank, title) VALUES ($1, 'fr-par', $2, $3, $4)"
        ))
        .bind(tenant)
        .bind(id)
        .bind(rank)
        .bind(title)
        .execute(&admin)
        .await
        .expect("seed an issue");
    }

    let viewer = Principal::stub(
        PrincipalId("p:viewer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    for (subject, relation, object) in [
        ("p:viewer", "read", "ENG-1"),
        ("p:viewer", "read", "ENG-2"),
        ("p:viewer", "read", "ENG-3"),
        ("p:viewer", "confidential", "ENG-2"),
        ("p:viewer", "confidential", "ENG-3"),
        ("p:viewer", "confidential_grant", "ENG-2"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {av_tbl} (tenant_id, subject, relation, object_id) VALUES ('acme', $1, $2, $3)"
        ))
        .bind(subject)
        .bind(relation)
        .bind(object)
        .execute(&admin)
        .await
        .expect("grant a tuple");
    }

    let lowered = lower_over_issue_id(&view_set_expr(), &viewer);
    assert_eq!(
        lowered.joins.len(),
        3,
        "the view set-difference lowers to 3 JOINs (read/confidential/confidential_grant), no N+1"
    );

    let mut join_clauses = String::new();
    for j in &lowered.joins {
        let clause = j
            .clause
            .replace(AUTHZ_VISIBLE_TABLE, &av_tbl)
            .replace("issue.id", &format!("{issue_tbl}.id"))
            .replace(":subject_0", "'p:viewer'")
            .replace(":subject_1", "'p:viewer'")
            .replace(":subject_2", "'p:viewer'")
            .replace(":rel_for_read", "'read'")
            .replace(":rel_for_confidential_grant", "'confidential_grant'")
            .replace(":rel_for_confidential", "'confidential'");
        join_clauses.push(' ');
        join_clauses.push_str(&clause);
    }
    let predicate = lowered.sql_predicate.clone();

    let join_clauses_left = join_clauses.replace("JOIN", "LEFT JOIN");
    let list_sql = format!(
        "SELECT {issue_tbl}.id FROM {issue_tbl}{join_clauses_left} \
         WHERE {issue_tbl}.tenant_id = 'acme' AND {issue_tbl}.region = 'fr-par' AND ({predicate}) \
         ORDER BY {issue_tbl}.rank LIMIT 50"
    );
    let rows = sqlx::query(&list_sql)
        .fetch_all(&admin)
        .await
        .unwrap_or_else(|e| panic!("the ONE board query runs: {e}\nSQL: {list_sql}"));

    let ids: Vec<String> = rows.iter().map(|r| r.get::<String, _>("id")).collect();
    assert_eq!(
        ids,
        vec!["ENG-1".to_string(), "ENG-2".to_string()],
        "exactly the 2 visible issues (0 leak): {ids:?}"
    );
    assert!(
        !ids.iter().any(|i| i == "ENG-3"),
        "0 leak: the confidential issue (no grant) is ABSENT (the set-difference JOIN excluded it)"
    );
    assert!(
        !ids.iter().any(|i| i == "ENG-X"),
        "0 cross-tenant: the tenant predicate excluded evilcorp's issue"
    );

    let plan = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {list_sql}"))
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN the board query");
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
        "DELETE FROM {av_tbl} WHERE tenant_id = 'acme' AND subject = 'p:viewer' AND relation = 'confidential_grant' AND object_id = 'ENG-2'"
    ))
    .execute(&admin)
    .await
    .expect("revoke the confidential_grant on ENG-2");
    let rows_after = sqlx::query(&list_sql)
        .fetch_all(&admin)
        .await
        .expect("re-run the board after revoke");
    let ids_after: Vec<String> = rows_after
        .iter()
        .map(|r| r.get::<String, _>("id"))
        .collect();
    assert_eq!(
        ids_after,
        vec!["ENG-1".to_string()],
        "the just-revoked ENG-2 drops out - the confidential issue is absent again (revoke reflected): {ids_after:?}"
    );

    println!(
        "[P-379 INTEGRATION GREEN] ISS-D3 board SetExpr push-down PROVEN against live Postgres: \
         2 visible of 4 issues → ONE JOIN query over issue.id (0 leak: confidential ENG-3 + \
         cross-tenant ENG-X absent; EXPLAIN shows one join plan, no per-row check); revoking the \
         confidential_grant drops ENG-2 from the SAME query (the new-enemy guard)."
    );

    sqlx::query(&format!("DROP TABLE {issue_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {av_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
