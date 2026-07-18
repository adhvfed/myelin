//! # R2.1 — object-level authorization on the git JSON PRODUCT API (the action-only bypass, closed)
//!
//! The adversarial oracle for the R2.1 fix, mirroring the R0.3/R2.1a WIRE oracle tests at the JSON
//! front door: before R2.1 the `repo_authz` seam injected into [`DurableGitBackend`] was consulted
//! ONLY by the wire handlers — every object-addressed `/v1/git/...` JSON handler dispatched on the
//! gateway's ACTION gate alone, so an in-tenant principal holding the action grant could read/merge/
//! configure ANY repo in the tenant. These tests drive the REAL gateway lifecycle (real PASETO
//! Bearer auth → tenant-from-token → the ACTION gate deliberately wide open via `AllowAll` → the
//! R2.1 object guard) over the REAL engine stack: [`StoreBackedCheck`] with the frozen Git fragment
//! admitted, [`CheckBackedRepoAuthorizer`] as the object seam, [`TupleRepoBootstrap`] writing the
//! creator→admin grant — the exact production composition (`main.rs`) minus PG (the in-memory S3
//! double; the `--features integration` durable leg rides the same seams).
//!
//! The matrix (every ACTION granted — the object seam is what must decide):
//! - an in-tenant principal with NO repo grant is DENIED on every object-addressed route
//!   (read → the 0-leak 404; write/merge/branch-protection/endorse → 403);
//! - the creator (bootstrap `admin`) is admitted per permission;
//! - a `writer` grant admits push-class writes but NOT merge / branch-protection / endorse
//!   (the stronger-permission split — `protected_push = admin`, `approve_untrusted_ci` distinct);
//! - a `reader` grant admits reads only;
//! - an `approve_untrusted_ci` grant admits the endorsement only;
//! - cross-repo isolation + the leak-free `GET /v1/git/repos` list (a principal granted repo A
//!   never sees repo B's name).

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

/// The thresholds-file `[fail_static]` seed (mirrors the `repo_authz_live.rs` fixture).
fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN — LEGAL".into(),
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

/// Seed a service principal + link its Bearer credential subject key.
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

/// The R2.1 harness: the REAL gateway over the production object-authz composition —
/// `StoreBackedCheck` (Git fragment admitted) → `CheckBackedRepoAuthorizer` + `TupleRepoBootstrap`
/// injected into the durable git backend; the gateway's ACTION authorizer is `AllowAll` ON PURPOSE
/// (every principal holds every action — only the OBJECT seam separates them).
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
        // subject-key → principal-id: the tuple subjects below use these principal ids.
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

        // The REAL engine stack (the main.rs composition over the in-memory S3 double).
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

    /// Drive one request through the FULL gateway lifecycle (real Bearer authn → action gate →
    /// object guard → handler). Returns `(status, json-body)`.
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
        let resp: EdgeResponse = self.gw.handle(EdgeRequest::new(
            method,
            path,
            "",
            vec![
                ("authorization".into(), format!("Bearer {token}")),
                ("x-myelin-token-scheme".into(), scheme),
            ],
            body.to_vec(),
        ));
        let status = resp.status();
        let v = resp.json_body().unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    /// Write one raw relation tuple `repo:<slug>#<relation>@<principal_id>` through the ordinary
    /// 4.6 write path (the same store the checker reads).
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

    /// Creator-side fixture: create `slug` (bootstrap admin grant fires), commit a README on main,
    /// and open PR #1 — so every object-addressed route has a real object behind it.
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

