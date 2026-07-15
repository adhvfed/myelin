//! # Edge integration proofs — the gateway is REAL, over a real hyper HTTP listener.
//!
//! These tests bind an ephemeral TCP port, serve the edge with [`myelin_edge::serve_edge`], and drive
//! REAL HTTP round-trips with a hyper client. The auth path uses the REAL `PasetoCapabilityVerifier`
//! (genuine Ed25519/PASETO crypto) over a real cell key + a seeded S1 principal directory — a minted
//! token is presented as a `Authorization: Bearer`, a forged/expired/revoked one is rejected.
//!
//! No live PG is needed: the tenant SCOPE (tenant-from-token + the IDOR reject/audit) is the in-memory
//! `PublicSurface`/`TenantScope` logic, and the whoami handler reports the SET scope. The live-PG
//! `with_tenant_tx` scope is proven separately under `--features integration`
//! (`edge_tenant_scope_integration.rs`).
//!
//! Covered: (a) valid token authenticates → resolves principal → scope set → whoami returns the
//! verified tenant/principal; (b) forged / expired / revoked → 401 with the `{error:{message}}`
//! envelope; (c) tenant isolation at the edge (token for A cannot reach B's path — 403 + audited);
//! (d) the error-envelope shape matches the canon (`error.message` present); (e) a malformed request
//! → a clean error, no panic; (f) the SSE endpoint streams (a smoke).

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use myelin_edge::{
    serve_edge, sse_scope_for_resource, sse_scope_for_tenant, AllowAll, Gateway, Method, SseEvent, WhoamiHandler,
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
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};

const TENANT: &str = "acme";
const REGION: &str = "eu-west";
const OTHER_TENANT: &str = "globex";
const SCHEME: &str = "agent"; // a TTL-constrained token (no DPoP) — simplest real proof.

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn admin_scope(tenant: &str) -> TenantScope {
    TenantScope::from_verified_token(
        &Principal::stub(PrincipalId("admin".into()), PrincipalKind::Human, TenantId(tenant.into())),
        Region(REGION.into()),
    )
}

/// Build the gateway + a seeded S1 directory + the real cell authority, returning the gateway, the
/// cell (to mint tokens), and the revocation store (to revoke).
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
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(Arc::new(
        KmsEngine::new(),
    ))));

    let gateway = Arc::new(
        Gateway::builder(authn, human_login, Arc::new(AllowAll))
            .route(Method::Get, "/v1/whoami", "edge.whoami", Arc::new(WhoamiHandler))
            .route(
                Method::Get,
                "/v1/t/{tenant}/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
            .sse_route("/v1/t/{tenant}/events", "edge.events.subscribe", "edge")
            // R2.2: an OBJECT-ADDRESSED stream registers through the scoped path (the tenant-coarse
            // sse_route refuses an object-addressing pattern at composition time).
            .sse_route_scoped(
                "/v1/t/{tenant}/repos/{repo}/events",
                "git.repo.events.subscribe",
                "git",
                "repo",
            )
            .build(),
    );
    (gateway, cell, revocations)
}

/// Mint a real capability token for `(tenant, subj-1, jti)` expiring at `exp_unix`.
fn mint(cell: &CellTokenAuthority, tenant: &str, jti: &str, exp_unix: i64) -> String {
    cell.mint(&CapabilityMintSpec {
        tenant: tenant.into(),
        region: REGION.into(),
        subject_key: "subj-1".into(),
        jti: jti.into(),
        exp_unix,
        authority: vec!["agent:run".into()],
        dpop_jkt: None,
    })
}

/// Spawn the server on an ephemeral port; return its address.
async fn spawn(gateway: Arc<Gateway>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = serve_edge(listener, gateway).await;
    });
    addr
}

/// One finished HTTP round-trip → (status, body string).
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

/// Open a connection + send a request, returning the streaming response (for SSE).
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
    let mut builder = Request::builder().method(method).uri(path).header("host", "edge.test");
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    let req = builder.body(Full::new(Bytes::from(body))).unwrap();
    sender.send_request(req).await.unwrap()
}

