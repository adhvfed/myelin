//! **CT-004c.1 — the DURABLE `job_queue` store + the dead-runner reaper, PROVEN against live Postgres.**
//!
//! The sibling `integration_ci_p12_scheduler_claim.rs` proved the raw claim/reap/cancel SQL against a
//! per-pid SCRATCH table (string-replacing `job_queue`), NOT through a production store (there was
//! none — the scheduler was two-form: the `&str` SQL + the DB-free `SchedulerState` model). CT-004c.1
//! builds the real store ([`myelin_ci_controlplane::CiJobQueueStore`]) and re-proves the whole claim
//! intelligence THROUGH it, under the tenant-/region-scoped RLS transactions the FORCE-RLS `job_queue`
//! table requires:
//!
//!   1. **claim** leases exactly ONE eligible row — residency (out-of-region NOT claimed) + affinity
//!      (wrong-label NOT claimed) + trust (untrusted/self-hosted NOT claimed by a trusted-only claim)
//!      + lane order (interactive before an older batch) honoured.
//!   2. **SECURITY (the seam CT-004c.2 depends on):** a claim listing only trusted tiers NEVER leases
//!      an `untrusted_fork` / `self_hosted` job — the predicate that keeps untrusted code off trusted
//!      runners.
//!   3. **SKIP LOCKED:** two CONCURRENT claims take DIFFERENT rows (0 double-lease).
//!   4. **reap** re-queues an expired dead lease in place (0 orphans), a re-claim works, and the
//!      re-dispatch is idempotent (`jq_idem` → DuplicateIdem, 0 duplicate enqueue).
//!   5. **cancel_superseded** terminalises a prior `pr:%` head, keeping the latest.
//!   6. **Kill-9/reopen:** a leased row survives a pool drop + reopen (durable, not in-process); an
//!      UNCOMMITTED enqueue leaves NO ghost row after reopen.
//!   7. **Tenant-scoped RLS under the APP role (NOBYPASSRLS):** the store's tenant-scoped `enqueue`
//!      inserts correctly under `myelin_app`, and a read under a DIFFERENT tenant GUC sees 0 rows
//!      (isolation) — proving the `with_tenant_tx` seam works, not just as admin.
//!
//! Roles (honest): the region-scoped, cross-tenant claim/reap (the DRR fairness spans tenants by
//! design, arch 02 §2.2) run under the migration/owner role (`myelin_admin`, a superuser — RLS
//! bypassed), the SAME role the P12 test uses; a region-scoped scheduler DB role is the named
//! follow-on. The TENANT-scoped writes are ALSO proven under the non-superuser app role (case 7).
//!
//! Gated behind the `integration` cargo feature. Run against the docker-compose dev stack:
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_ct004c_job_queue_store -- --nocapture
#![cfg(feature = "integration")]

mod common;

use common::with_schema_cleanup;
use myelin_ci_controlplane::CI_RUNNER_EXECUTION_LEASE_TTL_SECS;
use myelin_ci_controlplane::{
    ci_job_queue_store, ci_region_queue_store_test_support, make_tenant_scoped_ddl,
    CiJobLaunchClaim, CiJobQueueStore, DurableEnqueue, EnqueueOutcome, Lane,
    ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL, ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
    ALTER_JOB_QUEUE_ADD_RESERVATION_WRITE_VERSION_DDL, ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL,
    CREATE_FAIR_DEFICIT_DDL, CREATE_JOB_QUEUE_DDL, CREATE_JOB_QUEUE_INDEXES_DDL,
    INSERT_JOB_QUEUE_QUERY,
};
use myelin_ci_sandbox::TrustTier;
use sqlx::types::Uuid;
use sqlx::{Executor, PgPool, Row};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// The per-pid schema every pool pins `search_path` to — so the store's unqualified `job_queue`
/// resolves to an ISOLATED table (never another test's rows / the real shared `job_queue`).
fn schema_name() -> String {
    format!("ci_ct004c_{}", std::process::id())
}

/// A FRESH admin (superuser) pool with `search_path` pinned to the per-pid schema. Reopening after a
/// `drop(prev)` models a process restart (the kill-9 "reopen" half).
async fn reopen_admin() -> PgPool {
    let schema = schema_name();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _meta| {
            let schema = schema.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? eval \"$(scripts/dev-stack.sh env)\")")
}

