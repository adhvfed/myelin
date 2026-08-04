//! **CT-004d.2 CULMINATION — a pushed CI trigger runs a REAL pipeline END-TO-END, in ONE process,
//! against live PG + real `runsc`.** This is the payoff: the durable `ci_run` a push armed (CT-004b)
//! is started as a parked `ci.pipeline` run (chunk 3), its stage is dispatched through the DURABLE
//! `job_queue`+`ci_job_spec` (chunk 5's `DurableJobRunner` — trust_tier/region forwarded UNCHANGED),
//! the CT-004c.2 runner CLAIMS it + EXECUTES it in a REAL gVisor (`runsc`) guest + reports `job.done`,
//! and the parked run WAKES through PostgreSQL (`CiPipelineReporter` + reconstructed `PgFlowWorker`),
//! advances, and COMPLETES — the X-1 producer emits the green check/result durably.
//!
//! What it proves:
//!   1. **END TO END (requires real `runsc`):** a queued `ci_run` → durable workflow start → drive
//!      dispatches the
//!      stage into the DURABLE queue → a `job_queue` row + its `ci_job_spec` appear → the durable-backed
//!      runner claims it → a REAL `runsc` guest runs the stage command → `job.done` (re-encoded to the
//!      stage-verdict codec) wakes a newly reconstructed PostgreSQL worker → the run COMPLETES →
//!      `ci.run.succeeded` + `ci.check.updated{success}` commit to the durable outbox. SKIPS green if
//!      `runsc`/rootfs absent; HARD-FAILS under
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

mod common;

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use common::with_schema_cleanup;

use myelin_ci_controlplane::{
    ci_job_queue_store, ci_job_spec_store, ci_region_queue_store_test_support,
    ci_run_store_factory, durable_spec_resolver_test_support, fixed_command_spec_builder,
    CheckFacts, CiJobLaunchClaim, CiJobQueueStore, CiJobTokenIssueError, CiJobTokenIssuer,
    CiJobTokenRequest, CiPipelineDriver, CiPipelineReporter, CiPipelineReporterFactory,
    CiPipelineReporterFactoryError, CiPipelineReporterRouter, CiRunInsert, DurableJobRunner,
    DurableLeaseAdapter, JobScheduleTerms, Lane, PipelineRun, PipelineStage,
    ALTER_CI_JOB_SPEC_ADD_STAGE_DDL, ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL,
    ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL, ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL, ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
    ALTER_JOB_QUEUE_ADD_RESERVATION_WRITE_VERSION_DDL, ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL,
    CREATE_CI_JOB_DDL, CREATE_CI_JOB_SPEC_DDL,
    CREATE_CI_RUN_DDL, CREATE_FAIR_DEFICIT_DDL, CREATE_JOB_QUEUE_DDL, CREATE_JOB_QUEUE_INDEXES_DDL,
};
use myelin_ci_sandbox::asset_registry::GvisorAssetRegistry;
use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    resolved_gvisor_rootfs, CompletionClaim, FirehoseSink, HookError, ImageRef, LaunchOwnership,
    LaunchPermit, ReserveHandle, ResourceUsage, RunTokenAuthorizationContext, RunTokenCredential,
    RunnerAgent, RunnerHooks, TerminalReport, TerminalReporter, TrustTier,
    LINUX_SMALL_V1_ROOTFS_SHA256,
};
use myelin_events::{Actor, IdMinter, MonotonicMinter, OutboxStore};
use myelin_flow::{
    migrations::migrations as flow_migrations, partition_for_run_id, CiStage, DriveOutcome,
    DurableExecutor, ExecutorError, MicroUsd, PgFlowExecutor, PgFlowWorker, PgRunOnceOutcome,
    PgWorkerScope, RunId, SignalOutcome, StartSpec, CI_PIPELINE_WF_TYPE, JOB_DONE_SIGNAL,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_refs::ArtifactRef;
use myelin_storage::{provider::foundation_migrations, HotTables, PgMigrator};
use myelin_tenancy::{Region, TenantId};
use sqlx::types::Uuid;
use sqlx::{Executor, PgPool};
use tokio::sync::Mutex as AsyncMutex;

static TEST_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

fn unused_secret_terminal_reporter(region: &str) -> CiPipelineReporterRouter {
    let factory: CiPipelineReporterFactory =
        Arc::new(|_, _| Err(CiPipelineReporterFactoryError));
    CiPipelineReporterRouter::new(Region(region.into()), factory).unwrap()
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
                format!("culmination-bearer:{}", request.claim_nonce),
                format!("culmination:{}:{}", request.job_id, request.claim_nonce),
                300,
            )
            .map_err(|error| CiJobTokenIssueError(error.to_string()))
        })
    }
}

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
        .execute(ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL)
        .await
        .expect("add ci_run causal provenance");
    admin
        .execute(ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL)
        .await
        .expect("add ci_run concurrency identity");
    admin
        .execute(ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL)
        .await
        .expect("add ci_run PR ordering authority");
    // BUG FIX (investigation, 2026-07-25): this per-pid schema never created its own `ci_job`
    // table. `AUTHORIZE_JOB_LAUNCH_QUERY` (the launch CAS every real claim crosses) requires a
    // matching `ci_job` row to also cross `queued`/`leased` -> `running` in the SAME statement —
    // without this table, `search_path`'s `public` fallback silently resolved every `ci_job`
    // reference to the SHARED dev database's `public.ci_job` (leftover rows from unrelated runs),
    // which never has a row for this schema's `job_id`. The CAS therefore matched zero rows EVERY
    // time (100% reproducible, not a timing/race artifact — confirmed by 5/5 identical failures
    // within ~1.3s each, far under any claim TTL). Creating the real per-schema table here — and
    // seeding the matching row at dispatch time below — is what a real push's starter
    // (`pg_pipeline_starter.rs`'s `materialize_ci_jobs`) already does for every run.
    admin
        .execute(CREATE_CI_JOB_DDL)
        .await
        .expect("create ci_job (the starter-owned DAG surface the launch CAS crosses)");
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

