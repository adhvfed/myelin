#![cfg(feature = "integration")]

use base64::Engine as _;
use myelin_ci_controlplane::surfacing_store::canonical_visible_repo_refs;
use myelin_ci_controlplane::{
    ci_controlplane_migrations, CiRunStore, DurableLogPersist, LogPipelineSink,
};
use myelin_ci_sandbox::FirehoseSink;
use myelin_config::MyelinConfig;
use myelin_edge::repo_authz::{GrantBackedRepos, RepoPermission};
use myelin_edge::{
    register_ci, AuthenticatedActionPolicy, DurableCiReadApi, DurableGitBackend, EdgeError,
    EdgeRequest, Gateway, RepoAuthorizer,
};
use myelin_git::core::RepoLoc;
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, CredentialAudience,
    CredentialPurpose, HumanSsoAuthenticator, PasetoCapabilityVerifier, PrincipalStore,
    RevocationStore,
};
use myelin_storage::s3blob::S3BlobStore;
use myelin_storage::{
    with_tenant_tx, BlobStore, ContentHash, FsBlobStore, KmsEngine, PgError, PgMigrator,
    TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TENANT: &str = "acme";
const REGION: &str = "eu-north";
const VISIBLE_RUN: &str = "81000000-0000-4000-8000-000000000001";
const HIDDEN_RUN: &str = "81000000-0000-4000-8000-000000000002";
const ABSENT_RUN: &str = "81000000-0000-4000-8000-000000000003";
const VISIBLE_JOB: &str = "85000000-0000-4000-8000-000000000001";
const ABSENT_JOB: &str = "85000000-0000-4000-8000-000000000002";
const CORRUPT_JOB: &str = "85000000-0000-4000-8000-000000000003";
const HIDDEN_JOB: &str = "85000000-0000-4000-8000-000000000004";
const BOUNDARY_JOB: &str = "85000000-0000-4000-8000-000000000005";
const SCHEME: &str = "agent";
const GOLDEN_NEWEST_RUN: &str = "91000000-0000-4000-8000-000000000001";
const GOLDEN_OLDER_RUN: &str = "91000000-0000-4000-8000-000000000002";
const GOLDEN_FAILED_JOB: &str = "92000000-0000-4000-8000-000000000001";
const GOLDEN_LIVE_JOB: &str = "92000000-0000-4000-8000-000000000002";

static SCHEMA_SEQ: AtomicU64 = AtomicU64::new(0);
static SCHEMA_SETUP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct RevocableRepoAuthorizer {
    grants: GrantBackedRepos,
    alpha_enabled: Arc<AtomicBool>,
}

impl RepoAuthorizer for RevocableRepoAuthorizer {
    fn authorize_repo_permission(
        &self,
        principal: &Principal,
        repo: &RepoLoc,
        permission: RepoPermission,
    ) -> bool {
        if repo.repo == "alpha"
            && permission == RepoPermission::Pull
            && !self.alpha_enabled.load(Ordering::SeqCst)
        {
            return false;
        }
        self.grants
            .authorize_repo_permission(principal, repo, permission)
    }
}

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    std::env::var("DATABASE_MIGRATION_URL").unwrap_or_else(|_| {
        app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
    })
}

fn schema_name() -> String {
    format!(
        "ci_http_surface_{}_{}",
        std::process::id(),
        SCHEMA_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

async fn pool(url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _| {
            let schema = schema.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("connect to dev Postgres (is the stack up?)")
}

async fn setup_schema(admin: &PgPool, schema: &str) {
    let _setup_guard = SCHEMA_SETUP_LOCK.lock().await;
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop stale isolated schema");
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated schema");
    PgMigrator::apply(admin, &myelin_storage::foundation_migrations())
        .await
        .expect("apply outbox foundation migrations in the isolated schema");
    PgMigrator::apply(admin, &myelin_flow::migrations::migrations())
        .await
        .expect("apply flow migrations in the isolated schema");
    PgMigrator::apply(admin, &ci_controlplane_migrations())
        .await
        .expect("apply CI migrations in the isolated schema");
    let resolved_outbox_schema: String = sqlx::query_scalar(
        "SELECT namespace.nspname \
         FROM pg_class AS relation \
         JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace \
         WHERE relation.oid = to_regclass('outbox')",
    )
    .fetch_one(admin)
    .await
    .expect("resolve the transactional outbox used by the isolated test");
    assert_eq!(
        resolved_outbox_schema, schema,
        "CI HTTP tests must never fall through to the shared public outbox"
    );
    admin
        .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant isolated schema usage");
    admin
        .execute(
            format!("GRANT SELECT, INSERT, UPDATE ON ALL TABLES IN SCHEMA {schema} TO myelin_app")
                .as_str(),
        )
        .await
        .expect("grant fixture access");
}

struct CatchUnwind<F> {
    inner: std::pin::Pin<Box<F>>,
}

impl<F: std::future::Future> std::future::Future for CatchUnwind<F> {
    type Output = std::thread::Result<F::Output>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.inner.as_mut().poll(cx)
        })) {
            Ok(std::task::Poll::Ready(value)) => std::task::Poll::Ready(Ok(value)),
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Err(payload) => std::task::Poll::Ready(Err(payload)),
        }
    }
}

async fn with_schema_cleanup<Fut>(pool: &PgPool, schema: &str, body: impl FnOnce() -> Fut)
where
    Fut: std::future::Future<Output = ()>,
{
    let result = CatchUnwind {
        inner: Box::pin(body()),
    }
    .await;
    let _cleanup_guard = SCHEMA_SETUP_LOCK.lock().await;
    let cleanup = sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(pool)
        .await;
    match (result, cleanup) {
        (Ok(()), Ok(_)) => {}
        (Ok(()), Err(error)) => panic!("drop isolated CI HTTP schema `{schema}`: {error}"),
        (Err(payload), cleanup) => {
            if let Err(error) = cleanup {
                eprintln!(
                    "drop isolated CI HTTP schema `{schema}` while unwinding failed: {error}"
                );
            }
            std::panic::resume_unwind(payload);
        }
    }
}

