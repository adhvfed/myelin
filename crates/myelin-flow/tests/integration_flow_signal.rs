//! Live-Postgres integration test (Stage 1 / infra) — the P-FLOW-09 durable-signal DELIVERY +
//! idempotency proven against REAL Postgres: a doubly-delivered signal under the same `(tenant,
//! run_id, signal_name, idem_key)` is buffered EXACTLY ONCE via `INSERT … ON CONFLICT DO NOTHING`.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding-policy floor — no DB at build). This runs ONLY
//! against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-flow --features integration --test integration_flow_signal -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the same
//! test runs against Scaleway (fr-par) by exporting the prod env vars — never a code change.
//!
//! It proves, against REAL Postgres, the P-FLOW-09 gate the in-memory `SignalStore` model asserts:
//! the frozen `wf_signal` PK `(tenant_id, run_id, signal_name, idem_key)` is EXACTLY what makes a
//! double-delivered signal buffer once. The dedup is the PK, NOT an application-level read-then-write
//! (a redelivery under at-least-once bus delivery wakes the workflow once, §4.9).
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

    // The REAL frozen DDL (per-pid table name so concurrent runs isolate). The wf_signal DDL names
    // the PK (tenant_id, run_id, signal_name, idem_key) — the per-effect idempotency anchor (§3.4).
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

    // The DELIVERY: INSERT … ON CONFLICT (tenant_id, run_id, signal_name, idem_key) DO NOTHING — the
    // exact statement `DurableExecutor::signal` issues. The references-not-payloads payload is jsonb
    // ArtifactRefs; consumed_seq NULL = buffered, unconsumed (the wait, P-FLOW-11, stamps it).
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

    // FIRST delivery: the row is inserted (RETURNING yields a row — buffered).
    let first = deliver("first").await;
    assert!(
        first.is_some(),
        "the FIRST delivery buffered the signal (RETURNING a row)"
    );

    // SECOND delivery (the at-least-once redelivery, §4.9): SAME (tenant, run, signal_name, idem_key),
    // a DIFFERENT payload — ON CONFLICT DO NOTHING fires, NOTHING is inserted (RETURNING is empty).
    let second = deliver("second").await;
    assert!(
        second.is_none(),
        "the RE-delivery is a no-op (ON CONFLICT DO NOTHING — the workflow wakes once)"
    );

    // EXACTLY ONE buffered row for the run (not two) — the §4.9 wake-once gate, in real Postgres.
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

    // the buffered payload is the FIRST delivery's (ON CONFLICT DO NOTHING never overwrote it) +
    // references-not-payloads (a jsonb array of ArtifactRefs, no inline PII).
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
        "the buffered payload is the FIRST delivery's (DO NOTHING never overwrote) — references-not-payloads"
    );

    // a DISTINCT per-effect key (different idem_key) DOES buffer (the multi-effect anchor, §6.4) —
    // the PK separates the two, so a batch/partial approval (P-FLOW-10) rides exactly this.
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

    // the buffered-pending index covers exactly the unconsumed signals (the wait's wake lookup) — both
    // rows are unconsumed (consumed_seq IS NULL), so the signal-buffer-depth is 2.
    let pending: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {sig_tbl} WHERE tenant_id='acme' AND run_id='R-SIG' AND consumed_seq IS NULL"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(
        pending, 2,
        "two BUFFERED (unconsumed) signals — the signal-buffer-depth source (§1.8 / §5.4)"
    );

    sqlx::query(&format!("DROP TABLE IF EXISTS {sig_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    println!(
        "[2026-06-21] PASS  drill=FLOW-P09(live-PG)  double-deliver(same idem_key)->buffer once  rows=1 redelivery=no-op  distinct-key->2  buffered_depth=2  (real Postgres wf_signal PK + ON CONFLICT DO NOTHING)"
    );
}
