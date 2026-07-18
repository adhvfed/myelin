//! # R2.6 — the production action gate over real HTTP (AuthenticatedActionPolicy, not AllowAll).
//!
//! Boots the edge over the SAME route surface the production `main.rs` composes (whoami + the SSE
//! subscribe route + `register_git_durable` + `register_git_wire`) with the PRODUCTION action
//! authorizer — [`AuthenticatedActionPolicy::mounted`] — and proves over a real TCP listener:
//!
//!  - **No mounted route regresses:** every route in Git's own catalogue (iterated from
//!    `myelin_git::api::http_catalogue()`, re-rooted exactly as `register_git_durable` re-roots it),
//!    the GT-004 browse reads, the wire endpoints, and whoami are all ADMITTED by the action gate
//!    (whatever else they return — 200/201/4xx domain errors — they never die on the action-gate
//!    403). Because the catalogue is iterated from Git's OWN table, a future catalogue entry whose
//!    action is missing from [`myelin_edge::MOUNTED_EDGE_ACTIONS`] (or that falls into the
//!    `git.unmapped:*` placeholder) FAILS this test — the anti-drift gate for the allowlist.
//!  - **Deny-by-default is live on the wire:** a route registered with an action verb OUTSIDE the
//!    allowlist is refused with the action-gate 403 (`authorization denied for action ...`).
//!  - **The gate runs after authn:** an unauthenticated request to a mounted route is a 401 (the
//!    policy never turns a missing token into anything weaker).

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use myelin_edge::{
    action_requirement, register_git_durable, register_git_wire, serve_edge,
    AuthenticatedActionPolicy, DurableGitBackend, Gateway, Method, WhoamiHandler,
};
use myelin_git::api::{http_catalogue, Method as GitMethod};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore,
};
use myelin_storage::{KmsEngine, TenantScope};
use myelin_tenancy::{Region, TenantId};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};

const REGION: &str = "eu-west";
const SCHEME: &str = "agent";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn temp_root(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("myelin-edge-r26-{tag}-{nanos}"));
    p
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

fn seed_principal(store: &PrincipalStore, tenant: &str, pid: &str, subject_key: &str) {
    let scope = admin_scope(tenant);
    store
        .put_principal(
            &scope,
            PrincipalId(pid.into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .expect("seed principal");
    store
        .link_credential(&scope, SCHEME, subject_key, &PrincipalId(pid.into()))
        .expect("link credential");
}

/// Build the gateway over the PRODUCTION-shaped route surface (whoami + SSE + git JSON API + git
/// wire) with the PRODUCTION action authorizer, plus one probe route whose action is deliberately
/// NOT in the mounted allowlist (the deny-by-default oracle).
fn build(root: &std::path::Path) -> (Arc<Gateway>, CellTokenAuthority) {
    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    seed_principal(&store, "acme", "svc:agent", "subj-1");

    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));

    let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(root.to_path_buf()));
    // The PRODUCTION action gate — the whole point of this harness.
    let mut builder = Gateway::builder(
        authn,
        human_login,
        Arc::new(AuthenticatedActionPolicy::mounted()),
    )
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
    .sse_route("/v1/t/{tenant}/events", "edge.events.subscribe", "edge")
    // The deny-by-default probe: a real handler behind an action verb that is NOT mounted.
    .route(
        Method::Get,
        "/v1/unmounted-probe",
        "edge.test.unmounted_probe",
        Arc::new(WhoamiHandler),
    );
    builder = register_git_durable(builder, backend.clone());
    builder = register_git_wire(builder, backend);
    let registered: Vec<String> = builder.registered_actions().map(str::to_string).collect();
    let missing: Vec<&str> = registered
        .iter()
        .map(String::as_str)
        .filter(|action| {
            *action != "edge.test.unmounted_probe" && action_requirement(action).is_none()
        })
        .collect();
    assert!(
        missing.is_empty(),
        "registered route actions without capability rules: {missing:?}"
    );
    assert!(
        registered.iter().any(|action| action == "git.pr.commits"),
        "composition regression must include a route discovered missing from the old allowlist"
    );
    (Arc::new(builder.build()), cell)
}

