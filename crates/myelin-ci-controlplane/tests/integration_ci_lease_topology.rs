//! Live-PostgreSQL proof for the CT-007 lease/topology reconciliation slice.
//!
//! Four properties, all against the real migration set and the real production SQL constants:
//! 1. the claim sizes the IMMUTABLE window from the job's own topology while the heartbeat-
//!    extendable EXECUTION lease stays flat at `CI_RUNNER_EXECUTION_LEASE_TTL_SECS`;
//! 2. a legacy NULL-window row still claims under the flat fallback, and a checkout-bearing job on
//!    such a row is refused BEFORE any credential is minted;
//! 3. the exact-generation preparation renewal accepts only the complete live generation and is
//!    capped at the immutable claim expiry;
//! 4. the reaper seals exactly the generation it requeued, in the SAME transaction — and a failing
//!    seal rolls the whole sweep back rather than committing a claimable-but-unresolved state.
//!
//! Plus the two guards the definition-version bump this slice forces made necessary: the
//! superseded-definition boot guard, and the claim-window expand's refusal to adopt a same-named
//! constraint whose definition diverges.
#![cfg(feature = "integration")]

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use common::with_schema_cleanup;
use myelin_ci_controlplane::{
    ci_job_queue_store, ci_job_spec_store, ci_region_queue_store_test_support,
    claim_window_secs_for_template, durable_spec_resolver_test_support,
    resolve_prelaunch_usage_on_conn, CiJobLaunchClaim, CiJobSpecStore, CiJobTokenIssueError,
    CiJobTokenIssuer, CiJobTokenRequest, CiPipelineReporterFactory,
    CiPipelineReporterFactoryError, CiPipelineReporterRouter, CiPrelaunchParentExpectation,
    CiPrelaunchSettlementIdentity, CiPrelaunchUnresolvedPolicy, DurableCiJobLaunchTemplate,
    DurableEnqueue, Lane, CI_RUNNER_EXECUTION_LEASE_TTL_SECS, MAX_CI_JOB_CLAIM_WINDOW_SECS,
};
use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind, JobSpecTemplate, MeterTarget, ResourceLimits,
    RunTokenCredential, TrustTier, WorkspaceSpec,
};
use myelin_storage::{HotTables, PgMigrator};
use myelin_tenancy::Region;
use sqlx::{Executor, PgPool, Row};

/// Independent `PgMigrator` sequences against the same live PostgreSQL deadlock on the migration
/// advisory lock when run concurrently — the same guard `integration_ci_terminal_accounting_atomic`
/// already uses.
static MIGRATION_SCENARIO_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const TENANT: &str = "lease-topology";
const REGION: &str = "fr-par";
const REPO_REF: &str = "myelin://lease-topology/git/repo/core";
const COMMIT_OID: &str = "deadbeef00deadbeef00deadbeef00deadbeef00";
const RESERVE_HANDLE: &str = "ci-reserve:v2:lease-topology";

fn unused_secret_terminal_reporter() -> CiPipelineReporterRouter {
    let factory: CiPipelineReporterFactory =
        Arc::new(|_, _| Err(CiPipelineReporterFactoryError));
    CiPipelineReporterRouter::new(Region(REGION.into()), factory).unwrap()
}

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

async fn pinned_pool(url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
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
        .expect("connect to live PostgreSQL (is the dev stack up?)")
}

fn schema_name(tag: &str) -> String {
    format!(
        "ci_lease_topology_{}_{}_{}",
        std::process::id(),
        tag,
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    )
}



/// A fresh schema carrying the real Flow + CI control-plane migration sets.
async fn migrated_schema(tag: &str) -> (String, PgPool, PgPool, PgPool) {
    let schema = schema_name(tag);
    let bootstrap = pinned_pool(&admin_url(), "public").await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap
        .execute(format!("CREATE SCHEMA {schema} AUTHORIZATION myelin_admin").as_str())
        .await
        .unwrap();
    let admin = pinned_pool(&admin_url(), &schema).await;
    admin
        .execute(
            format!(
                "GRANT USAGE ON SCHEMA {schema} TO myelin_app;
                 ALTER DEFAULT PRIVILEGES FOR ROLE myelin_admin IN SCHEMA {schema}
                   GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO myelin_app;"
            )
            .as_str(),
        )
        .await
        .unwrap();
    PgMigrator::apply_validated(
        &admin,
        &myelin_flow::migrations::migrations(),
        &HotTables::declare(["workflow_run"]),
    )
    .await
    .unwrap();
    PgMigrator::apply_validated(
        &admin,
        &myelin_ci_controlplane::ci_controlplane_migrations(),
        &myelin_ci_controlplane::ci_controlplane_hot_tables(),
    )
    .await
    .unwrap();
    let app = pinned_pool(&app_url(), &schema).await;
    (schema, bootstrap, admin, app)
}

fn uuid(prefix: u8, seed: u64) -> String {
    format!("{prefix:02x}000000-0000-4000-8000-{seed:012x}")
}