fn bridge<F: Future>(rt: &tokio::runtime::Handle, future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(future)),
        Err(_) => rt.block_on(future),
    }
}

/// Historical culmination support still uses a test credential issuer, but completion must cross
/// the same durable `leased` -> `running` CAS as production. The permit defers that CAS until the
/// real gVisor backend has armed its child launch gate.
fn durable_launch_hooks(queue_store: CiJobQueueStore, rt: tokio::runtime::Handle) -> RunnerHooks {
    common::with_stage_b_compute_admission_for_legacy_runsc_test(
        RunnerHooks::new_with_launch_fence(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(move |spec| {
            let RunTokenAuthorizationContext::CiJob(context) = spec
                .run_token_authorization
                .clone()
                .ok_or_else(|| HookError("test launch is missing durable claim facts".into()))?;
            let claim = CiJobLaunchClaim {
                tenant_id: context.tenant_id,
                region: context.region,
                wf_run_id: context.wf_run_id,
                job_id: context.job_id,
                lease_owner: context.lease_owner,
                lease_epoch: context.lease_epoch,
                claim_nonce: context.claim_nonce,
                claim_started_at_epoch_secs: context.claim_started_at_epoch_secs,
                claim_expires_at_epoch_secs: context.claim_expires_at_epoch_secs,
            };
            let queue_store = queue_store.clone();
            let rt = rt.clone();
            Ok(LaunchPermit::retained(move || {
                let authorized =
                    bridge(&rt, queue_store.authorize_launch(&claim)).map_err(|error| {
                        HookError(format!("authorize durable test launch: {error}"))
                    })?;
                if !authorized {
                    return Err(HookError(
                        "durable test launch claim was no longer live".into(),
                    ));
                }
                Ok(LaunchOwnership::immediate())
            }))
        }),
            Box::new(|_s| Ok(())),
        ),
    )
}