async fn insert_run(app: &PgPool, run_id: &str, repo: &str, created_at: &str) {
    let run_id = run_id.to_string();
    let repo_ref = format!("myelin://{TENANT}/git/repo/{repo}");
    let created_at = created_at.to_string();
    with_tenant_tx(app, TENANT, REGION, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO ci_run (
                   tenant_id, region, run_id, project_id, repo_ref, commit_oid, pipeline_id,
                   wf_run_id, definition_snapshot, trigger_kind, trust_tier, state,
                   cost_settled, correlation_id, created_at
                 ) VALUES (
                   $1, $2, $3::uuid, '82000000-0000-4000-8000-000000000001'::uuid, $4,
                   '0123456789abcdef', '83000000-0000-4000-8000-000000000001'::uuid,
                   '84000000-0000-4000-8000-000000000001'::uuid, 'cas:test', 'push',
                   'trusted', 'running', FALSE, $3, $5::timestamptz
                 )",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(run_id)
            .bind(repo_ref)
            .bind(created_at)
            .execute(&mut *conn)
            .await
            .map(|_| ())
            .map_err(|error| PgError::Query(error.to_string()))
        })
    })
    .await
    .expect("insert tenant-scoped run");
}

async fn insert_job_and_segments(
    app: &PgPool,
    run_id: &str,
    job_id: &str,
    segments: &[(String, i64, i64)],
) {
    let run_id = run_id.to_string();
    let job_id = job_id.to_string();
    let segments = segments.to_vec();
    with_tenant_tx(app, TENANT, REGION, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO ci_job (
                   tenant_id, region, job_id, run_id, stage, name, needs, matrix_key,
                   spec_ref, state, attempt
                 ) VALUES (
                   $1, $2, $3::uuid, $4::uuid, 'build', 'archive', '{}', NULL,
                   'cas:spec', 'running', 1
                 )",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&job_id)
            .bind(&run_id)
            .execute(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
            for (sequence, (blob_ref, byte_start, byte_end)) in segments.into_iter().enumerate() {
                sqlx::query(
                    "INSERT INTO log_segment (
                       tenant_id, region, run_id, job_id, segment_seq, blob_ref,
                       byte_start, byte_end, pii_key_ref
                     ) VALUES ($1, $2, $3::uuid, $4::uuid, $5, $6, $7, $8, 'tenant:test')",
                )
                .bind(TENANT)
                .bind(REGION)
                .bind(&run_id)
                .bind(&job_id)
                .bind(sequence as i32)
                .bind(blob_ref)
                .bind(byte_start)
                .bind(byte_end)
                .execute(&mut *conn)
                .await
                .map_err(|error| PgError::Query(error.to_string()))?;
            }
            Ok(())
        })
    })
    .await
    .expect("insert tenant-scoped CI log archive");
}

async fn insert_segment(
    app: &PgPool,
    run_id: &str,
    job_id: &str,
    sequence: i32,
    blob_ref: String,
    byte_start: i64,
    byte_end: i64,
) {
    let run_id = run_id.to_string();
    let job_id = job_id.to_string();
    with_tenant_tx(app, TENANT, REGION, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO log_segment (
                   tenant_id, region, run_id, job_id, segment_seq, blob_ref,
                   byte_start, byte_end, pii_key_ref
                 ) VALUES ($1, $2, $3::uuid, $4::uuid, $5, $6, $7, $8, 'tenant:test')",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(run_id)
            .bind(job_id)
            .bind(sequence)
            .bind(blob_ref)
            .bind(byte_start)
            .bind(byte_end)
            .execute(&mut *conn)
            .await
            .map(|_| ())
            .map_err(|error| PgError::Query(error.to_string()))
        })
    })
    .await
    .expect("insert tenant-scoped CI log segment");
}

async fn insert_golden_ci_surface(
    app: &PgPool,
    blob_ref: String,
    log_len: i64,
    live_blob_ref: String,
    live_log_len: i64,
) {
    with_tenant_tx(app, TENANT, REGION, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO ci_run (
                   tenant_id, region, run_id, project_id, repo_ref, source_ref, commit_oid, pipeline_id,
                   wf_run_id, definition_snapshot, trigger_kind, trust_tier, state,
                   cost_settled, correlation_id, created_at, finished_at
                 ) VALUES
                 (
                   $1, $2, $3::uuid, '94000000-0000-4000-8000-000000000001'::uuid, $5,
                   'refs/heads/main', '0123456789abcdef',
                   '93000000-0000-4000-8000-000000000001'::uuid,
                   '95000000-0000-4000-8000-000000000001'::uuid, 'cas:golden-newest', 'push',
                   'trusted', 'failed', TRUE, $3, '2026-07-24T12:00:00Z'::timestamptz,
                   '2026-07-24T12:05:00Z'::timestamptz
                 ),
                 (
                   $1, $2, $4::uuid, '94000000-0000-4000-8000-000000000001'::uuid, $5,
                   NULL, 'fedcba9876543210', '93000000-0000-4000-8000-000000000001'::uuid,
                   '95000000-0000-4000-8000-000000000002'::uuid, 'cas:golden-older',
                   'pull_request', 'trusted', 'running', FALSE, $4,
                   '2026-07-24T11:00:00Z'::timestamptz, NULL
                 )",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(GOLDEN_NEWEST_RUN)
            .bind(GOLDEN_OLDER_RUN)
            .bind(format!("myelin://{TENANT}/git/repo/alpha"))
            .execute(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
            sqlx::query(
                "INSERT INTO ci_job (
                   tenant_id, region, job_id, run_id, stage, name, needs, matrix_key,
                   spec_ref, state, attempt, result_summary
                ) VALUES (
                   $1, $2, $3::uuid, $4::uuid, 'test', 'contract', '{}', NULL,
                   'cas:golden-job', 'failed', 1,
                   '{\"passed\":false,\"timed_out\":false,\"disposition\":\"workload_failed\",\"workload_started\":true,\"diagnostic\":\"Process exited with status 1.\"}'::jsonb
                 ), (
                   $1, $2, $5::uuid, $6::uuid, 'test', 'live-contract', '{}', NULL,
                   'cas:golden-live-job', 'running', 1, NULL
                 )",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(GOLDEN_FAILED_JOB)
            .bind(GOLDEN_NEWEST_RUN)
            .bind(GOLDEN_LIVE_JOB)
            .bind(GOLDEN_OLDER_RUN)
            .execute(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
            sqlx::query(
                "INSERT INTO log_segment (
                   tenant_id, region, run_id, job_id, segment_seq, blob_ref,
                   byte_start, byte_end, pii_key_ref
                ) VALUES
                  ($1, $2, $3::uuid, $4::uuid, 0, $5, 0, $6, 'tenant:golden'),
                  ($1, $2, $7::uuid, $8::uuid, 0, $9, 0, $10, 'tenant:golden-live')",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(GOLDEN_NEWEST_RUN)
            .bind(GOLDEN_FAILED_JOB)
            .bind(blob_ref)
            .bind(log_len)
            .bind(GOLDEN_OLDER_RUN)
            .bind(GOLDEN_LIVE_JOB)
            .bind(live_blob_ref)
            .bind(live_log_len)
            .execute(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
            sqlx::query(
                "INSERT INTO log_anchor (
                   tenant_id, region, run_id, job_id, step_id, byte_start, byte_end, status
                 ) VALUES ($1, $2, $3::uuid, $4::uuid, 'contract', 0, $5, 'failed')",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(GOLDEN_NEWEST_RUN)
            .bind(GOLDEN_FAILED_JOB)
            .bind(log_len)
            .execute(&mut *conn)
            .await
            .map(|_| ())
            .map_err(|error| PgError::Query(error.to_string()))
        })
    })
    .await
    .expect("insert shared golden CI surface");
}

