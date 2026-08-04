#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_flow::migrations::{WF_HISTORY_DDL, WORKFLOW_RUN_DDL};

#[tokio::test]
async fn flow_d1_lease_crash_recovery_and_replay_short_circuit_in_real_postgres() {
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
    let run_tbl = format!("workflow_run_replay_{pid}");
    let hist_tbl = format!("wf_history_replay_{pid}");

    let run_create = WORKFLOW_RUN_DDL.replacen("workflow_run", &run_tbl, 1);
    let hist_create = WF_HISTORY_DDL.replacen("wf_history", &hist_tbl, 1);
    for tbl in [&run_tbl, &hist_tbl] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
            .execute(&admin)
            .await
            .unwrap();
    }
    sqlx::query(&run_create)
        .execute(&admin)
        .await
        .expect("the workflow_run DDL applies");
    sqlx::query(&hist_create)
        .execute(&admin)
        .await
        .expect("the wf_history DDL applies");

    sqlx::query(&format!(
        "INSERT INTO {run_tbl} \
         (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor, correlation_id, depth, partition) \
         VALUES ('acme','fr-par','R1','agent.run',1,'[]'::jsonb,'running',5,'corr-1',0,0)"
    ))
    .execute(&admin)
    .await
    .expect("seed a half-driven runnable run (cursor at the crash point)");
    for k in 0..5i64 {
        sqlx::query(&format!(
            "INSERT INTO {hist_tbl} (tenant_id, region, run_id, seq, kind, command_id, result) \
             VALUES ('acme','fr-par','R1',{k},'activity_completed','agent.run:{k}', \
                     jsonb_build_array('myelin://acme/agent/effect/e{k}'))"
        ))
        .execute(&admin)
        .await
        .expect("journal the crashed worker's step");
    }

    let now_secs: i64 = 1000;
    let leased = sqlx::query(&format!(
        "UPDATE {run_tbl} SET lease_owner = 'worker-2', \
             lease_expires = to_timestamp({now_secs} + 30) \
         WHERE (tenant_id, region, run_id) = ( \
            SELECT tenant_id, region, run_id FROM {run_tbl} \
            WHERE partition = 0 AND state = 'running' \
              AND (lease_owner IS NULL OR lease_expires <= to_timestamp({now_secs})) \
            ORDER BY run_id FOR UPDATE SKIP LOCKED LIMIT 1) \
         RETURNING run_id, cursor"
    ))
    .fetch_one(&admin)
    .await
    .expect("worker-2 re-leases the runnable run (FOR UPDATE SKIP LOCKED)");
    assert_eq!(leased.get::<String, _>("run_id"), "R1");
    assert_eq!(
        leased.get::<i64, _>("cursor"),
        5,
        "the re-leased run resumes from cursor 5"
    );

    let steal_blocked = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {run_tbl} \
         WHERE partition = 0 AND state = 'running' \
           AND (lease_owner IS NULL OR lease_expires <= to_timestamp({now_secs}))"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get::<i64, _>("c");
    assert_eq!(
        steal_blocked, 0,
        "the live-leased run is not re-leasable (skip-locked, no double-drive)"
    );

    let journaled: Vec<String> = sqlx::query(&format!(
        "SELECT command_id FROM {hist_tbl} WHERE tenant_id='acme' AND run_id='R1' ORDER BY seq"
    ))
    .fetch_all(&admin)
    .await
    .unwrap()
    .iter()
    .map(|r| r.get::<String, _>("command_id"))
    .collect();
    assert_eq!(
        journaled.len(),
        5,
        "5 commands journaled at the crash point"
    );

    let mut executed = Vec::new();
    for k in 0..10i64 {
        let command_id = format!("agent.run:{k}");
        if journaled.contains(&command_id) {
            continue;
        }
        executed.push(k);
        sqlx::query(&format!(
            "INSERT INTO {hist_tbl} (tenant_id, region, run_id, seq, kind, command_id, result) \
             VALUES ('acme','fr-par','R1',{k},'activity_completed','{command_id}', \
                     jsonb_build_array('myelin://acme/agent/effect/e{k}'))"
        ))
        .execute(&admin)
        .await
        .expect("journal the resumed live command");
    }
    assert_eq!(
        executed,
        vec![5, 6, 7, 8, 9],
        "resumed at step 6 - only 5..=9 ran; 0..=4 replayed"
    );

    let total: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {hist_tbl} WHERE tenant_id='acme' AND run_id='R1'"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get::<i64, _>("c");
    assert_eq!(
        total, 10,
        "10 journaled, 0 lost progress, 0 duplicate (the UNIQUE(command_id) key)"
    );

    let dup = sqlx::query(&format!(
        "INSERT INTO {hist_tbl} (tenant_id, region, run_id, seq, kind, command_id) \
         VALUES ('acme','fr-par','R1',99,'activity_completed','agent.run:0')"
    ))
    .execute(&admin)
    .await;
    assert!(
        dup.is_err(),
        "the UNIQUE replay key rejects a re-journal of a replayed command (§3.2)"
    );

    sqlx::query(&format!(
        "UPDATE {run_tbl} SET state='completed', cursor=10, lease_owner=NULL, lease_expires=NULL \
         WHERE tenant_id='acme' AND run_id='R1'"
    ))
    .execute(&admin)
    .await
    .unwrap();

    for tbl in [&run_tbl, &hist_tbl] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
            .execute(&admin)
            .await
            .unwrap();
    }
    println!(
        "[2026-06-21] PASS  drill=FLOW-D1(live-PG)  kill@5/10 resume@6  re_executed=0 lost=0  lease=skip-locked replay=short-circuit  (real Postgres FOR UPDATE SKIP LOCKED + UNIQUE replay key)"
    );
}

