use myelin_edge::{
    register_git_durable, AllowAll, CheckBackedRepoAuthorizer, DurableGitBackend, EdgeRequest,
    EdgeResponse, Gateway, Method, TupleRepoBootstrap, WhoamiHandler,
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
use myelin_storage::{KmsEngine, TenantScope};
use myelin_substrate::FailStaticThreshold;
use myelin_tenancy::{Region, TenantId};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const REGION: &str = "eu-west";
const SCHEME: &str = "agent";
const TENANT: &str = "acme";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn temp_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "myelin-r21-authz-{tag}-{}-{nanos}",
        std::process::id()
    ))
}

fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    }
}

fn principal(id: &str) -> Principal {
    Principal::new(
        TenantId(TENANT.into()),
        Region(REGION.into()),
        PrincipalId(id.into()),
        PrincipalKind::Service,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn admin_scope() -> TenantScope {
    let p = principal("admin");
    TenantScope::from_verified_token(&p, p.region.clone())
}

fn seed_principal(store: &PrincipalStore, pid: &str, subject_key: &str) {
    let scope = admin_scope();
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

struct Harness {
    gw: Gateway,
    cell: CellTokenAuthority,
    sbc: StoreBackedCheck,
    revocations: RevocationStore,
}

impl Harness {
    fn new(tag: &str) -> Harness {
        let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority");
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        seed_principal(&store, "svc:creator", "subj-creator");
        seed_principal(&store, "svc:mallory", "subj-mallory");
        seed_principal(&store, "svc:dev", "subj-dev");
        seed_principal(&store, "svc:reader", "subj-reader");
        seed_principal(&store, "svc:bot", "subj-bot");
        seed_principal(&store, "svc:other", "subj-other");

        let revocations = RevocationStore::new();
        let authn = Arc::new(CapabilityAuthenticator::with_verifier(
            store,
            Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
            revocations.clone(),
        ));
        let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
            Arc::new(KmsEngine::new()),
        )));

        let sbc = StoreBackedCheck::new(TupleStore::new(myelin_events::OutboxStore::new()));
        for admit in sbc.admit_git_fragment() {
            assert!(
                matches!(admit, FragmentAdmit::Admitted { .. }),
                "the Git fragment admits: {admit:?}"
            );
        }
        let authz = CheckBackedRepoAuthorizer::try_new(sbc.clone(), 300, &threshold())
            .expect("valid staleness bound");
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let backend = Arc::new(
            DurableGitBackend::rooted_inmem_for_test(temp_root(tag))
                .with_repo_authorizer(Arc::new(authz))
                .with_repo_bootstrap(Arc::new(bootstrap)),
        );

        let mut builder = Gateway::builder(authn, human_login, Arc::new(AllowAll)).route(
            Method::Get,
            "/v1/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler),
        );
        builder = register_git_durable(builder, backend);
        Harness {
            gw: builder.build(),
            cell,
            sbc,
            revocations,
        }
    }

    fn token(&self, subject_key: &str, ci_job: bool) -> (String, String) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let jti = format!("jti-{subject_key}-{nonce}");
        let purpose = if ci_job {
            let scope = admin_scope();
            self.revocations.register_run_token_ttl(
                &scope,
                &jti,
                myelin_events::Timestamp("2020-01-01T00:00:00Z".into()),
                myelin_events::Timestamp("2099-01-01T00:00:00Z".into()),
            );
            myelin_identity_service::CredentialPurpose::CiJob {
                run_id: format!("ci-{subject_key}"),
            }
        } else {
            myelin_identity_service::CredentialPurpose::OperatorBootstrap
        };
        let scheme = if ci_job { "ci" } else { SCHEME };
        let token = self.cell.mint(&CapabilityMintSpec {
            tenant: TENANT.into(),
            region: REGION.into(),
            subject_key: subject_key.into(),
            jti,
            exp_unix: now() + 3600,
            authority: vec![if ci_job {
                "ci.checks.report".into()
            } else {
                "edge.operator".into()
            }],
            dpop_jkt: None,
            purpose,
            audience: myelin_identity_service::CredentialAudience::Edge,
        });
        (scheme.into(), token)
    }

    fn call(
        &self,
        subject_key: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> (u16, serde_json::Value) {
        self.call_with_ci_purpose(
            subject_key,
            method,
            path,
            body,
            method == "POST" && path.ends_with("/checks"),
        )
    }

    fn call_with_ci_purpose(
        &self,
        subject_key: &str,
        method: &str,
        path: &str,
        body: &[u8],
        ci_job: bool,
    ) -> (u16, serde_json::Value) {
        let (scheme, token) = self.token(subject_key, ci_job);
        static NEXT_OPERATION: AtomicU64 = AtomicU64::new(1);
        let mut headers = vec![
            ("authorization".into(), format!("Bearer {token}")),
            ("x-myelin-token-scheme".into(), scheme),
        ];
        if method == "POST" {
            headers.push((
                "idempotency-key".into(),
                format!(
                    "git-authz-integration-{}",
                    NEXT_OPERATION.fetch_add(1, Ordering::Relaxed)
                ),
            ));
        }
        let resp: EdgeResponse =
            self.gw
                .handle(EdgeRequest::new(method, path, "", headers, body.to_vec()));
        let status = resp.status();
        let v = resp.json_body().unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    fn call_query(
        &self,
        subject_key: &str,
        method: &str,
        path: &str,
        query: &str,
    ) -> (u16, serde_json::Value) {
        let (scheme, token) = self.token(subject_key, false);
        let resp = self.gw.handle(EdgeRequest::new(
            method,
            path,
            query,
            vec![
                ("authorization".into(), format!("Bearer {token}")),
                ("x-myelin-token-scheme".into(), scheme),
            ],
            vec![],
        ));
        let status = resp.status();
        let body = resp.json_body().unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    fn write_relation(&self, principal_id: &str, relation: &str, slug: &str) {
        let p = principal(principal_id);
        let scope = TenantScope::from_verified_token(&p, p.region.clone());
        self.sbc
            .tuples()
            .write_tuples(
                &scope,
                &p,
                &[TupleDelta::Add(RelationTuple {
                    object: ObjectId(format!("repo:{slug}")),
                    relation: RelName(relation.into()),
                    subject: PrincipalId(principal_id.into()),
                    caveat: None,
                })],
                None,
                None,
                Timestamp("2026-07-15T00:00:00Z".into()),
            )
            .expect("write relation tuple");
    }

    fn seed_repo_with_pr(&self, creator_key: &str, slug: &str) {
        let (st, v) = self.call(
            creator_key,
            "POST",
            "/v1/git/repos",
            format!(r#"{{"slug":"{slug}"}}"#).as_bytes(),
        );
        assert_eq!(st, 201, "create {slug}: {v}");
        let (st, v) = self.call(
            creator_key,
            "POST",
            &format!("/v1/git/repos/{slug}/blob/main/README.md"),
            br##"{"base_oid":"","contents":"# seeded\n","message":"init"}"##,
        );
        assert_eq!(st, 200, "seed commit on {slug}: {v}");
        let (st, v) = self.call(
            creator_key,
            "POST",
            &format!("/v1/git/repos/{slug}/prs"),
            br#"{"title":"Seed PR","base_ref":"refs/heads/main","head_ref":"refs/heads/feature","head_oid":"a"}"#,
        );
        assert_eq!(st, 201, "open PR on {slug}: {v}");
    }
}

#[test]
fn ungranted_in_tenant_principal_is_denied_on_every_object_route() {
    let h = Harness::new("deny-all-routes");
    h.seed_repo_with_pr("subj-creator", "alpha");

    let assert_denied_read_is_hidden_as = |path: &str, hidden_as: &str| {
        let (st, v) = h.call("subj-mallory", "GET", path, b"");
        assert_eq!(
            st, 404,
            "un-granted READ {path} must be the 0-leak 404: {v}"
        );
        assert_eq!(
            v["error"]["message"], hidden_as,
            "the deny body must be indistinguishable from an absent resource ({path}): {v}"
        );
    };
    for path in [
        "/v1/git/repos/alpha",
        "/v1/git/repos/alpha/commits/main",
        "/v1/git/repos/alpha/commit/0000000000000000000000000000000000000000",
        "/v1/git/repos/alpha/blob/main/README.md",
        "/v1/git/repos/alpha/refs",
        "/v1/git/repos/alpha/tree/main",
        "/v1/git/repos/alpha/tree/main/crates/inner",
        "/v1/git/repos/alpha/blob/main/crates/inner/deep.rs",
        "/v1/git/repos/alpha/raw/main/README.md",
        "/v1/git/repos/alpha/download/main/README.md",
    ] {
        assert_denied_read_is_hidden_as(path, "repository not found");
    }
    for path in [
        "/v1/git/repos/alpha/prs/1",
        "/v1/git/repos/alpha/prs/1/commits",
        "/v1/git/repos/alpha/prs/1/checks",
    ] {
        assert_denied_read_is_hidden_as(path, "pull request not found");
    }

    let (status, body) = h.call_query(
        "subj-mallory",
        "GET",
        "/v1/git/repos/alpha/refs",
        "limit=01&cursor=not-a-cursor",
    );
    assert_eq!(status, 404, "Pull deny must precede refs parsing: {body}");
    assert_eq!(body["error"]["message"], "repository not found");
    let (status, body) = h.call_query(
        "subj-mallory",
        "GET",
        "/v1/git/repos/alpha/prs",
        "state=open&state=all&limit=01&unknown=value",
    );
    assert_eq!(
        status, 404,
        "Pull deny must precede strict PR-list parsing: {body}"
    );
    assert_eq!(body["error"]["message"], "repository not found");
    let (status, body) = h.call_query(
        "subj-mallory",
        "GET",
        "/v1/git/repos/alpha/prs/1/commits",
        "cursor=malformed&limit=01",
    );
    assert_eq!(
        status, 404,
        "Pull deny must precede PR commit cursor parsing: {body}"
    );
    assert_eq!(body["error"]["message"], "pull request not found");

    for (path, body) in [
        (
            "/v1/git/repos/alpha/blob/main/README.md",
            br##"{"base_oid":"","contents":"# stomp\n"}"##.as_slice(),
        ),
        ("/v1/git/repos/alpha/prs", br#"{"head_oid":"a"}"#.as_slice()),
        (
            "/v1/git/repos/alpha/prs/1/reviews",
            br#"{"verdict":"approve"}"#.as_slice(),
        ),
        (
            "/v1/git/repos/alpha/prs/1/checks",
            br#"{"green_contexts":["ci/forged"]}"#.as_slice(),
        ),
    ] {
        let (st, v) = h.call("subj-mallory", "POST", path, body);
        assert_eq!(st, 403, "un-granted WRITE {path} must be 403: {v}");
    }

    for (path, body) in [
        ("/v1/git/repos/alpha/prs/1/merge", b"".as_slice()),
        (
            "/v1/git/repos/alpha/branch-protection",
            br#"{"rulesets":[]}"#.as_slice(),
        ),
        ("/v1/git/repos/alpha/prs/1/endorse-fork-ci", b"".as_slice()),
    ] {
        let (st, v) = h.call("subj-mallory", "POST", path, body);
        assert_eq!(st, 403, "un-granted {path} must be 403: {v}");
    }
}

#[test]
fn creator_bootstrap_admin_is_admitted_per_permission() {
    let h = Harness::new("creator-allowed");
    h.seed_repo_with_pr("subj-creator", "alpha");

    for path in [
        "/v1/git/repos/alpha",
        "/v1/git/repos/alpha/commits/main",
        "/v1/git/repos/alpha/blob/main/README.md",
        "/v1/git/repos/alpha/prs/1",
        "/v1/git/repos/alpha/prs/1/checks",
    ] {
        let (st, v) = h.call("subj-creator", "GET", path, b"");
        assert_eq!(st, 200, "creator READ {path}: {v}");
    }

    let (st, v) = h.call(
        "subj-creator",
        "POST",
        "/v1/git/repos/alpha/prs/1/reviews",
        br#"{"verdict":"approve"}"#,
    );
    assert_eq!(st, 200, "creator review: {v}");

    let (st, v) = h.call(
        "subj-creator",
        "POST",
        "/v1/git/repos/alpha/branch-protection",
        br#"{"rulesets":[{"ref_pattern":"refs/heads/main","required_contexts":["ci/build"]}]}"#,
    );
    assert_eq!(st, 200, "creator sets branch protection: {v}");

    let (st, v) = h.call(
        "subj-creator",
        "POST",
        "/v1/git/repos/alpha/prs/1/merge",
        b"",
    );
    assert!(
        st != 403 && st != 404,
        "the admin creator must pass the merge OBJECT seam (got {st}: {v})"
    );
}

#[test]
fn a_writer_uses_a_feature_branch_without_crossing_repository_administration() {
    let h = Harness::new("writer-split");
    h.seed_repo_with_pr("subj-creator", "alpha");
    h.write_relation("svc:dev", "writer", "alpha");

    let (st, v) = h.call(
        "subj-dev",
        "POST",
        "/v1/git/repos/alpha/blob/main/NOTES.md",
        br#"{"base_oid":"","contents":"dev notes\n"}"#,
    );
    assert_eq!(
        st, 409,
        "the writer is authorized, but branch protection blocks a direct default-branch edit: {v}"
    );
    assert_eq!(v["error"]["code"], "conflict");
    let (st, v) = h.call(
        "subj-dev",
        "POST",
        "/v1/git/repos/alpha/blob/dev-notes/NOTES.md",
        br#"{"base_oid":"","contents":"dev notes\n","start_ref":"main"}"#,
    );
    assert_eq!(st, 200, "the writer can work on a feature branch: {v}");
    let (st, v) = h.call(
        "subj-dev",
        "POST",
        "/v1/git/repos/alpha/prs",
        br#"{"title":"Writer PR","head_oid":"b"}"#,
    );
    assert_eq!(st, 201, "writer opens PR: {v}");
    let (st, v) = h.call(
        "subj-dev",
        "POST",
        "/v1/git/repos/alpha/prs/1/reviews",
        br#"{"verdict":"comment"}"#,
    );
    assert_eq!(st, 200, "writer reviews: {v}");
    let (st, v) = h.call("subj-dev", "GET", "/v1/git/repos/alpha", b"");
    assert_eq!(st, 200, "writer reads repo home: {v}");

    let (st, v) = h.call("subj-dev", "POST", "/v1/git/repos/alpha/prs/1/merge", b"");
    assert_eq!(st, 403, "a push-only writer must NOT merge: {v}");
    let (st, v) = h.call(
        "subj-dev",
        "POST",
        "/v1/git/repos/alpha/branch-protection",
        br#"{"rulesets":[]}"#,
    );
    assert_eq!(
        st, 403,
        "a push-only writer must NOT set branch protection: {v}"
    );
    let (st, v) = h.call(
        "subj-dev",
        "POST",
        "/v1/git/repos/alpha/prs/1/endorse-fork-ci",
        b"",
    );
    assert_eq!(st, 403, "a push-only writer must NOT endorse fork CI: {v}");
}

#[test]
fn reader_grant_admits_reads_not_writes() {
    let h = Harness::new("reader");
    h.seed_repo_with_pr("subj-creator", "alpha");
    h.write_relation("svc:reader", "reader", "alpha");

    let (st, v) = h.call("subj-reader", "GET", "/v1/git/repos/alpha", b"");
    assert_eq!(st, 200, "reader repo home: {v}");
    let (st, v) = h.call(
        "subj-reader",
        "GET",
        "/v1/git/repos/alpha/blob/main/README.md",
        b"",
    );
    assert_eq!(st, 200, "reader blob view: {v}");
    let (st, v) = h.call("subj-reader", "GET", "/v1/git/repos/alpha/prs/1", b"");
    assert_eq!(st, 200, "reader PR view (pull covers PR reads): {v}");

    let (st, v) = h.call(
        "subj-reader",
        "POST",
        "/v1/git/repos/alpha/blob/main/README.md",
        br##"{"base_oid":"","contents":"# nope\n"}"##,
    );
    assert_eq!(st, 403, "a reader must NOT write: {v}");
    let (st, v) = h.call("subj-reader", "POST", "/v1/git/repos/alpha/prs", br#"{}"#);
    assert_eq!(st, 403, "a reader must NOT open a PR: {v}");
}

#[test]
fn endorser_grant_admits_endorse_only() {
    let h = Harness::new("endorser");
    h.seed_repo_with_pr("subj-creator", "alpha");
    h.write_relation("svc:bot", "approve_untrusted_ci", "alpha");

    let (st, v) = h.call(
        "subj-bot",
        "POST",
        "/v1/git/repos/alpha/prs/1/endorse-fork-ci",
        br#"{"contexts":["ci/fork-build"]}"#,
    );
    assert_eq!(st, 200, "the endorser endorses: {v}");

    let (st, v) = h.call("subj-bot", "GET", "/v1/git/repos/alpha", b"");
    assert_eq!(
        st, 404,
        "the endorsement relation confers NO read (0-leak 404): {v}"
    );
    let (st, v) = h.call(
        "subj-bot",
        "POST",
        "/v1/git/repos/alpha/blob/main/README.md",
        br#"{"base_oid":"","contents":"x"}"#,
    );
    assert_eq!(st, 403, "the endorsement relation confers no write: {v}");
    let (st, v) = h.call("subj-bot", "POST", "/v1/git/repos/alpha/prs/1/merge", b"");
    assert_eq!(st, 403, "the endorsement relation confers no merge: {v}");
}

#[test]
fn cross_repo_isolation_and_leak_free_list() {
    let h = Harness::new("isolation-list");
    h.seed_repo_with_pr("subj-creator", "alpha");
    h.seed_repo_with_pr("subj-other", "beta");

    let (st, v) = h.call("subj-creator", "GET", "/v1/git/repos", b"");
    assert_eq!(st, 200);
    let body = v.to_string();
    assert!(body.contains("alpha"), "creator sees alpha: {v}");
    assert!(
        !body.contains("beta"),
        "creator must NOT see beta anywhere in the list body (leak): {v}"
    );
    assert_eq!(
        v["items"].as_array().unwrap().len(),
        1,
        "exactly one visible repo: {v}"
    );

    let (st, v) = h.call("subj-other", "GET", "/v1/git/repos", b"");
    assert_eq!(st, 200);
    let body = v.to_string();
    assert!(body.contains("beta"), "other sees beta: {v}");
    assert!(
        !body.contains("alpha"),
        "other must NOT see alpha (leak): {v}"
    );

    let (st, v) = h.call("subj-mallory", "GET", "/v1/git/repos", b"");
    assert_eq!(st, 200);
    assert!(
        v["items"].as_array().unwrap().is_empty(),
        "an un-granted principal lists the EMPTY set: {v}"
    );
    let body = v.to_string();
    assert!(
        !body.contains("alpha") && !body.contains("beta"),
        "no repo name leaks to the un-granted lister: {v}"
    );

    let (st, v) = h.call("subj-creator", "GET", "/v1/git/repos/beta", b"");
    assert_eq!(st, 404, "cross-repo READ is the 0-leak 404: {v}");
    let (st, v) = h.call(
        "subj-creator",
        "POST",
        "/v1/git/repos/beta/prs",
        br#"{"head_oid":"c"}"#,
    );
    assert_eq!(st, 403, "cross-repo WRITE is 403: {v}");
    let (st, v) = h.call(
        "subj-creator",
        "POST",
        "/v1/git/repos/beta/prs/1/merge",
        b"",
    );
    assert_eq!(st, 403, "cross-repo MERGE is 403: {v}");
}

#[test]
fn report_checks_requires_the_repo_write_grant() {
    let h = Harness::new("report-checks");
    h.seed_repo_with_pr("subj-creator", "alpha");
    h.write_relation("svc:dev", "writer", "alpha");

    let (st, v) = h.call_with_ci_purpose(
        "subj-dev",
        "POST",
        "/v1/git/repos/alpha/prs/1/checks",
        br#"{"green_contexts":["ci/operator-forged"]}"#,
        false,
    );
    assert_eq!(
        st, 403,
        "an edge.operator credential cannot self-certify CI facts even with object write: {v}"
    );

    let (st, v) = h.call(
        "subj-mallory",
        "POST",
        "/v1/git/repos/alpha/prs/1/checks",
        br#"{"green_contexts":["ci/forged"]}"#,
    );
    assert_eq!(st, 403, "an un-granted producer cannot stamp greens: {v}");

    let (st, v) = h.call(
        "subj-dev",
        "POST",
        "/v1/git/repos/alpha/prs/1/checks",
        br#"{"green_contexts":["ci/build"]}"#,
    );
    assert_eq!(st, 200, "a granted producer stamps greens: {v}");
}

use myelin_identity::RuntimeRef;
use serde_json::json;

fn principal_of_kind(id: &str, kind: PrincipalKind) -> Principal {
    Principal::new(
        TenantId(TENANT.into()),
        Region(REGION.into()),
        PrincipalId(id.into()),
        kind,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn backend_with_open_pr(tag: &str) -> (DurableGitBackend, PathBuf) {
    let root = temp_root(tag);
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    be.create_repo(TENANT, REGION, "widgets")
        .expect("create repo");
    let author = principal_of_kind("author", PrincipalKind::Human);
    be.open_pr(
        TENANT,
        REGION,
        "widgets",
        &json!({
            "title": "PR #1",
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            "head_oid": "c0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ff",
        }),
        &author,
    )
    .expect("open PR #1");
    (be, root)
}

#[test]
fn human_writer_cannot_report_checks_on_its_own_pr() {
    let (be, root) = backend_with_open_pr("human-deny");
    let human_writer = principal_of_kind("author", PrincipalKind::Human);

    let err = be
        .report_checks(
            TENANT,
            REGION,
            "widgets",
            1,
            &human_writer,
            &json!({ "green_contexts": ["ci/build"] }),
        )
        .expect_err("a human writer must NOT be able to attest CI check facts");
    let msg = err.to_string();
    assert!(
        msg.contains("forbidden") && msg.contains("CI-producer"),
        "the refusal names the CI-producer capability floor: {msg}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn agent_writer_cannot_report_checks() {
    let (be, root) = backend_with_open_pr("agent-deny");
    let agent = principal_of_kind(
        "agent",
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt-1".into()),
            on_behalf_of: None,
        },
    );
    assert!(
        be.report_checks(
            TENANT,
            REGION,
            "widgets",
            1,
            &agent,
            &json!({ "green_contexts": ["ci/build"] })
        )
        .is_err(),
        "an agent is not a CI service producer"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn ci_service_producer_can_report_checks() {
    let (be, root) = backend_with_open_pr("service-ok");
    let ci_producer = principal_of_kind("ci:runner", PrincipalKind::Service);

    let rec = be
        .report_checks(
            TENANT,
            REGION,
            "widgets",
            1,
            &ci_producer,
            &json!({ "green_contexts": ["ci/build"] }),
        )
        .expect("a CI service producer reports checks");
    assert_eq!(
        rec.green_contexts,
        vec!["ci/build".to_string()],
        "the producer's attested greens are recorded (the facts the merge gate reads)"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn malformed_check_facts_are_atomic_and_leave_the_projection_unchanged() {
    let (be, root) = backend_with_open_pr("malformed-checks");
    let ci_producer = principal_of_kind("ci:runner", PrincipalKind::Service);

    let error = be
        .report_checks(
            TENANT,
            REGION,
            "widgets",
            1,
            &ci_producer,
            &json!({ "green_contexts": ["ci/build", 7] }),
        )
        .expect_err("a mixed check array must be rejected whole");
    assert!(matches!(
        error,
        myelin_git::durable::DurableError::InvalidInput(_)
    ));
    let record = be
        .get_pr(TENANT, REGION, "widgets", 1, &ci_producer)
        .unwrap()
        .unwrap();
    assert!(
        record.green_contexts.is_empty(),
        "the valid prefix of malformed CI facts must not reach the merge gate"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn malformed_fork_endorsements_are_atomic() {
    let (be, root) = backend_with_open_pr("malformed-endorsement");
    let ci_producer = principal_of_kind("ci:runner", PrincipalKind::Service);
    let approver = principal_of_kind("maintainer", PrincipalKind::Human);
    be.report_checks(
        TENANT,
        REGION,
        "widgets",
        1,
        &ci_producer,
        &json!({ "fork_unendorsed_contexts": ["ci/fork-build"] }),
    )
    .unwrap();

    let mixed = be
        .endorse_fork_ci(
            TENANT,
            REGION,
            "widgets",
            1,
            &json!({ "contexts": ["ci/fork-build", 7] }),
            &approver,
        )
        .expect_err("a mixed endorsement array must be rejected whole");
    assert!(matches!(
        mixed,
        myelin_git::durable::DurableError::InvalidInput(_)
    ));
    let record = be
        .get_pr(TENANT, REGION, "widgets", 1, &approver)
        .unwrap()
        .unwrap();
    assert!(
        record.endorsed_contexts.is_empty(),
        "the malformed request may not leave a partial endorsement"
    );
    std::fs::remove_dir_all(&root).ok();
}
