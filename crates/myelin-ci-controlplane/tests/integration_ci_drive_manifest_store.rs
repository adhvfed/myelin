//! Live PostgreSQL proof for the canonical, insert-only CI drive-manifest store.
#![cfg(feature = "integration")]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use myelin_ci_controlplane::{
    ci_job_authorization_context, ci_runner_identity_authorities, CiDriveManifestStore,
    CiDriveManifestV1, CiJobCredentialMinter, CiJobRuntimeAuthorityRequest, CiJobTokenIssueError,
    CiJobTokenIssuer, CiJobTokenRequest, CiManifestLaneV1, CiManifestLimitsV1,
    CiManifestSchedulingV1, CiManifestTrustTierV1, CiManifestWorkspaceV1, GrantedCiJobV1,
    LockedManifestCiJobTokenIssuer, ManifestBoundCiJobTokenAuthority,
    ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL, ALTER_JOB_QUEUE_ADD_COMPLETION_DDL, CI_PIPELINE_WF_TYPE,
    CREATE_CI_DRIVE_MANIFEST_DDL, CREATE_CI_RUN_DDL, CREATE_JOB_QUEUE_DDL,
};
use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind, JobSpecTemplate, MeterTarget, ResourceLimits,
    RunTokenCredential, TrustTier, WorkspaceSpec,
};
use myelin_config::MyelinConfig;
use myelin_storage::{
    cell_root_durable_migrations, identity_durable_migrations, PgMigrator, SealKey,
    SubstrateProvider,
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

const PROJECT_ID: &str = "44444444-4444-8444-8444-444444444444";

fn authority() -> CiJobRuntimeAuthorityRequest {
    CiJobRuntimeAuthorityRequest {
        tenant_id: "manifest-live".into(),
        region: "fr-par".into(),
        ci_run_id: "22222222-2222-8222-8222-222222222222".into(),
        wf_run_id: "11111111-1111-8111-8111-111111111111".into(),
        project_id: PROJECT_ID.into(),
        job_id: "33333333-3333-8333-8333-333333333333".into(),
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
    }
}

fn manifest() -> CiDriveManifestV1 {
    let repo_ref = "myelin://manifest-live/git/repo/core".to_string();
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
        commit_oid: "deadbeef".into(),
        run_ref: "myelin://manifest-live/ci/run/22222222-2222-8222-8222-222222222222".into(),
        started_at: "2026-07-21T12:34:56.000000Z".into(),
        trust_tier: CiManifestTrustTierV1::Trusted,
        check_attempts: BTreeMap::from([("build".into(), 9)]),
        merge_waiter: None,
        jobs: vec![GrantedCiJobV1 {
            job_id: "33333333-3333-8333-8333-333333333333".into(),
            stage: "build".into(),
            name: "build".into(),
            check_context: "build".into(),
            needs: Vec::new(),
            matrix_key: BTreeMap::new(),
            image: format!("registry.example/build@sha256:{}", "d".repeat(64)),
            command: vec!["/bin/true".into()],
            env: BTreeMap::new(),
            secret_handles: BTreeMap::new(),
            egress_allow: Vec::new(),
            limits: authority.limits.clone(),
            workspace: CiManifestWorkspaceV1 {
                repo_ref,
                commit_oid: "deadbeef".into(),
                read_only_root: true,
                tmpfs_scratch: true,
            },
            scheduling: CiManifestSchedulingV1 {
                lane: CiManifestLaneV1::Batch,
                labels: vec!["linux".into()],
                concurrency_group: None,
                fair_key: "project:core".into(),
            },
            reserve_handle: "reserve:live-run".into(),
            token_authority_handle: ManifestBoundCiJobTokenAuthority::handle_for(&authority),
            continue_on_error: false,
        }],
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

    let admin = pinned_pool(&admin_url(), &schema).await;
    let app = pinned_pool(&app_url(), &schema).await;
    PgMigrator::apply(&admin, &identity_durable_migrations())
        .await
        .expect("apply the durable Identity/S7 schema");
    PgMigrator::apply(&admin, &cell_root_durable_migrations())
        .await
        .expect("apply the durable cell-root schema");
    sqlx::raw_sql(&format!(
        "{CREATE_CI_RUN_DDL};
         {ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL};
         SELECT myelin_make_tenant_scoped('ci_run');
         {CREATE_CI_DRIVE_MANIFEST_DDL};
         SELECT myelin_make_tenant_scoped('ci_drive_manifest');
         {CREATE_JOB_QUEUE_DDL};
         {ALTER_JOB_QUEUE_ADD_COMPLETION_DDL};
         {ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL};
         {ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL};
         SELECT myelin_make_tenant_scoped('job_queue');"
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
    let mut exact_claim = claim(&expected);
    let persisted_claim_times: (i64, i64) = sqlx::query_as(
        "INSERT INTO job_queue (
           tenant_id, region, job_id, run_id, lane, labels, trust_tier, fair_key, idem_token,
           lease_owner, lease_expires, state, lease_epoch, claim_nonce, stage,
           claim_started_at, claim_expires_at
         ) VALUES (
           $1, $2, $3::uuid, $4::uuid, 'batch', ARRAY['linux'], 'trusted', $5, $6,
           $7, statement_timestamp() + interval '300 seconds', 'leased', $8, $9::uuid, 'build',
           statement_timestamp(), statement_timestamp() + interval '300 seconds'
         )
         RETURNING EXTRACT(EPOCH FROM claim_started_at)::bigint,
                   EXTRACT(EPOCH FROM claim_expires_at)::bigint",
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
    let signed = first_identity
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
    let spec = JobSpecTemplate::new(
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
            pids_max: expected.jobs[0].limits.pids_max,
            timeout_secs: expected.jobs[0].limits.timeout_secs,
        },
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        MeterTarget {
            reserve_id: expected.jobs[0].reserve_handle.clone(),
        },
        IdemToken(exact_claim.idem_token.clone()),
    )
    .unwrap()
    .resolve_with_authorization(signed, Some(ci_job_authorization_context(&exact_claim)));
    second_identity
        .launch_authorizer()
        .authorize(&spec)
        .expect("a reconstructed production verifier authorizes the exact durable claim");
    let launched_state: String = sqlx::query_scalar(
        "SELECT state FROM job_queue WHERE tenant_id = $1 AND job_id = $2::uuid",
    )
    .bind(&expected.tenant_id)
    .bind(&expected.jobs[0].job_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(launched_state, "running");

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
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&bare_admin)
        .await
        .unwrap();
    bare_admin.close().await;
}
