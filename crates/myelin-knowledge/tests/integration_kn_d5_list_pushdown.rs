#![cfg(feature = "integration")]

use myelin_identity::{Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_knowledge::{db_row_id_colref, lower_over_db_row_id, AUTHZ_VISIBLE_TABLE};
use myelin_tenancy::TenantId;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn kn_d5_db_row_list_and_count_setexpr_join_zero_leak_zero_count_leak_revoke_reflected() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? run `fed test:backend`)");

    let suffix = std::process::id();
    let row_tbl = format!("db_row_p306_{suffix}");
    let av_tbl = format!("authz_visible_p306_{suffix}");

    sqlx::query(&format!(
        "CREATE TABLE {row_tbl} (\
           tenant text NOT NULL, region text NOT NULL, db_id text NOT NULL, id text NOT NULL, \
           PRIMARY KEY (tenant, id))"
    ))
    .execute(&admin)
    .await
    .expect("create the db_row table");
    sqlx::query(&format!(
        "CREATE TABLE {av_tbl} (\
           tenant text NOT NULL, subject text NOT NULL, relation text NOT NULL, \
           object_id text NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create the authz_visible reverse index table");

    for (tenant, db_id, id) in [
        ("acme", "db:projects", "row:1"),
        ("acme", "db:projects", "row:2"),
        ("acme", "db:projects", "row:secret"),
        ("acme", "db:other", "row:otherdb"),
        ("evilcorp", "db:projects", "row:x"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {row_tbl} (tenant, region, db_id, id) VALUES ($1, 'fr-par', $2, $3)"
        ))
        .bind(tenant)
        .bind(db_id)
        .bind(id)
        .execute(&admin)
        .await
        .expect("seed a db_row");
    }

    let viewer = Principal::stub(
        PrincipalId("p:viewer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    for (subject, object) in [
        ("p:viewer", "row:1"),
        ("p:viewer", "row:2"),
        ("p:other", "row:secret"),
        ("p:viewer", "row:otherdb"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {av_tbl} (tenant, subject, relation, object_id) \
             VALUES ('acme', $1, 'read', $2)"
        ))
        .bind(subject)
        .bind(object)
        .execute(&admin)
        .await
        .expect("grant read");
    }

    let lowered = lower_over_db_row_id(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: db_row_id_colref(),
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
        .replace("db_row.id", &format!("{row_tbl}.id"))
        .replace(":subject_0", "'p:viewer'")
        .replace(":rel_for_read", "'read'");
    let predicate = lowered.sql_predicate;

    let list_sql = format!(
        "SELECT {row_tbl}.id FROM {row_tbl} {join_clause} \
         WHERE {row_tbl}.tenant = 'acme' AND {row_tbl}.db_id = 'db:projects' AND ({predicate}) \
         ORDER BY {row_tbl}.id LIMIT 50"
    );
    let rows = sqlx::query(&list_sql)
        .fetch_all(&admin)
        .await
        .unwrap_or_else(|e| panic!("the ONE db-list query runs: {e}\nSQL: {list_sql}"));

    let ids: Vec<String> = rows.iter().map(|r| r.get::<String, _>("id")).collect();
    assert_eq!(
        ids,
        vec!["row:1".to_string(), "row:2".to_string()],
        "exactly the 2 visible rows (0 leak): {ids:?}"
    );
    assert!(
        !ids.iter().any(|i| i == "row:secret"),
        "0 leak: the confidential row is ABSENT (the SetExpr JOIN excluded it)"
    );
    assert!(
        !ids.iter().any(|i| i == "row:otherdb"),
        "no-cross-db: the db_id predicate excluded the other db's row (despite a grant)"
    );
    assert!(
        !ids.iter().any(|i| i == "row:x"),
        "0 cross-tenant: the tenant predicate excluded evilcorp's row"
    );

    let count_sql = format!(
        "SELECT COUNT(*) AS n FROM {row_tbl} {join_clause} \
         WHERE {row_tbl}.tenant = 'acme' AND {row_tbl}.db_id = 'db:projects' AND ({predicate})"
    );
    let count: i64 = sqlx::query(&count_sql)
        .fetch_one(&admin)
        .await
        .unwrap_or_else(|e| panic!("the ONE COUNT query runs: {e}\nSQL: {count_sql}"))
        .get::<i64, _>("n");
    assert_eq!(
        count, 2,
        "0 count-leak: the permission-correct COUNT is 2 (the granted rows in this db), NOT 3 (would leak row:secret's existence)"
    );
    assert_eq!(
        count as usize,
        ids.len(),
        "the COUNT equals the listed cardinality - the ACL is the SAME conjunct, no second path can diverge"
    );

    let plan = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {list_sql}"))
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN the db-list query");
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
    let count_plan = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {count_sql}"))
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN the COUNT query");
    let count_plan_text: String = count_plan
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        count_plan_text.to_lowercase().contains("aggregate"),
        "the COUNT is a single aggregate over the JOINed/filtered set (ACL inside the aggregate): {count_plan_text}"
    );

    sqlx::query(&format!(
        "DELETE FROM {av_tbl} WHERE tenant = 'acme' AND subject = 'p:viewer' AND relation = 'read' AND object_id = 'row:1'"
    ))
    .execute(&admin)
    .await
    .expect("revoke read of row:1");
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
        vec!["row:2".to_string()],
        "the just-revoked row:1 drops out (read-your-writes): {ids_after:?}"
    );
    let count_after: i64 = sqlx::query(&count_sql)
        .fetch_one(&admin)
        .await
        .expect("re-run the COUNT after revoke")
        .get::<i64, _>("n");
    assert_eq!(count_after, 1, "0 count-leak after revoke: the COUNT decremented to 1 - a revoked grant cannot be counted stale");

    println!(
        "[P-306 INTEGRATION GREEN] KN-D5 db-row-list + permission-correct COUNT SetExpr push-down \
         PROVEN against live Postgres: 2 visible of 5 rows → ONE JOIN query over db_row.id \
         (0 leak: row:secret + cross-db + cross-tenant absent); COUNT = 2 not 4 (0 count-leak, ACL \
         inside the aggregate); EXPLAIN shows one join plan + one aggregate (no per-row check); a \
         revoke drops row:1 from the SAME view AND decrements the COUNT to 1 (read-your-writes)."
    );

    sqlx::query(&format!("DROP TABLE {row_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {av_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
