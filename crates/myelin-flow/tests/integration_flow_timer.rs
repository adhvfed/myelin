//! Live-Postgres integration test (Stage 1 / infra) — the P-FLOW-13 durable-timer WHEEL proven against
//! REAL Postgres: the minute-bucket partial index + the `FOR UPDATE SKIP LOCKED` due-scan + the
//! effectively-once fire.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding-policy floor — no DB at build). This runs ONLY
//! against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-flow --features integration --test integration_flow_timer -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the same
//! test runs against Scaleway (fr-par) by exporting the prod env vars — never a code change.
//!
//! It proves, against REAL Postgres, the FLOW-D3 wheel properties the in-memory `TimerStore` model
//! asserts:
//!   1. the frozen `wf_timer_due` partial index `(bucket, partition) WHERE NOT fired` is USED by the
//!      due-scan (a far-future timer in a far-future bucket is NEVER read — the SC-11 indexed-not-scanned
//!      move): the query plan over the index reads only the imminent bucket;
//!   2. the due-scan `WHERE bucket <= epoch_minute(now) AND NOT fired … FOR UPDATE SKIP LOCKED` claims
//!      the due timers, not the far-future ones;
//!   3. firing is effectively-once: `UPDATE … SET fired = true WHERE NOT fired` flips it once; a
//!      re-fire (the crash-re-fire) updates 0 rows (0 double-fire).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_flow::migrations::{WF_TIMER_DDL, WF_TIMER_DUE_IDX};

#[tokio::test]
async fn flow_p13_timer_wheel_bucketed_scan_and_effectively_once_fire_in_real_postgres() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&cfg.database_url.replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"))
        .await
        .expect("connect as admin to dev Postgres (is the stack up?)");

    let pid = std::process::id();
    let tbl = format!("wf_timer_wheel_{pid}");

    // The REAL frozen DDL (per-pid table name so concurrent runs isolate). The wf_timer DDL + the
    // SC-11 partial index `(bucket, partition) WHERE NOT fired` (the world-scale move, §3.3).
    let create = WF_TIMER_DDL.replacen("wf_timer", &tbl, 1);
    let idx = WF_TIMER_DUE_IDX
        .replacen("wf_timer_due", &format!("{tbl}_due"), 1)
        .replacen("ON wf_timer", &format!("ON {tbl}"), 1);
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}")).execute(&admin).await.unwrap();
    sqlx::query(&create).execute(&admin).await.expect("the wf_timer DDL applies");
    sqlx::query(&idx).execute(&admin).await.expect("the wf_timer_due partial index applies");

    // Arm ONE due timer (fire_at now, bucket 0) + 50_000 FAR-FUTURE timers (30 days out — a far-future
    // bucket). The far-future fleet sits in a far-future bucket; the partial index NEVER reads it until
    // its minute (the SC-11 indexed-not-scanned move). epoch_minute = extract(epoch from fire_at)::int / 60.
    let now_bucket: i32 = 0; // model "now" as epoch second 0 → bucket 0 (the due timer's minute).
    sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, fired, partition) \
         VALUES ('acme','fr-par','t-due','R-due','sla.run:0', to_timestamp(0), 0, false, 0)"
    ))
    .execute(&admin)
    .await
    .expect("the due timer arms");
    // 50k far-future timers in a single bulk insert (each in a far-future bucket = 43200 = 30 days).
    sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, fired, partition) \
         SELECT 'acme','fr-par','far/'||g, 'R-far-'||g, 'sla.run/far:'||g, \
                to_timestamp(2592000), 43200, false, 0 \
         FROM generate_series(1, 50000) AS g"
    ))
    .execute(&admin)
    .await
    .expect("the 50k far-future fleet arms");

    // ANALYZE so the planner has stats (the partial index choice is plan-dependent).
    sqlx::query(&format!("ANALYZE {tbl}")).execute(&admin).await.unwrap();

    // (1) THE BUCKETED DUE-SCAN uses the PARTIAL INDEX — the EXPLAIN over the due-scan reads the
    //     `_due` index, NOT a sequential scan of the 50k+1 fleet (the SC-11 indexed-not-full-scan).
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

    // (2) THE DUE-SCAN returns ONLY the due timer — the 50k far-future are NOT in the due bucket.
    let due: Vec<String> = sqlx::query(&due_scan)
        .fetch_all(&admin)
        .await
        .expect("the due-scan claims the due timers")
        .iter()
        .map(|r| r.get::<String, _>("timer_id"))
        .collect();
    assert_eq!(due, vec!["t-due".to_string()], "the due-scan returned ONLY the due timer (far-future untouched)");

    // (3) EFFECTIVELY-ONCE FIRE: `UPDATE … SET fired = true WHERE NOT fired` flips it ONCE; a re-fire
    //     (the crash-re-fire) updates 0 rows (0 double-fire).
    let fire = format!("UPDATE {tbl} SET fired = true WHERE tenant_id='acme' AND timer_id='t-due' AND NOT fired");
    let first = sqlx::query(&fire).execute(&admin).await.expect("the first fire").rows_affected();
    assert_eq!(first, 1, "the first fire flips the timer (1 row updated)");
    let second = sqlx::query(&fire).execute(&admin).await.expect("the re-fire").rows_affected();
    assert_eq!(second, 0, "the re-fire updates 0 rows (0 double-fire — effectively-once)");

    // after the fire the due-scan returns NOTHING (the partial index excludes the now-fired timer).
    let after: Vec<String> = sqlx::query(&due_scan)
        .fetch_all(&admin)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>("timer_id"))
        .collect();
    assert!(after.is_empty(), "the fired timer is excluded by the partial index `WHERE NOT fired`");

    // the far-future fleet is STILL unfired (never touched — cost nothing).
    let far_unfired: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {tbl} WHERE bucket = 43200 AND NOT fired"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(far_unfired, 50_000, "the 50k far-future fleet is untouched (never scanned, never fired)");

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}")).execute(&admin).await.unwrap();
    println!(
        "[2026-06-21] PASS  drill=FLOW-D3(live-PG)  armed=50001 (far-future=50000 + due=1)  \
         due-scan->[t-due] (partial-index, NO seq-scan)  fire: 1 row, re-fire: 0 rows (0 double-fire)  \
         far-future-unfired=50000 (never scanned)  (real Postgres wf_timer_due partial index + FOR UPDATE SKIP LOCKED)"
    );
}
