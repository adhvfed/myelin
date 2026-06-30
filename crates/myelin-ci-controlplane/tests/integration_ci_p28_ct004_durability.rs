//! **CT-004 / E2.3 — CI backend HARDEN: the load-bearing CI control-plane state is DURABLE on real
//! Postgres, KILL-9 PROVEN.**
//!
//! The census (CT-004) found every CI control-plane SQL contract proven *in-tx* on live Postgres
//! (CI-P12 claim/reaper, CI-P18 check_attempt, CI-P20 anchors) — but NONE proven across a process
//! boundary, and metering had NO durable Postgres impl at all (model-only). This is the spine's
//! durability discipline (MR-022..025) applied to the CI backend: **state written + committed before a
//! simulated crash is present after a FRESH store opens; an uncommitted write leaves no ghost; a
//! one-tx co-commit is all-or-nothing across a crash between steps.**
//!
//! The "kill-9 / reopen" is modelled the way the spine models it: the work commits through one
//! `PgPool`, that pool is **dropped without a graceful close** (the process "dies"), and a brand-new
//! pool reconnects to the SAME live Postgres and reads the state back — proving the state lived in
//! Postgres, never in process memory. The SQL under test is the BYTE-IDENTICAL production constant
//! (`CLAIM_QUERY` / `REAP_QUERY` / `BUMP_CHECK_ATTEMPT_SQL` / `INSERT_COST_EVENT_QUERY` /
//! `UPSERT_LOG_ANCHOR_QUERY`); only the table identifier is per-pid suffixed for isolation + cleanup
//! (the CI-P12/P18 convention). The in-memory models stay the DB-free default; this proves the same
//! semantics hold durably.
//!
//! Gated behind the `integration` cargo feature so the default `cargo build`/`cargo test --workspace`
//! stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_p28_ct004_durability -- --nocapture
#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    BUMP_CHECK_ATTEMPT_SQL, CLAIM_QUERY, CREATE_CHECK_ATTEMPT_DDL, CREATE_CI_JOB_DDL,
    CREATE_CI_RUN_DDL, CREATE_COST_EVENT_DDL, CREATE_JOB_QUEUE_DDL, CREATE_LOG_ANCHOR_DDL,
    INSERT_COST_EVENT_QUERY, REAP_QUERY, SELECT_COST_EVENTS_FOR_RUN_QUERY, UPSERT_LOG_ANCHOR_QUERY,
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

/// Open a FRESH admin pool — a brand-new connection to the SAME live Postgres. Calling this after
/// `drop(prev_pool)` models a process restart (the "reopen" half of a kill-9 durability proof: the
/// only way the state is still here is if it lives in Postgres).
async fn reopen() -> PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("reconnect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)")
}

/// A stable uuid from a name (the `uuid` columns) — deterministic (a simple FNV-1a fill, no extra
/// crate feature) so a reopened pool can assert equality against the SAME id the pre-crash pool wrote.
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

/// Run the production `check_attempt` bump (retargeted to a suffixed table) and return the stamped
/// `run_attempt`. A free async fn (not a closure) so the returned future does not borrow the `&str`s.
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

