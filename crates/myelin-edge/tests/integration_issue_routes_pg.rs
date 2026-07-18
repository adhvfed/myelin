//! Live PostgreSQL + real HTTP proof for the production Issues route surface.
#![cfg(feature = "integration")]

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use myelin_config::{Mode, MyelinConfig};
use myelin_edge::issue_authz::reconcile_pending_issue_authorizations;
use myelin_edge::{
    register_issues, serve_edge, AuthenticatedActionPolicy, Gateway, StoreBackedIssueAuthorizer,
    MAX_ISSUE_JSON_BYTES,
};
use myelin_events::Timestamp;
use myelin_identity::{
    DataRole, FragmentAdmit, ObjectId, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
    RelName, RelationTuple, TupleDelta,
};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore, StoreBackedCheck, TupleStore,
};
use myelin_issues::events::ISSUE_CLOSED;
use myelin_issues::{issues_hot_tables, issues_migrations, PgIssueStore};
use myelin_storage::{
    all_durable_migrations, DurableTupleBacking, KmsEngine, PgBootstrap, TenantScope,
};
use myelin_substrate::HotTables;
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};

const REGION: &str = "fr-par";
const PROJECT_ID: &str = "11111111-1111-1111-1111-111111111111";
const TYPE_ID: &str = "22222222-2222-2222-2222-222222222222";
const SCHEME: &str = "agent";

fn admin_url() -> String {
    std::env::var("DATABASE_MIGRATION_URL")
        .unwrap_or_else(|_| "postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin".into())
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn suffix() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn principal(tenant: &str, id: &str, kind: PrincipalKind) -> Principal {
    Principal::new(
        TenantId::from_token(tenant),
        Region::new(REGION),
        PrincipalId(id.into()),
        kind,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn seed_login(store: &PrincipalStore, principal: &Principal, subject_key: &str) {
    let scope = TenantScope::from_verified_token(principal, principal.region.clone());
    store
        .put_principal(
            &scope,
            principal.principal_id.clone(),
            principal.kind.clone(),
            principal.data_role,
            principal.status,
            None,
        )
        .expect("seed route-test principal");
    store
        .link_credential(&scope, SCHEME, subject_key, &principal.principal_id)
        .expect("link route-test credential");
}

fn mint(cell: &CellTokenAuthority, principal: &Principal, subject_key: &str, jti: &str) -> String {
    cell.mint(&CapabilityMintSpec {
        tenant: principal.tenant.as_str().into(),
        region: principal.region.as_str().into(),
        subject_key: subject_key.into(),
        jti: jti.into(),
        exp_unix: now() + 3_600,
        authority: vec!["edge.operator".into()],
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::OperatorBootstrap,
        audience: myelin_identity_service::CredentialAudience::Edge,
    })
}

async fn spawn(gateway: Arc<Gateway>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = serve_edge(listener, gateway).await;
    });
    address
}

async fn open(
    address: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Vec<u8>,
) -> Response<Incoming> {
    let stream = TcpStream::connect(address).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, connection) = hyper::client::conn::http1::handshake(io).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "edge.test")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder
            .header("authorization", format!("Bearer {token}"))
            .header("x-myelin-token-scheme", SCHEME);
    }
    sender
        .send_request(builder.body(Full::new(Bytes::from(body))).unwrap())
        .await
        .unwrap()
}