fn launch_template(seed: u64, timeout_secs: u32, checkout: bool) -> DurableCiJobLaunchTemplate {
    DurableCiJobLaunchTemplate {
        project_id: "55555555-5555-4555-8555-555555555555".into(),
        spec: JobSpecTemplate {
            kind: JobKind::Ci,
            image: ImageRef::pinned(format!("registry.example/ci@sha256:{}", "b".repeat(64)))
                .unwrap(),
            command: vec!["true".into()],
            env: Vec::new(),
            secret_refs: Vec::new(),
            egress: EgressPolicy::deny_all(),
            limits: ResourceLimits {
                cpu_millis: 1_000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1 << 30,
                tmpfs_bytes: 1 << 30,
                pids_max: 128,
                timeout_secs,
            },
            workspace: if checkout {
                WorkspaceSpec {
                    repo_ref: Some(REPO_REF.into()),
                    commit: Some(COMMIT_OID.into()),
                }
            } else {
                WorkspaceSpec::default()
            },
            trust_tier: TrustTier::Trusted,
            meter_to: MeterTarget {
                reserve_id: RESERVE_HANDLE.into(),
            },
            idem_token: IdemToken(format!("lease-topology-{seed}")),
        },
        ci_run_id: uuid(0x10, seed),
        token_authority_handle: format!("identity-authority:{seed}"),
    }
}

/// Dispatch one job through the REAL co-persist path and activate its Flow/CI owners so the claim's
/// lifecycle conjunction holds. Returns `(job_id, wf_run_id, ci_run_id, derived window)`.
async fn dispatch(
    admin: &PgPool,
    specs: &CiJobSpecStore,
    seed: u64,
    label: &str,
    timeout_secs: u32,
    checkout: bool,
) -> (String, String, String, i64) {
    let job_id = uuid(0x40, seed);
    let wf_run_id = uuid(0x20, seed);
    let launch = launch_template(seed, timeout_secs, checkout);
    let ci_run_id = launch.ci_run_id.clone();
    let window = claim_window_secs_for_template(&launch.spec).unwrap();
    sqlx::query(
        "INSERT INTO ci_run (
           tenant_id, region, run_id, project_id, repo_ref, commit_oid, pipeline_id, wf_run_id,
           definition_snapshot, trigger_kind, trust_tier, state, correlation_id
         ) VALUES ($1, $2, $3::uuid, $4::uuid, $5, $6, $7::uuid, $8::uuid,
                   'snapshot', 'push', 'trusted', 'running', $9)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&ci_run_id)
    .bind(uuid(0x30, seed))
    .bind(REPO_REF)
    .bind(COMMIT_OID)
    .bind(uuid(0x50, seed))
    .bind(&wf_run_id)
    .bind(format!("corr-{seed}"))
    .execute(admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_run (
           tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
           depth, partition
         ) VALUES ($1, $2, $3, 'ci.pipeline', 1, '[]'::jsonb, 'running', $3, 0, 0)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&wf_run_id)
    .execute(admin)
    .await
    .unwrap();
    specs
        .co_persist_dispatch(
            &DurableEnqueue {
                tenant_id: TENANT.into(),
                region: REGION.into(),
                job_id: job_id.clone(),
                run_id: wf_run_id.clone(),
                lane: Lane::Batch,
                labels: vec![label.into()],
                trust_tier: TrustTier::Trusted,
                concurrency_group: None,
                fair_key: format!("project-{seed}"),
                idem_token: format!("lease-topology-{seed}"),
                stage: "build".into(),
                claim_window_secs: window,
                reservation_write_version: myelin_ci_controlplane::ReservationWriteVersionMarker::derive_from_reserve_handle(
                    &launch.spec.meter_to.reserve_id,
                ),
            },
            &launch,
            "build",
        )
        .await
        .unwrap();
    (job_id, wf_run_id, ci_run_id, window)
}

/// The two durable deadlines of a leased row, as whole seconds relative to `claim_started_at`.
async fn deadlines(admin: &PgPool, job_id: &str) -> (i64, i64, Option<i64>) {
    let row = sqlx::query(
        "SELECT EXTRACT(EPOCH FROM (claim_expires_at - claim_started_at))::bigint AS claim_span,
                EXTRACT(EPOCH FROM (lease_expires - claim_started_at))::bigint AS lease_span,
                claim_window_secs
         FROM job_queue WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(job_id)
    .fetch_one(admin)
    .await
    .unwrap();
    (
        row.get("claim_span"),
        row.get("lease_span"),
        row.get("claim_window_secs"),
    )
}

// ═════════════ 1. topology-derived claim ceiling, flat per-execution lease ════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_claim_sizes_the_immutable_ceiling_from_topology_and_the_lease_per_execution() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin, app) = migrated_schema("claim").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let specs = ci_job_spec_store(app.clone());
        let region_store = ci_region_queue_store_test_support(admin.clone());

        // (timeout, checkout, expected immutable window)
        let cases: [(u32, bool, i64); 5] = [
            (1, false, CI_RUNNER_EXECUTION_LEASE_TTL_SECS),
            (2 * 60 * 60, false, CI_RUNNER_EXECUTION_LEASE_TTL_SECS),
            (6 * 60 * 60, false, CI_RUNNER_EXECUTION_LEASE_TTL_SECS),
            (2 * 60 * 60, true, 4 * (7_200 + 600)),
            (6 * 60 * 60, true, MAX_CI_JOB_CLAIM_WINDOW_SECS as i64),
        ];
        for (index, (timeout_secs, checkout, expected)) in cases.into_iter().enumerate() {
            let seed = 100 + index as u64;
            let label = format!("lane-{seed}");
            let (job_id, _, _, derived) =
                dispatch(&admin, &specs, seed, &label, timeout_secs, checkout).await;
            assert_eq!(derived, expected, "case {index}: derivation");

            let leased = region_store
                .claim(
                    REGION,
                    std::slice::from_ref(&label),
                    &[TrustTier::Trusted],
                    &format!("runner-{seed}"),
                    CI_RUNNER_EXECUTION_LEASE_TTL_SECS as u64,
                )
                .await
                .unwrap()
                .expect("the dispatched job claims");
            assert_eq!(leased.job_id.to_string(), job_id);
            assert_eq!(
                leased.claim_window_secs,
                Some(expected),
                "case {index}: the claim returns the durable window"
            );
            assert_eq!(
                leased.claim_expires_at_epoch_secs - leased.claim_started_at_epoch_secs,
                expected,
                "case {index}: the immutable ceiling is the durable window"
            );

            let (claim_span, lease_span, stored_window) = deadlines(&admin, &job_id).await;
            assert_eq!(claim_span, expected);
            assert_eq!(stored_window, Some(expected));
            assert_eq!(
                lease_span, CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
                "case {index}: the EXECUTION lease is one execution slot, never the whole window"
            );
        }

        // The two 6-hour cases are the headline of the slice: identical job timeouts, the checkout
        // job's generation lives four times as long while both leases stay at 22,200 seconds.
        let (flat_claim, flat_lease, _) = deadlines(&admin, &uuid(0x40, 102)).await;
        let (checkout_claim, checkout_lease, _) = deadlines(&admin, &uuid(0x40, 104)).await;
        assert_eq!(flat_claim, 22_200);
        assert_eq!(checkout_claim, 88_800);
        assert_eq!(flat_lease, checkout_lease);
        assert_eq!(flat_lease, CI_RUNNER_EXECUTION_LEASE_TTL_SECS);
    })
    .await;
    })
    .await;
}

