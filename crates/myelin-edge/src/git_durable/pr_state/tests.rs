use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
const REPO: &str = "product";
const AUTHOR_PSN: &str = "human:author@acme.noreply";

/// Everyone may read; only `writer` may push.
struct OnlyWriterPushes;

impl RepoAuthorizer for OnlyWriterPushes {
    fn authorize_repo_permission(
        &self,
        principal: &Principal,
        _repo: &RepoLoc,
        permission: RepoPermission,
    ) -> bool {
        permission == RepoPermission::Pull || principal.principal_id.0 == "writer"
    }
}

struct Fixture {
    backend: DurableGitBackend,
    repo: DurableGitRepo,
    author: Principal,
    number: u64,
    root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

fn principal(id: &str, kind: PrincipalKind) -> Principal {
    Principal::new(
        TenantId(TENANT.into()),
        Region(REGION.into()),
        PrincipalId(id.into()),
        kind,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn agent(id: &str) -> Principal {
    principal(
        id,
        PrincipalKind::Agent {
            runtime_ref: myelin_identity::RuntimeRef("runtime:local".into()),
            on_behalf_of: None,
        },
    )
}

fn loc() -> RepoLoc {
    RepoLoc::new(TENANT, REGION, REPO)
}

fn pull_request(author: Principal, draft: bool) -> Fixture {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("myelin-pr-state-{nonce}"));
    let backend = DurableGitBackend::rooted_inmem_for_test(&root)
        .with_repo_authorizer(Arc::new(OnlyWriterPushes));
    backend.create_repo(TENANT, REGION, REPO).unwrap();
    let repo = backend.store.open_repo(&loc()).unwrap();
    land(&repo, "refs/heads/main", None, "README.md", b"# start\n");
    let head = land(
        &repo,
        "refs/heads/feature",
        Some("refs/heads/main"),
        "feature.txt",
        b"one\n",
    );
    let opened = backend
        .open_pr_with_operation(
            TENANT,
            REGION,
            REPO,
            &serde_json::json!({
                "title": "Feature",
                "base_ref": "refs/heads/main",
                "head_ref": "refs/heads/feature",
                "head_oid": head.0,
                "draft": draft,
                "reviewers": [],
            }),
            &author,
            &PrOperationId::parse("open").unwrap(),
        )
        .unwrap();
    Fixture {
        backend,
        repo,
        author,
        number: opened.number,
        root,
    }
}

fn land(
    repo: &DurableGitRepo,
    gitref: &str,
    start: Option<&str>,
    path: &str,
    contents: &[u8],
) -> CoreOid {
    let (commit, _, prior) = repo
        .build_file_commit_from_ref(gitref, start, path, contents, "change", "psn", AUTHOR_PSN)
        .unwrap();
    repo.update_ref_cas(gitref, prior.as_ref(), Some(&commit), "seed", AUTHOR_PSN)
        .unwrap();
    commit
}

impl Fixture {
    fn change(&self, actor: &Principal, change: PrStateChange) -> Result<PrRecord, DurableError> {
        self.backend.change_pr_state_with_operation(
            RepoActorContext::new(TENANT, REGION, REPO, actor).for_pr(self.number),
            change,
            &self.backend.fresh_operation_id().unwrap(),
        )
    }

    fn approve(&self) {
        let reviewer = principal("reviewer", PrincipalKind::Human);
        self.backend
            .pr_mutate(
                &loc(),
                self.number,
                PrMutation::SubmitReview(ReviewRecord {
                    reviewer_pseudonym: DurableGitBackend::pseudonym(TENANT, &reviewer),
                    state: ReviewState::Submitted(ReviewVerdict::Approve),
                    is_agent: false,
                }),
                &self.backend.fresh_operation_id().unwrap(),
                &reviewer,
            )
            .unwrap();
    }

    fn merge(&self) -> Result<MergeAttempt, DurableError> {
        self.backend.merge_with_operation(
            RepoActorContext::new(TENANT, REGION, REPO, &self.author).for_pr(self.number),
            &self.backend.fresh_operation_id().unwrap(),
            MergeMethod::FastForward,
        )
    }
}

#[test]
fn its_author_or_a_writer_closes_it_and_an_agent_only_its_own() {
    let fixture = pull_request(principal("author", PrincipalKind::Human), false);
    let reader = principal("reader", PrincipalKind::Human);
    assert!(matches!(
        fixture.change(&reader, PrStateChange::Close),
        Err(DurableError::Forbidden(message))
            if message == "only its author or someone who can push to product can close this pull request"
    ));
    let pushing_agent = agent("writer");
    assert!(matches!(
        fixture.change(&pushing_agent, PrStateChange::Close),
        Err(DurableError::Forbidden(message))
            if message == "an agent changes only the pull requests it opened"
    ));

    let writer = principal("writer", PrincipalKind::Human);
    let closed = fixture.change(&writer, PrStateChange::Close).unwrap();
    assert_eq!(closed.state, PrState::Closed);
    assert!(matches!(
        fixture.change(&writer, PrStateChange::Close),
        Err(DurableError::Conflict(message)) if message == "only an open or draft pull request can be closed"
    ));
    let reopened = fixture
        .change(&fixture.author, PrStateChange::Reopen)
        .unwrap();
    assert_eq!(reopened.state, PrState::Open);

    let agents = pull_request(agent("builder"), true);
    let own = agents.author.clone();
    assert_eq!(
        agents.change(&own, PrStateChange::MarkReady).unwrap().state,
        PrState::Open
    );
    assert_eq!(
        agents.change(&own, PrStateChange::Close).unwrap().state,
        PrState::Closed
    );
}

#[test]
fn a_draft_or_closed_pull_request_merges_only_once_it_is_open() {
    let fixture = pull_request(principal("author", PrincipalKind::Human), true);
    fixture.approve();
    assert!(matches!(
        fixture.merge(),
        Err(DurableError::Conflict(message)) if message == DRAFT_MERGE_REFUSAL
    ));
    fixture
        .change(&fixture.author, PrStateChange::MarkReady)
        .unwrap();
    fixture
        .change(&fixture.author, PrStateChange::Close)
        .unwrap();
    assert!(matches!(
        fixture.merge(),
        Err(DurableError::Conflict(message)) if message == CLOSED_MERGE_REFUSAL
    ));
    fixture
        .change(&fixture.author, PrStateChange::Reopen)
        .unwrap();
    let merged = fixture.merge();
    assert!(
        matches!(merged, Ok(MergeAttempt::Merged { .. })),
        "{merged:?}"
    );
    assert!(matches!(
        fixture.change(&fixture.author, PrStateChange::Reopen),
        Err(DurableError::Conflict(message)) if message == "only a closed pull request can be reopened"
    ));
}

#[test]
fn reopening_follows_a_branch_that_moved_and_refuses_one_that_is_gone() {
    let fixture = pull_request(principal("author", PrincipalKind::Human), false);
    fixture.approve();
    fixture
        .change(&fixture.author, PrStateChange::Close)
        .unwrap();
    let moved = land(
        &fixture.repo,
        "refs/heads/feature",
        None,
        "feature.txt",
        b"two\n",
    );
    let reopened = fixture
        .change(&fixture.author, PrStateChange::Reopen)
        .unwrap();
    assert_eq!(reopened.state, PrState::Open);
    assert_eq!(reopened.head_oid, moved.0, "it reopens on the branch's tip");
    assert!(
        reopened.reviews.is_empty(),
        "an approval of the old head does not carry over to new commits"
    );

    fixture
        .change(&fixture.author, PrStateChange::Close)
        .unwrap();
    fixture
        .repo
        .update_ref_cas(
            "refs/heads/feature",
            Some(&moved),
            None,
            "delete",
            AUTHOR_PSN,
        )
        .unwrap();
    assert!(matches!(
        fixture.change(&fixture.author, PrStateChange::Reopen),
        Err(DurableError::Conflict(message))
            if message == "its branch feature no longer exists; push it again to reopen this pull request"
    ));
}
