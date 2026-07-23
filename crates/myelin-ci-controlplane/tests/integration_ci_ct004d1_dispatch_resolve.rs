//! **CT-004d.1 — a REAL dispatch co-persists the durable job + its JobSpec, and the durable-backed
//! runner resolves + EXECUTES it, PROVEN against live PG + real `runsc`.** This closes the
//! dispatch→durable→resolve bridge CT-004c.2 left open (it hand-enqueued a bare row + injected a no-op
//! resolver). Here the dispatch is REAL ([`CiJobSpecStore::co_persist_dispatch`] — one tx writes BOTH
//! the `job_queue` row AND the `ci_job_spec` spec, idempotent on the shared `idem_token`), and the
//! resolver is REAL ([`durable_spec_resolver`] over the durable spec store).
//!
//! What it proves:
//!   1. **END TO END (requires real `runsc`):** a real dispatch co-persists the durable job + spec →
//!      the durable-backed runner (REAL resolver) CLAIMS it → resolves the EXACT persisted spec → a
//!      REAL `runsc` guest runs it → `job.done` fires ONCE → the durable lease is completed. SKIPS
//!      green if `runsc`/rootfs absent; HARD-FAILS under `MYELIN_REQUIRE_RUNSC=1`.
//!   2. **THE SECURITY INVARIANT (the gate is fed from the spec, not widened):** the `job_queue` row's
//!      `trust_tier`/`region` come from the dispatched spec's real trust/region — a co-persist that
//!      tries to enqueue a `trusted` gate over an `untrusted_fork` spec is REFUSED fail-closed
//!      (`TrustTierMismatch`), and every honest tier round-trips onto the row.
//!   3. **SECURITY REGRESSION — the tier gate survives the dispatch path:** a trusted-only runner
//!      NEVER claims/executes an `untrusted_fork` stage dispatched into the queue (the CT-004c.1/c.2
//!      predicate, unbroken by the real dispatch + real resolver).
//!   4. **FAIL-CLOSED RESOLVE:** a leased row with NO persisted spec (or a corrupt one) resolves to a
//!      fail-closed error → the runner does not launch → the row stays leased for the reaper (never a
//!      fabricated/default-spec launch).
//!   5. **IDEMPOTENT:** a re-dispatch on the same `idem_token`/`job_id` collapses to ONE `job_queue`
//!      row + ONE spec row (effectively-once).
//!
//! Gated behind the `integration` cargo feature. Run against the docker-compose dev stack:
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   export MYELIN_REQUIRE_RUNSC=1
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_ct004d1_dispatch_resolve -- --nocapture
#![cfg(feature = "integration")]

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use myelin_ci_controlplane::{
    ci_job_queue_store, ci_job_spec_store, ci_region_queue_store_test_support,
    durable_spec_resolver_test_support, CiJobSpecStoreError, CiJobTokenIssueError,
    CiJobTokenIssuer, CiJobTokenRequest, DurableCiJobLaunchTemplate, DurableEnqueue,
    DurableLeaseAdapter, EnqueueOutcome, Lane, ALTER_CI_JOB_SPEC_ADD_STAGE_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
    ALTER_JOB_QUEUE_ADD_COMPLETION_DDL, CI_RUNNER_LEASE_TTL_SECS, CREATE_CI_JOB_SPEC_DDL,
    CREATE_FAIR_DEFICIT_DDL, CREATE_JOB_QUEUE_DDL, CREATE_JOB_QUEUE_INDEXES_DDL,
    MAX_JOB_TIMEOUT_SECS,
};
use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    resolved_gvisor_rootfs, EgressPolicy, FirehoseSink, IdemToken, ImageRef, JobKind, JobSpec,
    LeaseStore, MeterTarget, ReserveHandle, ResourceLimits, RunTokenCredential, RunnerAgent,
    RunnerHooks, TrustTier, WorkspaceSpec,
};
use myelin_events::{IdMinter, Ulid};
use myelin_flow::{
    job_idem_token, DurableExecutor, FlowExecutor, SignalOutcome, StartSpec, JOB_DONE_SIGNAL,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::types::Uuid;
use sqlx::{Executor, PgPool, Row};

// ─────────────────────────────── PG / schema plumbing (reuses CT-004c.2 shapes) ──────────────────

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}
fn schema_name(tag: &str) -> String {
    format!("ci_ct004d1_{}_{}", std::process::id(), tag)
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
        .expect("create the per-test schema");
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
    for (_name, idx) in CREATE_JOB_QUEUE_INDEXES_DDL {
        let idx = idx.replace("CONCURRENTLY ", "");
        admin.execute(idx.as_str()).await.expect("index");
    }
    admin
        .execute(CREATE_FAIR_DEFICIT_DDL)
        .await
        .expect("create fair_deficit");
    // CT-004d.1: the durable JobSpec store table + the CT-004d.2 durable stage column.
    admin
        .execute(CREATE_CI_JOB_SPEC_DDL)
        .await
        .expect("create ci_job_spec");
    admin
        .execute(ALTER_CI_JOB_SPEC_ADD_STAGE_DDL)
        .await
        .expect("add ci_job_spec.stage");
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

/// Build the durable enqueue terms for a dispatched stage. **`trust` is fed FROM the spec's tier by
/// the caller** — `co_persist_dispatch` refuses a mismatch, so this mirrors the security invariant.
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
    }
}

