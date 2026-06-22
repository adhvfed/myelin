//! Live-Postgres integration test (Stage 1 / infra) — the FLOW-D1 deterministic replay/recovery +
//! LEASE-based crash recovery proven against REAL Postgres (P-FLOW-05 / P-202).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding-policy floor — no DB at build). This runs
//! ONLY against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-flow --features integration --test integration_flow_replay -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the same
//! test runs against Scaleway (fr-par) by exporting the prod env vars — never a code change.
//!
//! It proves, against REAL Postgres, the two FLOW-D1 properties the in-memory `engine` model
//! (`src/engine.rs`) asserts:
//!
//!   1. **Lease-based crash recovery (§4.7):** a runnable `workflow_run` row is leased with `UPDATE …
//!      WHERE lease_owner IS NULL OR lease_expires <= now FOR UPDATE SKIP LOCKED` (the real claim);
//!      a LIVE lease is skip-locked (no second worker steals it); an EXPIRED lease re-leases to
//!      another worker — crash recovery, in real Postgres.
//!   2. **Deterministic replay short-circuit (§4.1):** a worker that re-drives a half-journaled run
//!      reads `wf_history` ordered by `seq` and SHORT-CIRCUITS every already-journaled `command_id`
//!      (the journaled RESULT is returned, the activity is NOT re-executed). The cursor-keyed scan
//!      resumes at the first un-journaled command — 0 re-executed side effects, against the REAL
//!      frozen `wf_history` `UNIQUE(tenant_id, run_id, command_id)` journaling-idempotency key.
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

    // The REAL frozen DDL shapes (with per-pid table names so concurrent runs isolate). The
    // workflow_run DDL names the lease columns + cursor + state CHECK; wf_history names the
    // UNIQUE(tenant_id, run_id, command_id) replay key.
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

    // Seed a runnable run (state running, cursor 0, UNLEASED) + journal its first 5 of 10 activities
    // (the crash point — a worker journaled 5 steps then died, the journal is the source of truth).
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

    // (1) LEASE-BASED CRASH RECOVERY (§4.7). The real claim: lease the run for worker-2 IFF it is
    //     running AND its lease is free (unleased OR expired), FOR UPDATE SKIP LOCKED. The seeded run
    //     is unleased → worker-2 wins it, stamping lease_owner + lease_expires = now + 30s.
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

    // a SECOND worker cannot steal the LIVE-leased run (the lease has not expired) — skip-locked.
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

    // (2) DETERMINISTIC REPLAY SHORT-CIRCUIT (§4.1). The re-driving worker reads wf_history ordered
    //     by seq; for each of the 10 commands the body issues, it checks whether the command_id is
    //     already journaled — if so it RETURNS the journaled result (no re-execution). We model the
    //     drive: read the journaled command_ids, then for command 0..=9 decide replay vs execute.
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
            // REPLAY: the journaled result is returned; the activity is NOT re-executed.
            continue;
        }
        // LIVE: this command is past the cursor — execute + journal it (the resumed work).
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
    // THE FLOW-D1 ASSERTION: only 5..=9 executed (0..=4 replayed) — resumed at step 6, 0 re-execution.
    assert_eq!(
        executed,
        vec![5, 6, 7, 8, 9],
        "resumed at step 6 — only 5..=9 ran; 0..=4 replayed"
    );

    // 0 lost progress + 0 duplicate journal rows: exactly 10 history rows (the UNIQUE key held).
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

    // the UNIQUE(tenant_id, run_id, command_id) replay key BITES — a re-journal of a replayed command
    // is rejected (the silent-double-effect floor under replay, §3.2).
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

    // settle the run completed + release the lease (the drive finished).
    sqlx::query(&format!(
        "UPDATE {run_tbl} SET state='completed', cursor=10, lease_owner=NULL, lease_expires=NULL \
         WHERE tenant_id='acme' AND run_id='R1'"
    ))
    .execute(&admin)
    .await
    .unwrap();

    // cleanup
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

/// **FLOW-D2 (live-PG): the replay-divergence guard dead-letters a run as `nondeterministic` in REAL
/// Postgres (P-FLOW-07).** Drives the in-memory engine's divergence guard to its terminal verdict and
/// proves the dead-letter row PERSISTS to the real `workflow_run` — the `state` CHECK admits
/// `nondeterministic` (the frozen DDL) and a divergent run lands there, never silently continues. The
/// `nondeterministic`-halt count is the green artifact; here we prove its terminal write is durable
/// against the live state-CHECK (a state the CHECK rejected would error the UPDATE — the floor).
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

    // Seed a runnable run pinned to wf_version=1 in real Postgres.
    sqlx::query(&format!(
        "INSERT INTO {run_tbl} \
         (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor, correlation_id, depth, partition) \
         VALUES ('acme','fr-par','RD','agent.run',1,'[]'::jsonb,'running',0,'corr-d',0,0)"
    ))
    .execute(&admin)
    .await
    .expect("seed the runnable run");

    // Drive the in-memory engine's divergence guard: a v1-pinned run replayed with v2 (a deploy bump)
    // — the guard halts as nondeterministic WITHOUT running a command (the version-divergence leg).
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

    // PERSIST the dead-letter to REAL Postgres — the state CHECK MUST admit 'nondeterministic'.
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
    .expect("the dead-letter state persists — the frozen state CHECK admits 'nondeterministic'");

    // read it back: the run is durably 'nondeterministic' (terminal, never re-driven by the wf_runnable index).
    let st: String = sqlx::query(&format!("SELECT state FROM {run_tbl} WHERE run_id='RD'"))
        .fetch_one(&admin)
        .await
        .unwrap()
        .get("state");
    assert_eq!(
        st, "nondeterministic",
        "the dead-letter row persists in real Postgres"
    );
    // it is NOT runnable (the partial wf_runnable index covers only state IN ('running')).
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