// ═════════════ 2. the legacy NULL-window generation ══════════════════════════════════════════════

/// An issuer that must never be reached: a checkout-bearing job on a legacy row has to be refused
/// BEFORE any credential is minted.
struct NeverMintIssuer(Arc<std::sync::Mutex<u32>>);

impl CiJobTokenIssuer for NeverMintIssuer {
    fn mint(
        &self,
        _request: CiJobTokenRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + '_>>
    {
        *self.0.lock().unwrap() += 1;
        Box::pin(async move {
            RunTokenCredential::new("bearer", "lease-topology-jti", 300)
                .map_err(|error| CiJobTokenIssueError(error.to_string()))
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_legacy_null_window_row_claims_flat_and_refuses_checkout_before_any_mint() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin, app) = migrated_schema("legacy").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let specs = ci_job_spec_store(app.clone());
        let queue = ci_job_queue_store(app.clone());
        let region_store = ci_region_queue_store_test_support(admin.clone());

        for (seed, label, checkout) in [
            (200_u64, "legacy-compute", false),
            (201, "legacy-ckout", true),
        ] {
            let (job_id, _, _, _) = dispatch(&admin, &specs, seed, label, 600, checkout).await;
            // Rewrite the row to the pre-expand shape an older dispatch binary would have left.
            sqlx::query(
                "UPDATE job_queue SET claim_window_secs = NULL
                 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&job_id)
            .execute(&admin)
            .await
            .unwrap();
        }

        let mints = Arc::new(std::sync::Mutex::new(0_u32));
        let resolver = durable_spec_resolver_test_support(
            specs.clone(),
            REGION,
            tokio::runtime::Handle::current(),
            Arc::new(NeverMintIssuer(mints.clone())),
            myelin_ci_controlplane::unavailable_ci_job_secret_resolver(),
            unused_secret_terminal_reporter(),
        );

        // The non-checkout legacy row is claimed under the flat fallback and resolves normally.
        let compute = region_store
            .claim(
                REGION,
                &["legacy-compute".to_string()],
                &[TrustTier::Trusted],
                "runner-legacy-compute",
                CI_RUNNER_EXECUTION_LEASE_TTL_SECS as u64,
            )
            .await
            .unwrap()
            .expect("the legacy compute row still claims");
        assert_eq!(compute.claim_window_secs, None);
        assert_eq!(
            compute.claim_expires_at_epoch_secs - compute.claim_started_at_epoch_secs,
            CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
            "a legacy row COALESCEs to the flat execution-lease TTL, byte-identical to before"
        );
        let resolved = {
            let resolver = resolver.clone();
            tokio::task::spawn_blocking(move || resolver(&compute))
                .await
                .unwrap()
        };
        assert!(
            resolved.is_ok(),
            "a legacy NON-checkout row is still resolvable: {resolved:?}"
        );
        assert_eq!(*mints.lock().unwrap(), 1, "the compute job did mint");

        // The checkout-bearing legacy row is refused BEFORE the mint and left for the reaper.
        let checkout = region_store
            .claim(
                REGION,
                &["legacy-ckout".to_string()],
                &[TrustTier::Trusted],
                "runner-legacy-checkout",
                CI_RUNNER_EXECUTION_LEASE_TTL_SECS as u64,
            )
            .await
            .unwrap()
            .expect("the legacy checkout row claims before the resolver refuses it");
        assert_eq!(checkout.claim_window_secs, None);
        let checkout_job_id = checkout.job_id.to_string();
        let refused = {
            let resolver = resolver.clone();
            tokio::task::spawn_blocking(move || resolver(&checkout))
                .await
                .unwrap()
        };
        let error = refused.expect_err("a checkout-bearing legacy row must not resolve");
        assert!(
            error.contains("no durable claim window"),
            "unexpected refusal: {error}"
        );
        assert_eq!(
            *mints.lock().unwrap(),
            1,
            "the refusal happens BEFORE the mint — no second credential was requested"
        );
        let state: String = sqlx::query_scalar(
            "SELECT state FROM job_queue WHERE tenant_id = $1 AND job_id = $2::uuid",
        )
        .bind(TENANT)
        .bind(&checkout_job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(state, "leased", "the refused row is left for the reaper");
        drop(queue);
    })
    .await;
    })
    .await;
}

