//! **CT-004c.2 — the durable-backed runner EXECUTES a real job in gVisor, PROVEN against live PG +
//! real `runsc`.** This is the security-sensitive binding: the `RunnerAgent` claims from the DURABLE
//! `job_queue` (CT-004c.1's `CiJobQueueStore`, adapted to the sandbox `LeaseStore` port) and executes
//! the leased job in a REAL `runsc` (gVisor) guest, then delivers `job.done` through the engine signal
//! path and settles the lease.
//!
//! What it proves:
//!   1. **END TO END (requires real `runsc`):** enqueue a compute `JobSpec` into the durable queue →
//!      the durable-backed runner CLAIMS it → a REAL `runsc` guest runs the command → the guest's
//!      exit/output is correct → `job.done` fires ONCE via the engine signal → the durable lease is
//!      completed. SKIPS green if `runsc`/rootfs are absent; HARD-FAILS under `MYELIN_REQUIRE_RUNSC=1`.
//!   2. **SECURITY (a) — the tier filter survives the adapter:** a runner with trusted-only
//!      `allowed_tiers` NEVER claims/executes an `untrusted_fork` job (the durable predicate, forwarded
//!      unchanged). The fork row stays `queued`, never leased.
//!   3. **SECURITY (b) — region isolation:** a runner in region A does not claim a region-B job.
//!   4. **SECURITY (c) — stolen-lease / no double-run:** a job whose lease was stolen mid-flight fails
//!      the runner's heartbeat (`run_one` Step-2 guard, exercised through the durable store) → the
//!      runner would NOT proceed to a second run.
//!   5. **SECURITY (d) — references-not-payloads:** the buffered `job.done` payload carries the pass
//!      marker + the `ci.log.available` ref, NEVER the guest's captured stdout bytes (those ride the
//!      firehose).
//!
//! Gated behind the `integration` cargo feature. Run against the docker-compose dev stack:
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   export MYELIN_REQUIRE_RUNSC=1
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_ct004c2_runner_exec -- --nocapture
#![cfg(feature = "integration")]

use std::path::Path;
use std::sync::{Arc, Mutex};

use myelin_ci_controlplane::{
    ci_job_queue_store, DurableEnqueue, DurableLeaseAdapter, EnqueueOutcome, JobSpecResolver, Lane,
    LeasedJob, CREATE_FAIR_DEFICIT_DDL, CREATE_JOB_QUEUE_DDL, CREATE_JOB_QUEUE_INDEXES_DDL,
};
use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    resolved_gvisor_rootfs, EgressPolicy, FirehoseSink, IdemToken, ImageRef, JobKind, JobSpec,
    LeaseStore, MeterTarget, ReserveHandle, ResourceLimits, RunTokenRef, RunnerAgent, RunnerError,
    RunnerHooks, TrustTier, WorkspaceSpec,
};
use myelin_events::{IdMinter, Ulid};
use myelin_flow::{
    job_idem_token, DurableExecutor, FlowExecutor, SignalOutcome, StartSpec, JOB_DONE_SIGNAL,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::types::Uuid;
use sqlx::{Executor, PgPool};

// ─────────────────────────────── PG / schema plumbing (reuses CT-004c.1 shapes) ──────────────────

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}
// A per-TEST schema name (not just per-pid): the four tests run in parallel in ONE test binary, so a
// pid-only name collided on `CREATE SCHEMA` (the setup raced) — the `tag` makes each test's schema unique.
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
        .expect("connect to dev Postgres as admin (is the stack up? eval \"$(scripts/dev-stack.sh env)\")")
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
    for (_name, idx) in CREATE_JOB_QUEUE_INDEXES_DDL {
        let idx = idx.replace("CONCURRENTLY ", "");
        admin.execute(idx.as_str()).await.expect("index");
    }
    admin
        .execute(CREATE_FAIR_DEFICIT_DDL)
        .await
        .expect("create fair_deficit");
}

async fn drop_schema(admin: &PgPool, schema: &str) {
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .ok();
}

/// A stable uuid from a name (FNV-1a fill) — the durable `uuid` columns require real uuids.
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
    }
}

// ─────────────────────────────── runsc gating (mirrors gvisor_prod_exec_test) ────────────────────

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

