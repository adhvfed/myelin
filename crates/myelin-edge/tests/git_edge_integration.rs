use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use myelin_edge::{
    register_git, serve_edge, AllowAll, Gateway, GitEdgeState, Method, WhoamiHandler,
};
use myelin_git::web::{switch_test_representative_pr_page, RepoHome, WebEditForm};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore,
};
use myelin_storage::{KmsEngine, TenantScope};
use myelin_tenancy::{Region, TenantId};
use std::net::SocketAddr;
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

fn repo(slug: &str) -> RepoHome {
    RepoHome::Populated {
        slug: slug.into(),
        readme_excerpt: format!("# {slug}"),
        entries: vec![("README.md".into(), false)],
        clone_url: format!("ssh://git@myelin/{slug}.git"),
    }
}

fn seed_tenant(store: &PrincipalStore, tenant: &str) {
    let scope = admin_scope(tenant);
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
}

fn build() -> (Arc<Gateway>, CellTokenAuthority) {
    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    seed_tenant(&store, "acme");
    seed_tenant(&store, "globex");

    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));

    let git_state = Arc::new(
        GitEdgeState::new()
            .with_repo("acme", "alpha", repo("acme/alpha"))
            .with_repo("acme", "beta", repo("acme/beta"))
            .with_repo("acme", "gamma", repo("acme/gamma"))
            .with_pr(
                "acme",
                "alpha",
                1,
                switch_test_representative_pr_page("acme"),
            )
            .with_blob(
                "acme",
                "alpha",
                "main",
                "README.md",
                WebEditForm {
                    path: "README.md".into(),
                    contents: "# acme/alpha\n".into(),
                    base_oid: "blake3:acmehead".into(),
                    viewer_may_edit: true,
                },
            )
            .with_repo("globex", "zeta", repo("globex/zeta"))
            .with_pr(
                "globex",
                "zeta",
                7,
                switch_test_representative_pr_page("globex"),
            ),
    );

    let mut builder = Gateway::builder(authn, human_login, Arc::new(AllowAll)).route(
        Method::Get,
        "/v1/whoami",
        "edge.whoami",
        Arc::new(WhoamiHandler),
    );
    builder = register_git(builder, git_state);
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