// ═════════════ 3. the exact-generation preparation renewal ═══════════════════════════════════════

/// Seed one leased generation with a durable parent attempt and a public `ci_job` surface row.
#[allow(clippy::too_many_arguments)]
async fn seed_leased_generation(
    admin: &PgPool,
    specs: &CiJobSpecStore,
    seed: u64,
    label: &str,
    lease_epoch: i64,
    claim_nonce: &str,
    claim_started_at: i64,
    claim_expires_at: i64,
    lease_expires: i64,
    admit_parent_attempt: bool,
) -> CiJobLaunchClaim {
    let (job_id, wf_run_id, ci_run_id, _) = dispatch(admin, specs, seed, label, 600, true).await;
    sqlx::query(
        "INSERT INTO ci_job (tenant_id, region, job_id, run_id, stage, name, spec_ref, state)
         VALUES ($1, $2, $3::uuid, $4::uuid, 'build', 'build', 'spec', 'queued')",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&job_id)
    .bind(&ci_run_id)
    .execute(admin)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE job_queue
         SET state = 'leased', lease_owner = $1, lease_epoch = $2, claim_nonce = $3::uuid,
             claim_started_at = to_timestamp($4), claim_expires_at = to_timestamp($5),
             lease_expires = to_timestamp($6)
         WHERE tenant_id = $7 AND region = $8 AND job_id = $9::uuid",
    )
    .bind(format!("runner-{seed}"))
    .bind(lease_epoch)
    .bind(claim_nonce)
    .bind(claim_started_at)
    .bind(claim_expires_at)
    .bind(lease_expires)
    .bind(TENANT)
    .bind(REGION)
    .bind(&job_id)
    .execute(admin)
    .await
    .unwrap();
    if admit_parent_attempt {
        insert_parent_attempt(
            admin,
            &job_id,
            &wf_run_id,
            &ci_run_id,
            &format!("runner-{seed}"),
            lease_epoch,
            claim_nonce,
            claim_started_at,
            claim_expires_at,
        )
        .await;
    }
    CiJobLaunchClaim {
        tenant_id: TENANT.into(),
        region: REGION.into(),
        wf_run_id,
        job_id,
        lease_owner: format!("runner-{seed}"),
        lease_epoch,
        claim_nonce: claim_nonce.into(),
        claim_started_at_epoch_secs: claim_started_at,
        claim_expires_at_epoch_secs: claim_expires_at,
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_parent_attempt(
    admin: &PgPool,
    job_id: &str,
    wf_run_id: &str,
    ci_run_id: &str,
    lease_owner: &str,
    lease_epoch: i64,
    claim_nonce: &str,
    claim_started_at: i64,
    claim_expires_at: i64,
) {
    sqlx::query(
        "INSERT INTO ci_job_parent_attempt (
           tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle, lease_owner,
           lease_epoch, claim_nonce, claim_started_at_epoch_secs, claim_expires_at_epoch_secs,
           budget_revision, max_parent_attempts
         ) VALUES ($1, $2, $3::uuid, $4::uuid, $5::uuid, $6, $7, $8, $9::uuid, $10, $11, 1, 4)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(job_id)
    .bind(wf_run_id)
    .bind(ci_run_id)
    .bind(RESERVE_HANDLE)
    .bind(lease_owner)
    .bind(lease_epoch)
    .bind(claim_nonce)
    .bind(claim_started_at)
    .bind(claim_expires_at)
    .execute(admin)
    .await
    .unwrap();
}

async fn insert_started_phase(
    admin: &PgPool,
    job_id: &str,
    lease_epoch: i64,
    claim_nonce: &str,
    phase: &str,
) {
    sqlx::query(
        "INSERT INTO ci_job_prelaunch_usage (
           tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status,
           ceiling_cpu_seconds, ceiling_mem_byte_seconds, started_at, seal_after
         ) VALUES ($1, $2, $3::uuid, $4, $5::uuid, $6, 'started', 10, 20,
                   statement_timestamp(), statement_timestamp() + interval '1 day')",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(job_id)
    .bind(lease_epoch)
    .bind(claim_nonce)
    .bind(phase)
    .execute(admin)
    .await
    .unwrap();
}

async fn phase_status(admin: &PgPool, job_id: &str, lease_epoch: i64, phase: &str) -> String {
    sqlx::query_scalar(
        "SELECT status FROM ci_job_prelaunch_usage
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND lease_epoch = $4 AND phase = $5",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(job_id)
    .bind(lease_epoch)
    .bind(phase)
    .fetch_one(admin)
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_preparation_renewal_accepts_only_the_complete_live_generation() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin, app) = migrated_schema("renew").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let specs = ci_job_spec_store(app.clone());
        let queue = ci_job_queue_store(app.clone());
        let now = chrono::Utc::now().timestamp();
        let claim = seed_leased_generation(
            &admin,
            &specs,
            300,
            "renew-lane",
            1,
            &uuid(0x60, 300),
            now - 10,
            now + 80_000,
            now + 30,
            true,
        )
        .await;

        // The exact live generation renews and pushes the execution lease forward by one slot.
        assert!(queue.renew_preparation_lease(&claim).await.unwrap());
        let lease_ahead: i64 = sqlx::query_scalar(
            "SELECT EXTRACT(EPOCH FROM (lease_expires - statement_timestamp()))::bigint
             FROM job_queue WHERE tenant_id = $1 AND job_id = $2::uuid",
        )
        .bind(TENANT)
        .bind(&claim.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(
            (CI_RUNNER_EXECUTION_LEASE_TTL_SECS - 60..=CI_RUNNER_EXECUTION_LEASE_TTL_SECS)
                .contains(&lease_ahead),
            "the renewal installs one execution slot, got {lease_ahead}s"
        );

        // EVERY identity component is load-bearing: a single divergent field refuses.
        type ClaimMutation = fn(&mut CiJobLaunchClaim);
        let mutations: [(&str, ClaimMutation); 8] = [
            ("tenant", |c| c.tenant_id = "other-tenant".into()),
            ("region", |c| c.region = "de-fra".into()),
            ("job", |c| c.job_id = uuid(0x40, 999)),
            ("workflow run", |c| c.wf_run_id = uuid(0x20, 999)),
            ("owner", |c| c.lease_owner = "other-runner".into()),
            ("epoch", |c| c.lease_epoch += 1),
            ("nonce", |c| c.claim_nonce = uuid(0x61, 999)),
            ("claim start", |c| c.claim_started_at_epoch_secs += 1),
        ];
        for (name, mutate) in mutations {
            let mut divergent = claim.clone();
            mutate(&mut divergent);
            assert!(
                !queue.renew_preparation_lease(&divergent).await.unwrap(),
                "a divergent {name} must lose ownership"
            );
        }
        let mut divergent_expiry = claim.clone();
        divergent_expiry.claim_expires_at_epoch_secs += 1;
        assert!(
            !queue
                .renew_preparation_lease(&divergent_expiry)
                .await
                .unwrap(),
            "a divergent claim expiry must lose ownership"
        );

        // Surface/queue states: only `leased` + a pre-workload `ci_job` surface may renew.
        for (queue_state, refuses) in [("running", true), ("terminal", true), ("leased", false)] {
            sqlx::query(
                "UPDATE job_queue SET state = $1 WHERE tenant_id = $2 AND job_id = $3::uuid",
            )
            .bind(queue_state)
            .bind(TENANT)
            .bind(&claim.job_id)
            .execute(&admin)
            .await
            .unwrap();
            assert_eq!(
                !queue.renew_preparation_lease(&claim).await.unwrap(),
                refuses,
                "queue state `{queue_state}` renewal expectation"
            );
        }
        for (surface_state, refuses) in [
            ("running", true),
            ("succeeded", true),
            ("failed", true),
            ("cancelled", true),
            ("reaped", true),
            ("leased", false),
            ("queued", false),
        ] {
            sqlx::query("UPDATE ci_job SET state = $1 WHERE tenant_id = $2 AND job_id = $3::uuid")
                .bind(surface_state)
                .bind(TENANT)
                .bind(&claim.job_id)
                .execute(&admin)
                .await
                .unwrap();
            assert_eq!(
                !queue.renew_preparation_lease(&claim).await.unwrap(),
                refuses,
                "surface state `{surface_state}` renewal expectation"
            );
        }
        sqlx::query("DELETE FROM ci_job WHERE tenant_id = $1 AND job_id = $2::uuid")
            .bind(TENANT)
            .bind(&claim.job_id)
            .execute(&admin)
            .await
            .unwrap();
        assert!(
            !queue.renew_preparation_lease(&claim).await.unwrap(),
            "a missing public surface row must lose ownership"
        );
        sqlx::query(
            "INSERT INTO ci_job (tenant_id, region, job_id, run_id, stage, name, spec_ref, state)
             SELECT $1, $2, $3::uuid, run_id, 'build', 'build', 'spec', 'queued'
             FROM ci_run WHERE tenant_id = $1 AND wf_run_id = $4::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&claim.job_id)
        .bind(&claim.wf_run_id)
        .execute(&admin)
        .await
        .unwrap();

        // A recorded completion receipt terminalizes ownership for renewal purposes.
        sqlx::query(
            "UPDATE job_queue SET completion_receipt = 'receipt'
             WHERE tenant_id = $1 AND job_id = $2::uuid",
        )
        .bind(TENANT)
        .bind(&claim.job_id)
        .execute(&admin)
        .await
        .unwrap();
        assert!(!queue.renew_preparation_lease(&claim).await.unwrap());
        sqlx::query(
            "UPDATE job_queue SET completion_receipt = NULL
             WHERE tenant_id = $1 AND job_id = $2::uuid",
        )
        .bind(TENANT)
        .bind(&claim.job_id)
        .execute(&admin)
        .await
        .unwrap();

        assert!(
            queue.renew_preparation_lease(&claim).await.unwrap(),
            "the restored generation renews again"
        );

        // The exact durable parent attempt must exist: an identical live generation that was never
        // admitted to the journal has no preparation to renew for. (The parent-attempt table is
        // structurally immutable, so this is a separately seeded job rather than a deletion.)
        let unadmitted = seed_leased_generation(
            &admin,
            &specs,
            302,
            "renew-unadmitted",
            1,
            &uuid(0x60, 302),
            now - 10,
            now + 80_000,
            now + 30,
            false,
        )
        .await;
        assert!(
            !queue.renew_preparation_lease(&unadmitted).await.unwrap(),
            "no durable parent attempt means no preparation to renew for"
        );

        // An already-expired claim window can never be renewed back to life.
        let expired_claim = seed_leased_generation(
            &admin,
            &specs,
            301,
            "renew-expired",
            1,
            &uuid(0x60, 301),
            now - 100,
            now - 1,
            now - 1,
            true,
        )
        .await;
        assert!(
            !queue.renew_preparation_lease(&expired_claim).await.unwrap(),
            "an expired immutable window is the hard ceiling; renewal cannot reopen it"
        );
    })
    .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_renewal_is_capped_at_the_immutable_claim_expiry() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin, app) = migrated_schema("cap").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let specs = ci_job_spec_store(app.clone());
        let queue = ci_job_queue_store(app.clone());
        let now = chrono::Utc::now().timestamp();
        // A window far shorter than one execution slot: the cap, not the slot, must win.
        let claim = seed_leased_generation(
            &admin,
            &specs,
            400,
            "cap-lane",
            1,
            &uuid(0x60, 400),
            now - 10,
            now + 60,
            now + 5,
            true,
        )
        .await;
        assert!(queue.renew_preparation_lease(&claim).await.unwrap());
        let (lease_epoch_secs, claim_epoch_secs): (i64, i64) = sqlx::query_as(
            "SELECT FLOOR(EXTRACT(EPOCH FROM lease_expires))::bigint,
                    FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint
             FROM job_queue WHERE tenant_id = $1 AND job_id = $2::uuid",
        )
        .bind(TENANT)
        .bind(&claim.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            lease_epoch_secs, claim_epoch_secs,
            "the renewed execution lease is clamped to the immutable claim expiry"
        );
    })
    .await;
    })
    .await;
}