/// HARD-FAIL under `MYELIN_REQUIRE_RUNSC=1`; else GRACEFUL SKIP. Returns whether to run the real-exec.
fn require_or_skip(test: &str) -> bool {
    if runsc_present() {
        return true;
    }
    if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") {
        panic!(
            "[{test}] MYELIN_REQUIRE_RUNSC=1 but `runsc` is not on PATH or the staged rootfs ({}) is \
             absent — CT-004c.2 refuses a VACUOUS green: a real `runsc` guest MUST run the leased job.",
            resolved_gvisor_rootfs().display()
        );
    }
    eprintln!("[{test}] SKIPPED: `runsc`/rootfs absent — this host cannot run a gVisor guest.");
    false
}

// ─────────────────────────────── sandbox seam helpers ────────────────────────────────────────────

/// A firehose sink that CAPTURES the shipped frames so the test can assert the REAL guest output flowed
/// through the references-not-payloads firehose (never the signal payload).
#[derive(Clone, Default)]
struct CapturingFirehose {
    bytes: Arc<Mutex<Vec<u8>>>,
}
impl FirehoseSink for CapturingFirehose {
    fn ship_frame(&self, _run_id: &str, _job_id: &str, _tenant: &TenantId, frame: &[u8]) {
        self.bytes.lock().unwrap().extend_from_slice(frame);
    }
    fn finish(&self, _run_id: &str, _job_id: &str, _tenant: &TenantId, _passed: bool) {}
}
impl CapturingFirehose {
    fn captured(&self) -> Vec<u8> {
        self.bytes.lock().unwrap().clone()
    }
}

/// A minter that yields a FIXED ULID whose string is a real uuid — so the executor's started run id ==
/// the durable row's `run_id` (uuid column), which the `job.done` targets.
struct FixedMinter(String);
impl IdMinter for FixedMinter {
    fn mint(&self) -> Ulid {
        Ulid(self.0.clone())
    }
}

fn ok_hooks() -> RunnerHooks {
    RunnerHooks {
        reserve: Box::new(|m| Ok(ReserveHandle(m.reserve_id.clone()))),
        settle: Box::new(|_h, _u| Ok(())),
        attribute: Box::new(|_t| Ok(())),
        isolation_floor: Box::new(|_s| Ok(())),
    }
}

/// A real compute `JobSpec` running `command` — the shape a `runsc` guest actually executes.
fn compute_spec(command: Vec<String>, idem: &str) -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap(),
        command,
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 * 1024 * 1024,
            disk_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs: 60,
        },
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        RunTokenRef { jti: "ct004c2-jti".into() },
        MeterTarget { reserve_id: "ct004c2-reserve".into() },
        IdemToken(idem.into()),
    )
    .unwrap()
}

