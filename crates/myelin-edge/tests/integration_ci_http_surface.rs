//! Live CT-005a proof through the production CI HTTP handlers and action policy.
#![cfg(feature = "integration")]

use base64::Engine as _;
use myelin_ci_controlplane::surfacing_store::canonical_visible_repo_refs;
use myelin_ci_controlplane::{ci_controlplane_migrations, CiRunStore};
use myelin_edge::repo_authz::GrantBackedRepos;
use myelin_edge::{
    register_ci, AuthenticatedActionPolicy, DurableCiReadApi, DurableGitBackend, EdgeError,
    EdgeRequest, Gateway,
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
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const TENANT: &str = "acme";
const REGION: &str = "eu-north";
const VISIBLE_RUN: &str = "81000000-0000-4000-8000-000000000001";
const HIDDEN_RUN: &str = "81000000-0000-4000-8000-000000000002";
const ABSENT_RUN: &str = "81000000-0000-4000-8000-000000000003";
const VISIBLE_JOB: &str = "85000000-0000-4000-8000-000000000001";
const ABSENT_JOB: &str = "85000000-0000-4000-8000-000000000002";
const CORRUPT_JOB: &str = "85000000-0000-4000-8000-000000000003";
const HIDDEN_JOB: &str = "85000000-0000-4000-8000-000000000004";
const SCHEME: &str = "agent";
const GOLDEN_NEWEST_RUN: &str = "91000000-0000-4000-8000-000000000001";
const GOLDEN_OLDER_RUN: &str = "91000000-0000-4000-8000-000000000002";
const GOLDEN_FAILED_JOB: &str = "92000000-0000-4000-8000-000000000001";

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

async fn insert_golden_ci_surface(app: &PgPool, blob_ref: String, log_len: i64) {
    with_tenant_tx(app, TENANT, REGION, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO ci_run (
                   tenant_id, region, run_id, project_id, repo_ref, commit_oid, pipeline_id,
                   wf_run_id, definition_snapshot, trigger_kind, trust_tier, state,
                   cost_settled, correlation_id, created_at, finished_at
                 ) VALUES
                 (
                   $1, $2, $3::uuid, '94000000-0000-4000-8000-000000000001'::uuid, $5,
                   '0123456789abcdef', '93000000-0000-4000-8000-000000000001'::uuid,
                   '95000000-0000-4000-8000-000000000001'::uuid, 'cas:golden-newest', 'push',
                   'trusted', 'failed', TRUE, $3, '2026-07-24T12:00:00Z'::timestamptz,
                   '2026-07-24T12:05:00Z'::timestamptz
                 ),
                 (
                   $1, $2, $4::uuid, '94000000-0000-4000-8000-000000000001'::uuid, $5,
                   'fedcba9876543210', '93000000-0000-4000-8000-000000000001'::uuid,
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
                   'cas:golden-job', 'failed', 1, '{\"message\":\"contract failed\"}'::jsonb
                 )",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(GOLDEN_FAILED_JOB)
            .bind(GOLDEN_NEWEST_RUN)
            .execute(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
            sqlx::query(
                "INSERT INTO log_segment (
                   tenant_id, region, run_id, job_id, segment_seq, blob_ref,
                   byte_start, byte_end, pii_key_ref
                 ) VALUES ($1, $2, $3::uuid, $4::uuid, 0, $5, 0, $6, 'tenant:golden')",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(GOLDEN_NEWEST_RUN)
            .bind(GOLDEN_FAILED_JOB)
            .bind(blob_ref)
            .bind(log_len)
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
) -> (Gateway, CellTokenAuthority, DurableCiReadApi, Principal) {
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
    let git = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(git_root).with_repo_authorizer(Arc::new(authz)),
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
    (builder.build(), cell, direct, viewer)
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
    let (gateway, cell, direct, viewer) = authenticated_gateway(app.clone(), &root, blobs);
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

/// FRONTEND-CONTRACT: ci-read-dev-edge-parity
///
/// The production provider executes the same committed request/response vectors as the TypeScript
/// dev Edge. Only the keyed cursor bytes are normalized; their scope-bound behavior remains part of
/// the vectors. Golden artifact: `contracts/ci-read-dev-edge.golden.json`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_ci_reads_match_the_shared_dev_edge_golden_vectors() {
    const GOLDEN: &str = include_str!("../../../contracts/ci-read-dev-edge.golden.json");
    let golden: serde_json::Value =
        serde_json::from_str(GOLDEN).expect("valid CI read golden JSON");
    assert_eq!(golden["contract_id"], "ci-read-dev-edge-parity");

    let schema = schema_name();
    let admin = pool(&admin_url(), &schema).await;
    setup_schema(&admin, &schema).await;
    let app = pool(&app_url(), &schema).await;
    let blobs = Arc::new(FsBlobStore::new());
    let log = "prep\ncafé\nfailed\n".as_bytes();
    let blob_ref = blobs
        .put(&TenantId(TENANT.into()), log)
        .expect("store golden archived log")
        .to_multihash_string();
    insert_golden_ci_surface(&app, blob_ref, log.len() as i64).await;

    let root = std::env::temp_dir().join(format!("{schema}_git"));
    let repo_dir = root.join(TENANT).join(REGION);
    for repo in ["😀", "alpha", "é", "e\u{301}"] {
        std::fs::create_dir_all(repo_dir.join(format!("{repo}.git")))
            .expect("create golden visible repo");
    }
    let (gateway, cell, _direct, _viewer) = authenticated_gateway(app.clone(), &root, blobs);
    let token = mint(&cell);
    let mut cursors = BTreeMap::<String, String>::new();

    for vector in golden["vectors"].as_array().expect("golden vectors") {
        let id = vector["id"].as_str().expect("vector id");
        let endpoint = vector["endpoint"].as_str().expect("vector endpoint");
        let request = &vector["request"];
        if vector["mutation"].as_str() == Some("add-visible-repo") {
            std::fs::create_dir_all(repo_dir.join("z.git")).expect("add golden visible repository");
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
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop isolated golden schema");
    admin.close().await;
    std::fs::remove_dir_all(root).expect("remove golden git fixture");
}
