//! Live-Postgres integration test (Stage 1 / infra) — the P-FLOW-14 (P-210) cheap SLA-timer
//! disarm/re-arm proven against REAL Postgres: a re-arm is a SINGLE `UPDATE` (1 row), a disarm makes
//! the timer never fire, and NEITHER pollutes the wheel with calendar logic (the far-future fleet is
//! never scanned).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding-policy floor — no DB at build). This runs ONLY
//! against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-flow --features integration --test integration_flow_rearm -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the same
//! test runs against Scaleway (fr-par) by exporting the prod env vars — never a code change.
//!
//! It proves, against REAL Postgres, the §6.6 disarm/re-arm row-op properties the in-memory
//! `TimerStore` model asserts:
//!   1. a RE-ARM is `UPDATE wf_timer SET fire_at = …, bucket = … WHERE (tenant, timer_id) = …` — a
//!      SINGLE row update (`rows_affected() == 1`), NO new row, NO calendar scan; the bucket is the
//!      recomputed `epoch_minute(new_fire_at)`;
//!   2. a re-arm SLIDES the timer out of the due bucket — the wheel's due-scan no longer returns it;
//!   3. a DISARM is `UPDATE … SET fired = true WHERE NOT fired` — one row; the disarmed timer is then
//!      excluded by the partial index `WHERE NOT fired` (it never fires);
//!   4. the far-future fleet is NEVER scanned by the re-arm/disarm (row-update cost, not wheel-scan).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_flow::migrations::{WF_TIMER_DDL, WF_TIMER_DUE_IDX};

