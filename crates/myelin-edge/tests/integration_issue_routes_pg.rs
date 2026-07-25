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
    DataRole, FragmentAdmit, Principal, PrincipalId, PrincipalKind, PrincipalStatus, Zookie,
};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore, StoreBackedCheck, TupleStore,
};
use myelin_issues::events::ISSUE_CLOSED;
use myelin_issues::{
    issues_hot_tables, issues_migrations, IssueAuthorizationBinding, IssueAuthorizationStatus,
    IssueTupleWriter, PgIssueStore, ISSUE_KEY_PREFIX_LIST_INDEX, ISSUE_RECENT_LIST_INDEX,
};
use myelin_storage::{
    all_durable_migrations, DurableTupleBacking, KmsEngine, PgBootstrap, TenantScope,
};
use myelin_substrate::HotTables;
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};

const REGION: &str = "fr-par";
const PROJECT_ID: &str = "11111111-1111-1111-1111-111111111111";
const TYPE_ID: &str = "22222222-2222-2222-2222-222222222222";
const SCHEME: &str = "agent";

#[derive(Default)]
struct CountingTupleWriter {
    calls: AtomicUsize,
}

impl IssueTupleWriter for CountingTupleWriter {
    fn ensure_parent_project<'a>(
        &'a self,
        _scope: &'a TenantScope,
        _actor: &'a Principal,
        _binding: &'a IssueAuthorizationBinding,
    ) -> Pin<Box<dyn Future<Output = Result<Zookie, String>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Zookie("must-not-be-used".into())) })
    }
}

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
    assert!(
        create_headers.get("location").is_none(),
        "a create-only credential must not receive a Location that requires issue.view"
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
    assert_eq!(status, 202, "create returns a staged receipt: {receipt}");
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
    for index in [ISSUE_RECENT_LIST_INDEX, ISSUE_KEY_PREFIX_LIST_INDEX] {
        let (index_ready, index_valid): (bool, bool) = sqlx::query_as(
            "SELECT ix.indisready, ix.indisvalid FROM pg_index ix \
             JOIN pg_class c ON c.oid = ix.indexrelid WHERE c.relname = $1",
        )
        .bind(index)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(index_ready && index_valid, "{index} is live-ready");
    }
    sqlx::query(
        "UPDATE issue_authz_binding SET issue_object = 'issue:00000000-0000-0000-0000-000000000000' \
         WHERE tenant_id = $1 AND request_event_id = $2",
    )
    .bind(&tenant)
    .bind(&request_event_id)
    .execute(&admin)
    .await
    .unwrap();
    let counting_writer = CountingTupleWriter::default();
    assert!(issue_store
        .reconcile_authorization(&worker, &issue_id, &counting_writer)
        .await
        .is_err());
    assert_eq!(
        counting_writer.calls.load(Ordering::SeqCst),
        0,
        "canonical preflight rejects a mutable binding copy before Identity"
    );
    let unintended_tuple_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rebac_tuple WHERE tenant_id = $1 AND region = $2 \
         AND object_id IN ($3, $4) AND relation = 'parent_project'",
    )
    .bind(&tenant)
    .bind(REGION)
    .bind(format!("issue:{issue_id}"))
    .bind("issue:00000000-0000-0000-0000-000000000000")
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(unintended_tuple_count, 0);
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
    let activated_zookie = outcomes[0].1.as_ref().unwrap().zookie.clone();
    sqlx::query("DELETE FROM outbox WHERE event_id = $1")
        .bind(&request_event_id)
        .execute(&admin)
        .await
        .unwrap();
    let active_retry_writer = CountingTupleWriter::default();
    let active_retry = issue_store
        .reconcile_authorization(&worker, &issue_id, &active_retry_writer)
        .await
        .expect("active reconciliation survives request-outbox reaping");
    assert!(!active_retry.newly_activated);
    assert_eq!(active_retry.issue.id, issue_id);
    assert_eq!(active_retry.issue.title, title);
    assert_eq!(active_retry.zookie, activated_zookie);
    assert_eq!(active_retry_writer.calls.load(Ordering::SeqCst), 0);
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

    // Realistic skew proof for the exact served authorization/state/prefix CTE. Every synthetic
    // issue is visible to this subject, while the requested prefix is absent, so a scan of the
    // entire authorization partition would be both tempting and pathological without the prefix
    // range index.
    sqlx::query(
        "INSERT INTO issue (tenant_id, region, id, key, prefix, type_id, type_rank, state, \
                            state_category, project_id, rank, title, title_nonce, title_ciphertext, \
                            created_by_principal, pii_key_ref, contains_personal_data, version) \
         SELECT $1, $2, md5($1 || ':skew:' || g::text)::uuid, \
                'SKW-' || lpad(g::text, 6, '0'), 'SKW', source.type_id, 0, 'Todo', \
                'unstarted', source.project_id, 'skew|' || lpad(g::text, 6, '0'), \
                '<encrypted>', source.title_nonce, source.title_ciphertext, \
                source.created_by_principal, source.pii_key_ref, true, 1 \
         FROM issue source CROSS JOIN generate_series(1, 2000) AS g \
         WHERE source.tenant_id = $1 AND source.region = $2 AND source.id = $3::uuid",
    )
    .bind(&tenant)
    .bind(REGION)
    .bind(&additional[0])
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO issue_authz_binding (tenant_id, region, issue_id, project_id, issue_object, \
                                           project_userset, relation, request_event_id, \
                                           created_event_id, state, zookie, activated_at) \
         SELECT tenant_id, region, id, project_id, 'issue:' || id::text, \
                'project:' || project_id::text || '#view', 'parent_project', \
                'skew-request:' || tenant_id || ':' || id::text, \
                'skew-created:' || tenant_id || ':' || id::text, 'active', 'skew-zookie', now() \
         FROM issue WHERE tenant_id = $1 AND region = $2 AND key LIKE 'SKW-%'",
    )
    .bind(&tenant)
    .bind(REGION)
    .execute(&admin)
    .await
    .unwrap();
    let skew_revision: i64 = sqlx::query_scalar(
        "SELECT source_revision FROM authz_projection_state \
         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view'",
    )
    .bind(&tenant)
    .bind(REGION)
    .fetch_one(&admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO issue_authz_visible (tenant_id, region, projection, subject, permission, \
                                           object_type, object_id, revision) \
         SELECT tenant_id, region, 'issue:view', $3, 'view', 'issue', id::text, $4 \
         FROM issue WHERE tenant_id = $1 AND region = $2 AND key LIKE 'SKW-%'",
    )
    .bind(&tenant)
    .bind(REGION)
    .bind(&creator.principal_id.0)
    .bind(skew_revision)
    .execute(&admin)
    .await
    .unwrap();
    for table in ["issue", "issue_authz_binding", "issue_authz_visible"] {
        sqlx::query(&format!("ANALYZE {table}"))
            .execute(&admin)
            .await
            .unwrap();
    }
    let served_explain: Vec<String> = sqlx::query_scalar(&format!(
        "EXPLAIN (COSTS OFF) {}",
        myelin_issues::pg_issue_store::authoritative_issue_list_sql()
    ))
    .bind(&tenant)
    .bind(REGION)
    .bind(&creator.principal_id.0)
    .bind(Option::<i64>::None)
    .bind(Option::<sqlx::types::Uuid>::None)
    .bind("open")
    .bind(Some("ZZZ-".to_string()))
    .bind(51_i64)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert!(
        served_explain
            .iter()
            .any(|line| line.contains("Index Cond") && line.contains("key")),
        "served prefix query has no bounded key range: {served_explain:?}"
    );
    assert!(
        !served_explain
            .iter()
            .any(|line| line.contains("Seq Scan on issue ")),
        "served prefix query scanned the issue table instead of using its bounded key range: \
         {served_explain:?}"
    );

    let (status, missing_outbox_receipt) = http(
        address,
        "POST",
        "/v1/issues",
        Some(&creator_token),
        create_body("pending request outbox reaped"),
    )
    .await;
    assert_eq!(status, 202);
    let missing_outbox_issue = missing_outbox_receipt["issue"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let missing_outbox_request = missing_outbox_receipt["authorization"]["request_event_id"]
        .as_str()
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE event_id = $1")
        .bind(missing_outbox_request)
        .execute(&admin)
        .await
        .unwrap();
    let pending_retry_writer = CountingTupleWriter::default();
    assert!(issue_store
        .reconcile_authorization(&worker, &missing_outbox_issue, &pending_retry_writer)
        .await
        .is_err());
    assert_eq!(pending_retry_writer.calls.load(Ordering::SeqCst), 0);
    let pending_state: String = sqlx::query_scalar(
        "SELECT state FROM issue_authz_binding WHERE tenant_id = $1 AND region = $2 \
         AND issue_id = $3::uuid",
    )
    .bind(&tenant)
    .bind(REGION)
    .bind(&missing_outbox_issue)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(pending_state, "pending");
    let missing_outbox_tuple_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rebac_tuple WHERE tenant_id = $1 AND region = $2 \
         AND object_id = $3 AND relation = 'parent_project'",
    )
    .bind(&tenant)
    .bind(REGION)
    .bind(format!("issue:{missing_outbox_issue}"))
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(missing_outbox_tuple_count, 0);

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
    // intentionally invalidate it again. `outbox` is handled separately, below, because a live
    // elected relay (e.g. `myelin-outbox-publisher serve`) sharing this Postgres continuously
    // quarantines this tenant's own `iam.tuple.written` outbox rows (`iam` is not an admitted Bus
    // taxonomy subsystem token, so every one of them is permanently rejected —
    // `myelin_events::taxonomy::SUBSYSTEM_TOKENS`), and `outbox_quarantine_event_id_fkey` is
    // `ON DELETE RESTRICT`, so a plain `DELETE FROM outbox` can lose a race against the relay.
    for statement in [
        "DELETE FROM issue_authz_binding WHERE tenant_id = $1",
        "DELETE FROM issue WHERE tenant_id = $1",
        "DELETE FROM prefix_counter WHERE tenant_id = $1",
        "DELETE FROM rebac_tuple WHERE tenant_id = $1",
        "DELETE FROM issue_authz_visible WHERE tenant_id = $1",
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
    delete_tenant_outbox_despite_concurrent_relay_quarantine(&admin, &tenant).await;
    delete_tenant_outbox_despite_concurrent_relay_quarantine(&admin, &foreign_tenant).await;
}