/// A pool for the NON-superuser app role (`myelin_app`, NOBYPASSRLS), `search_path` pinned to the
/// per-pid schema — the role under which RLS is actually ENFORCED (case 7).
async fn app_pool() -> PgPool {
    let schema = schema_name();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |conn, _meta| {
            let schema = schema.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the app role")
}

/// A stable uuid from a name (a deterministic FNV-1a fill) — so a reopened pool asserts equality
/// against the SAME id the pre-crash pool wrote.
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

#[allow(clippy::too_many_arguments)]
fn job(
    tenant: &str,
    region: &str,
    id: &str,
    lane: Lane,
    labels: &[&str],
    trust: TrustTier,
    group: Option<&str>,
    idem: &str,
) -> DurableEnqueue {
    DurableEnqueue {
        tenant_id: tenant.into(),
        region: region.into(),
        job_id: uid(id).to_string(),
        run_id: uid(&format!("run-{id}")).to_string(),
        lane,
        labels: labels.iter().map(|s| s.to_string()).collect(),
        trust_tier: trust,
        concurrency_group: group.map(|g| g.into()),
        fair_key: tenant.into(),
        idem_token: idem.into(),
        stage: "build".into(),
        claim_window_secs: CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
        reservation_write_version: myelin_ci_controlplane::ReservationWriteVersionMarker::legacy(),
    }
}

/// Build the isolated queue, fairness, and authoritative lifecycle tables in the per-pid schema.
/// Claim/reap must see a real active Flow + CI owner; the minimal lifecycle tables keep that
/// production predicate load-bearing without importing unrelated schema into this focused store
/// test. Queue/fairness tables are FORCE-RLS via the platform helper. Indexes are applied with
/// `CONCURRENTLY` stripped (a fresh empty table takes no meaningful lock; CONCURRENTLY cannot run in
/// the implicit transaction).
async fn create_schema(admin: &PgPool, schema: &str) {
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop any prior schema");
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create the per-pid schema");
    admin
        .execute(CREATE_JOB_QUEUE_DDL)
        .await
        .expect("create job_queue");
    admin
        .execute(ALTER_JOB_QUEUE_ADD_COMPLETION_DDL)
        .await
        .expect("add job_queue lease_epoch + completion_receipt");
    admin
        .execute(ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL)
        .await
        .expect("add job_queue claim nonce + stage authority");
    admin
        .execute(ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL)
        .await
        .expect("add persisted job_queue claim times");
    admin
        .execute(ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL)
        .await
        .expect("add retryable-attempt usage accrual");
    admin
        .execute(ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL)
        .await
        .expect("add the durable claim window");
    admin
        .execute(ALTER_JOB_QUEUE_ADD_RESERVATION_WRITE_VERSION_DDL)
        .await
        .expect("add the reservation writer marker");
    for (_name, idx) in CREATE_JOB_QUEUE_INDEXES_DDL {
        let idx = idx.replace("CONCURRENTLY ", "");
        admin
            .execute(idx.as_str())
            .await
            .expect("create a job_queue index");
    }
    admin
        .execute(make_tenant_scoped_ddl("job_queue").as_str())
        .await
        .expect("FORCE-RLS job_queue");
    admin
        .execute(CREATE_FAIR_DEFICIT_DDL)
        .await
        .expect("create fair_deficit");
    admin
        .execute(make_tenant_scoped_ddl("fair_deficit").as_str())
        .await
        .expect("FORCE-RLS fair_deficit");
    admin
        .execute(
            "CREATE TABLE workflow_run (
               tenant_id text NOT NULL,
               region text NOT NULL,
               run_id text NOT NULL,
               state text NOT NULL,
               PRIMARY KEY (tenant_id, run_id)
             )",
        )
        .await
        .expect("create authoritative Flow lifecycle table");
    admin
        .execute(
            "CREATE TABLE ci_run (
               tenant_id text NOT NULL,
               region text NOT NULL,
               wf_run_id uuid NOT NULL,
               state text NOT NULL,
               PRIMARY KEY (tenant_id, wf_run_id)
             )",
        )
        .await
        .expect("create authoritative CI lifecycle table");
    admin
        .execute(
            "CREATE TABLE ci_job (
               tenant_id text NOT NULL,
               region text NOT NULL,
               job_id uuid NOT NULL,
               state text NOT NULL,
               PRIMARY KEY (tenant_id, job_id)
             )",
        )
        .await
        .expect("create public CI job surface table");
    // Let the non-superuser app role reach the per-pid schema's tables (case 7). Default privileges
    // only cover schema `public`; grant explicitly for this custom schema.
    admin
        .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant schema usage to app role");
    admin
        .execute(
            format!("GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA {schema} TO myelin_app")
                .as_str(),
        )
        .await
        .expect("grant table DML to app role");
}

