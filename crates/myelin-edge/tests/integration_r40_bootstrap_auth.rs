//! # R4.0 item B — the operator BOOTSTRAP mints a token that AUTHENTICATES against a composed edge.
//!
//! The founder-dogfood exit for the JSON surface, proven against LIVE Postgres (`--features integration`):
//! the DURABLE cell token authority (`load_or_generate` from `cell_token_root`) + the durable S1 store
//! (`with_pg`) + the `edge bootstrap` library body ([`bootstrap_principal_and_mint`]) compose so a
//! minted capability token, presented as a Bearer credential to a SEPARATELY-composed [`Gateway`] over
//! the SAME durable stores, authenticates and drives the real product handlers:
//!   1. `GET /v1/whoami` → 200, resolving to the bootstrapped principal;
//!   2. `POST /v1/git/repos` (create) → 201;
//!   3. `POST .../blob/...` (web-edit commit) → 200 → a head oid;
//!   4. `POST .../prs` (open) → 201, `POST .../prs/1/reviews` (review) → 200, `POST .../prs/1/merge` → 200.
//!
//! Plus: **bootstrap is idempotent** — a second run for the SAME principal mints a NEW token (distinct
//! jti) without corrupting the principal, and BOTH tokens authenticate.
//!
//! Run (dev docker stack up):
//!   DATABASE_URL=postgres://… DATABASE_MIGRATION_URL=postgres://… \
//!     cargo test -p myelin-edge --features integration \
//!     --test integration_r40_bootstrap_auth -- --nocapture
//!
//! Skips gracefully if `DATABASE_URL` is unset (hermetic without a DB). The live path migrates with
//! the admin credential, closes it, and drives every store through the constrained runtime pool.
#![cfg(feature = "integration")]

use myelin_config::{Mode, MyelinConfig};
use myelin_edge::{
    bootstrap_principal_and_mint, register_git_durable, serve_edge, AllowAll, BootstrapParams,
    DurableGitBackend, Gateway, Method, StoreBackedIssueAuthorizer, WhoamiHandler,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    CapabilityAuthenticator, CellTokenAuthority, HumanSsoAuthenticator, PasetoCapabilityVerifier,
    PrincipalStore, RevocationStore, StoreBackedCheck, TupleStore,
};
use myelin_issues::IssueAuthorizer;
use myelin_storage::{
    all_durable_migrations, DurableCellRootBacking, DurableKmsBacking, DurablePrincipalBacking,
    DurableRevocationBacking, DurableTupleBacking, HotTables, PgBootstrap, SealKey, TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use tokio::net::TcpListener;

const REGION: &str = "fr-par";
const ISSUES_PROJECT: &str = "11111111-1111-1111-1111-111111111111";
const SEAL_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

fn uniq() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A minimal HTTP/1.1 request over a raw TcpStream (no client dep): returns `(status, body_json)`.
fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut s = TcpStream::connect(addr).expect("connect edge");
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\n\
         X-Myelin-Token-Scheme: agent\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).expect("write request");
    let mut out = String::new();
    s.read_to_string(&mut out).expect("read response");
    let status = out
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    let body_json = out
        .split_once("\r\n\r\n")
        .and_then(|(_, b)| serde_json::from_str(b.trim()).ok())
        .unwrap_or(serde_json::Value::Null);
    (status, body_json)
}

