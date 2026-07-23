//! Live CT-005a proof through the production CI HTTP handlers and action policy.
#![cfg(feature = "integration")]

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
use myelin_storage::{with_tenant_tx, KmsEngine, PgError, TenantScope};
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

fn authenticated_gateway(app: PgPool, git_root: &std::path::Path) -> (Gateway, CellTokenAuthority) {
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
    gateway.handle(EdgeRequest::new(
        "GET",
        path,
        "",
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

    let root = std::env::temp_dir().join(format!("{schema}_git"));
    let repo_dir = root.join(TENANT).join(REGION);
    std::fs::create_dir_all(repo_dir.join("alpha.git")).expect("create visible repo");
    std::fs::create_dir_all(repo_dir.join("hidden.git")).expect("create hidden repo");
    let (gateway, cell) = authenticated_gateway(app.clone(), &root);
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

    app.close().await;
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop isolated schema");
    admin.close().await;
    std::fs::remove_dir_all(root).expect("remove isolated git fixture");
}