// ─────────────────────────────── runsc gating (mirrors CT-004c.2) ─────────────────────────────────

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
             absent — CT-004d.1 refuses a VACUOUS green: a real `runsc` guest MUST run the resolved job.",
            resolved_gvisor_rootfs().display()
        );
    }
    eprintln!("[{test}] SKIPPED: `runsc`/rootfs absent — this host cannot run a gVisor guest.");
    false
}

// ─────────────────────────────── sandbox seam helpers ────────────────────────────────────────────

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

struct FixedMinter(String);
impl IdMinter for FixedMinter {
    fn mint(&self) -> Ulid {
        Ulid(self.0.clone())
    }
}

fn ok_hooks() -> RunnerHooks {
    RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

/// A real compute `JobSpec` running `command` at `trust`. The spec's `trust_tier` is the truth the
/// dispatch feeds onto the `job_queue` gate.
fn compute_spec(command: Vec<String>, trust: TrustTier, idem: &str) -> DurableCiJobLaunchTemplate {
    let resolved = JobSpec::new(
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
        trust,
        RunTokenCredential::new("test-bearer", "ct004d1-jti", 300).unwrap(),
        MeterTarget { reserve_id: "ct004d1-reserve".into() },
        IdemToken(idem.into()),
    )
    .unwrap();
    let (spec, _token) = resolved.into_template();
    DurableCiJobLaunchTemplate {
        spec,
        ci_run_id: uid(&format!("ci-run:{idem}")).to_string(),
        token_authority_handle: format!("identity-authority:{idem}"),
    }
}

struct ClaimTokenIssuer;

impl CiJobTokenIssuer for ClaimTokenIssuer {
    fn mint(
        &self,
        request: CiJobTokenRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + '_>>
    {
        Box::pin(async move {
            RunTokenCredential::new(
                format!("claim-bearer:{}", request.claim_nonce),
                format!(
                    "claim-jti:{}:{}:{}",
                    request.job_id, request.lease_epoch, request.claim_nonce
                ),
                300,
            )
            .map_err(|error| CiJobTokenIssueError(error.to_string()))
        })
    }
}

fn claim_token_issuer() -> Arc<dyn CiJobTokenIssuer> {
    Arc::new(ClaimTokenIssuer)
}

// ═══════════════ 1. END-TO-END: real dispatch → durable job+spec → real resolve → runsc ═══════════

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn real_dispatch_co_persists_then_durable_resolver_executes_in_runsc() {
    if !require_or_skip("ct004d1-e2e") {
        return;
    }
    let schema = schema_name("e2e");
    let region = "fr-par";
    let tenant = "tenantA";
    let admin = admin_pool("e2e").await;
    create_schema(&admin, &schema).await;

    let queue = ci_job_queue_store(admin.clone());
    let specs = ci_job_spec_store(admin.clone());

    let run_uuid = uid("e2e-run").to_string();
    let idem = job_idem_token(&run_uuid, "ci.pipeline:0");

    // ── THE REAL DISPATCH: co-persist the durable job_queue row + the JobSpec in ONE tx. ──
    let spec = compute_spec(
        vec![
            "sh".into(),
            "-c".into(),
            "echo hello-ct004d1; exit 0".into(),
        ],
        TrustTier::Trusted,
        &idem,
    );
    let terms = enq(
        tenant,
        region,
        "e2e-job",
        "e2e-run",
        TrustTier::Trusted,
        &["linux"],
        &idem,
    );
    let outcome = specs
        .co_persist_dispatch(&terms, &spec, "build")
        .await
        .expect("the dispatch co-persists the job_queue row + the JobSpec");
    assert_eq!(
        outcome.enqueue,
        EnqueueOutcome::Inserted,
        "a fresh job_queue row"
    );
    assert!(outcome.spec_inserted, "a fresh ci_job_spec row");

    // ── the durable job_queue row carries the SPEC's trust_tier + the run's region (fed, not defaulted). ──
    let (jq_trust, jq_region, jq_state): (String, String, String) =
        sqlx::query("SELECT trust_tier, region, state FROM job_queue WHERE job_id = $1")
            .bind(uid("e2e-job"))
            .fetch_one(&admin)
            .await
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .expect("the job_queue row");
    assert_eq!(
        jq_trust, "trusted",
        "the row's trust_tier is the SPEC's tier (not widened/defaulted)"
    );
    assert_eq!(
        jq_region, region,
        "the row's region is the run's residency (not defaulted)"
    );
    assert_eq!(jq_state, "queued", "the dispatched job is claimable");

    // ── the ci_job_spec row exists and round-trips the exact spec. ──
    let resolved_back = specs
        .get_launch_template(tenant, region, &uid("e2e-job").to_string())
        .await
        .expect("the persisted spec resolves back");
    assert_eq!(
        resolved_back, spec,
        "the persisted spec round-trips faithfully (every field)"
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
    assert_eq!(
        started.0, run_uuid,
        "the started run id == the durable run_id"
    );

    // ── the durable adapter + the REAL durable resolver (over ci_job_spec) — NOT an injected closure. ──
    let resolver = durable_spec_resolver_test_support(
        specs.clone(),
        region,
        tokio::runtime::Handle::current(),
        claim_token_issuer(),
    );
    let adapter = DurableLeaseAdapter::new(
        ci_region_queue_store_test_support(admin.clone()),
        queue.clone(),
        region,
        tokio::runtime::Handle::current(),
        resolver,
    );

    let backend = GvisorBackend::new();
    let firehose = CapturingFirehose::default();
    let reporter = myelin_ci_sandbox::EngineTerminalReporter::new(executor.clone());
    let agent = RunnerAgent::new(
        "e2e-worker",
        vec!["linux".into()],
        vec![TrustTier::Trusted], // trusted-only
        Region(region.into()),
        CI_RUNNER_LEASE_TTL_SECS, // the CT-004d.1 lease-TTL floor (> max job timeout)
        adapter,
        &backend,
        &firehose,
        &reporter,
        ok_hooks(),
    );

    // ── DRIVE the full claim → REAL resolve → runsc launch → job.done → settle cycle. ──
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let ran = agent.run_one(now).expect(
        "the runner claims + resolves the REAL spec + runs it in gVisor + reports terminal",
    );

    let captured = firehose.captured();
    let guest = String::from_utf8_lossy(&captured);
    println!("=== CT-004d.1 REAL end-to-end (dispatch co-persist → durable resolve → runsc → job.done) ===");
    println!(
        "job_id={} run_id={} passed={}",
        ran.job_id, ran.run_id, ran.report.passed
    );
    println!("REAL guest stdout (via firehose) = {guest:?}");
    println!("job.done delivery = {:?}", ran.signal_outcome);

    assert_eq!(ran.job_id, uid("e2e-job").to_string());
    assert_eq!(ran.run_id, run_uuid);
    assert!(ran.report.passed, "the real `runsc` guest exited 0 (clean)");
    assert!(
        guest.contains("hello-ct004d1"),
        "the REAL runsc guest ran the RESOLVED spec's command. got: {guest:?}"
    );
    assert_eq!(
        ran.signal_outcome,
        SignalOutcome::Buffered,
        "the first job.done wakes the parked run"
    );
    assert_eq!(
        executor
            .signals()
            .count_for_run(&TenantId(tenant.into()), &run_uuid),
        1,
        "exactly one job.done buffered"
    );

    // references-not-payloads: the job.done payload carries refs, not the guest bytes.
    let row = executor
        .signals()
        .get(&TenantId(tenant.into()), &run_uuid, JOB_DONE_SIGNAL, &idem)
        .expect("the job.done buffered under the echoed idem_token");
    for r in &row.payload {
        assert!(
            !r.0.contains("hello-ct004d1"),
            "captured guest stdout MUST NEVER enter the job.done payload: {r:?}"
        );
    }

    // the durable lease is SETTLED (terminal).
    let state: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("e2e-job"))
        .fetch_one(&admin)
        .await
        .expect("read job state");
    assert_eq!(state, "terminal", "the lease is completed on terminal");

    // ── IDEMPOTENT re-dispatch: same idem_token/job_id → no second job_queue row + no second spec. ──
    let again = specs
        .co_persist_dispatch(&terms, &spec, "build")
        .await
        .expect("re-dispatch is a clean no-op");
    assert_eq!(
        again.enqueue,
        EnqueueOutcome::DuplicateIdem,
        "the job_queue jq_idem collapses the re-enqueue"
    );
    assert!(
        !again.spec_inserted,
        "the (tenant, job_id) PK collapses the re-persist"
    );

    drop_schema(&admin, &schema).await;
    println!(
        "[CT-004d.1] PASS end-to-end: real dispatch co-persisted job+spec → durable resolver read the \
         EXACT spec → REAL runsc guest ran it → job.done ONCE → lease terminal → re-dispatch idempotent"
    );
}

// ═══════════════ 2. THE SECURITY INVARIANT: the gate is fed from the spec, not widened ════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_feeds_trust_and_region_from_the_spec_never_widened() {
    let schema = schema_name("feed");
    let region = "fr-par";
    let tenant = "tenantA";
    let admin = admin_pool("feed").await;
    create_schema(&admin, &schema).await;
    let specs = ci_job_spec_store(admin.clone());

    // Every honest tier round-trips onto the job_queue row from the SPEC's tier.
    for (i, tier) in [
        TrustTier::Trusted,
        TrustTier::UntrustedFork,
        TrustTier::SelfHosted,
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("feed-job-{i}");
        let run = format!("feed-run-{i}");
        let idem = format!("feed-idem-{i}");
        let spec = compute_spec(vec!["true".into()], tier, &idem);
        let terms = enq(tenant, region, &id, &run, tier, &["linux"], &idem);
        specs
            .co_persist_dispatch(&terms, &spec, "build")
            .await
            .expect("dispatch");
        let (jq_trust, jq_region): (String, String) =
            sqlx::query("SELECT trust_tier, region FROM job_queue WHERE job_id = $1")
                .bind(uid(&id))
                .fetch_one(&admin)
                .await
                .map(|r| (r.get(0), r.get(1)))
                .unwrap();
        let expect = match tier {
            TrustTier::Trusted => "trusted",
            TrustTier::UntrustedFork => "untrusted_fork",
            TrustTier::SelfHosted => "self_hosted",
        };
        assert_eq!(jq_trust, expect, "the row's trust_tier == the SPEC's tier");
        assert_eq!(jq_region, region, "the row's region == the run's residency");
    }

    // THE WIDENING ATTEMPT: gate a fork spec behind a `trusted` enqueue tier → REFUSED fail-closed,
    // and NO row is written (the gate can never carry a wider tier than the spec that executes).
    let fork_spec = compute_spec(vec!["true".into()], TrustTier::UntrustedFork, "widen-idem");
    let widened = enq(
        tenant,
        region,
        "widen-job",
        "widen-run",
        TrustTier::Trusted,
        &["linux"],
        "widen-idem",
    );
    let err = specs
        .co_persist_dispatch(&widened, &fork_spec, "build")
        .await
        .expect_err("a widened gate tier is refused");
    assert!(
        matches!(err, CiJobSpecStoreError::TrustTierMismatch { .. }),
        "SECURITY: enqueue trust must equal spec trust — got {err:?}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM job_queue WHERE job_id = $1")
        .bind(uid("widen-job"))
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "the refused dispatch wrote NO job_queue row (fail-closed before any write)"
    );

    drop_schema(&admin, &schema).await;
    println!("[CT-004d.1] PASS: the job_queue trust_tier/region are FED from the spec; a widened gate is refused");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_replay_requires_exact_queue_and_spec_identity() {
    let schema = schema_name("exact_replay");
    let admin = admin_pool("exact_replay").await;
    create_schema(&admin, &schema).await;
    let store = ci_job_spec_store(admin.clone());
    let idem = "exact-replay-idem";
    let original = enq(
        "tenantA",
        "fr-par",
        "exact-job",
        "exact-run",
        TrustTier::Trusted,
        &["linux"],
        idem,
    );
    let spec = compute_spec(vec!["true".into()], TrustTier::Trusted, idem);
    store
        .co_persist_dispatch(&original, &spec, "build")
        .await
        .expect("persist original dispatch");

    let colliding_job = enq(
        "tenantA",
        "fr-par",
        "forged-job",
        "exact-run",
        TrustTier::Trusted,
        &["linux"],
        idem,
    );
    let collision = store
        .co_persist_dispatch(&colliding_job, &spec, "build")
        .await
        .expect_err("one idem token cannot be rebound to another job UUID");
    assert!(collision
        .to_string()
        .contains("replay conflicts with the existing queue/spec identity"));

    let mut drifted = original.clone();
    drifted.fair_key = "forged-fair-key".into();
    let drift = store
        .co_persist_dispatch(&drifted, &spec, "build")
        .await
        .expect_err("an exact job replay cannot change scheduling authority");
    assert!(drift
        .to_string()
        .contains("replay conflicts with the existing queue/spec identity"));

    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM job_queue), (SELECT count(*) FROM ci_job_spec)",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(counts, (1, 1), "both divergent transactions rolled back");
    assert_eq!(
        store
            .get_launch_template("tenantA", "fr-par", &original.job_id)
            .await
            .unwrap(),
        spec,
        "the original executable spec remains authoritative"
    );

    drop_schema(&admin, &schema).await;
}