#[tokio::test]
async fn repos_list_and_pr_overview_serve_the_viewmodel_json() {
    let (gw, cell) = build();
    let addr = spawn(gw).await;
    let h = bearer(&mint(&cell, "acme", "jti-a1"));

    let (status, v) = http(addr, "GET", "/v1/git/repos", &hdr(&h), vec![]).await;
    assert_eq!(status, 200, "repos list: {v}");
    let items = v["items"].as_array().expect("items array");
    assert_eq!(items.len(), 3, "acme has 3 repos");
    assert_eq!(
        items[0]["state"], "populated",
        "the RepoHome ViewModel vocabulary"
    );
    assert!(items[0]["slug"].as_str().unwrap().starts_with("acme/"));
    assert_eq!(v["page"]["limit"], 50, "the default page limit");

    let (status, pr) = http(addr, "GET", "/v1/git/repos/alpha/prs/1", &hdr(&h), vec![]).await;
    assert_eq!(status, 200, "pr overview: {pr}");
    assert_eq!(pr["visible"], true);
    assert!(
        pr["title"].as_str().unwrap().contains("acme"),
        "the projection title (tenant-framed): {}",
        pr["title"]
    );
    assert_eq!(pr["pr_state"], "open");
    assert_eq!(pr["checks"]["state"], "live", "the checks panel ViewModel");
    assert_eq!(pr["checks"]["rows"][0]["context"], "ci/build");
    assert_eq!(pr["checks"]["rows"][0]["cue"]["label"], "passed");
    assert_eq!(
        pr["merge"]["state"], "ready",
        "the merge-readiness ViewModel"
    );
    assert_eq!(pr["merge"]["approvals"]["required"], 2);

    let (cs, checks) = http(
        addr,
        "GET",
        "/v1/git/repos/alpha/prs/1/checks",
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(cs, 200);
    assert_eq!(checks["state"], "live");
    assert_eq!(checks["rows"][0]["required"], true);
}

#[tokio::test]
async fn tenant_isolation_git_data_is_partitioned_by_the_verified_token() {
    let (gw, cell) = build();
    let addr = spawn(gw).await;
    let acme = bearer(&mint(&cell, "acme", "jti-acme"));
    let globex = bearer(&mint(&cell, "globex", "jti-globex"));

    let (_, av) = http(addr, "GET", "/v1/git/repos", &hdr(&acme), vec![]).await;
    let acme_repos = av["items"].as_array().unwrap();
    assert_eq!(acme_repos.len(), 3);
    assert!(acme_repos
        .iter()
        .all(|r| r["slug"].as_str().unwrap().starts_with("acme/")));

    let (_, gv) = http(addr, "GET", "/v1/git/repos", &hdr(&globex), vec![]).await;
    let globex_repos = gv["items"].as_array().unwrap();
    assert_eq!(globex_repos.len(), 1, "globex has exactly its own repo");
    assert_eq!(globex_repos[0]["slug"], "globex/zeta");
    assert!(
        !gv.to_string().contains("acme"),
        "no acme data bleeds into globex's response: {gv}"
    );

    let (st, _) = http(addr, "GET", "/v1/git/repos/zeta/prs/7", &hdr(&acme), vec![]).await;
    assert_eq!(st, 404, "globex's PR is invisible to an acme token");
    let (st_ok, gpr) = http(
        addr,
        "GET",
        "/v1/git/repos/zeta/prs/7",
        &hdr(&globex),
        vec![],
    )
    .await;
    assert_eq!(st_ok, 200);
    assert!(gpr["title"].as_str().unwrap().contains("globex"));
}

#[tokio::test]
async fn forged_token_is_401_on_a_git_route() {
    let (gw, _cell) = build();
    let addr = spawn(gw).await;
    let h = bearer("acme|eu-west|subj-1|jti|0|agent:run");
    let (status, v) = http(addr, "GET", "/v1/git/repos", &hdr(&h), vec![]).await;
    assert_eq!(status, 401, "a forged token is rejected on git routes");
    assert_eq!(v["error"]["message"], "authentication required");
    assert_eq!(v["error"]["code"], "unauthorized");
    let (nc, _) = http(addr, "GET", "/v1/git/repos", &[], vec![]).await;
    assert_eq!(nc, 401);
}

#[tokio::test]
async fn repo_list_pagination_limit_and_cursor() {
    let (gw, cell) = build();
    let addr = spawn(gw).await;
    let h = bearer(&mint(&cell, "acme", "jti-page"));

    let (_, p1) = http(addr, "GET", "/v1/git/repos?limit=2", &hdr(&h), vec![]).await;
    assert_eq!(p1["items"].as_array().unwrap().len(), 2);
    assert_eq!(p1["page"]["limit"], 2);
    let cursor = p1["page"]["next_cursor"].as_str().expect("a next cursor");

    let (_, p2) = http(
        addr,
        "GET",
        &format!("/v1/git/repos?limit=2&cursor={cursor}"),
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(p2["items"].as_array().unwrap().len(), 1, "the last page");
    assert!(
        p2["page"]["next_cursor"].is_null(),
        "no cursor past the end"
    );
}

#[tokio::test]
async fn writes_are_wired_but_durable_effect_is_deferred_to_e1_1() {
    let (gw, cell) = build();
    let addr = spawn(gw).await;
    let h = bearer(&mint(&cell, "acme", "jti-write"));

    let (st, mv) = http(
        addr,
        "POST",
        "/v1/git/repos/alpha/prs/1/merge",
        &hdr(&h),
        br#"{"strategy":"squash"}"#.to_vec(),
    )
    .await;
    assert_eq!(st, 200, "the merge route is wired: {mv}");
    assert_eq!(
        mv["durable"], false,
        "the durable effect is honestly deferred to E1.1"
    );
    assert_eq!(mv["applied"]["action"], "git.pr.merge");
    assert!(mv["note"].as_str().unwrap().contains("E1.1"));

    let (stale, sv) = http(
        addr,
        "POST",
        "/v1/git/repos/alpha/blob/main/README.md",
        &hdr(&h),
        br##"{"base_oid":"blake3:STALE","contents":"# edited\n","message":"edit"}"##.to_vec(),
    )
    .await;
    assert_eq!(stale, 409, "a stale base is refused (GF-6): {sv}");
    assert_eq!(sv["error"]["code"], "conflict");

    let (ok, ov) = http(
        addr,
        "POST",
        "/v1/git/repos/alpha/blob/main/README.md",
        &hdr(&h),
        br##"{"base_oid":"blake3:acmehead","contents":"# edited\n","message":"edit"}"##.to_vec(),
    )
    .await;
    assert_eq!(ok, 200, "a clean base is accepted: {ov}");
    assert_eq!(ov["applied"]["outcome"], "committed");
    assert_eq!(ov["durable"], false);
}
