//! **CT-004d.2 CULMINATION — a pushed CI trigger runs a REAL pipeline END-TO-END, in ONE process,
//! against live PG + real `runsc`.** This is the payoff: the durable `ci_run` a push armed (CT-004b)
//! is started as a parked `ci.pipeline` run (chunk 3), its stage is dispatched through the DURABLE
//! `job_queue`+`ci_job_spec` (chunk 5's `DurableJobRunner` — trust_tier/region forwarded UNCHANGED),
//! the CT-004c.2 runner CLAIMS it + EXECUTES it in a REAL gVisor (`runsc`) guest + reports `job.done`,
//! and the parked run WAKES (chunk 2's driver + the `CiPipelineReporter` verdict bridge), advances, and
//! COMPLETES — the X-1 producer emits the green check/result reflecting the real guest outcome.
//!
//! What it proves:
//!   1. **END TO END (requires real `runsc`):** a queued `ci_run` → `start_run` → tick dispatches the
//!      stage into the DURABLE queue → a `job_queue` row + its `ci_job_spec` appear → the durable-backed
//!      runner claims it → a REAL `runsc` guest runs the stage command → `job.done` (re-encoded to the
//!      stage-verdict codec) wakes the parked run → the run COMPLETES → `ci.run.succeeded` +
//!      `ci.check.updated{success}` emitted. SKIPS green if `runsc`/rootfs absent; HARD-FAILS under
//!      `MYELIN_REQUIRE_RUNSC=1`.
//!   2. **SECURITY — the trust tier is forwarded UNCHANGED into the durable dispatch:** the `job_queue`
//!      row + the `ci_job_spec` carry the run's stamped `trust_tier` (`trusted`), never widened.
//!
//! Gated behind the `integration` cargo feature. Run against the docker-compose dev stack:
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   export MYELIN_REQUIRE_RUNSC=1
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_ct004d2_culmination -- --nocapture
#![cfg(feature = "integration")]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use myelin_ci_controlplane::{
    ci_job_queue_store, ci_job_spec_store, ci_region_queue_store_test_support,
    ci_run_store_factory, durable_spec_resolver, fixed_command_spec_builder, CheckFacts,
    CiPipelineDriver, CiRunInsert, DurableLeaseAdapter, PipelineRun, PipelineStage,
    ALTER_CI_JOB_SPEC_ADD_STAGE_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL,
    ALTER_JOB_QUEUE_ADD_COMPLETION_DDL, CREATE_CI_JOB_SPEC_DDL, CREATE_CI_RUN_DDL,
    CREATE_FAIR_DEFICIT_DDL, CREATE_JOB_QUEUE_DDL, CREATE_JOB_QUEUE_INDEXES_DDL,
};
use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    resolved_gvisor_rootfs, CompletionClaim, FirehoseSink, ReserveHandle, RunnerAgent, RunnerHooks,
    TerminalReport, TerminalReporter, TrustTier,
};
use myelin_events::OutboxStore;
use myelin_flow::{
    migrations::migrations as flow_migrations, CiStage, DriveOutcome, ExecutorError, MinorUnits,
    RunId, SignalOutcome, JOB_DONE_SIGNAL,
};
use myelin_refs::ArtifactRef;
use myelin_storage::{provider::foundation_migrations, HotTables, PgMigrator};
use myelin_tenancy::{Region, TenantId};
use sqlx::types::Uuid;
use sqlx::{Executor, PgPool};
use tokio::sync::Mutex as AsyncMutex;

static TEST_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

// ─────────────────────────────── PG / schema plumbing (mirrors CT-004c.2) ─────────────────────────

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}
fn schema_name(suffix: &str) -> String {
    format!("ci_ct004d2_{}_{}", std::process::id(), suffix)
}

