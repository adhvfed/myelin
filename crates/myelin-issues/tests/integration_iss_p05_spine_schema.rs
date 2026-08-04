#![cfg(feature = "integration")]

use myelin_issues::{CREATE_ISSUE_DDL, CREATE_ISSUE_INDEXES_DDL, CREATE_ISSUE_RELATION_DDL};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn rename(ddl: &str, base: &str, tbl: &str) -> String {
    ddl.replace(&format!("EXISTS {base} ("), &format!("EXISTS {tbl} ("))
        .replace(&format!("ON {base} ("), &format!("ON {tbl} ("))
        .replace(&format!("('{base}')"), &format!("('{tbl}')"))
}

#[tokio::test]
async fn issue_spine_applies_forward_only_with_rls_and_check_vocab() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the app role");

    let suffix = std::process::id();
    let tbl = format!("issue_p371_{suffix}");

    let create = rename(CREATE_ISSUE_DDL, "issue", &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the issue CREATE TABLE forward-only");

    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{tbl}')"))
        .execute(&admin)
        .await
        .expect("the issue table is made tenant-scoped (RLS)");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant the app role");

    for (tenant, id) in [
        ("tenantA", "11111111-1111-1111-1111-111111111111"),
        ("tenantB", "22222222-2222-2222-2222-222222222222"),
    ] {
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
               (tenant_id, region, id, key, prefix, type_id, type_rank, state, state_category, \
                reporter, project_id, rank, title, version) \
             VALUES ($1, 'fr-par', $2::uuid, $3, 'ENG', gen_random_uuid(), 0, 'Todo', 'unstarted', \
                gen_random_uuid(), gen_random_uuid(), '0|hzzzzz:', 'fix the login bug', 1)"
        ))
        .bind(tenant)
        .bind(id)
        .bind(format!("ENG-{}", if tenant == "tenantA" { 1 } else { 2 }))
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
        "RLS must hide the other tenant's issue (no cross-tenant query path)"
    );
    assert_eq!(rows[0].get::<String, _>("tenant_id"), "tenantA");

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
        let bad = sqlx::query(&format!(
            "INSERT INTO {tbl} \
               (tenant_id, region, id, key, prefix, type_id, type_rank, state, state_category, \
                reporter, project_id, rank, title, version) \
             VALUES ('tenantA', 'fr-par', gen_random_uuid(), 'ENG-3', 'ENG', gen_random_uuid(), 0, \
                'Todo', 'WIDE_OPEN', gen_random_uuid(), gen_random_uuid(), '0|i00000:', 't', 1)"
        ))
        .execute(&mut *conn)
        .await;
        assert!(
            bad.is_err(),
            "the state_category CHECK rejects an out-of-vocabulary value ('WIDE_OPEN') - the frozen \
             four-category invariant is enforced by Postgres"
        );
    }

    let rel_tbl = format!("issue_relation_p371_{suffix}");
    let rel_create = rename(CREATE_ISSUE_RELATION_DDL, "issue_relation", &rel_tbl).replace(
        "REFERENCES issue(tenant_id, id)",
        &format!("REFERENCES {tbl}(tenant_id, id)"),
    );
    sqlx::query(&rel_create)
        .execute(&admin)
        .await
        .expect("apply the issue_relation CREATE TABLE forward-only (FK on src_issue)");
    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{rel_tbl}')"))
        .execute(&admin)
        .await
        .expect("issue_relation is made tenant-scoped");
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
        let orphan = sqlx::query(&format!(
            "INSERT INTO {rel_tbl} (tenant_id, region, relation_id, src_issue, dst_ref, rel, created_by) \
             VALUES ('tenantA', 'fr-par', gen_random_uuid(), gen_random_uuid(), \
                'myelin://tenantA/issue/issue/ENG-9', 'blocks', gen_random_uuid())"
        ))
        .execute(&mut *conn)
        .await;
        assert!(
            orphan.is_err(),
            "the issue_relation FK rejects a forward edge whose src_issue does not exist (TE-7 integrity)"
        );
        sqlx::query(&format!(
            "INSERT INTO {rel_tbl} (tenant_id, region, relation_id, src_issue, dst_ref, rel, created_by) \
             VALUES ('tenantA', 'fr-par', gen_random_uuid(), \
                '11111111-1111-1111-1111-111111111111'::uuid, \
                'myelin://tenantA/issue/issue/ENG-2', 'blocks', gen_random_uuid())"
        ))
        .execute(&mut *conn)
        .await
        .expect("a valid forward edge anchored on an existing src_issue is accepted");
        let bad_rel = sqlx::query(&format!(
            "INSERT INTO {rel_tbl} (tenant_id, region, relation_id, src_issue, dst_ref, rel, created_by) \
             VALUES ('tenantA', 'fr-par', gen_random_uuid(), \
                '11111111-1111-1111-1111-111111111111'::uuid, \
                'myelin://tenantA/issue/issue/ENG-2', 'frobnicates', gen_random_uuid())"
        ))
        .execute(&mut *conn)
        .await;
        assert!(
            bad_rel.is_err(),
            "the rel CHECK rejects an out-of-vocabulary relation ('frobnicates')"
        );
    }

    let board_ddl = CREATE_ISSUE_INDEXES_DDL
        .iter()
        .find(|(n, _)| *n == "issue_board")
        .map(|(_, d)| *d)
        .unwrap();
    let board_idx = board_ddl
        .replace("ON issue (", &format!("ON {tbl} ("))
        .replace("issue_board", &format!("issue_board_{suffix}"));
    sqlx::query(&board_idx)
        .execute(&admin)
        .await
        .expect("apply the issue_board index CONCURRENTLY");
    let row =
        sqlx::query("SELECT indexdef FROM pg_indexes WHERE tablename = $1 AND indexname = $2")
            .bind(&tbl)
            .bind(format!("issue_board_{suffix}"))
            .fetch_one(&admin)
            .await
            .expect("the issue_board index exists");
    let def: String = row.get("indexdef");
    assert!(
        def.contains("project_id") && def.contains("state_category") && def.contains("rank"),
        "issue_board keys (tenant, project, state_category, rank): {def}"
    );
    assert!(
        def.to_lowercase().contains("deleted_at is null"),
        "issue_board is the live-only board scan (soft-delete predicate): {def}"
    );

    assert!(
        !CREATE_ISSUE_DDL.to_ascii_uppercase().contains("DROP"),
        "the issue schema migration is forward-only (no DROP)"
    );

    sqlx::query(&format!("DROP TABLE {rel_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
