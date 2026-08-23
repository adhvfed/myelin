#![cfg(feature = "integration")]

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};

use common::with_schema_cleanup;
use myelin_ci_controlplane::CI_RUNNER_EXECUTION_LEASE_TTL_SECS;
use myelin_ci_controlplane::{
    ci_job_queue_store, ci_region_queue_store_test_support, DurableEnqueue, DurableLeaseAdapter,
    DurableLogPersist, EnqueueOutcome, JobSpecResolver, Lane, LeasedJob, LogCoord, LogPipelineSink,
    ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL, ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
    ALTER_JOB_QUEUE_ADD_RESERVATION_WRITE_VERSION_DDL, ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL,
    CREATE_CI_JOB_SPEC_DDL, CREATE_FAIR_DEFICIT_DDL, CREATE_JOB_QUEUE_DDL,
    CREATE_JOB_QUEUE_INDEXES_DDL, CREATE_LOG_ANCHOR_DDL, CREATE_LOG_SEGMENT_DDL, SINGLE_STEP_NO,
};
use myelin_ci_sandbox::asset_registry::GvisorAssetRegistry;
use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    resolved_gvisor_rootfs, EgressPolicy, FirehoseSink, IdemToken, ImageRef, JobKind, JobSpec,
    LeaseStore, MeterTarget, ResourceLimits, RunTokenCredential, RunnerAgent, RunnerError,
    RunnerHooks, TrustTier, WorkspaceSpec, LINUX_SMALL_V1_ROOTFS_SHA256,
};
use myelin_config::MyelinConfig;
use myelin_events::OUTBOX_MIGRATION;
use myelin_events::{IdMinter, Ulid};
use myelin_flow::{
    job_idem_token, DurableExecutor, FlowExecutor, SignalOutcome, StartSpec, JOB_DONE_SIGNAL,
};
use myelin_storage::s3blob::S3BlobStore;
use myelin_storage::{BlobStore, ContentHash};
use myelin_tenancy::{Region, TenantId};
use sqlx::types::Uuid;
use sqlx::{Executor, PgPool};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}
fn schema_name(tag: &str) -> String {
    format!("ci_ct004c2_{}_{}", std::process::id(), tag)
}

async fn admin_pool(tag: &str) -> PgPool {
    let schema = schema_name(tag);
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
        .expect("connect to dev Postgres as admin (is the stack up? `fed test:backend`)")
}

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
        admin.execute(idx.as_str()).await.expect("index");
    }
    admin
        .execute(CREATE_FAIR_DEFICIT_DDL)
        .await
        .expect("create fair_deficit");
    admin
        .execute(
            "CREATE TABLE workflow_run (
               tenant_id text NOT NULL,
               region text NOT NULL,
               run_id text NOT NULL,
               state text NOT NULL,
               PRIMARY KEY (tenant_id, run_id)
             );
             CREATE TABLE ci_run (
               tenant_id text NOT NULL,
               region text NOT NULL,
               wf_run_id uuid NOT NULL,
               state text NOT NULL,
               PRIMARY KEY (tenant_id, wf_run_id)
             )",
        )
        .await
        .expect("create authoritative lifecycle tables used by claim/reap");
}

async fn activate_job_owner(admin: &PgPool, job: &DurableEnqueue) {
    sqlx::query(
        "INSERT INTO workflow_run (tenant_id, region, run_id, state)
         VALUES ($1, $2, $3, 'running')",
    )
    .bind(&job.tenant_id)
    .bind(&job.region)
    .bind(&job.run_id)
    .execute(admin)
    .await
    .expect("insert active Flow owner");
    sqlx::query(
        "INSERT INTO ci_run (tenant_id, region, wf_run_id, state)
         VALUES ($1, $2, $3::uuid, 'running')",
    )
    .bind(&job.tenant_id)
    .bind(&job.region)
    .bind(&job.run_id)
    .execute(admin)
    .await
    .expect("insert active CI owner");
}