// ═════════════════════════════════════ 1. END-TO-END (real runsc) ════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn durable_backed_runner_executes_real_runsc_end_to_end() {
    if !require_or_skip("ct004c2-e2e") {
        return;
    }
    let schema = schema_name("e2e");
    let region = "fr-par";
    let tenant = "tenantA";
    let admin = admin_pool("e2e").await;
    create_schema(&admin, &schema).await;
    let store = ci_job_queue_store(admin.clone());

    // The durable run id is a uuid; the executor's started run must carry the SAME id (job.done target).
    let run_uuid = uid("e2e-run").to_string();
    let idem = job_idem_token(&run_uuid, "ci.pipeline:0");

    // ── enqueue a real compute job into the DURABLE queue (trusted, linux). ──
    let job = enq(tenant, region, "e2e-job", "e2e-run", TrustTier::Trusted, &["linux"], &idem);
    assert_eq!(
        store.enqueue(&job).await.expect("enqueue"),
        EnqueueOutcome::Inserted
    );

    // ── the engine executor the job.done wakes (the ONE signal path). Run id == the durable run_id. ──
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
    assert_eq!(started.0, run_uuid, "the started run id == the durable run_id");

    // ── the durable-store lease adapter (the security pass-through) + a REAL resolver → compute spec. ──
    let idem_for_resolver = idem.clone();
    let resolve: JobSpecResolver = Arc::new(move |_l: &LeasedJob| {
        Ok(compute_spec(
            vec!["sh".into(), "-c".into(), "echo hello-ct004c2; exit 0".into()],
            &idem_for_resolver,
        ))
    });
    let adapter = DurableLeaseAdapter::new(
        store.clone(),
        region,
        tokio::runtime::Handle::current(),
        resolve,
    );

    let backend = GvisorBackend::new();
    let firehose = CapturingFirehose::default();
    let reporter = myelin_ci_sandbox::EngineTerminalReporter::new(executor.clone());
    let agent = RunnerAgent::new(
        "e2e-worker",
        vec!["linux".into()],
        vec![TrustTier::Trusted], // trusted-only
        Region(region.into()),
        30,
        adapter,
        &backend,
        &firehose,
        &reporter,
        ok_hooks(),
    );

    // ── DRIVE the full claim → REAL runsc launch → job.done → settle cycle. ──
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
    println!("job_id={} run_id={} passed={}", outcome.job_id, outcome.run_id, outcome.report.passed);
    println!("REAL guest stdout (via firehose) = {guest:?}");
    println!("job.done delivery = {:?}", outcome.signal_outcome);

    assert_eq!(outcome.job_id, uid("e2e-job").to_string());
    assert_eq!(outcome.run_id, run_uuid);
    assert!(
        outcome.report.passed,
        "the real `runsc` guest exited 0 (clean) — derived passed=true"
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
        executor.signals().count_for_run(&TenantId(tenant.into()), &run_uuid),
        1,
        "the engine buffered EXACTLY ONE job.done"
    );

    // ── SECURITY (d): references-not-payloads — the job.done payload carries refs, NOT guest bytes. ──
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
            "SECURITY(d): captured guest stdout MUST NEVER enter the job.done signal payload — it \
             rides the firehose (references-not-payloads). offending ref: {r:?}"
        );
    }
    assert_eq!(row.payload_key_ref, None, "no inline PII payload");

    // ── the durable lease was SETTLED (the row moved to terminal). ──
    let state: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("e2e-job"))
        .fetch_one(&admin)
        .await
        .expect("read job state");
    assert_eq!(state, "terminal", "the lease is completed on terminal (settle)");

    drop_schema(&admin, &schema).await;
    println!(
        "[CT-004c.2] PASS end-to-end: durable claim → REAL runsc guest ran `echo hello-ct004c2` \
         (exit 0) → job.done buffered ONCE (references-not-payloads) → durable lease completed"
    );
}

// ═════════════════════════════ 2. SECURITY (a): tier filter survives the adapter ═════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_only_runner_never_claims_untrusted_fork_through_adapter() {
    let schema = schema_name("tier");
    let region = "fr-par";
    let tenant = "tenantA";
    let admin = admin_pool("tier").await;
    create_schema(&admin, &schema).await;
    let store = ci_job_queue_store(admin.clone());

    // ONLY an untrusted_fork job is queued (linux, in-region).
    let fork = enq(tenant, region, "fork-job", "fork-run", TrustTier::UntrustedFork, &["linux"], "idem-fork");
    store.enqueue(&fork).await.expect("enqueue fork");

    // A trusted-only adapter claim (the exact seam run_one drives) must return None — the fork job is
    // NEVER claimable by a trusted-only runner (the durable predicate, forwarded UNCHANGED).
    let resolve: JobSpecResolver = Arc::new(|l: &LeasedJob| {
        // If we ever reach here the tier filter was breached — force a loud failure.
        panic!("SECURITY BREACH: the resolver was called for a leased fork job {}!", l.job_id)
    });
    let adapter = DurableLeaseAdapter::new(store.clone(), region, tokio::runtime::Handle::current(), resolve);
    let claimed = adapter.claim_for_labels(
        "trusted-worker",
        &["linux".to_string()],
        &[TrustTier::Trusted], // trusted-only
        &Region(region.into()),
        1000,
        30,
    );
    assert!(
        claimed.is_none(),
        "SECURITY(a): a trusted-only runner NEVER claims an untrusted_fork job (tier filter survives the adapter)"
    );
    // The fork row is untouched — still queued (never leased, never executed).
    let state: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("fork-job"))
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(state, "queued", "the untrusted_fork job stays queued (unclaimed by the trusted-only runner)");

    // Control: a fork-admitting adapter DOES claim it (the gate is exact, not a blanket deny).
    let resolve_ok: JobSpecResolver =
        Arc::new(|_l: &LeasedJob| Ok(compute_spec(vec!["true".into()], "idem-fork")));
    let fork_adapter = DurableLeaseAdapter::new(store.clone(), region, tokio::runtime::Handle::current(), resolve_ok);
    let ok = fork_adapter.claim_for_labels(
        "fork-worker",
        &["linux".to_string()],
        &[TrustTier::UntrustedFork],
        &Region(region.into()),
        1000,
        30,
    );
    assert!(ok.is_some(), "a fork-admitting runner DOES claim the fork job (exact tier gate)");
    assert_eq!(ok.unwrap().job_id, uid("fork-job").to_string());

    drop_schema(&admin, &schema).await;
    println!("[CT-004c.2] PASS SECURITY(a): the trust-tier claim predicate survives the durable adapter");
}

