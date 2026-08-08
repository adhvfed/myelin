#![cfg(feature = "integration")]

use myelin_search::SEARCH_INDEX_DIR_MIGRATION;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn rename(ddl: &str, tbl: &str) -> String {
    ddl.replace("search_index_directory", tbl)
}

#[tokio::test]
async fn index_directory_migration_applies_forward_only_with_per_tenant_pk() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? run `fed test:backend`)");

    let suffix = std::process::id();
    let tbl = format!("search_index_directory_p166_{suffix}");

    let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await;

    assert!(
        !myelin_substrate::is_destructive(SEARCH_INDEX_DIR_MIGRATION),
        "the per-tenant index-directory migration is forward-only (a CREATE, never a DROP)"
    );
    let create = rename(SEARCH_INDEX_DIR_MIGRATION, &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the per-tenant index-directory CREATE forward-only against live Postgres");

    let pk_cols: Vec<String> = sqlx::query(
        "SELECT a.attname AS col \
         FROM pg_index i \
         JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) \
         WHERE i.indrelid = $1::regclass AND i.indisprimary \
         ORDER BY a.attnum",
    )
    .bind(&tbl)
    .fetch_all(&admin)
    .await
    .expect("read the primary key columns")
    .iter()
    .map(|r| r.get::<String, _>("col"))
    .collect();
    assert_eq!(
        pk_cols,
        vec!["tenant".to_string(), "region".to_string()],
        "the per-tenant index directory is keyed by (tenant, region) - residency-pinned, §3.4"
    );

    sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant, region, index_dek_ref) VALUES \
         ('acme', 'fr-par', 'kms://acme/0/tenant'), \
         ('globex', 'fr-par', 'kms://globex/0/tenant')"
    ))
    .execute(&admin)
    .await
    .expect("insert two distinct per-tenant index directories");

    let rows = sqlx::query(&format!(
        "SELECT tenant, index_dek_ref FROM {tbl} ORDER BY tenant"
    ))
    .fetch_all(&admin)
    .await
    .expect("read the two directory rows");
    assert_eq!(rows.len(), 2, "two distinct per-tenant index directories");
    assert_eq!(rows[0].get::<String, _>("tenant"), "acme");
    assert_eq!(
        rows[0].get::<String, _>("index_dek_ref"),
        "kms://acme/0/tenant",
        "the directory carries the per-tenant index DEK ref (the encrypted-from-birth anchor, §3.1)"
    );
    assert_eq!(rows[1].get::<String, _>("tenant"), "globex");

    let dup = sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant, region, index_dek_ref) VALUES ('acme', 'fr-par', 'kms://acme/0/tenant')"
    ))
    .execute(&admin)
    .await;
    assert!(
        dup.is_err(),
        "a duplicate (tenant, region) index directory is rejected by the PRIMARY KEY \
         (exactly one per-tenant, per-region index directory)"
    );

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .expect("drop the throwaway test table");
}