async fn spawn(gateway: Arc<Gateway>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = serve_edge(listener, gateway).await;
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bootstrap_token_authenticates_and_drives_the_product_surface() {
    let Ok(_) = std::env::var("DATABASE_URL") else {
        eprintln!(
            "SKIP bootstrap_token_authenticates_and_drives_the_product_surface: DATABASE_URL unset"
        );
        return;
    };
    let config = MyelinConfig::from_env(Mode::DevDefaults).expect("dev config");
    let bootstrap = match PgBootstrap::connect(config, 8).await {
        Ok(bootstrap) => bootstrap,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    let handle = tokio::runtime::Handle::current();
    bootstrap
        .migrate_foundation()
        .await
        .expect("migrate foundation");
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("migrate the full durable aggregate (incl 0060_cell_token_root)");
    let provider = bootstrap
        .into_runtime()
        .await
        .expect("close the privileged pool and hand off to the constrained runtime role");

    let seal = SealKey::from_encoded(SEAL_HEX).expect("seal key");
    let cell_id = format!("cell-r40b-{}", uniq());

    // The DURABLE KMS + the DURABLE cell token authority (the R4.0 make-or-break) — both sealed under
    // the same key, both recovered from PG.
    let kms = Arc::new(
        DurableKmsBacking::new(provider.db_pool().clone(), &cell_id)
            .load_or_generate(&seal)
            .await
            .expect("KMS load_or_generate"),
    );
    let cell_material = DurableCellRootBacking::new(provider.db_pool().clone(), &cell_id)
        .load_or_generate(&seal)
        .await
        .expect("cell-root load_or_generate");
    let cell = Arc::new(CellTokenAuthority::from_material(&cell_material).expect("cell authority"));

    // The durable S1 store the bootstrap seeds into (and the serving edge resolves through).
    let store = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(provider.clone()),
        handle.clone(),
    );
    let tuples = TupleStore::with_pg(DurableTupleBacking::new(provider.clone()), handle.clone());

    let tenant = format!("t-{}", uniq());
    let params = BootstrapParams {
        tenant: &tenant,
        region: REGION,
        principal: "founder",
        issues_project: ISSUES_PROJECT,
        display: Some("The Founder"),
        ttl_days: 30,
    };
    // (B1) bootstrap — seed + mint. (B2) idempotent re-run — a NEW token, distinct jti, same principal.
    let out1 = bootstrap_principal_and_mint(&store, &tuples, &cell, &params, now_unix())
        .expect("bootstrap #1");
    let out2 = bootstrap_principal_and_mint(&store, &tuples, &cell, &params, now_unix())
        .expect("bootstrap #2");
    assert_ne!(
        out1.jti, out2.jti,
        "a re-run mints a NEW token (distinct revocation handle) for the SAME principal"
    );
    assert_eq!(out1.principal_id, "founder");

    // A FRESH tuple-store handle proves the grant is durable. Re-bootstrap converges on exactly one
    // project reader edge and grants no other project/relation.
    let founder = Principal::new(
        TenantId::from_token(tenant.clone()),
        Region::new(REGION),
        PrincipalId("founder".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let founder_scope = TenantScope::from_verified_token(&founder, Region::new(REGION));
    let restarted_tuples =
        TupleStore::with_pg(DurableTupleBacking::new(provider.clone()), handle.clone());
    let durable_edges = restarted_tuples.tuples_in(&founder_scope);
    assert_eq!(
        durable_edges.len(),
        1,
        "re-bootstrap never duplicates or widens"
    );
    assert_eq!(
        durable_edges[0].tuple.object.0,
        format!("project:{ISSUES_PROJECT}")
    );
    assert_eq!(durable_edges[0].tuple.relation.0, "reader");
    assert_eq!(durable_edges[0].tuple.subject.0, "founder");

    // The product still goes through the ordinary may_create check: the exact project is admitted,
    // while an ungranted project remains denied.
    let issue_check = StoreBackedCheck::new(restarted_tuples);
    for verdict in issue_check.admit_issue_fragment() {
        assert!(matches!(
            verdict,
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }
    let issue_authorizer = StoreBackedIssueAuthorizer::new(issue_check);
    assert!(issue_authorizer.may_create(&founder, ISSUES_PROJECT));
    assert!(!issue_authorizer.may_create(&founder, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"));

    // Compose a serving Gateway over the SAME durable stores + the SAME cell trust anchor, exactly as
    // production `serve()` authenticates (a SEPARATE PrincipalStore handle over the same PG backing,
    // the durable revocation store, the real PASETO verifier). The git backend is the in-memory test
    // double (AllowAllRepos) so the bootstrap TOKEN drives the real product handlers without the
    // per-object ReBAC composition (proven separately) — the point here is the AUTH loop.
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        PrincipalStore::with_pg(
            kms.clone(),
            DurablePrincipalBacking::new(provider.clone()),
            handle.clone(),
        ),
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::with_pg(
            DurableRevocationBacking::new(provider.clone()),
            handle.clone(),
        ),
    ));
    let human = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(provider.clone()),
        handle.clone(),
    )));
    let git_root = std::env::temp_dir().join(format!("myelin-r40b-git-{}", uniq()));
    let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(git_root));
    let builder = Gateway::builder(authn, human, Arc::new(AllowAll))
        .default_token_scheme("agent")
        .route(
            Method::Get,
            "/v1/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler),
        );
    let builder = register_git_durable(builder, backend);
    let gateway = Arc::new(builder.build());
    let addr = spawn(gateway).await;

    let token = &out1.token;

    // (1) whoami — the token authenticates + resolves the bootstrapped principal.
    let (st, who) = http(addr, "GET", "/v1/whoami", token, None);
    assert_eq!(st, 200, "the bootstrap token authenticates whoami: {who}");
    assert_eq!(who["principal_id"], "founder");
    assert_eq!(who["tenant"], tenant.as_str());
    assert_eq!(who["region"], REGION);

    // The SECOND token also authenticates (idempotent bootstrap did not corrupt the principal).
    let (st2, _who2) = http(addr, "GET", "/v1/whoami", &out2.token, None);
    assert_eq!(st2, 200, "the re-minted token also authenticates");

    // (2) create a repo.
    let (st, cr) = http(
        addr,
        "POST",
        "/v1/git/repos",
        token,
        Some(r#"{"slug":"widgets"}"#),
    );
    assert_eq!(st, 201, "the token creates a repo: {cr}");

    // (3) a web-edit commit on a feature ref → a head oid.
    let (st, wv) = http(
        addr,
        "POST",
        "/v1/git/repos/widgets/blob/feature/app.txt",
        token,
        Some(r#"{"base_oid":"","contents":"v1\n","message":"feat: app"}"#),
    );
    assert_eq!(st, 200, "web-edit commit: {wv}");
    let head_oid = wv["applied"]["new_oid"]
        .as_str()
        .expect("web-edit commit returns a new_oid")
        .to_string();

    // (4) open a PR against an UNPROTECTED base ref (required_approvals defaults to 0 there), review,
    //     and merge — all with the ONE bootstrap token (the creator is authorized for every verb).
    let open_body = format!(
        r#"{{"title":"first PR","base_ref":"refs/heads/dev","head_ref":"refs/heads/feature","head_oid":"{head_oid}"}}"#
    );
    let (st, pr) = http(
        addr,
        "POST",
        "/v1/git/repos/widgets/prs",
        token,
        Some(&open_body),
    );
    assert_eq!(st, 201, "open PR: {pr}");

    let (st, rv) = http(
        addr,
        "POST",
        "/v1/git/repos/widgets/prs/1/reviews",
        token,
        Some(r#"{"verdict":"approve"}"#),
    );
    assert_eq!(st, 200, "submit review: {rv}");

    let (st, mg) = http(
        addr,
        "POST",
        "/v1/git/repos/widgets/prs/1/merge",
        token,
        Some("{}"),
    );
    assert_eq!(
        st, 200,
        "merge PR (unprotected base, 0 approvals required): {mg}"
    );
    assert_eq!(mg["applied"]["merged"], true, "the PR merged: {mg}");

    // cleanup the durable cell-infra + principal rows for this test's cell/tenant.
    let pool = provider.db_pool();
    let _ = sqlx::query("DELETE FROM cell_token_root WHERE cell_id = $1")
        .bind(&cell_id)
        .execute(pool)
        .await;
    for t in ["kms_wrapped_dek", "kms_wrapped_kek", "kms_sealed_root"] {
        let _ = sqlx::query(&format!("DELETE FROM {t} WHERE cell_id = $1"))
            .bind(&cell_id)
            .execute(pool)
            .await;
    }
    let _ = sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(pool)
        .await;

    println!(
        "OK: bootstrap token authenticates (whoami/create/open/review/merge) over the durable cell \
         authority; idempotent re-mint works (jti1={} jti2={}).",
        out1.jti, out2.jti
    );
}
