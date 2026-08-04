#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_flow::migrations::{WF_SIGNAL_DDL, WF_SIGNAL_PENDING_IDX};

#[tokio::test]
async fn flow_p11_wait_consume_stamps_consumed_seq_exactly_once_in_real_postgres() {
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
    let sig_tbl = format!("wf_signal_wait_{pid}");

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

    sqlx::query(&format!(
        "INSERT INTO {sig_tbl} (tenant_id, region, run_id, signal_name, idem_key, payload, consumed_seq) \
         VALUES ('acme','fr-par','R-WAIT','approval:call-1','card-7', \
                 jsonb_build_array('myelin://acme/agent/decision/approve'), NULL) \
         ON CONFLICT (tenant_id, run_id, signal_name, idem_key) DO NOTHING"
    ))
    .execute(&admin)
    .await
    .expect("buffer the approval");

    let before: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {sig_tbl} WHERE tenant_id='acme' AND run_id='R-WAIT' AND consumed_seq IS NULL"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(
        before, 1,
        "the approval is buffered, unconsumed (signal-buffer-depth = 1)"
    );

    let consume = |seq: i64| {
        let tbl = sig_tbl.clone();
        let pool = admin.clone();
        async move {
            sqlx::query(&format!(
                "UPDATE {tbl} SET consumed_seq = $1 \
                 WHERE tenant_id='acme' AND run_id='R-WAIT' AND signal_name='approval:call-1' \
                   AND idem_key='card-7' AND consumed_seq IS NULL \
                 RETURNING idem_key"
            ))
            .bind(seq)
            .fetch_optional(&pool)
            .await
            .expect("the consume UPDATE applies")
        }
    };

    let first = consume(5).await;
    assert!(
        first.is_some(),
        "the FIRST consume stamped consumed_seq (the wait resumed on this signal)"
    );

    let second = consume(9).await;
    assert!(
        second.is_none(),
        "the SECOND consume is a no-op (consumed_seq IS NULL guard lost) - consume-exactly-once"
    );

    let stamped: i64 = sqlx::query(&format!(
        "SELECT consumed_seq FROM {sig_tbl} WHERE tenant_id='acme' AND run_id='R-WAIT' AND idem_key='card-7'"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("consumed_seq");
    assert_eq!(
        stamped, 5,
        "the FIRST consume's seq stuck (the second consume never overwrote it)"
    );

    let after: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {sig_tbl} WHERE tenant_id='acme' AND run_id='R-WAIT' AND consumed_seq IS NULL"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(
        after, 0,
        "the signal-buffer-depth dropped by EXACTLY one (FLOW-D4: 1 consume, real Postgres)"
    );

    sqlx::query(&format!("DROP TABLE IF EXISTS {sig_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    println!(
        "[2026-06-21] PASS  drill=FLOW-D4(live-PG)  wait consume stamps consumed_seq once  \
         consume(5)->stamped  consume(9)->no-op(WHERE consumed_seq IS NULL)  buffered_depth 1->0  \
         (real Postgres wf_signal consume-exactly-once)"
    );
}