/// Delete every `outbox` row for `tenant`, tolerating a concurrently running elected relay (this
/// dev Postgres has a real `myelin-outbox-publisher serve` polling it) that may quarantine one of
/// this tenant's own rows between the moment we clear `outbox_quarantine` and the moment we issue
/// `DELETE FROM outbox`. `outbox_quarantine_event_id_fkey` is `ON DELETE RESTRICT`, so that plain
/// delete fails whenever the relay wins the race. The relay quarantines each row at most once and
/// this tenant's row set is finite, so re-clearing quarantine and retrying converges quickly.
async fn delete_tenant_outbox_despite_concurrent_relay_quarantine(admin: &sqlx::PgPool, tenant: &str) {
    const ATTEMPTS: u32 = 20;
    for attempt in 0..ATTEMPTS {
        sqlx::query(
            "DELETE FROM outbox_quarantine WHERE event_id IN \
             (SELECT event_id FROM outbox WHERE envelope->>'tenant' = $1)",
        )
        .bind(tenant)
        .execute(admin)
        .await
        .expect("clear this tenant's quarantined outbox rows");
        match sqlx::query("DELETE FROM outbox WHERE envelope->>'tenant' = $1")
            .bind(tenant)
            .execute(admin)
            .await
        {
            Ok(_) => return,
            Err(sqlx::Error::Database(db_error))
                if db_error.constraint() == Some("outbox_quarantine_event_id_fkey")
                    && attempt + 1 < ATTEMPTS =>
            {
                continue;
            }
            Err(err) => panic!("tenant outbox cleanup failed for {tenant}: {err}"),
        }
    }
}