// =================================================================================================
// 1. Scheduler claim/lease/reaper — the committed lease survives kill-9; an uncommitted claim leaves
//    NO ghost lease; an expired lease is reclaimable AFTER a reopen (the reaper seam, CI-P12).
// =================================================================================================
#[tokio::test]
async fn scheduler_claim_lease_survives_kill9_and_no_ghost_lease() {
    let suffix = std::process::id();
    let tbl = format!("job_queue_ct004_{suffix}");
    let fair = format!("fair_deficit_ct004_{suffix}");

    let p1 = reopen().await;
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&p1)
        .await
        .ok();
    sqlx::query(&format!("DROP TABLE IF EXISTS {fair}"))
        .execute(&p1)
        .await
        .ok();
    let create = CREATE_JOB_QUEUE_DDL.replace("EXISTS job_queue (", &format!("EXISTS {tbl} ("));
    sqlx::query(&create).execute(&p1).await.expect("apply job_queue DDL");
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {fair} (tenant_id text NOT NULL, region text NOT NULL, \
         fair_key text NOT NULL, deficit bigint NOT NULL DEFAULT 0, \
         PRIMARY KEY (tenant_id, region, fair_key))"
    ))
    .execute(&p1)
    .await
    .expect("apply fair_deficit");

    // Seed two eligible queued jobs: `kept` (claimed + committed) and `ghost` (claimed in an aborted tx).
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

    let claim_sql = CLAIM_QUERY.replace("job_queue", &tbl).replace("fair_deficit", &fair);
    let bind_claim = |owner: &str| {
        claim_sql
            .replacen("$1", "'fr-par'", 1)
            .replacen("$2", "ARRAY['linux']::text[]", 1)
            .replacen("$3", "ARRAY['trusted']::text[]", 1)
            .replacen("$4", &format!("'{owner}'"), 1)
            .replacen("$5", "'30'", 1)
    };

    // ── COMMITTED claim: lease `kept` (the older job wins) on the pool (autocommit) — durable. ──
    let row = sqlx::query(&bind_claim("worker-kept"))
        .fetch_one(&p1)
        .await
        .expect("the claim leases the older eligible job");
    assert_eq!(
        row.get::<Uuid, _>("job_id"),
        uid("kept"),
        "the committed claim leases `kept` (oldest in-region eligible)"
    );

    // ── UNCOMMITTED claim: lease `ghost` inside a tx, then DROP the tx (the process dies before
    //    commit). sqlx rolls the tx back — there must be NO ghost lease after reopen. ──
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
        // Drop `tx` WITHOUT commit → rollback (the crash before commit).
        drop(tx);
    }

    // ── KILL-9: drop the pool without a graceful close. The process "dies". ──
    drop(p1);

    // ── REOPEN: a brand-new pool reconnects to the SAME Postgres. ──
    let p2 = reopen().await;

    // The COMMITTED lease survived the crash.
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

    // The UNCOMMITTED claim left NO ghost: `ghost` is still queued (reclaimable) — no half-claimed row.
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
        "the uncommitted claim left NO ghost lease — `ghost` is reclaimable (no lost/ghost job)"
    );
    assert!(
        ghost.try_get::<String, _>("lease_owner").is_err(),
        "the rolled-back claim wrote no lease_owner"
    );

    // ── REAPER after reopen: a claimed-but-crashed runner's lease expires → reclaimable. Force the
    //    `kept` lease into the past (its owner died) and run the production reaper on the fresh pool. ──
    sqlx::query(&format!(
        "UPDATE {tbl} SET lease_expires = now() - interval '1 second' WHERE job_id = '{}'",
        uid("kept")
    ))
    .execute(&p2)
    .await
    .unwrap();
    let reaped = sqlx::query(&REAP_QUERY.replace("job_queue", &tbl).replacen("$1", "'fr-par'", 1))
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
    assert_eq!(after.get::<String, _>("state"), "queued", "reclaimed → queued");

    sqlx::query(&format!("DROP TABLE {tbl}")).execute(&p2).await.ok();
    sqlx::query(&format!("DROP TABLE {fair}")).execute(&p2).await.ok();
    println!(
        "[CT-004] PASS scheduler: committed lease survives kill-9/reopen (leased,worker-kept); \
         uncommitted claim → 0 ghost (ghost stays queued); reaper reclaims expired lease after reopen → queued"
    );
}

