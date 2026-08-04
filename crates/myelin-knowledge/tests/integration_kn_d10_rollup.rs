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
async fn kn_d10_read_time_rollup_permission_filtered_conjoin_zero_leak() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    let row_tbl = format!("db_row_p308_{suffix}");
    let rel_tbl = format!("db_relation_p308_{suffix}");
    let av_tbl = format!("authz_visible_p308_{suffix}");

    sqlx::query(&format!(
        "CREATE TABLE {row_tbl} (\
           tenant text NOT NULL, id text NOT NULL, props jsonb NOT NULL, \
           PRIMARY KEY (tenant, id))"
    ))
    .execute(&admin)
    .await
    .expect("create the target db_row table");
    sqlx::query(&format!(
        "CREATE TABLE {rel_tbl} (\
           tenant text NOT NULL, src_row text NOT NULL, dst_ref text NOT NULL, rel text NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create the db_relation table");
    sqlx::query(&format!(
        "CREATE TABLE {av_tbl} (\
           tenant text NOT NULL, subject text NOT NULL, relation text NOT NULL, object_id text NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create the authz_visible reverse index table");

    let targets: &[(&str, i64)] = &[("t:1", 10), ("t:2", 20), ("t:secret", 100_000)];
    for (id, amount) in targets {
        sqlx::query(&format!(
            "INSERT INTO {row_tbl} (tenant, id, props) VALUES ('acme', $1, $2::jsonb)"
        ))
        .bind(id)
        .bind(serde_json::json!({ "amount": amount }))
        .execute(&admin)
        .await
        .expect("seed a target row");
        sqlx::query(&format!(
            "INSERT INTO {rel_tbl} (tenant, src_row, dst_ref, rel) VALUES ('acme', 'src:1', $1, 'rollup_source')"
        ))
        .bind(id)
        .execute(&admin)
        .await
        .expect("seed a rollup_source edge");
    }
    let viewer = Principal::stub(
        PrincipalId("p:viewer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    for object in ["t:1", "t:2"] {
        sqlx::query(&format!(
            "INSERT INTO {av_tbl} (tenant, subject, relation, object_id) VALUES ('acme', 'p:viewer', 'read', $1)"
        ))
        .bind(object)
        .execute(&admin)
        .await
        .expect("grant read of a visible target");
    }

    let lowered_acl = lower_over_db_row_id(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: db_row_id_colref(),
        },
        &viewer,
    );
    assert_eq!(
        lowered_acl.joins.len(),
        1,
        "the InRelation lowers to ONE JOIN (no N+1 over the related set)"
    );
    let join_clause = lowered_acl.joins[0]
        .clause
        .replace(AUTHZ_VISIBLE_TABLE, &av_tbl)
        .replace("db_row.id", "r.id")
        .replace(":subject_0", "'p:viewer'")
        .replace(":rel_for_read", "'read'");
    let acl_pred = lowered_acl.sql_predicate;

    let rollup_sql = format!(
        "SELECT COUNT(*)::bigint AS n, COALESCE(SUM((r.props ->> 'amount')::bigint), 0)::bigint AS total, \
                COALESCE(MAX((r.props ->> 'amount')::bigint), 0)::bigint AS hi \
         FROM {rel_tbl} e \
         JOIN {row_tbl} r ON r.tenant = e.tenant AND r.id = e.dst_ref \
         {join_clause} \
         WHERE e.tenant = 'acme' AND e.src_row = 'src:1' AND e.rel = 'rollup_source' \
           AND ({acl_pred})"
    );
    let row = sqlx::query(&rollup_sql)
        .fetch_one(&admin)
        .await
        .unwrap_or_else(|e| panic!("the ONE rollup query runs: {e}\nSQL: {rollup_sql}"));
    let n: i64 = row.get("n");
    let total: i64 = row.get("total");
    let hi: i64 = row.get("hi");

    assert_eq!(
        n, 2,
        "0 rollup leak: COUNT = 2 (the visible targets), NOT 3 (t:secret uncounted)"
    );
    assert_eq!(
        total, 30,
        "0 rollup leak: SUM = 30 (10+20), NOT 100030 (t:secret unsummed)"
    );
    assert_eq!(
        hi, 20,
        "0 rollup leak: MAX = 20 (visible), NOT 100000 (t:secret's value not disclosed)"
    );

    for object in ["t:1", "t:2", "t:secret"] {
        sqlx::query(&format!(
            "INSERT INTO {av_tbl} (tenant, subject, relation, object_id) VALUES ('acme', 'p:admin', 'read', $1)"
        ))
        .bind(object)
        .execute(&admin)
        .await
        .expect("grant the admin read of every target");
    }
    let admin_join = lowered_acl.joins[0]
        .clause
        .replace(AUTHZ_VISIBLE_TABLE, &av_tbl)
        .replace("db_row.id", "r.id")
        .replace(":subject_0", "'p:admin'")
        .replace(":rel_for_read", "'read'");
    let admin_sql = format!(
        "SELECT COALESCE(SUM((r.props ->> 'amount')::bigint), 0)::bigint AS total \
         FROM {rel_tbl} e \
         JOIN {row_tbl} r ON r.tenant = e.tenant AND r.id = e.dst_ref \
         {admin_join} \
         WHERE e.tenant = 'acme' AND e.src_row = 'src:1' AND e.rel = 'rollup_source' \
           AND ({acl_pred})"
    );
    let admin_total: i64 = sqlx::query(&admin_sql)
        .fetch_one(&admin)
        .await
        .expect("admin rollup runs")
        .get("total");
    assert_eq!(
        admin_total, 100_030,
        "the authorized viewer sees the full SUM (per-viewer conjoin, not a blanket hide)"
    );

    println!(
        "[P-308 INTEGRATION GREEN] KN-D10 read-time rollup PROVEN against live Postgres: a SUM/COUNT/MAX \
         over the db_relation rollup_source edges of src:1, JOINED to db_row for the amount, CONJOINED \
         with the list_objects SetExpr ACL (authz_visible JOIN over db_row.id) in ONE query → the viewer \
         sees COUNT=2 SUM=30 MAX=20 (0 rollup leak: t:secret's 100000 never counted/summed/maxed); the \
         authorized admin sees SUM=100030 (per-viewer conjoin, not a blanket hide). Computed at READ TIME, \
         never stored (KN-3)."
    );

    sqlx::query(&format!("DROP TABLE {row_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {rel_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {av_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