fn mint(cell: &CellTokenAuthority, tenant: &str, jti: &str) -> String {
    cell.mint(&CapabilityMintSpec {
        tenant: tenant.into(),
        region: REGION.into(),
        subject_key: "subj-1".into(),
        jti: jti.into(),
        exp_unix: now() + 3600,
        authority: vec!["edge.operator".into()],
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

async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (u16, serde_json::Value) {
    let resp = open(addr, method, path, headers, body).await;
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
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

/// Whether a response is a 403. The edge's `Forbidden` envelope is deliberately UNIFORM/oracle-free
/// (`{"error":{"code":"forbidden","message":"forbidden"}}` — it never names the action/resource),
/// so the harness attributes a 403 structurally instead: authn is a valid token (else 401), the
/// path tenant equals the token tenant (no IDOR 403), and the object-authz seam is the
/// `AllowAllRepos` test fixture (never denies) — leaving the ACTION GATE as the only 403 source
/// for the routes this test drives.
fn is_action_gate_denial(status: u16, _body: &serde_json::Value) -> bool {
    status == 403
}

/// Substitute the catalogue's `{param}` segments with concrete values that resolve against the
/// `alpha` repo this test creates.
fn concretize(pattern: &str) -> String {
    pattern
        .split('/')
        .map(|seg| match seg {
            "{repo}" => "alpha",
            "{n}" => "1",
            "{ref}" => "main",
            "{path}" => "README.md",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The anti-drift gate: EVERY route the production edge mounts is admitted by the mounted policy.
/// The git JSON routes are iterated from Git's OWN catalogue (the same table
/// `register_git_durable` iterates), so a new catalogue entry whose action verb is not in
/// `MOUNTED_EDGE_ACTIONS` (or that lands in the `git.unmapped:*` placeholder) breaks THIS test.
#[tokio::test]
async fn the_mounted_policy_admits_every_production_route() {
    let root = temp_root("admit");
    let (gw, cell) = build(&root);
    let addr = spawn(gw).await;
    let h = bearer(&mint(&cell, "acme", "jti-r26-a"));

    // whoami — the canonical mounted non-git action, end-to-end 200.
    let (st, v) = http(addr, "GET", "/v1/whoami", &hdr(&h), vec![]).await;
    assert_eq!(st, 200, "whoami must pass the action gate: {v}");

    // Create the repo the concrete catalogue paths resolve against (also proves git.repo.create).
    let (st, v) = http(
        addr,
        "POST",
        "/v1/git/repos",
        &hdr(&h),
        br#"{"slug":"alpha"}"#.to_vec(),
    )
    .await;
    assert_eq!(
        st, 201,
        "create-repo must pass the action gate and apply: {v}"
    );

    // Every git catalogue route, re-rooted exactly as register_git_durable re-roots it.
    for ep in http_catalogue() {
        let path = concretize(&ep.path.replacen("/api/git", "/v1/git", 1));
        let (method, body) = match ep.method {
            GitMethod::Get => ("GET", Vec::new()),
            GitMethod::Post => ("POST", b"{}".to_vec()),
        };
        let (st, v) = http(addr, method, &path, &hdr(&h), body).await;
        if path.ends_with("/checks") && method == "POST" {
            assert_eq!(
                st, 403,
                "OperatorBootstrap is deliberately not a CI attestation purpose: {v}"
            );
            continue;
        }
        assert!(
            !is_action_gate_denial(st, &v),
            "the action gate DENIED the mounted catalogue route {method} {path} \
             (status {st}: {v}) — MOUNTED_EDGE_ACTIONS drifted from register_git_durable"
        );
    }

    // The GT-004 browse reads register_git_durable adds beyond the catalogue.
    for path in [
        "/v1/git/repos/alpha",
        "/v1/git/repos/alpha/commits/main",
        "/v1/git/repos/alpha/commit/0000000000000000000000000000000000000000",
    ] {
        let (st, v) = http(addr, "GET", path, &hdr(&h), vec![]).await;
        assert!(
            !is_action_gate_denial(st, &v),
            "the action gate DENIED the mounted browse route GET {path} (status {st}: {v})"
        );
    }

    // The git smart-HTTP wire routes (register_git_wire).
    for (method, path) in [
        (
            "GET",
            "/acme/eu-west/alpha/info/refs?service=git-upload-pack",
        ),
        ("POST", "/acme/eu-west/alpha/git-upload-pack"),
        ("POST", "/acme/eu-west/alpha/git-receive-pack"),
    ] {
        let (st, v) = http(addr, method, path, &hdr(&h), vec![]).await;
        assert!(
            !is_action_gate_denial(st, &v),
            "the action gate DENIED the mounted wire route {method} {path} (status {st}: {v})"
        );
    }
}

/// Deny-by-default is LIVE on the wire: an action verb outside the mounted allowlist is refused
/// with the action-gate 403 — for an otherwise fully authenticated, tenant-scoped principal.
#[tokio::test]
async fn an_unmounted_action_is_denied_with_the_action_gate_403() {
    let root = temp_root("deny");
    let (gw, cell) = build(&root);
    let addr = spawn(gw).await;
    let h = bearer(&mint(&cell, "acme", "jti-r26-d"));

    let (st, v) = http(addr, "GET", "/v1/unmounted-probe", &hdr(&h), vec![]).await;
    assert_eq!(
        st, 403,
        "a route behind an UNMOUNTED action verb must be refused by the action gate \
         (the handler would have answered 200 if admitted — same handler as whoami): {v}"
    );
    // The denial envelope stays UNIFORM/oracle-free (never names the action/resource to the client).
    assert_eq!(
        v["error"]["code"], "forbidden",
        "uniform forbidden envelope: {v}"
    );
    assert_eq!(
        v["error"]["message"], "forbidden",
        "no detail leak in the 403 body: {v}"
    );
}

/// The policy never weakens authn: a mounted action WITHOUT a token is still the uniform 401 (the
/// gate runs AFTER authenticate — an anonymous caller never reaches an allowlist decision).
#[tokio::test]
async fn a_mounted_action_without_a_token_is_still_401() {
    let root = temp_root("authn");
    let (gw, _cell) = build(&root);
    let addr = spawn(gw).await;

    let (st, v) = http(addr, "GET", "/v1/whoami", &[], vec![]).await;
    assert_eq!(
        st, 401,
        "no token → uniform 401 (never a policy allow): {v}"
    );
}