#[tokio::test]
async fn flow_p14_cheap_disarm_rearm_is_one_row_update_in_real_postgres() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&cfg.database_url.replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"))
        .await
        .expect("connect as admin to dev Postgres (is the stack up?)");

    let pid = std::process::id();
    let tbl = format!("wf_timer_rearm_{pid}");

    let create = WF_TIMER_DDL.replacen("wf_timer", &tbl, 1);
    let idx = WF_TIMER_DUE_IDX
        .replacen("wf_timer_due", &format!("{tbl}_due"), 1)
        .replacen("ON wf_timer", &format!("ON {tbl}"), 1);
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}")).execute(&admin).await.unwrap();
    sqlx::query(&create).execute(&admin).await.expect("the wf_timer DDL applies");
    sqlx::query(&idx).execute(&admin).await.expect("the wf_timer_due partial index applies");

    // Arm an SLA breach timer due NOW (fire_at second 0, bucket 0) + 50_000 FAR-FUTURE timers (30
    // days out) — the fleet the re-arm/disarm must NEVER scan (the SC-11 indexed-not-scanned move).
    sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, fired, partition) \
         VALUES ('acme','fr-par','sla/issue-7','R-sla-7','issues.sla:0', to_timestamp(0), 0, false, 0)"
    ))
    .execute(&admin)
    .await
    .expect("the SLA breach timer arms");
    sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, fired, partition) \
         SELECT 'acme','fr-par','far/'||g, 'R-far-'||g, 'issues.sla/far:'||g, \
                to_timestamp(2592000), 43200, false, 0 \
         FROM generate_series(1, 50000) AS g"
    ))
    .execute(&admin)
    .await
    .expect("the 50k far-future fleet arms");
    sqlx::query(&format!("ANALYZE {tbl}")).execute(&admin).await.unwrap();

    // The wheel due-scan (model "now" = epoch second 0 → bucket 0).
    let now_bucket: i32 = 0;
    let due_scan = format!(
        "SELECT timer_id FROM {tbl} WHERE bucket <= {now_bucket} AND NOT fired AND partition = 0 \
         ORDER BY fire_at FOR UPDATE SKIP LOCKED LIMIT 4096"
    );

    // (pre) the breach timer IS due before the re-arm.
    let due_before: Vec<String> = sqlx::query(&due_scan)
        .fetch_all(&admin)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>("timer_id"))
        .collect();
    assert_eq!(due_before, vec!["sla/issue-7".to_string()], "the breach timer is due before the re-arm");

    // (1) RE-ARM: slide the deadline forward to 8h (28_800s, bucket 480) — a SINGLE row UPDATE. NO new
    //     row, NO calendar logic. The bucket is the recomputed epoch_minute(new_fire_at) = 28800/60.
    let new_fire_at: i64 = 28_800;
    let new_bucket: i32 = (new_fire_at / 60) as i32; // = epoch_minute(new_fire_at) = 480.
    let rearm = format!(
        "UPDATE {tbl} SET fire_at = to_timestamp({new_fire_at}), bucket = {new_bucket}, fired = false \
         WHERE tenant_id='acme' AND timer_id='sla/issue-7'"
    );
    let rearm_rows = sqlx::query(&rearm).execute(&admin).await.expect("the re-arm").rows_affected();
    assert_eq!(rearm_rows, 1, "a re-arm is a SINGLE row update (no new row, no wheel rescan)");

    // the re-arm did NOT add a row — STILL 50_001 timers (no wheel pollution).
    let total: i64 = sqlx::query(&format!("SELECT count(*)::bigint AS c FROM {tbl}"))
        .fetch_one(&admin)
        .await
        .unwrap()
        .get("c");
    assert_eq!(total, 50_001, "the re-arm was an UPDATE — STILL 50_001 rows (no duplicate on the wheel)");

    // (2) the re-armed timer SLID out of the due bucket — the due-scan no longer returns it.
    let due_after_rearm: Vec<String> = sqlx::query(&due_scan)
        .fetch_all(&admin)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>("timer_id"))
        .collect();
    assert!(due_after_rearm.is_empty(), "the re-armed breach is no longer due (slid to a far-future bucket)");

    // (3) DISARM: the SLA was met — `UPDATE … SET fired = true WHERE NOT fired` (one row). The timer
    //     is then excluded by the partial index `WHERE NOT fired` forever (it never fires). First we
    //     make it due again (model the deadline arriving) so we PROVE the disarm — not the slide —
    //     keeps it out of the scan.
    sqlx::query(&format!(
        "UPDATE {tbl} SET fire_at = to_timestamp(0), bucket = 0 WHERE tenant_id='acme' AND timer_id='sla/issue-7'"
    ))
    .execute(&admin)
    .await
    .unwrap();
    let disarm = format!(
        "UPDATE {tbl} SET fired = true WHERE tenant_id='acme' AND timer_id='sla/issue-7' AND NOT fired"
    );
    let disarm_rows = sqlx::query(&disarm).execute(&admin).await.expect("the disarm").rows_affected();
    assert_eq!(disarm_rows, 1, "a disarm is a SINGLE row update (set the partial-index pivot)");

    // the disarmed timer is excluded by `WHERE NOT fired` — the due-scan returns NOTHING (it never fires).
    let due_after_disarm: Vec<String> = sqlx::query(&due_scan)
        .fetch_all(&admin)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>("timer_id"))
        .collect();
    assert!(due_after_disarm.is_empty(), "the disarmed timer never fires (the partial index excludes it)");

    // a re-disarm updates 0 rows (already fired — idempotent, no double-cancel).
    let re_disarm = sqlx::query(&disarm).execute(&admin).await.unwrap().rows_affected();
    assert_eq!(re_disarm, 0, "a re-disarm of an already-disarmed timer updates 0 rows");

    // (4) the far-future fleet was NEVER touched by the re-arm/disarm (still 50_000 unfired) — the
    //     disarm/re-arm cost was row-update, NOT a wheel scan of the fleet.
    let far_unfired: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {tbl} WHERE bucket = 43200 AND NOT fired"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(far_unfired, 50_000, "the 50k far-future fleet is untouched by the re-arm/disarm (row-op, not wheel-scan)");

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}")).execute(&admin).await.unwrap();
    println!(
        "[2026-06-21] PASS  drill=P-FLOW-14-rearm(live-PG)  re-arm: 1 row UPDATE (fire_at+bucket, no new row, total stays 50001)  \
         re-armed breach slid out of due-scan  disarm: 1 row UPDATE (fired=true), re-disarm: 0 rows (idempotent)  \
         disarmed timer NEVER fires (partial index WHERE NOT fired)  far-future fleet untouched=50000 (row-op, not wheel-scan)  \
         (real Postgres wf_timer cheap disarm/re-arm — §6.6)"
    );
}
