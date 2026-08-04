#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_notif::migrations::{rls_scope_sql, INBOX_ITEM_DDL};

#[tokio::test]
async fn notif_snooze_resurface_update_round_trips_exactly_once_and_rls_denies_cross_tenant() {
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

    let tbl = format!("notif_inbox_item_p196_{}", std::process::id());
    let ddl = INBOX_ITEM_DDL.replacen("notif_inbox_item", &tbl, 1);

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&ddl)
        .execute(&admin)
        .await
        .expect("the notif_inbox_item DDL applies");
    sqlx::query(&rls_scope_sql(&tbl))
        .execute(&admin)
        .await
        .expect("RLS policy installs");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .unwrap();

    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();

    let insert = format!(
        "INSERT INTO {tbl} \
         (tenant_id, region, item_id, recipient, subject, subject_root, reason, class, origin_event, \
          template_key, template_args_json, dedup_key, occurred_at, dek_ref) \
         VALUES ('tenantA','fr-par','itm-1','psn:alice','myelin://tenantA/issues/issue/7', \
                 'myelin://tenantA/issues/issue/7','assigned','direct','myelin://tenantA/bus/event/e1', \
                 'issue.assigned','[]'::jsonb,'itm-1', now(), 'kms://tenantA/0/tenant')"
    );
    sqlx::query(&insert)
        .execute(&mut *conn)
        .await
        .expect("the inbox item inserts (state defaults unread)");

    let snooze = format!(
        "UPDATE {tbl} SET state='snoozed', snooze_until='2026-06-25T09:00:00Z'::timestamptz \
         WHERE recipient='psn:alice' AND item_id='itm-1'"
    );
    let n = sqlx::query(&snooze)
        .execute(&mut *conn)
        .await
        .expect("the snooze UPDATE applies")
        .rows_affected();
    assert_eq!(n, 1, "exactly the one item is snoozed");
    let row = sqlx::query(&format!(
        "SELECT state, (snooze_until IS NOT NULL) AS has_until FROM {tbl} WHERE item_id='itm-1'"
    ))
    .fetch_one(&mut *conn)
    .await
    .expect("the durable snooze handle reads back");
    assert_eq!(
        row.get::<String, _>("state"),
        "snoozed",
        "the item is parked (durable snooze handle)"
    );
    assert!(
        row.get::<bool, _>("has_until"),
        "the snooze_until is persisted (the re-surface instant the timer fires at)"
    );

    let resurface = format!(
        "UPDATE {tbl} SET state='unread', snooze_until=NULL \
         WHERE recipient='psn:alice' AND item_id='itm-1' AND state='snoozed'"
    );
    let first = sqlx::query(&resurface)
        .execute(&mut *conn)
        .await
        .expect("re-surface UPDATE")
        .rows_affected();
    assert_eq!(
        first, 1,
        "the first re-surface flips the snoozed row (NOT zero - 0 missed)"
    );
    let after = sqlx::query(&format!(
        "SELECT state, (snooze_until IS NULL) AS until_cleared FROM {tbl} WHERE item_id='itm-1'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        after.get::<String, _>("state"),
        "unread",
        "snoozed → unread (back in the active inbox)"
    );
    assert!(
        after.get::<bool, _>("until_cleared"),
        "the snooze_until is cleared on re-surface"
    );

    let replay = sqlx::query(&resurface)
        .execute(&mut *conn)
        .await
        .expect("replayed re-surface")
        .rows_affected();
    assert_eq!(
        replay, 0,
        "a replayed re-surface flips 0 rows (0 duplicate re-surface - the guard holds)"
    );
    let still = sqlx::query(&format!("SELECT state FROM {tbl} WHERE item_id='itm-1'"))
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        still.get::<String, _>("state"),
        "unread",
        "still exactly one re-surface"
    );

    let bad = format!("UPDATE {tbl} SET state='paused' WHERE item_id='itm-1'");
    let err = sqlx::query(&bad).execute(&mut *conn).await;
    assert!(
        err.is_err(),
        "an off-grammar state ('paused') is rejected by the CHECK constraint"
    );
    drop(conn);

    let mut conn_b = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantB', false)")
        .execute(&mut *conn_b)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn_b)
        .await
        .unwrap();
    let cross: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn_b)
        .await
        .unwrap()
        .get("n");
    assert_eq!(
        cross, 0,
        "RLS denies cross-tenant: tenant B reads 0 of tenant A's inbox rows"
    );

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .ok();
}
