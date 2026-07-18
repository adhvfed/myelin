//! # GT-003 — Git durable front door: real HTTP integration proofs (builder ≠ verifier oracle).
//!
//! Binds an ephemeral TCP port, serves the edge with Git's routes registered over the DURABLE on-disk
//! backend ([`myelin_edge::register_git_durable`]), and drives REAL HTTP round-trips with real minted
//! capability tokens. Proves:
//!  - (A) **writes PERSIST** — create-repo + a web-edit commit through the edge are read back from disk;
//!        a FRESH backend instance over the SAME root (a simulated restart) still serves them.
//!  - (B) **merge-gate ENFORCED + durable ref-advance** — a merge with unmet required checks is REFUSED
//!        (no ref advance); with checks green + an approval the merge advances the base ref durably.
//!  - (C) **tenant isolation + traversal-safety** — an acme repo is invisible to globex; a `../`-laden
//!        repo slug is refused (the validated resolver), never escaping the tenant root.

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::Request;
use hyper::Response;
use hyper_util::rt::TokioIo;
use myelin_edge::{
    register_git_durable, serve_edge, AllowAll, DurableGitBackend, Gateway, Method, WhoamiHandler,
};
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
    p.push(format!("myelin-edge-gt003-{tag}-{nanos}"));
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
    store
        .link_credential(&scope, "ci", subject_key, &PrincipalId(pid.into()))
        .expect("link CI credential");
}

fn seed_tenant(store: &PrincipalStore, tenant: &str) {
    // Two principals per tenant: an author (subj-1) and a distinct reviewer (subj-2) — so a NON-author
    // approval can be genuinely submitted (a self-approval must not count toward the threshold).
    seed_principal(store, tenant, "svc:agent", "subj-1");
    seed_principal(store, tenant, "svc:reviewer", "subj-2");
}

/// A test authorizer that DENIES one action (proving the gateway re-authorizes per action). All else is
/// allowed. Used to prove the repo-admin branch-protection op is authorization-gated.
struct DenyAction(&'static str);
impl myelin_substrate::Authorizer for DenyAction {
    fn authorize(&self, _principal: &Principal, action: &str) -> bool {
        action != self.0
    }
}

/// Build a gateway with Git registered over a DURABLE backend rooted at `root` + a chosen authorizer.
fn build_with(
    root: &std::path::Path,
    authorizer: Arc<dyn myelin_substrate::Authorizer>,
) -> (Arc<Gateway>, CellTokenAuthority) {
    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    seed_tenant(&store, "acme");
    seed_tenant(&store, "globex");

    let revocations = RevocationStore::new();
    for tenant in ["acme", "globex"] {
        revocations.register_run_token_ttl(
            &admin_scope(tenant),
            &format!("jti-ci-{tenant}"),
            myelin_events::Timestamp("2020-01-01T00:00:00Z".into()),
            myelin_events::Timestamp("2099-01-01T00:00:00Z".into()),
        );
    }
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        revocations,
    ));
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));

    let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(root.to_path_buf()));
    let mut builder = Gateway::builder(authn, human_login, authorizer).route(
        Method::Get,
        "/v1/whoami",
        "edge.whoami",
        Arc::new(WhoamiHandler),
    );
    builder = register_git_durable(builder, backend);
    (Arc::new(builder.build()), cell)
}

fn build(root: &std::path::Path) -> (Arc<Gateway>, CellTokenAuthority) {
    build_with(root, Arc::new(AllowAll))
}

fn mint(cell: &CellTokenAuthority, tenant: &str, jti: &str) -> String {
    mint_as(cell, tenant, "subj-1", jti)
}

fn mint_as(cell: &CellTokenAuthority, tenant: &str, subject_key: &str, jti: &str) -> String {
    cell.mint(&CapabilityMintSpec {
        tenant: tenant.into(),
        region: REGION.into(),
        subject_key: subject_key.into(),
        jti: jti.into(),
        exp_unix: now() + 3600,
        authority: vec!["edge.operator".into()],
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::OperatorBootstrap,
        audience: myelin_identity_service::CredentialAudience::Edge,
    })
}

