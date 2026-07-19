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
    bootstrap_principal_and_mint, register_issues, serve_edge, AuthenticatedActionPolicy,
    BootstrapParams, Gateway, StoreBackedIssueAuthorizer, MAX_ISSUE_JSON_BYTES,
};
use myelin_events::EventEnvelope;
use myelin_identity::{
    DataRole, FragmentAdmit, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore, StoreBackedCheck, TupleStore,
};
use myelin_issues::events::ISSUE_CLOSED;
use myelin_issues::{
    issues_hot_tables, issues_migrations, IssueAuthorizationStatus, PgIssueStore,
    ISSUE_RECENT_LIST_INDEX,
};
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
    let (status, _, body) = http_with_headers(address, method, path, token, body).await;
    (status, body)
}

async fn http_with_headers(
    address: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Vec<u8>,
) -> (u16, hyper::HeaderMap, serde_json::Value) {
    let response = open(address, method, path, token, body).await;
    let status = response.status().as_u16();
    let (parts, body) = response.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (
        status,
        parts.headers,
        serde_json::from_slice(&bytes).unwrap(),
    )
}

fn create_body(title: &str) -> Vec<u8> {
    create_body_with_prefix("ENG", title)
}

fn create_body_with_prefix(prefix: &str, title: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "project_id": PROJECT_ID,
        "type_id": TYPE_ID,
        "prefix": prefix,
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
    let peer = principal(
        &tenant,
        &format!("human:peer:{unique}"),
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
    let foreign_worker = principal(
        &foreign_tenant,
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
    let kms = Arc::new(KmsEngine::new());
    let cell = Arc::new(CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).unwrap());
    let directory = PrincipalStore::new(kms.clone());

    // Drive the production bootstrap body instead of manually seeding the project tuple. This one
    // call provisions the login, writes the exact durable project reader edge, and mints the token
    // that drives the complete authenticated founder lifecycle below.
    let bootstrap_directory = directory.clone();
    let bootstrap_tuples = tuples.clone();
    let bootstrap_cell = cell.clone();
    let bootstrap_tenant = tenant.clone();
    let bootstrap_principal = creator.principal_id.0.clone();
    let bootstrap_outcome = tokio::task::spawn_blocking(move || {
        bootstrap_principal_and_mint(
            &bootstrap_directory,
            &bootstrap_tuples,
            &bootstrap_cell,
            &BootstrapParams {
                tenant: &bootstrap_tenant,
                region: REGION,
                principal: &bootstrap_principal,
                issues_project: PROJECT_ID,
                display: None,
                ttl_days: 1,
            },
            now(),
        )
    })
    .await
    .unwrap()
    .expect("production bootstrap grants project reader and mints creator token");

    // A second project reader proves that request ownership is the exact authenticated creator,
    // not merely project membership and never an agent's optional on_behalf_of attribution.
    let peer_directory = directory.clone();
    let peer_tuples = tuples.clone();
    let peer_cell = cell.clone();
    let peer_tenant = tenant.clone();
    let peer_principal = peer.principal_id.0.clone();
    let peer_bootstrap_outcome = tokio::task::spawn_blocking(move || {
        bootstrap_principal_and_mint(
            &peer_directory,
            &peer_tuples,
            &peer_cell,
            &BootstrapParams {
                tenant: &peer_tenant,
                region: REGION,
                principal: &peer_principal,
                issues_project: PROJECT_ID,
                display: None,
                ttl_days: 1,
            },
            now(),
        )
    })
    .await
    .unwrap()
    .expect("production bootstrap grants the peer project reader access");

    let issue_authorizer = StoreBackedIssueAuthorizer::new(check.clone());
    let issue_store = Arc::new(PgIssueStore::new(
        provider.clone(),
        kms.clone(),
        issue_authorizer.clone(),
    ));

    seed_login(&directory, &intruder, "intruder-subject");
    seed_login(&directory, &foreign, "foreign-subject");
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        directory,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        kms.clone(),
    )));
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

    let creator_token = bootstrap_outcome.token;
    let peer_token = peer_bootstrap_outcome.token;
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
    let (status, create_headers, receipt) = http_with_headers(
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
    let request_event_id = receipt["authorization"]["request_event_id"]
        .as_str()
        .unwrap()
        .to_string();
    let authorization_path = format!("/v1/issues/authorization-requests/{request_event_id}");
    assert_eq!(
        create_headers
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some(authorization_path.as_str())
    );
    let issue_path = format!("/v1/issues/{issue_id}");
    let close_path = format!("{issue_path}/close");

    let (status, pending_headers, pending_status) = http_with_headers(
        address,
        "GET",
        &authorization_path,
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 202);
    assert_eq!(pending_status["status"], "pending");
    assert_eq!(pending_status["issue"]["id"], issue_id);
    assert!(pending_status["retry_after_ms"].as_u64().unwrap() <= 10_000);
    assert_eq!(
        pending_headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    for forbidden in ["attempts", "last_error", title] {
        assert!(!pending_status.to_string().contains(forbidden));
    }
    let peer_denied = http(
        address,
        "GET",
        &authorization_path,
        Some(&peer_token),
        Vec::new(),
    )
    .await;
    let intruder_denied = http(
        address,
        "GET",
        &authorization_path,
        Some(&intruder_token),
        Vec::new(),
    )
    .await;
    let foreign_denied = http(
        address,
        "GET",
        &authorization_path,
        Some(&foreign_token),
        Vec::new(),
    )
    .await;
    let absent_status = http(
        address,
        "GET",
        "/v1/issues/authorization-requests/01ARZ3NDEKTSV4RRFFQ69G5FAV",
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(peer_denied, intruder_denied);
    assert_eq!(peer_denied, foreign_denied);
    assert_eq!(peer_denied, absent_status);
    assert_eq!(peer_denied.0, 404);
    assert_eq!(
        http(
            address,
            "GET",
            "/v1/issues/authorization-requests/not-a-ulid",
            Some(&creator_token),
            Vec::new(),
        )
        .await
        .0,
        400
    );

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
    let (index_ready, index_valid): (bool, bool) = sqlx::query_as(
        "SELECT ix.indisready, ix.indisvalid FROM pg_index ix \
         JOIN pg_class c ON c.oid = ix.indexrelid WHERE c.relname = $1",
    )
    .bind(ISSUE_RECENT_LIST_INDEX)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(
        index_ready && index_valid,
        "recent-list index is live-ready"
    );
    let mut explain_conn = admin.acquire().await.unwrap();
    sqlx::query("SET enable_seqscan = off")
        .execute(&mut *explain_conn)
        .await
        .unwrap();
    let explain: Vec<String> = sqlx::query_scalar(
        "EXPLAIN (COSTS OFF) SELECT id FROM issue \
         WHERE tenant_id = $1 AND region = $2 AND deleted_at IS NULL AND NOT archived \
         ORDER BY updated_at DESC, id DESC LIMIT 10",
    )
    .bind(&tenant)
    .bind(REGION)
    .fetch_all(&mut *explain_conn)
    .await
    .unwrap();
    assert!(
        explain
            .iter()
            .any(|line| line.contains(ISSUE_RECENT_LIST_INDEX)),
        "recent-list plan did not use {ISSUE_RECENT_LIST_INDEX}: {explain:?}"
    );
    sqlx::query("RESET enable_seqscan")
        .execute(&mut *explain_conn)
        .await
        .unwrap();
    drop(explain_conn);

    sqlx::query(
        "UPDATE issue_authz_binding SET issue_object = 'issue:00000000-0000-0000-0000-000000000000' \
         WHERE tenant_id = $1 AND request_event_id = $2",
    )
    .bind(&tenant)
    .bind(&request_event_id)
    .execute(&admin)
    .await
    .unwrap();
    let tampered_status = http(
        address,
        "GET",
        &authorization_path,
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(tampered_status, absent_status);
    sqlx::query(
        "UPDATE issue_authz_binding SET issue_object = 'issue:' || issue_id::text \
         WHERE tenant_id = $1 AND request_event_id = $2",
    )
    .bind(&tenant)
    .bind(&request_event_id)
    .execute(&admin)
    .await
    .unwrap();
    let bootstrap_event_json: serde_json::Value = sqlx::query_scalar(
        "SELECT envelope FROM outbox WHERE aggregate = $1 AND envelope->>'type_' = $2",
    )
    .bind(format!("iam:tuple:{tenant}:project:{PROJECT_ID}"))
    .bind(myelin_identity::IAM_TUPLE_WRITTEN)
    .fetch_one(&admin)
    .await
    .unwrap();
    let bootstrap_event: EventEnvelope = serde_json::from_value(bootstrap_event_json).unwrap();
    assert_eq!(bootstrap_event.actor.0.principal_id.0, "bootstrap-operator");
    assert_eq!(bootstrap_event.actor.0.tenant.as_str(), tenant);
    assert_eq!(bootstrap_event.actor.0.region.as_str(), REGION);
    assert_eq!(bootstrap_event.tenant.as_str(), tenant);
    assert_eq!(bootstrap_event.region.as_str(), REGION);
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
    issue_store
        .rebuild_effective_issue_view(&foreign_worker)
        .await
        .unwrap();

    let (status, active_headers, active_status) = http_with_headers(
        address,
        "GET",
        &authorization_path,
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(active_status["status"], "active");
    assert_eq!(active_status["issue"]["id"], issue_id);
    assert_eq!(active_status["issue"]["title"], title);
    assert_eq!(
        active_headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let restarted_store = PgIssueStore::new(
        provider.clone(),
        kms.clone(),
        StoreBackedIssueAuthorizer::new(check.clone()),
    );
    assert!(matches!(
        restarted_store
            .authorization_status(&creator, &request_event_id)
            .await
            .unwrap(),
        IssueAuthorizationStatus::Active(issue) if issue.id == issue_id && issue.title == title
    ));

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

    let mut additional = Vec::new();
    for (prefix, extra_title) in [
        ("ENG", "recent engineering issue"),
        ("OPS", "older operations issue"),
        ("ENG", "tampered binding must stay hidden"),
        ("OPS", "archived issue must stay hidden"),
    ] {
        let (status, receipt) = http(
            address,
            "POST",
            "/v1/issues",
            Some(&creator_token),
            create_body_with_prefix(prefix, extra_title),
        )
        .await;
        assert_eq!(status, 202, "stages `{extra_title}`: {receipt}");
        additional.push(receipt["issue"]["id"].as_str().unwrap().to_string());
    }
    let outcomes = reconcile_pending_issue_authorizations(&issue_store, &check, &worker, 100)
        .await
        .unwrap();
    assert_eq!(outcomes.len(), 4);
    assert!(outcomes
        .iter()
        .all(|(_, outcome)| outcome.as_ref().unwrap().newly_activated));
    issue_store
        .rebuild_effective_issue_view(&worker)
        .await
        .unwrap();

    for (id, timestamp) in [
        (&issue_id, "2026-07-19 10:00:00+00"),
        (&additional[0], "2026-07-19 12:00:00+00"),
        (&additional[1], "2026-07-19 11:00:00+00"),
        (&additional[2], "2026-07-19 13:00:00+00"),
        (&additional[3], "2026-07-19 14:00:00+00"),
    ] {
        sqlx::query(
            "UPDATE issue SET updated_at = $3::timestamptz WHERE tenant_id = $1 AND id = $2::uuid",
        )
        .bind(&tenant)
        .bind(id)
        .bind(timestamp)
        .execute(&admin)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE issue SET archived = true WHERE tenant_id = $1 AND id = $2::uuid")
        .bind(&tenant)
        .bind(&additional[3])
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE issue_authz_binding SET project_userset = \
         'project:00000000-0000-0000-0000-000000000000#view' \
         WHERE tenant_id = $1 AND issue_id = $2::uuid",
    )
    .bind(&tenant)
    .bind(&additional[2])
    .execute(&admin)
    .await
    .unwrap();
    issue_store
        .rebuild_effective_issue_view(&worker)
        .await
        .unwrap();

    let (status, default_open) = http(
        address,
        "GET",
        "/v1/issues",
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 200);
    let open_ids: Vec<_> = default_open["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(open_ids, [additional[0].as_str(), additional[1].as_str()]);
    assert!(!default_open.to_string().contains(&additional[2]));
    assert!(!default_open.to_string().contains(&additional[3]));

    let (status, closed_page) = http(
        address,
        "GET",
        "/v1/issues?state=closed",
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(closed_page["items"].as_array().unwrap().len(), 1);
    assert_eq!(closed_page["items"][0]["id"], issue_id);

    let (status, eng_page) = http(
        address,
        "GET",
        "/v1/issues?state=all&key=eng-",
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 200);
    let eng_ids: Vec<_> = eng_page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(eng_ids, [additional[0].as_str(), issue_id.as_str()]);

    let (status, first_page) = http(
        address,
        "GET",
        "/v1/issues?state=all&limit=2",
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 200);
    let cursor = first_page["page"]["next_cursor"]
        .as_str()
        .expect("a third authorized row requires a cursor");
    let first_ids: Vec<_> = first_page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(first_ids, [additional[0].as_str(), additional[1].as_str()]);
    let (status, second_page) = http(
        address,
        "GET",
        &format!("/v1/issues?state=all&limit=2&cursor={cursor}"),
        Some(&creator_token),
        Vec::new(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(second_page["items"].as_array().unwrap().len(), 1);
    assert_eq!(second_page["items"][0]["id"], issue_id);
    assert!(!first_ids.contains(&second_page["items"][0]["id"].as_str().unwrap()));
    assert_eq!(
        http(
            address,
            "GET",
            &format!("/v1/issues?state=open&limit=2&cursor={cursor}"),
            Some(&creator_token),
            Vec::new(),
        )
        .await
        .0,
        400,
        "cursor is bound to its normalized state filter"
    );

    let peer_page = http(
        address,
        "GET",
        "/v1/issues?state=all",
        Some(&peer_token),
        Vec::new(),
    )
    .await;
    assert_eq!(peer_page.0, 200);
    assert_eq!(peer_page.1["items"].as_array().unwrap().len(), 3);
    let intruder_page = http(
        address,
        "GET",
        "/v1/issues?state=all&key=ENG-",
        Some(&intruder_token),
        Vec::new(),
    )
    .await;
    assert_eq!(intruder_page.0, 200);
    assert!(intruder_page.1["items"].as_array().unwrap().is_empty());
    let foreign_page = http(
        address,
        "GET",
        "/v1/issues?state=all",
        Some(&foreign_token),
        Vec::new(),
    )
    .await;
    assert_eq!(foreign_page.0, 200);
    assert!(foreign_page.1["items"].as_array().unwrap().is_empty());

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