// ═════════════════════════════ 3. SECURITY (b): region isolation ═════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn region_a_runner_never_claims_region_b_job() {
    let schema = schema_name("region");
    let tenant = "tenantA";
    let admin = admin_pool("region").await;
    create_schema(&admin, &schema).await;
    let store = ci_job_queue_store(admin.clone());

    // A trusted linux job in region B only.
    let jb = enq(tenant, "de-fra", "rb-job", "rb-run", TrustTier::Trusted, &["linux"], "idem-rb");
    store.enqueue(&jb).await.expect("enqueue region-B job");

    let resolve: JobSpecResolver = Arc::new(|l: &LeasedJob| {
        panic!("SECURITY BREACH: a region-A runner leased a region-B job {}!", l.job_id)
    });
    let adapter = DurableLeaseAdapter::new(store.clone(), "fr-par", tokio::runtime::Handle::current(), resolve);
    let claimed = adapter.claim_for_labels(
        "fr-par-worker",
        &["linux".to_string()],
        &[TrustTier::Trusted],
        &Region("fr-par".into()), // region A
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

    drop_schema(&admin, &schema).await;
    println!("[CT-004c.2] PASS SECURITY(b): region isolation survives the durable adapter");
}

// ═════════════════════════════ 4. SECURITY (c): stolen lease → no double-run ══════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stolen_lease_fails_heartbeat_no_double_run() {
    let schema = schema_name("steal");
    let region = "fr-par";
    let tenant = "tenantA";
    let admin = admin_pool("steal").await;
    create_schema(&admin, &schema).await;
    let store = ci_job_queue_store(admin.clone());

    let job = enq(tenant, region, "steal-job", "steal-run", TrustTier::Trusted, &["linux"], "idem-steal");
    store.enqueue(&job).await.expect("enqueue");

    let resolve: JobSpecResolver =
        Arc::new(|_l: &LeasedJob| Ok(compute_spec(vec!["true".into()], "idem-steal")));
    let adapter_a =
        DurableLeaseAdapter::new(store.clone(), region, tokio::runtime::Handle::current(), resolve);

    // worker-A claims the job (through the adapter — the exact run_one Step-1 path).
    let claimed = adapter_a
        .claim_for_labels("worker-A", &["linux".into()], &[TrustTier::Trusted], &Region(region.into()), 1000, 30)
        .expect("worker-A claims the job");
    assert_eq!(claimed.job_id, uid("steal-job").to_string());

    // The lease is STOLEN mid-flight: force it expired, the reaper re-queues it, worker-B re-claims it.
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
    assert!(store.reap(region).await.expect("reap") >= 1, "the dead lease is re-queued");
    let stolen = store
        .claim(region, &["linux".to_string()], &[TrustTier::Trusted], "worker-B", 30)
        .await
        .expect("claim")
        .expect("worker-B steals the re-queued lease");
    assert_eq!(stolen.job_id, uid("steal-job"));

    // worker-A's heartbeat (run_one Step-2 confirm) now FAILS — it lost the lease → it must NOT launch a
    // second run of the job worker-B owns. This is the double-run guard, through the durable store.
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
    // worker-B, the true owner, CAN heartbeat (sanity: the guard is owner-exact, not a blanket deny).
    let b_held = adapter_a.heartbeat(
        "worker-B",
        &TenantId(tenant.into()),
        &uid("steal-job").to_string(),
        1005,
        30,
    );
    assert!(b_held, "the true owner (worker-B) heartbeats its own lease");

    drop_schema(&admin, &schema).await;
    println!("[CT-004c.2] PASS SECURITY(c): a stolen lease fails the heartbeat guard — no double-run");
}

// A tiny compile/behaviour anchor so the RunnerError variants the loop matches stay in view.
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

/// A `LeaseStore` object-safety anchor: the durable adapter IS a `LeaseStore` (the port `run_one`
/// drives), so the compiler proves the seam is the SAME one the in-memory floor satisfies.
#[allow(dead_code)]
fn _adapter_is_a_lease_store(a: &DurableLeaseAdapter) -> &dyn LeaseStore {
    a
}
