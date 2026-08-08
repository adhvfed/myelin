#![cfg(feature = "integration")]

use myelin_refs_service::{
    CREATE_EDGE_INDEXES_DDL, CREATE_EDGE_TABLE_DDL, EDGE_BY_REL_INDEX, EDGE_INBOUND_INDEX,
    EDGE_OUTBOUND_INDEX, MAKE_EDGE_TENANT_SCOPED_DDL,
};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn rename(ddl: &str, tbl: &str) -> String {
    ddl.replace("EXISTS edge (", &format!("EXISTS {tbl} ("))
        .replace("ON edge (", &format!("ON {tbl} ("))
        .replace("ON edge ", &format!("ON {tbl} "))
        .replace("('edge')", &format!("('{tbl}')"))
        .replace("edge_inbound", &format!("{tbl}_inbound"))
        .replace("edge_outbound", &format!("{tbl}_outbound"))
        .replace("edge_by_rel", &format!("{tbl}_by_rel"))
}

#[tokio::test]
async fn edge_schema_applies_forward_only_with_rls_and_three_indexes() {
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
    let tbl = format!("edge_p154_{suffix}");

    let create = rename(CREATE_EDGE_TABLE_DDL, &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the edge CREATE TABLE forward-only");

    for (name, idx_ddl) in CREATE_EDGE_INDEXES_DDL {
        let idx = rename(idx_ddl, &tbl);
        sqlx::query(&idx)
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("apply index {name}: {e}"));
    }

    let rls = rename(MAKE_EDGE_TENANT_SCOPED_DDL, &tbl);
    sqlx::query(&rls)
        .execute(&admin)
        .await
        .expect("the edge table is made tenant-scoped (RLS)");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant the app role");

    let idx_rows = sqlx::query(
        "SELECT indexname, indexdef FROM pg_indexes WHERE tablename = $1 ORDER BY indexname",
    )
    .bind(&tbl)
    .fetch_all(&admin)
    .await
    .expect("read pg_indexes for the edge table");
    let defs: std::collections::HashMap<String, String> = idx_rows
        .iter()
        .map(|r| {
            (
                r.get::<String, _>("indexname"),
                r.get::<String, _>("indexdef"),
            )
        })
        .collect();

    let inbound = rename(EDGE_INBOUND_INDEX, &tbl);
    let outbound = rename(EDGE_OUTBOUND_INDEX, &tbl);
    let by_rel = rename(EDGE_BY_REL_INDEX, &tbl);

    let inbound_def = defs
        .get(&inbound)
        .unwrap_or_else(|| panic!("edge_inbound index exists: {defs:?}"));
    assert!(
        inbound_def.contains("target_root"),
        "edge_inbound keys target_root: {inbound_def}"
    );
    assert!(
        inbound_def
            .to_lowercase()
            .contains("where (not tombstoned)")
            || inbound_def.to_lowercase().contains("where not tombstoned"),
        "edge_inbound is live-edges-only (WHERE NOT tombstoned): {inbound_def}"
    );

    let outbound_def = defs
        .get(&outbound)
        .unwrap_or_else(|| panic!("edge_outbound index exists: {defs:?}"));
    assert!(
        outbound_def.contains("source_root"),
        "edge_outbound keys source_root: {outbound_def}"
    );
    assert!(
        !outbound_def.to_lowercase().contains("where"),
        "edge_outbound has no partial predicate: {outbound_def}"
    );

    let by_rel_def = defs
        .get(&by_rel)
        .unwrap_or_else(|| panic!("edge_by_rel index exists: {defs:?}"));
    assert!(
        by_rel_def.contains("target_root") && by_rel_def.contains("rel"),
        "edge_by_rel keys (target_root, rel): {by_rel_def}"
    );
    assert!(
        by_rel_def
            .to_lowercase()
            .contains("rel_class = 'lifecycle'"),
        "edge_by_rel is lifecycle-class only: {by_rel_def}"
    );

    for (tenant, edge_id) in [("tenantA", "e-aaa"), ("tenantB", "e-bbb")] {
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
               (tenant_id, region, edge_id, source, source_root, target, target_root, rel, \
                rel_class, origin_event, origin_actor, created_at, zookie, tombstoned, dek_ref) \
             VALUES ($1, 'fr-par', $2, 'myelin://t/chat/message/m1', 'myelin://t/chat/message/m1', \
                'myelin://t/issues/issue/ENG-1', 'myelin://t/issues/issue/ENG-1', 'mentions', \
                'reference', 'evt-1', 'principal-opaque-1', now(), 'zk-1', false, \
                'kms://t/0/tenant')"
        ))
        .bind(tenant)
        .bind(edge_id)
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
    let rows = sqlx::query(&format!("SELECT tenant_id, edge_id FROM {tbl}"))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "RLS must hide the other tenant's edge (no cross-tenant query path)"
    );
    assert_eq!(rows[0].get::<String, _>("tenant_id"), "tenantA");
    assert_eq!(rows[0].get::<String, _>("edge_id"), "e-aaa");

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
        let dup = sqlx::query(&format!(
            "INSERT INTO {tbl} \
               (tenant_id, region, edge_id, source, source_root, target, target_root, rel, \
                rel_class, origin_event, origin_actor, created_at, zookie, tombstoned, dek_ref) \
             VALUES ('tenantA', 'fr-par', 'e-DIFFERENT', 'myelin://t/chat/message/m1', \
                'myelin://t/chat/message/m1', 'myelin://t/issues/issue/ENG-1', \
                'myelin://t/issues/issue/ENG-1', 'mentions', 'reference', 'evt-2', \
                'principal-opaque-1', now(), 'zk-2', false, 'kms://t/0/tenant')"
        ))
        .execute(&mut *conn)
        .await;
        assert!(
            dup.is_err(),
            "the UNIQUE (tenant_id, source, target, rel) key rejects a duplicate edge \
             (the idempotency backstop the deterministic edge_id rebuild relies on)"
        );
    }

    assert!(
        !CREATE_EDGE_TABLE_DDL.to_ascii_uppercase().contains("DROP"),
        "the edge schema migration is forward-only (no DROP in the create DDL)"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
