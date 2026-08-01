//! Live PostgreSQL proof for the canonical, insert-only CI drive-manifest store.
#![cfg(feature = "integration")]

mod common;

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{legacy_streaming_hooks, with_schema_cleanup, LegacyStreamingGvisor};
use myelin_ci_controlplane::{
    ci_job_authorization_context, ci_region_queue_store_test_support,
    ci_runner_cancellation_coordinator, ci_runner_hooks, ci_runner_identity_authorities,
    CiDriveManifestStore, CiDriveManifestV1, CiJobCredentialMinter, CiJobRuntimeAuthorityRequest,
    CiJobTokenIssueError, CiJobTokenIssuer, CiJobTokenRequest, CiManifestLaneV1,
    CiManifestLimitsV1, CiManifestSchedulingV1, CiManifestTrustTierV1, CiManifestWorkspaceV1,
    DurableCiJobLaunchTemplate, GrantedCiJobV1, LockedManifestCiJobTokenIssuer,
    ManifestBoundCiJobTokenAuthority, ALTER_CI_JOB_SPEC_ADD_STAGE_DDL,
    ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL, ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL,
    ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL,
    ALTER_JOB_QUEUE_ADD_COMPLETION_DDL, ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL,
    CI_PIPELINE_WF_TYPE, CREATE_CI_DRIVE_MANIFEST_DDL, CREATE_CI_JOB_DDL, CREATE_CI_JOB_SPEC_DDL,
    CREATE_CI_RUN_DDL, CREATE_FAIR_DEFICIT_DDL, CREATE_JOB_QUEUE_DDL,
};
use myelin_ci_sandbox::asset_registry::GvisorAssetRegistry;
use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    resolved_gvisor_rootfs, EgressPolicy, EnvVar, IdemToken, ImageRef, JobKind, JobSpecTemplate,
    MeterTarget, ResourceLimits, RunTokenCredential, SandboxBackend, SecretRef, TrustTier,
    WorkspaceSpec, LINUX_SMALL_V1_ROOTFS_SHA256,
};
use myelin_config::MyelinConfig;
use myelin_flow::migrations::migrations as flow_migrations;
use myelin_storage::{
    cell_root_durable_migrations, identity_durable_migrations, reserve_settle_durable_migrations,
    HotTables, PgMigrator, SealKey, SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn scoped_url(url: &str, schema: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}options=-csearch_path%3D{schema}%2Cpublic")
}