async fn activate_job_owner(admin: &PgPool, queued: &DurableEnqueue) {
    sqlx::query(
        "INSERT INTO workflow_run (tenant_id, region, run_id, state)
         VALUES ($1, $2, $3, 'running')",
    )
    .bind(&queued.tenant_id)
    .bind(&queued.region)
    .bind(&queued.run_id)
    .execute(admin)
    .await
    .expect("seed the active Flow owner");
    sqlx::query(
        "INSERT INTO ci_run (tenant_id, region, wf_run_id, state)
         VALUES ($1, $2, $3::uuid, 'running')",
    )
    .bind(&queued.tenant_id)
    .bind(&queued.region)
    .bind(&queued.run_id)
    .execute(admin)
    .await
    .expect("seed the active CI owner");
    sqlx::query(
        "INSERT INTO ci_job (tenant_id, region, job_id, state)
         VALUES ($1, $2, $3::uuid, 'queued')",
    )
    .bind(&queued.tenant_id)
    .bind(&queued.region)
    .bind(&queued.job_id)
    .execute(admin)
    .await
    .expect("seed the public CI job surface");
}

#[tokio::test]
async fn job_queue_store_claim_serialize_reaper_cancel_kill9_rls_on_live_postgres() {
    let schema = schema_name();
    // A cleanup-dedicated pool, independent of `admin`/`admin2` below (the kill-9 drill drops `admin`
    // mid-test) — `with_schema_cleanup` unconditionally drops `schema` through THIS pool regardless of
    // how the test body finishes (success, assertion failure, or panic).
    let cleanup_admin = reopen_admin().await;
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_admin, &schema_for_cleanup, move || async move {
    let region = "fr-par";
    let admin = reopen_admin().await;
    create_schema(&admin, &schema).await;
    let store = ci_job_queue_store(admin.clone());
    let region_store = ci_region_queue_store_test_support(admin.clone());

    // ── 1. Seed via the STORE's enqueue (tenant-scoped path): a spread of eligibility cases. ──
    // interactive (newer) + batch (older) in-region trusted; out-of-region; wrong-label; and the two
    // UNTRUSTED tiers a trusted-only claim must never lease.
    for j in [
        job(
            "tenantA",
            region,
            "jbatch",
            Lane::Batch,
            &["linux"],
            TrustTier::Trusted,
            None,
            "idem-b",
        ),
        job(
            "tenantA",
            region,
            "jint",
            Lane::Interactive,
            &["linux"],
            TrustTier::Trusted,
            None,
            "idem-i",
        ),
        job(
            "tenantA",
            "us-east",
            "joutreg",
            Lane::Interactive,
            &["linux"],
            TrustTier::Trusted,
            None,
            "idem-o",
        ),
        job(
            "tenantA",
            region,
            "jwinlabel",
            Lane::Interactive,
            &["windows"],
            TrustTier::Trusted,
            None,
            "idem-w",
        ),
        job(
            "tenantA",
            region,
            "jfork",
            Lane::Interactive,
            &["linux"],
            TrustTier::UntrustedFork,
            None,
            "idem-f",
        ),
        job(
            "tenantA",
            region,
            "jself",
            Lane::Interactive,
            &["linux"],
            TrustTier::SelfHosted,
            None,
            "idem-s",
        ),
    ] {
        assert_eq!(
            store.enqueue(&j).await.expect("enqueue"),
            EnqueueOutcome::Inserted,
            "each distinct idem is inserted"
        );
        activate_job_owner(&admin, &j).await;
    }
    // Idempotent enqueue: a duplicate idem is a no-op (jq_idem unique).
    let dup = job(
        "tenantA",
        region,
        "jint",
        Lane::Interactive,
        &["linux"],
        TrustTier::Trusted,
        None,
        "idem-i",
    );
    assert_eq!(
        store.enqueue(&dup).await.expect("dup enqueue"),
        EnqueueOutcome::DuplicateIdem,
        "a re-enqueue of the same (tenant, idem_token) is a no-op"
    );

    // ── 2. CLAIM: a trusted linux/gpu runner leases the in-region INTERACTIVE trusted job. ──
    let runner_labels = vec!["linux".to_string(), "gpu".to_string()];
    let trusted_only = vec![TrustTier::Trusted];
    let leased = region_store
        .claim(region, &runner_labels, &trusted_only, "r1", 30)
        .await
        .expect("claim")
        .expect("an eligible job is leased");
    assert_eq!(
        leased.job_id,
        uid("jint"),
        "the in-region INTERACTIVE trusted job is leased (residency + affinity + trust + lane) — the \
         out-of-region / wrong-label / untrusted jobs are NOT"
    );
    assert_eq!(leased.lane, Lane::Interactive);
    assert_eq!(
        leased.trust_tier,
        TrustTier::Trusted,
        "a trusted-only claim leases a trusted job"
    );

    // ── 3. SECURITY: a trusted-only claim NEVER leases the untrusted_fork / self_hosted jobs. ──
    // Claim repeatedly with only trusted tiers; assert the fork + self-hosted job ids never appear.
    let mut trusted_claims = Vec::new();
    for owner in ["t1", "t2", "t3", "t4", "t5"] {
        if let Some(l) = region_store
            .claim(region, &runner_labels, &trusted_only, owner, 30)
            .await
            .expect("claim")
        {
            trusted_claims.push(l.job_id);
        } else {
            break;
        }
    }
    assert!(
        !trusted_claims.contains(&uid("jfork")),
        "SECURITY: an untrusted_fork job is NEVER claimed by a trusted-only claim"
    );
    assert!(
        !trusted_claims.contains(&uid("jself")),
        "SECURITY: a self_hosted job is NEVER claimed by a claim that does not list that tier"
    );
    assert!(
        !trusted_claims.contains(&uid("jwinlabel")),
        "affinity: a windows-labelled job is never claimed by a linux/gpu runner"
    );
    // A claim that DOES list untrusted_fork leases the fork job (the tier gate is exact, not a blanket).
    let fork_claim = region_store
        .claim(
            region,
            &runner_labels,
            &[TrustTier::UntrustedFork],
            "fork-runner",
            30,
        )
        .await
        .expect("claim")
        .expect("the fork-allowed runner leases the fork job");
    assert_eq!(fork_claim.job_id, uid("jfork"));
    assert_eq!(fork_claim.trust_tier, TrustTier::UntrustedFork);

    // ── 4. SKIP LOCKED: two CONCURRENT claims take DIFFERENT rows (0 double-lease). ──
    // Seed two fresh eligible trusted jobs; fire two claims concurrently.
    for j in [
        job(
            "tenantA",
            region,
            "cc1",
            Lane::Batch,
            &["linux"],
            TrustTier::Trusted,
            None,
            "idem-cc1",
        ),
        job(
            "tenantA",
            region,
            "cc2",
            Lane::Batch,
            &["linux"],
            TrustTier::Trusted,
            None,
            "idem-cc2",
        ),
    ] {
        store.enqueue(&j).await.expect("enqueue cc");
        activate_job_owner(&admin, &j).await;
    }
    let s_a = region_store.clone();
    let s_b = region_store.clone();
    let labels_a = runner_labels.clone();
    let labels_b = runner_labels.clone();
    let (ra, rb) = tokio::join!(
        async move {
            s_a.claim(region, &labels_a, &[TrustTier::Trusted], "conc-a", 30)
                .await
        },
        async move {
            s_b.claim(region, &labels_b, &[TrustTier::Trusted], "conc-b", 30)
                .await
        },
    );
    let ja = ra.expect("claim a");
    let jb = rb.expect("claim b");
    if let (Some(a), Some(b)) = (&ja, &jb) {
        assert_ne!(
            a.job_id, b.job_id,
            "two CONCURRENT claims take DIFFERENT rows (FOR UPDATE SKIP LOCKED — 0 double-lease)"
        );
    }

    // ── 5. REAPER: force jint's lease into the past → reap re-queues it (0 orphans), re-claim works. ──
    admin
        .execute(
            format!(
                "UPDATE job_queue SET lease_expires = now() - interval '1 second' WHERE job_id = '{}'",
                uid("jint")
            )
            .as_str(),
        )
        .await
        .expect("expire jint's lease (the runner died)");
    let before_count: i64 = sqlx::query("SELECT count(*) AS n FROM job_queue")
        .fetch_one(&admin)
        .await
        .unwrap()
        .get("n");
    let reaped = region_store.reap(region).await.expect("reap");
    assert!(
        reaped >= 1,
        "the dead lease is re-queued by the reaper (0 orphans): {reaped} swept"
    );
    let state: String = sqlx::query("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("jint"))
        .fetch_one(&admin)
        .await
        .unwrap()
        .get("state");
    assert_eq!(
        state, "queued",
        "the reaped job is re-queued (claimable again)"
    );
    // 0 duplicate enqueues: a redundant re-dispatch (same idem) is a no-op; count unchanged.
    let retry = job(
        "tenantA",
        region,
        "jint",
        Lane::Interactive,
        &["linux"],
        TrustTier::Trusted,
        None,
        "idem-i",
    );
    assert_eq!(
        store.enqueue(&retry).await.expect("retry enqueue"),
        EnqueueOutcome::DuplicateIdem,
        "the re-dispatch is idempotent on idem_token (ONE row, never a duplicate)"
    );
    let after_count: i64 = sqlx::query("SELECT count(*) AS n FROM job_queue")
        .fetch_one(&admin)
        .await
        .unwrap()
        .get("n");
    assert_eq!(
        before_count, after_count,
        "0 duplicate enqueues after the reaper re-queue"
    );
    // A fresh runner re-claims the recovered job.
    let reclaim = region_store
        .claim(region, &runner_labels, &trusted_only, "live-runner", 30)
        .await
        .expect("re-claim")
        .expect("the recovered job re-claims");
    assert_eq!(
        reclaim.job_id,
        uid("jint"),
        "a live runner picks up the reaped job"
    );

    // A HEART-BEATING lease is NOT reaped: extend jint's lease, expire nothing, reap finds it live.
    assert!(
        store
            .heartbeat(
                "tenantA",
                region,
                &uid("jint").to_string(),
                "live-runner",
                60
            )
            .await
            .expect("heartbeat"),
        "the lease owner extends its lease"
    );
    let swept = region_store
        .reap(region)
        .await
        .expect("reap after heartbeat");
    let jint_state: String = sqlx::query("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("jint"))
        .fetch_one(&admin)
        .await
        .unwrap()
        .get("state");
    assert_eq!(
        jint_state, "leased",
        "a heart-beating lease is NOT reaped (stays leased)"
    );
    let _ = swept;

    // ── 6. CANCEL-SUPERSEDED: a new PR head terminalises the prior head. ──
    store
        .enqueue(&job(
            "tenantA",
            region,
            "h1",
            Lane::Interactive,
            &["linux"],
            TrustTier::Trusted,
            Some("pr:web:42"),
            "idem-h1",
        ))
        .await
        .expect("enqueue h1");
    store
        .enqueue(&job(
            "tenantA",
            region,
            "h2",
            Lane::Interactive,
            &["linux"],
            TrustTier::Trusted,
            Some("pr:web:42"),
            "idem-h2",
        ))
        .await
        .expect("enqueue h2");
    let cancelled = store
        .cancel_superseded("tenantA", region, "pr:web:42", &uid("h2").to_string())
        .await
        .expect("cancel_superseded");
    assert!(
        cancelled.contains(&uid("h1")),
        "the prior PR head h1 is cancelled"
    );
    let h1_state: String = sqlx::query("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("h1"))
        .fetch_one(&admin)
        .await
        .unwrap()
        .get("state");
    assert_eq!(h1_state, "terminal", "h1 is terminal (superseded)");
    let h2_state: String = sqlx::query("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("h2"))
        .fetch_one(&admin)
        .await
        .unwrap()
        .get("state");
    assert_eq!(h2_state, "queued", "the latest head h2 stays schedulable");

    // ── 6b. FINAL LAUNCH CAS vs CANCEL: exactly one row transition wins. ──
    let race_old = job(
        "tenantA",
        region,
        "launch-race-old",
        Lane::Interactive,
        &["launch-race"],
        TrustTier::Trusted,
        Some("pr:web:launch-race"),
        "idem-launch-race-old",
    );
    store
        .enqueue(&race_old)
        .await
        .expect("enqueue launch race old");
    activate_job_owner(&admin, &race_old).await;
    let race_lease = region_store
        .claim(
            region,
            &["launch-race".into()],
            &trusted_only,
            "launch-race-runner",
            60,
        )
        .await
        .expect("claim launch race old")
        .expect("launch race old is eligible");
    store
        .enqueue(&job(
            "tenantA",
            region,
            "launch-race-new",
            Lane::Interactive,
            &["launch-race"],
            TrustTier::Trusted,
            Some("pr:web:launch-race"),
            "idem-launch-race-new",
        ))
        .await
        .expect("enqueue launch race new");
    let exact = CiJobLaunchClaim {
        tenant_id: race_lease.tenant_id.clone(),
        region: region.into(),
        wf_run_id: race_lease.run_id.to_string(),
        job_id: race_lease.job_id.to_string(),
        lease_owner: race_lease.lease_owner.clone(),
        lease_epoch: race_lease.lease_epoch,
        claim_nonce: race_lease.claim_nonce.clone(),
        claim_started_at_epoch_secs: race_lease.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: race_lease.claim_expires_at_epoch_secs,
    };

    // A pooled PostgreSQL session lock is re-entrant. Deliberately return a dirty session to a
    // one-connection pool and prove the launch fence closes/refuses it before the durable CAS,
    // rather than accepting stale ownership and decrementing only one lock level at release.
    let dirty_schema = schema.clone();
    let dirty_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |conn, _meta| {
            let dirty_schema = dirty_schema.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {dirty_schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&admin_url())
        .await
        .expect("connect one-session stale-lock probe");
    {
        let mut dirty_connection = dirty_pool.acquire().await.unwrap();
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(9_223_372_036_i64)
            .execute(&mut *dirty_connection)
            .await
            .expect("seed a stale pooled advisory lock");
    }
    let dirty_store = ci_job_queue_store(dirty_pool.clone());
    let stale_lock_error = dirty_store
        .authorize_launch(&exact)
        .await
        .expect_err("a dirty pooled session must be refused");
    assert!(
        stale_lock_error
            .to_string()
            .contains("retained an advisory lock"),
        "the refusal must identify stale session ownership"
    );
    let state_after_dirty_session: String =
        sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1")
            .bind(uid("launch-race-old"))
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(
        state_after_dirty_session, "leased",
        "dirty session refusal must happen before the launch CAS"
    );
    let surface_after_dirty_session: String =
        sqlx::query_scalar("SELECT state FROM ci_job WHERE job_id = $1")
            .bind(uid("launch-race-old"))
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(
        surface_after_dirty_session, "queued",
        "dirty session refusal cannot publish a running job"
    );
    drop(dirty_store);
    dirty_pool.close().await;

    let launch_store = store.clone();
    let cancel_store = store.clone();
    let exact_for_launch = exact.clone();
    let (launch, cancel) = tokio::join!(
        async move { launch_store.authorize_launch(&exact_for_launch).await },
        async move {
            cancel_store
                .cancel_superseded(
                    "tenantA",
                    region,
                    "pr:web:launch-race",
                    &uid("launch-race-new").to_string(),
                )
                .await
        },
    );
    let launch_won = launch.expect("launch CAS completes");
    let cancelled = cancel.expect("cancel completes");
    let cancel_won = cancelled.contains(&uid("launch-race-old"));
    assert_ne!(
        launch_won, cancel_won,
        "launch CAS and cancel serialize: exactly one transition wins"
    );
    let race_state: String = sqlx::query("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("launch-race-old"))
        .fetch_one(&admin)
        .await
        .unwrap()
        .get("state");
    assert_eq!(race_state, if launch_won { "running" } else { "terminal" });
    let race_surface_state: String =
        sqlx::query_scalar("SELECT state FROM ci_job WHERE job_id = $1")
            .bind(uid("launch-race-old"))
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(
        race_surface_state,
        if launch_won { "running" } else { "queued" },
        "the public running state co-commits only with the winning launch CAS"
    );
    assert!(
        !store
            .authorize_launch(&exact)
            .await
            .expect("replayed launch CAS"),
        "the exact launch generation is one-shot"
    );

    // The launch boundary refuses to advance the durable queue when its public job projection is
    // missing. The writable CTE is one statement, so returning no projected row rolls the queue
    // transition back with the transaction.
    let missing_surface_job = job(
        "tenantA",
        region,
        "launch-no-surface",
        Lane::Interactive,
        &["launch-no-surface"],
        TrustTier::Trusted,
        None,
        "idem-launch-no-surface",
    );
    store
        .enqueue(&missing_surface_job)
        .await
        .expect("enqueue missing-surface launch");
    activate_job_owner(&admin, &missing_surface_job).await;
    sqlx::query("DELETE FROM ci_job WHERE job_id = $1")
        .bind(uid("launch-no-surface"))
        .execute(&admin)
        .await
        .expect("remove public job projection");
    let missing_surface_lease = region_store
        .claim(
            region,
            &["launch-no-surface".into()],
            &trusted_only,
            "launch-no-surface-runner",
            60,
        )
        .await
        .expect("claim missing-surface launch")
        .expect("missing-surface launch is eligible");
    assert!(
        !store
            .authorize_launch(&CiJobLaunchClaim {
                tenant_id: missing_surface_lease.tenant_id.clone(),
                region: region.into(),
                wf_run_id: missing_surface_lease.run_id.to_string(),
                job_id: missing_surface_lease.job_id.to_string(),
                lease_owner: missing_surface_lease.lease_owner.clone(),
                lease_epoch: missing_surface_lease.lease_epoch,
                claim_nonce: missing_surface_lease.claim_nonce.clone(),
                claim_started_at_epoch_secs: missing_surface_lease.claim_started_at_epoch_secs,
                claim_expires_at_epoch_secs: missing_surface_lease.claim_expires_at_epoch_secs,
            })
            .await
            .expect("missing-surface launch authorization"),
        "launch refuses when the public job projection is absent"
    );
    let missing_surface_queue_state: String =
        sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1")
            .bind(uid("launch-no-surface"))
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(
        missing_surface_queue_state, "leased",
        "a refused public-state transition rolls the durable launch CAS back"
    );

    // A runner that dies after the launch fence is still recoverable when its execution lease
    // expires; `running` is not an orphan state.
    let reap_job = job(
        "tenantA",
        region,
        "launch-reap",
        Lane::Interactive,
        &["launch-reap"],
        TrustTier::Trusted,
        None,
        "idem-launch-reap",
    );
    store.enqueue(&reap_job).await.expect("enqueue launch reap");
    activate_job_owner(&admin, &reap_job).await;
    let reap_lease = region_store
        .claim(
            region,
            &["launch-reap".into()],
            &trusted_only,
            "launch-reap-runner",
            60,
        )
        .await
        .expect("claim launch reap")
        .expect("launch reap is eligible");
    assert!(store
        .authorize_launch(&CiJobLaunchClaim {
            tenant_id: reap_lease.tenant_id.clone(),
            region: region.into(),
            wf_run_id: reap_lease.run_id.to_string(),
            job_id: reap_lease.job_id.to_string(),
            lease_owner: reap_lease.lease_owner.clone(),
            lease_epoch: reap_lease.lease_epoch,
            claim_nonce: reap_lease.claim_nonce.clone(),
            claim_started_at_epoch_secs: reap_lease.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: reap_lease.claim_expires_at_epoch_secs,
        })
        .await
        .expect("authorize launch reap"));
    let launched_surface_state: String =
        sqlx::query_scalar("SELECT state FROM ci_job WHERE job_id = $1")
            .bind(uid("launch-reap"))
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(
        launched_surface_state, "running",
        "a launched job is visible as running before sandbox output begins"
    );
    sqlx::query(
        "UPDATE job_queue SET lease_expires = now() - interval '1 second' WHERE job_id = $1",
    )
    .bind(uid("launch-reap"))
    .execute(&admin)
    .await
    .expect("expire running launch lease");
    region_store
        .reap(region)
        .await
        .expect("reap running launch");
    let launch_reap_state: String = sqlx::query("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("launch-reap"))
        .fetch_one(&admin)
        .await
        .unwrap()
        .get("state");
    assert_eq!(launch_reap_state, "queued");

    // ── 7. KILL-9 / reopen: a leased row survives; an UNCOMMITTED enqueue leaves NO ghost. ──
    // jint is leased. Start an enqueue in a raw tx and DROP it without commit (the crash mid-write).
    {
        let mut tx = admin.begin().await.unwrap();
        sqlx::query(INSERT_JOB_QUEUE_QUERY)
            .bind("tenantA")
            .bind(region)
            .bind(uid("ghost"))
            .bind(uid("run-ghost"))
            .bind("batch")
            .bind(vec!["linux".to_string()])
            .bind("trusted")
            .bind(Option::<String>::None)
            .bind("tenantA")
            .bind("idem-ghost")
            .bind("build")
            .bind(CI_RUNNER_EXECUTION_LEASE_TTL_SECS)
            .bind(Option::<i16>::None)
            .execute(&mut *tx)
            .await
            .expect("the in-tx enqueue writes (uncommitted)");
        drop(tx); // rollback — the crash before commit.
    }
    // KILL-9: drop the pool + store without a graceful close.
    drop(store);
    drop(admin);
    // REOPEN: a fresh pool reads the state back from Postgres.
    let admin2 = reopen_admin().await;
    let leased_after: String = sqlx::query("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("jint"))
        .fetch_one(&admin2)
        .await
        .unwrap()
        .get("state");
    assert_eq!(
        leased_after, "leased",
        "the leased row survives kill-9/reopen (durable, not in-memory)"
    );
    let ghost: Option<Uuid> = sqlx::query("SELECT job_id FROM job_queue WHERE job_id = $1")
        .bind(uid("ghost"))
        .fetch_optional(&admin2)
        .await
        .unwrap()
        .map(|r| r.get("job_id"));
    assert!(
        ghost.is_none(),
        "the uncommitted enqueue left NO ghost row (all-or-nothing)"
    );

    // ── 8. TENANT-SCOPED RLS under the APP role (NOBYPASSRLS): enqueue works; a wrong-tenant read sees 0. ──
    let app = app_pool().await;
    let app_store: CiJobQueueStore = ci_job_queue_store(app.clone());
    let rls_job = job(
        "tenantRLS",
        region,
        "rls1",
        Lane::Batch,
        &["linux"],
        TrustTier::Trusted,
        None,
        "idem-rls1",
    );
    assert_eq!(
        app_store.enqueue(&rls_job).await.expect("app-role enqueue under RLS"),
        EnqueueOutcome::Inserted,
        "the tenant-scoped enqueue INSERTs under the app role (RLS WITH CHECK passes with the tenant GUC)"
    );
    // Under a DIFFERENT tenant GUC the app role sees 0 rows (isolation); under the right tenant it sees 1.
    let count_under = |tenant: &'static str| {
        let app = app.clone();
        async move {
            let mut tx = app.begin().await.unwrap();
            sqlx::query("SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)")
                .bind(tenant)
                .bind(region)
                .execute(&mut *tx)
                .await
                .unwrap();
            let n: i64 = sqlx::query("SELECT count(*) AS n FROM job_queue WHERE job_id = $1")
                .bind(uid("rls1"))
                .fetch_one(&mut *tx)
                .await
                .unwrap()
                .get("n");
            tx.commit().await.unwrap();
            n
        }
    };
    assert_eq!(
        count_under("tenantOTHER").await,
        0,
        "RLS: a DIFFERENT tenant GUC sees 0 rows of tenantRLS's job (no cross-tenant read under the app role)"
    );
    assert_eq!(
        count_under("tenantRLS").await,
        1,
        "RLS: the OWNING tenant GUC sees its own row (isolation is scope, not a blanket deny)"
    );

    println!(
        "[CT-004c.1] PASS job_queue store: claim honours residency+affinity+trust+lane; trusted-only \
         claim NEVER leases untrusted_fork/self_hosted (security seam); concurrent claims take \
         different rows (SKIP LOCKED); reaper re-queues dead leases (0 orphans, 0 dup enqueue) + a \
         heart-beating lease is NOT reaped; cancel and the exact launch-generation CAS serialize \
         with one winner, CAS replay refuses, public running co-commits, a missing public projection \
         refuses launch without advancing the queue, and an expired running lease reaps; \
         cancel-superseded keeps the latest head; leased row survives kill-9/reopen with no ghost; \
         tenant-scoped enqueue + isolation proven under the NOBYPASSRLS app role"
    );
    })
    .await;
}