async fn http(
    address: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Vec<u8>,
) -> (u16, serde_json::Value) {
    let response = open(address, method, path, token, body).await;
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn create_body(title: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "project_id": PROJECT_ID,
        "type_id": TYPE_ID,
        "prefix": "ENG",
        "title": title,
    }))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_issue_routes_are_scoped_leak_free_and_emit_once() {
    let mut config = MyelinConfig::from_env(Mode::DevDefaults).expect("dev config");
    config.region = REGION.into();
    let bootstrap = PgBootstrap::connect(config, 8)
        .await
        .expect("validate split database roles (is docker-compose.dev.yml up?)");
    bootstrap.migrate_foundation().await.unwrap();
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    bootstrap
        .migrate(&issues_migrations(), &issues_hot_tables())
        .await
        .unwrap();
    let provider = bootstrap.into_runtime().await.unwrap();

    let unique = suffix();
    let tenant = format!("edge_issues_{unique}");
    let foreign_tenant = format!("edge_issues_foreign_{unique}");
    let creator = principal(
        &tenant,
        &format!("human:creator:{unique}"),
        PrincipalKind::Human,
    );
    let intruder = principal(
        &tenant,
        &format!("human:intruder:{unique}"),
        PrincipalKind::Human,
    );
    let foreign = principal(
        &foreign_tenant,
        &format!("human:foreign:{unique}"),
        PrincipalKind::Human,
    );
    let worker = principal(
        &tenant,
        &format!("service:issues-reconciler:{unique}"),
        PrincipalKind::Service,
    );

    let tuples = TupleStore::with_pg(
        DurableTupleBacking::new(provider.clone()),
        tokio::runtime::Handle::current(),
    );
    let check = StoreBackedCheck::new(tuples.clone());
    for verdict in check.admit_issue_fragment() {
        assert!(matches!(verdict, FragmentAdmit::Admitted { .. }));
    }
    let creator_scope = TenantScope::from_verified_token(&creator, creator.region.clone());
    let tuples_for_seed = tuples.clone();
    let creator_for_seed = creator.clone();
    tokio::task::spawn_blocking(move || {
        tuples_for_seed.write_tuples(
            &creator_scope,
            &creator_for_seed,
            &[TupleDelta::Add(RelationTuple {
                object: ObjectId(format!("project:{PROJECT_ID}")),
                relation: RelName("reader".into()),
                subject: creator_for_seed.principal_id.clone(),
                caveat: None,
            })],
            None,
            None,
            Timestamp("2026-07-18T00:00:00Z".into()),
        )
    })
    .await
    .unwrap()
    .unwrap();

    let kms = Arc::new(KmsEngine::new());
    let issue_authorizer = StoreBackedIssueAuthorizer::new(check.clone());
    let issue_store = Arc::new(PgIssueStore::new(
        provider.clone(),
        kms.clone(),
        issue_authorizer.clone(),
    ));

    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).unwrap();
    let directory = PrincipalStore::new(kms.clone());
    seed_login(&directory, &creator, "creator-subject");
    seed_login(&directory, &intruder, "intruder-subject");
    seed_login(&directory, &foreign, "foreign-subject");
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        directory,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(kms)));
    let builder = register_issues(
        Gateway::builder(
            authn,
            human_login,
            Arc::new(AuthenticatedActionPolicy::mounted()),
        )
        .default_token_scheme(SCHEME),
        issue_store.clone(),
        issue_authorizer,
        tokio::runtime::Handle::current(),
    );
    let gateway = Arc::new(builder.build());
    let address = spawn(gateway).await;

    let creator_token = mint(&cell, &creator, "creator-subject", "issues-creator");
    let intruder_token = mint(&cell, &intruder, "intruder-subject", "issues-intruder");
    let foreign_token = mint(&cell, &foreign, "foreign-subject", "issues-foreign");

    let (status, _) = http(address, "GET", "/v1/issues", None, Vec::new()).await;
    assert_eq!(status, 401);
    let (status, body) = http(
        address,
        "POST",
        "/v1/issues",
        Some(&intruder_token),
        create_body("must not exist"),
    )
    .await;
    assert_eq!(status, 404, "project denial is leak-free: {body}");
    let (status, _) = http(
        address,
        "POST",
        "/v1/issues",
        Some(&creator_token),
        vec![b'x'; MAX_ISSUE_JSON_BYTES + 1],
    )
    .await;
    assert_eq!(status, 413);
    let (status, _) = http(
        address,
        "POST",
        "/v1/issues",
        Some(&creator_token),
        serde_json::to_vec(&serde_json::json!({
            "project_id": PROJECT_ID,
            "type_id": TYPE_ID,
            "prefix": "ENG",
            "title": "scope smuggling",
            "tenant": foreign_tenant,
        }))
        .unwrap(),
    )
    .await;
    assert_eq!(status, 400);

    let title = "private production route title";
    let (status, receipt) = http(
        address,
        "POST",
        "/v1/issues",
        Some(&creator_token),
        create_body(title),
    )
    .await;
    assert_eq!(status, 202, "create returns a staged receipt: {receipt}");
    assert_eq!(receipt["authorization"]["status"], "pending");
    let issue_id = receipt["issue"]["id"].as_str().unwrap().to_string();
    let issue_path = format!("/v1/issues/{issue_id}");
    let close_path = format!("{issue_path}/close");

    let pending_view = http(
        address,
        "GET",
        &issue_path,
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(pending_view.0, 404, "pending issue stays invisible");
    let pending_list = http(
        address,
        "GET",
        "/v1/issues",
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(pending_list.0, 503);
    assert!(!pending_list.1.to_string().contains(title));
    assert!(!pending_list.1.to_string().contains(&issue_id));

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .unwrap();
    sqlx::query("DELETE FROM authz_projection_state WHERE tenant_id = $1 AND region = $2")
        .bind(&tenant)
        .bind(REGION)
        .execute(&admin)
        .await
        .unwrap();
    let missing_list = http(
        address,
        "GET",
        "/v1/issues",
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(missing_list.0, 503);
    assert_eq!(missing_list.1, pending_list.1);

    let outcomes = reconcile_pending_issue_authorizations(&issue_store, &check, &worker, 100)
        .await
        .unwrap();
    assert_eq!(outcomes.len(), 1);
    assert!(outcomes[0].1.as_ref().unwrap().newly_activated);
    issue_store
        .rebuild_effective_issue_view(&worker)
        .await
        .unwrap();

    let (status, issue) = http(
        address,
        "GET",
        &issue_path,
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(issue["title"], title);
    let (status, page) = http(
        address,
        "GET",
        "/v1/issues?limit=1",
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["id"], issue_id);
    let (status, empty_page) = http(
        address,
        "GET",
        "/v1/issues",
        Some(&intruder_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 200);
    assert!(empty_page["items"].as_array().unwrap().is_empty());
    let (status, _) = http(
        address,
        "GET",
        "/v1/issues?cursor=not-a-uuid",
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 400);

    let denied_view = http(
        address,
        "GET",
        &issue_path,
        Some(&intruder_token),
        Vec::new(),
    )
    .await;
    let denied_close = http(
        address,
        "POST",
        &close_path,
        Some(&intruder_token),
        b"{}".to_vec(),
    )
    .await;
    let absent = http(
        address,
        "GET",
        "/v1/issues/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(denied_view, denied_close);
    assert_eq!(denied_view, absent);
    assert_eq!(denied_view.0, 404);
    let foreign_view = http(
        address,
        "GET",
        &issue_path,
        Some(&foreign_token),
        Vec::new(),
    )
    .await;
    assert_eq!(foreign_view, denied_view);

    let (status, first_close) = http(
        address,
        "POST",
        &close_path,
        Some(&creator_token),
        b"{}".to_vec(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(first_close["state_category"], "completed");
    let version = first_close["version"].as_i64().unwrap();
    let (status, second_close) = http(
        address,
        "POST",
        &close_path,
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(second_close["version"], version);

    let closed_rows = sqlx::query(
        "SELECT event_id, envelope FROM outbox WHERE envelope->>'tenant' = $1 \
         AND envelope->>'type_' = $2 AND aggregate = $3",
    )
    .bind(&tenant)
    .bind(ISSUE_CLOSED)
    .bind(format!("issue:{issue_id}"))
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(closed_rows.len(), 1, "retry emits no second close event");
    let closed_envelope: serde_json::Value = closed_rows[0].get("envelope");
    assert!(!closed_envelope.to_string().contains(title));
    assert_eq!(closed_envelope["payload"]["category"], "completed");

    // Exact generated-tenant cleanup. Projection state is last because row/tuple deletion triggers
    // intentionally invalidate it again.
    for statement in [
        "DELETE FROM issue_authz_binding WHERE tenant_id = $1",
        "DELETE FROM issue WHERE tenant_id = $1",
        "DELETE FROM prefix_counter WHERE tenant_id = $1",
        "DELETE FROM rebac_tuple WHERE tenant_id = $1",
        "DELETE FROM issue_authz_visible WHERE tenant_id = $1",
        "DELETE FROM outbox WHERE envelope->>'tenant' = $1",
        "DELETE FROM authz_projection_state WHERE tenant_id = $1",
    ] {
        sqlx::query(statement)
            .bind(&tenant)
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query(statement)
            .bind(&foreign_tenant)
            .execute(&admin)
            .await
            .unwrap();
    }
}