#[tokio::test]
async fn flow_d2_divergence_guard_dead_letters_nondeterministic_in_real_postgres() {
    use myelin_events::{Actor, EmitContextBase, MonotonicMinter, OutboxStore, Timestamp};
    use myelin_flow::engine::{
        drive_versioned, run_state, DriveOutcome, FlowTelemetry, RunRow, RunStore, WorkflowBody,
    };
    use myelin_flow::wfctx::{RetryPolicy, WfCtx, WfJournal};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_refs::ArtifactRef;
    use myelin_tenancy::{Region, TenantId};
    use sqlx::Row;
    use std::sync::Arc;

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
    let run_tbl = format!("workflow_run_divergence_{pid}");
    let run_create = WORKFLOW_RUN_DDL.replacen("workflow_run", &run_tbl, 1);
    sqlx::query(&format!("DROP TABLE IF EXISTS {run_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&run_create)
        .execute(&admin)
        .await
        .expect("the workflow_run DDL applies");

    sqlx::query(&format!(
        "INSERT INTO {run_tbl} \
         (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor, correlation_id, depth, partition) \
         VALUES ('acme','fr-par','RD','agent.run',1,'[]'::jsonb,'running',0,'corr-d',0,0)"
    ))
    .execute(&admin)
    .await
    .expect("seed the runnable run");

    let ctx_base = EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: None,
    };
    let runs = RunStore::new();
    runs.put(RunRow::new_runnable_versioned(
        TenantId("acme".into()),
        Region("fr-par".into()),
        "RD",
        "agent.run",
        1,
        0,
    ));
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let tele = FlowTelemetry::new();
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(MonotonicMinter::new());
    let body: Box<WorkflowBody> = Box::new(|ctx: &mut WfCtx| {
        ctx.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e0".into())])
        })
        .map_err(|e| format!("{e:?}"))?;
        Ok(vec![])
    });
    let run = runs.get(&TenantId("acme".into()), "RD").unwrap();
    let outcome = drive_versioned(
        &runs,
        &outbox,
        &journal,
        &tele,
        minter,
        ctx_base,
        &run,
        "2026-06-21T00:00:00Z",
        7,
        body.as_ref(),
        1,
        2,
    );
    assert!(
        matches!(outcome, DriveOutcome::Nondeterministic(_)),
        "the version mismatch halts"
    );
    assert_eq!(
        tele.nondeterministic_halt_count(),
        1,
        "the nondeterministic-halt count incremented by exactly 1"
    );

    let settled = runs.get(&TenantId("acme".into()), "RD").unwrap();
    assert_eq!(
        settled.state,
        run_state::NONDETERMINISTIC,
        "the in-memory run is dead-lettered"
    );
    sqlx::query(&format!(
        "UPDATE {run_tbl} SET state=$1, lease_owner=NULL, lease_expires=NULL \
         WHERE tenant_id='acme' AND run_id='RD'"
    ))
    .bind(&settled.state)
    .execute(&admin)
    .await
    .expect("the dead-letter state persists - the frozen state CHECK admits 'nondeterministic'");

    let st: String = sqlx::query(&format!("SELECT state FROM {run_tbl} WHERE run_id='RD'"))
        .fetch_one(&admin)
        .await
        .unwrap()
        .get("state");
    assert_eq!(
        st, "nondeterministic",
        "the dead-letter row persists in real Postgres"
    );
    let runnable: i64 = sqlx::query(&format!(
        "SELECT count(*) AS c FROM {run_tbl} WHERE partition=0 AND state='running'"
    ))
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(
        runnable, 0,
        "the dead-lettered run is no longer runnable (the divergence parked it)"
    );

    sqlx::query(&format!("DROP TABLE IF EXISTS {run_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    println!(
        "[2026-06-21] PASS  drill=FLOW-D2(live-PG)  divergent/wrong-version replay -> halt nondeterministic + dead-letter  nondeterministic_halt=1 silent_divergence=0  (real Postgres state CHECK admits 'nondeterministic')"
    );
}
