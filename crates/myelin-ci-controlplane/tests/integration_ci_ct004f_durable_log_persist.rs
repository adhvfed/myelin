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

use std::sync::Arc;

use myelin_ci_controlplane::{
    ci_controlplane_migrations, log_pipeline::AnchorStatus, log_pipeline::CoalesceBudget,
    log_pipeline::LogCoord, log_pipeline::LogPipeline, log_pipeline::SealThreshold,
    log_pipeline::SecretRedactor, DurableLogPersist, FlushedJobLogs, LogPipelineSink, LogPersist,
    SINGLE_STEP_ID,
};
use myelin_ci_sandbox::FirehoseSink;
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
fn schema_name() -> String {
    format!("ci_ct004f_{}", std::process::id())
}

/// A pool whose connections pin `search_path` to the per-pid schema (so the store's UNQUALIFIED
/// `log_segment`/`log_anchor` resolve to the isolated tables; `public` follows for the RLS helper).
async fn pool(url: &str) -> sqlx::PgPool {
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

/// Fresh per-pid schema + the REAL CI durable migrations (log_segment/log_anchor with FORCE-RLS) +
/// grants so the RLS-enforced app role can exercise the tenant-scoped write.
async fn setup_schema(admin: &sqlx::PgPool) {
    let schema = schema_name();
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
    // Grant the RLS-enforced app role access to the schema + its tables (the real tenant-scoped write
    // runs as this role; FORCE-RLS + the tx GUC — not a grant — is what isolates it to the tenant).
    admin
        .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant schema usage to app");
    admin
        .execute(
            format!("GRANT ALL ON ALL TABLES IN SCHEMA {schema} TO myelin_app").as_str(),
        )
        .await
        .expect("grant table access to app");
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
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, false), set_config('myelin.region', $2, false)")
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

#[tokio::test(flavor = "multi_thread")]
async fn live_log_path_writes_the_index_through_the_tenant_scoped_store() {
    let admin = pool(&admin_url()).await;
    setup_schema(&admin).await;
    let app = pool(&app_url()).await;

    let tenant = TenantId::from_token("acme-ct004f");
    let region = Region::new("fr-par");
    let run = uid("ct004f-run");
    let job = uid("ct004f-job");

    // Drive the sink on a DEDICATED off-runtime thread — exactly how the CiRunnerLoop runs it. The
    // sink holds a `LogPipeline` (non-Send: the firehose uses Rc), so it is CONSTRUCTED inside the
    // thread (its inputs — pool, region, blobs, rt handle — are all Send) and never crosses a thread.
    // Off-runtime → `try_current()` is Err → the persist bridge runs `block_on` directly (production).
    let app_for_thread = app.clone();
    let rt = tokio::runtime::Handle::current();
    let (run_c, job_c, tenant_c, region_c) =
        (run.clone(), job.clone(), tenant.clone(), region.clone());
    std::thread::spawn(move || {
        let persist = DurableLogPersist::with_pg(app_for_thread, rt);
        let sink = LogPipelineSink::new(region_c, Arc::new(FsBlobStore::new()), persist);
        // Ship the job's output + finish (the runner's live path). `finish` seals + closes the anchor
        // `passed` and persists the index through `with_tenant_tx`.
        sink.ship_frame(&run_c, &job_c, &tenant_c, b"compiling crate\nrunning tests\nall green\n");
        sink.finish(&run_c, &job_c, &tenant_c, true);
    })
    .join()
    .expect("the runner thread joins");

    let (seg_count, status, byte_end, dangling) =
        read_back(&app, tenant.as_str(), region.as_str(), &run, &job).await;
    assert!(seg_count >= 1, "at least one sealed segment landed (got {seg_count})");
    assert_eq!(status, "passed", "the step anchor closed as passed");
    assert!(byte_end.is_some(), "the finished step's anchor is closed (byte_end set)");
    assert_eq!(dangling, 0, "0 dangling anchors — every anchor's span is within the sealed bytes");
}

#[tokio::test(flavor = "multi_thread")]
async fn re_delivered_persist_is_idempotent_no_duplicate_rows() {
    let admin = pool(&admin_url()).await;
    setup_schema(&admin).await;
    let app = pool(&app_url()).await;

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
        .with_thresholds(CoalesceBudget::default(), SealThreshold { seal_at_bytes: 8 });
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
    assert!(flushed.segments.len() >= 2, "multiple segments to exercise the PK");

    let app_for_thread = app.clone();
    let rt = tokio::runtime::Handle::current();
    let (t1, f1) = (tenant.clone(), flushed.clone());
    let (t2, f2) = (tenant.clone(), flushed.clone());
    // Persist TWICE (the re-delivered terminal report) on a dedicated off-runtime thread (block_on
    // direct — the production bridge path).
    std::thread::spawn(move || {
        let persist = DurableLogPersist::with_pg(app_for_thread, rt);
        persist.persist(&t1, f1).expect("first persist");
        persist.persist(&t2, f2).expect("re-delivered persist (idempotent)");
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
}
