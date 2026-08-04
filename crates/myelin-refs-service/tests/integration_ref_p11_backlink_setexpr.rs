#![cfg(feature = "integration")]

use myelin_identity::{Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_refs_service::{lower_over_source_root, source_root_colref};
use myelin_tenancy::TenantId;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn backlink_setexpr_join_is_one_query_leak_free_tenant_scoped_paginated() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    let edge_tbl = format!("edge_p160_{suffix}");
    let av_tbl = format!("authz_visible_p160_{suffix}");

    sqlx::query(&format!(
        "CREATE TABLE {edge_tbl} (\
           tenant_id text NOT NULL, region text NOT NULL, edge_id text NOT NULL, \
           source text NOT NULL, source_root text NOT NULL, target text NOT NULL, \
           target_root text NOT NULL, rel text NOT NULL, rel_class text NOT NULL, \
           created_at timestamptz NOT NULL, tombstoned boolean NOT NULL DEFAULT false, \
           PRIMARY KEY (tenant_id, edge_id))"
    ))
    .execute(&admin)
    .await
    .expect("create the edge table");
    sqlx::query(&format!(
        "CREATE TABLE {av_tbl} (\
           tenant_id text NOT NULL, subject text NOT NULL, relation text NOT NULL, \
           object_id text NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create the authz_visible reverse index table");

    let target_root = "myelin://acme/issue/issue/PUBLIC-1";
    for (eid, src, tenant, secs) in [
        (
            "e-secret",
            "myelin://acme/issue/issue/SECRET-9",
            "acme",
            10_i64,
        ),
        ("e-public", "myelin://acme/issue/issue/OPEN-2", "acme", 20),
        (
            "e-crosstenant",
            "myelin://evilcorp/issue/issue/X-1",
            "evilcorp",
            30,
        ),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {edge_tbl} (tenant_id, region, edge_id, source, source_root, target, \
               target_root, rel, rel_class, created_at, tombstoned) \
             VALUES ($1, 'fr-par', $2, $3, $3, $4, $4, 'mentions', 'reference', \
               now() - ($5 || ' seconds')::interval, false)"
        ))
        .bind(tenant)
        .bind(eid)
        .bind(src)
        .bind(target_root)
        .bind(secs.to_string())
        .execute(&admin)
        .await
        .expect("seed an edge");
    }

    let viewer = Principal::stub(
        PrincipalId("p:viewer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    sqlx::query(&format!(
        "INSERT INTO {av_tbl} (tenant_id, subject, relation, object_id) \
         VALUES ('acme', 'p:viewer', 'view', 'myelin://acme/issue/issue/OPEN-2')"
    ))
    .execute(&admin)
    .await
    .expect("grant view of the public source");

    let lowered = lower_over_source_root(
        &SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
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
        .replace("authz_visible", &av_tbl)
        .replace("edge.source_root", &format!("{edge_tbl}.source_root"))
        .replace(":subject_0", "'p:viewer'")
        .replace(":rel_for_view", "'view'");
    let predicate = lowered.sql_predicate;

    let sql = format!(
        "SELECT {edge_tbl}.source FROM {edge_tbl} {join_clause} \
         WHERE {edge_tbl}.tenant_id = 'acme' AND {edge_tbl}.target_root = $1 \
           AND NOT {edge_tbl}.tombstoned AND ({predicate}) \
         ORDER BY {edge_tbl}.created_at DESC LIMIT 50"
    );
    let rows = sqlx::query(&sql)
        .bind(target_root)
        .fetch_all(&admin)
        .await
        .unwrap_or_else(|e| panic!("the ONE backlink query runs: {e}\nSQL: {sql}"));

    let sources: Vec<String> = rows.iter().map(|r| r.get::<String, _>("source")).collect();
    assert_eq!(
        sources.len(),
        1,
        "exactly the ONE authorized backlink (0 leak): {sources:?}"
    );
    assert_eq!(
        sources[0], "myelin://acme/issue/issue/OPEN-2",
        "the public referrer is present"
    );
    assert!(
        !sources.iter().any(|s| s.contains("SECRET")),
        "0 leak: the confidential referrer is ABSENT (the SetExpr JOIN excluded it)"
    );
    assert!(
        !sources.iter().any(|s| s.contains("evilcorp")),
        "0 cross-tenant: the tenant predicate excluded tenant B's edge (no cross-tenant query path)"
    );

    let plan = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {sql}"))
        .bind(target_root)
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN the backlink query");
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

    sqlx::query(&format!("DROP TABLE {edge_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {av_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
