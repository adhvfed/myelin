use super::*;
use myelin_tenancy::TenantId;
use std::time::{SystemTime, UNIX_EPOCH};

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
const REPO: &str = "product";
const AUTHOR_PSN: &str = "human:author@acme.noreply";

struct Fixture {
    backend: DurableGitBackend,
    repo: DurableGitRepo,
    author: Principal,
    number: u64,
    reviewed_head: CoreOid,
    base_tip: CoreOid,
    root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

fn human(id: &str) -> Principal {
    Principal::new(
        TenantId(TENANT.into()),
        Region(REGION.into()),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

/// An approved pull request whose base moved on after its branch was made.
fn approved_pull_request_behind_its_base(branch_change: (&str, &[u8])) -> Fixture {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("myelin-update-branch-{nonce}"));
    let backend = DurableGitBackend::rooted_inmem_for_test(&root);
    backend.create_repo(TENANT, REGION, REPO).unwrap();
    let repo = backend
        .store
        .open_repo(&RepoLoc::new(TENANT, REGION, REPO))
        .unwrap();
    let land = |gitref: &str, start: Option<&str>, path: &str, contents: &[u8]| {
        let (commit, _, prior) = repo
            .build_file_commit_from_ref(gitref, start, path, contents, "change", "psn", AUTHOR_PSN)
            .unwrap();
        repo.update_ref_cas(gitref, prior.as_ref(), Some(&commit), "seed", AUTHOR_PSN)
            .unwrap();
        commit
    };
    land("refs/heads/main", None, "README.md", b"# start\n");
    let reviewed_head = land(
        "refs/heads/feature",
        Some("refs/heads/main"),
        branch_change.0,
        branch_change.1,
    );
    let base_tip = land("refs/heads/main", None, "README.md", b"# main moved\n");

    let author = human("author");
    let opened = backend
        .open_pr_with_operation(
            TENANT,
            REGION,
            REPO,
            &serde_json::json!({
                "title": "Feature",
                "base_ref": "refs/heads/main",
                "head_ref": "refs/heads/feature",
                "head_oid": reviewed_head.0,
                "reviewers": [],
            }),
            &author,
            &PrOperationId::parse("open").unwrap(),
        )
        .unwrap();
    let reviewer = human("reviewer");
    backend
        .pr_mutate(
            &RepoLoc::new(TENANT, REGION, REPO),
            opened.number,
            PrMutation::SubmitReview(ReviewRecord {
                reviewer_pseudonym: DurableGitBackend::pseudonym(TENANT, &reviewer),
                state: ReviewState::Submitted(ReviewVerdict::Approve),
                is_agent: false,
            }),
            &PrOperationId::parse("approve").unwrap(),
            &reviewer,
        )
        .unwrap();
    Fixture {
        backend,
        repo,
        author,
        number: opened.number,
        reviewed_head,
        base_tip,
        root,
    }
}

impl Fixture {
    fn update(&self, key: &str) -> Result<BranchUpdate, DurableError> {
        self.backend.update_pr_branch_with_operation(
            RepoActorContext::new(TENANT, REGION, REPO, &self.author).for_pr(self.number),
            None,
            &PrOperationId::parse(key).unwrap(),
        )
    }

    fn record(&self) -> PrRecord {
        self.backend
            .pr_get(
                &RepoLoc::new(TENANT, REGION, REPO),
                self.number,
                &self.author,
            )
            .unwrap()
            .unwrap()
    }

    fn approved(&self) -> bool {
        self.record()
            .reviews
            .iter()
            .any(|review| review.state == ReviewState::Submitted(ReviewVerdict::Approve))
    }
}

#[test]
fn an_update_merges_the_base_keeps_the_approval_and_replays() {
    let fixture = approved_pull_request_behind_its_base(("feature.txt", b"feature\n"));
    let BranchUpdate::Updated {
        head_oid: merged,
        replayed: false,
    } = fixture.update("update").unwrap()
    else {
        panic!("a clean base merges");
    };
    let parents = fixture
        .repo
        .commit_meta_at_oid(&CoreOid::new(merged.clone()))
        .unwrap()
        .unwrap()
        .parents;
    assert_eq!(
        parents,
        [fixture.reviewed_head.0.clone(), fixture.base_tip.0.clone()]
    );
    assert_eq!(fixture.record().head_oid, merged);
    assert!(fixture.approved(), "the reviewed change is unchanged");

    assert_eq!(
        fixture.update("update").unwrap(),
        BranchUpdate::Updated {
            head_oid: merged.clone(),
            replayed: true,
        }
    );
    assert_eq!(
        fixture.update("again").unwrap(),
        BranchUpdate::UpToDate { head_oid: merged }
    );
}

#[test]
fn a_conflict_writes_nothing() {
    let fixture = approved_pull_request_behind_its_base(("README.md", b"# feature\n"));
    assert_eq!(
        fixture.update("update").unwrap(),
        BranchUpdate::Conflicted {
            paths: vec!["README.md".into()]
        }
    );
    assert_eq!(
        fixture.repo.read_ref("refs/heads/feature").unwrap(),
        Some(fixture.reviewed_head.clone())
    );
    assert!(fixture.approved());
}

#[test]
fn a_pushed_commit_with_forged_trailers_never_keeps_approvals() {
    let fixture = approved_pull_request_behind_its_base(("feature.txt", b"feature\n"));
    let key = PrOperationId::parse("attacker-key").unwrap();
    let request = update_request_hash(
        RepoActorContext::new(TENANT, REGION, REPO, &fixture.author),
        fixture.number,
        None,
    );
    let forged = format!(
        "Merge branch 'main' into feature\n\nMyelin-Operation: {}\nMyelin-Request: {request}",
        key.digest()
    );
    // A push lands the commit, and the update arrives before the push's own follow-up ran.
    let (unreviewed, _, prior) = fixture
        .repo
        .build_file_commit(
            "refs/heads/feature",
            "unreviewed.txt",
            b"x\n",
            &forged,
            "psn",
            AUTHOR_PSN,
        )
        .unwrap();
    fixture
        .repo
        .update_ref_cas(
            "refs/heads/feature",
            prior.as_ref(),
            Some(&unreviewed),
            "push",
            AUTHOR_PSN,
        )
        .unwrap();

    assert_eq!(
        fixture.update("attacker-key").unwrap(),
        BranchUpdate::Updated {
            head_oid: unreviewed.0.clone(),
            replayed: true,
        }
    );
    assert_eq!(fixture.record().head_oid, unreviewed.0);
    assert!(
        !fixture.approved(),
        "a commit Myelin did not make is followed like any push"
    );
}