// =================================================================================================
// 2. Run/step state + metering co-commit — ci_run + ci_job + cost_event commit in ONE tx and survive
//    kill-9 (settled stays settled, cost attributed to (run_id, job_id)); a crash BETWEEN steps (no
//    commit) leaves NOTHING (no half-billed run, no ghost run-state). The spine's one-tx rule.
// =================================================================================================
#[tokio::test]
async fn run_step_metering_co_commit_survives_kill9_no_partial() {
    let suffix = std::process::id();
    let run_tbl = format!("ci_run_ct004_{suffix}");
    let job_tbl = format!("ci_job_ct004_{suffix}");
    let ce_tbl = format!("cost_event_ct004_{suffix}");

    let p1 = reopen().await;
    for t in [&ce_tbl, &job_tbl, &run_tbl] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {t}")).execute(&p1).await.ok();
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
    sqlx::query(&CREATE_COST_EVENT_DDL.replace("EXISTS cost_event (", &format!("EXISTS {ce_tbl} (")))
        .execute(&p1)
        .await
        .expect("apply cost_event DDL");

    let insert_cost = INSERT_COST_EVENT_QUERY.replace("INTO cost_event", &format!("INTO {ce_tbl}"));

    // Insert a run + its job, transition the run terminal, AND record its metered cost — all in ONE tx
    // (the run-state/metering co-commit: a crash here must not half-bill nor leave a ghost run).
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

    // ── The committed co-commit. ──
    let mut tx = p1.begin().await.unwrap();
    sqlx::query(&insert_run("run-ok", "running", false))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(&insert_job("job-ok", "run-ok", "running"))
        .execute(&mut *tx)
        .await
        .unwrap();
    // Record the metered cost via the production durable settle query (wholesale ≠ markup).
    sqlx::query(&insert_cost)
        .bind("acme")
        .bind("fr-par")
        .bind(uid("cost-ok"))
        .bind(uid("run-ok"))
        .bind(uid("job-ok"))
        .bind("cpu_seconds")
        .bind(120_i64)
        .bind(100_i64) // wholesale
        .bind(20_i64) // markup (distinct from wholesale)
        .bind("ci")
        .execute(&mut *tx)
        .await
        .unwrap();
    // Transition run terminal + settle in the SAME tx (the co-commit bookend).
    sqlx::query(&format!(
        "UPDATE {run_tbl} SET state='succeeded', cost_settled=true, finished_at=now() WHERE run_id='{}'",
        uid("run-ok")
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    // Exactly-once: a re-delivered settle of the same cost_id is a no-op (double-effect = 0).
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

    // ── Crash BETWEEN steps: begin a second run, write run + job, but record NO cost + NO commit. ──
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
        // Drop tx2 WITHOUT commit → the crash before the co-commit completes.
        drop(tx2);
    }

    // ── KILL-9 + REOPEN. ──
    drop(p1);
    let p2 = reopen().await;

    // The committed run is durable + SETTLED stays settled across the crash.
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

    // The cost is durable + ATTRIBUTED to its (run_id, job_id), with wholesale ≠ markup (two columns).
    let select = SELECT_COST_EVENTS_FOR_RUN_QUERY.replace("FROM cost_event", &format!("FROM {ce_tbl}"));
    let costs = sqlx::query(&select)
        .bind("acme")
        .bind(uid("run-ok"))
        .fetch_all(&p2)
        .await
        .unwrap();
    assert_eq!(costs.len(), 1, "exactly one metered unit (cost_events_per_unit == 1)");
    assert_eq!(costs[0].get::<Uuid, _>("job_id"), uid("job-ok"), "attributed to its job");
    assert_eq!(costs[0].get::<i64, _>("wholesale_minor_units"), 100);
    assert_eq!(costs[0].get::<i64, _>("markup_minor_units"), 20);
    assert_ne!(
        costs[0].get::<i64, _>("wholesale_minor_units"),
        costs[0].get::<i64, _>("markup_minor_units"),
        "wholesale ≠ markup (the two cost columns are distinct, arch 02 §8)"
    );

    // The crashed-between-steps run left NOTHING — no ghost run-state, no half-billed cost.
    let ghost_run: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS n FROM {run_tbl} WHERE run_id='{}'",
        uid("run-crash")
    ))
    .fetch_one(&p2)
    .await
    .unwrap()
    .get("n");
    assert_eq!(ghost_run, 0, "the uncommitted run-state left no ghost run (all-or-nothing co-commit)");
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
        sqlx::query(&format!("DROP TABLE {t}")).execute(&p2).await.ok();
    }
    println!(
        "[CT-004] PASS run/metering co-commit: run+job+cost commit in ONE tx survive kill-9 \
         (succeeded, settled stays settled, cost attributed wholesale=100≠markup=20, settle idempotent); \
         crash-between-steps → 0 ghost run, 0 half-billed cost (all-or-nothing)"
    );
}

