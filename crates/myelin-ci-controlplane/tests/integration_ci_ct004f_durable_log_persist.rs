//! **CT-004f sub-step 4a — the DURABLE `LogPersist` store, PROVEN through the tenant-scoped write.**
//!
//! The CI-P20 sibling (`integration_ci_p20_log_pipeline`) proved the frozen `log_segment` /
//! `log_anchor` bind-param SQL applies + round-trips, but with RAW inline sqlx on a session-GUC
//! connection — there was no production STORE (the same "model-only, no store" gap CT-004a closed for
//! metering). This proves the real store ([`myelin_ci_controlplane::DurableLogPersist`]) end to end:
//!
//!   1. **The live log path writes the index THROUGH the sink → store, tenant-scoped.** A
//!      [`LogPipelineSink`] over the real `DurableLogPersist` ships frames + `finish`es a job; the
//!      sealed `log_segment` + closed `log_anchor` rows land in REAL Postgres via ONE
//!      `with_tenant_tx` transaction (FORCE-RLS), under the RLS-enforced **app** role (the GUC the
//!      tenant-scoped tx sets is what admits the write) — a read-back proves the `(job, step,
//!      byte-range)` index is present, the anchor closed `passed` with a bounded span, 0 dangling.
//!   2. **A re-delivered finish is idempotent (double-effect 0).** Persisting the SAME flushed index
//!      twice affects the rows via `ON CONFLICT … DO UPDATE` (the PK upsert) — the row COUNT is
//!      unchanged (no duplicate segment/anchor), the closed status stays `passed`.
//!
//! Gated behind the `integration` cargo feature. Run against the docker-compose dev stack:
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_ct004f_durable_log_persist -- --nocapture
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use myelin_ci_controlplane::{
    ci_controlplane_migrations, log_pipeline::AnchorStatus, log_pipeline::CoalesceBudget,
    log_pipeline::LogAnchorRow, log_pipeline::LogCoord, log_pipeline::LogPipeline,
    log_pipeline::SealThreshold, log_pipeline::SecretRedactor, DurableLogPersist, FlushedJobLogs,
    LogPersist, LogPipelineSink, CREATE_CI_JOB_SPEC_DDL, CREATE_JOB_QUEUE_DDL, SINGLE_STEP_ID,
};
use myelin_ci_sandbox::FirehoseSink;
use myelin_events::OUTBOX_MIGRATION;
use myelin_storage::FsBlobStore;
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, Row};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}
static SCHEMA_SEQ: AtomicU64 = AtomicU64::new(0);
fn schema_name() -> String {
    format!(
        "ci_ct004f_{}_{}",
        std::process::id(),
        SCHEMA_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// A pool whose connections pin `search_path` to the per-pid schema (so the store's UNQUALIFIED
/// `log_segment`/`log_anchor` resolve to the isolated tables; `public` follows for the RLS helper).
async fn pool(url: &str, schema: &str) -> sqlx::PgPool {
    let schema = schema.to_owned();
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
        .connect(url)
        .await
        .expect("connect to dev Postgres (is the stack up? eval \"$(scripts/dev-stack.sh env)\")")
}

/// A stable uuid string from a name (deterministic FNV-1a fill) — the durable id columns are `uuid`.
fn uid(name: &str) -> String {
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
    sqlx::types::Uuid::from_bytes(bytes).to_string()
}

struct SynchronizedResumePersist {
    inner: DurableLogPersist,
    both_resumed: Arc<Barrier>,
}

impl LogPersist for SynchronizedResumePersist {
    fn resume(
        &self,
        tenant: &TenantId,
        region: &Region,
        run_id: &str,
        job_id: &str,
    ) -> Result<myelin_ci_controlplane::log_sink::LogResume, Box<dyn std::error::Error + Send + Sync>>
    {
        let head = self.inner.resume(tenant, region, run_id, job_id)?;
        self.both_resumed.wait();
        Ok(head)
    }

    fn persist(
        &self,
        tenant: &TenantId,
        flushed: FlushedJobLogs,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.persist(tenant, flushed)
    }
}

/// Fresh per-pid schema + the REAL CI durable migrations (log_segment/log_anchor with FORCE-RLS) +
/// grants so the RLS-enforced app role can exercise the tenant-scoped write.
async fn setup_schema(admin: &sqlx::PgPool, schema: &str) {
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop any prior schema");
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create the per-pid schema");
    // Only the log index tables (ci_0007_log_segment / ci_0008_log_anchor) — the rest of the full CI
    // set is irrelevant here and `job_queue`'s CREATE INDEX CONCURRENTLY cannot run in a tx block.
    for m in ci_controlplane_migrations()
        .0
        .iter()
        .filter(|m| m.id.contains("log_segment") || m.id.contains("log_anchor"))
    {
        admin
            .execute(m.ddl)
            .await
            .unwrap_or_else(|e| panic!("apply CI durable migration {} into the schema: {e}", m.id));
    }
    // The durable outbox (sub-step 4b: the ci.log.available pointer co-commits here on the SAME tx as
    // the index rows). Relay-internal, NOT tenant-scoped (no RLS) — created before the grant below.
    admin
        .execute(OUTBOX_MIGRATION)
        .await
        .expect("apply the outbox migration into the schema");
    for (table, ddl) in [
        ("job_queue", CREATE_JOB_QUEUE_DDL),
        ("ci_job_spec", CREATE_CI_JOB_SPEC_DDL),
    ] {
        admin
            .execute(ddl)
            .await
            .unwrap_or_else(|error| panic!("create {table} log-route authority: {error}"));
        admin
            .execute(format!("SELECT myelin_make_tenant_scoped('{table}')").as_str())
            .await
            .unwrap_or_else(|error| panic!("scope {table} log-route authority: {error}"));
    }
    // Grant the RLS-enforced app role access to the schema + its tables (the real tenant-scoped write
    // runs as this role; FORCE-RLS + the tx GUC — not a grant — is what isolates it to the tenant).
    admin
        .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant schema usage to app");
    admin
        .execute(format!("GRANT ALL ON ALL TABLES IN SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant table access to app");
}

async fn seed_log_route(
    admin: &sqlx::PgPool,
    tenant: &TenantId,
    region: &Region,
    workflow_run: &str,
    ci_run: &str,
    job: &str,
) {
    sqlx::query(
        "INSERT INTO job_queue \
         (tenant_id,region,job_id,run_id,lane,labels,trust_tier,fair_key,idem_token,state) \
         VALUES ($1,$2,$3::uuid,$4::uuid,'batch','{}','trusted',$5,$6,'queued')",
    )
    .bind(tenant.as_str())
    .bind(region.as_str())
    .bind(job)
    .bind(workflow_run)
    .bind(format!("fair-{job}"))
    .bind(format!("idem-{job}"))
    .execute(admin)
    .await
    .expect("seed workflow queue log route");
    sqlx::query(
        "INSERT INTO ci_job_spec (tenant_id,region,job_id,run_id,idem_token,spec) \
         VALUES ($1,$2,$3::uuid,$4::uuid,$5,jsonb_build_object('ci_run_id',$6))",
    )
    .bind(tenant.as_str())
    .bind(region.as_str())
    .bind(job)
    .bind(workflow_run)
    .bind(format!("idem-{job}"))
    .bind(ci_run)
    .execute(admin)
    .await
    .expect("seed immutable CI log route");
}

async fn cleanup_schema(admin: sqlx::PgPool, app: sqlx::PgPool, schema: &str) {
    app.close().await;
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop isolated log-persist schema");
    admin.close().await;
}

/// Read back the `(segment_count, anchor_status, anchor_byte_end, dangling)` for a run/job, as the
/// tenant (RLS: set the GUC on the read connection so the app role can see its own rows).
async fn read_back(
    app: &sqlx::PgPool,
    tenant: &str,
    region: &str,
    run: &str,
    job: &str,
) -> (i64, String, Option<i64>, i64) {
    let mut conn = app.acquire().await.unwrap();
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, false), set_config('myelin.region', $2, false)",
    )
    .bind(tenant)
    .bind(region)
    .execute(&mut *conn)
    .await
    .unwrap();
    let seg_count: i64 = sqlx::query(
        "SELECT COUNT(*) AS c FROM log_segment WHERE run_id = $1::uuid AND job_id = $2::uuid",
    )
    .bind(run)
    .bind(job)
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("c");
    let anchor = sqlx::query(
        "SELECT status, byte_end FROM log_anchor WHERE run_id = $1::uuid AND job_id = $2::uuid AND step_id = $3",
    )
    .bind(run)
    .bind(job)
    .bind(SINGLE_STEP_ID)
    .fetch_one(&mut *conn)
    .await
    .expect("the closed anchor is present");
    let status: String = anchor.get("status");
    let byte_end: Option<i64> = anchor.get("byte_end");
    // Dangling: an anchor whose closed byte_end exceeds the max sealed segment byte_end.
    let dangling: i64 = sqlx::query(
        "SELECT COUNT(*) AS c FROM log_anchor a WHERE a.byte_end IS NOT NULL AND a.byte_end > \
         (SELECT COALESCE(MAX(byte_end), 0) FROM log_segment s WHERE s.run_id = a.run_id AND s.job_id = a.job_id)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("c");
    (seg_count, status, byte_end, dangling)
}

/// The count of `ci.log.available` outbox rows for a run/job aggregate (sub-step 4b). The outbox is
/// relay-internal (no RLS), so no tenant GUC is needed to read it.
async fn outbox_count(app: &sqlx::PgPool, run: &str, job: &str) -> i64 {
    let aggregate = format!("ci/run/{run}/job/{job}");
    sqlx::query(
        "SELECT COUNT(*) AS c FROM outbox WHERE aggregate = $1 AND envelope->>'type_' = 'ci.log.available'",
    )
    .bind(aggregate)
    .fetch_one(app)
    .await
    .unwrap()
    .get("c")
}

#[tokio::test(flavor = "multi_thread")]
async fn live_log_path_writes_the_index_through_the_tenant_scoped_store() {
    let schema = schema_name();
    let admin = pool(&admin_url(), &schema).await;
    setup_schema(&admin, &schema).await;
    let app = pool(&app_url(), &schema).await;

    let tenant = TenantId::from_token("acme-ct004f");
    let region = Region::new("fr-par");
    let run = uid("ct004f-run");
    let workflow_run = uid("ct004f-workflow-run");
    let job = uid("ct004f-job");
    seed_log_route(&admin, &tenant, &region, &workflow_run, &run, &job).await;

    // Drive the sink on a DEDICATED off-runtime thread — exactly how the CiRunnerLoop runs it. The
    // sink holds a `LogPipeline` (non-Send: the firehose uses Rc), so it is CONSTRUCTED inside the
    // thread (its inputs — pool, region, blobs, rt handle — are all Send) and never crosses a thread.
    // Off-runtime → `try_current()` is Err → the persist bridge runs `block_on` directly (production).
    let app_for_thread = app.clone();
    let rt = tokio::runtime::Handle::current();
    let (run_c, job_c, tenant_c, region_c) = (
        workflow_run.clone(),
        job.clone(),
        tenant.clone(),
        region.clone(),
    );
    let (live_tx, live_rx) = std::sync::mpsc::channel();
    let (finish_tx, finish_rx) = std::sync::mpsc::channel();
    let runner = std::thread::spawn(move || {
        let persist = DurableLogPersist::with_pg(app_for_thread, rt);
        let sink = LogPipelineSink::new(region_c, Arc::new(FsBlobStore::new()), persist);
        // Ship one frame, then deliberately hold the command open. The test reads PostgreSQL before
        // allowing finish, proving this is a during-execution durable checkpoint rather than merely
        // a different post-exit call order.
        sink.ship_frame(
            &run_c,
            &job_c,
            &tenant_c,
            b"compiling crate\nrunning tests\nall green\n",
        )
        .expect("incremental checkpoint persists before finish");
        live_tx.send(()).unwrap();
        finish_rx.recv().unwrap();
        sink.finish(&run_c, &job_c, &tenant_c, true)
            .expect("terminal anchor persists");
    });

    live_rx.recv().expect("live checkpoint committed");
    let (live_segments, live_status, live_byte_end, live_dangling) =
        read_back(&app, tenant.as_str(), region.as_str(), &run, &job).await;
    assert_eq!(live_segments, 1, "one segment is visible before finish");
    assert_eq!(live_status, "running");
    assert_eq!(
        live_byte_end, None,
        "the job has not reached terminal finish"
    );
    assert_eq!(live_dangling, 0);
    assert_eq!(
        outbox_count(&app, &run, &job).await,
        1,
        "the live pointer co-committed with the segment"
    );
    finish_tx.send(()).unwrap();
    runner.join().expect("the runner thread joins");

    let (seg_count, status, byte_end, dangling) =
        read_back(&app, tenant.as_str(), region.as_str(), &run, &job).await;
    assert!(
        seg_count >= 1,
        "at least one sealed segment landed (got {seg_count})"
    );
    assert_eq!(status, "passed", "the step anchor closed as passed");
    assert!(
        byte_end.is_some(),
        "the finished step's anchor is closed (byte_end set)"
    );
    assert_eq!(
        dangling, 0,
        "0 dangling anchors — every anchor's span is within the sealed bytes"
    );
    // Sub-step 4b: the ci.log.available pointer co-committed to the outbox on the SAME tx.
    assert!(
        outbox_count(&app, &run, &job).await >= 1,
        "a ci.log.available pointer landed in the outbox (co-committed with the index)"
    );
    cleanup_schema(admin, app, &schema).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn retried_sink_appends_after_the_committed_live_prefix() {
    let schema = schema_name();
    let admin = pool(&admin_url(), &schema).await;
    setup_schema(&admin, &schema).await;
    let app = pool(&app_url(), &schema).await;

    let tenant = TenantId::from_token("acme-ct004f-resume");
    let region = Region::new("fr-par");
    let run = uid("ct004f-resume-run");
    let workflow_run = uid("ct004f-resume-workflow-run");
    let job = uid("ct004f-resume-job");
    seed_log_route(&admin, &tenant, &region, &workflow_run, &run, &job).await;
    let app_for_thread = app.clone();
    let rt = tokio::runtime::Handle::current();
    let blobs = Arc::new(FsBlobStore::new());
    let (tenant_c, region_c, run_c, job_c) =
        (tenant.clone(), region.clone(), workflow_run, job.clone());

    std::thread::spawn(move || {
        let first = LogPipelineSink::new(
            region_c.clone(),
            blobs.clone(),
            DurableLogPersist::with_pg(app_for_thread.clone(), rt.clone()),
        );
        first
            .ship_frame(&run_c, &job_c, &tenant_c, b"attempt-one\n")
            .expect("first attempt commits its live prefix");
        drop(first); // injected runner loss before terminal finish

        let retry = LogPipelineSink::new(
            region_c,
            blobs,
            DurableLogPersist::with_pg(app_for_thread, rt),
        );
        retry
            .ship_frame(&run_c, &job_c, &tenant_c, b"attempt-two\n")
            .expect("retry recovers and appends after the durable head");
        retry
            .finish(&run_c, &job_c, &tenant_c, true)
            .expect("retry reaches terminal");
    })
    .join()
    .expect("retry runner thread joins");

    let mut conn = app.acquire().await.unwrap();
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, false), \
                set_config('myelin.region', $2, false)",
    )
    .bind(tenant.as_str())
    .bind(region.as_str())
    .execute(&mut *conn)
    .await
    .unwrap();
    let rows = sqlx::query(
        "SELECT segment_seq, byte_start, byte_end FROM log_segment \
         WHERE run_id = $1::uuid AND job_id = $2::uuid ORDER BY segment_seq",
    )
    .bind(&run)
    .bind(&job)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let coordinates: Vec<(i32, i64, i64)> = rows
        .iter()
        .map(|row| {
            (
                row.get("segment_seq"),
                row.get("byte_start"),
                row.get("byte_end"),
            )
        })
        .collect();
    assert_eq!(
        coordinates,
        vec![(0, 0, 12), (1, 12, 24)],
        "retry appends without overwriting or opening a byte/sequence gap"
    );
    drop(conn);
    let (_, status, byte_end, dangling) =
        read_back(&app, tenant.as_str(), region.as_str(), &run, &job).await;
    assert_eq!(status, "passed");
    assert_eq!(byte_end, Some(24));
    assert_eq!(dangling, 0);
    cleanup_schema(admin, app, &schema).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_retries_cannot_overwrite_the_same_committed_append_position() {
    let schema = schema_name();
    let admin = pool(&admin_url(), &schema).await;
    setup_schema(&admin, &schema).await;
    let app = pool(&app_url(), &schema).await;

    let tenant = TenantId::from_token("acme-ct004f-concurrent-resume");
    let region = Region::new("fr-par");
    let run = uid("ct004f-concurrent-resume-run");
    let workflow_run = uid("ct004f-concurrent-resume-workflow-run");
    let job = uid("ct004f-concurrent-resume-job");
    seed_log_route(&admin, &tenant, &region, &workflow_run, &run, &job).await;
    let rt = tokio::runtime::Handle::current();
    let blobs = Arc::new(FsBlobStore::new());
    let prefix = b"prefix\n";

    {
        let app = app.clone();
        let tenant = tenant.clone();
        let region = region.clone();
        let run = workflow_run.clone();
        let job = job.clone();
        let blobs = blobs.clone();
        let rt = rt.clone();
        std::thread::spawn(move || {
            let sink = LogPipelineSink::new(region, blobs, DurableLogPersist::with_pg(app, rt));
            sink.ship_frame(&run, &job, &tenant, prefix)
                .expect("the common prefix commits");
        })
        .join()
        .expect("prefix writer joins");
    }

    let both_resumed = Arc::new(Barrier::new(2));
    let attempts = [
        ("alpha", b"alpha\n".as_slice()),
        ("beta-long", b"beta-long\n".as_slice()),
    ];
    let handles: Vec<_> = attempts
        .into_iter()
        .map(|(name, bytes)| {
            let app = app.clone();
            let tenant = tenant.clone();
            let region = region.clone();
            let run = workflow_run.clone();
            let job = job.clone();
            let blobs = blobs.clone();
            let rt = rt.clone();
            let both_resumed = both_resumed.clone();
            std::thread::spawn(move || {
                let sink = LogPipelineSink::new(
                    region,
                    blobs,
                    SynchronizedResumePersist {
                        inner: DurableLogPersist::with_pg(app, rt),
                        both_resumed,
                    },
                );
                (
                    name,
                    bytes.len() as i64,
                    sink.ship_frame(&run, &job, &tenant, bytes),
                )
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("concurrent writer joins"))
        .collect();
    let winners: Vec<_> = results
        .iter()
        .filter(|(_, _, result)| result.is_ok())
        .collect();
    let losers: Vec<_> = results
        .iter()
        .filter(|(_, _, result)| result.is_err())
        .collect();
    assert_eq!(winners.len(), 1, "exactly one stale candidate may append");
    assert_eq!(losers.len(), 1, "the competing stale candidate is refused");
    assert!(
        losers[0]
            .2
            .as_ref()
            .unwrap_err()
            .contains("immutable committed segment"),
        "the refusal names immutable-prefix divergence: {:?}",
        losers[0].2
    );

    let mut conn = app.acquire().await.unwrap();
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, false), \
                set_config('myelin.region', $2, false)",
    )
    .bind(tenant.as_str())
    .bind(region.as_str())
    .execute(&mut *conn)
    .await
    .unwrap();
    let rows = sqlx::query(
        "SELECT segment_seq, byte_start, byte_end FROM log_segment \
         WHERE run_id = $1::uuid AND job_id = $2::uuid ORDER BY segment_seq",
    )
    .bind(&run)
    .bind(&job)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "the losing retry created no third segment");
    assert_eq!(rows[0].get::<i32, _>("segment_seq"), 0);
    assert_eq!(rows[0].get::<i64, _>("byte_start"), 0);
    assert_eq!(rows[0].get::<i64, _>("byte_end"), prefix.len() as i64);
    assert_eq!(rows[1].get::<i32, _>("segment_seq"), 1);
    assert_eq!(rows[1].get::<i64, _>("byte_start"), prefix.len() as i64);
    assert_eq!(
        rows[1].get::<i64, _>("byte_end"),
        prefix.len() as i64 + winners[0].1,
        "the committed winner remains authoritative; the later loser cannot overwrite it"
    );
    drop(conn);

    let app_for_finish = app.clone();
    let tenant_for_finish = tenant.clone();
    let region_for_finish = region.clone();
    let run_for_finish = workflow_run;
    let job_for_finish = job.clone();
    std::thread::spawn(move || {
        let sink = LogPipelineSink::new(
            region_for_finish,
            blobs,
            DurableLogPersist::with_pg(app_for_finish, rt),
        );
        sink.finish(&run_for_finish, &job_for_finish, &tenant_for_finish, true)
            .expect("the serialized winner can close the terminal anchor");
    })
    .join()
    .expect("terminal writer joins");

    let stale = FlushedJobLogs {
        run_id: run.clone(),
        job_id: job.clone(),
        segments: vec![],
        anchors: vec![LogAnchorRow {
            tenant_id: tenant.as_str().to_string(),
            region: region.as_str().to_string(),
            run_id: run.clone(),
            job_id: job.clone(),
            step_id: SINGLE_STEP_ID.into(),
            byte_start: 0,
            byte_end: None,
            status: AnchorStatus::Running,
        }],
        pointers: vec![],
    };
    let app_for_stale = app.clone();
    let tenant_for_stale = tenant.clone();
    let rt_for_stale = tokio::runtime::Handle::current();
    let stale_error = std::thread::spawn(move || {
        DurableLogPersist::with_pg(app_for_stale, rt_for_stale)
            .persist(&tenant_for_stale, stale)
            .expect_err("a stale running checkpoint cannot regress a terminal anchor")
            .to_string()
    })
    .join()
    .expect("stale writer joins");
    assert!(
        stale_error.contains("immutable terminal checkpoint"),
        "{stale_error}"
    );
    let (_, status, _, _) = read_back(&app, tenant.as_str(), region.as_str(), &run, &job).await;
    assert_eq!(status, "passed", "terminal anchor state is monotone");

    cleanup_schema(admin, app, &schema).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn re_delivered_persist_is_idempotent_no_duplicate_rows() {
    let schema = schema_name();
    let admin = pool(&admin_url(), &schema).await;
    setup_schema(&admin, &schema).await;
    let app = pool(&app_url(), &schema).await;

    let tenant = TenantId::from_token("acme-ct004f-idem");
    let region = Region::new("fr-par");
    let run = uid("ct004f-idem-run");
    let job = uid("ct004f-idem-job");

    // Build a deterministic flushed index from a real pipeline (seal small so multiple segments form).
    let flushed = {
        let mut p = LogPipeline::new(
            tenant.clone(),
            region.clone(),
            FsBlobStore::new(),
            SecretRedactor::default(),
        )
        .with_thresholds(
            CoalesceBudget::default(),
            SealThreshold { seal_at_bytes: 8 },
        );
        let coord = LogCoord::new(&run, &job, SINGLE_STEP_ID);
        for _ in 0..5 {
            p.ship_line(&coord, "0123456789").expect("ship");
        }
        p.close_step(&coord, AnchorStatus::Passed).expect("close");
        p.flush_job(&run, &job, SINGLE_STEP_ID).expect("flush");
        FlushedJobLogs {
            run_id: run.clone(),
            job_id: job.clone(),
            segments: p.segment_rows().to_vec(),
            anchors: p.anchor_rows().into_iter().cloned().collect(),
            pointers: p.drain_pointers(),
        }
    };
    assert!(
        flushed.segments.len() >= 2,
        "multiple segments to exercise the PK"
    );

    let app_for_thread = app.clone();
    let rt = tokio::runtime::Handle::current();
    let (t1, f1) = (tenant.clone(), flushed.clone());
    let (t2, f2) = (tenant.clone(), flushed.clone());
    // Persist TWICE (the re-delivered terminal report) on a dedicated off-runtime thread (block_on
    // direct — the production bridge path).
    std::thread::spawn(move || {
        let persist = DurableLogPersist::with_pg(app_for_thread, rt);
        persist.persist(&t1, f1).expect("first persist");
        persist
            .persist(&t2, f2)
            .expect("re-delivered persist (idempotent)");
    })
    .join()
    .expect("the persist thread joins");

    let (seg_count, status, _byte_end, dangling) =
        read_back(&app, tenant.as_str(), region.as_str(), &run, &job).await;
    assert_eq!(
        seg_count as usize,
        flushed.segments.len(),
        "re-delivery did NOT duplicate segment rows (ON CONFLICT upsert)"
    );
    assert_eq!(status, "passed");
    assert_eq!(dangling, 0);
    // Sub-step 4b idempotency: the re-delivered persist did NOT duplicate the outbox pointers — the
    // deterministic event_id + ON CONFLICT (event_id) DO NOTHING dedups (double-emit 0).
    assert_eq!(
        outbox_count(&app, &run, &job).await as usize,
        flushed.pointers.len(),
        "re-delivery did NOT duplicate ci.log.available outbox rows"
    );
    assert!(
        !flushed.pointers.is_empty(),
        "the flush produced at least one pointer"
    );
    cleanup_schema(admin, app, &schema).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn byte_budget_coalesce_pointer_without_a_seal_persists() {
    // The mid-stream COALESCE pointer path (segment_ref = None): a long job crosses the byte budget
    // before a segment seals, so the drained set at finish carries a pointer that names a byte range
    // with NO sealed-segment ref. This path is reachable via the real sink (drain_pointers returns
    // ALL buffered pointers at finish) but was previously unexercised end-to-end (verifier gap).
    let schema = schema_name();
    let admin = pool(&admin_url(), &schema).await;
    setup_schema(&admin, &schema).await;
    let app = pool(&app_url(), &schema).await;

    let tenant = TenantId::from_token("acme-ct004f-coalesce");
    let region = Region::new("fr-par");
    let run = uid("ct004f-coalesce-run");
    let job = uid("ct004f-coalesce-job");

    // Small COALESCE budget + a LARGE seal threshold → a pointer emits from coalescing (segment_ref
    // None) with no seal.
    let flushed = {
        let mut p = LogPipeline::new(
            tenant.clone(),
            region.clone(),
            FsBlobStore::new(),
            SecretRedactor::default(),
        )
        .with_thresholds(
            CoalesceBudget {
                bytes_per_pointer: 10,
            },
            SealThreshold {
                seal_at_bytes: 1_000_000,
            },
        );
        let coord = LogCoord::new(&run, &job, SINGLE_STEP_ID);
        for _ in 0..4 {
            p.ship_line(&coord, "0123456789").expect("ship"); // 10 bytes each → crosses the 10-byte budget
        }
        p.close_step(&coord, AnchorStatus::Passed).expect("close");
        // Do NOT flush_job here — we want the pre-seal coalesce pointer, not a seal pointer.
        let pointers = p.drain_pointers();
        assert!(
            pointers.iter().any(|pt| pt.segment_ref.is_none()),
            "a coalesce pointer with NO sealed-segment ref was produced"
        );
        FlushedJobLogs {
            run_id: run.clone(),
            job_id: job.clone(),
            segments: p.segment_rows().to_vec(),
            anchors: p.anchor_rows().into_iter().cloned().collect(),
            pointers,
        }
    };
    let pointer_count = flushed.pointers.len();

    let app_for_thread = app.clone();
    let rt = tokio::runtime::Handle::current();
    let (t1, f1) = (tenant.clone(), flushed);
    std::thread::spawn(move || {
        let persist = DurableLogPersist::with_pg(app_for_thread, rt);
        persist
            .persist(&t1, f1)
            .expect("persist the coalesce-only index");
    })
    .join()
    .expect("the persist thread joins");

    // The segment_ref=None pointer co-committed to the outbox (the anchor closed even with no seal).
    assert_eq!(
        outbox_count(&app, &run, &job).await as usize,
        pointer_count,
        "the coalesce pointer(s) landed in the outbox"
    );
    let (_seg, status, byte_end, _dangling) =
        read_back(&app, tenant.as_str(), region.as_str(), &run, &job).await;
    assert_eq!(
        status, "passed",
        "the anchor closed even with no sealed segment"
    );
    assert!(byte_end.is_some());
    cleanup_schema(admin, app, &schema).await;
}