// ═══════════════ 3. SECURITY REGRESSION: the tier gate survives the real dispatch+resolve path ════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_runner_never_executes_a_dispatched_untrusted_fork() {
    let schema = schema_name("fork");
    let region = "fr-par";
    let tenant = "tenantA";
    let admin = admin_pool("fork").await;
    create_schema(&admin, &schema).await;
    let queue = ci_job_queue_store(admin.clone());
    let specs = ci_job_spec_store(admin.clone());

    // A REAL dispatch of an untrusted_fork stage (spec + row both fork) into the queue.
    let fork_spec = compute_spec(vec!["true".into()], TrustTier::UntrustedFork, "fork-idem");
    let terms = enq(
        tenant,
        region,
        "fork-job",
        "fork-run",
        TrustTier::UntrustedFork,
        &["linux"],
        "fork-idem",
    );
    specs
        .co_persist_dispatch(&terms, &fork_spec, "build")
        .await
        .expect("dispatch the fork stage");

    // A trusted-only runner, using the REAL durable resolver, drives the exact run_one claim seam.
    let resolver = durable_spec_resolver_test_support(
        specs.clone(),
        region,
        tokio::runtime::Handle::current(),
        claim_token_issuer(),
    );
    let adapter = DurableLeaseAdapter::new(
        ci_region_queue_store_test_support(admin.clone()),
        queue.clone(),
        region,
        tokio::runtime::Handle::current(),
        resolver,
    );
    let claimed = adapter.claim_for_labels(
        "trusted-worker",
        &["linux".to_string()],
        &[TrustTier::Trusted], // trusted-only
        &Region(region.into()),
        1000,
        CI_RUNNER_LEASE_TTL_SECS,
    );
    assert!(
        claimed.is_none(),
        "SECURITY REGRESSION: a trusted-only runner NEVER claims a dispatched untrusted_fork job"
    );
    // the fork row is untouched — still queued, never leased/executed.
    let state: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("fork-job"))
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        state, "queued",
        "the untrusted_fork stage stays queued (unclaimed by the trusted-only runner)"
    );

    drop_schema(&admin, &schema).await;
    println!(
        "[CT-004d.1] PASS: the trust-tier gate survives the real dispatch + real resolver path"
    );
}