fn unique_schema() -> String {
    format!(
        "ci_drive_manifest_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

async fn pinned_pool(url: &str, schema: &str) -> PgPool {
    let schema = schema.to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                connection
                    .execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("connect to live PostgreSQL")
}

fn digest(byte: char) -> String {
    format!("blake3:{}", byte.to_string().repeat(64))
}

/// The real, already-founder-pipeline-pinned `linux-small-v1` image (`.myelin/ci.toml`'s own pin).
/// CT-007 gate 2/4 made `spec.image` the real launch authority: ONLY `manifest().jobs[0]` (the
/// "build" job) ever actually reaches `GvisorBackend::launch` in this file (`jobs[1]`/`jobs[2]` are
/// exercised only through `RunnerHooks::reserve`/`attribute`/`release_unused` directly, never through
/// the sandbox backend, so their fabricated placeholder `image` digests are untouched — they never
/// reach the registry). `jobs[0]`'s image is therefore the one that must be genuinely verifiable.
fn linux_small_v1_image() -> ImageRef {
    ImageRef::pinned(format!(
        "myelin.local/linux-small-v1-rootfs@sha256:{LINUX_SMALL_V1_ROOTFS_SHA256}"
    ))
    .unwrap()
}

fn test_registry() -> std::sync::Arc<GvisorAssetRegistry> {
    std::sync::Arc::new(
        GvisorAssetRegistry::from_bindings(vec![
            myelin_ci_sandbox::asset_registry::RootfsAssetBinding {
                image: linux_small_v1_image(),
                rootfs: resolved_gvisor_rootfs(),
            },
        ])
        .expect("the base linux-small-v1 rootfs binding verifies"),
    )
}

const PROJECT_ID: &str = "44444444-4444-8444-8444-444444444444";
const PRIMARY_JOB_ID: &str = "33333333-3333-8333-8333-333333333333";
const REFUSED_JOB_ID: &str = "66666666-6666-8666-8666-666666666666";
const CRASH_JOB_ID: &str = "aaaaaaaa-aaaa-8aaa-aaaa-aaaaaaaaaaaa";
const REPO_REF: &str = "myelin://manifest-live/git/repo/core";
const COMMIT_OID: &str = "deadbeef00deadbeef00deadbeef00deadbeef00";

fn checkout_scope() -> myelin_ci_sandbox::CheckoutAuthorizationScope {
    myelin_ci_sandbox::derive_checkout_authorization_scope(
        JobKind::Ci,
        &WorkspaceSpec {
            repo_ref: Some(REPO_REF.into()),
            commit: Some(COMMIT_OID.into()),
        },
    )
    .unwrap()
    .expect("a real repo_ref + commit pair must derive Some(scope)")
}

fn reserve_handle(job_id: &str, digest_byte: char) -> String {
    format!(
        "ci-reserve:v1:22222222-2222-8222-8222-222222222222:{}:{job_id}:{}",
        digest_byte.to_string().repeat(64),
        digest_byte.to_string().repeat(64)
    )
}

fn authority() -> CiJobRuntimeAuthorityRequest {
    CiJobRuntimeAuthorityRequest {
        tenant_id: "manifest-live".into(),
        region: "fr-par".into(),
        ci_run_id: "22222222-2222-8222-8222-222222222222".into(),
        wf_run_id: "11111111-1111-8111-8111-111111111111".into(),
        project_id: PROJECT_ID.into(),
        job_id: PRIMARY_JOB_ID.into(),
        stage: "build".into(),
        concrete_name: "build".into(),
        trigger_kind: "push".into(),
        trust_tier: "trusted".into(),
        source_snapshot_digest: digest('a'),
        workflow_definition_version: 3,
        workflow_code_hash: digest('c'),
        policy_revision: "linux-small-v1:1".into(),
        limits: CiManifestLimitsV1 {
            cpu_millis: 1_000,
            mem_bytes: 1_073_741_824,
            disk_bytes: 2_147_483_648,
            pids_max: 128,
            timeout_secs: 600,
        },
        reserve_id: Some(reserve_handle(PRIMARY_JOB_ID, '1')),
        checkout: Some(checkout_scope()),
    }
}

fn manifest() -> CiDriveManifestV1 {
    let repo_ref = REPO_REF.to_string();
    let authority = authority();
    CiDriveManifestV1 {
        schema_version: 1,
        tenant_id: "manifest-live".into(),
        region: "fr-par".into(),
        wf_run_id: "11111111-1111-8111-8111-111111111111".into(),
        ci_run_id: "22222222-2222-8222-8222-222222222222".into(),
        source_snapshot_ref: format!(
            "myelin://manifest-live/ci/artifact/snapshot-{}",
            digest('a')
        ),
        source_plan_schema_version: 2,
        launch_request_digest: digest('b'),
        workflow_type: CI_PIPELINE_WF_TYPE.into(),
        workflow_definition_version: authority.workflow_definition_version,
        workflow_code_hash: authority.workflow_code_hash.clone(),
        authority_policy_revision: authority.policy_revision.clone(),
        repo_ref: repo_ref.clone(),
        commit_oid: COMMIT_OID.into(),
        run_ref: "myelin://manifest-live/ci/run/22222222-2222-8222-8222-222222222222".into(),
        started_at: "2026-07-21T12:34:56.000000Z".into(),
        trust_tier: CiManifestTrustTierV1::Trusted,
        check_attempts: BTreeMap::from([
            ("build".into(), 9),
            ("lint".into(), 10),
            ("test".into(), 11),
        ]),
        merge_waiter: None,
        jobs: vec![
            GrantedCiJobV1 {
                job_id: PRIMARY_JOB_ID.into(),
                stage: "build".into(),
                name: "build".into(),
                check_context: "build".into(),
                needs: Vec::new(),
                matrix_key: BTreeMap::new(),
                image: linux_small_v1_image().reference,
                command: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf crash-recovered".into(),
                ],
                env: BTreeMap::new(),
                secret_handles: BTreeMap::new(),
                egress_allow: Vec::new(),
                limits: authority.limits.clone(),
                workspace: CiManifestWorkspaceV1 {
                    repo_ref: repo_ref.clone(),
                    commit_oid: COMMIT_OID.into(),
                    read_only_root: true,
                    tmpfs_scratch: true,
                },
                scheduling: CiManifestSchedulingV1 {
                    lane: CiManifestLaneV1::Batch,
                    labels: vec!["linux".into()],
                    concurrency_group: Some("pr:web:42".into()),
                    fair_key: "project:core".into(),
                },
                reserve_handle: reserve_handle(PRIMARY_JOB_ID, '1'),
                token_authority_handle: ManifestBoundCiJobTokenAuthority::handle_for(&authority),
                continue_on_error: false,
            },
            GrantedCiJobV1 {
                job_id: REFUSED_JOB_ID.into(),
                stage: "lint".into(),
                name: "lint".into(),
                check_context: "lint".into(),
                needs: Vec::new(),
                matrix_key: BTreeMap::new(),
                image: format!("registry.example/build@sha256:{}", "e".repeat(64)),
                command: vec!["/bin/false".into()],
                env: BTreeMap::new(),
                secret_handles: BTreeMap::new(),
                egress_allow: Vec::new(),
                limits: authority.limits.clone(),
                workspace: CiManifestWorkspaceV1 {
                    repo_ref: repo_ref.clone(),
                    commit_oid: COMMIT_OID.into(),
                    read_only_root: true,
                    tmpfs_scratch: true,
                },
                scheduling: CiManifestSchedulingV1 {
                    lane: CiManifestLaneV1::Batch,
                    labels: vec!["linux".into()],
                    concurrency_group: None,
                    fair_key: "project:core".into(),
                },
                reserve_handle: reserve_handle(REFUSED_JOB_ID, '2'),
                token_authority_handle: format!("ci-token-authority:v1:{}", "2".repeat(64)),
                continue_on_error: false,
            },
            GrantedCiJobV1 {
                job_id: CRASH_JOB_ID.into(),
                stage: "test".into(),
                name: "test".into(),
                check_context: "test".into(),
                needs: Vec::new(),
                matrix_key: BTreeMap::new(),
                image: format!("registry.example/build@sha256:{}", "f".repeat(64)),
                command: vec!["/bin/true".into()],
                env: BTreeMap::new(),
                secret_handles: BTreeMap::new(),
                egress_allow: Vec::new(),
                limits: authority.limits,
                workspace: CiManifestWorkspaceV1 {
                    repo_ref,
                    commit_oid: COMMIT_OID.into(),
                    read_only_root: true,
                    tmpfs_scratch: true,
                },
                scheduling: CiManifestSchedulingV1 {
                    lane: CiManifestLaneV1::Batch,
                    labels: vec!["linux".into()],
                    concurrency_group: Some("pr:web:42".into()),
                    fair_key: "project:core".into(),
                },
                reserve_handle: reserve_handle(CRASH_JOB_ID, '3'),
                token_authority_handle: format!("ci-token-authority:v1:{}", "3".repeat(64)),
                continue_on_error: false,
            },
        ],
    }
}

#[derive(Default)]
struct RecordingCredentialMinter {
    calls: AtomicUsize,
    authority: Mutex<Option<CiJobRuntimeAuthorityRequest>>,
}

impl CiJobCredentialMinter for RecordingCredentialMinter {
    fn mint_verified<'a>(
        &'a self,
        claim: CiJobTokenRequest,
        authority: CiJobRuntimeAuthorityRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + 'a>>
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.authority.lock().unwrap() = Some(authority);
        Box::pin(async move {
            RunTokenCredential::new(
                format!("identity-bearer:{}", claim.claim_nonce),
                format!("identity-jti:{}", claim.claim_nonce),
                30,
            )
            .map_err(|error| CiJobTokenIssueError(error.to_string()))
        })
    }
}

fn claim(manifest: &CiDriveManifestV1) -> CiJobTokenRequest {
    CiJobTokenRequest {
        tenant_id: manifest.tenant_id.clone(),
        region: manifest.region.clone(),
        wf_run_id: manifest.wf_run_id.clone(),
        ci_run_id: manifest.ci_run_id.clone(),
        job_id: manifest.jobs[0].job_id.clone(),
        token_authority_handle: manifest.jobs[0].token_authority_handle.clone(),
        idem_token: "manifest-live/claim".into(),
        lease_owner: "runner-live".into(),
        lease_epoch: 1,
        claim_nonce: "55555555-5555-8555-8555-555555555555".into(),
        claim_started_at_epoch_secs: 1_785_000_000,
        claim_expires_at_epoch_secs: 1_785_000_030,
    }
}

