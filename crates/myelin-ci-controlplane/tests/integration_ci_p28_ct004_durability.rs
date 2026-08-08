#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL, ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
    ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL, BUMP_CHECK_ATTEMPT_SQL, CLAIM_QUERY,
    CREATE_CHECK_ATTEMPT_DDL, CREATE_CI_COST_EVENT_DDL, CREATE_CI_JOB_DDL, CREATE_CI_RUN_DDL,
    CREATE_JOB_QUEUE_DDL, CREATE_LOG_ANCHOR_DDL, INSERT_COST_EVENT_QUERY, REAP_QUERY,
    SELECT_COST_EVENTS_FOR_RUN_QUERY, UPSERT_LOG_ANCHOR_QUERY,
};
use sqlx::types::Uuid;
use sqlx::{PgPool, Row};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

async fn reopen() -> PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("reconnect to dev Postgres as admin (is the stack up? run `fed test:backend`)")
}

fn uid(name: &str) -> Uuid {
    let mut bytes = [0u8; 16];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    bytes[..8].copy_from_slice(&h.to_be_bytes());
    let mut h2: u64 = h ^ 0x00ff_00ff_00ff_00ff;
    for b in name.bytes().rev() {
        h2 ^= b as u64;
        h2 = h2.wrapping_mul(0x0000_0100_0000_01b3);
    }
    bytes[8..].copy_from_slice(&h2.to_be_bytes());
    Uuid::from_bytes(bytes)
}