// ═══════════════ 4. FAIL-CLOSED RESOLVE: a leased row with no spec never launches ════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_leased_row_without_a_spec_resolves_fail_closed() {
    let schema = schema_name("nospec");
    let region = "fr-par";
    let tenant = "tenantA";
    let admin = admin_pool("nospec").await;
    create_schema(&admin, &schema).await;
    let queue = ci_job_queue_store(admin.clone());
    let specs = ci_job_spec_store(admin.clone());

    // A bare job_queue row (via the queue store directly) — NO ci_job_spec row co-persisted.
    let bare = enq(
        tenant,
        region,
        "nospec-job",
        "nospec-run",
        TrustTier::Trusted,
        &["linux"],
        "nospec-idem",
    );
    queue.enqueue(&bare).await.expect("enqueue a bare row");

    // The real resolver over the (empty) spec store must FAIL CLOSED for this leased row.
    let missing = specs
        .get_launch_template(tenant, region, &uid("nospec-job").to_string())
        .await;
    assert!(
        matches!(missing, Err(CiJobSpecStoreError::SpecNotFound { .. })),
        "an absent spec is SpecNotFound (fail-closed), got {missing:?}"
    );

    // Through the runner seam: claim resolves fail-closed → no launch → the row stays leased for the reaper.
    let resolver = durable_spec_resolver_test_support(
        specs.clone(),
        region,
        tokio::runtime::Handle::current(),
        claim_token_issuer(),
    );
    let adapter = DurableLeaseAdapter::new(
        ci_region_queue_store_test_support(admin.clone()),
        queue.clone(),
        region,
        tokio::runtime::Handle::current(),
        resolver,
    );
    let claimed = adapter.claim_for_labels(
        "worker",
        &["linux".to_string()],
        &[TrustTier::Trusted],
        &Region(region.into()),
        1000,
        CI_RUNNER_LEASE_TTL_SECS,
    );
    assert!(
        claimed.is_none(),
        "an unresolved spec makes the claim a no-op (None) — never a launch of an unresolved job"
    );
    // the row WAS leased by the claim (the durable claim leases before the resolve) — left for the reaper.
    let state: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE job_id = $1")
        .bind(uid("nospec-job"))
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        state, "leased",
        "the unresolved row stays leased (the reaper re-queues it; never launched)"
    );

    drop_schema(&admin, &schema).await;
    println!("[CT-004d.1] PASS: a leased row with no durable spec resolves fail-closed — no launch, reaped");
}

// ═══════════════ 5. the wired lease TTL exceeds the max job timeout (the double-run fix) ══════════

#[test]
fn the_wired_lease_ttl_exceeds_the_max_job_timeout() {
    assert!(
        CI_RUNNER_LEASE_TTL_SECS > MAX_JOB_TIMEOUT_SECS as i64,
        "the wired runner lease TTL ({CI_RUNNER_LEASE_TTL_SECS}) must exceed the max job timeout \
         ({MAX_JOB_TIMEOUT_SECS}) so a job never lapses its lease mid-run (no reaper double-run)"
    );
}
