//! Live-Postgres integration test (Stage 1 / infra) — the P-FLOW-11 durable-signal WAIT CONSUME proven
//! against REAL Postgres: the wait that resumes a parked run stamps `consumed_seq` via
//! `UPDATE … SET consumed_seq = $seq WHERE consumed_seq IS NULL` — the WHERE clause IS the
//! consume-EXACTLY-ONCE guard (a re-drive races to the same NULL guard and consumes NOTHING new).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding-policy floor — no DB at build). This runs ONLY
//! against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-flow --features integration --test integration_flow_wait -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the same
//! test runs against Scaleway (fr-par) by exporting the prod env vars — never a code change.
//!
//! It proves, against REAL Postgres, the P-FLOW-11 consume gate the in-memory `SignalStore::consume`
//! model asserts (the FLOW-D4 "1 consume" threshold): the wait stamps the FIRST buffered signal's
//! `consumed_seq` ONCE; a second consume attempt (a re-drive of the same wait) is a no-op (the WHERE
//! consumed_seq IS NULL guard already lost) — so the signal-buffer-depth drops by EXACTLY one (the
//! workflow wakes once, withhold/run is decided off the one consumed row, §4.3).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_flow::migrations::{WF_SIGNAL_DDL, WF_SIGNAL_PENDING_IDX};

#[tokio::test]
async fn flow_p11_wait_consume_stamps_consumed_seq_exactly_once_in_real_postgres() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&cfg.database_url.replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"))
        .await
        .expect("connect as admin to dev Postgres (is the stack up?)");

    let pid = std::process::id();
    let sig_tbl = format!("wf_signal_wait_{pid}");

    // The REAL frozen DDL (per-pid table name so concurrent runs isolate). The wf_signal DDL names the
    // PK (tenant_id, run_id, signal_name, idem_key) + the consumed_seq column the wait stamps.
    let sig_create = WF_SIGNAL_DDL.replacen("wf_signal", &sig_tbl, 1);
    let idx_create = WF_SIGNAL_PENDING_IDX
        .replacen("wf_signal_pending", &format!("{sig_tbl}_pending"), 1)
        .replacen("ON wf_signal", &format!("ON {sig_tbl}"), 1);
    sqlx::query(&format!("DROP TABLE IF EXISTS {sig_tbl}")).execute(&admin).await.unwrap();
    sqlx::query(&sig_create).execute(&admin).await.expect("the wf_signal DDL applies");
    sqlx::query(&idx_create).execute(&admin).await.expect("the wf_signal_pending index applies");

    // Buffer the approval (the §6.3 round-trip: Chat posted the decision) — unconsumed (consumed_seq
    // NULL). A double-click would be a second INSERT under the SAME key → ON CONFLICT DO NOTHING → still
    // one row (proven in integration_flow_signal.rs); here we focus on the CONSUME side.
    sqlx::query(&format!(
        "INSERT INTO {sig_tbl} (tenant_id, region, run_id, signal_name, idem_key, payload, consumed_seq) \
         VALUES ('acme','fr-par','R-WAIT','approval:call-1','card-7', \
                 jsonb_build_array('myelin://acme/agent/decision/approve'), NULL) \
         ON CONFLICT (tenant_id, run_id, signal_name, idem_key) DO NOTHING"
    ))
    .execute(&admin)
    .await
    .expect("buffer the approval");

    // The signal-buffer-depth before the consume: one BUFFERED (unconsumed) row.
    let before: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {sig_tbl} WHERE tenant_id='acme' AND run_id='R-WAIT' AND consumed_seq IS NULL"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(before, 1, "the approval is buffered, unconsumed (signal-buffer-depth = 1)");

    // THE CONSUME (the wait resuming): UPDATE … SET consumed_seq = $seq WHERE consumed_seq IS NULL — the
    // WHERE clause IS the consume-EXACTLY-ONCE guard. This is the exact statement the wait issues when it
    // finds the buffered signal + stamps the `wf_history` seq that consumed it.
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

    // FIRST consume (the wait resumes, stamping the signal_received seq): the row is stamped (RETURNING
    // yields a row — consumed).
    let first = consume(5).await;
    assert!(first.is_some(), "the FIRST consume stamped consumed_seq (the wait resumed on this signal)");

    // SECOND consume (a re-drive of the SAME wait, e.g. a later step crashed + the run re-leased): the
    // WHERE consumed_seq IS NULL guard already LOST — UPDATE matches NOTHING (RETURNING empty) → the
    // consume is EXACTLY ONCE (the replay returns the journaled signal, never re-consumes, §4.3/§4.1).
    let second = consume(9).await;
    assert!(second.is_none(), "the SECOND consume is a no-op (consumed_seq IS NULL guard lost) — consume-exactly-once");

    // the consumed_seq is the FIRST consume's seq (5), NEVER overwritten by the second (9).
    let stamped: i64 = sqlx::query(&format!(
        "SELECT consumed_seq FROM {sig_tbl} WHERE tenant_id='acme' AND run_id='R-WAIT' AND idem_key='card-7'"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("consumed_seq");
    assert_eq!(stamped, 5, "the FIRST consume's seq stuck (the second consume never overwrote it)");

    // the signal-buffer-depth dropped by EXACTLY one (the FLOW-D4 "1 consume" threshold) — 0 unconsumed.
    let after: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {sig_tbl} WHERE tenant_id='acme' AND run_id='R-WAIT' AND consumed_seq IS NULL"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(after, 0, "the signal-buffer-depth dropped by EXACTLY one (FLOW-D4: 1 consume, real Postgres)");

    sqlx::query(&format!("DROP TABLE IF EXISTS {sig_tbl}")).execute(&admin).await.unwrap();
    println!(
        "[2026-06-21] PASS  drill=FLOW-D4(live-PG)  wait consume stamps consumed_seq once  \
         consume(5)->stamped  consume(9)->no-op(WHERE consumed_seq IS NULL)  buffered_depth 1->0  \
         (real Postgres wf_signal consume-exactly-once)"
    );
}