/// **THE R2.1 ORACLE — an in-tenant principal WITH every action grant and NO repo grant is DENIED
/// on EVERY object-addressed JSON route.** Reads are the 0-leak 404 (`repository not found` — the
/// same body an absent repo serves); writes/merge/branch-protection/endorse are fail-closed 403s.
/// This is the exact live production bypass, closed: pre-R2.1 every one of these returned 200/201.
#[test]
fn ungranted_in_tenant_principal_is_denied_on_every_object_route() {
    let h = Harness::new("deny-all-routes");
    h.seed_repo_with_pr("subj-creator", "alpha");

    // READS → the 0-leak 404 (repo existence is not leaked; identical to the absent-repo body).
    // R3.4: the new browse read routes (refs / tree-at-path / nested blob / raw / download) go through
    // the SAME `guarded(Pull)` wrapper — they must inherit the identical 0-leak posture (no ref/path
    // existence leak on deny).
    for path in [
        "/v1/git/repos/alpha",
        "/v1/git/repos/alpha/commits/main",
        "/v1/git/repos/alpha/commit/0000000000000000000000000000000000000000",
        "/v1/git/repos/alpha/blob/main/README.md",
        "/v1/git/repos/alpha/prs/1",
        "/v1/git/repos/alpha/prs/1/checks",
        "/v1/git/repos/alpha/refs",
        "/v1/git/repos/alpha/tree/main",
        "/v1/git/repos/alpha/tree/main/crates/inner",
        "/v1/git/repos/alpha/blob/main/crates/inner/deep.rs",
        "/v1/git/repos/alpha/raw/main/README.md",
        "/v1/git/repos/alpha/download/main/README.md",
    ] {
        let (st, v) = h.call("subj-mallory", "GET", path, b"");
        assert_eq!(
            st, 404,
            "un-granted READ {path} must be the 0-leak 404: {v}"
        );
        assert_eq!(
            v["error"]["message"], "repository not found",
            "the deny body must be indistinguishable from an absent repo ({path}): {v}"
        );
    }

    // WRITE-class → fail-closed 403 (push permission).
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

    // The STRONGER permissions → 403 (protected_push / approve_untrusted_ci).
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

/// **The positive half — the creator (bootstrap `admin`) is ADMITTED per permission** (the grant is
/// load-bearing, not vacuous): every read 200, every push-class write lands, branch-protection
/// (admin-only) lands, and the merge attempt passes the OBJECT seam (its outcome is then the merge
/// GATE's business — policy 409, never an authz 403/404).
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

    // Branch protection: admin-only — the creator's bootstrap admin clears it.
    let (st, v) = h.call(
        "subj-creator",
        "POST",
        "/v1/git/repos/alpha/branch-protection",
        br#"{"rulesets":[{"ref_pattern":"refs/heads/main","required_contexts":["ci/build"]}]}"#,
    );
    assert_eq!(st, 200, "creator sets branch protection: {v}");

    // Merge: PAST the object seam (403/404 would be the seam refusing an admin — the bug); the
    // merge GATE then rules on policy (409/422 acceptable — an authz outcome is not).
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

/// **The stronger-permission split — a `writer` grant admits push-class writes but NOT merge, NOT
/// branch-protection, NOT endorse.** The exact "do NOT collapse everything to Read/Write" oracle:
/// `protected_push = admin` (frozen fragment) and `approve_untrusted_ci` is its own relation.
#[test]
fn push_grant_does_not_admit_merge_branch_protection_or_endorse() {
    let h = Harness::new("writer-split");
    h.seed_repo_with_pr("subj-creator", "alpha");
    h.write_relation("svc:dev", "writer", "alpha");

    // Push-class: admitted.
    let (st, v) = h.call(
        "subj-dev",
        "POST",
        "/v1/git/repos/alpha/blob/main/NOTES.md",
        br#"{"base_oid":"","contents":"dev notes\n"}"#,
    );
    assert_eq!(st, 200, "writer web-edit: {v}");
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
    // Reads: write implies read (pull = reader ∪ writer ∪ admin).
    let (st, v) = h.call("subj-dev", "GET", "/v1/git/repos/alpha", b"");
    assert_eq!(st, 200, "writer reads repo home: {v}");

    // The STRONGER rungs: refused with 403 (the object seam, not the merge gate — the body names
    // the missing grant class, and no policy evaluation ever ran).
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

/// **A `reader` grant admits reads and nothing else** (pull ⊅ push — the fragment's split, live at
/// the JSON door).
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

/// **The `approve_untrusted_ci` relation admits the endorsement ONLY** — and neither read nor write
/// nor merge come with it (a distinct trust decision, exactly as the fragment freezes it).
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

/// **Cross-repo isolation + the leak-free LIST.** Two creators, two repos: each lists ONLY its own
/// (the other's slug never appears anywhere in the response body — the `list_objects` prefilter is
/// by construction, not a post-filter); an un-granted third principal lists the EMPTY set; and a
/// grant on repo A admits nothing on repo B (read → 0-leak 404, write → 403).
#[test]
fn cross_repo_isolation_and_leak_free_list() {
    let h = Harness::new("isolation-list");
    h.seed_repo_with_pr("subj-creator", "alpha");
    h.seed_repo_with_pr("subj-other", "beta");

    // The list is prefiltered to the caller's pull-visible set.
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

    // Cross-repo: alpha's admin is nobody on beta.
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

/// **The CI check-report route requires the per-repo write grant** — an in-tenant principal with
/// the `git.checks.report` ACTION (every action is granted here) but no grant on the repo cannot
/// stamp greens on its PRs (the forged-greens hole at the object level, closed); a writer can.
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

// ═══════════════ R2-EXIT BLOCKER — the writer→protected-branch escalation, flipped to DENIED ═══════
//
// Two independent red-team adversaries proved a plain in-tenant WRITER (a `push` grant, NO
// `admin`/`protected_push`) could land arbitrary code on the PROTECTED branch `refs/heads/main` over
// the git wire — defeating required status checks, required reviews, CODEOWNERS, AND the admin
// requirement. Three compounding defects; the tests below pin Defect 1 at the durable backend
// (`report_checks` is a CI-PRODUCER capability — the SERVICE-kind floor), IN ADDITION to the R2.1
// object-guard leg proven above (`report_checks_requires_the_repo_write_grant`):
//
// - **Defect 1 (self-certifiable CI).** `report_checks` fail-closed refuses any non-SERVICE principal:
//   a HUMAN (or AGENT) writer attempting to stamp its own PR's checks green is REFUSED (403), while a
//   SERVICE principal (a CI run token — the legitimate producer) is admitted.
// - **Defect 2 (direct push needs `protected_push`).** R2.1's `RepoPermission::ProtectedPush` rung is
//   admin-only (proven in `repo_authz` / `repo_authz_live`); the wire receive-pack path resolves it via
//   `authorize_repo_permission` and holds a writer's direct push to a protected ref to the FULL ruleset.
// - **Defect 3 (full ruleset, not just contexts).** Proven in `myelin-git`'s
//   `evaluate_protected_ref_push` gate tests (approvals/CODEOWNERS make a direct push unsatisfiable).
//
// The end-to-end REAL-`git`-push leg (a writer's direct push to protected `main` refused over the
// wire) lives in `git_wire_http_push_oracle_test.rs` (gated on `MYELIN_REQUIRE_RUNSC=1`).

use myelin_identity::RuntimeRef;
use serde_json::json;

/// A bare principal of an arbitrary KIND (the producer floor gates on the kind, not on any grant).
fn principal_of_kind(id: &str, kind: PrincipalKind) -> Principal {
    Principal::stub(PrincipalId(id.into()), kind, TenantId(TENANT.into()))
}

/// Stand up a durable backend, create the repo, and open one PR (head = an arbitrary oid — the
/// producer floor gates on the reporting PRINCIPAL, not the commit).
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

/// **THE EXPLOIT (Defect 1), FLIPPED TO DENIED — the rewrite of the mis-codified expectation.** A
/// plain writer that is a HUMAN principal cannot attest CI check facts on its own PR. `report_checks`
/// is a CI-producer capability; a human writer stamping greens is REFUSED (fail-closed), so it can
/// never self-certify the required checks the merge gate / the protected-push gate read.
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

/// An AGENT writer is likewise refused (only a SERVICE CI run token produces check facts).
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

/// **LEGIT (Defect 1): a SERVICE principal (a CI run token — the authorized producer) CAN report
/// checks.** The floor denies human/agent writers, never the legitimate CI producer path.
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