async fn create_log_tables(admin: &PgPool) {
    for (base, ddl) in [
        ("log_segment", CREATE_LOG_SEGMENT_DDL),
        ("log_anchor", CREATE_LOG_ANCHOR_DDL),
    ] {
        admin
            .execute(ddl)
            .await
            .unwrap_or_else(|e| panic!("create {base}: {e}"));
        admin
            .execute(format!("SELECT myelin_make_tenant_scoped('{base}')").as_str())
            .await
            .expect("RLS-scope the log index table");
    }
    admin
        .execute(CREATE_CI_JOB_SPEC_DDL)
        .await
        .expect("create ci_job_spec (the durable log-resume route table)");
    admin
        .execute(OUTBOX_MIGRATION)
        .await
        .expect("create the durable outbox");
}

async fn seed_log_route(admin: &PgPool, tenant: &str, region: &str, job_id: Uuid, run_id: Uuid) {
    sqlx::query(
        "INSERT INTO ci_job_spec (tenant_id, region, job_id, run_id, idem_token, spec) \
         VALUES ($1, $2, $3, $4, $5, jsonb_build_object('ci_run_id', $6))",
    )
    .bind(tenant)
    .bind(region)
    .bind(job_id)
    .bind(run_id)
    .bind(format!("idem-log-route-{job_id}"))
    .bind(run_id.to_string())
    .execute(admin)
    .await
    .expect("seed the ci_job_spec log route (resume_async's job_queue join target)");
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

#[allow(clippy::too_many_arguments)]
fn enq(
    tenant: &str,
    region: &str,
    id: &str,
    run: &str,
    trust: TrustTier,
    labels: &[&str],
    idem: &str,
) -> DurableEnqueue {
    DurableEnqueue {
        tenant_id: tenant.into(),
        region: region.into(),
        job_id: uid(id).to_string(),
        run_id: uid(run).to_string(),
        lane: Lane::Batch,
        labels: labels.iter().map(|s| s.to_string()).collect(),
        trust_tier: trust,
        concurrency_group: None,
        fair_key: tenant.into(),
        idem_token: idem.into(),
        stage: "build".into(),
        claim_window_secs: CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
        reservation_write_version: myelin_ci_controlplane::ReservationWriteVersionMarker::legacy(),
    }
}

fn runsc_present() -> bool {
    let bin = std::env::var("MYELIN_RUNSC_BIN").unwrap_or_else(|_| "runsc".to_string());
    let on_path = if bin.contains('/') {
        Path::new(&bin).exists()
    } else {
        std::env::var("PATH")
            .ok()
            .map(|p| p.split(':').any(|d| Path::new(d).join(&bin).exists()))
            .unwrap_or(false)
    };
    on_path && resolved_gvisor_rootfs().exists()
}

fn require_or_skip(test: &str) -> bool {
    if runsc_present() {
        return true;
    }
    if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") {
        panic!(
            "[{test}] MYELIN_REQUIRE_RUNSC=1 but `runsc` is not on PATH or the staged rootfs ({}) is \
             absent - CT-004c.2 refuses a VACUOUS green: a real `runsc` guest MUST run the leased job.",
            resolved_gvisor_rootfs().display()
        );
    }
    eprintln!("[{test}] SKIPPED: `runsc`/rootfs absent - this host cannot run a gVisor guest.");
    false
}

#[derive(Clone, Default)]
struct CapturingFirehose {
    bytes: Arc<Mutex<Vec<u8>>>,
}
impl FirehoseSink for CapturingFirehose {
    fn ship_frame(
        &self,
        _run_id: &str,
        _job_id: &str,
        _tenant: &TenantId,
        frame: &[u8],
    ) -> Result<(), String> {
        self.bytes.lock().unwrap().extend_from_slice(frame);
        Ok(())
    }
    fn finish(
        &self,
        _run_id: &str,
        _job_id: &str,
        _tenant: &TenantId,
        _passed: bool,
    ) -> Result<(), String> {
        Ok(())
    }
}
impl CapturingFirehose {
    fn captured(&self) -> Vec<u8> {
        self.bytes.lock().unwrap().clone()
    }
}