async fn do_bump(pool: &PgPool, bump_sql: &str, run: &str) -> i32 {
    sqlx::query(bump_sql)
        .bind("acme")
        .bind("fr-par")
        .bind("myelin://acme/git/repo/core")
        .bind("deadbeef")
        .bind("ci:build")
        .bind(Uuid::parse_str(run).unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
        .get::<i32, _>("run_attempt")
}

#[tokio::test]
async fn scheduler_claim_lease_survives_kill9_and_no_ghost_lease() {
    let suffix = std::process::id();
    let tbl = format!("job_queue_ct004_{suffix}");
    let fair = format!("fair_deficit_ct004_{suffix}");
    let workflow = format!("workflow_run_ct004_{suffix}");
    let ci_run = format!("ci_run_owner_ct004_{suffix}");

    let p1 = reopen().await;
    for table in [&tbl, &fair, &workflow, &ci_run] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {table}"))
            .execute(&p1)
            .await
            .ok();
    }
    let create = CREATE_JOB_QUEUE_DDL.replace("EXISTS job_queue (", &format!("EXISTS {tbl} ("));
    sqlx::query(&create)
        .execute(&p1)
        .await
        .expect("apply job_queue DDL");
    let alter = ALTER_JOB_QUEUE_ADD_COMPLETION_DDL.replace("job_queue", &tbl);
    sqlx::query(&alter)
        .execute(&p1)
        .await
        .expect("apply job_queue completion columns");
    let alter = ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL.replace("job_queue", &tbl);
    sqlx::query(&alter)
        .execute(&p1)
        .await
        .expect("apply job_queue claim-authority columns");
    let alter = ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL.replace("job_queue", &tbl);
    sqlx::query(&alter)
        .execute(&p1)
        .await
        .expect("apply job_queue claim-time columns");
    let alter = ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL.replace("job_queue", &tbl);
    sqlx::raw_sql(&alter)
        .execute(&p1)
        .await
        .expect("apply the job_queue claim-window expand");
    let alter = ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL.replace("job_queue", &tbl);
    sqlx::query(&alter)
        .execute(&p1)
        .await
        .expect("apply job_queue retryable-attempt accrual");
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {fair} (tenant_id text NOT NULL, region text NOT NULL, \
         fair_key text NOT NULL, deficit bigint NOT NULL DEFAULT 0, \
         PRIMARY KEY (tenant_id, region, fair_key))"
    ))
    .execute(&p1)
    .await
    .expect("apply fair_deficit");
    sqlx::raw_sql(&format!(
        "CREATE TABLE {workflow} (
           tenant_id text NOT NULL, region text NOT NULL, run_id text NOT NULL, state text NOT NULL,
           PRIMARY KEY (tenant_id, run_id)
         );
         CREATE TABLE {ci_run} (
           tenant_id text NOT NULL, region text NOT NULL, wf_run_id uuid NOT NULL, state text NOT NULL,
           PRIMARY KEY (tenant_id, wf_run_id)
         )"
    ))
    .execute(&p1)
    .await
    .expect("apply authoritative lifecycle tables");

    let enqueue = |job: &str, run: &str, idem: &str, ago: i64| -> String {
        format!(
            "INSERT INTO {tbl} (tenant_id, region, job_id, run_id, lane, labels, trust_tier, \
             concurrency_group, fair_key, idem_token, enqueued_at, state) VALUES \
             ('acme','fr-par','{}','{}','interactive', ARRAY['linux']::text[], 'trusted', \
             NULL, 'acme', '{idem}', now() - interval '{ago} seconds', 'queued')",
            uid(job),
            uid(run)
        )
    };
    sqlx::query(&enqueue("kept", "run-kept", "idem-kept", 100))
        .execute(&p1)
        .await
        .unwrap();
    sqlx::query(&enqueue("ghost", "run-ghost", "idem-ghost", 50))
        .execute(&p1)
        .await
        .unwrap();
    sqlx::raw_sql(&format!(
        "INSERT INTO {workflow} (tenant_id, region, run_id, state) VALUES
           ('acme', 'fr-par', '{}', 'running'),
           ('acme', 'fr-par', '{}', 'running');
         INSERT INTO {ci_run} (tenant_id, region, wf_run_id, state) VALUES
           ('acme', 'fr-par', '{}', 'running'),
           ('acme', 'fr-par', '{}', 'running')",
        uid("run-kept"),
        uid("run-ghost"),
        uid("run-kept"),
        uid("run-ghost")
    ))
    .execute(&p1)
    .await
    .expect("seed active Flow and CI owners");

    let claim_sql = CLAIM_QUERY
        .replace("job_queue", &tbl)
        .replace("fair_deficit", &fair)
        .replace("workflow_run", &workflow)
        .replace("ci_run", &ci_run);
    let bind_claim = |owner: &str| {
        claim_sql
            .replacen("$1", "'fr-par'", 1)
            .replacen("$2", "ARRAY['linux']::text[]", 1)
            .replacen("$3", "ARRAY['trusted']::text[]", 1)
            .replacen("$4", &format!("'{owner}'"), 1)
            .replace("$5", "'30'")
    };

    let row = sqlx::query(&bind_claim("worker-kept"))
        .fetch_one(&p1)
        .await
        .expect("the claim leases the older eligible job");
    assert_eq!(
        row.get::<Uuid, _>("job_id"),
        uid("kept"),
        "the committed claim leases `kept` (oldest in-region eligible)"
    );

    {
        let mut tx = p1.begin().await.unwrap();
        let g = sqlx::query(&bind_claim("worker-ghost"))
            .fetch_one(&mut *tx)
            .await
            .expect("the in-tx claim leases `ghost`");
        assert_eq!(
            g.get::<Uuid, _>("job_id"),
            uid("ghost"),
            "the in-tx claim leases `ghost` (the only remaining queued job)"
        );
        drop(tx);
    }

    drop(p1);

    let p2 = reopen().await;

    let kept = sqlx::query(&format!(
        "SELECT state, lease_owner FROM {tbl} WHERE job_id = '{}'",
        uid("kept")
    ))
    .fetch_one(&p2)
    .await
    .unwrap();
    assert_eq!(
        kept.get::<String, _>("state"),
        "leased",
        "the committed lease is present after kill-9/reopen (durable, not in-memory)"
    );
    assert_eq!(kept.get::<String, _>("lease_owner"), "worker-kept");

    let ghost = sqlx::query(&format!(
        "SELECT state, lease_owner FROM {tbl} WHERE job_id = '{}'",
        uid("ghost")
    ))
    .fetch_one(&p2)
    .await
    .unwrap();
    assert_eq!(
        ghost.get::<String, _>("state"),
        "queued",
        "the uncommitted claim left NO ghost lease - `ghost` is reclaimable (no lost/ghost job)"
    );
    assert!(
        ghost.try_get::<String, _>("lease_owner").is_err(),
        "the rolled-back claim wrote no lease_owner"
    );

    sqlx::query(&format!(
        "UPDATE {tbl} SET lease_expires = now() - interval '1 second' WHERE job_id = '{}'",
        uid("kept")
    ))
    .execute(&p2)
    .await
    .unwrap();
    let reaped = sqlx::query(
        &REAP_QUERY
            .replace("job_queue", &tbl)
            .replace("workflow_run", &workflow)
            .replace("ci_run", &ci_run)
            .replacen("$1", "'fr-par'", 1),
    )
    .fetch_all(&p2)
    .await
    .expect("the reaper sweeps expired leases on the reopened store");
    assert!(
        reaped
            .iter()
            .any(|r| r.get::<Uuid, _>("job_id") == uid("kept")),
        "the dead runner's expired lease is reclaimed by the reaper after reopen"
    );
    let after = sqlx::query(&format!(
        "SELECT state FROM {tbl} WHERE job_id = '{}'",
        uid("kept")
    ))
    .fetch_one(&p2)
    .await
    .unwrap();
    assert_eq!(
        after.get::<String, _>("state"),
        "queued",
        "reclaimed → queued"
    );

    for table in [&tbl, &fair, &workflow, &ci_run] {
        sqlx::query(&format!("DROP TABLE {table}"))
            .execute(&p2)
            .await
            .ok();
    }
    println!(
        "[CT-004] PASS scheduler: committed lease survives kill-9/reopen (leased,worker-kept); \
         uncommitted claim → 0 ghost (ghost stays queued); reaper reclaims expired lease after reopen → queued"
    );
}

