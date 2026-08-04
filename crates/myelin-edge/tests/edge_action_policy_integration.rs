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

fn is_action_gate_denial(status: u16, _body: &serde_json::Value) -> bool {
    status == 403
}

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

#[tokio::test]
async fn the_mounted_policy_admits_every_production_route() {
    let root = temp_root("admit");
    let (gw, cell) = build(&root);
    let addr = spawn(gw).await;
    let h = bearer(&mint(&cell, "acme", "jti-r26-a"));

    let (st, v) = http(addr, "GET", "/v1/whoami", &hdr(&h), vec![]).await;
    assert_eq!(st, 200, "whoami must pass the action gate: {v}");

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
             (status {st}: {v}) - MOUNTED_EDGE_ACTIONS drifted from register_git_durable"
        );
    }

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
         (the handler would have answered 200 if admitted - same handler as whoami): {v}"
    );
    assert_eq!(
        v["error"]["code"], "forbidden",
        "uniform forbidden envelope: {v}"
    );
    assert_eq!(
        v["error"]["message"], "forbidden",
        "no detail leak in the 403 body: {v}"
    );
}

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