fn mint_ci(cell: &CellTokenAuthority, tenant: &str, subject_key: &str) -> String {
    cell.mint(&CapabilityMintSpec {
        tenant: tenant.into(),
        region: REGION.into(),
        subject_key: subject_key.into(),
        jti: format!("jti-ci-{tenant}"),
        exp_unix: now() + 3600,
        authority: vec!["ci.checks.report".into()],
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::CiJob {
            run_id: "ci-checks".into(),
        },
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

fn bearer_ci(token: &str) -> [(&'static str, String); 2] {
    [
        ("authorization", format!("Bearer {token}")),
        ("x-myelin-token-scheme", "ci".into()),
    ]
}
fn hdr<'a>(b: &'a [(&'static str, String); 2]) -> Vec<(&'a str, &'a str)> {
    b.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

/// (A) Writes PERSIST: create-repo + a web-edit commit are durable, reads reflect them, and a FRESH
/// backend over the SAME on-disk root (a simulated restart) still serves them.
#[tokio::test]
async fn writes_persist_across_a_fresh_backend_restart() {
    let root = temp_root("persist");
    let (gw, cell) = build(&root);
    let addr = spawn(gw).await;
    let h = bearer(&mint(&cell, "acme", "jti-a1"));

    // create-repo → durable:true.
    let (st, cv) = http(
        addr,
        "POST",
        "/v1/git/repos",
        &hdr(&h),
        br#"{"slug":"alpha"}"#.to_vec(),
    )
    .await;
    assert_eq!(st, 201, "create-repo: {cv}");
    assert_eq!(cv["durable"], true);

    // a fresh repo lists as Empty (no commits) — the durable read, not a seed.
    let (_, lv) = http(addr, "GET", "/v1/git/repos", &hdr(&h), vec![]).await;
    assert_eq!(
        lv["items"].as_array().unwrap().len(),
        1,
        "alpha listed: {lv}"
    );
    assert_eq!(lv["items"][0]["state"], "empty");

    // web-edit commit on main creates README.md durably.
    let (wc, wv) = http(
        addr,
        "POST",
        "/v1/git/repos/alpha/blob/main/README.md",
        &hdr(&h),
        br##"{"base_oid":"","contents":"# acme/alpha durable\n","message":"init"}"##.to_vec(),
    )
    .await;
    assert_eq!(wc, 200, "web-edit: {wv}");
    assert_eq!(
        wv["durable"], true,
        "the web-edit commit PERSISTS (durable:true)"
    );
    assert_eq!(wv["applied"]["outcome"], "committed");

    // the blob view now reflects the durable write.
    let (bc, bv) = http(
        addr,
        "GET",
        "/v1/git/repos/alpha/blob/main/README.md",
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(bc, 200, "blob: {bv}");
    assert_eq!(bv["contents"], "# acme/alpha durable\n");
    let base_oid = bv["base_oid"].as_str().unwrap().to_string();
    assert!(
        !base_oid.is_empty(),
        "the durable blob carries a real content-address"
    );

    // repo now lists as Populated with the README in the tree.
    let (_, lv2) = http(addr, "GET", "/v1/git/repos", &hdr(&h), vec![]).await;
    assert_eq!(lv2["items"][0]["state"], "populated");

    // A stale base is the honest 409 (GF-6) — no silent overwrite.
    let (sc, _sv) = http(
        addr,
        "POST",
        "/v1/git/repos/alpha/blob/main/README.md",
        &hdr(&h),
        br##"{"base_oid":"blake3:STALE","contents":"# clobber\n","message":"x"}"##.to_vec(),
    )
    .await;
    assert_eq!(sc, 409, "a stale base is refused");

    // === RESTART: a FRESH backend + gateway over the SAME root. ===
    let (gw2, cell2) = build(&root);
    let addr2 = spawn(gw2).await;
    let h2 = bearer(&mint(&cell2, "acme", "jti-a2"));
    let (rc, rv) = http(addr2, "GET", "/v1/git/repos", &hdr(&h2), vec![]).await;
    assert_eq!(rc, 200);
    assert_eq!(
        rv["items"].as_array().unwrap().len(),
        1,
        "alpha survived the restart"
    );
    assert_eq!(rv["items"][0]["state"], "populated");
    let (bc2, bv2) = http(
        addr2,
        "GET",
        "/v1/git/repos/alpha/blob/main/README.md",
        &hdr(&h2),
        vec![],
    )
    .await;
    assert_eq!(bc2, 200);
    assert_eq!(
        bv2["contents"], "# acme/alpha durable\n",
        "the web-edit survived the restart"
    );
    assert_eq!(
        bv2["base_oid"], base_oid,
        "same durable content-address after restart"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// (B) **Merge bypass CLOSED + gated, durable ref-advance.** Branch-protection policy is REPO-OWNED, not
/// author-supplied: a PR author cannot weaken the gate by passing loose policy / self-claimed greens at
/// open (those fields are ignored) or by self-approving. A protected ref defaults CLOSED. The merge
/// advances the base ref durably ONLY with the repo-required checks genuinely green (CI-reported) + a
/// genuine NON-author approval; an arbitrary head_oid is refused.
#[tokio::test]
async fn merge_bypass_is_closed_and_advance_is_gated_and_durable() {
    let root = temp_root("merge");
    let (gw, cell) = build(&root);
    let addr = spawn(gw).await;
    let author = bearer(&mint(&cell, "acme", "jti-author")); // subj-1 → svc:agent (the PR author)
    let reviewer = bearer(&mint_as(&cell, "acme", "subj-2", "jti-rev")); // svc:reviewer (non-author)
    let ci = bearer_ci(&mint_ci(&cell, "acme", "subj-1"));

    http(
        addr,
        "POST",
        "/v1/git/repos",
        &hdr(&author),
        br#"{"slug":"svc"}"#.to_vec(),
    )
    .await;
    // The author creates a feature head commit via a web-edit; capture its commit oid.
    let (_, wv) = http(
        addr,
        "POST",
        "/v1/git/repos/svc/blob/feature/app.txt",
        &hdr(&author),
        br#"{"base_oid":"","contents":"v1\n","message":"feat"}"#.to_vec(),
    )
    .await;
    let head_oid = wv["applied"]["new_oid"].as_str().unwrap().to_string();

    // ===== ATTACK (A+B): the author opens a PR to PROTECTED main supplying loose policy + greens in the
    // body (all IGNORED), then SELF-approves, then tries to merge. Must be BLOCKED. =====
    let attack_body = format!(
        r#"{{"title":"Attack PR","base_ref":"refs/heads/main","head_ref":"refs/heads/feature","head_oid":"{head_oid}","required_contexts":[],"required_approvals":0,"green_contexts":["ci/build"]}}"#
    );
    let (oc1, _o1) = http(
        addr,
        "POST",
        "/v1/git/repos/svc/prs",
        &hdr(&author),
        attack_body.into_bytes(),
    )
    .await;
    assert_eq!(oc1, 201);
    // self-approval by the author.
    http(
        addr,
        "POST",
        "/v1/git/repos/svc/prs/1/reviews",
        &hdr(&author),
        br#"{"verdict":"approve"}"#.to_vec(),
    )
    .await;
    let (mc1, mv1) = http(
        addr,
        "POST",
        "/v1/git/repos/svc/prs/1/merge",
        &hdr(&author),
        vec![],
    )
    .await;
    assert_eq!(
        mc1, 409,
        "the bypass is closed: loose policy + self-approval cannot merge protected main: {mv1}"
    );
    let (gb, _gv) = http(
        addr,
        "GET",
        "/v1/git/repos/svc/blob/main/app.txt",
        &hdr(&author),
        vec![],
    )
    .await;
    assert_eq!(gb, 404, "no ref advance on the blocked bypass attempt");

    // ===== LEGIT: repo-admin configures protection (repo-owned); CI reports greens; a NON-author
    // reviewer approves; then the merge is admitted and advances the ref durably. =====
    let (sp, _sv) = http(
        addr,
        "POST",
        "/v1/git/repos/svc/branch-protection",
        &hdr(&author), // AllowAll here; the AUTHZ gate is proven in the dedicated test below
        br#"{"rulesets":[{"ref_pattern":"refs/heads/main","required_contexts":["ci/build"],"required_approvals":1}]}"#.to_vec(),
    )
    .await;
    assert_eq!(sp, 200, "repo-admin sets branch protection");

    // open a fresh PR #2 (the proposal only — no policy/greens accepted).
    let body2 = format!(
        r#"{{"title":"PR two","base_ref":"refs/heads/main","head_ref":"refs/heads/feature","head_oid":"{head_oid}"}}"#
    );
    http(
        addr,
        "POST",
        "/v1/git/repos/svc/prs",
        &hdr(&author),
        body2.into_bytes(),
    )
    .await;

    // no greens, no approval → blocked.
    let (m2a, _) = http(
        addr,
        "POST",
        "/v1/git/repos/svc/prs/2/merge",
        &hdr(&author),
        vec![],
    )
    .await;
    assert_eq!(m2a, 409, "repo-required ci/build not green → blocked");

    // CI reports the required check green (the authorized producer path — NOT the author at open).
    let (cr, _crv) = http(
        addr,
        "POST",
        "/v1/git/repos/svc/prs/2/checks",
        &hdr(&ci),
        br#"{"green_contexts":["ci/build"]}"#.to_vec(),
    )
    .await;
    assert_eq!(cr, 200);
    // green but still no NON-author approval → blocked.
    let (m2b, _) = http(
        addr,
        "POST",
        "/v1/git/repos/svc/prs/2/merge",
        &hdr(&author),
        vec![],
    )
    .await;
    assert_eq!(m2b, 409, "green but no non-author approval → blocked");

    // a genuine NON-author approval (the reviewer principal).
    http(
        addr,
        "POST",
        "/v1/git/repos/svc/prs/2/reviews",
        &hdr(&reviewer),
        br#"{"verdict":"approve"}"#.to_vec(),
    )
    .await;
    let (m2c, mv2c) = http(
        addr,
        "POST",
        "/v1/git/repos/svc/prs/2/merge",
        &hdr(&author),
        vec![],
    )
    .await;
    assert_eq!(
        m2c, 200,
        "genuine greens + non-author approval → admitted: {mv2c}"
    );
    assert_eq!(
        mv2c["applied"]["new_oid"], head_oid,
        "base ref advanced to the head"
    );

    // The merged ref is durable across a fresh-backend restart.
    let (gw2, cell2) = build(&root);
    let addr2 = spawn(gw2).await;
    let h2 = bearer(&mint(&cell2, "acme", "jti-m2"));
    let (gb3, gv3) = http(
        addr2,
        "GET",
        "/v1/git/repos/svc/blob/main/app.txt",
        &hdr(&h2),
        vec![],
    )
    .await;
    assert_eq!(gb3, 200, "the merged ref survived the restart");
    assert_eq!(gv3["contents"], "v1\n");

    // ===== INVALID HEAD: a PR naming a bogus head_oid is refused (no advance to an arbitrary oid). =====
    let bogus = "0".repeat(40);
    let body3 = format!(
        r#"{{"title":"PR three","base_ref":"refs/heads/feat2","head_ref":"refs/heads/x","head_oid":"{bogus}"}}"#
    );
    http(
        addr,
        "POST",
        "/v1/git/repos/svc/prs",
        &hdr(&author),
        body3.into_bytes(),
    )
    .await;
    let (m3, _m3v) = http(
        addr,
        "POST",
        "/v1/git/repos/svc/prs/3/merge",
        &hdr(&author),
        vec![],
    )
    .await;
    assert_eq!(m3, 400, "a non-existent head_oid is refused");

    std::fs::remove_dir_all(&root).ok();
}

/// (D) The repo-admin branch-protection op is AUTHORIZATION-gated: a gateway whose authorizer denies
/// `git.repo.branch_protection.set` rejects the call (403) — a non-admin cannot set/weaken protection.
#[tokio::test]
async fn branch_protection_set_is_authorization_gated() {
    let root = temp_root("authz");
    let (gw, cell) = build_with(
        &root,
        Arc::new(DenyAction("git.repo.branch_protection.set")),
    );
    let addr = spawn(gw).await;
    let h = bearer(&mint(&cell, "acme", "jti-d"));
    http(
        addr,
        "POST",
        "/v1/git/repos",
        &hdr(&h),
        br#"{"slug":"svc"}"#.to_vec(),
    )
    .await;
    let (sc, sv) = http(
        addr,
        "POST",
        "/v1/git/repos/svc/branch-protection",
        &hdr(&h),
        br#"{"rulesets":[{"ref_pattern":"refs/heads/main","required_approvals":0}]}"#.to_vec(),
    )
    .await;
    assert_eq!(
        sc, 403,
        "a non-admin cannot set/weaken branch protection: {sv}"
    );
    // A non-denied git action still works (the deny is per-action, not blanket).
    let (lc, _lv) = http(addr, "GET", "/v1/git/repos", &hdr(&h), vec![]).await;
    assert_eq!(lc, 200, "other git actions are unaffected");
    std::fs::remove_dir_all(&root).ok();
}

/// (C) Tenant isolation + traversal-safety: an acme repo is invisible to globex; a `../`-laden slug is
/// refused by the validated resolver (never escaping the tenant root).
#[tokio::test]
async fn tenant_isolation_and_traversal_safety() {
    let root = temp_root("iso");
    let (gw, cell) = build(&root);
    let addr = spawn(gw).await;
    let acme = bearer(&mint(&cell, "acme", "jti-acme"));
    let globex = bearer(&mint(&cell, "globex", "jti-globex"));

    // acme creates a repo.
    let (c, _) = http(
        addr,
        "POST",
        "/v1/git/repos",
        &hdr(&acme),
        br#"{"slug":"secret"}"#.to_vec(),
    )
    .await;
    assert_eq!(c, 201);

    // acme sees it; globex sees NOTHING (tenant-partitioned by the verified token).
    let (_, av) = http(addr, "GET", "/v1/git/repos", &hdr(&acme), vec![]).await;
    assert_eq!(av["items"].as_array().unwrap().len(), 1);
    let (_, gv) = http(addr, "GET", "/v1/git/repos", &hdr(&globex), vec![]).await;
    assert_eq!(
        gv["items"].as_array().unwrap().len(),
        0,
        "globex cannot see acme's repo: {gv}"
    );

    // globex cannot read acme's repo blob (it does not exist under globex's tenant path).
    let (gb, _) = http(
        addr,
        "GET",
        "/v1/git/repos/secret/blob/main/README.md",
        &hdr(&globex),
        vec![],
    )
    .await;
    assert_eq!(gb, 404);

    // A traversal-laden repo slug is refused by the validated resolver → 400 (never escapes the root).
    let (tc, _tv) = http(
        addr,
        "POST",
        "/v1/git/repos",
        &hdr(&acme),
        br#"{"slug":"../../globex/eu-west/secret"}"#.to_vec(),
    )
    .await;
    assert_eq!(tc, 400, "a traversal slug is refused");

    std::fs::remove_dir_all(&root).ok();
}

/// (D) **GT-004 browse READ endpoints over the durable graph, tenant-scoped.** The repo home, the
/// commit log (libgit2 revwalk), and the commit diff (libgit2 tree diff) serve the REAL on-disk state
/// the web-edit commit produced — and a cross-tenant viewer gets a 0-leak 404 (the repo is not found
/// under its tenant path), never another tenant's bytes.
#[tokio::test]
async fn browse_endpoints_serve_the_durable_graph_tenant_scoped() {
    let root = temp_root("browse");
    let (gw, cell) = build(&root);
    let addr = spawn(gw).await;
    let h = bearer(&mint(&cell, "acme", "jti-br1"));

    let (st, _) = http(
        addr,
        "POST",
        "/v1/git/repos",
        &hdr(&h),
        br#"{"slug":"browse"}"#.to_vec(),
    )
    .await;
    assert_eq!(st, 201);
    let (wc, _) = http(
        addr,
        "POST",
        "/v1/git/repos/browse/blob/main/README.md",
        &hdr(&h),
        br##"{"base_oid":"","contents":"# hello browse\n","message":"init"}"##.to_vec(),
    )
    .await;
    assert_eq!(wc, 200);

    // GET /v1/git/repos/{repo} → the single RepoHome, populated, slug tenant-qualified.
    let (rc, rv) = http(addr, "GET", "/v1/git/repos/browse", &hdr(&h), vec![]).await;
    assert_eq!(rc, 200, "repo home: {rv}");
    assert_eq!(rv["state"], "populated");
    assert_eq!(rv["slug"], "acme/browse");
    assert!(rv["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["path"] == "README.md"));

    // GET /commits/main → the log (one commit, newest-first), PII-free pseudonymous author.
    let (cc, cv) = http(
        addr,
        "GET",
        "/v1/git/repos/browse/commits/main",
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(cc, 200, "commit log: {cv}");
    let items = cv["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let oid = items[0]["oid"].as_str().unwrap().to_string();
    assert_eq!(items[0]["short_oid"], oid[..12].to_string());
    assert!(items[0]["author"]
        .as_str()
        .unwrap()
        .ends_with("@acme.noreply"));

    // GET /commit/{oid} → the diff (README.md ADDED, with a + line).
    let (dc, dv) = http(
        addr,
        "GET",
        &format!("/v1/git/repos/browse/commit/{oid}"),
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(dc, 200, "commit diff: {dv}");
    assert_eq!(dv["oid"], oid);
    let files = dv["files"].as_array().unwrap();
    assert_eq!(files[0]["path"], "README.md");
    assert_eq!(files[0]["status"], "A");
    assert!(files[0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["origin"] == "+"));

    // A bogus commit oid → a clean 404 (never a panic).
    let (nc, _) = http(
        addr,
        "GET",
        "/v1/git/repos/browse/commit/deadbeef",
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(nc, 404);

    // Tenant isolation: globex sees a 0-leak 404 for acme's repo home + log.
    let g = bearer(&mint(&cell, "globex", "jti-br2"));
    let (gc, _) = http(addr, "GET", "/v1/git/repos/browse", &hdr(&g), vec![]).await;
    assert_eq!(gc, 404, "cross-tenant repo home is not found (0-leak)");
    let (glc, _) = http(
        addr,
        "GET",
        "/v1/git/repos/browse/commits/main",
        &hdr(&g),
        vec![],
    )
    .await;
    assert_eq!(glc, 404, "cross-tenant commit log is not found");

    std::fs::remove_dir_all(&root).ok();
}

/// **R3.4 repo-browsing completeness — the new read endpoints serve the durable graph.** refs (switcher
/// source), tree-at-root, the enriched nested blob (binary/size/raw URLs), tree/blob kind-mismatch
/// redirect hints, the commit-log prev_cursor/range round-trip, and the gateway-proxied raw/download
/// byte-serving with `Content-Disposition`.
#[tokio::test]
async fn r34_browse_endpoints_refs_tree_blob_paging_and_raw() {
    let root = temp_root("r34browse");
    let (gw, cell) = build(&root);
    let addr = spawn(gw).await;
    let h = bearer(&mint(&cell, "acme", "jti-r34"));

    let (st, _) = http(
        addr,
        "POST",
        "/v1/git/repos",
        &hdr(&h),
        br#"{"slug":"br"}"#.to_vec(),
    )
    .await;
    assert_eq!(st, 201);
    // Three commits on main via DISTINCT top-level files (each a real commit; avoids the GF-6
    // stale-base refusal that re-editing one file at base_oid="" would trip) so the log pages.
    for (i, path) in ["a.txt", "b.txt", "c.txt"].iter().enumerate() {
        let (wc, wv) = http(
            addr,
            "POST",
            &format!("/v1/git/repos/br/blob/main/{path}"),
            &hdr(&h),
            format!(r#"{{"base_oid":"","contents":"file {i}\n","message":"add {path}"}}"#)
                .into_bytes(),
        )
        .await;
        assert_eq!(wc, 200, "commit {path}: {wv}");
    }

    // refs → branches includes main (default), no tags.
    let (rc, rv) = http(addr, "GET", "/v1/git/repos/br/refs", &hdr(&h), vec![]).await;
    assert_eq!(rc, 200, "refs: {rv}");
    assert_eq!(rv["default_branch"], "main");
    assert!(rv["branches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "main" && b["is_default"] == true));
    assert_eq!(rv["tags"].as_array().unwrap().len(), 0);

    // tree at root → the three files, each with a full `path` + a resolved latest_commit (bounded walk).
    let (tc, tv) = http(addr, "GET", "/v1/git/repos/br/tree/main", &hdr(&h), vec![]).await;
    assert_eq!(tc, 200, "tree: {tv}");
    let entries = tv["entries"].as_array().unwrap();
    assert!(entries
        .iter()
        .any(|e| e["name"] == "a.txt" && e["path"] == "a.txt"));
    assert!(
        entries
            .iter()
            .any(|e| e["latest_commit"]["summary"].is_string()),
        "the bounded walk resolved at least one per-entry latest commit: {tv}"
    );

    // tree/{a file} → the kind-mismatch redirect hint (never a spurious 404).
    let (kc, kv) = http(
        addr,
        "GET",
        "/v1/git/repos/br/tree/main/a.txt",
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(kc, 200, "tree-of-file: {kv}");
    assert_eq!(kv["redirect_to_blob"], true);

    // blob → enriched: not-binary, real size, gateway-proxied raw/download URLs, not truncated.
    let (bc, bv) = http(
        addr,
        "GET",
        "/v1/git/repos/br/blob/main/a.txt",
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(bc, 200, "blob: {bv}");
    assert_eq!(bv["is_binary"], false);
    assert_eq!(bv["is_truncated"], false);
    assert!(bv["size_bytes"].as_u64().unwrap() > 0);
    assert_eq!(bv["raw_url"], "/v1/git/repos/br/raw/main/a.txt");
    assert_eq!(bv["download_url"], "/v1/git/repos/br/download/main/a.txt");
    assert!(bv["contents"].as_str().unwrap().contains("file 0"));

    // blob/{a dir} → the reverse kind-mismatch hint. (Seed a nested dir via a nested-path web edit is
    // out of scope; the root itself is a dir, so blob at empty path is the reverse case — proven at the
    // unit level. Here: an absent file is a clean 404.)
    let (nc, _) = http(
        addr,
        "GET",
        "/v1/git/repos/br/blob/main/nope.txt",
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(nc, 404, "absent blob is a clean 404");

    // commit-log paging: page 1 (limit=1) has a next_cursor and NO prev_cursor; range starts at 1.
    let (p1c, p1) = http(
        addr,
        "GET",
        "/v1/git/repos/br/commits/main?limit=1",
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(p1c, 200, "page1: {p1}");
    assert_eq!(p1["items"].as_array().unwrap().len(), 1);
    assert!(p1["page"]["next_cursor"].is_string());
    assert!(
        p1["page"]["prev_cursor"].is_null(),
        "first page has no Newer: {p1}"
    );
    assert_eq!(p1["page"]["range"]["from"], 1);
    assert_eq!(p1["page"]["range"]["to"], 1);
    // page 2 (cursor from page 1): prev_cursor is now present (the Newer link) and range advances.
    let cursor = p1["page"]["next_cursor"].as_str().unwrap().to_string();
    let (p2c, p2) = http(
        addr,
        "GET",
        &format!("/v1/git/repos/br/commits/main?limit=1&cursor={cursor}"),
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(p2c, 200, "page2: {p2}");
    assert_eq!(
        p2["page"]["prev_cursor"], "0",
        "page 2 Newer points back to offset 0: {p2}"
    );
    assert_eq!(p2["page"]["range"]["from"], 2);

    // raw = inline disposition; download = attachment disposition (Content-Disposition set server-side).
    let raw = open(
        addr,
        "GET",
        "/v1/git/repos/br/raw/main/a.txt",
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(raw.status().as_u16(), 200);
    let raw_cd = raw
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(raw_cd.starts_with("inline"), "raw is inline: {raw_cd}");
    let dl = open(
        addr,
        "GET",
        "/v1/git/repos/br/download/main/a.txt",
        &hdr(&h),
        vec![],
    )
    .await;
    assert_eq!(dl.status().as_u16(), 200);
    let dl_cd = dl
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        dl_cd.starts_with("attachment"),
        "download is an attachment: {dl_cd}"
    );
    assert!(
        dl_cd.contains("a.txt"),
        "the attachment carries the filename: {dl_cd}"
    );

    std::fs::remove_dir_all(&root).ok();
}
