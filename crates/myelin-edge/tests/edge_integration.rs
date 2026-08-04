use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use myelin_edge::{
    serve_edge, serve_edge_until_shutdown, serve_edge_until_shutdown_with_probe,
    sse_scope_for_resource, sse_scope_for_tenant, AllowAll, EdgeError, EdgeResponse, Gateway,
    Handler, HandlerCtx, Method, ReadinessCheck, ReadinessProbe, ShutdownOutcome, SseEvent,
    WhoamiHandler,
};
use myelin_events::Timestamp;
use myelin_identity::{
    DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RevokeTarget,
};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore,
};
use myelin_storage::{KmsEngine, TenantScope};
use myelin_tenancy::{Region, TenantId};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};

const TENANT: &str = "acme";
const REGION: &str = "eu-west";
const OTHER_TENANT: &str = "globex";
const SCHEME: &str = "agent";
static SLOW_GIT_STARTED: AtomicUsize = AtomicUsize::new(0);
static SLOW_JSON_STARTED: AtomicUsize = AtomicUsize::new(0);

struct SlowGitWireHandler;

impl Handler for SlowGitWireHandler {
    fn handle(&self, _ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        SLOW_GIT_STARTED.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(500));
        Ok(EdgeResponse::json(200, &serde_json::json!({ "status": "ok" })))
    }
}

struct SlowJsonHandler;

impl Handler for SlowJsonHandler {
    fn handle(&self, _ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        SLOW_JSON_STARTED.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(500));
        Ok(EdgeResponse::json(200, &serde_json::json!({ "status": "ok" })))
    }
}

struct PanicHandler;

impl Handler for PanicHandler {
    fn handle(&self, _ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        panic!("intentional handler panic for transport containment proof")
    }
}

struct InvalidResponseHandler;

impl Handler for InvalidResponseHandler {
    fn handle(&self, _ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        Ok(EdgeResponse::json(
            200,
            &serde_json::json!({ "false": "success" }),
        )
        .with_header("content-length", "0"))
    }
}

struct ToggleReadiness(AtomicBool);

impl ReadinessProbe for ToggleReadiness {
    fn check(&self) -> ReadinessCheck<'_> {
        Box::pin(std::future::ready(self.0.load(Ordering::SeqCst)))
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn admin_scope(tenant: &str) -> TenantScope {
    TenantScope::from_verified_token(
        &Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        ),
        Region(REGION.into()),
    )
}

fn build_gateway() -> (Arc<Gateway>, CellTokenAuthority, RevocationStore) {
    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    let scope = admin_scope(TENANT);
    store
        .put_principal(
            &scope,
            PrincipalId("svc:agent".into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .expect("seed principal");
    store
        .link_credential(&scope, SCHEME, "subj-1", &PrincipalId("svc:agent".into()))
        .expect("link credential");

    let revocations = RevocationStore::new();
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        revocations.clone(),
    ));
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));

    let gateway = Arc::new(
        Gateway::builder(authn, human_login, Arc::new(AllowAll))
            .route(
                Method::Get,
                "/v1/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
            .route(
                Method::Get,
                "/v1/t/{tenant}/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
            .route(
                Method::Get,
                "/v1/panic",
                "edge.whoami",
                Arc::new(PanicHandler),
            )
            .route(
                Method::Get,
                "/v1/invalid-response",
                "edge.whoami",
                Arc::new(InvalidResponseHandler),
            )
            .route(
                Method::Get,
                "/v1/slow-json",
                "edge.whoami",
                Arc::new(SlowJsonHandler),
            )
            .route(
                Method::Get,
                "/acme/eu-west/widgets.git/info/refs",
                "git.wire.upload_pack",
                Arc::new(SlowGitWireHandler),
            )
            .sse_route("/v1/t/{tenant}/events", "edge.events.subscribe", "edge")
            .sse_route_scoped(
                "/v1/t/{tenant}/repos/{repo}/events",
                "edge.events.subscribe",
                "git",
                "repo",
            )
            .build(),
    );
    (gateway, cell, revocations)
}

fn mint(cell: &CellTokenAuthority, tenant: &str, jti: &str, exp_unix: i64) -> String {
    mint_with_authority(cell, tenant, jti, exp_unix, vec!["edge.operator".into()])
}

fn mint_with_authority(
    cell: &CellTokenAuthority,
    tenant: &str,
    jti: &str,
    exp_unix: i64,
    authority: Vec<String>,
) -> String {
    cell.mint(&CapabilityMintSpec {
        tenant: tenant.into(),
        region: REGION.into(),
        subject_key: "subj-1".into(),
        jti: jti.into(),
        exp_unix,
        authority,
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::OperatorBootstrap,
        audience: myelin_identity_service::CredentialAudience::Edge,
    })
}

async fn spawn(gateway: Arc<Gateway>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = serve_edge(listener, gateway).await;
    });
    addr
}