async fn seed_log_route(admin: &PgPool, tenant: &str, region: &str, job_id: &str, run_id: &str) {
    sqlx::query(
        "INSERT INTO job_queue \
         (tenant_id,region,job_id,run_id,lane,labels,trust_tier,fair_key,idem_token,state) \
         VALUES ($1,$2,$3::uuid,$4::uuid,'batch','{}','trusted',$5,$6,'queued')",
    )
    .bind(tenant)
    .bind(region)
    .bind(job_id)
    .bind(run_id)
    .bind(format!("fair-{job_id}"))
    .bind(format!("idem-{job_id}"))
    .execute(admin)
    .await
    .expect("seed job_queue log route");
    sqlx::query(
        "INSERT INTO ci_job_spec (tenant_id,region,job_id,run_id,idem_token,spec) \
         VALUES ($1,$2,$3::uuid,$4::uuid,$5,jsonb_build_object('ci_run_id',$6))",
    )
    .bind(tenant)
    .bind(region)
    .bind(job_id)
    .bind(run_id)
    .bind(format!("idem-{job_id}"))
    .bind(run_id)
    .execute(admin)
    .await
    .expect("seed ci_job_spec log route (resume_async's job_queue join target)");
}

fn admin_scope() -> TenantScope {
    TenantScope::from_verified_token(
        &Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(TENANT.into()),
        ),
        Region(REGION.into()),
    )
}

fn authenticated_gateway(
    app: PgPool,
    git_root: &std::path::Path,
    blobs: Arc<dyn BlobStore + Send + Sync>,
) -> (
    Gateway,
    CellTokenAuthority,
    DurableCiReadApi,
    Principal,
    Arc<AtomicBool>,
) {
    let cell = CellTokenAuthority::from_seed(&[17; 32], &[19; 32]).expect("cell authority");
    let principals = PrincipalStore::new(Arc::new(KmsEngine::new()));
    principals
        .put_principal(
            &admin_scope(),
            PrincipalId("svc:viewer".into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .expect("seed viewer");
    principals
        .link_credential(
            &admin_scope(),
            SCHEME,
            "viewer-subject",
            &PrincipalId("svc:viewer".into()),
        )
        .expect("link viewer credential");
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        principals,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));

    let authz = ["alpha", "e\u{301}", "é", "😀", "z"]
        .into_iter()
        .fold(GrantBackedRepos::new(), |grants, repo| {
            grants.grant_read("svc:viewer", TENANT, repo)
        });
    let alpha_enabled = Arc::new(AtomicBool::new(true));
    let git = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(git_root).with_repo_authorizer(Arc::new(
            RevocableRepoAuthorizer {
                grants: authz,
                alpha_enabled: alpha_enabled.clone(),
            },
        )),
    );
    let viewer = Principal::new(
        TenantId(TENANT.into()),
        Region(REGION.into()),
        PrincipalId("svc:viewer".into()),
        PrincipalKind::Service,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let direct = DurableCiReadApi::new(
        CiRunStore::with_pg(app.clone()),
        git.clone(),
        blobs.clone(),
        tokio::runtime::Handle::current(),
    );
    let builder = Gateway::builder(authn, human, Arc::new(AuthenticatedActionPolicy::mounted()));
    let builder = register_ci(
        builder,
        CiRunStore::with_pg_surface_cursor_key(
            app,
            myelin_storage::SealKey::from_bytes([0x62; 32])
                .derive_service_key("myelin test edge ci run surface cursor v1"),
        ),
        git,
        blobs,
        tokio::runtime::Handle::current(),
    );
    (builder.build(), cell, direct, viewer, alpha_enabled)
}

fn mint(cell: &CellTokenAuthority) -> String {
    let exp_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time after epoch")
        .as_secs() as i64
        + 3_600;
    cell.mint(&CapabilityMintSpec {
        tenant: TENANT.into(),
        region: REGION.into(),
        subject_key: "viewer-subject".into(),
        jti: "ci-http-surface-viewer".into(),
        exp_unix,
        authority: vec!["edge.operator".into()],
        dpop_jkt: None,
        purpose: CredentialPurpose::OperatorBootstrap,
        audience: CredentialAudience::Edge,
    })
}

fn get(gateway: &Gateway, token: &str, path: &str) -> myelin_edge::EdgeResponse {
    get_query(gateway, token, path, "")
}

fn get_query(gateway: &Gateway, token: &str, path: &str, query: &str) -> myelin_edge::EdgeResponse {
    get_query_headers(gateway, token, path, query, Vec::new())
}

