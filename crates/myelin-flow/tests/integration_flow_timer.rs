#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_flow::migrations::{WF_TIMER_DDL, WF_TIMER_DUE_IDX};

#[tokio::test]
async fn flow_p13_timer_wheel_bucketed_scan_and_effectively_once_fire_in_real_postgres() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(
            &cfg.database_url
                .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"),
        )
        .await
        .expect("connect as admin to dev Postgres (is the stack up?)");

    let pid = std::process::id();
    let tbl = format!("wf_timer_wheel_{pid}");

    let create = WF_TIMER_DDL.replacen("wf_timer", &tbl, 1);
    let idx = WF_TIMER_DUE_IDX
        .replacen("wf_timer_due", &format!("{tbl}_due"), 1)
        .replacen("ON wf_timer", &format!("ON {tbl}"), 1);
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("the wf_timer DDL applies");
    sqlx::query(&idx)
        .execute(&admin)
        .await
        .expect("the wf_timer_due partial index applies");

    let now_bucket: i32 = 0;
    sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, fired, partition) \
         VALUES ('acme','fr-par','t-due','R-due','sla.run:0', to_timestamp(0), 0, false, 0)"
    ))
    .execute(&admin)
    .await
    .expect("the due timer arms");
    sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, fired, partition) \
         SELECT 'acme','fr-par','far/'||g, 'R-far-'||g, 'sla.run/far:'||g, \
                to_timestamp(2592000), 43200, false, 0 \
         FROM generate_series(1, 50000) AS g"
    ))
    .execute(&admin)
    .await
    .expect("the 50k far-future fleet arms");

    sqlx::query(&format!("ANALYZE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();

    let due_scan = format!(
        "SELECT timer_id FROM {tbl} WHERE bucket <= {now_bucket} AND NOT fired AND partition = 0 \
         ORDER BY fire_at FOR UPDATE SKIP LOCKED LIMIT 4096"
    );
    let plan: String = sqlx::query(&format!("EXPLAIN {due_scan}"))
        .fetch_all(&admin)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains(&format!("{tbl}_due")) || plan.contains("Index"),
        "the due-scan uses the partial index (indexed, not a seq-scan of the far-future fleet): {plan}"
    );
    assert!(
        !plan.contains("Seq Scan"),
        "the due-scan does NOT sequentially scan the 50k+1 fleet (the SC-11 partial-index move): {plan}"
    );

    let due: Vec<String> = sqlx::query(&due_scan)
        .fetch_all(&admin)
        .await
        .expect("the due-scan claims the due timers")
        .iter()
        .map(|r| r.get::<String, _>("timer_id"))
        .collect();
    assert_eq!(
        due,
        vec!["t-due".to_string()],
        "the due-scan returned ONLY the due timer (far-future untouched)"
    );

    let fire = format!(
        "UPDATE {tbl} SET fired = true WHERE tenant_id='acme' AND timer_id='t-due' AND NOT fired"
    );
    let first = sqlx::query(&fire)
        .execute(&admin)
        .await
        .expect("the first fire")
        .rows_affected();
    assert_eq!(first, 1, "the first fire flips the timer (1 row updated)");
    let second = sqlx::query(&fire)
        .execute(&admin)
        .await
        .expect("the re-fire")
        .rows_affected();
    assert_eq!(
        second, 0,
        "the re-fire updates 0 rows (0 double-fire - effectively-once)"
    );

    let after: Vec<String> = sqlx::query(&due_scan)
        .fetch_all(&admin)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>("timer_id"))
        .collect();
    assert!(
        after.is_empty(),
        "the fired timer is excluded by the partial index `WHERE NOT fired`"
    );

    let far_unfired: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {tbl} WHERE bucket = 43200 AND NOT fired"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(
        far_unfired, 50_000,
        "the 50k far-future fleet is untouched (never scanned, never fired)"
    );

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    println!(
        "[2026-06-21] PASS  drill=FLOW-D3(live-PG)  armed=50001 (far-future=50000 + due=1)  \
         due-scan->[t-due] (partial-index, NO seq-scan)  fire: 1 row, re-fire: 0 rows (0 double-fire)  \
         far-future-unfired=50000 (never scanned)  (real Postgres wf_timer_due partial index + FOR UPDATE SKIP LOCKED)"
    );
}