/// Bearer auth header pair for a token (scheme = agent).
fn bearer(token: &str) -> [(&'static str, String); 2] {
    [
        ("authorization", format!("Bearer {token}")),
        ("x-myelin-token-scheme", SCHEME.to_string()),
    ]
}

fn hdr<'a>(b: &'a [(&'static str, String); 2]) -> Vec<(&'a str, &'a str)> {
    b.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

/// (a) A VALID capability token authenticates → resolves the principal → the tenant scope is set →
/// the whoami handler returns the verified tenant/principal.
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

/// (b) FORGED → 401 with the `{error:{message}}` envelope.
#[tokio::test]
async fn forged_token_is_401_with_envelope() {
    let (gw, _cell, _rev) = build_gateway();
    let addr = spawn(gw).await;
    // A hand-rolled plaintext envelope (what the OLD mock verifier would accept) — the real PASETO
    // verifier rejects it (not a signed v4.public token).
    let h = bearer("acme|eu-west|subj-1|jti|0|agent:run");
    let (status, body) = http(addr, "GET", "/v1/whoami", &hdr(&h), vec![]).await;
    assert_eq!(status, 401, "a forged token is rejected");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["message"], "authentication required", "envelope shape (d)");
    assert_eq!(v["error"]["code"], "unauthorized");
}

/// (b) EXPIRED → 401.
#[tokio::test]
async fn expired_token_is_401() {
    let (gw, cell, _rev) = build_gateway();
    let addr = spawn(gw).await;
    let token = mint(&cell, TENANT, "jti-exp", now() - 10); // already expired
    let h = bearer(&token);
    let (status, _body) = http(addr, "GET", "/v1/whoami", &hdr(&h), vec![]).await;
    assert_eq!(status, 401, "an expired token is rejected");
}

/// (b) REVOKED → 401 (the durable S7 revocation consult).
#[tokio::test]
async fn revoked_token_is_401() {
    let (gw, cell, revocations) = build_gateway();
    let addr = spawn(gw).await;
    let token = mint(&cell, TENANT, "jti-rev", now() + 3600);
    let h = bearer(&token);
    // Before revocation: authenticates.
    let (ok, _b) = http(addr, "GET", "/v1/whoami", &hdr(&h), vec![]).await;
    assert_eq!(ok, 200, "live token authenticates before revocation");
    // Revoke the jti in the verified (tenant, region) partition.
    revocations.revoke(
        &admin_scope(TENANT),
        &RevokeTarget::Jti("jti-rev".into()),
        Timestamp("2026-06-27T00:00:00Z".into()),
    );
    // After revocation: 401 (fail-closed).
    let (status, _body) = http(addr, "GET", "/v1/whoami", &hdr(&h), vec![]).await;
    assert_eq!(status, 401, "a revoked token fails closed at the edge");
}

/// (c) Tenant isolation AT THE EDGE: a token scoped to A cannot reach B's path — the path-tenant
/// mismatch is rejected (403) + AUDITED as a cross-tenant IDOR, and is never served. The same token
/// on its OWN tenant path is served.
#[tokio::test]
async fn tenant_isolation_at_the_edge_is_the_idor_floor() {
    let (gw, cell, _rev) = build_gateway();
    let token = mint(&cell, TENANT, "jti-idor", now() + 3600);
    let h = bearer(&token);
    let addr = spawn(gw.clone()).await;

    // The token (tenant acme) tries to reach globex's path → 403 + audited, NEVER served.
    let (status, body) = http(
        addr,
        "GET",
        &format!("/v1/t/{OTHER_TENANT}/whoami"),
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(status, 403, "a cross-tenant path is rejected at the edge: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "forbidden");
    assert_eq!(
        gw.public_surface().audit().count(),
        1,
        "the cross-tenant IDOR attempt was AUDITED (never swallowed)"
    );

    // The SAME token on its OWN tenant path is served against the token's tenant.
    let (ok, ok_body) = http(addr, "GET", &format!("/v1/t/{TENANT}/whoami"), &hdr(&h), vec![]).await;
    assert_eq!(ok, 200, "the own-tenant path is served");
    let ov: serde_json::Value = serde_json::from_str(&ok_body).unwrap();
    assert_eq!(ov["tenant"], TENANT, "served against the TOKEN's tenant, never the path");
}

/// (e) A malformed request → a clean error, no panic (the edge is total over garbage input).
#[tokio::test]
async fn malformed_requests_are_clean_errors_no_panic() {
    let (gw, _cell, _rev) = build_gateway();
    let addr = spawn(gw).await;
    // No credential → 401.
    let (no_cred, _) = http(addr, "GET", "/v1/whoami", &[], vec![]).await;
    assert_eq!(no_cred, 401);
    // Unknown route → 404.
    let (not_found, _) = http(addr, "GET", "/v1/does/not/exist", &[], vec![]).await;
    assert_eq!(not_found, 404);
    // Malformed login body → 400 (clean, no panic).
    let (bad_login, _) = http(addr, "POST", "/v1/auth/login", &[], b"{not json".to_vec()).await;
    assert_eq!(bad_login, 400);
    // Login with a well-formed body still REFUSES-not-mocks (503) — the human verifier is deferred.
    let (login, _) = http(
        addr,
        "POST",
        "/v1/auth/login",
        &[],
        br#"{"scheme":"oidc","material":"acme|eu-west|subj-1"}"#.to_vec(),
    )
    .await;
    assert_eq!(login, 503, "human login refuses-not-mocks until configured");
}

/// (f) The SSE endpoint streams (a smoke): subscribe over real HTTP, publish a frame to the verified
/// tenant's bounded scope, read the streamed event.
#[tokio::test]
async fn sse_endpoint_streams_a_frame() {
    let (gw, cell, _rev) = build_gateway();
    let token = mint(&cell, TENANT, "jti-sse", now() + 3600);
    let h = bearer(&token);
    let addr = spawn(gw.clone()).await;

    // Open the SSE stream (authenticated; scoped to the verified tenant).
    let resp = open(addr, "GET", &format!("/v1/t/{TENANT}/events"), &hdr(&h), vec![]).await;
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/event-stream",
        "the SSE content-type"
    );
    let mut body = resp.into_body();

    // The subscription now exists (the handler subscribed before returning the response). Publish a
    // frame to the SAME (stream, scope) the gateway derived (tenant-bounded), retrying to avoid the
    // subscribe/publish scheduling race, and read the streamed event.
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
    assert!(frame.contains("event: ping"), "the SSE frame carries the event type: {frame}");
    assert!(frame.contains("data: {\"hello\":true}"), "the SSE frame carries the data: {frame}");
}

/// (g) R2.2: an OBJECT-ADDRESSED SSE route subscribes at the tenant+resource scope — NOT the
/// tenant-coarse one. A frame published tenant-coarse (or to a DIFFERENT object) never reaches the
/// per-object subscriber; the frame published to the derived `(tenant, repo)` scope does. This is
/// the live proof of the registration contract: the subscription key is the object's, so a
/// per-object stream cannot leak other objects' frames.
#[tokio::test]
async fn scoped_sse_route_isolates_per_object() {
    let (gw, cell, _rev) = build_gateway();
    let token = mint(&cell, TENANT, "jti-sse-scoped", now() + 3600);
    let h = bearer(&token);
    let addr = spawn(gw.clone()).await;

    // Open the per-object stream for repo `widgets`.
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
        // Frames the widgets subscriber must NEVER receive: the tenant-coarse scope and another
        // object's scope (different (stream, scope) channels entirely).
        gw.sse_hub().broadcast("git", &coarse, SseEvent::typed("leak", "{\"coarse\":true}"));
        gw.sse_hub().broadcast("git", &other, SseEvent::typed("leak", "{\"other\":true}"));
        // The frame it MUST receive: its own derived (tenant, repo) scope.
        gw.sse_hub().broadcast("git", &widgets, SseEvent::typed("push", "{\"repo\":\"widgets\"}"));
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