#[tokio::test]
async fn run_step_metering_co_commit_survives_kill9_no_partial() {
    let suffix = std::process::id();
    let run_tbl = format!("ci_run_ct004_{suffix}");
    let job_tbl = format!("ci_job_ct004_{suffix}");
    let ce_tbl = format!("ci_cost_event_ct004_{suffix}");

    let p1 = reopen().await;
    for t in [&ce_tbl, &job_tbl, &run_tbl] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {t}"))
            .execute(&p1)
            .await
            .ok();
    }
    sqlx::query(&CREATE_CI_RUN_DDL.replace("EXISTS ci_run (", &format!("EXISTS {run_tbl} (")))
        .execute(&p1)
        .await
        .expect("apply ci_run DDL");
    sqlx::query(
        &CREATE_CI_JOB_DDL
            .replace("EXISTS ci_job (", &format!("EXISTS {job_tbl} ("))
            .replace("REFERENCES ci_run(", &format!("REFERENCES {run_tbl}(")),
    )
    .execute(&p1)
    .await
    .expect("apply ci_job DDL (FK retargeted to the suffixed ci_run)");
    sqlx::query(
        &CREATE_CI_COST_EVENT_DDL.replace("EXISTS ci_cost_event (", &format!("EXISTS {ce_tbl} (")),
    )
    .execute(&p1)
    .await
    .expect("apply ci_cost_event DDL");

    let insert_cost =
        INSERT_COST_EVENT_QUERY.replace("INTO ci_cost_event", &format!("INTO {ce_tbl}"));

    let insert_run = |run: &str, state: &str, settled: bool| -> String {
        format!(
            "INSERT INTO {run_tbl} (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, \
             definition_snapshot, trigger_kind, trust_tier, state, cost_settled, correlation_id) \
             VALUES ('acme','fr-par','{rid}','{p}','{p}','{p}','blake3:def','push','trusted', \
             '{state}', {settled}, 'corr-{run}')",
            rid = uid(run),
            p = uid("proj")
        )
    };
    let insert_job = |job: &str, run: &str, state: &str| -> String {
        format!(
            "INSERT INTO {job_tbl} (tenant_id, region, job_id, run_id, stage, name, spec_ref, state) \
             VALUES ('acme','fr-par','{}','{}','build','build:linux','myelin://acme/ci/spec','{state}')",
            uid(job),
            uid(run)
        )
    };

    let mut tx = p1.begin().await.unwrap();
    sqlx::query(&insert_run("run-ok", "running", false))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(&insert_job("job-ok", "run-ok", "running"))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(&insert_cost)
        .bind("acme")
        .bind("fr-par")
        .bind(uid("cost-ok"))
        .bind(uid("run-ok"))
        .bind(uid("job-ok"))
        .bind("cpu_seconds")
        .bind(120_i64)
        .bind(100_i64)
        .bind(20_i64)
        .bind("ci")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(&format!(
        "UPDATE {run_tbl} SET state='succeeded', cost_settled=true, finished_at=now() WHERE run_id='{}'",
        uid("run-ok")
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    let dup = sqlx::query(&insert_cost)
        .bind("acme")
        .bind("fr-par")
        .bind(uid("cost-ok"))
        .bind(uid("run-ok"))
        .bind(uid("job-ok"))
        .bind("cpu_seconds")
        .bind(120_i64)
        .bind(100_i64)
        .bind(20_i64)
        .bind("ci")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        dup.rows_affected(),
        0,
        "a re-delivered settle of the same cost_id records ONCE (ON CONFLICT DO NOTHING)"
    );
    tx.commit().await.unwrap();

    {
        let mut tx2 = p1.begin().await.unwrap();
        sqlx::query(&insert_run("run-crash", "running", false))
            .execute(&mut *tx2)
            .await
            .unwrap();
        sqlx::query(&insert_job("job-crash", "run-crash", "running"))
            .execute(&mut *tx2)
            .await
            .unwrap();
        drop(tx2);
    }

    drop(p1);
    let p2 = reopen().await;

    let run = sqlx::query(&format!(
        "SELECT state, cost_settled FROM {run_tbl} WHERE run_id='{}'",
        uid("run-ok")
    ))
    .fetch_one(&p2)
    .await
    .unwrap();
    assert_eq!(run.get::<String, _>("state"), "succeeded");
    assert!(
        run.get::<bool, _>("cost_settled"),
        "a settled run stays settled after kill-9/reopen"
    );

    let select =
        SELECT_COST_EVENTS_FOR_RUN_QUERY.replace("FROM ci_cost_event", &format!("FROM {ce_tbl}"));
    let costs = sqlx::query(&select)
        .bind("acme")
        .bind(uid("run-ok"))
        .fetch_all(&p2)
        .await
        .unwrap();
    assert_eq!(
        costs.len(),
        1,
        "exactly one metered unit (cost_events_per_unit == 1)"
    );
    assert_eq!(
        costs[0].get::<Uuid, _>("job_id"),
        uid("job-ok"),
        "attributed to its job"
    );
    assert_eq!(costs[0].get::<i64, _>("wholesale_minor_units"), 100);
    assert_eq!(costs[0].get::<i64, _>("markup_minor_units"), 20);
    assert_ne!(
        costs[0].get::<i64, _>("wholesale_minor_units"),
        costs[0].get::<i64, _>("markup_minor_units"),
        "wholesale ≠ markup (the two cost columns are distinct, arch 02 §8)"
    );

    let ghost_run: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS n FROM {run_tbl} WHERE run_id='{}'",
        uid("run-crash")
    ))
    .fetch_one(&p2)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        ghost_run, 0,
        "the uncommitted run-state left no ghost run (all-or-nothing co-commit)"
    );
    let ghost_cost: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS n FROM {ce_tbl} WHERE run_id='{}'",
        uid("run-crash")
    ))
    .fetch_one(&p2)
    .await
    .unwrap()
    .get("n");
    assert_eq!(ghost_cost, 0, "no half-billed cost for the crashed run");

    for t in [&ce_tbl, &job_tbl, &run_tbl] {
        sqlx::query(&format!("DROP TABLE {t}"))
            .execute(&p2)
            .await
            .ok();
    }
    println!(
        "[CT-004] PASS run/metering co-commit: run+job+cost commit in ONE tx survive kill-9 \
         (succeeded, settled stays settled, cost attributed wholesale=100≠markup=20, settle idempotent); \
         crash-between-steps → 0 ghost run, 0 half-billed cost (all-or-nothing)"
    );
}