fn launch_template(
    manifest: &CiDriveManifestV1,
    job_index: usize,
    idem_token: &str,
) -> DurableCiJobLaunchTemplate {
    let job = &manifest.jobs[job_index];
    DurableCiJobLaunchTemplate {
        spec: JobSpecTemplate::new(
            JobKind::Ci,
            ImageRef::pinned(job.image.clone()).unwrap(),
            job.command.clone(),
            job.env
                .iter()
                .map(|(name, value)| EnvVar {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
            job.secret_handles
                .iter()
                .map(|(name, handle)| SecretRef {
                    name: name.clone(),
                    handle: handle.clone(),
                })
                .collect(),
            EgressPolicy {
                allow: job.egress_allow.clone(),
            },
            ResourceLimits {
                cpu_millis: job.limits.cpu_millis,
                mem_bytes: job.limits.mem_bytes,
                disk_bytes: job.limits.disk_bytes,
                tmpfs_bytes: job.limits.disk_bytes,
                pids_max: job.limits.pids_max,
                timeout_secs: job.limits.timeout_secs,
            },
            WorkspaceSpec {
                repo_ref: Some(job.workspace.repo_ref.clone()),
                commit: Some(job.workspace.commit_oid.clone()),
            },
            TrustTier::Trusted,
            MeterTarget {
                reserve_id: job.reserve_handle.clone(),
            },
            IdemToken(idem_token.into()),
        )
        .unwrap(),
        ci_run_id: manifest.ci_run_id.clone(),
        token_authority_handle: job.token_authority_handle.clone(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_replays_exact_bytes_and_refuses_divergent_authority() {
    let bare_admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .expect("connect to dev PostgreSQL as migration role");
    let schema = unique_schema();
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema} AUTHORIZATION myelin_admin;
         GRANT USAGE ON SCHEMA {schema} TO myelin_app;
         ALTER DEFAULT PRIVILEGES FOR ROLE myelin_admin IN SCHEMA {schema}
           GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO myelin_app;"
    ))
    .execute(&bare_admin)
    .await
    .unwrap();

    // A cleanup-dedicated clone of `bare_admin` (a cheap `Arc` handle clone, same underlying
    // pool — `bare_admin` is never `.close()`d itself, only `admin`/`app` are): `with_schema_cleanup`
    // unconditionally drops `schema` through it once the test body (success, assertion failure, or
    // panic) finishes, so the schema never outlives this test regardless of outcome (previously it
    // was dropped ONLY at the natural end of a passing run).
    let cleanup_admin = bare_admin.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_admin, &schema_for_cleanup, move || async move {
    let admin = pinned_pool(&admin_url(), &schema).await;
    let app = pinned_pool(&app_url(), &schema).await;
    PgMigrator::apply(&admin, &identity_durable_migrations())
        .await
        .expect("apply the durable Identity/S7 schema");
    PgMigrator::apply(&admin, &cell_root_durable_migrations())
        .await
        .expect("apply the durable cell-root schema");
    PgMigrator::apply(&admin, &reserve_settle_durable_migrations())
        .await
        .expect("apply the durable reservation schema");
    PgMigrator::apply_validated(
        &admin,
        &flow_migrations(),
        &HotTables::declare(["workflow_run"]),
    )
    .await
    .expect("apply the authoritative durable Flow schema");
    sqlx::raw_sql(&format!(
        "{CREATE_CI_RUN_DDL};
         {ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL};
         {ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL};
         {ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL};
         SELECT myelin_make_tenant_scoped('ci_run');
         {CREATE_CI_DRIVE_MANIFEST_DDL};
         SELECT myelin_make_tenant_scoped('ci_drive_manifest');
         {CREATE_CI_JOB_DDL};
         SELECT myelin_make_tenant_scoped('ci_job');
         {CREATE_FAIR_DEFICIT_DDL};
         {CREATE_JOB_QUEUE_DDL};
         {ALTER_JOB_QUEUE_ADD_COMPLETION_DDL};
         {ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL};
         {ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL};
         {ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL};
         {ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL};
         SELECT myelin_make_tenant_scoped('job_queue');
         {CREATE_CI_JOB_SPEC_DDL};
         {ALTER_CI_JOB_SPEC_ADD_STAGE_DDL};
         SELECT myelin_make_tenant_scoped('ci_job_spec');"
    ))
    .execute(&admin)
    .await
    .expect("apply the production run and manifest migrations");

    let expected = manifest();
    let mut parent = admin.begin().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, true)")
        .bind(&expected.tenant_id)
        .execute(&mut *parent)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', $1, true)")
        .bind(&expected.region)
        .execute(&mut *parent)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO ci_run (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id,
         repo_ref, commit_oid, definition_snapshot, trigger_kind, trust_tier, state, correlation_id)
         VALUES ($1, $2, $3::uuid, $4::uuid, gen_random_uuid(), $5::uuid,
                 $6, $7, $8, 'push', 'trusted', 'running', 'manifest-live')",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.ci_run_id)
    .bind(PROJECT_ID)
    .bind(&expected.wf_run_id)
    .bind(&expected.repo_ref)
    .bind(&expected.commit_oid)
    .bind(&expected.source_snapshot_ref)
    .execute(&mut *parent)
    .await
    .unwrap();
    // BUG FIX (investigation, 2026-07-25): this schema never created its own `ci_job` table.
    // `AUTHORIZE_JOB_LAUNCH_QUERY` (the launch CAS `runner_hooks.attribute` drives) requires a
    // matching `ci_job` row to also cross `queued`/`leased` -> `running` in the SAME statement —
    // without one, `search_path`'s `public` fallback silently resolved every `ci_job` reference to
    // the SHARED dev database's `public.ci_job` (leftover rows from unrelated runs), which never
    // has a row for this schema's job ids. The CAS therefore matched zero rows EVERY time (100%
    // reproducible, not a timing/race artifact). Seed the starter-owned `ci_job` DAG row for each
    // manifest job here, mirroring `pg_pipeline_starter.rs`'s `materialize_ci_jobs`.
    for job in &expected.jobs {
        sqlx::query(
            "INSERT INTO ci_job (tenant_id, region, job_id, run_id, stage, name, needs, spec_ref, \
             state, attempt) \
             VALUES ($1, $2, $3::uuid, $4::uuid, $5, $6, '{}'::uuid[], $7, 'queued', 1)",
        )
        .bind(&expected.tenant_id)
        .bind(&expected.region)
        .bind(&job.job_id)
        .bind(&expected.ci_run_id)
        .bind(&job.stage)
        .bind(&job.name)
        .bind(&expected.source_snapshot_ref)
        .execute(&mut *parent)
        .await
        .expect("seed the starter-owned ci_job surface row the launch CAS crosses");
    }
    sqlx::query(
        "INSERT INTO workflow_run
           (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor,
            correlation_id, depth, partition)
         VALUES ($1, $2, $3, $4, 1, '[]'::jsonb, 'waiting', 0,
                 'manifest-live', 0, 0)",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.wf_run_id)
    .bind(CI_PIPELINE_WF_TYPE)
    .execute(&mut *parent)
    .await
    .expect("seed the active authoritative Flow owner");
    sqlx::query(
        "INSERT INTO cost_reservation (tenant_id, region, run_id, reserved, state)
         VALUES ($1, $2, $3, 1200, 'reserved'),
                ($1, $2, $4, 1200, 'reserved'),
                ($1, $2, $5, 1200, 'reserved')",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[0].reserve_handle)
    .bind(&expected.jobs[1].reserve_handle)
    .bind(&expected.jobs[2].reserve_handle)
    .execute(&mut *parent)
    .await
    .unwrap();
    let mut exact_claim = claim(&expected);
    let persisted_claim_times: (i64, i64) = sqlx::query_as(
        "INSERT INTO job_queue (
           tenant_id, region, job_id, run_id, lane, labels, trust_tier, fair_key, idem_token,
           lease_owner, lease_expires, state, lease_epoch, claim_nonce, stage,
           claim_started_at, claim_expires_at, claim_window_secs
         ) VALUES (
           $1, $2, $3::uuid, $4::uuid, 'batch', ARRAY['linux'], 'trusted', $5, $6,
           $7, statement_timestamp() + interval '300 seconds', 'leased', $8, $9::uuid, 'build',
           statement_timestamp(), statement_timestamp() + interval '4800 seconds', 4800
         )
         RETURNING FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint,
                   FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&exact_claim.job_id)
    .bind(&expected.wf_run_id)
    .bind(format!("project:{PROJECT_ID}"))
    .bind(&exact_claim.idem_token)
    .bind(&exact_claim.lease_owner)
    .bind(exact_claim.lease_epoch)
    .bind(&exact_claim.claim_nonce)
    .fetch_one(&mut *parent)
    .await
    .unwrap();
    exact_claim.claim_started_at_epoch_secs = persisted_claim_times.0;
    exact_claim.claim_expires_at_epoch_secs = persisted_claim_times.1;
    let mut refused_claim = exact_claim.clone();
    refused_claim.job_id = expected.jobs[1].job_id.clone();
    refused_claim.token_authority_handle = expected.jobs[1].token_authority_handle.clone();
    refused_claim.idem_token = "manifest-live/refused".into();
    refused_claim.lease_owner = "runner-refused".into();
    refused_claim.claim_nonce = "77777777-7777-8777-8777-777777777777".into();
    let refused_claim_times: (i64, i64) = sqlx::query_as(
        "INSERT INTO job_queue (
           tenant_id, region, job_id, run_id, lane, labels, trust_tier, concurrency_group,
           fair_key, idem_token,
           lease_owner, lease_expires, state, lease_epoch, claim_nonce, stage,
           claim_started_at, claim_expires_at, claim_window_secs
         ) VALUES (
           $1, $2, $3::uuid, $4::uuid, 'batch', ARRAY['linux'], 'trusted', 'pr:web:42', $5, $6,
           $7, statement_timestamp() + interval '300 seconds', 'leased', $8, $9::uuid, 'lint',
           statement_timestamp(), statement_timestamp() + interval '4800 seconds', 4800
         )
         RETURNING FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint,
                   FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&refused_claim.job_id)
    .bind(&expected.wf_run_id)
    .bind(format!("project:{PROJECT_ID}"))
    .bind(&refused_claim.idem_token)
    .bind(&refused_claim.lease_owner)
    .bind(refused_claim.lease_epoch)
    .bind(&refused_claim.claim_nonce)
    .fetch_one(&mut *parent)
    .await
    .unwrap();
    refused_claim.claim_started_at_epoch_secs = refused_claim_times.0;
    refused_claim.claim_expires_at_epoch_secs = refused_claim_times.1;
    let mut crash_claim = exact_claim.clone();
    crash_claim.job_id = expected.jobs[2].job_id.clone();
    crash_claim.token_authority_handle = expected.jobs[2].token_authority_handle.clone();
    crash_claim.idem_token = "manifest-live/crash".into();
    crash_claim.lease_owner = "runner-crash".into();
    crash_claim.claim_nonce = "bbbbbbbb-bbbb-8bbb-bbbb-bbbbbbbbbbbb".into();
    let crash_claim_times: (i64, i64) = sqlx::query_as(
        "INSERT INTO job_queue (
           tenant_id, region, job_id, run_id, lane, labels, trust_tier, concurrency_group,
           fair_key, idem_token,
           lease_owner, lease_expires, state, lease_epoch, claim_nonce, stage,
           claim_started_at, claim_expires_at, claim_window_secs
         ) VALUES (
           $1, $2, $3::uuid, $4::uuid, 'batch', ARRAY['linux'], 'trusted', 'pr:web:42', $5, $6,
           $7, statement_timestamp() + interval '300 seconds', 'leased', $8, $9::uuid, 'test',
           statement_timestamp(), statement_timestamp() + interval '4800 seconds', 4800
         )
         RETURNING FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint,
                   FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&crash_claim.job_id)
    .bind(&expected.wf_run_id)
    .bind(format!("project:{PROJECT_ID}"))
    .bind(&crash_claim.idem_token)
    .bind(&crash_claim.lease_owner)
    .bind(crash_claim.lease_epoch)
    .bind(&crash_claim.claim_nonce)
    .fetch_one(&mut *parent)
    .await
    .unwrap();
    crash_claim.claim_started_at_epoch_secs = crash_claim_times.0;
    crash_claim.claim_expires_at_epoch_secs = crash_claim_times.1;
    for (job_index, claim) in [(0, &exact_claim), (1, &refused_claim), (2, &crash_claim)] {
        sqlx::query(
            "INSERT INTO ci_job_spec
               (tenant_id, region, job_id, run_id, idem_token, spec, stage)
             VALUES ($1, $2, $3::uuid, $4::uuid, $5, $6, $7)",
        )
        .bind(&expected.tenant_id)
        .bind(&expected.region)
        .bind(&claim.job_id)
        .bind(&expected.wf_run_id)
        .bind(&claim.idem_token)
        .bind(
            serde_json::to_value(launch_template(&expected, job_index, &claim.idem_token)).unwrap(),
        )
        .bind(&expected.jobs[job_index].stage)
        .execute(&mut *parent)
        .await
        .unwrap();
    }
    parent.commit().await.unwrap();

    let store = CiDriveManifestStore::new(
        app.clone(),
        TenantId(expected.tenant_id.clone()),
        Region(expected.region.clone()),
    )
    .unwrap();
    let first_digest = store.insert(&expected).await.unwrap();
    assert_eq!(store.insert(&expected).await.unwrap(), first_digest);
    assert_eq!(
        store
            .load_expected(&expected.wf_run_id, &expected.ci_run_id, &first_digest)
            .await
            .unwrap(),
        expected
    );

    let mut divergent = expected.clone();
    divergent.authority_policy_revision = "ci-policy-live-v2".into();
    assert!(matches!(
        store.insert(&divergent).await,
        Err(myelin_ci_controlplane::CiDriveManifestError::IdentityMismatch)
    ));
    assert!(store
        .load_expected(&expected.wf_run_id, &expected.ci_run_id, &digest('f'))
        .await
        .is_err());

    let raw_minter = Arc::new(RecordingCredentialMinter::default());
    let issuer = LockedManifestCiJobTokenIssuer::new(app.clone(), "fr-par", raw_minter.clone());
    let credential = issuer
        .mint(exact_claim.clone())
        .await
        .expect("durable claim authority reaches Identity mint");
    assert_eq!(credential.ttl_secs(), 30);
    assert_eq!(raw_minter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*raw_minter.authority.lock().unwrap(), Some(authority()));
    let mut cross_cell_region = exact_claim.clone();
    cross_cell_region.region = "us-east".into();
    let cross_cell_error = issuer.mint(cross_cell_region).await.unwrap_err();
    assert!(cross_cell_error
        .0
        .contains("region differs from the runner cell"));
    assert_eq!(
        raw_minter.calls.load(Ordering::SeqCst),
        1,
        "a cross-cell-region claim is refused before durable reads or Identity mint"
    );

    // The exact production Identity composition survives a factory reconstruction: the first
    // instance mints through the locked durable claim, while a second instance reloads the same
    // sealed cell root and durable S7 state, verifies the signed credential, and wins the one-shot
    // durable launch CAS.
    let mut provider_config = MyelinConfig::dev();
    provider_config.database_url = scoped_url(&app_url(), &schema);
    provider_config.region = expected.region.clone();
    let provider = SubstrateProvider::connect(provider_config, 4)
        .await
        .expect("connect the production app-role provider");
    let seal_key = SealKey::from_bytes([0x5a; 32]);
    let first_identity = ci_runner_identity_authorities(
        provider.clone(),
        "ci-identity-restart-cell",
        &seal_key,
        tokio::runtime::Handle::current(),
    )
    .await
    .expect("compose the first production Identity instance");
    let pre_cas_signed = first_identity
        .token_issuer()
        .mint(exact_claim.clone())
        .await
        .expect("mint a real claim-bound PASETO credential");
    let wrong_seal_error = ci_runner_identity_authorities(
        provider.clone(),
        "ci-identity-restart-cell",
        &SealKey::from_bytes([0xa5; 32]),
        tokio::runtime::Handle::current(),
    )
    .await
    .err()
    .expect("an existing cell root must never be replaced after unseal refusal");
    assert_eq!(
        wrong_seal_error,
        myelin_ci_controlplane::CiRunnerIdentityCompositionError::DurableCellRootUnavailable
    );
    let second_identity = ci_runner_identity_authorities(
        provider.clone(),
        "ci-identity-restart-cell",
        &seal_key,
        tokio::runtime::Handle::current(),
    )
    .await
    .expect("reconstruct production Identity from durable state");
    let pre_cas_spec = JobSpecTemplate::new(
        JobKind::Ci,
        ImageRef::pinned(expected.jobs[0].image.clone()).unwrap(),
        expected.jobs[0].command.clone(),
        Vec::new(),
        Vec::new(),
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: expected.jobs[0].limits.cpu_millis,
            mem_bytes: expected.jobs[0].limits.mem_bytes,
            disk_bytes: expected.jobs[0].limits.disk_bytes,
            tmpfs_bytes: expected.jobs[0].limits.disk_bytes,
            pids_max: expected.jobs[0].limits.pids_max,
            timeout_secs: expected.jobs[0].limits.timeout_secs,
        },
        WorkspaceSpec {
            repo_ref: Some(expected.jobs[0].workspace.repo_ref.clone()),
            commit: Some(expected.jobs[0].workspace.commit_oid.clone()),
        },
        TrustTier::Trusted,
        MeterTarget {
            reserve_id: expected.jobs[0].reserve_handle.clone(),
        },
        IdemToken(exact_claim.idem_token.clone()),
    )
    .unwrap()
    .resolve_with_authorization(
        pre_cas_signed,
        Some(ci_job_authorization_context(
            &exact_claim,
            &expected.jobs[0].reserve_handle,
            Some(&checkout_scope()),
        )),
    );
    let launch_authorizer = second_identity.launch_authorizer();
    // CT-007 slice 5b.3-2c (Sol's review): prove `authorize_checkout` against a REAL durable claim
    // through a live PostgreSQL round-trip -- the fake claim gates only prove orchestration.
    launch_authorizer
        .authorize_checkout(&pre_cas_spec, &checkout_scope())
        .expect("an exact live generation passes checkout authorization");
    let (job_queue_state_before, ci_job_state_before): (String, String) = sqlx::query_as(
        "SELECT q.state, j.state FROM job_queue q
           JOIN ci_job j ON j.tenant_id = q.tenant_id AND j.region = q.region
             AND j.job_id = q.job_id
         WHERE q.tenant_id = $1 AND q.region = $2 AND q.job_id = $3::uuid",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[0].job_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        (job_queue_state_before.as_str(), ci_job_state_before.as_str()),
        ("leased", "queued"),
        "checkout authorization must never advance either durable row"
    );
    // A canceled/ineligible `ci_job` surface must fail checkout authorization even though
    // `job_queue` alone still says `leased` -- exactly the gap the read-only
    // `VERIFY_JOB_LAUNCH_LIVE_QUERY` EXISTS join against `ci_job` closes (Sol's review). Flips
    // `ci_job` to `cancelled` and immediately back so the rest of this test's `queued`/`running`
    // assumptions for this row are undisturbed.
    sqlx::query(
        "UPDATE ci_job SET state = 'cancelled'
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[0].job_id)
    .execute(&admin)
    .await
    .unwrap();
    assert!(
        launch_authorizer
            .authorize_checkout(&pre_cas_spec, &checkout_scope())
            .is_err(),
        "a canceled ci_job surface must fail checkout authorization even with a live job_queue row"
    );
    sqlx::query(
        "UPDATE ci_job SET state = 'queued'
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[0].job_id)
    .execute(&admin)
    .await
    .unwrap();
    let runner_hooks = ci_runner_hooks(
        provider.clone(),
        second_identity.launch_authorizer(),
        tokio::runtime::Handle::current(),
    );
    let cancellation_coordinator =
        ci_runner_cancellation_coordinator(provider.clone(), tokio::runtime::Handle::current());

    // Crash window 1, mint → launch CAS: the first production issuer has committed a credential,
    // but no reservation begin or launch CAS follows. Expiry + the real regional reaper return the
    // leased row to the queue. The next real claim increments the durable generation, the stale
    // credential/context can no longer begin or launch, and production Identity remints for the
    // replacement generation.
    sqlx::query(
        "UPDATE job_queue SET lease_expires = statement_timestamp() - interval '1 second'
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[0].job_id)
    .execute(&admin)
    .await
    .expect("expire the minted-but-not-launched generation");
    let region_store = ci_region_queue_store_test_support(admin.clone());
    assert_eq!(
        region_store.reap(&expected.region).await.unwrap(),
        1,
        "the production reaper recovers the minted-but-not-launched lease"
    );
    assert!(
        launch_authorizer
            .authorize_checkout(&pre_cas_spec, &checkout_scope())
            .is_err(),
        "a stale/reaped generation must fail checkout authorization"
    );
    let reminted_lease = region_store
        .claim(
            &expected.region,
            &["linux".into()],
            &[TrustTier::Trusted],
            "runner-reminted",
            300,
        )
        .await
        .unwrap()
        .expect("the recovered row is claimable by a fresh runner generation");
    assert_eq!(reminted_lease.job_id.to_string(), expected.jobs[0].job_id);
    assert_eq!(reminted_lease.lease_epoch, exact_claim.lease_epoch + 1);
    assert_ne!(reminted_lease.claim_nonce, exact_claim.claim_nonce);
    assert!(
        runner_hooks.reserve(&pre_cas_spec).is_err(),
        "the stale pre-crash generation cannot begin the reservation"
    );
    assert!(
        runner_hooks.attribute(&pre_cas_spec).is_err(),
        "the stale pre-crash credential cannot win the launch CAS"
    );
    exact_claim.lease_owner = "runner-reminted".into();
    exact_claim.lease_epoch = reminted_lease.lease_epoch;
    exact_claim.claim_nonce = reminted_lease.claim_nonce;
    exact_claim.claim_started_at_epoch_secs = reminted_lease.claim_started_at_epoch_secs;
    exact_claim.claim_expires_at_epoch_secs = reminted_lease.claim_expires_at_epoch_secs;
    let signed = second_identity
        .token_issuer()
        .mint(exact_claim.clone())
        .await
        .expect("production Identity remints for the reaped claim generation");
    let spec = launch_template(&expected, 0, &exact_claim.idem_token)
        .spec
        .resolve_with_authorization(
            signed.clone(),
            Some(ci_job_authorization_context(
                &exact_claim,
                &expected.jobs[0].reserve_handle,
                Some(&checkout_scope()),
            )),
        );

    let mut mutated_spec = spec.clone();
    mutated_spec.command = vec!["/bin/echo".into(), "mutated".into()];
    assert!(
        runner_hooks.reserve(&mutated_spec).is_err(),
        "same-job executable mutation is refused before reservation begin"
    );
    assert!(
        runner_hooks.attribute(&mutated_spec).is_err(),
        "same-job executable mutation is refused before the durable launch CAS"
    );
    let reservation = runner_hooks
        .reserve(&spec)
        .expect("the exact manifest-scoped reservation begins before final launch");
    assert_eq!(reservation.0, expected.jobs[0].reserve_handle);
    assert_eq!(
        runner_hooks
            .reserve(&spec)
            .expect("an acknowledgement-loss begin retry is idempotent"),
        reservation
    );
    let begun_state: String = sqlx::query_scalar(
        "SELECT state FROM cost_reservation
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[0].reserve_handle)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(begun_state, "inflight");
    // Crash window 2, launch CAS → spawn: arm the lazy production permit, commit the exact CAS, and
    // retain only its session advisory ownership while the sandbox child would still be gated. The
    // committed `running` state is immediately visible (no hours-long transaction/row lock). Even
    // after forcing its execution lease expired, the real reaper must not replace this paused live
    // continuation. Dropping ownership simulates process/connection death; only then may a fresh
    // generation claim and spawn.
    let paused_ownership = runner_hooks
        .acquire_launch_permit(&spec)
        .expect("the production hook returns a lazy exact-generation permit")
        .commit()
        .expect("the armed-child boundary commits CAS and retains session ownership");
    let committed_state: String = sqlx::query_scalar(
        "SELECT state FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[0].job_id)
    .fetch_one(&admin)
    .await
    .expect("the committed launch state is visible outside the ownership session");
    assert_eq!(committed_state, "running");
    sqlx::query(
        "UPDATE job_queue SET lease_expires = statement_timestamp() - interval '1 second'
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[0].job_id)
    .execute(&admin)
    .await
    .expect("expire the committed generation while its live session owns the launch");
    assert_eq!(
        region_store.reap(&expected.region).await.unwrap(),
        0,
        "the reaper refuses a paused post-CAS continuation whose session lock is still live"
    );
    assert!(
        region_store
            .claim(
                &expected.region,
                &["linux".into()],
                &[TrustTier::Trusted],
                "runner-must-not-double-spawn",
                300,
            )
            .await
            .unwrap()
            .is_none(),
        "no replacement generation exists while the original launch fence is retained"
    );
    drop(paused_ownership);
    let mut reaped_after_death = 0;
    for _ in 0..50 {
        reaped_after_death = region_store.reap(&expected.region).await.unwrap();
        if reaped_after_death == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        reaped_after_death, 1,
        "connection death releases ownership and makes the committed running row recoverable"
    );
    let spawned_lease = region_store
        .claim(
            &expected.region,
            &["linux".into()],
            &[TrustTier::Trusted],
            "runner-spawn-recovery",
            300,
        )
        .await
        .unwrap()
        .expect("the committed-CAS-before-release crash is claimable as a fresh generation");
    assert_eq!(spawned_lease.job_id.to_string(), expected.jobs[0].job_id);
    assert_eq!(spawned_lease.lease_epoch, exact_claim.lease_epoch + 1);
    let stale_cas_spec = spec.clone();
    exact_claim.lease_owner = "runner-spawn-recovery".into();
    exact_claim.lease_epoch = spawned_lease.lease_epoch;
    exact_claim.claim_nonce = spawned_lease.claim_nonce;
    exact_claim.claim_started_at_epoch_secs = spawned_lease.claim_started_at_epoch_secs;
    exact_claim.claim_expires_at_epoch_secs = spawned_lease.claim_expires_at_epoch_secs;
    assert!(
        runner_hooks.reserve(&stale_cas_spec).is_err(),
        "the generation that died after CAS cannot begin again after reaping"
    );
    assert!(
        runner_hooks.attribute(&stale_cas_spec).is_err(),
        "the generation that died after CAS cannot relaunch after reaping"
    );
    let spawn_signed = second_identity
        .token_issuer()
        .mint(exact_claim.clone())
        .await
        .expect("production Identity remints after the CAS-before-spawn crash");
    let spawn_spec = launch_template(&expected, 0, &exact_claim.idem_token)
        .spec
        .resolve_with_authorization(
            spawn_signed,
            Some(ci_job_authorization_context(
                &exact_claim,
                &expected.jobs[0].reserve_handle,
                Some(&checkout_scope()),
            )),
        );
    launch_authorizer
        .authorize_checkout(&spawn_spec, &checkout_scope())
        .expect("the fresh reminted generation passes checkout authorization before real launch");
    let recovered_hooks = legacy_streaming_hooks(
        ci_runner_hooks(
            provider.clone(),
            second_identity.launch_authorizer(),
            tokio::runtime::Handle::current(),
        ),
        expected.jobs[0].workspace.repo_ref.clone(),
        expected.jobs[0].workspace.commit_oid.clone(),
    );
    let backend = GvisorBackend::new(test_registry());
    let legacy_streaming_backend = LegacyStreamingGvisor(&backend);
    let launch = legacy_streaming_backend
        .launch(&spawn_spec, &recovered_hooks)
        .expect("the recovered generation reaches the real production gVisor spawn");
    assert!(launch.result.passed(), "the recovered guest passes");
    assert_eq!(
        launch.result.stdout, b"crash-recovered",
        "the immutable recovered command ran inside the real guest"
    );
    legacy_streaming_backend
        .kill(&launch.handle)
        .expect("the recovered one-job guest tears down");
    let recovered_shape: (String, i64, String, i64, i64) = sqlx::query_as(
        "SELECT q.state, q.lease_epoch, r.state,
                (SELECT count(*) FROM cost_reservation rr
                 WHERE rr.tenant_id = q.tenant_id AND rr.region = q.region AND rr.run_id = $4),
                (SELECT count(*) FROM cost_event e
                 WHERE e.tenant_id = q.tenant_id AND e.region = q.region AND e.run_id = $4)
         FROM job_queue q
         JOIN cost_reservation r
           ON r.tenant_id = q.tenant_id AND r.region = q.region AND r.run_id = $4
         WHERE q.tenant_id = $1 AND q.region = $2 AND q.job_id = $3::uuid",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[0].job_id)
    .bind(&expected.jobs[0].reserve_handle)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        recovered_shape,
        (
            "running".into(),
            exact_claim.lease_epoch,
            "inflight".into(),
            1,
            0,
        ),
        "recovery spawns under only the fresh generation, duplicates no reservation, and retains \
         reporter settlement ownership"
    );

    // A final-attribution refusal while the exact claim is still retryable retains its deterministic
    // reservation. The signed credential belongs to the first job, so changing only the
    // non-serializable expected context makes Identity refuse before any launch CAS. Once a
    // cancel-superseded transition has made the same generation terminal, zero-release becomes safe.
    let refused_spec = JobSpecTemplate::new(
        JobKind::Ci,
        ImageRef::pinned(expected.jobs[1].image.clone()).unwrap(),
        expected.jobs[1].command.clone(),
        Vec::new(),
        Vec::new(),
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: expected.jobs[1].limits.cpu_millis,
            mem_bytes: expected.jobs[1].limits.mem_bytes,
            disk_bytes: expected.jobs[1].limits.disk_bytes,
            tmpfs_bytes: expected.jobs[1].limits.disk_bytes,
            pids_max: expected.jobs[1].limits.pids_max,
            timeout_secs: expected.jobs[1].limits.timeout_secs,
        },
        WorkspaceSpec {
            repo_ref: Some(expected.jobs[1].workspace.repo_ref.clone()),
            commit: Some(expected.jobs[1].workspace.commit_oid.clone()),
        },
        TrustTier::Trusted,
        MeterTarget {
            reserve_id: expected.jobs[1].reserve_handle.clone(),
        },
        IdemToken(refused_claim.idem_token.clone()),
    )
    .unwrap()
    .resolve_with_authorization(
        signed.clone(),
        Some(ci_job_authorization_context(
            &refused_claim,
            &expected.jobs[1].reserve_handle,
            Some(&checkout_scope()),
        )),
    );
    let crash_spec = launch_template(&expected, 2, &crash_claim.idem_token)
        .spec
        .resolve_with_authorization(
            signed,
            Some(ci_job_authorization_context(
                &crash_claim,
                &expected.jobs[2].reserve_handle,
                Some(&checkout_scope()),
            )),
        );
    let mut cross_tenant_claim = refused_claim.clone();
    cross_tenant_claim.tenant_id = "manifest-other".into();
    let mut cross_tenant_spec = refused_spec.clone();
    cross_tenant_spec.run_token_authorization = Some(ci_job_authorization_context(
        &cross_tenant_claim,
        &expected.jobs[1].reserve_handle,
        Some(&checkout_scope()),
    ));
    assert!(
        runner_hooks.reserve(&cross_tenant_spec).is_err(),
        "caller-supplied cross-tenant scope never begins another tenant's reservation"
    );
    let mut stale_generation_claim = refused_claim.clone();
    stale_generation_claim.lease_epoch += 1;
    stale_generation_claim.claim_nonce = "88888888-8888-8888-8888-888888888888".into();
    let mut stale_generation_spec = refused_spec.clone();
    stale_generation_spec.run_token_authorization = Some(ci_job_authorization_context(
        &stale_generation_claim,
        &expected.jobs[1].reserve_handle,
        Some(&checkout_scope()),
    ));
    assert!(
        runner_hooks.reserve(&stale_generation_spec).is_err(),
        "a copied context for a nonexistent claim generation cannot begin the reservation"
    );
    let state_after_cross_tenant_refusal: String = sqlx::query_scalar(
        "SELECT state FROM cost_reservation
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[1].reserve_handle)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(state_after_cross_tenant_refusal, "reserved");
    let (runner_hooks, refused_spec, refused_reservation) = std::thread::spawn(move || {
        let refused_reservation = runner_hooks
            .reserve(&refused_spec)
            .expect("the second exact manifest reservation begins off-runtime");
        assert_eq!(
            runner_hooks
                .reserve(&refused_spec)
                .expect("off-runtime begin retry is idempotent"),
            refused_reservation
        );
        assert!(runner_hooks.attribute(&refused_spec).is_err());
        runner_hooks
            .release_unused(&refused_spec, &refused_reservation)
            .expect("a retryable attribution refusal retains the deterministic reservation");
        (runner_hooks, refused_spec, refused_reservation)
    })
    .join()
    .expect("the dedicated production runner thread completes without a bridge panic");
    let retryable_state: String = sqlx::query_scalar(
        "SELECT state FROM cost_reservation
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[1].reserve_handle)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(retryable_state, "inflight");
    let retryable_event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cost_event
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[1].reserve_handle)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(retryable_event_count, 0);
    let mut replacement_claim = refused_claim.clone();
    replacement_claim.lease_owner = "runner-replacement".into();
    replacement_claim.lease_epoch += 1;
    replacement_claim.claim_nonce = "99999999-9999-8999-8999-999999999999".into();
    let replacement_times: (i64, i64) = sqlx::query_as(
        "UPDATE job_queue
         SET state = 'leased', lease_owner = $4, lease_epoch = $5, claim_nonce = $6::uuid,
             claim_started_at = statement_timestamp(),
             claim_expires_at = statement_timestamp() + interval '4800 seconds',
             lease_expires = statement_timestamp() + interval '300 seconds'
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
         RETURNING FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint,
                   FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[1].job_id)
    .bind(&replacement_claim.lease_owner)
    .bind(replacement_claim.lease_epoch)
    .bind(&replacement_claim.claim_nonce)
    .fetch_one(&admin)
    .await
    .unwrap();
    replacement_claim.claim_started_at_epoch_secs = replacement_times.0;
    replacement_claim.claim_expires_at_epoch_secs = replacement_times.1;
    runner_hooks
        .release_unused(&refused_spec, &refused_reservation)
        .expect("a stale generation cannot release its replacement's reservation");
    let state_after_stale_release: String = sqlx::query_scalar(
        "SELECT state FROM cost_reservation
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[1].reserve_handle)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(state_after_stale_release, "inflight");
    let crash_reservation = runner_hooks
        .reserve(&crash_spec)
        .expect("the third exact reservation begin commits before simulated acknowledgement loss");
    assert_eq!(crash_reservation.0, expected.jobs[2].reserve_handle);
    drop(crash_reservation);
    let mut canceled = cancellation_coordinator
        .cancel_superseded(
            &TenantId(expected.tenant_id.clone()),
            "pr:web:42",
            PRIMARY_JOB_ID,
        )
        .expect("cancel and zero-settle both superseded reservations atomically");
    canceled.sort();
    let mut expected_canceled = vec![
        expected.jobs[1].job_id.clone(),
        expected.jobs[2].job_id.clone(),
    ];
    expected_canceled.sort();
    assert_eq!(canceled, expected_canceled);
    assert!(cancellation_coordinator
        .cancel_superseded(
            &TenantId(expected.tenant_id.clone()),
            "pr:web:42",
            PRIMARY_JOB_ID,
        )
        .expect("cancel acknowledgement-loss retry is idempotent")
        .is_empty());
    let released_state: String = sqlx::query_scalar(
        "SELECT state FROM cost_reservation
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[1].reserve_handle)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(released_state, "settled");
    let released_units: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT unit, wholesale, markup FROM cost_event
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 ORDER BY ord",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[1].reserve_handle)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(
        released_units,
        vec![
            ("cpu_seconds".into(), 0, 0),
            ("mem_gb_seconds".into(), 0, 0),
        ]
    );
    let crash_released: (String, i64) = sqlx::query_as(
        "SELECT r.state, count(e.ord)
         FROM cost_reservation r
         LEFT JOIN cost_event e
           ON e.tenant_id = r.tenant_id AND e.region = r.region AND e.run_id = r.run_id
         WHERE r.tenant_id = $1 AND r.region = $2 AND r.run_id = $3
         GROUP BY r.state",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.region)
    .bind(&expected.jobs[2].reserve_handle)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(crash_released, ("settled".into(), 2));

    let mut forged = exact_claim.clone();
    forged.token_authority_handle = format!("ci-token-authority:v1:{}", "0".repeat(64));
    assert!(issuer.mint(forged).await.is_err());
    assert_eq!(
        raw_minter.calls.load(Ordering::SeqCst),
        1,
        "a divergent durable handle is refused before the raw Identity minter"
    );

    let mut forged_owner = exact_claim.clone();
    forged_owner.lease_owner = "forged-runner".into();
    assert!(issuer.mint(forged_owner).await.is_err());
    let mut forged_time = exact_claim.clone();
    forged_time.claim_started_at_epoch_secs += 1;
    forged_time.claim_expires_at_epoch_secs += 1;
    assert!(issuer.mint(forged_time).await.is_err());
    assert_eq!(raw_minter.calls.load(Ordering::SeqCst), 1);

    sqlx::query(
        "UPDATE job_queue SET state = 'queued', lease_owner = NULL, lease_expires = NULL,
         claim_nonce = NULL WHERE tenant_id = $1 AND job_id = $2::uuid",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.jobs[0].job_id)
    .execute(&admin)
    .await
    .unwrap();
    assert!(issuer.mint(exact_claim.clone()).await.is_err());

    sqlx::query(
        "UPDATE job_queue SET state = 'leased', lease_owner = 'replacement-runner',
         lease_epoch = lease_epoch + 1, claim_nonce = gen_random_uuid(),
         claim_started_at = statement_timestamp(), claim_expires_at = statement_timestamp() + interval '30 seconds',
         lease_expires = statement_timestamp() + interval '30 seconds'
         WHERE tenant_id = $1 AND job_id = $2::uuid",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.jobs[0].job_id)
    .execute(&admin)
    .await
    .unwrap();
    assert!(issuer.mint(exact_claim).await.is_err());
    assert_eq!(
        raw_minter.calls.load(Ordering::SeqCst),
        1,
        "stale, reaped, reclaimed, or forged claim facts never reach Identity"
    );

    drop(second_identity);
    drop(first_identity);
    drop(provider);
    admin.close().await;
    app.close().await;
    // `with_schema_cleanup` (wrapping this whole body) now owns dropping `schema` unconditionally
    // through its own `cleanup_admin` handle — it runs after this closure returns, success or panic
    // alike, so no explicit `DROP SCHEMA`/`bare_admin.close()` is needed here anymore (closing
    // `bare_admin` itself here would also close `cleanup_admin`, since `PgPool::close()` shuts down
    // every clone of the same underlying pool, and make that unconditional cleanup a silent no-op).
    })
    .await;
}