// =================================================================================================
// 3. check_attempt monotonic ACROSS processes — the counter persists across a kill-9; the reopened
//    store continues the sequence (never resets to 1). CI is the durable source of run_attempt (X-1).
// =================================================================================================
#[tokio::test]
async fn check_attempt_is_monotonic_across_kill9() {
    let suffix = std::process::id();
    let tbl = format!("check_attempt_ct004_{suffix}");

    let p1 = reopen().await;
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}")).execute(&p1).await.ok();
    sqlx::query(&CREATE_CHECK_ATTEMPT_DDL.replace("EXISTS check_attempt (", &format!("EXISTS {tbl} (")))
        .execute(&p1)
        .await
        .expect("apply check_attempt DDL");

    let bump = BUMP_CHECK_ATTEMPT_SQL
        .replace("INTO check_attempt (", &format!("INTO {tbl} ("))
        .replace("check_attempt.next_attempt", &format!("{tbl}.next_attempt"));

    let run_a = "11111111-1111-1111-1111-111111111111";
    let run_b = "22222222-2222-2222-2222-222222222222";
    assert_eq!(do_bump(&p1, &bump, run_a).await, 1, "first dispatch → 1");
    assert_eq!(do_bump(&p1, &bump, run_b).await, 2, "re-dispatch → 2");

    // ── KILL-9 mid-sequence + REOPEN. The counter must NOT reset. ──
    drop(p1);
    let p2 = reopen().await;
    assert_eq!(
        do_bump(&p2, &bump, run_a).await,
        3,
        "the reopened store continues the sequence → 3 (monotonic across the process boundary, never reset)"
    );
    let stored: i32 = sqlx::query(&format!("SELECT next_attempt FROM {tbl} WHERE context='ci:build'"))
        .fetch_one(&p2)
        .await
        .unwrap()
        .get("next_attempt");
    assert_eq!(stored, 4, "next_attempt persisted as 4 after three issued attempts");

    sqlx::query(&format!("DROP TABLE {tbl}")).execute(&p2).await.ok();
    println!("[CT-004] PASS check_attempt: 1,2 then kill-9 → reopen → 3 (monotonic across processes, no reset)");
}

// =================================================================================================
// 4. Log anchor (the ci.log.available pointer index) — the anchor persists across kill-9 and its
//    idempotent in-place status transition survives the reopen (CI-P20).
// =================================================================================================
#[tokio::test]
async fn log_anchor_persists_across_kill9() {
    let suffix = std::process::id();
    let tbl = format!("log_anchor_ct004_{suffix}");

    let p1 = reopen().await;
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}")).execute(&p1).await.ok();
    sqlx::query(&CREATE_LOG_ANCHOR_DDL.replace("EXISTS log_anchor (", &format!("EXISTS {tbl} (")))
        .execute(&p1)
        .await
        .expect("apply log_anchor DDL");

    let upsert = UPSERT_LOG_ANCHOR_QUERY.replace("INTO log_anchor", &format!("INTO {tbl}"));
    // Open the anchor (running) and commit it.
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
    assert_eq!(a.get::<String, _>("status"), "running", "the anchor survived kill-9/reopen");
    assert_eq!(a.get::<i64, _>("byte_start"), 0);

    // The idempotent in-place transition (running → failed, byte_end set) applies on the reopened store.
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
    assert_eq!(a2.get::<String, _>("status"), "failed", "in-place status transition after reopen");
    assert_eq!(a2.get::<i64, _>("byte_end"), 4096);

    sqlx::query(&format!("DROP TABLE {tbl}")).execute(&p2).await.ok();
    println!("[CT-004] PASS log_anchor: anchor (running) survives kill-9/reopen; idempotent in-place → failed after reopen");
}
