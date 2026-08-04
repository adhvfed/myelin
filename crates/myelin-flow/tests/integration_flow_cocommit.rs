#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::OUTBOX_MIGRATION;
use myelin_flow::migrations::WF_HISTORY_DDL;

#[tokio::test]
async fn flow_d5_journal_and_outbox_co_commit_atomically_in_one_pg_transaction() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(
            &cfg.database_url
                .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"),
        )
        .await
        .expect("connect as admin to dev Postgres (is the stack up?)");

    let pid = std::process::id();
    let hist_tbl = format!("wf_history_cocommit_{pid}");
    let outbox_tbl = format!("outbox_cocommit_{pid}");

    let hist_create = WF_HISTORY_DDL.replacen("wf_history", &hist_tbl, 1);
    let outbox_create = OUTBOX_MIGRATION.replace("outbox", &outbox_tbl);

    for tbl in [&hist_tbl, &outbox_tbl] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
            .execute(&admin)
            .await
            .unwrap();
    }
    sqlx::query(&hist_create)
        .execute(&admin)
        .await
        .expect("the wf_history DDL applies");
    for stmt in outbox_create.split(';') {
        let s: String = stmt
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        if s.trim().is_empty() {
            continue;
        }
        sqlx::query(&s)
            .execute(&admin)
            .await
            .expect("the outbox DDL statement applies");
    }

    let count = |pool: sqlx::PgPool, tbl: String| async move {
        sqlx::query(&format!("SELECT count(*)::bigint AS c FROM {tbl}"))
            .fetch_one(&pool)
            .await
            .unwrap()
            .get::<i64, _>("c")
    };

    {
        let mut tx = admin.begin().await.expect("begin co-commit txn");
        sqlx::query(&format!(
            "INSERT INTO {hist_tbl} (tenant_id, region, run_id, seq, kind, command_id) \
             VALUES ('acme','fr-par','R1',0,'activity_completed','agent.run:0')"
        ))
        .execute(&mut *tx)
        .await
        .expect("journal the wf_history row in the txn");
        sqlx::query(&format!(
            "INSERT INTO {outbox_tbl} (event_id, aggregate, seq, subject, envelope) \
             VALUES ('evt-1','run:R1',0,'myelin://acme/agent/run/R1','{{}}'::jsonb)"
        ))
        .execute(&mut *tx)
        .await
        .expect("emit the outbox row in the SAME txn");
        tx.commit()
            .await
            .expect("co-commit: journal + outbox become durable together");
    }
    assert_eq!(
        count(admin.clone(), hist_tbl.clone()).await,
        1,
        "one journal row durable"
    );
    assert_eq!(
        count(admin.clone(), outbox_tbl.clone()).await,
        1,
        "one outbox row durable"
    );

    {
        let mut tx = admin.begin().await.expect("begin crash txn");
        sqlx::query(&format!(
            "INSERT INTO {hist_tbl} (tenant_id, region, run_id, seq, kind, command_id) \
             VALUES ('acme','fr-par','R1',1,'activity_completed','agent.run:1')"
        ))
        .execute(&mut *tx)
        .await
        .expect("journal the second step in the txn");
        sqlx::query(&format!(
            "INSERT INTO {outbox_tbl} (event_id, aggregate, seq, subject, envelope) \
             VALUES ('evt-2','run:R1',1,'myelin://acme/agent/run/R1','{{}}'::jsonb)"
        ))
        .execute(&mut *tx)
        .await
        .expect("emit the second step in the SAME txn");
        tx.rollback()
            .await
            .expect("the crash transaction rolls back");
    }
    assert_eq!(
        count(admin.clone(), hist_tbl.clone()).await,
        1,
        "0 lost: the rolled-back step journaled NOTHING (still exactly one row)"
    );
    assert_eq!(
        count(admin.clone(), outbox_tbl.clone()).await,
        1,
        "0 ghost: the rolled-back step emitted NOTHING (still exactly one row)"
    );

    let dup = sqlx::query(&format!(
        "INSERT INTO {hist_tbl} (tenant_id, region, run_id, seq, kind, command_id) \
         VALUES ('acme','fr-par','R1',2,'activity_completed','agent.run:0')"
    ))
    .execute(&admin)
    .await;
    assert!(
        dup.is_err(),
        "the UNIQUE(tenant_id, run_id, command_id) rejects a duplicate journal (replay-safe §3.2)"
    );

    for tbl in [&hist_tbl, &outbox_tbl] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
            .execute(&admin)
            .await
            .unwrap();
    }
    println!(
        "[2026-06-21] PASS  drill=FLOW-D5(live-PG)  co_commit=atomic  ghost=0 lost=0  journal_rows=1 outbox_rows=1  (real Postgres txn)"
    );
}