// ═════════════ 4. atomic reap + exact-generation journal sealing ═════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_reaper_seals_exactly_the_generation_it_requeued_and_rolls_back_a_failed_seal() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin, app) = migrated_schema("reap").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let specs = ci_job_spec_store(app.clone());
        let region_store = ci_region_queue_store_test_support(admin.clone());
        let now = chrono::Utc::now().timestamp();

        // Reapable: leased, lease lapsed, claim window still open. Its CURRENT generation is
        // epoch 2; an OLDER generation (epoch 1) also has an unresolved phase and must be left
        // alone, as must a phase the worker already measured.
        let reapable = seed_leased_generation(
            &admin,
            &specs,
            500,
            "reap-lane",
            2,
            &uuid(0x60, 500),
            now - 100,
            now + 80_000,
            now - 1,
            true,
        )
        .await;
        insert_parent_attempt(
            &admin,
            &reapable.job_id,
            &reapable.wf_run_id,
            &uuid(0x10, 500),
            "runner-500",
            1,
            &uuid(0x62, 500),
            now - 200,
            now + 79_000,
        )
        .await;
        insert_started_phase(
            &admin,
            &reapable.job_id,
            2,
            &uuid(0x60, 500),
            "checkout_transport",
        )
        .await;
        insert_started_phase(
            &admin,
            &reapable.job_id,
            2,
            &uuid(0x60, 500),
            "checkout_materialization",
        )
        .await;
        insert_started_phase(
            &admin,
            &reapable.job_id,
            1,
            &uuid(0x62, 500),
            "checkout_transport",
        )
        .await;
        sqlx::query(
            "UPDATE ci_job_prelaunch_usage
             SET status = 'measured', exact_cpu_seconds = 3, exact_mem_byte_seconds = 4,
                 resolved_at = statement_timestamp()
             WHERE tenant_id = $1 AND job_id = $2::uuid AND lease_epoch = 2
               AND phase = 'checkout_materialization'",
        )
        .bind(TENANT)
        .bind(&reapable.job_id)
        .execute(&admin)
        .await
        .unwrap();
        // The job's public surface is `running` so the reap's surface reset is exercised too.
        sqlx::query(
            "UPDATE ci_job SET state = 'running' WHERE tenant_id = $1 AND job_id = $2::uuid",
        )
        .bind(TENANT)
        .bind(&reapable.job_id)
        .execute(&admin)
        .await
        .unwrap();

        // A LIVE neighbour whose lease has not lapsed: nothing about it may change.
        let live = seed_leased_generation(
            &admin,
            &specs,
            501,
            "live-lane",
            1,
            &uuid(0x60, 501),
            now - 10,
            now + 80_000,
            now + 10_000,
            true,
        )
        .await;
        insert_started_phase(
            &admin,
            &live.job_id,
            1,
            &uuid(0x60, 501),
            "checkout_transport",
        )
        .await;

        // ── (a) an injected seal failure rolls the COMPLETE sweep back ──
        admin
            .execute(
                "CREATE FUNCTION myelin_test_refuse_seal() RETURNS trigger LANGUAGE plpgsql AS $$
                 BEGIN
                   IF NEW.status = 'sealed_ceiling' THEN
                     RAISE EXCEPTION 'injected seal failure';
                   END IF;
                   RETURN NEW;
                 END $$;
                 CREATE TRIGGER myelin_test_refuse_seal
                 BEFORE UPDATE ON ci_job_prelaunch_usage
                 FOR EACH ROW EXECUTE FUNCTION myelin_test_refuse_seal();",
            )
            .await
            .unwrap();
        let failed = region_store.reap(REGION).await;
        assert!(failed.is_err(), "the injected seal failure must surface");
        let (state, owner, epoch, nonce): (String, Option<String>, i64, Option<String>) =
            sqlx::query_as(
                "SELECT state, lease_owner, lease_epoch, claim_nonce::text
                 FROM job_queue WHERE tenant_id = $1 AND job_id = $2::uuid",
            )
            .bind(TENANT)
            .bind(&reapable.job_id)
            .fetch_one(&admin)
            .await
            .unwrap();
        assert_eq!(state, "leased", "the requeue rolled back with the seal");
        assert_eq!(owner.as_deref(), Some("runner-500"));
        assert_eq!(epoch, 2);
        assert_eq!(nonce.as_deref(), Some(uuid(0x60, 500).as_str()));
        assert_eq!(
            phase_status(&admin, &reapable.job_id, 2, "checkout_transport").await,
            "started",
            "the phase is unchanged after the rolled-back sweep"
        );
        let surface: String = sqlx::query_scalar(
            "SELECT state FROM ci_job WHERE tenant_id = $1 AND job_id = $2::uuid",
        )
        .bind(TENANT)
        .bind(&reapable.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(surface, "running", "the surface reset rolled back too");

        // ── (b) the next sweep, with the injection removed, commits all three effects ──
        admin
            .execute(
                "DROP TRIGGER myelin_test_refuse_seal ON ci_job_prelaunch_usage;
                 DROP FUNCTION myelin_test_refuse_seal();",
            )
            .await
            .unwrap();
        assert_eq!(region_store.reap(REGION).await.unwrap(), 1);
        let (state, owner, nonce): (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT state, lease_owner, claim_nonce::text
             FROM job_queue WHERE tenant_id = $1 AND job_id = $2::uuid",
        )
        .bind(TENANT)
        .bind(&reapable.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(state, "queued");
        assert_eq!(owner, None);
        assert_eq!(nonce, None);
        assert_eq!(
            phase_status(&admin, &reapable.job_id, 2, "checkout_transport").await,
            "sealed_ceiling",
            "the reaped generation's unresolved phase is sealed in the same transaction"
        );
        assert_eq!(
            phase_status(&admin, &reapable.job_id, 2, "checkout_materialization").await,
            "measured",
            "an already-measured phase is never overwritten by the seal"
        );
        assert_eq!(
            phase_status(&admin, &reapable.job_id, 1, "checkout_transport").await,
            "started",
            "a NEIGHBOURING generation of the same job is never touched"
        );
        assert_eq!(
            phase_status(&admin, &live.job_id, 1, "checkout_transport").await,
            "started",
            "a job whose lease has not lapsed is never touched"
        );
        let surface: String = sqlx::query_scalar(
            "SELECT state FROM ci_job WHERE tenant_id = $1 AND job_id = $2::uuid",
        )
        .bind(TENANT)
        .bind(&reapable.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            surface, "queued",
            "the surface is reset after a successful seal"
        );

        // ── (c) a replacement attempt settles with the old generation already ceiling-sealed ──
        // Resolve the older still-`started` generation away first (it stands in for the
        // independent deadline sealer's work), then admit a fresh generation and settle.
        sqlx::query(
            "UPDATE ci_job_prelaunch_usage
             SET status = 'sealed_ceiling', resolved_at = statement_timestamp()
             WHERE tenant_id = $1 AND job_id = $2::uuid AND lease_epoch = 1",
        )
        .bind(TENANT)
        .bind(&reapable.job_id)
        .execute(&admin)
        .await
        .unwrap();
        let replacement_nonce = uuid(0x63, 500);
        insert_parent_attempt(
            &admin,
            &reapable.job_id,
            &reapable.wf_run_id,
            &uuid(0x10, 500),
            "runner-replacement",
            3,
            &replacement_nonce,
            now + 1,
            now + 80_001,
        )
        .await;
        insert_started_phase(
            &admin,
            &reapable.job_id,
            3,
            &replacement_nonce,
            "checkout_transport",
        )
        .await;
        sqlx::query(
            "UPDATE ci_job_prelaunch_usage
             SET status = 'measured', exact_cpu_seconds = 1, exact_mem_byte_seconds = 2,
                 resolved_at = statement_timestamp()
             WHERE tenant_id = $1 AND job_id = $2::uuid AND lease_epoch = 3",
        )
        .bind(TENANT)
        .bind(&reapable.job_id)
        .execute(&admin)
        .await
        .unwrap();

        let mut settle = app.begin().await.unwrap();
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true),
                    set_config('myelin.region', $2, true)",
        )
        .bind(TENANT)
        .bind(REGION)
        .execute(&mut *settle)
        .await
        .unwrap();
        let accrual = resolve_prelaunch_usage_on_conn(
            &mut settle,
            CiPrelaunchSettlementIdentity {
                tenant_id: TENANT,
                region: REGION,
                job_id: &reapable.job_id,
                wf_run_id: &reapable.wf_run_id,
                ci_run_id: &uuid(0x10, 500),
                reserve_handle: RESERVE_HANDLE,
            },
            CiPrelaunchParentExpectation::Required,
            CiPrelaunchUnresolvedPolicy::Refuse,
        )
        .await
        .expect("a ceiling-sealed old generation never blocks the replacement's settlement");
        settle.commit().await.unwrap();
        assert_eq!(accrual.parent_attempts, 3);
        assert_eq!(accrual.measured_phases, 2, "the two measured phases");
        assert_eq!(accrual.sealed_phases, 2, "both ceiling-sealed phases");
        assert_eq!(
            accrual.usage.cpu_seconds,
            10 + 10 + 3 + 1,
            "two ceilings plus two exact measurements, counted exactly once each"
        );
    })
    .await;
    })
    .await;
}

