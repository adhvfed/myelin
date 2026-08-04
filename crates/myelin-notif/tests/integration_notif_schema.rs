#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_notif::migrations::{rls_scope_sql, INBOX_ITEM_DDL};

#[tokio::test]
async fn notif_inbox_item_rls_denies_cross_tenant_and_dedup_unique_bites() {
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

    let tbl = format!("notif_inbox_item_rls_probe_{}", std::process::id());
    let create = INBOX_ITEM_DDL.replacen("notif_inbox_item", &tbl, 1);

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("the inbox_item DDL applies");
    sqlx::query(&rls_scope_sql(&tbl))
        .execute(&admin)
        .await
        .expect("myelin_make_tenant_scoped installs the (tenant_id, region) RLS policy");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .unwrap();

    for (item_id, t) in [("itm-A", "tenantA"), ("itm-B", "tenantB")] {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
            .bind(t)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(&format!(
            "INSERT INTO {tbl} \
               (tenant_id, region, item_id, recipient, subject, subject_root, reason, class, \
                origin_event, template_key, template_args_json, dedup_key, state, occurred_at, dek_ref) \
             VALUES ($1, 'fr-par', $2, 'psn:alice', 'myelin://x/issue/1', 'myelin://x/issue/1', \
                     'mentioned', 'direct', 'myelin://x/event/1', 'issue.mentioned', '[]'::jsonb, \
                     'dk-1', 'unread', now(), 'kms://acme/0/tenant')"
        ))
        .bind(t)
        .bind(item_id)
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
        "RLS must hide the other tenant's inbox item - 0 cross-tenant rows"
    );
    assert_eq!(rows[0].get::<String, _>("tenant_id"), "tenantA");

    let cross: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {tbl} WHERE tenant_id = 'tenantB'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        cross, 0,
        "a tenant-A session must read 0 cross-tenant (tenantB) rows"
    );

    let dup = sqlx::query(&format!(
        "INSERT INTO {tbl} \
           (tenant_id, region, item_id, recipient, subject, subject_root, reason, class, \
            origin_event, template_key, template_args_json, dedup_key, state, occurred_at, dek_ref) \
         VALUES ('tenantA', 'fr-par', 'itm-A2', 'psn:alice', 'myelin://x/issue/1', \
                 'myelin://x/issue/1', 'mentioned', 'direct', 'myelin://x/event/2', \
                 'issue.mentioned', '[]'::jsonb, 'dk-1', 'unread', now(), 'kms://acme/0/tenant')"
    ))
    .execute(&mut *conn)
    .await;
    assert!(
        dup.is_err(),
        "a duplicate (recipient, dedup_key) must be REJECTED by UNIQUE(tenant_id, recipient, dedup_key) (§3.2)"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