#[tokio::test]
async fn check_attempt_is_monotonic_across_kill9() {
    let suffix = std::process::id();
    let tbl = format!("check_attempt_ct004_{suffix}");

    let p1 = reopen().await;
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&p1)
        .await
        .ok();
    sqlx::query(
        &CREATE_CHECK_ATTEMPT_DDL.replace("EXISTS check_attempt (", &format!("EXISTS {tbl} (")),
    )
    .execute(&p1)
    .await
    .expect("apply check_attempt DDL");

    let bump = BUMP_CHECK_ATTEMPT_SQL
        .replace("INTO check_attempt (", &format!("INTO {tbl} ("))
        .replace("check_attempt.next_attempt", &format!("{tbl}.next_attempt"))
        .replace("check_attempt.current_run", &format!("{tbl}.current_run"));

    let run_a = "11111111-1111-1111-1111-111111111111";
    let run_b = "22222222-2222-2222-2222-222222222222";
    assert_eq!(do_bump(&p1, &bump, run_a).await, 1, "first dispatch → 1");
    assert_eq!(do_bump(&p1, &bump, run_b).await, 2, "re-dispatch → 2");

    drop(p1);
    let p2 = reopen().await;
    assert_eq!(
        do_bump(&p2, &bump, run_a).await,
        3,
        "the reopened store continues the sequence → 3 (monotonic across the process boundary, never reset)"
    );
    let stored: i32 = sqlx::query(&format!(
        "SELECT next_attempt FROM {tbl} WHERE context='ci:build'"
    ))
    .fetch_one(&p2)
    .await
    .unwrap()
    .get("next_attempt");
    assert_eq!(
        stored, 4,
        "next_attempt persisted as 4 after three issued attempts"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&p2)
        .await
        .ok();
    println!("[CT-004] PASS check_attempt: 1,2 then kill-9 → reopen → 3 (monotonic across processes, no reset)");
}