struct FixedMinter(String);
impl IdMinter for FixedMinter {
    fn mint(&self) -> Ulid {
        Ulid(self.0.clone())
    }
}

fn ok_hooks() -> RunnerHooks {
    common::stage_b_compute_hooks_for_legacy_runsc_test()
}

fn linux_small_v1_image() -> ImageRef {
    ImageRef::pinned(format!(
        "myelin.local/linux-small-v1-rootfs@sha256:{LINUX_SMALL_V1_ROOTFS_SHA256}"
    ))
    .unwrap()
}

fn test_registry() -> Arc<GvisorAssetRegistry> {
    Arc::new(
        GvisorAssetRegistry::from_bindings(vec![
            myelin_ci_sandbox::asset_registry::RootfsAssetBinding {
                image: linux_small_v1_image(),
                rootfs: resolved_gvisor_rootfs(),
            },
        ])
        .expect("the base linux-small-v1 rootfs binding verifies"),
    )
}

fn compute_spec(command: Vec<String>, idem: &str) -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        linux_small_v1_image(),
        command,
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 * 1024 * 1024,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs: 60,
        },
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        RunTokenCredential::new("test-bearer", "ct004c2-jti", 300).unwrap(),
        MeterTarget {
            reserve_id: "ct004c2-reserve".into(),
        },
        IdemToken(idem.into()),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn durable_backed_runner_executes_real_runsc_end_to_end() {
    if !require_or_skip("ct004c2-e2e") {
        return;
    }
    let schema = schema_name("e2e");
    let admin = admin_pool("e2e").await;
    let cleanup_admin = admin.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_admin, &schema_for_cleanup, move || async move {
    let region = "fr-par";
    let tenant = "tenantA";
    create_schema(&admin, &schema).await;
    let store = ci_job_queue_store(admin.clone());
    let region_store = ci_region_queue_store_test_support(admin.clone());

    let run_uuid = uid("e2e-run").to_string();
    let idem = job_idem_token(&run_uuid, "ci.pipeline:0");

    let job = enq(
        tenant,
        region,
        "e2e-job",
        "e2e-run",
        TrustTier::Trusted,
        &["linux"],
        &idem,
    );
    assert_eq!(
        store.enqueue(&job).await.expect("enqueue"),
        EnqueueOutcome::Inserted
    );
    activate_job_owner(&admin, &job).await;

    let executor = FlowExecutor::new(
        Arc::new(FixedMinter(run_uuid.clone())),
        TenantId(tenant.into()),
        Region(region.into()),
    );
    executor.register_definition("ci.pipeline");
    let started = executor
        .start(StartSpec {
            wf_type: "ci.pipeline".into(),
            input: vec![],
            budget: None,
            idem_key: "ci:e2e-run".into(),
        })
        .expect("start the run");
    assert_eq!(
        started.0, run_uuid,
        "the started run id == the durable run_id"
    );

    let idem_for_resolver = idem.clone();
    let resolve: JobSpecResolver = Arc::new(move |_l: &LeasedJob| {
        Ok(compute_spec(
            vec![
                "sh".into(),
                "-c".into(),
                "echo hello-ct004c2; exit 0".into(),
            ],
            &idem_for_resolver,
        ))
    });
    let adapter = DurableLeaseAdapter::new(
        region_store,
        store.clone(),
        region,
        tokio::runtime::Handle::current(),
        resolve,
    );

    let backend = GvisorBackend::new(test_registry());
    let firehose = CapturingFirehose::default();
    let reporter = myelin_ci_sandbox::EngineTerminalReporter::new(executor.clone());
    let agent = RunnerAgent::new(
        "e2e-worker",
        vec!["linux".into()],
        vec![TrustTier::Trusted],
        Region(region.into()),
        30,
        adapter,
        &backend,
        &firehose,
        &reporter,
        ok_hooks(),
    );

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let outcome = agent
        .run_one(now)
        .expect("the durable-backed runner claims + runs the job in gVisor + reports terminal");

    println!("=== CT-004c.2 REAL end-to-end (durable claim → runsc → job.done) ===");
    let captured = firehose.captured();
    let guest = String::from_utf8_lossy(&captured);
    println!(
        "job_id={} run_id={} passed={}",
        outcome.job_id, outcome.run_id, outcome.report.passed
    );
    println!("REAL guest stdout (via firehose) = {guest:?}");
    println!("job.done delivery = {:?}", outcome.signal_outcome);

    assert_eq!(outcome.job_id, uid("e2e-job").to_string());
    assert_eq!(outcome.run_id, run_uuid);
    assert!(
        outcome.report.passed,
        "the real `runsc` guest exited 0 (clean) - derived passed=true"
    );
    assert!(
        guest.contains("hello-ct004c2"),
        "the REAL runsc guest stdout must contain the command's output (proves real exec). got: {guest:?}"
    );
    assert_eq!(
        outcome.signal_outcome,
        SignalOutcome::Buffered,
        "the FIRST job.done delivery wakes the parked workflow"
    );
    assert_eq!(
        executor
            .signals()
            .count_for_run(&TenantId(tenant.into()), &run_uuid),
        1,
        "the engine buffered EXACTLY ONE job.done"
    );

    let row = executor
        .signals()
        .get(&TenantId(tenant.into()), &run_uuid, JOB_DONE_SIGNAL, &idem)
        .expect("the job.done buffered under the echoed idem_token");
    assert_eq!(
        row.payload[0],
        myelin_refs::ArtifactRef("myelin://job-done/passed-true".into())
    );
    for r in &row.payload {
        assert!(
            !r.0.contains("hello-ct004c2"),
            "SECURITY(d): captured guest stdout MUST NEVER enter the job.done signal payload - it \
             rides the firehose (references-not-payloads). offending ref: {r:?}"
        );
    }
    assert_eq!(row.payload_key_ref, None, "no inline PII payload");

    let state: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("e2e-job"))
        .fetch_one(&admin)
        .await
        .expect("read job state");
    assert_eq!(
        state, "terminal",
        "the lease is completed on terminal (settle)"
    );

    println!(
        "[CT-004c.2] PASS end-to-end: durable claim → REAL runsc guest ran `echo hello-ct004c2` \
         (exit 0) → job.done buffered ONCE (references-not-payloads) → durable lease completed"
    );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn real_runsc_guest_output_seals_to_cas_and_is_readable_via_the_live_log_sink() {
    if !require_or_skip("ct004f-capstone") {
        return;
    }
    let schema = schema_name("capstone");
    let admin = admin_pool("capstone").await;
    let cleanup_admin = admin.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_admin, &schema_for_cleanup, move || async move {
    const MARKER: &str = "CAPSTONE-LOG-MARKER-9f3a";
    let region = "fr-par";
    let tenant = "tenantA";
    create_schema(&admin, &schema).await;
    create_log_tables(&admin).await;
    let store = ci_job_queue_store(admin.clone());
    let region_store = ci_region_queue_store_test_support(admin.clone());

    let run_uuid = uid("capstone-run").to_string();
    let idem = job_idem_token(&run_uuid, "ci.pipeline:0");
    let job = enq(
        tenant,
        region,
        "capstone-job",
        "capstone-run",
        TrustTier::Trusted,
        &["linux"],
        &idem,
    );
    assert_eq!(
        store.enqueue(&job).await.expect("enqueue"),
        EnqueueOutcome::Inserted
    );
    activate_job_owner(&admin, &job).await;
    seed_log_route(&admin, tenant, region, uid("capstone-job"), uid("capstone-run")).await;

    let executor = FlowExecutor::new(
        Arc::new(FixedMinter(run_uuid.clone())),
        TenantId(tenant.into()),
        Region(region.into()),
    );
    executor.register_definition("ci.pipeline");
    executor
        .start(StartSpec {
            wf_type: "ci.pipeline".into(),
            input: vec![],
            budget: None,
            idem_key: "ci:capstone-run".into(),
        })
        .expect("start the run");

    let idem_for_resolver = idem.clone();
    let resolve: JobSpecResolver = Arc::new(move |_l: &LeasedJob| {
        Ok(compute_spec(
            vec!["sh".into(), "-c".into(), format!("echo {MARKER}; exit 0")],
            &idem_for_resolver,
        ))
    });
    let adapter = DurableLeaseAdapter::new(
        region_store,
        store.clone(),
        region,
        tokio::runtime::Handle::current(),
        resolve,
    );
    let backend = GvisorBackend::new(test_registry());

    let cfg = MyelinConfig::dev();
    let handle = tokio::runtime::Handle::current();
    let firehose = LogPipelineSink::new(
        Region(region.into()),
        S3BlobStore::connect(&cfg.s3, handle.clone()),
        DurableLogPersist::with_pg(admin.clone(), handle.clone()),
    );
    let reporter = myelin_ci_sandbox::EngineTerminalReporter::new(executor.clone());
    let agent = RunnerAgent::new(
        "capstone-worker",
        vec!["linux".into()],
        vec![TrustTier::Trusted],
        Region(region.into()),
        30,
        adapter,
        &backend,
        &firehose,
        &reporter,
        ok_hooks(),
    );

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let outcome = agent
        .run_one(now)
        .expect("claim + REAL runsc + seal logs + job.done");
    assert!(outcome.report.passed, "the runsc guest exited 0");

    let run_id = uid("capstone-run");
    let job_id = uid("capstone-job");
    let seg_rows: Vec<Option<String>> =
        sqlx::query_scalar("SELECT blob_ref FROM log_segment WHERE run_id = $1 AND job_id = $2")
            .bind(run_id)
            .bind(job_id)
            .fetch_all(&admin)
            .await
            .expect("read log_segment rows");
    assert!(
        !seg_rows.is_empty(),
        "the guest's output sealed to at least one log_segment"
    );
    let anchor_status: String =
        sqlx::query_scalar("SELECT status FROM log_anchor WHERE run_id = $1 AND job_id = $2")
            .bind(run_id)
            .bind(job_id)
            .fetch_one(&admin)
            .await
            .expect("the step anchor landed");
    assert_eq!(
        anchor_status, "passed",
        "the anchor closed with the job verdict"
    );

    let aggregate = LogCoord::new(&run_uuid, job_id.to_string(), SINGLE_STEP_NO)
        .aggregate_key()
        .expect("the capstone log coordinate is canonical")
        .0;
    let pointer_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM outbox WHERE aggregate = $1 AND envelope->>'type_' = 'ci.log.available'",
    )
    .bind(&aggregate)
    .fetch_one(&admin)
    .await
    .expect("count ci.log.available");
    assert!(
        pointer_count >= 1,
        "a ci.log.available pointer rode the outbox"
    );

    let cas = S3BlobStore::connect(&cfg.s3, handle);
    let mut sealed = Vec::new();
    for blob_ref in seg_rows.into_iter().flatten() {
        let addr = ContentHash::parse(&blob_ref).expect("blob_ref parses");
        let bytes = cas
            .get(&TenantId(tenant.into()), &addr)
            .expect("read the sealed log segment back from the real S3 CAS");
        sealed.extend_from_slice(&bytes);
    }
    let readable = String::from_utf8_lossy(&sealed);
    assert!(
        readable.contains(MARKER),
        "the REAL runsc guest's stdout must be readable back from the CAS via the log index. got: {readable:?}"
    );

    println!(
        "[CT-004f] PASS capstone: durable claim → REAL runsc guest ran `echo {MARKER}` → sealed to \
         S3 CAS → log_segment/log_anchor index + ci.log.available outbox → guest stdout readable from CAS"
    );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_only_runner_never_claims_untrusted_fork_through_adapter() {
    let schema = schema_name("tier");
    let admin = admin_pool("tier").await;
    let cleanup_admin = admin.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_admin, &schema_for_cleanup, move || async move {
    let region = "fr-par";
    let tenant = "tenantA";
    create_schema(&admin, &schema).await;
    let store = ci_job_queue_store(admin.clone());
    let region_store = ci_region_queue_store_test_support(admin.clone());

    let fork = enq(
        tenant,
        region,
        "fork-job",
        "fork-run",
        TrustTier::UntrustedFork,
        &["linux"],
        "idem-fork",
    );
    store.enqueue(&fork).await.expect("enqueue fork");
    activate_job_owner(&admin, &fork).await;

    let resolve: JobSpecResolver = Arc::new(|l: &LeasedJob| {
        panic!(
            "SECURITY BREACH: the resolver was called for a leased fork job {}!",
            l.job_id
        )
    });
    let adapter = DurableLeaseAdapter::new(
        region_store.clone(),
        store.clone(),
        region,
        tokio::runtime::Handle::current(),
        resolve,
    );
    let claimed = adapter.claim_for_labels(
        "trusted-worker",
        &["linux".to_string()],
        &[TrustTier::Trusted],
        &Region(region.into()),
        1000,
        30,
    );
    assert!(
        claimed.is_none(),
        "SECURITY(a): a trusted-only runner NEVER claims an untrusted_fork job (tier filter survives the adapter)"
    );
    let state: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("fork-job"))
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        state, "queued",
        "the untrusted_fork job stays queued (unclaimed by the trusted-only runner)"
    );

    let resolve_ok: JobSpecResolver =
        Arc::new(|_l: &LeasedJob| Ok(compute_spec(vec!["true".into()], "idem-fork")));
    let fork_adapter = DurableLeaseAdapter::new(
        region_store,
        store.clone(),
        region,
        tokio::runtime::Handle::current(),
        resolve_ok,
    );
    let ok = fork_adapter.claim_for_labels(
        "fork-worker",
        &["linux".to_string()],
        &[TrustTier::UntrustedFork],
        &Region(region.into()),
        1000,
        30,
    );
    assert!(
        ok.is_some(),
        "a fork-admitting runner DOES claim the fork job (exact tier gate)"
    );
    assert_eq!(ok.unwrap().job_id, uid("fork-job").to_string());

    println!(
        "[CT-004c.2] PASS SECURITY(a): the trust-tier claim predicate survives the durable adapter"
    );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn region_a_runner_never_claims_region_b_job() {
    let schema = schema_name("region");
    let admin = admin_pool("region").await;
    let cleanup_admin = admin.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_admin, &schema_for_cleanup, move || async move {
        let tenant = "tenantA";
        create_schema(&admin, &schema).await;
        let store = ci_job_queue_store(admin.clone());
        let region_store = ci_region_queue_store_test_support(admin.clone());

        let jb = enq(
            tenant,
            "de-fra",
            "rb-job",
            "rb-run",
            TrustTier::Trusted,
            &["linux"],
            "idem-rb",
        );
        store.enqueue(&jb).await.expect("enqueue region-B job");
        activate_job_owner(&admin, &jb).await;

        let resolve: JobSpecResolver = Arc::new(|l: &LeasedJob| {
            panic!(
                "SECURITY BREACH: a region-A runner leased a region-B job {}!",
                l.job_id
            )
        });
        let adapter = DurableLeaseAdapter::new(
            region_store,
            store.clone(),
            "fr-par",
            tokio::runtime::Handle::current(),
            resolve,
        );
        let claimed = adapter.claim_for_labels(
            "fr-par-worker",
            &["linux".to_string()],
            &[TrustTier::Trusted],
            &Region("fr-par".into()),
            1000,
            30,
        );
        assert!(
        claimed.is_none(),
        "SECURITY(b): a region-A runner NEVER claims a region-B job (residency, no global pool)"
    );
        let state: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1")
            .bind(uid("rb-job"))
            .fetch_one(&admin)
            .await
            .unwrap();
        assert_eq!(state, "queued", "the region-B job stays queued");

        println!("[CT-004c.2] PASS SECURITY(b): region isolation survives the durable adapter");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stolen_lease_fails_heartbeat_no_double_run() {
    let schema = schema_name("steal");
    let admin = admin_pool("steal").await;
    let cleanup_admin = admin.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_admin, &schema_for_cleanup, move || async move {
    let region = "fr-par";
    let tenant = "tenantA";
    create_schema(&admin, &schema).await;
    let store = ci_job_queue_store(admin.clone());
    let region_store = ci_region_queue_store_test_support(admin.clone());

    let job = enq(
        tenant,
        region,
        "steal-job",
        "steal-run",
        TrustTier::Trusted,
        &["linux"],
        "idem-steal",
    );
    store.enqueue(&job).await.expect("enqueue");
    activate_job_owner(&admin, &job).await;

    let resolve: JobSpecResolver =
        Arc::new(|_l: &LeasedJob| Ok(compute_spec(vec!["true".into()], "idem-steal")));
    let adapter_a = DurableLeaseAdapter::new(
        region_store.clone(),
        store.clone(),
        region,
        tokio::runtime::Handle::current(),
        resolve,
    );

    let claimed = adapter_a
        .claim_for_labels(
            "worker-A",
            &["linux".into()],
            &[TrustTier::Trusted],
            &Region(region.into()),
            1000,
            30,
        )
        .expect("worker-A claims the job");
    assert_eq!(claimed.job_id, uid("steal-job").to_string());

    admin
        .execute(
            format!(
                "UPDATE job_queue SET lease_expires = now() - interval '1 second' WHERE job_id = '{}'",
                uid("steal-job")
            )
            .as_str(),
        )
        .await
        .unwrap();
    assert!(
        region_store.reap(region).await.expect("reap") >= 1,
        "the dead lease is re-queued"
    );
    let stolen = region_store
        .claim(
            region,
            &["linux".to_string()],
            &[TrustTier::Trusted],
            "worker-B",
            30,
        )
        .await
        .expect("claim")
        .expect("worker-B steals the re-queued lease");
    assert_eq!(stolen.job_id, uid("steal-job"));

    let held = adapter_a.heartbeat(
        "worker-A",
        &TenantId(tenant.into()),
        &uid("steal-job").to_string(),
        1005,
        30,
    );
    assert!(
        !held,
        "SECURITY(c): worker-A's heartbeat on a STOLEN lease is refused → run_one Step-2 stops (no double-run)"
    );
    let b_held = adapter_a.heartbeat(
        "worker-B",
        &TenantId(tenant.into()),
        &uid("steal-job").to_string(),
        1005,
        30,
    );
    assert!(b_held, "the true owner (worker-B) heartbeats its own lease");

    println!(
        "[CT-004c.2] PASS SECURITY(c): a stolen lease fails the heartbeat guard - no double-run"
    );
    })
    .await;
}

#[allow(dead_code)]
fn _runner_error_shapes(e: &RunnerError) -> bool {
    matches!(
        e,
        RunnerError::NoWork
            | RunnerError::LaunchFailed(_)
            | RunnerError::LeaseLost { .. }
            | RunnerError::ReportFailed(_)
    )
}

#[allow(dead_code)]
fn _adapter_is_a_lease_store(a: &DurableLeaseAdapter) -> &dyn LeaseStore {
    a
}