// ═════════════ 5. the rolling upgrade of an already-populated queue ══════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_claim_window_expand_upgrades_a_populated_queue_without_touching_existing_rows() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let schema = schema_name("upgrade");
    let bootstrap = pinned_pool(&admin_url(), "public").await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap
        .execute(format!("CREATE SCHEMA {schema} AUTHORIZATION myelin_admin").as_str())
        .await
        .unwrap();
    let cleanup = bootstrap.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup, &schema_for_cleanup, || async move {
        let admin = pinned_pool(&admin_url(), &schema).await;
        // The PRE-expand shape: the frozen create plus every previously shipped queue ALTER.
        for ddl in [
            myelin_ci_controlplane::CREATE_JOB_QUEUE_DDL,
            myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
            myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL,
            myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
            myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL,
        ] {
            admin.execute(ddl).await.expect("pre-expand job_queue shape");
        }
        sqlx::query(
            "INSERT INTO job_queue (tenant_id, region, job_id, run_id, lane, trust_tier,
                                    fair_key, idem_token, state, stage)
             VALUES ($1, $2, $3::uuid, $4::uuid, 'batch', 'trusted', 'k', 'legacy', 'queued', 'build')",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(uuid(0x40, 600))
        .bind(uuid(0x20, 600))
        .execute(&admin)
        .await
        .expect("a legacy row exists before the expand");

        // The expand + its online validation.
        admin
            .execute(myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL)
            .await
            .expect("the expand applies to a populated hot table");
        admin
            .execute(myelin_ci_controlplane::VALIDATE_JOB_QUEUE_CLAIM_WINDOW_DDL)
            .await
            .expect("the bounded CHECK validates against the existing rows");
        // Re-applying the expand is a no-op (several fixtures replay this exact DDL text).
        admin
            .execute(myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL)
            .await
            .expect("the expand is idempotent");

        let legacy: Option<i64> = sqlx::query_scalar(
            "SELECT claim_window_secs FROM job_queue WHERE tenant_id = $1 AND idem_token = 'legacy'",
        )
        .bind(TENANT)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(legacy, None, "the pre-expand row survives as a legacy NULL");

        // The bound is enforced in both directions, at exactly the Rust maximum.
        for (window, admitted) in [
            (1_i64, true),
            (MAX_CI_JOB_CLAIM_WINDOW_SECS as i64, true),
            (0, false),
            (MAX_CI_JOB_CLAIM_WINDOW_SECS as i64 + 1, false),
        ] {
            let result = sqlx::query(
                "UPDATE job_queue SET claim_window_secs = $1 WHERE tenant_id = $2 AND idem_token = 'legacy'",
            )
            .bind(window)
            .bind(TENANT)
            .execute(&admin)
            .await;
            assert_eq!(
                result.is_ok(),
                admitted,
                "claim window {window} admitted={admitted}"
            );
        }
    })
    .await;
    })
    .await;
}

