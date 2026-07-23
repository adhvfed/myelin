//! Live CT-005a proof through the production CI HTTP handlers and action policy.
#![cfg(feature = "integration")]

use base64::Engine as _;
use myelin_ci_controlplane::{ci_controlplane_migrations, CiRunStore};
use myelin_edge::repo_authz::GrantBackedRepos;
use myelin_edge::{
    register_ci, AuthenticatedActionPolicy, DurableGitBackend, EdgeRequest, Gateway,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, CredentialAudience,
    CredentialPurpose, HumanSsoAuthenticator, PasetoCapabilityVerifier, PrincipalStore,
    RevocationStore,
};
use myelin_storage::{
    with_tenant_tx, BlobStore, ContentHash, FsBlobStore, KmsEngine, PgError, TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const TENANT: &str = "ci_http_surface";
const REGION: &str = "eu-north";
const VISIBLE_RUN: &str = "81000000-0000-4000-8000-000000000001";
const HIDDEN_RUN: &str = "81000000-0000-4000-8000-000000000002";
const ABSENT_RUN: &str = "81000000-0000-4000-8000-000000000003";
const VISIBLE_JOB: &str = "85000000-0000-4000-8000-000000000001";
const ABSENT_JOB: &str = "85000000-0000-4000-8000-000000000002";
const CORRUPT_JOB: &str = "85000000-0000-4000-8000-000000000003";
const HIDDEN_JOB: &str = "85000000-0000-4000-8000-000000000004";
const SCHEME: &str = "agent";

static SCHEMA_SEQ: AtomicU64 = AtomicU64::new(0);

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
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop stale isolated schema");
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated schema");
    for migration in myelin_flow::migrations::migrations()
        .0
        .iter()
        .chain(ci_controlplane_migrations().0.iter())
    {
        admin
            .execute(migration.ddl)
            .await
            .unwrap_or_else(|error| panic!("apply {}: {error}", migration.id));
    }
    admin
        .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant isolated schema usage");
    admin
        .execute(
            format!("GRANT SELECT, INSERT ON ALL TABLES IN SCHEMA {schema} TO myelin_app").as_str(),
        )
        .await
        .expect("grant fixture access");
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
) -> (Gateway, CellTokenAuthority) {
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

    let authz = GrantBackedRepos::new().grant_read("svc:viewer", TENANT, "alpha");
    let git = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(git_root).with_repo_authorizer(Arc::new(authz)),
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
    (builder.build(), cell)
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
    gateway.handle(EdgeRequest::new(
        "GET",
        path,
        query,
        vec![
            ("Authorization".into(), format!("Bearer {token}")),
            ("x-myelin-token-scheme".into(), SCHEME.into()),
        ],
        Vec::new(),
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_handlers_conjoin_repo_visibility_and_hide_denied_detail() {
    let schema = schema_name();
    let admin = pool(&admin_url(), &schema).await;
    setup_schema(&admin, &schema).await;
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
            (first_ref, 0, first.len() as i64),
            (
                second_ref,
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

    let root = std::env::temp_dir().join(format!("{schema}_git"));
    let repo_dir = root.join(TENANT).join(REGION);
    std::fs::create_dir_all(repo_dir.join("alpha.git")).expect("create visible repo");
    std::fs::create_dir_all(repo_dir.join("hidden.git")).expect("create hidden repo");
    let (gateway, cell) = authenticated_gateway(app.clone(), &root, blobs);
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

    let hidden = get(&gateway, &token, &format!("/v1/ci/runs/{HIDDEN_RUN}"));
    let absent = get(&gateway, &token, &format!("/v1/ci/runs/{ABSENT_RUN}"));
    assert_eq!(hidden.status(), 404);
    assert_eq!(absent.status(), 404);
    assert_eq!(
        hidden.json_body(),
        absent.json_body(),
        "denied and absent detail are the same public response"
    );

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
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop isolated schema");
    admin.close().await;
    std::fs::remove_dir_all(root).expect("remove isolated git fixture");
}