fn get_query_headers(
    gateway: &Gateway,
    token: &str,
    path: &str,
    query: &str,
    extra_headers: Vec<(String, String)>,
) -> myelin_edge::EdgeResponse {
    let mut headers = vec![
        ("Authorization".into(), format!("Bearer {token}")),
        ("x-myelin-token-scheme".into(), SCHEME.into()),
    ];
    headers.extend(extra_headers);
    gateway.handle(EdgeRequest::new("GET", path, query, headers, Vec::new()))
}

/// FRONTEND-CONTRACT: ci-read-dev-edge-parity
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_handlers_conjoin_repo_visibility_and_hide_denied_detail() {
    let schema = schema_name();
    let admin = pool(&admin_url(), &schema).await;
    setup_schema(&admin, &schema).await;
    with_schema_cleanup(&admin, &schema, || async {
        let app = pool(&app_url(), &schema).await;
        insert_run(&app, VISIBLE_RUN, "alpha", "2026-07-24T10:00:00Z").await;
        insert_run(&app, HIDDEN_RUN, "hidden", "2026-07-24T11:00:00Z").await;
        let blobs = Arc::new(FsBlobStore::new());
        let tenant = TenantId(TENANT.into());
        let first = b"alpha\n";
        let second = b"beta\n";
        let first_ref = blobs
            .put(&tenant, first)
            .expect("store first sealed log segment")
            .to_multihash_string();
        let second_ref = blobs
            .put(&tenant, second)
            .expect("store second sealed log segment")
            .to_multihash_string();
        insert_job_and_segments(
            &app,
            VISIBLE_RUN,
            VISIBLE_JOB,
            &[
                (first_ref.clone(), 0, first.len() as i64),
                (
                    second_ref.clone(),
                    first.len() as i64,
                    (first.len() + second.len()) as i64,
                ),
            ],
        )
        .await;
        insert_job_and_segments(
            &app,
            HIDDEN_RUN,
            HIDDEN_JOB,
            &[(
                ContentHash::blake3(b"hidden archive").to_multihash_string(),
                0,
                14,
            )],
        )
        .await;
        insert_job_and_segments(
            &app,
            VISIBLE_RUN,
            CORRUPT_JOB,
            &[(
                ContentHash::blake3(b"not present").to_multihash_string(),
                0,
                11,
            )],
        )
        .await;
        let boundary_segments = (0..65)
            .map(|sequence| {
                let byte_start = if sequence == 64 {
                    257
                } else {
                    i64::from(sequence) * 4
                };
                (
                    ContentHash::blake3(format!("boundary-{sequence}").as_bytes())
                        .to_multihash_string(),
                    byte_start,
                    byte_start + 4,
                )
            })
            .collect::<Vec<_>>();
        insert_job_and_segments(&app, VISIBLE_RUN, BOUNDARY_JOB, &boundary_segments).await;

        let root = std::env::temp_dir().join(format!("{schema}_git"));
        let repo_dir = root.join(TENANT).join(REGION);
        std::fs::create_dir_all(repo_dir.join("alpha.git")).expect("create visible repo");
        std::fs::create_dir_all(repo_dir.join("hidden.git")).expect("create hidden repo");
        let (gateway, cell, direct, viewer, alpha_enabled) =
            authenticated_gateway(app.clone(), &root, blobs.clone());
        let token = mint(&cell);

        let list = get(&gateway, &token, "/v1/ci/runs");
        assert_eq!(list.status(), 200);
        let list_body = list.json_body().expect("list JSON");
        let items = list_body["items"].as_array().expect("list items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["run_id"], VISIBLE_RUN);
        assert!(
            list_body.to_string().find(HIDDEN_RUN).is_none(),
            "the denied parent never enters the handler response"
        );

        let visible = get(&gateway, &token, &format!("/v1/ci/runs/{VISIBLE_RUN}"));
        assert_eq!(visible.status(), 200);
        assert_eq!(
            visible.json_body().expect("visible detail JSON")["run"]["run_id"],
            VISIBLE_RUN
        );
        assert_eq!(
            direct
                .read_run(&viewer, VISIBLE_RUN)
                .expect("direct visible run"),
            visible.json_body().expect("HTTP visible detail JSON"),
            "HTTP and agent/MCP use the same permission-checked durable read adapter"
        );

        let hidden = get(&gateway, &token, &format!("/v1/ci/runs/{HIDDEN_RUN}"));
        let absent = get(&gateway, &token, &format!("/v1/ci/runs/{ABSENT_RUN}"));
        assert_eq!(hidden.status(), 404);
        assert_eq!(absent.status(), 404);
        assert_eq!(
            hidden.json_body(),
            absent.json_body(),
            "denied and absent detail are the same public response"
        );
        assert!(matches!(
            direct.read_run(&viewer, HIDDEN_RUN),
            Err(EdgeError::NotFound(_))
        ));

        let log = get_query(
            &gateway,
            &token,
            &format!("/v1/ci/runs/{VISIBLE_RUN}/jobs/{VISIBLE_JOB}/log"),
            "start=3&limit=6",
        );
        assert_eq!(log.status(), 200);
        let log_body = log.json_body().expect("log JSON");
        assert_eq!(log_body["byte_start"], 3);
        assert_eq!(log_body["byte_end"], 9);
        assert_eq!(log_body["total_end"], 11);
        assert_eq!(log_body["next_offset"], 9);
        assert_eq!(log_body["encoding"], "base64");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(log_body["data"].as_str().expect("base64 log bytes"))
                .expect("decode log bytes"),
            b"ha\nbet"
        );
        assert_eq!(
            direct
                .read_log(&viewer, VISIBLE_RUN, VISIBLE_JOB, 3, 6)
                .expect("direct visible log"),
            log_body,
            "HTTP and agent/MCP share exact archived-log materialization"
        );

        let live_path = format!("/v1/ci/runs/{VISIBLE_RUN}/jobs/{VISIBLE_JOB}/log/live");
        let live = get_query_headers(
            &gateway,
            &token,
            &live_path,
            "",
            vec![("Last-Event-ID".into(), "0".into())],
        );
        let mut live_rx = match live {
            myelin_edge::EdgeResponse::Sse { sub, .. } => sub.into_receiver(),
            other => panic!(
                "visible live tail must stream, got status {}",
                other.status()
            ),
        };
        let first_live = tokio::time::timeout(Duration::from_secs(2), live_rx.recv())
            .await
            .expect("first live pointer deadline")
            .expect("first live pointer");
        let second_live = tokio::time::timeout(Duration::from_secs(2), live_rx.recv())
            .await
            .expect("second live pointer deadline")
            .expect("second live pointer");
        assert_eq!(first_live.id.as_deref(), Some("1"));
        assert_eq!(second_live.id.as_deref(), Some("2"));
        assert_eq!(first_live.event.as_deref(), Some("ci.log.appended"));
        let first_pointer: serde_json::Value =
            serde_json::from_str(&first_live.data).expect("first pointer JSON");
        assert_eq!(first_pointer["byte_start"], 0);
        assert_eq!(first_pointer["byte_end"], 6);
        assert!(
            first_pointer.get("blob_ref").is_none() && first_pointer.get("data").is_none(),
            "SSE carries a bounded archive coordinate, never a content address or log bytes"
        );

        let fresh = get(&gateway, &token, &live_path);
        let mut fresh_rx = match fresh {
            myelin_edge::EdgeResponse::Sse { sub, .. } => sub.into_receiver(),
            other => panic!(
                "fresh live tail must stream from the current head, got status {}",
                other.status()
            ),
        };
        let checkpoint = tokio::time::timeout(Duration::from_secs(2), fresh_rx.recv())
            .await
            .expect("fresh subscription checkpoint deadline")
            .expect("fresh subscription checkpoint");
        assert_eq!(checkpoint.event.as_deref(), Some("ci.log.ready"));
        assert_eq!(
            checkpoint.id.as_deref(),
            Some("2"),
            "a fresh subscription checkpoints the current head without backfilling"
        );

        let third = b"gamma\n";
        let third_ref = blobs
            .put(&TenantId(TENANT.into()), third)
            .expect("store third live segment")
            .to_multihash_string();
        insert_segment(&app, VISIBLE_RUN, VISIBLE_JOB, 2, third_ref, 11, 17).await;
        let third_live = tokio::time::timeout(Duration::from_secs(2), live_rx.recv())
            .await
            .expect("cross-service live pointer deadline")
            .expect("cross-service live pointer");
        assert_eq!(third_live.id.as_deref(), Some("3"));
        let fresh_third = tokio::time::timeout(Duration::from_secs(2), fresh_rx.recv())
            .await
            .expect("fresh subscription live pointer deadline")
            .expect("fresh subscription live pointer");
        assert_eq!(
            fresh_third.id.as_deref(),
            Some("3"),
            "a fresh subscription observes only segments appended after it opened"
        );

        sqlx::query(
            "UPDATE ci_job SET state = 'succeeded'
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid AND job_id = $4::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(VISIBLE_RUN)
        .bind(VISIBLE_JOB)
        .execute(&admin)
        .await
        .expect("terminalize visible job");
        let complete = tokio::time::timeout(Duration::from_secs(2), live_rx.recv())
            .await
            .expect("terminal live event deadline")
            .expect("terminal live event");
        assert_eq!(complete.event.as_deref(), Some("ci.log.complete"));
        assert_eq!(complete.id.as_deref(), Some("3"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&complete.data).unwrap()["byte_end"],
            17
        );

        let resumed = get_query_headers(
            &gateway,
            &token,
            &live_path,
            "",
            vec![("Last-Event-ID".into(), "1".into())],
        );
        let mut resumed_rx = match resumed {
            myelin_edge::EdgeResponse::Sse { sub, .. } => sub.into_receiver(),
            other => panic!(
                "resumed live tail must stream, got status {}",
                other.status()
            ),
        };
        let resumed_first = tokio::time::timeout(Duration::from_secs(2), resumed_rx.recv())
            .await
            .expect("resume deadline")
            .expect("resume pointer");
        assert_eq!(
            resumed_first.id.as_deref(),
            Some("2"),
            "resume backfills strictly after Last-Event-ID without duplicating cursor 1"
        );

        let revocable_path = format!("/v1/ci/runs/{VISIBLE_RUN}/jobs/{CORRUPT_JOB}/log/live");
        let revocable = get_query_headers(
            &gateway,
            &token,
            &revocable_path,
            "",
            vec![("Last-Event-ID".into(), "1".into())],
        );
        let mut revocable_rx = match revocable {
            myelin_edge::EdgeResponse::Sse { sub, .. } => sub.into_receiver(),
            other => panic!(
                "revocation proof tail must initially open, got status {}",
                other.status()
            ),
        };
        alpha_enabled.store(false, Ordering::SeqCst);
        insert_segment(
            &app,
            VISIBLE_RUN,
            CORRUPT_JOB,
            1,
            ContentHash::blake3(b"not exposed after revoke").to_multihash_string(),
            11,
            35,
        )
        .await;
        let revoked = tokio::time::timeout(Duration::from_secs(2), revocable_rx.recv())
            .await
            .expect("revoked stream closes by the next authorization poll");
        assert!(
            revoked.is_err(),
            "revoking parent Pull closes the open stream before the appended pointer is exposed"
        );
        alpha_enabled.store(true, Ordering::SeqCst);

        let hidden_live = get(
            &gateway,
            &token,
            &format!("/v1/ci/runs/{HIDDEN_RUN}/jobs/{HIDDEN_JOB}/log/live"),
        );
        let absent_live = get(
            &gateway,
            &token,
            &format!("/v1/ci/runs/{ABSENT_RUN}/jobs/{ABSENT_JOB}/log/live"),
        );
        assert_eq!(hidden_live.status(), 404);
        assert_eq!(hidden_live.json_body(), absent_live.json_body());

        sqlx::query(
            "DELETE FROM log_segment
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid
           AND job_id = $4::uuid AND segment_seq = 0",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(VISIBLE_RUN)
        .bind(VISIBLE_JOB)
        .execute(&admin)
        .await
        .expect("simulate retention floor advancement");
        let stale = get_query_headers(
            &gateway,
            &token,
            &live_path,
            "",
            vec![("Last-Event-ID".into(), "0".into())],
        );
        assert_eq!(stale.status(), 409);

        insert_segment(
            &app,
            VISIBLE_RUN,
            VISIBLE_JOB,
            0,
            first_ref,
            0,
            first.len() as i64,
        )
        .await;
        sqlx::query(
            "DELETE FROM log_segment
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid
           AND job_id = $4::uuid AND segment_seq = 1",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(VISIBLE_RUN)
        .bind(VISIBLE_JOB)
        .execute(&admin)
        .await
        .expect("simulate an internal archive gap");
        let discontinuous = get_query_headers(
            &gateway,
            &token,
            &live_path,
            "",
            vec![("Last-Event-ID".into(), "0".into())],
        );
        assert_eq!(
            discontinuous.status(),
            503,
            "a missing durable segment never becomes a successful cursor jump"
        );
        let discontinuous_predecessor = get_query_headers(
            &gateway,
            &token,
            &live_path,
            "",
            vec![("Last-Event-ID".into(), "2".into())],
        );
        assert_eq!(
            discontinuous_predecessor.status(),
            503,
            "a claimed cursor cannot bypass its missing internal predecessor"
        );

        sqlx::query(
            "DELETE FROM log_segment
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid AND job_id = $4::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(VISIBLE_RUN)
        .bind(VISIBLE_JOB)
        .execute(&admin)
        .await
        .expect("simulate full retention pruning");
        let fully_pruned = get_query_headers(
            &gateway,
            &token,
            &live_path,
            "",
            vec![("Last-Event-ID".into(), "0".into())],
        );
        assert_eq!(
            fully_pruned.status(),
            409,
            "an explicit cursor over a fully pruned archive requires resynchronization"
        );

        let boundary_path = format!("/v1/ci/runs/{VISIBLE_RUN}/jobs/{BOUNDARY_JOB}/log/live");
        let boundary = get_query_headers(
            &gateway,
            &token,
            &boundary_path,
            "",
            vec![("Last-Event-ID".into(), "0".into())],
        );
        let mut boundary_rx = match boundary {
            myelin_edge::EdgeResponse::Sse { sub, .. } => sub.into_receiver(),
            other => panic!(
                "the first bounded batch must open before the boundary gap, got status {}",
                other.status()
            ),
        };
        for expected_cursor in 1..=64 {
            let expected_id = expected_cursor.to_string();
            let pointer = tokio::time::timeout(Duration::from_secs(2), boundary_rx.recv())
                .await
                .expect("bounded batch pointer deadline")
                .expect("bounded batch pointer");
            assert_eq!(pointer.id.as_deref(), Some(expected_id.as_str()));
        }
        let boundary_closed = tokio::time::timeout(Duration::from_secs(2), boundary_rx.recv())
            .await
            .expect("cross-batch discontinuity closes the stream");
        assert!(
            boundary_closed.is_err(),
            "byte discontinuity at the producer's second poll fails closed"
        );

        let absent_job = get(
            &gateway,
            &token,
            &format!("/v1/ci/runs/{VISIBLE_RUN}/jobs/{ABSENT_JOB}/log"),
        );
        let hidden_log = get(
            &gateway,
            &token,
            &format!("/v1/ci/runs/{HIDDEN_RUN}/jobs/{HIDDEN_JOB}/log"),
        );
        assert_eq!(absent_job.status(), 404);
        assert_eq!(hidden_log.status(), 404);
        assert_eq!(
            absent_job.json_body(),
            hidden_log.json_body(),
            "a denied parent and absent child are the same public response"
        );

        let corrupt = get(
            &gateway,
            &token,
            &format!("/v1/ci/runs/{VISIBLE_RUN}/jobs/{CORRUPT_JOB}/log"),
        );
        assert_eq!(corrupt.status(), 503);
        let corrupt_body = corrupt.json_body().expect("generic unavailable JSON");
        assert_eq!(
            corrupt_body["error"]["message"],
            "CI log data is temporarily unavailable"
        );
        assert!(
            !corrupt_body.to_string().contains("blake3:"),
            "content addresses never enter the public error"
        );

        app.close().await;
    })
    .await;
    admin.close().await;
    std::fs::remove_dir_all(std::env::temp_dir().join(format!("{schema}_git")))
        .expect("remove isolated git fixture");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_sink_and_edge_resume_exactly_after_both_services_are_severed() {
    const RUN: &str = "81000000-0000-4000-8000-000000000011";
    const JOB: &str = "85000000-0000-4000-8000-000000000011";
    const FIRST: &[u8] = b"producer-before-sever\n";
    const SECOND: &[u8] = b"producer-after-restart\n";

    let schema = schema_name();
    let admin = pool(&admin_url(), &schema).await;
    setup_schema(&admin, &schema).await;
    with_schema_cleanup(&admin, &schema, || async {
        let app = pool(&app_url(), &schema).await;
        insert_run(&app, RUN, "alpha", "2026-07-24T13:00:00Z").await;
        insert_job_and_segments(&app, RUN, JOB, &[]).await;
        seed_log_route(&admin, TENANT, REGION, JOB, RUN).await;

        let blobs =
            S3BlobStore::connect(&MyelinConfig::dev().s3, tokio::runtime::Handle::current());
        let edge_blobs: Arc<dyn BlobStore + Send + Sync> = Arc::new(blobs.clone());
        let root = std::env::temp_dir().join(format!("{schema}_git"));
        let repo_dir = root.join(TENANT).join(REGION);
        std::fs::create_dir_all(repo_dir.join("alpha.git")).expect("create visible repo");
        let (gateway, cell, _, _, _) =
            authenticated_gateway(app.clone(), &root, edge_blobs.clone());
        let token = mint(&cell);
        let live_path = format!("/v1/ci/runs/{RUN}/jobs/{JOB}/log/live");

        let live = get(&gateway, &token, &live_path);
        let mut live_rx = match live {
            myelin_edge::EdgeResponse::Sse { sub, .. } => sub.into_receiver(),
            other => panic!(
                "fresh composed live tail returned status {}",
                other.status()
            ),
        };
        let ready = tokio::time::timeout(Duration::from_secs(2), live_rx.recv())
            .await
            .expect("initial checkpoint deadline")
            .expect("initial checkpoint");
        assert_eq!(ready.event.as_deref(), Some("ci.log.ready"));
        assert_eq!(ready.id.as_deref(), Some("0"));

        let runtime = tokio::runtime::Handle::current();
        let first_app = app.clone();
        let first_blobs = blobs.clone();
        std::thread::spawn(move || {
            let sink = LogPipelineSink::new(
                Region(REGION.into()),
                first_blobs,
                DurableLogPersist::with_pg(first_app, runtime),
            );
            sink.ship_frame(RUN, JOB, &TenantId(TENANT.into()), FIRST)
                .expect("first production sink commits before service loss");
        })
        .join()
        .expect("first producer service joins");

        let first_pointer = tokio::time::timeout(Duration::from_secs(2), live_rx.recv())
            .await
            .expect("first live pointer deadline")
            .expect("first live pointer");
        assert_eq!(first_pointer.event.as_deref(), Some("ci.log.appended"));
        assert_eq!(first_pointer.id.as_deref(), Some("1"));
        let first_data: serde_json::Value =
            serde_json::from_str(&first_pointer.data).expect("first pointer JSON");
        assert_eq!(first_data["byte_start"], 0);
        assert_eq!(first_data["byte_end"], FIRST.len() as i64);

        drop(live_rx);
        drop(gateway);

        let runtime = tokio::runtime::Handle::current();
        let second_app = app.clone();
        let second_blobs = blobs.clone();
        std::thread::spawn(move || {
            let sink = LogPipelineSink::new(
                Region(REGION.into()),
                second_blobs,
                DurableLogPersist::with_pg(second_app, runtime),
            );
            sink.ship_frame(RUN, JOB, &TenantId(TENANT.into()), SECOND)
                .expect("restarted production sink appends after the durable prefix");
            sink.finish(RUN, JOB, &TenantId(TENANT.into()), true)
                .expect("restarted production sink closes its durable anchor");
        })
        .join()
        .expect("restarted producer service joins");

        let (gateway, cell, _, _, _) = authenticated_gateway(app.clone(), &root, edge_blobs);
        let token = mint(&cell);
        let resumed = get_query_headers(
            &gateway,
            &token,
            &live_path,
            "",
            vec![("Last-Event-ID".into(), "1".into())],
        );
        let mut resumed_rx = match resumed {
            myelin_edge::EdgeResponse::Sse { sub, .. } => sub.into_receiver(),
            other => panic!(
                "resumed composed live tail returned status {}",
                other.status()
            ),
        };
        let second_pointer = tokio::time::timeout(Duration::from_secs(2), resumed_rx.recv())
            .await
            .expect("resumed pointer deadline")
            .expect("resumed pointer");
        assert_eq!(second_pointer.event.as_deref(), Some("ci.log.appended"));
        assert_eq!(
            second_pointer.id.as_deref(),
            Some("2"),
            "resume starts strictly after the acknowledged pointer; id 1 is not duplicated"
        );
        let second_data: serde_json::Value =
            serde_json::from_str(&second_pointer.data).expect("second pointer JSON");
        assert_eq!(second_data["byte_start"], FIRST.len() as i64);
        assert_eq!(second_data["byte_end"], (FIRST.len() + SECOND.len()) as i64);

        let archive = get_query(
            &gateway,
            &token,
            &format!("/v1/ci/runs/{RUN}/jobs/{JOB}/log"),
            &format!("start=0&limit={}", FIRST.len() + SECOND.len()),
        );
        assert_eq!(archive.status(), 200);
        let archive = archive.json_body().expect("composed archive JSON");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(archive["data"].as_str().expect("archive base64"))
            .expect("decode composed archive");
        assert_eq!(
            bytes,
            [FIRST, SECOND].concat(),
            "producer restart plus viewer resume loses and duplicates zero bytes"
        );

        let coordinates = sqlx::query_as::<_, (i32, i64, i64)>(
            "SELECT segment_seq, byte_start, byte_end FROM log_segment
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid AND job_id = $4::uuid
         ORDER BY segment_seq",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(RUN)
        .bind(JOB)
        .fetch_all(&admin)
        .await
        .expect("read composed durable coordinates");
        assert_eq!(
            coordinates,
            vec![
                (0, 0, FIRST.len() as i64),
                (1, FIRST.len() as i64, (FIRST.len() + SECOND.len()) as i64)
            ]
        );

        sqlx::query(
            "UPDATE ci_job SET state = 'succeeded'
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid AND job_id = $4::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(RUN)
        .bind(JOB)
        .execute(&admin)
        .await
        .expect("terminalize composed job");
        let complete = tokio::time::timeout(Duration::from_secs(2), resumed_rx.recv())
            .await
            .expect("composed completion deadline")
            .expect("composed completion");
        assert_eq!(complete.event.as_deref(), Some("ci.log.complete"));
        assert_eq!(complete.id.as_deref(), Some("2"));

        drop(resumed_rx);
        drop(gateway);
        app.close().await;
    })
    .await;
    admin.close().await;
    std::fs::remove_dir_all(std::env::temp_dir().join(format!("{schema}_git")))
        .expect("remove composed git fixture");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_ci_reads_match_the_shared_dev_edge_golden_vectors() {
    const GOLDEN: &str = include_str!("../../../contracts/ci-read-dev-edge.golden.json");
    let golden: serde_json::Value =
        serde_json::from_str(GOLDEN).expect("valid CI read golden JSON");
    assert_eq!(golden["contract_id"], "ci-read-dev-edge-parity");

    let schema = schema_name();
    let admin = pool(&admin_url(), &schema).await;
    setup_schema(&admin, &schema).await;
    with_schema_cleanup(&admin, &schema, || async {
        let app = pool(&app_url(), &schema).await;
        let blobs = Arc::new(FsBlobStore::new());
        let log = "prep\ncafé\nfailed\n".as_bytes();
        let blob_ref = blobs
            .put(&TenantId(TENANT.into()), log)
            .expect("store golden archived log")
            .to_multihash_string();
        let live_log = b"boot\n";
        let live_blob_ref = blobs
            .put(&TenantId(TENANT.into()), live_log)
            .expect("store golden live log")
            .to_multihash_string();
        insert_golden_ci_surface(
            &app,
            blob_ref,
            log.len() as i64,
            live_blob_ref,
            live_log.len() as i64,
        )
        .await;

        let root = std::env::temp_dir().join(format!("{schema}_git"));
        let repo_dir = root.join(TENANT).join(REGION);
        for repo in ["😀", "alpha", "é", "e\u{301}"] {
            std::fs::create_dir_all(repo_dir.join(format!("{repo}.git")))
                .expect("create golden visible repo");
        }
        let (gateway, cell, _direct, _viewer, _alpha_enabled) =
            authenticated_gateway(app.clone(), &root, blobs);
        let token = mint(&cell);
        let mut cursors = BTreeMap::<String, String>::new();

        for vector in golden["vectors"].as_array().expect("golden vectors") {
            let id = vector["id"].as_str().expect("vector id");
            let endpoint = vector["endpoint"].as_str().expect("vector endpoint");
            let request = &vector["request"];
            if vector["mutation"].as_str() == Some("add-visible-repo") {
                std::fs::create_dir_all(repo_dir.join("z.git"))
                    .expect("add golden visible repository");
            }
            if vector["mutation"].as_str() == Some("prune-live-log") {
                sqlx::query(
                    "DELETE FROM log_segment
                 WHERE tenant_id = $1 AND region = $2
                   AND run_id = $3::uuid AND job_id = $4::uuid",
                )
                .bind(TENANT)
                .bind(REGION)
                .bind(GOLDEN_OLDER_RUN)
                .bind(GOLDEN_LIVE_JOB)
                .execute(&admin)
                .await
                .expect("prune golden live-log cursor authority");
            }
            if endpoint == "visibility" {
                let visible = request["visible_repo_refs"]
                    .as_array()
                    .expect("visible repository vector")
                    .iter()
                    .map(|value| value.as_str().expect("visible repository ref").to_string())
                    .collect::<Vec<_>>();
                assert_eq!(
                    serde_json::json!({
                        "status": 200,
                        "visible_repo_refs": canonical_visible_repo_refs(&visible)
                            .expect("canonical visible repository set"),
                    }),
                    vector["expected"],
                    "golden vector {id}"
                );
                continue;
            }
            let after = vector["after"].as_str();
            let cursor = after.map(|source| {
                cursors
                    .get(source)
                    .unwrap_or_else(|| panic!("missing cursor from {source}"))
                    .as_str()
            });

            if endpoint == "live" {
                let path = format!(
                    "/v1/ci/runs/{}/jobs/{}/log/live",
                    request["run_id"].as_str().expect("live run id"),
                    request["job_id"].as_str().expect("live job id")
                );
                let extra_headers = request["last_event_id"]
                    .as_str()
                    .map(|value| vec![("Last-Event-ID".into(), value.to_string())])
                    .unwrap_or_default();
                let response = get_query_headers(&gateway, &token, &path, "", extra_headers);
                let status = response.status();
                let mut normalized = serde_json::Map::new();
                normalized.insert("status".into(), serde_json::json!(status));
                if status == 200 {
                    let expected_events = vector["expected"]["events"]
                        .as_array()
                        .expect("golden live events");
                    let mut receiver = match response {
                        myelin_edge::EdgeResponse::Sse { sub, .. } => sub.into_receiver(),
                        myelin_edge::EdgeResponse::Bytes { .. } => {
                            panic!("golden live vector {id} returned bytes")
                        }
                    };
                    let mut events = Vec::with_capacity(expected_events.len());
                    for _ in expected_events {
                        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
                            .await
                            .unwrap_or_else(|_| panic!("golden live vector {id} event deadline"))
                            .unwrap_or_else(|error| {
                                panic!("golden live vector {id} receive failed: {error}")
                            });
                        events.push(serde_json::json!({
                            "event": event.event,
                            "id": event.id,
                            "data": serde_json::from_str::<serde_json::Value>(&event.data)
                                .expect("golden live event JSON"),
                        }));
                    }
                    normalized.insert("events".into(), serde_json::Value::Array(events));
                }
                assert_eq!(
                    serde_json::Value::Object(normalized),
                    vector["expected"],
                    "golden vector {id}"
                );
                continue;
            }

            let response = match endpoint {
                "runs" => {
                    let mut query = vec![
                        format!("state={}", request["state"].as_str().expect("list state")),
                        format!("limit={}", request["limit"].as_u64().expect("list limit")),
                    ];
                    if let Some(cursor) = cursor {
                        query.push(format!("cursor={cursor}"));
                    }
                    get_query(&gateway, &token, "/v1/ci/runs", &query.join("&"))
                }
                "run" => get(
                    &gateway,
                    &token,
                    &format!(
                        "/v1/ci/runs/{}",
                        request["run_id"].as_str().expect("run id")
                    ),
                ),
                "log" => get_query(
                    &gateway,
                    &token,
                    &format!(
                        "/v1/ci/runs/{}/jobs/{}/log",
                        request["run_id"].as_str().expect("log run id"),
                        request["job_id"].as_str().expect("log job id")
                    ),
                    &format!(
                        "start={}&limit={}",
                        request["start"].as_u64().expect("log start"),
                        request["limit"].as_u64().expect("log limit")
                    ),
                ),
                other => panic!("unknown golden endpoint {other}"),
            };

            let mut normalized = serde_json::Map::new();
            normalized.insert("status".into(), serde_json::json!(response.status()));
            if response.status() == 200 {
                let body = response.json_body().expect("golden response JSON");
                for (key, value) in body.as_object().expect("golden response object") {
                    normalized.insert(key.clone(), value.clone());
                }
                if endpoint == "runs" {
                    let next = normalized["page"]["next_cursor"]
                        .as_str()
                        .map(str::to_string);
                    if let Some(next) = next {
                        assert!(next.starts_with("cr1_"), "canonical opaque CI cursor");
                        cursors.insert(id.to_string(), next);
                        normalized
                            .get_mut("page")
                            .expect("page")
                            .as_object_mut()
                            .expect("page object")
                            .insert("next_cursor".into(), serde_json::json!("cr1_<opaque>"));
                    }
                }
            }
            assert_eq!(
                serde_json::Value::Object(normalized),
                vector["expected"],
                "golden vector {id}"
            );
        }

        app.close().await;
    })
    .await;
    admin.close().await;
    std::fs::remove_dir_all(std::env::temp_dir().join(format!("{schema}_git")))
        .expect("remove golden git fixture");
}
