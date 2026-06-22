//! Live-Postgres integration test (Stage 1 / infra) — the FLOW-D5 journal/outbox CO-COMMIT proven
//! against REAL Postgres (P-FLOW-04 / P-199; the GATE: a `wf_history` row and an `outbox` row are
//! committed TOGETHER in ONE transaction — 0 ghost, 0 lost).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding-policy floor — no DB at build). This runs
//! ONLY against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-flow --features integration --test integration_flow_cocommit -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the same
//! test runs against Scaleway (fr-par) by exporting the prod env vars — never a code change.
//!
//! It proves, against REAL Postgres, the FLOW-D5 co-commit invariant the in-memory `WfCtx` model
//! (`src/wfctx.rs`) asserts: the journal write and the outbox write are ATOMIC under a single PG
//! transaction.
//!   1. A COMMITTED transaction that INSERTs a `wf_history` row AND an `outbox` row makes BOTH
//!      durable — exactly one of each (the co-commit happy path).
//!   2. A ROLLED-BACK transaction (the crash between "journal the activity's DB write" and "emit
//!      its event") writes NEITHER — 0 journal rows, 0 outbox rows (the silent-data-loss floor,
//!      BUS-D4-equivalent for the workflow journal, in real Postgres).
//!   3. The `wf_history` `UNIQUE(tenant_id, run_id, command_id)` journaling-idempotency key BITES
//!      inside the same transaction discipline — a re-journal of the same command is rejected
//!      (replay-safe).
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

    // Per-process table names so concurrent runs isolate — the DDL is the REAL frozen shape.
    let pid = std::process::id();
    let hist_tbl = format!("wf_history_cocommit_{pid}");
    let outbox_tbl = format!("outbox_cocommit_{pid}");

    let hist_create = WF_HISTORY_DDL.replacen("wf_history", &hist_tbl, 1);
    // The outbox migration creates `outbox` (+ its constraints/index); rename every "outbox" token
    // to the per-pid probe table so the table + its constraint/index names stay unique per run.
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
    // The outbox migration is a MULTI-statement DDL (CREATE TABLE + the unsent-row index + a
    // line comment); a prepared statement runs ONE command, so split on `;` and run each (skipping
    // blank/comment-only fragments). This applies the REAL frozen 2.3 outbox shape.
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

    // (1) The co-commit HAPPY PATH: one transaction INSERTs the wf_history row AND the outbox row,
    //     then COMMITS. Both are durable — exactly one of each.
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

    // (2) The CRASH PATH (FLOW-D5): a transaction journals a SECOND step's history row AND emits
    //     its outbox row, then ROLLS BACK (the crash between journal and emit). NEITHER persists —
    //     the counts stay at exactly 1 (0 ghost, 0 lost).
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
        // CRASH: roll back instead of commit — the crash between journal and emit durability.
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

    // (3) The UNIQUE(tenant_id, run_id, command_id) journaling-idempotency key BITES — a re-journal
    //     of agent.run:0 is rejected (replay-safe, §3.2).
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

    // cleanup
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