// ═════════════ 6. the claim-window expand refuses a divergent same-named constraint ══════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_claim_window_expand_refuses_a_divergent_same_named_constraint() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let schema = schema_name("divergent");
    let bootstrap = pinned_pool(&admin_url(), "public").await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap
        .execute(format!("CREATE SCHEMA {schema} AUTHORIZATION myelin_admin").as_str())
        .await
        .unwrap();
    let cleanup = bootstrap.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup, &schema_for_cleanup, || async move {
        let admin = pinned_pool(&admin_url(), &schema).await;
        for ddl in [
            myelin_ci_controlplane::CREATE_JOB_QUEUE_DDL,
            myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
            myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL,
            myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
            myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL,
        ] {
            admin.execute(ddl).await.expect("pre-expand job_queue shape");
        }
        // A hand-patched / divergently-deployed constraint under the SAME name, with a WIDER bound
        // than Rust's authority. Swallowing `duplicate_object` would silently adopt it, and
        // `ci_0020f` would then VALIDATE the wrong ceiling.
        admin
            .execute(
                "ALTER TABLE job_queue ADD COLUMN claim_window_secs bigint;
                 ALTER TABLE job_queue
                   ADD CONSTRAINT job_queue_claim_window_range
                   CHECK (claim_window_secs BETWEEN 1 AND 999999) NOT VALID;",
            )
            .await
            .expect("seed the divergent constraint");

        let refused = admin
            .execute(myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL)
            .await
            .expect_err("the expand must refuse a divergent same-named constraint");
        let message = refused.to_string();
        assert!(
            message.contains("DIVERGENT definition"),
            "the refusal must name the divergence; got: {message}"
        );
        assert!(
            message.contains("999999"),
            "the refusal must show the constraint it found; got: {message}"
        );

        // The divergent bound is still in place — nothing was silently adopted or rewritten.
        let still_divergent: String = sqlx::query_scalar(
            "SELECT pg_get_constraintdef(oid) FROM pg_constraint
             WHERE conrelid = 'job_queue'::regclass AND conname = 'job_queue_claim_window_range'",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(still_divergent.contains("999999"));

        // Replacing it with the EXPECTED constraint makes the same DDL idempotent again, both
        // before and after `ci_0020f` strips the NOT VALID marker.
        admin
            .execute(
                "ALTER TABLE job_queue DROP CONSTRAINT job_queue_claim_window_range;
                 ALTER TABLE job_queue
                   ADD CONSTRAINT job_queue_claim_window_range
                   CHECK (claim_window_secs BETWEEN 1 AND 88800) NOT VALID;",
            )
            .await
            .unwrap();
        admin
            .execute(myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL)
            .await
            .expect("the matching NOT VALID constraint is accepted as already-applied");
        admin
            .execute(myelin_ci_controlplane::VALIDATE_JOB_QUEUE_CLAIM_WINDOW_DDL)
            .await
            .unwrap();
        admin
            .execute(myelin_ci_controlplane::ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL)
            .await
            .expect("the same constraint post-VALIDATE is still recognized as already-applied");
    })
    .await;
    })
    .await;
}