async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (u16, String) {
    let resp = open(addr, method, path, headers, body).await;
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn open(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Response<Incoming> {
    let stream = TcpStream::connect(addr).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "edge.test");
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    let req = builder.body(Full::new(Bytes::from(body))).unwrap();
    sender.send_request(req).await.unwrap()
}

fn bearer(token: &str) -> [(&'static str, String); 2] {
    [
        ("authorization", format!("Bearer {token}")),
        ("x-myelin-token-scheme", SCHEME.to_string()),
    ]
}

fn hdr<'a>(b: &'a [(&'static str, String); 2]) -> Vec<(&'a str, &'a str)> {
    b.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

#[tokio::test]
async fn valid_token_authenticates_resolves_principal_and_sets_scope() {
    let (gw, cell, _rev) = build_gateway();
    let addr = spawn(gw).await;
    let token = mint(&cell, TENANT, "jti-ok", now() + 3600);
    let h = bearer(&token);
    let (status, body) = http(addr, "GET", "/v1/whoami", &hdr(&h), vec![]).await;
    assert_eq!(status, 200, "a valid token authenticates: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["principal_id"], "svc:agent", "the resolved principal");
    assert_eq!(v["tenant"], TENANT, "the SET scope tenant is the token's");
    assert_eq!(v["region"], REGION);
    assert_eq!(v["kind"], "service");
}

#[tokio::test]
async fn forged_token_is_401_with_envelope() {
    let (gw, _cell, _rev) = build_gateway();
    let addr = spawn(gw).await;
    let h = bearer("acme|eu-west|subj-1|jti|0|agent:run");
    let (status, body) = http(addr, "GET", "/v1/whoami", &hdr(&h), vec![]).await;
    assert_eq!(status, 401, "a forged token is rejected");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        v["error"]["message"], "authentication required",
        "envelope shape (d)"
    );
    assert_eq!(v["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn expired_token_is_401() {
    let (gw, cell, _rev) = build_gateway();
    let addr = spawn(gw).await;
    let token = mint(&cell, TENANT, "jti-exp", now() - 10);
    let h = bearer(&token);
    let (status, _body) = http(addr, "GET", "/v1/whoami", &hdr(&h), vec![]).await;
    assert_eq!(status, 401, "an expired token is rejected");
}

#[tokio::test]
async fn revoked_token_is_401() {
    let (gw, cell, revocations) = build_gateway();
    let addr = spawn(gw).await;
    let token = mint(&cell, TENANT, "jti-rev", now() + 3600);
    let h = bearer(&token);
    let (ok, _b) = http(addr, "GET", "/v1/whoami", &hdr(&h), vec![]).await;
    assert_eq!(ok, 200, "live token authenticates before revocation");
    revocations.revoke(
        &admin_scope(TENANT),
        &RevokeTarget::Jti("jti-rev".into()),
        Timestamp("2026-06-27T00:00:00Z".into()),
    );
    let (status, _body) = http(addr, "GET", "/v1/whoami", &hdr(&h), vec![]).await;
    assert_eq!(status, 401, "a revoked token fails closed at the edge");
}

#[tokio::test]
async fn tenant_isolation_at_the_edge_is_the_idor_floor() {
    let (gw, cell, _rev) = build_gateway();
    let token = mint(&cell, TENANT, "jti-idor", now() + 3600);
    let h = bearer(&token);
    let addr = spawn(gw.clone()).await;

    let (status, body) = http(
        addr,
        "GET",
        &format!("/v1/t/{OTHER_TENANT}/whoami"),
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(
        status, 403,
        "a cross-tenant path is rejected at the edge: {body}"
    );
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "forbidden");
    assert_eq!(
        gw.public_surface().audit().count(),
        1,
        "the cross-tenant IDOR attempt was AUDITED (never swallowed)"
    );

    let (ok, ok_body) = http(
        addr,
        "GET",
        &format!("/v1/t/{TENANT}/whoami"),
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(ok, 200, "the own-tenant path is served");
    let ov: serde_json::Value = serde_json::from_str(&ok_body).unwrap();
    assert_eq!(
        ov["tenant"], TENANT,
        "served against the TOKEN's tenant, never the path"
    );
}

#[tokio::test]
async fn malformed_requests_are_clean_errors_no_panic() {
    let (gw, _cell, _rev) = build_gateway();
    let addr = spawn(gw).await;
    let (no_cred, _) = http(addr, "GET", "/v1/whoami", &[], vec![]).await;
    assert_eq!(no_cred, 401);
    let (not_found, _) = http(addr, "GET", "/v1/does/not/exist", &[], vec![]).await;
    assert_eq!(not_found, 404);
    let (bad_login, _) = http(addr, "POST", "/v1/auth/login", &[], b"{not json".to_vec()).await;
    assert_eq!(bad_login, 400);
    let (login, _) = http(
        addr,
        "POST",
        "/v1/auth/login",
        &[],
        br#"{"scheme":"oidc","material":"acme|eu-west|subj-1","nonce":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#.to_vec(),
    )
    .await;
    assert_eq!(login, 503, "human login refuses-not-mocks until configured");
}

#[tokio::test]
async fn handler_panic_is_a_generic_500_and_the_listener_survives() {
    let (gateway, cell, _revocations) = build_gateway();
    let token = mint(&cell, TENANT, "jti-panic", now() + 3600);
    let headers = bearer(&token);
    let addr = spawn(gateway).await;

    let (status, body) = http(addr, "GET", "/v1/panic", &hdr(&headers), vec![]).await;
    assert_eq!(status, 500);
    let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(envelope["error"]["code"], "internal");
    assert_eq!(envelope["error"]["message"], "internal error");
    assert!(!body.contains("intentional handler panic"));

    let (live, _) = http(addr, "GET", "/livez", &[], vec![]).await;
    assert_eq!(live, 200, "a contained handler panic must not kill the listener");
}

#[tokio::test]
async fn invalid_handler_response_metadata_fails_closed_without_false_success() {
    let (gateway, cell, _revocations) = build_gateway();
    let token = mint(&cell, TENANT, "jti-invalid-response", now() + 3600);
    let headers = bearer(&token);
    let addr = spawn(gateway).await;

    let response = open(
        addr,
        "GET",
        "/v1/invalid-response",
        &hdr(&headers),
        vec![],
    )
    .await;
    assert_eq!(response.status(), 500);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let envelope: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope["error"]["code"], "internal");
    assert_eq!(envelope["error"]["message"], "internal error");
    assert!(!body.windows(b"false".len()).any(|window| window == b"false"));
}

#[tokio::test]
async fn sse_endpoint_streams_a_frame() {
    let (gw, cell, _rev) = build_gateway();
    let token = mint(&cell, TENANT, "jti-sse", now() + 3600);
    let h = bearer(&token);
    let addr = spawn(gw.clone()).await;

    let resp = open(
        addr,
        "GET",
        &format!("/v1/t/{TENANT}/events"),
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/event-stream",
        "the SSE content-type"
    );
    let mut body = resp.into_body();

    let scope = sse_scope_for_tenant(TENANT);
    let mut got = None;
    for _ in 0..20 {
        gw.sse_hub()
            .broadcast("edge", &scope, SseEvent::typed("ping", "{\"hello\":true}"));
        match tokio::time::timeout(Duration::from_millis(100), body.frame()).await {
            Ok(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    got = Some(String::from_utf8_lossy(data).to_string());
                    break;
                }
            }
            _ => continue,
        }
    }
    let frame = got.expect("an SSE frame should stream to the client");
    assert!(
        frame.contains("event: ping"),
        "the SSE frame carries the event type: {frame}"
    );
    assert!(
        frame.contains("data: {\"hello\":true}"),
        "the SSE frame carries the data: {frame}"
    );
}

#[tokio::test]
async fn scoped_sse_route_isolates_per_object() {
    let (gw, cell, _rev) = build_gateway();
    let token = mint(&cell, TENANT, "jti-sse-scoped", now() + 3600);
    let h = bearer(&token);
    let addr = spawn(gw.clone()).await;

    let resp = open(
        addr,
        "GET",
        &format!("/v1/t/{TENANT}/repos/widgets/events"),
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    let mut body = resp.into_body();

    let coarse = sse_scope_for_tenant(TENANT);
    let other = sse_scope_for_resource(TENANT, "repo", "secrets");
    let widgets = sse_scope_for_resource(TENANT, "repo", "widgets");
    let mut got = None;
    for _ in 0..20 {
        gw.sse_hub()
            .broadcast("git", &coarse, SseEvent::typed("leak", "{\"coarse\":true}"));
        gw.sse_hub()
            .broadcast("git", &other, SseEvent::typed("leak", "{\"other\":true}"));
        gw.sse_hub().broadcast(
            "git",
            &widgets,
            SseEvent::typed("push", "{\"repo\":\"widgets\"}"),
        );
        match tokio::time::timeout(Duration::from_millis(100), body.frame()).await {
            Ok(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    got = Some(String::from_utf8_lossy(data).to_string());
                    break;
                }
            }
            _ => continue,
        }
    }
    let frame = got.expect("the per-object SSE frame should stream to the client");
    assert!(
        frame.contains("data: {\"repo\":\"widgets\"}"),
        "the subscriber receives ITS object's frame: {frame}"
    );
    assert!(
        !frame.contains("coarse") && !frame.contains("other"),
        "no tenant-coarse / foreign-object frame leaked into the per-object stream: {frame}"
    );
}

#[tokio::test]
async fn requested_shutdown_closes_the_listener_without_active_connections() {
    let (gateway, _cell, _revocations) = build_gateway();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (outcome, ()) = serve_edge_until_shutdown(
        listener,
        gateway,
        std::future::ready(()),
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    assert_eq!(outcome, ShutdownOutcome::Graceful { connections: 0 });
}

#[tokio::test]
async fn health_endpoints_split_dependency_free_liveness_from_readiness() {
    let (gateway, _cell, _revocations) = build_gateway();
    let readiness = Arc::new(ToggleReadiness(AtomicBool::new(false)));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let readiness_for_server = readiness.clone();
    let server = tokio::spawn(async move {
        serve_edge_until_shutdown_with_probe(
            listener,
            gateway,
            readiness_for_server,
            async move {
                let _ = shutdown_rx.await;
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap()
    });

    let (live, live_body) = http(addr, "GET", "/livez", &[], vec![]).await;
    let not_ready_response = open(addr, "GET", "/readyz", &[], vec![]).await;
    let not_ready = not_ready_response.status().as_u16();
    assert_eq!(not_ready_response.headers()["retry-after"], "5");
    let not_ready_body = String::from_utf8_lossy(
        &not_ready_response.into_body().collect().await.unwrap().to_bytes(),
    )
    .to_string();
    assert_eq!((live, live_body.as_str()), (200, "{\"status\":\"ok\"}"));
    assert_eq!(
        (not_ready, not_ready_body.as_str()),
        (503, "{\"status\":\"not_ready\"}")
    );

    readiness.0.store(true, Ordering::SeqCst);
    let (ready, ready_body) = http(addr, "GET", "/readyz", &[], vec![]).await;
    let (wrong_method, _) = http(addr, "POST", "/readyz", &[], vec![]).await;
    assert_eq!((ready, ready_body.as_str()), (200, "{\"status\":\"ok\"}"));
    assert_eq!(wrong_method, 405);

    shutdown_tx.send(()).unwrap();
    let (outcome, ()) = server.await.unwrap();
    assert!(matches!(outcome, ShutdownOutcome::Graceful { .. }));
}

#[tokio::test]
async fn every_response_carries_a_fresh_server_request_id() {
    let (gateway, _cell, _revocations) = build_gateway();
    let addr = spawn(gateway).await;

    let first = open(addr, "GET", "/livez", &[("x-request-id", "attacker-value")], vec![]).await;
    let second = open(addr, "GET", "/does-not-exist", &[], vec![]).await;
    let first_id = first.headers()["x-request-id"].to_str().unwrap();
    let second_id = second.headers()["x-request-id"].to_str().unwrap();
    assert_ne!(first_id, "attacker-value", "client request IDs are never trusted");
    assert_ne!(first_id, second_id);
    for id in [first_id, second_id] {
        assert_eq!(id.len(), 40);
        assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}

#[tokio::test]
async fn responses_are_non_cacheable_and_disable_content_sniffing() {
    let (gateway, cell, _revocations) = build_gateway();
    let token = mint(&cell, TENANT, "jti-response-headers", now() + 3600);
    let headers = bearer(&token);
    let addr = spawn(gateway).await;

    let api = open(addr, "GET", "/v1/whoami", &hdr(&headers), vec![]).await;
    assert_eq!(api.headers()["cache-control"], "no-store");
    assert_eq!(api.headers()["x-content-type-options"], "nosniff");

    let error = open(addr, "GET", "/does-not-exist", &[], vec![]).await;
    assert_eq!(error.headers()["cache-control"], "no-store");
    assert_eq!(error.headers()["x-content-type-options"], "nosniff");

    let sse = open(
        addr,
        "GET",
        &format!("/v1/t/{TENANT}/events"),
        &hdr(&headers),
        vec![],
    )
    .await;
    assert_eq!(sse.headers()["cache-control"], "no-cache");
    assert_eq!(sse.headers()["x-content-type-options"], "nosniff");
}

#[tokio::test(flavor = "current_thread")]
async fn blocking_git_wire_dispatch_does_not_stall_liveness() {
    SLOW_GIT_STARTED.store(0, Ordering::SeqCst);
    let (gateway, cell, _revocations) = build_gateway();
    let token = mint_with_authority(
        &cell,
        TENANT,
        "jti-slow-git",
        now() + 3600,
        vec!["edge.operator".into(), "git.wire.upload_pack".into()],
    );
    let headers = bearer(&token);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = serve_edge(listener, gateway).await;
    });

    let slow_headers = headers.clone();
    let started = std::time::Instant::now();
    let slow = tokio::spawn(async move {
        http(
            addr,
            "GET",
            "/acme/eu-west/widgets.git/info/refs",
            &hdr(&slow_headers),
            vec![],
        )
        .await
    });
    let handler_started = tokio::time::timeout(Duration::from_secs(1), async {
        while SLOW_GIT_STARTED.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    if handler_started.is_err() {
        if slow.is_finished() {
            let outcome = slow.await.expect("slow request task");
            panic!("Git handler was bypassed; response was {outcome:?}");
        }
        panic!("the blocking Git handler did not start within one second");
    }

    let (status, body) = http(addr, "GET", "/livez", &[], vec![]).await;
    assert_eq!((status, body.as_str()), (200, "{\"status\":\"ok\"}"));
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "liveness must not wait for the 500 ms blocking Git operation"
    );
    let (slow_status, _) = slow.await.unwrap();
    assert_eq!(slow_status, 200);
}

#[tokio::test(flavor = "current_thread")]
async fn blocking_json_dispatch_does_not_stall_liveness() {
    SLOW_JSON_STARTED.store(0, Ordering::SeqCst);
    let (gateway, cell, _revocations) = build_gateway();
    let token = mint(&cell, TENANT, "jti-slow-json", now() + 3600);
    let headers = bearer(&token);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = serve_edge(listener, gateway).await;
    });

    let slow_headers = headers.clone();
    let slow = tokio::spawn(async move {
        http(addr, "GET", "/v1/slow-json", &hdr(&slow_headers), vec![]).await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while SLOW_JSON_STARTED.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the blocking JSON handler must start");

    let started = std::time::Instant::now();
    let (status, body) = http(addr, "GET", "/livez", &[], vec![]).await;
    assert_eq!((status, body.as_str()), (200, "{\"status\":\"ok\"}"));
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "liveness must remain responsive while a JSON handler blocks"
    );
    let (slow_status, _) = slow.await.unwrap();
    assert_eq!(slow_status, 200);
}

#[tokio::test]
async fn saturated_git_wire_pool_returns_bounded_retry_guidance() {
    SLOW_GIT_STARTED.store(0, Ordering::SeqCst);
    let (gateway, cell, _revocations) = build_gateway();
    let token = mint_with_authority(
        &cell,
        TENANT,
        "jti-git-overload",
        now() + 3600,
        vec!["edge.operator".into(), "git.wire.upload_pack".into()],
    );
    let headers = bearer(&token);
    let addr = spawn(gateway).await;
    let mut active = Vec::new();
    for _ in 0..2 {
        let request_headers = headers.clone();
        active.push(tokio::spawn(async move {
            open(
                addr,
                "GET",
                "/acme/eu-west/widgets.git/info/refs",
                &hdr(&request_headers),
                vec![],
            )
            .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while SLOW_GIT_STARTED.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both effective Git wire response slots must be occupied");

    let shed = open(
        addr,
        "GET",
        "/acme/eu-west/widgets.git/info/refs",
        &hdr(&headers),
        vec![],
    )
    .await;
    assert_eq!(shed.status(), 503);
    assert_eq!(shed.headers()["retry-after"], "1");

    for request in active {
        assert_eq!(request.await.unwrap().status(), 200);
    }
}

#[tokio::test]
async fn shutdown_deadline_forces_an_open_sse_connection_closed() {
    let (gateway, cell, _revocations) = build_gateway();
    let token = mint(&cell, TENANT, "jti-sse-shutdown", now() + 3600);
    let headers = bearer(&token);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        serve_edge_until_shutdown(
            listener,
            gateway,
            async move {
                let _ = shutdown_rx.await;
            },
            Duration::from_millis(25),
        )
        .await
        .unwrap()
    });

    let response = open(
        addr,
        "GET",
        &format!("/v1/t/{TENANT}/events"),
        &hdr(&headers),
        vec![],
    )
    .await;
    assert_eq!(response.status(), 200);
    let _open_stream = response.into_body();
    shutdown_tx.send(()).unwrap();

    let (outcome, ()) = tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .expect("bounded shutdown must return")
        .expect("server task must not panic");
    assert!(
        matches!(outcome, ShutdownOutcome::Forced { connections } if connections >= 1),
        "an open SSE stream must be forcibly closed at the deadline: {outcome:?}"
    );
}