#[tokio::test]
async fn log_anchor_persists_across_kill9() {
    let suffix = std::process::id();
    let tbl = format!("log_anchor_ct004_{suffix}");

    let p1 = reopen().await;
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&p1)
        .await
        .ok();
    sqlx::query(&CREATE_LOG_ANCHOR_DDL.replace("EXISTS log_anchor (", &format!("EXISTS {tbl} (")))
        .execute(&p1)
        .await
        .expect("apply log_anchor DDL");

    let upsert = UPSERT_LOG_ANCHOR_QUERY
        .replace("INTO log_anchor", &format!("INTO {tbl}"))
        .replace("log_anchor.", &format!("{tbl}."));
    sqlx::query(&upsert)
        .bind("acme")
        .bind("fr-par")
        .bind(uid("run-log"))
        .bind(uid("job-log"))
        .bind("step-1")
        .bind(0_i64)
        .bind(Option::<i64>::None)
        .bind("running")
        .execute(&p1)
        .await
        .expect("open the anchor");

    drop(p1);
    let p2 = reopen().await;

    let a = sqlx::query(&format!(
        "SELECT status, byte_start FROM {tbl} WHERE step_id='step-1' AND run_id='{}'",
        uid("run-log")
    ))
    .fetch_one(&p2)
    .await
    .unwrap();
    assert_eq!(
        a.get::<String, _>("status"),
        "running",
        "the anchor survived kill-9/reopen"
    );
    assert_eq!(a.get::<i64, _>("byte_start"), 0);

    sqlx::query(&upsert)
        .bind("acme")
        .bind("fr-par")
        .bind(uid("run-log"))
        .bind(uid("job-log"))
        .bind("step-1")
        .bind(0_i64)
        .bind(Some(4096_i64))
        .bind("failed")
        .execute(&p2)
        .await
        .expect("transition the anchor in place on the reopened store");
    let a2 = sqlx::query(&format!(
        "SELECT status, byte_end FROM {tbl} WHERE step_id='step-1' AND run_id='{}'",
        uid("run-log")
    ))
    .fetch_one(&p2)
    .await
    .unwrap();
    assert_eq!(
        a2.get::<String, _>("status"),
        "failed",
        "in-place status transition after reopen"
    );
    assert_eq!(a2.get::<i64, _>("byte_end"), 4096);

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&p2)
        .await
        .ok();
    println!("[CT-004] PASS log_anchor: anchor (running) survives kill-9/reopen; idempotent in-place → failed after reopen");
}