async fn admin_pool(schema: &str) -> PgPool {
    let schema = schema.to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
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
    PgMigrator::apply(admin, &foundation_migrations())
        .await
        .expect("apply shared durable foundation");
    PgMigrator::apply_validated(
        admin,
        &flow_migrations(),
        &HotTables::declare(["workflow_run"]),
    )
    .await
    .expect("apply durable flow migrations");
    admin
        .execute(CREATE_CI_RUN_DDL)
        .await
        .expect("create ci_run");
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
    for (_name, idx) in CREATE_JOB_QUEUE_INDEXES_DDL {
        let idx = idx.replace("CONCURRENTLY ", "");
        admin.execute(idx.as_str()).await.expect("index");
    }
    admin
        .execute(CREATE_FAIR_DEFICIT_DDL)
        .await
        .expect("create fair_deficit");
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
             absent — CT-004d.2 refuses a VACUOUS green: a real `runsc` guest MUST run the pipeline stage.",
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

fn ok_hooks() -> RunnerHooks {
    RunnerHooks {
        reserve: Box::new(|m| Ok(ReserveHandle(m.reserve_id.clone()))),
        settle: Box::new(|_h, _u| Ok(())),
        attribute: Box::new(|_t| Ok(())),
        isolation_floor: Box::new(|_s| Ok(())),
    }
}

// A digest-pinned image (the runner ignores the reference for the staged local rootfs — the same
// posture the CT-004c.2 real-exec test uses; the guest runs the staged busybox-ish rootfs).
const PINNED_IMAGE: &str =
    "registry.example/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// ═══════════════════════════════ THE END-TO-END PROOF ════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_runs_a_real_pipeline_end_to_end() {
    let _test_guard = TEST_LOCK.lock().await;
    if !require_or_skip("ct004d2-culmination") {
        return;
    }
    let schema = schema_name("e2e");
    let region = "fr-par";
    let tenant = "tenantA";
    let admin = admin_pool(&schema).await;
    create_schema(&admin, &schema).await;

    // ── (CT-004b) the durable `ci_run` a push armed: state=queued, with the pre-minted wf_run_id. ──
    let run_uuid = uid("d2-ci-run").to_string();
    let wf_run_uuid = uid("d2-wf-run").to_string(); // the parked run's id (= the job_queue row's run_id)
    let ci_run_store = ci_run_store_factory(admin.clone());
    ci_run_store
        .insert_ci_run(&CiRunInsert {
            tenant_id: tenant.into(),
            region: region.into(),
            run_id: run_uuid.clone(),
            project_id: uid("d2-project").to_string(),
            pipeline_id: uid("d2-pipeline").to_string(),
            wf_run_id: wf_run_uuid.clone(),
            definition_snapshot: "blake3:d2snapshot".into(),
            trigger_kind: "push".into(),
            trust_tier: "trusted".into(), // the stamped tier — forwarded UNCHANGED into the dispatch
            state: "queued".into(),
            correlation_id: "corr-d2".into(),
            cause_event_id: Some("evt-push-d2".into()),
            repo_ref: Some("myelin/self".into()),
            commit_oid: Some("deadbeefcafe".into()),
            triggered_by: None,
        })
        .await
        .expect("arm the durable ci_run (the CT-004b reserve output)");
    let record = ci_run_store
        .get_ci_run(tenant, region, &run_uuid)
        .await
        .expect("read the armed ci_run")
        .expect("the queued ci_run is durable");
    assert_eq!(record.state, "queued");
    assert_eq!(record.wf_run_id, wf_run_uuid);

    // ── (chunk 2) the pipeline DRIVER over the shared executor; the stage runs `echo` in gVisor. ──
    let stage_target = "pipeline://myelin/self#build";
    let build_spec = fixed_command_spec_builder(
        PINNED_IMAGE,
        vec![
            "sh".into(),
            "-c".into(),
            "echo hello-ct004d2-pipeline; exit 0".into(),
        ],
        60,
    )
    .expect("pinned image");
    let outbox = OutboxStore::new(); // in-memory (test-support) — reads the body's X-1 producer emits
    let driver = CiPipelineDriver::new(
        TenantId(tenant.into()),
        region,
        ci_job_spec_store(admin.clone()),
        tokio::runtime::Handle::current(),
        build_spec,
        outbox,
    );

    // ── (chunk 3) start the parked ci.pipeline run under the pre-minted wf_run_id. ──
    let pipeline = PipelineRun {
        stages: vec![PipelineStage::job(CiStage::new(
            "build",
            stage_target,
            MinorUnits(0),
            Some(3600),
        ))],
        contexts: vec!["build".into()],
        facts: CheckFacts {
            repo: record.repo_ref.clone().unwrap(),
            commit_oid: record.commit_oid.clone().unwrap(),
            run_ref: format!("myelin://{tenant}/ci/run/{run_uuid}"),
            run_attempt: 1,
            trust_tier: record.trust_tier.clone(),
            merge_idem_token: format!("merge:{run_uuid}"),
        },
    };
    let run = driver
        .start_run(&record, pipeline, vec!["linux".into()])
        .expect("start the parked ci.pipeline run under the pre-minted wf_run_id");
    assert_eq!(
        run,
        RunId(wf_run_uuid.clone()),
        "the run's id == the pre-minted wf_run_id"
    );

    // ── DRIVE tick #1: the body dispatches the stage into the DURABLE queue + parks. ──
    let now = now_secs();
    let _ = driver.drive_once(now, "2026-07-17T00:00:00Z");
    assert_eq!(
        driver.run_state(&run).as_deref(),
        Some("waiting"),
        "the pipeline dispatched the stage + parked on job.done"
    );

    // a DURABLE job_queue row appeared (queued), carrying the run's stamped trust_tier UNCHANGED.
    let (jq_run, jq_trust, jq_state): (Uuid, String, String) =
        sqlx::query_as("SELECT run_id, trust_tier, state FROM job_queue WHERE run_id = $1")
            .bind(uid("d2-wf-run"))
            .fetch_one(&admin)
            .await
            .expect("the durable job_queue row the pipeline dispatched");
    assert_eq!(
        jq_run.to_string(),
        wf_run_uuid,
        "the job_queue row targets the parked run"
    );
    assert_eq!(
        jq_trust, "trusted",
        "SECURITY: the run's stamped trust_tier forwarded UNCHANGED"
    );
    assert_eq!(
        jq_state, "queued",
        "the stage job is queued, awaiting a runner claim"
    );
    // its ci_job_spec is resolvable (the spec that EXECUTES) + carries the SAME tier.
    let spec_trust: String =
        sqlx::query_scalar("SELECT spec->>'trust_tier' FROM ci_job_spec WHERE run_id = $1")
            .bind(uid("d2-wf-run"))
            .fetch_one(&admin)
            .await
            .expect("the co-persisted ci_job_spec row");
    assert_eq!(
        spec_trust, "Trusted",
        "the executing spec's tier == the gate tier (no widening)"
    );

    // ── (CT-004c.2) the durable-backed runner CLAIMS the row + EXECUTES it in a REAL runsc guest, and
    //    reports job.done through the driver's CiPipelineReporter (verdict re-encode + wake). ──
    let resolver = durable_spec_resolver(
        ci_job_spec_store(admin.clone()),
        region,
        tokio::runtime::Handle::current(),
    );
    let adapter = DurableLeaseAdapter::new(
        ci_region_queue_store_test_support(admin.clone()),
        ci_job_queue_store(admin.clone()),
        region,
        tokio::runtime::Handle::current(),
        resolver,
    );
    let backend = GvisorBackend::new();
    let firehose = CapturingFirehose::default();
    let reporter = driver.reporter(); // the CiPipelineReporter over the SHARED executor + verdict bridge
    let agent = RunnerAgent::new(
        "d2-worker",
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
    let outcome = agent
        .run_one(now_secs())
        .expect("the runner claims + runs the stage in gVisor + reports job.done");

    let guest = String::from_utf8_lossy(&firehose.captured()).to_string();
    println!("=== CT-004d.2 CULMINATION: REAL runsc guest ran the pipeline stage ===");
    println!(
        "job_id={} run_id={} passed={}",
        outcome.job_id, outcome.run_id, outcome.report.passed
    );
    println!("REAL guest stdout (via firehose) = {guest:?}");
    assert_eq!(
        outcome.run_id, wf_run_uuid,
        "the runner reported to the parked run"
    );
    assert!(
        outcome.report.passed,
        "the REAL runsc guest exited 0 → derived passed=true"
    );
    assert!(
        guest.contains("hello-ct004d2-pipeline"),
        "the REAL runsc guest ran the stage command (proves real exec). got: {guest:?}"
    );

    // ── DRIVE tick #2..N: the parked run WAKES, consumes the job.done (verdict pass:build), advances,
    //    and COMPLETES. ──
    let mut completed: Option<Vec<myelin_refs::ArtifactRef>> = None;
    for _ in 0..20 {
        for o in driver.drive_once(now_secs(), "2026-07-17T01:00:00Z") {
            if let DriveOutcome::Completed(refs) = o {
                completed = Some(refs);
            }
        }
        if driver.run_state(&run).as_deref() == Some("completed") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    assert_eq!(
        driver.run_state(&run).as_deref(),
        Some("completed"),
        "the parked ci.pipeline run WOKE on the real guest's job.done and COMPLETED"
    );
    assert_eq!(
        completed.as_deref(),
        Some(&[myelin_refs::ArtifactRef("outcome:succeeded:1".into())][..]),
        "the pipeline verdict reflects the real green guest (1 stage succeeded)"
    );

    // job.done buffered EXACTLY ONCE (the exactly-once wake).
    assert_eq!(
        driver
            .executor()
            .signals()
            .count_for_run(&TenantId(tenant.into()), &wf_run_uuid),
        1,
        "the engine buffered EXACTLY ONE job.done (exactly-once wake)"
    );

    // the durable lease was SETTLED (the job_queue row moved to terminal).
    let jq_final: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE run_id = $1")
        .bind(uid("d2-wf-run"))
        .fetch_one(&admin)
        .await
        .expect("read the settled job state");
    assert_eq!(jq_final, "terminal", "the runner settled the durable lease");

    // the X-1 PRODUCER emitted the green check/result reflecting the real guest outcome.
    let rows = driver.outbox().committed_rows();
    let types: Vec<String> = rows.iter().map(|r| r.envelope.type_.0.clone()).collect();
    assert!(
        types.iter().any(|t| t == "ci.run.succeeded"),
        "the pipeline emitted ci.run.succeeded (the run reflects the green guest). emitted: {types:?}"
    );
    assert!(
        types.iter().any(|t| t.contains("check")),
        "the pipeline emitted a terminal ci.check.updated. emitted: {types:?}"
    );

    // sanity: the job.done payload carried the STAGE VERDICT codec (the bridge translated the runner's
    // derived pass), NOT the raw passed marker — this is what let run_ci_pipeline decode the verdict.
    let jd = driver
        .executor()
        .signals()
        .get(
            &TenantId(tenant.into()),
            &wf_run_uuid,
            JOB_DONE_SIGNAL,
            &outcome_idem(&wf_run_uuid),
        )
        .expect("the buffered job.done");
    assert_eq!(
        jd.payload[0],
        myelin_flow::stage_verdict_marker("build", true),
        "the reporter re-encoded the real guest's pass into the stage-verdict codec the body decodes"
    );

    drop_schema(&admin, &schema).await;
    println!(
        "[CT-004d.2] PASS CULMINATION: a queued ci_run → parked ci.pipeline run → durable-queue stage \
         dispatch → REAL runsc guest ran `echo hello-ct004d2-pipeline` (exit 0) → job.done woke the \
         parked run → run COMPLETED → ci.run.succeeded emitted. A push runs a real pipeline."
    );
}

/// The engine-minted dispatch `idem_token` for the single build stage (command position 0): the
/// `<run_id>/ci.pipeline:0/job` shape `job_idem_token` mints (the runner echoes it on job.done).
fn outcome_idem(run_id: &str) -> String {
    myelin_flow::job_idem_token(run_id, "ci.pipeline:0")
}

// ═══════════════ CLAIM-BOUND COMPLETION — the adversarial proofs (no runsc needed) ═══════════════
// These drive the reporter's prove-and-consume CAS directly (seed the claim, no guest exec), proving
// dispatch EXISTENCE is not claim OWNERSHIP, that a stale/re-claimed generation is refused, that the
// receipt makes a redelivery idempotent, and that a flipped-verdict replay is refused.

/// Arm a queued `ci_run`, start its parked `ci.pipeline` run, and drive ONE tick so its build stage is
/// dispatched into the durable queue + `ci_job_spec` (with the durable stage). Returns the parked run,
/// the dispatched `job_id`, and the dispatch `idem_token` the reporter verifies against.
async fn arm_and_dispatch(
    admin: &PgPool,
    ci_run_store: &myelin_ci_controlplane::CiRunStore,
    driver: &CiPipelineDriver,
    tenant: &str,
    region: &str,
    seed: &str,
) -> (RunId, String, String) {
    let run_uuid = uid(&format!("{seed}-ci-run")).to_string();
    let wf_run_uuid = uid(&format!("{seed}-wf-run")).to_string();
    ci_run_store
        .insert_ci_run(&CiRunInsert {
            tenant_id: tenant.into(),
            region: region.into(),
            run_id: run_uuid.clone(),
            project_id: uid(&format!("{seed}-project")).to_string(),
            pipeline_id: uid(&format!("{seed}-pipeline")).to_string(),
            wf_run_id: wf_run_uuid.clone(),
            definition_snapshot: "blake3:cbcsnapshot".into(),
            trigger_kind: "push".into(),
            trust_tier: "trusted".into(),
            state: "queued".into(),
            correlation_id: format!("corr-{seed}"),
            cause_event_id: Some(format!("evt-{seed}")),
            repo_ref: Some("myelin/self".into()),
            commit_oid: Some("deadbeefcafe".into()),
            triggered_by: None,
        })
        .await
        .expect("arm the queued ci_run");
    let record = ci_run_store
        .get_ci_run(tenant, region, &run_uuid)
        .await
        .expect("read ci_run")
        .expect("queued ci_run is durable");
    let pipeline = PipelineRun {
        stages: vec![PipelineStage::job(CiStage::new(
            "build",
            "pipeline://myelin/self#build",
            MinorUnits(0),
            Some(3600),
        ))],
        contexts: vec!["build".into()],
        facts: CheckFacts {
            repo: record.repo_ref.clone().unwrap(),
            commit_oid: record.commit_oid.clone().unwrap(),
            run_ref: format!("myelin://{tenant}/ci/run/{run_uuid}"),
            run_attempt: 1,
            trust_tier: record.trust_tier.clone(),
            merge_idem_token: format!("merge:{run_uuid}"),
        },
    };
    let run = driver
        .start_run(&record, pipeline, vec!["linux".into()])
        .expect("start the parked ci.pipeline run");
    let _ = driver.drive_once(now_secs(), "2026-07-17T00:00:00Z");
    assert_eq!(
        driver.run_state(&run).as_deref(),
        Some("waiting"),
        "dispatched + parked"
    );
    let (job_id, idem): (Uuid, String) =
        sqlx::query_as("SELECT job_id, idem_token FROM ci_job_spec WHERE run_id = $1")
            .bind(uid(&format!("{seed}-wf-run")))
            .fetch_one(admin)
            .await
            .expect("the dispatched ci_job_spec row");
    (run, job_id.to_string(), idem)
}

/// Simulate a runner claim on the dispatched job: move it `leased` with the given generation.
async fn claim_job(admin: &PgPool, wf_run: &str, owner: &str, epoch: i64) -> String {
    let nonce = uid(&format!("{wf_run}:{owner}:{epoch}:claim"));
    sqlx::query(
        "UPDATE job_queue SET state='leased', lease_owner=$2, lease_epoch=$3, claim_nonce=$4 WHERE run_id=$1",
    )
    .bind(Uuid::parse_str(wf_run).unwrap())
    .bind(owner)
    .bind(epoch)
    .bind(nonce)
    .execute(admin)
    .await
    .expect("simulate the runner claim");
    nonce.to_string()
}

fn completion_claim(
    tenant: &TenantId,
    run: &RunId,
    job_id: &str,
    idem_token: &str,
    owner: &str,
    epoch: i64,
    nonce: &str,
) -> CompletionClaim {
    CompletionClaim {
        tenant: tenant.clone(),
        run: run.clone(),
        job_id: job_id.into(),
        idem_token: idem_token.into(),
        lease_owner: owner.into(),
        lease_epoch: epoch,
        claim_nonce: nonce.into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claim_bound_completion_refuses_forged_stale_and_flipped_verdict() {
    let _test_guard = TEST_LOCK.lock().await;
    let schema = schema_name("claim");
    let region = "fr-par";
    let tenant = "tenantA";
    let admin = admin_pool(&schema).await;
    create_schema(&admin, &schema).await;
    let ci_run_store = ci_run_store_factory(admin.clone());
    let build_spec =
        fixed_command_spec_builder(PINNED_IMAGE, vec!["true".into()], 60).expect("pinned image");
    let driver = CiPipelineDriver::new(
        TenantId(tenant.into()),
        region,
        ci_job_spec_store(admin.clone()),
        tokio::runtime::Handle::current(),
        build_spec,
        OutboxStore::new(),
    );
    let tid = TenantId(tenant.into());
    let pass = || TerminalReport {
        passed: true,
        result_refs: vec![],
    };

    // ── MAIN run: dispatch a stage. ──
    let (run, job_id, idem) =
        arm_and_dispatch(&admin, &ci_run_store, &driver, tenant, region, "cbc-main").await;
    let wf_run = run.0.clone();
    let reporter = driver.reporter();

    // (A) DISPATCH EXISTENCE ≠ OWNERSHIP: a caller who reconstructs the exact valid (run, job_id, idem)
    // tuple (all derivable from the public token grammar) but holds NO claim is REFUSED — the row is
    // still `queued`, so no live claim generation matches, and nothing is signalled.
    let forged_nonce = Uuid::nil().to_string();
    let forged = reporter.report_done(
        &completion_claim(
            &tid,
            &run,
            &job_id,
            &idem,
            "worker-forger",
            1,
            &forged_nonce,
        ),
        &pass(),
    );
    assert!(
        matches!(forged, Err(ExecutorError::InvalidInput(_))),
        "a valid tuple with no claim is refused, got {forged:?}"
    );
    assert_eq!(
        driver.executor().signals().count_for_run(&tid, &wf_run),
        0,
        "a refused forgery signals nothing"
    );

    // ── the runner claims at generation (worker-real, epoch 1). ──
    let nonce = claim_job(&admin, &wf_run, "worker-real", 1).await;

    let wrong_nonce = uid("wrong-completion-claim").to_string();
    let wrong_nonce_result = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &wrong_nonce),
        &pass(),
    );
    assert!(
        matches!(wrong_nonce_result, Err(ExecutorError::InvalidInput(_))),
        "the unguessable claim nonce is required"
    );

    let invalid_ref = TerminalReport {
        passed: true,
        result_refs: vec![ArtifactRef("myelin://acme/ci/run/deep/not-scoped".into())],
    };
    let invalid = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &nonce),
        &invalid_ref,
    );
    assert!(
        invalid.is_err(),
        "invalid refs fail the typed signal contract"
    );
    let before_success: (String, Option<String>) =
        sqlx::query_as("SELECT state, completion_receipt FROM job_queue WHERE run_id = $1")
            .bind(Uuid::parse_str(&wf_run).unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(
        before_success,
        ("leased".into(), None),
        "typed-signal failure rolls back claim consumption"
    );

    // (B) STALE GENERATION: a worker whose lease was reaped and re-claimed elsewhere presents a LOWER
    // epoch — refused (the CAS matches no live claim at that generation).
    let stale = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 0, &nonce),
        &pass(),
    );
    assert!(
        matches!(stale, Err(ExecutorError::InvalidInput(_))),
        "a stale epoch is refused, got {stale:?}"
    );
    // a DIFFERENT owner at the correct epoch is refused too.
    let wrong_owner = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-evil", 1, &nonce),
        &pass(),
    );
    assert!(
        matches!(wrong_owner, Err(ExecutorError::InvalidInput(_))),
        "a wrong lease owner is refused, got {wrong_owner:?}"
    );
    assert_eq!(
        driver.executor().signals().count_for_run(&tid, &wf_run),
        0,
        "no refused delivery signalled"
    );

    // (C) THE OWNING CLAIM consumes the claim + signals the verdict.
    let ok = reporter
        .report_done(
            &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &nonce),
            &pass(),
        )
        .expect("the owning claim consumes + signals");
    assert_eq!(ok, SignalOutcome::Buffered);
    let (state, receipt): (String, Option<String>) =
        sqlx::query_as("SELECT state, completion_receipt FROM job_queue WHERE run_id = $1")
            .bind(Uuid::parse_str(&wf_run).unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(state, "terminal", "the claim was consumed to terminal");
    assert!(receipt.is_some(), "a completion receipt was recorded");

    // (D) RECEIPT-BASED RETRY: the identical completion redelivered is idempotent (AlreadyConsumed →
    // re-signal, the engine dedups to Duplicate) — a signal that failed after the CAS retries safely.
    let retry = reporter
        .report_done(
            &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &nonce),
            &pass(),
        )
        .expect("an identical redelivery is idempotent");
    assert_eq!(
        retry,
        SignalOutcome::Duplicate,
        "the redelivery is a wake-once no-op"
    );

    // (E) FLIPPED-VERDICT REPLAY: the SAME valid claim but `passed=false` computes a DIFFERENT receipt
    // than the recorded one — REFUSED (the receipt binds the verdict; a consumed pass cannot be replayed
    // as a fail).
    let flipped = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &nonce),
        &TerminalReport {
            passed: false,
            result_refs: vec![],
        },
    );
    assert!(
        matches!(flipped, Err(ExecutorError::InvalidInput(_))),
        "a flipped-verdict replay with a valid receipt is refused, got {flipped:?}"
    );
    let divergent_refs = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &nonce),
        &TerminalReport {
            passed: true,
            result_refs: vec![ArtifactRef("myelin://acme/ci/artifact/build-output".into())],
        },
    );
    assert!(
        matches!(divergent_refs, Err(ExecutorError::InvalidInput(_))),
        "an ordered result-ref divergence changes the receipt and is refused"
    );

    // ── FAIL-CLOSED run: a dispatched stage whose durable spec stage is NULL. ──
    let (run2, job2, idem2) =
        arm_and_dispatch(&admin, &ci_run_store, &driver, tenant, region, "cbc-quar").await;
    let wf_run2 = run2.0.clone();
    sqlx::query("UPDATE ci_job_spec SET stage = NULL WHERE run_id = $1")
        .bind(uid("cbc-quar-wf-run"))
        .execute(&admin)
        .await
        .expect("null the stage (simulate a pre-rewire historical row)");
    let nonce2 = claim_job(&admin, &wf_run2, "worker-real", 1).await;
    let refused = reporter.report_done(
        &completion_claim(&tid, &run2, &job2, &idem2, "worker-real", 1, &nonce2),
        &pass(),
    );
    assert!(refused.is_err(), "a NULL-stage completion fails closed");
    assert_eq!(
        driver.executor().signals().count_for_run(&tid, &wf_run2),
        0,
        "a refused NULL-stage job signals no verdict"
    );
    let q_state: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE run_id = $1")
        .bind(Uuid::parse_str(&wf_run2).unwrap())
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        q_state, "leased",
        "the atomic transaction leaves the claim live for operator-visible recovery"
    );

    drop_schema(&admin, &schema).await;
}
