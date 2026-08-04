#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_flow::migrations::{WF_SIGNAL_DDL, WF_SIGNAL_PENDING_IDX};

#[tokio::test]
async fn flow_p09_signal_delivery_buffers_once_on_conflict_in_real_postgres() {
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
    let sig_tbl = format!("wf_signal_deliver_{pid}");

    let sig_create = WF_SIGNAL_DDL.replacen("wf_signal", &sig_tbl, 1);
    let idx_create = WF_SIGNAL_PENDING_IDX
        .replacen("wf_signal_pending", &format!("{sig_tbl}_pending"), 1)
        .replacen("ON wf_signal", &format!("ON {sig_tbl}"), 1);
    sqlx::query(&format!("DROP TABLE IF EXISTS {sig_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&sig_create)
        .execute(&admin)
        .await
        .expect("the wf_signal DDL applies");
    sqlx::query(&idx_create)
        .execute(&admin)
        .await
        .expect("the wf_signal_pending index applies");

    let deliver = |seq_marker: &str| {
        let tbl = sig_tbl.clone();
        let pool = admin.clone();
        let marker = seq_marker.to_string();
        async move {
            sqlx::query(&format!(
                "INSERT INTO {tbl} (tenant_id, region, run_id, signal_name, idem_key, payload, payload_key_ref, consumed_seq) \
                 VALUES ('acme','fr-par','R-SIG','job.done','tok-1', \
                         jsonb_build_array('myelin://acme/agent/result/{marker}'), NULL, NULL) \
                 ON CONFLICT (tenant_id, run_id, signal_name, idem_key) DO NOTHING \
                 RETURNING run_id"
            ))
            .fetch_optional(&pool)
            .await
            .expect("the ON CONFLICT DO NOTHING delivery applies")
        }
    };

    let first = deliver("first").await;
    assert!(
        first.is_some(),
        "the FIRST delivery buffered the signal (RETURNING a row)"
    );

    let second = deliver("second").await;
    assert!(
        second.is_none(),
        "the RE-delivery is a no-op (ON CONFLICT DO NOTHING - the workflow wakes once)"
    );

    let count: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {sig_tbl} WHERE tenant_id='acme' AND run_id='R-SIG'"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(
        count, 1,
        "EXACTLY ONE wf_signal row buffered (a double-delivery is one, not two)"
    );

    let payload: serde_json::Value = sqlx::query(&format!(
        "SELECT payload FROM {sig_tbl} WHERE tenant_id='acme' AND run_id='R-SIG' AND signal_name='job.done' AND idem_key='tok-1'"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("payload");
    assert_eq!(
        payload,
        serde_json::json!(["myelin://acme/agent/result/first"]),
        "the buffered payload is the FIRST delivery's (DO NOTHING never overwrote) - references-not-payloads"
    );

    sqlx::query(&format!(
        "INSERT INTO {sig_tbl} (tenant_id, region, run_id, signal_name, idem_key, payload, consumed_seq) \
         VALUES ('acme','fr-par','R-SIG','job.done','tok-2', jsonb_build_array('myelin://acme/agent/result/r2'), NULL) \
         ON CONFLICT (tenant_id, run_id, signal_name, idem_key) DO NOTHING"
    ))
    .execute(&admin)
    .await
    .unwrap();
    let count2: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {sig_tbl} WHERE tenant_id='acme' AND run_id='R-SIG'"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(
        count2, 2,
        "a distinct idem_key buffers distinctly (the per-effect anchor, §6.4)"
    );

    let pending: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {sig_tbl} WHERE tenant_id='acme' AND run_id='R-SIG' AND consumed_seq IS NULL"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(
        pending, 2,
        "two BUFFERED (unconsumed) signals - the signal-buffer-depth source (§1.8 / §5.4)"
    );

    sqlx::query(&format!("DROP TABLE IF EXISTS {sig_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    println!(
        "[2026-06-21] PASS  drill=FLOW-P09(live-PG)  double-deliver(same idem_key)->buffer once  rows=1 redelivery=no-op  distinct-key->2  buffered_depth=2  (real Postgres wf_signal PK + ON CONFLICT DO NOTHING)"
    );
}