// CT-007 gate 2/4: `spec.image` is now the real launch authority — this is the REAL,
// already-founder-pipeline-pinned `linux-small-v1` image (`.myelin/ci.toml`'s own pin), registered
// (see `test_registry` below) against the SAME base rootfs the founder pipeline actually runs. This
// used to be a fabricated placeholder the runner ignored entirely (the exact gap CT-007 gate 2/4
// closes); it is now a genuinely verifiable pin.
fn pinned_image() -> ImageRef {
    ImageRef::pinned(format!(
        "myelin.local/linux-small-v1-rootfs@sha256:{LINUX_SMALL_V1_ROOTFS_SHA256}"
    ))
    .unwrap()
}

fn test_registry() -> std::sync::Arc<GvisorAssetRegistry> {
    std::sync::Arc::new(
        GvisorAssetRegistry::from_bindings(vec![
            myelin_ci_sandbox::asset_registry::RootfsAssetBinding {
                image: pinned_image(),
                rootfs: resolved_gvisor_rootfs(),
            },
        ])
        .expect("the pinned rootfs binding verifies"),
    )
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

const TEST_CI_BODY_HASH: &str = "blake3:test-only-pinned-ci-pipeline-v1";

/// Build the restartable PostgreSQL worker used by this culmination proof. The captured plan is
/// deliberately test-only: production must derive this data from the future immutable drive-input
/// manifest, never from process memory.
fn pg_ci_test_worker(
    admin: &PgPool,
    tenant: &str,
    region: &str,
    run_id: &str,
    pipeline: PipelineRun,
    build_spec: myelin_ci_controlplane::StageSpecBuilder,
    worker_name: &str,
) -> PgFlowWorker {
    let tenant_id = TenantId(tenant.into());
    let region_id = Region(region.into());
    let actor = Actor(Principal::new(
        tenant_id.clone(),
        region_id.clone(),
        PrincipalId("ci-pg-worker-test".into()),
        PrincipalKind::Service,
        DataRole::Processor,
        PrincipalStatus::Active,
    ));
    let scope = PgWorkerScope::new(
        tenant_id.clone(),
        region_id,
        partition_for_run_id(run_id),
        worker_name,
        30,
        actor,
        1,
    )
    .expect("valid exact-cell worker scope");
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let mut worker = PgFlowWorker::new(
        admin.clone(),
        tokio::runtime::Handle::current(),
        minter,
        scope,
    );
    let mut terms = JobScheduleTerms::new(
        tenant,
        region,
        run_id,
        Lane::Interactive,
        TrustTier::Trusted,
        tenant,
    );
    terms.labels = vec!["linux".into()];
    let runner = DurableJobRunner::new(
        ci_job_spec_store(admin.clone()),
        tokio::runtime::Handle::current(),
        terms,
        build_spec,
        &pipeline.stages,
    );
    worker
        .register_definition(
            CI_PIPELINE_WF_TYPE,
            1,
            TEST_CI_BODY_HASH,
            move |_claimed, ctx| {
                let verdict = myelin_ci_controlplane::ci_pipeline::run_ci_pipeline_body(
                    ctx, &pipeline, &runner,
                )
                .map_err(|error| format!("{error:?}"))?;
                Ok(match verdict {
                    myelin_ci_controlplane::RunVerdict::Succeeded { stages_completed } => {
                        vec![ArtifactRef(format!("outcome:succeeded:{stages_completed}"))]
                    }
                    myelin_ci_controlplane::RunVerdict::Failed { stage } => {
                        vec![ArtifactRef(format!("outcome:failed:{stage}"))]
                    }
                    myelin_ci_controlplane::RunVerdict::Rejected { stage } => {
                        vec![ArtifactRef(format!("outcome:rejected:{stage}"))]
                    }
                    myelin_ci_controlplane::RunVerdict::Parked => vec![],
                })
            },
        )
        .expect("register the pinned test-only CI body");
    worker
}

// ═══════════════════════════════ THE END-TO-END PROOF ════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_runs_a_real_pipeline_end_to_end() {
    let _test_guard = TEST_LOCK.lock().await;
    if !require_or_skip("ct004d2-culmination") {
        return;
    }
    let schema = schema_name("e2e");
    // A cleanup-dedicated pool: `with_schema_cleanup` unconditionally drops `schema` through THIS
    // pool once the test body (success, assertion failure, or panic) finishes, so the schema never
    // outlives this test regardless of outcome (previously it was ONLY dropped at the START of this
    // same test's NEXT run, letting orphaned schemas accumulate on this shared dev Postgres).
    let cleanup_admin = admin_pool(&schema).await;
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_admin, &schema_for_cleanup, move || async move {
    let region = "fr-par";
    let tenant = "tenant-a";
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
            concurrency_group: None,
            pr_head_generation: None,
            trust_tier: "trusted".into(), // the stamped tier — forwarded UNCHANGED into the dispatch
            state: "queued".into(),
            correlation_id: "corr-d2".into(),
            cause_event_id: Some("evt-push-d2".into()),
            cause_depth: 0,
            caused_by: None,
            repo_ref: Some(format!("myelin://{tenant}/git/repo/myelin-self")),
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

    // ── Test-only pinned body input; the stage runs `echo` in gVisor. ──
    let stage_target = "pipeline://myelin/self#build";
    let build_spec = fixed_command_spec_builder(
        &pinned_image().reference,
        vec![
            "sh".into(),
            "-c".into(),
            "echo hello-ct004d2-pipeline; exit 0".into(),
        ],
        60,
    )
    .expect("pinned image");
    let pipeline = PipelineRun {
        stages: vec![PipelineStage::job(CiStage::new(
            "build",
            stage_target,
            MicroUsd(0),
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
    let worker = pg_ci_test_worker(
        &admin,
        tenant,
        region,
        &wf_run_uuid,
        pipeline.clone(),
        build_spec.clone(),
        "d2-flow-worker-before-restart",
    );
    let run = tokio::task::block_in_place(|| {
        worker.executor().start_with_id(
            StartSpec {
                wf_type: CI_PIPELINE_WF_TYPE.into(),
                input: vec![],
                budget: None,
                idem_key: format!("ci:{run_uuid}"),
            },
            Some(RunId(wf_run_uuid.clone())),
        )
    })
    .expect("start the PostgreSQL ci.pipeline run under the pre-minted wf_run_id");
    assert_eq!(
        run,
        RunId(wf_run_uuid.clone()),
        "the run's id == the pre-minted wf_run_id"
    );
    // This culmination test begins at the production starter's durable output but constructs the
    // worker directly so it can pin the one-stage body. Mirror the starter's queued -> running
    // transition explicitly; the dedicated starter integration proves the transition and workflow
    // start are one transaction. Leaving this row queued would make the production claim correctly
    // refuse the otherwise-active workflow and turn every downstream assertion into a false proof.
    let activated: u64 = sqlx::query(
        "UPDATE ci_run SET state = 'running'
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid
           AND wf_run_id = $4::uuid AND state = 'queued'",
    )
    .bind(tenant)
    .bind(region)
    .bind(&run_uuid)
    .bind(&wf_run_uuid)
    .execute(&admin)
    .await
    .expect("represent the production starter's active ci_run output")
    .rows_affected();
    assert_eq!(activated, 1, "exactly the armed CI run becomes active");

    // ── DRIVE #1: PgFlowWorker dispatches the stage durably and parks. ──
    let now = now_secs();
    let first = worker
        .run_once(now, "2026-07-17T00:00:00Z")
        .await
        .expect("first PostgreSQL drive");
    assert!(matches!(
        first,
        PgRunOnceOutcome::Driven {
            outcome: DriveOutcome::Waiting,
            ..
        }
    ));
    let durable_state: String =
        sqlx::query_scalar("SELECT state FROM workflow_run WHERE run_id = $1")
            .bind(&wf_run_uuid)
            .fetch_one(&admin)
            .await
            .expect("read parked workflow state");
    assert_eq!(
        durable_state, "waiting",
        "the pipeline dispatched the stage + parked on job.done"
    );

    // Destroy every process-local drive object, then rebuild from PostgreSQL with the same immutable
    // definition identity. The second drive below therefore proves journal/signal restart recovery.
    drop(worker);
    let worker = pg_ci_test_worker(
        &admin,
        tenant,
        region,
        &wf_run_uuid,
        pipeline,
        build_spec,
        "d2-flow-worker-after-restart",
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
        sqlx::query_scalar("SELECT spec->'spec'->>'trust_tier' FROM ci_job_spec WHERE run_id = $1")
            .bind(uid("d2-wf-run"))
            .fetch_one(&admin)
            .await
            .expect("the co-persisted ci_job_spec row");
    assert_eq!(
        spec_trust, "Trusted",
        "the executing spec's tier == the gate tier (no widening)"
    );

    // Mirror the starter's `materialize_ci_jobs` (`pg_pipeline_starter.rs`): a real push-triggered
    // run has its whole `ci_job` DAG row set (state='queued') materialized at arm time, BEFORE any
    // stage dispatch. This test drives the pipeline directly (see the note above), so it must seed
    // the single `build` job's `ci_job` row itself — `AUTHORIZE_JOB_LAUNCH_QUERY`'s launch CAS
    // requires a matching `ci_job` row to cross `queued`/`leased` -> `running` in the SAME
    // statement, and without it the CAS deterministically matches zero rows (see the `create_schema`
    // comment for the full diagnosis).
    let dispatched_job_id: Uuid =
        sqlx::query_scalar("SELECT job_id FROM job_queue WHERE run_id = $1")
            .bind(uid("d2-wf-run"))
            .fetch_one(&admin)
            .await
            .expect("the dispatched job_queue row's job_id");
    // `ci_job.run_id` FKs to `ci_run.run_id` (the CI run id, `run_uuid`) — NOT `job_queue.run_id`
    // (which is the workflow's `wf_run_id`). The launch CAS itself only matches `ci_job` on
    // `(tenant_id, region, job_id)`, so only `job_id` must line up with the dispatched row.
    sqlx::query(
        "INSERT INTO ci_job (tenant_id, region, job_id, run_id, stage, name, needs, spec_ref, \
         state, attempt) \
         VALUES ($1, $2, $3, $4::uuid, 'build', 'build', '{}'::uuid[], $5, 'queued', 1)",
    )
    .bind(tenant)
    .bind(region)
    .bind(dispatched_job_id)
    .bind(&run_uuid)
    .bind("blake3:d2snapshot")
    .execute(&admin)
    .await
    .expect("seed the starter-owned ci_job surface row the launch CAS crosses");

    // ── The durable-backed runner claims + executes in real runsc. The PostgreSQL-only reporter
    //    consumes the exact claim and inserts job.done atomically; it has no FlowExecutor mirror. ──
    let resolver = durable_spec_resolver_test_support(
        ci_job_spec_store(admin.clone()),
        region,
        tokio::runtime::Handle::current(),
        Arc::new(ClaimTokenIssuer),
        myelin_ci_controlplane::unavailable_ci_job_secret_resolver(),
        unused_secret_terminal_reporter(region),
    );
    let adapter = DurableLeaseAdapter::new(
        ci_region_queue_store_test_support(admin.clone()),
        ci_job_queue_store(admin.clone()),
        region,
        tokio::runtime::Handle::current(),
        resolver,
    );
    let backend = GvisorBackend::new(test_registry());
    let firehose = CapturingFirehose::default();
    let reporter_minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let reporter = CiPipelineReporter::new(
        PgFlowExecutor::new(
            admin.clone(),
            tokio::runtime::Handle::current(),
            reporter_minter,
            TenantId(tenant.into()),
            Region(region.into()),
        ),
        ci_job_spec_store(admin.clone()),
        ci_job_queue_store(admin.clone()),
        tokio::runtime::Handle::current(),
        TenantId(tenant.into()),
        region,
    );
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
        durable_launch_hooks(
            ci_job_queue_store(admin.clone()),
            tokio::runtime::Handle::current(),
        ),
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

    // ── DRIVE #2 after reconstruction: consume durable job.done and complete. ──
    let second = worker
        .run_once(now_secs(), "2026-07-17T01:00:00Z")
        .await
        .expect("restart drive consumes durable job.done");
    let completed = match second {
        PgRunOnceOutcome::Driven {
            outcome: DriveOutcome::Completed(refs),
            ..
        } => Some(refs),
        other => panic!("expected completed PostgreSQL drive, got {other:?}"),
    };
    let durable_state: String =
        sqlx::query_scalar("SELECT state FROM workflow_run WHERE run_id = $1")
            .bind(&wf_run_uuid)
            .fetch_one(&admin)
            .await
            .expect("read terminal workflow state");
    assert_eq!(durable_state, "completed");
    assert_eq!(
        completed.as_deref(),
        Some(&[myelin_refs::ArtifactRef("outcome:succeeded:1".into())][..]),
        "the pipeline verdict reflects the real green guest (1 stage succeeded)"
    );

    let signal_count: i64 = sqlx::query_scalar("SELECT count(*) FROM wf_signal WHERE run_id = $1")
        .bind(&wf_run_uuid)
        .fetch_one(&admin)
        .await
        .expect("count durable signals");
    assert_eq!(signal_count, 1, "exactly one durable job.done was buffered");

    // the durable lease was SETTLED (the job_queue row moved to terminal).
    let jq_final: String = sqlx::query_scalar("SELECT state FROM job_queue WHERE run_id = $1")
        .bind(uid("d2-wf-run"))
        .fetch_one(&admin)
        .await
        .expect("read the settled job state");
    assert_eq!(jq_final, "terminal", "the runner settled the durable lease");

    // The X-1 producer events co-committed through PgFlowDriveStore's durable outbox.
    let types: Vec<String> = sqlx::query_scalar(
        "SELECT envelope->>'type_' FROM outbox WHERE envelope->>'type_' LIKE 'ci.%' ORDER BY seq",
    )
    .fetch_all(&admin)
    .await
    .expect("read durable CI outbox events");
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
    let jd: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM wf_signal WHERE run_id = $1 AND signal_name = $2 AND idem_key = $3",
    )
    .bind(&wf_run_uuid)
    .bind(JOB_DONE_SIGNAL)
    .bind(outcome_idem(&wf_run_uuid))
    .fetch_one(&admin)
    .await
    .expect("read the durable job.done");
    assert_eq!(
        jd[0],
        serde_json::Value::String(myelin_flow::stage_verdict_marker("build", true).0),
        "the reporter re-encoded the real guest's pass into the stage-verdict codec the body decodes"
    );

    drop_schema(&admin, &schema).await;
    println!(
        "[CT-004d.2] PASS CULMINATION: a queued ci_run → parked ci.pipeline run → durable-queue stage \
         dispatch → REAL runsc guest ran `echo hello-ct004d2-pipeline` (exit 0) → job.done woke the \
         parked run → run COMPLETED → ci.run.succeeded emitted. A push runs a real pipeline."
    );
    })
    .await;
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
            concurrency_group: None,
            pr_head_generation: None,
            trust_tier: "trusted".into(),
            state: "queued".into(),
            correlation_id: format!("corr-{seed}"),
            cause_event_id: Some(format!("evt-{seed}")),
            cause_depth: 0,
            caused_by: None,
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
            MicroUsd(0),
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
    // Mirror the starter's `materialize_ci_jobs` (see the `create_schema`/`a_push_...` comments):
    // the launch CAS (`AUTHORIZE_JOB_LAUNCH_QUERY`) requires a matching `ci_job` row to cross
    // `queued`/`leased` -> `running` in the SAME statement. `ci_job.run_id` FKs to `ci_run.run_id`
    // (`run_uuid`), not the workflow `wf_run_id` `ci_job_spec`/`job_queue` key on.
    sqlx::query(
        "INSERT INTO ci_job (tenant_id, region, job_id, run_id, stage, name, needs, spec_ref, \
         state, attempt) \
         VALUES ($1, $2, $3, $4::uuid, 'build', 'build', '{}'::uuid[], 'blake3:cbcsnapshot', \
         'queued', 1)",
    )
    .bind(tenant)
    .bind(region)
    .bind(job_id)
    .bind(&run_uuid)
    .execute(admin)
    .await
    .expect("seed the starter-owned ci_job surface row the launch CAS crosses");
    (run, job_id.to_string(), idem)
}

#[derive(Debug)]
struct TestClaimFacts {
    owner: String,
    epoch: i64,
    nonce: String,
    started_at_epoch_secs: i64,
    expires_at_epoch_secs: i64,
}

impl TestClaimFacts {
    fn launch_claim(
        &self,
        tenant: &str,
        region: &str,
        wf_run: &str,
        job_id: &str,
    ) -> CiJobLaunchClaim {
        CiJobLaunchClaim {
            tenant_id: tenant.into(),
            region: region.into(),
            wf_run_id: wf_run.into(),
            job_id: job_id.into(),
            lease_owner: self.owner.clone(),
            lease_epoch: self.epoch,
            claim_nonce: self.nonce.clone(),
            claim_started_at_epoch_secs: self.started_at_epoch_secs,
            claim_expires_at_epoch_secs: self.expires_at_epoch_secs,
        }
    }
}

/// Simulate a runner claim on the dispatched job with complete durable generation facts.
async fn claim_job(admin: &PgPool, wf_run: &str, owner: &str, epoch: i64) -> TestClaimFacts {
    let nonce = uid(&format!("{wf_run}:{owner}:{epoch}:claim"));
    let (nonce, started_at_epoch_secs, expires_at_epoch_secs): (String, i64, i64) = sqlx::query_as(
        "UPDATE job_queue
         SET state = 'leased',
             lease_owner = $2,
             lease_epoch = $3,
             claim_nonce = $4,
             claim_started_at = date_trunc('second', statement_timestamp()),
             claim_expires_at = date_trunc('second', statement_timestamp()) + interval '5 minutes',
             lease_expires = date_trunc('second', statement_timestamp()) + interval '5 minutes'
         WHERE run_id = $1
         RETURNING claim_nonce::text,
                   FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint,
                   FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint",
    )
    .bind(Uuid::parse_str(wf_run).unwrap())
    .bind(owner)
    .bind(epoch)
    .bind(nonce)
    .fetch_one(admin)
    .await
    .expect("simulate the runner claim");
    TestClaimFacts {
        owner: owner.into(),
        epoch,
        nonce,
        started_at_epoch_secs,
        expires_at_epoch_secs,
    }
}

async fn authorize_test_launch(admin: &PgPool, claim: CiJobLaunchClaim) {
    assert!(
        ci_job_queue_store(admin.clone())
            .authorize_launch(&claim)
            .await
            .expect("authorize exact test launch"),
        "the exact live test claim must cross leased -> running"
    );
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
    // See `a_push_runs_a_real_pipeline_end_to_end`'s cleanup comment: this pool unconditionally
    // drops `schema` after the test body finishes, however it finishes.
    let cleanup_admin = admin_pool(&schema).await;
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_admin, &schema_for_cleanup, move || async move {
    let region = "fr-par";
    let tenant = "tenantA";
    let admin = admin_pool(&schema).await;
    create_schema(&admin, &schema).await;
    let ci_run_store = ci_run_store_factory(admin.clone());
    let build_spec = fixed_command_spec_builder(&pinned_image().reference, vec!["true".into()], 60)
        .expect("pinned image");
    let driver = CiPipelineDriver::new(
        TenantId(tenant.into()),
        region,
        ci_job_spec_store(admin.clone()),
        tokio::runtime::Handle::current(),
        build_spec,
        OutboxStore::new(),
    );
    let tid = TenantId(tenant.into());
    let usage = || ResourceUsage {
        cpu_seconds: 17,
        mem_byte_seconds: 4_096,
    };
    let pass = || TerminalReport {
        passed: true,
        timed_out: false,
        usage: usage(),
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
    let claim = claim_job(&admin, &wf_run, "worker-real", 1).await;

    let leased_completion = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &claim.nonce),
        &pass(),
    );
    assert!(
        matches!(leased_completion, Err(ExecutorError::InvalidInput(_))),
        "possession of a merely leased claim cannot invent completed execution"
    );
    authorize_test_launch(&admin, claim.launch_claim(tenant, region, &wf_run, &job_id)).await;

    let wrong_nonce = uid("wrong-completion-claim").to_string();
    let wrong_nonce_result = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &wrong_nonce),
        &pass(),
    );
    assert!(
        matches!(wrong_nonce_result, Err(ExecutorError::InvalidInput(_))),
        "the unguessable claim nonce is required"
    );

    let contradictory = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &claim.nonce),
        &TerminalReport {
            passed: true,
            timed_out: true,
            usage: usage(),
            result_refs: vec![],
        },
    );
    assert!(
        matches!(contradictory, Err(ExecutorError::InvalidInput(_))),
        "a timed-out job cannot forge a passing verdict"
    );

    let invalid_ref = TerminalReport {
        passed: true,
        timed_out: false,
        usage: usage(),
        result_refs: vec![ArtifactRef("myelin://acme/ci/run/deep/not-scoped".into())],
    };
    let invalid = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &claim.nonce),
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
        ("running".into(), None),
        "typed-signal failure rolls back running-claim consumption"
    );

    // (B) STALE GENERATION: a worker whose lease was reaped and re-claimed elsewhere presents a LOWER
    // epoch — refused (the CAS matches no live claim at that generation).
    let stale = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 0, &claim.nonce),
        &pass(),
    );
    assert!(
        matches!(stale, Err(ExecutorError::InvalidInput(_))),
        "a stale epoch is refused, got {stale:?}"
    );
    // a DIFFERENT owner at the correct epoch is refused too.
    let wrong_owner = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-evil", 1, &claim.nonce),
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

    // (C) THE OWNING RUNNING CLAIM consumes the claim + signals the verdict.
    let ok = reporter
        .report_done(
            &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &claim.nonce),
            &pass(),
        )
        .expect("the owning running claim consumes + signals");
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
            &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &claim.nonce),
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
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &claim.nonce),
        &TerminalReport {
            passed: false,
            timed_out: false,
            usage: usage(),
            result_refs: vec![],
        },
    );
    assert!(
        matches!(flipped, Err(ExecutorError::InvalidInput(_))),
        "a flipped-verdict replay with a valid receipt is refused, got {flipped:?}"
    );
    let divergent_refs = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &claim.nonce),
        &TerminalReport {
            passed: true,
            timed_out: false,
            usage: usage(),
            result_refs: vec![ArtifactRef("myelin://acme/ci/artifact/build-output".into())],
        },
    );
    assert!(
        matches!(divergent_refs, Err(ExecutorError::InvalidInput(_))),
        "an ordered result-ref divergence changes the receipt and is refused"
    );
    let divergent_usage = reporter.report_done(
        &completion_claim(&tid, &run, &job_id, &idem, "worker-real", 1, &claim.nonce),
        &TerminalReport {
            passed: true,
            timed_out: false,
            usage: ResourceUsage {
                cpu_seconds: usage().cpu_seconds + 1,
                ..usage()
            },
            result_refs: vec![],
        },
    );
    assert!(
        matches!(divergent_usage, Err(ExecutorError::InvalidInput(_))),
        "an actual-usage divergence changes the receipt and is refused"
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
    let claim2 = claim_job(&admin, &wf_run2, "worker-real", 1).await;
    authorize_test_launch(&admin, claim2.launch_claim(tenant, region, &wf_run2, &job2)).await;
    let refused = reporter.report_done(
        &completion_claim(&tid, &run2, &job2, &idem2, "worker-real", 1, &claim2.nonce),
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
        q_state, "running",
        "the atomic transaction leaves the launched generation live for operator-visible recovery"
    );

    drop_schema(&admin, &schema).await;
    })
    .await;
}
