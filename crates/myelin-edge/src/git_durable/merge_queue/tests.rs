use super::*;
use myelin_identity::RuntimeRef;
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

fn loc() -> RepoLoc {
    RepoLoc::new(TENANT, REGION, REPO)
}

/// A pull request whose base moved on after its branch was made.
fn pull_request_behind_its_base(branch_change: (&str, &[u8])) -> Fixture {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("myelin-merge-queue-{nonce}"));
    let backend = DurableGitBackend::rooted_inmem_for_test(&root);
    backend.create_repo(TENANT, REGION, REPO).unwrap();
    let repo = backend.store.open_repo(&loc()).unwrap();
    let land = |gitref: &str, start: Option<&str>, path: &str, contents: &[u8]| {
        let (commit, _, prior) = repo
            .build_file_commit_from_ref(gitref, start, path, contents, "change", "psn", AUTHOR_PSN)
            .unwrap();
        repo.update_ref_cas(gitref, prior.as_ref(), Some(&commit), "seed", AUTHOR_PSN)
            .unwrap();
        commit
    };
    land("refs/heads/main", None, "README.md", b"# start\n");
    let head = land(
        "refs/heads/feature",
        Some("refs/heads/main"),
        branch_change.0,
        branch_change.1,
    );
    land("refs/heads/main", None, "README.md", b"# main moved\n");
    let author = principal("author", PrincipalKind::Human);
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

impl Fixture {
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
                &PrOperationId::parse("approve").unwrap(),
                &reviewer,
            )
            .unwrap();
    }

    fn request(&self, actor: &Principal) -> Result<PrRecord, DurableError> {
        self.backend.request_auto_merge_with_operation(
            RepoActorContext::new(TENANT, REGION, REPO, actor).for_pr(self.number),
            &self.backend.fresh_operation_id().unwrap(),
            MergeMethod::FastForward,
        )
    }

    fn advance(&self) -> QueueStep {
        self.backend
            .advance_queued(&loc(), self.number, &merge_queue_principal(TENANT, REGION))
            .unwrap()
    }

    fn record(&self) -> PrRecord {
        self.backend
            .pr_get(&loc(), self.number, &self.author)
            .unwrap()
            .unwrap()
    }
}

#[test]
fn the_queue_updates_a_branch_left_behind_then_merges_it() {
    let fixture = pull_request_behind_its_base(("feature.txt", b"feature\n"));
    fixture.approve();
    fixture.request(&fixture.author).unwrap();

    assert_eq!(
        fixture.advance(),
        QueueStep::Holding,
        "first the base merges in"
    );
    let updated = fixture.record().head_oid;
    assert_eq!(
        fixture
            .repo
            .read_ref("refs/heads/feature")
            .unwrap()
            .unwrap()
            .0,
        updated
    );
    assert_eq!(fixture.advance(), QueueStep::Merged);
    assert_eq!(fixture.record().state, PrState::Merged);
    assert_eq!(
        fixture.repo.read_ref("refs/heads/main").unwrap().unwrap().0,
        updated,
        "main is exactly the head the queue tested"
    );
}

#[test]
fn a_pull_request_waiting_for_review_holds_nothing() {
    let fixture = pull_request_behind_its_base(("feature.txt", b"feature\n"));
    let before = fixture.record().head_oid;
    fixture.request(&fixture.author).unwrap();
    assert_eq!(fixture.advance(), QueueStep::WaitingForReview);
    assert_eq!(
        fixture.record().head_oid,
        before,
        "an unapproved branch is left alone"
    );
}

#[test]
fn a_conflict_takes_the_pull_request_out_of_the_queue_and_says_why() {
    let fixture = pull_request_behind_its_base(("README.md", b"# feature\n"));
    fixture.approve();
    fixture.request(&fixture.author).unwrap();
    let QueueStep::Stopped(reason) = fixture.advance() else {
        panic!("a conflicting base stops the automatic merge");
    };
    assert_eq!(reason, "main and this branch both changed README.md");
    let record = fixture.record();
    assert_eq!(record.auto_merge, None);
    assert_eq!(
        record.auto_merge_stopped.map(|stopped| stopped.reason),
        Some(reason)
    );
    assert_eq!(record.state, PrState::Open);
}

#[test]
fn only_a_person_can_schedule_a_merge() {
    let fixture = pull_request_behind_its_base(("feature.txt", b"feature\n"));
    let agent = principal(
        "agent:helper",
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("runtime:local".into()),
            on_behalf_of: Some(PrincipalId("author".into())),
        },
    );
    assert!(matches!(
        fixture.request(&agent),
        Err(DurableError::Forbidden(_))
    ));
    assert_eq!(fixture.record().auto_merge, None);
}

#[test]
fn commits_pushed_after_scheduling_are_not_merged_on_the_schedule() {
    let fixture = pull_request_behind_its_base(("feature.txt", b"feature\n"));
    fixture.approve();
    fixture.request(&fixture.author).unwrap();
    let (pushed, _, prior) = fixture
        .repo
        .build_file_commit(
            "refs/heads/feature",
            "late.txt",
            b"late\n",
            "late",
            "psn",
            AUTHOR_PSN,
        )
        .unwrap();
    fixture
        .repo
        .update_ref_cas(
            "refs/heads/feature",
            prior.as_ref(),
            Some(&pushed),
            "push",
            AUTHOR_PSN,
        )
        .unwrap();
    fixture
        .backend
        .follow_branch(&loc(), &fixture.author, "refs/heads/feature");

    let QueueStep::Stopped(reason) = fixture.advance() else {
        panic!("a new head needs a new decision");
    };
    assert!(reason.starts_with("new commits arrived after the merge was scheduled"));
    assert_eq!(fixture.record().state, PrState::Open);
}

#[test]
fn a_stop_never_overrides_a_newer_request() {
    let fixture = pull_request_behind_its_base(("feature.txt", b"feature\n"));
    let first = fixture.request(&fixture.author).unwrap();
    let stale = first.auto_merge.clone().unwrap();
    let record = fixture.record();
    let mut newer = stale.clone();
    newer.requested_at += 1;
    let step = fixture
        .backend
        .stop_auto_merge(
            &loc(),
            &record,
            &merge_queue_principal(TENANT, REGION),
            "late".into(),
            &newer,
        )
        .unwrap();
    assert_eq!(step, QueueStep::WaitingForReview);
    assert_eq!(fixture.record().auto_merge, Some(stale));
}

#[test]
fn a_deleted_base_stops_rather_than_recreating_it() {
    let fixture = pull_request_behind_its_base(("feature.txt", b"feature\n"));
    fixture.approve();
    fixture.request(&fixture.author).unwrap();
    let main = fixture.repo.read_ref("refs/heads/main").unwrap();
    fixture
        .repo
        .update_ref_cas("refs/heads/main", main.as_ref(), None, "delete", AUTHOR_PSN)
        .unwrap();
    assert_eq!(
        fixture.advance(),
        QueueStep::Stopped("main no longer exists".into())
    );
    assert_eq!(fixture.repo.read_ref("refs/heads/main").unwrap(), None);
}

#[test]
fn a_squash_merge_lands_one_commit_with_the_reviewed_tree() {
    let fixture = pull_request_behind_its_base(("feature.txt", b"feature\n"));
    fixture.approve();
    fixture.request(&fixture.author).unwrap();
    assert_eq!(
        fixture.advance(),
        QueueStep::Holding,
        "first the base merges in"
    );
    let reviewed = fixture.record().head_oid;
    let base = fixture.repo.read_ref("refs/heads/main").unwrap().unwrap();
    let MergeAttempt::Merged { new_oid, .. } = fixture
        .backend
        .merge_with_operation(
            RepoActorContext::new(TENANT, REGION, REPO, &fixture.author).for_pr(fixture.number),
            &fixture.backend.fresh_operation_id().unwrap(),
            MergeMethod::Squash,
        )
        .unwrap()
    else {
        panic!("an approved, up-to-date pull request squashes");
    };
    assert_ne!(new_oid, reviewed, "a squash is a new commit");
    let squash = fixture
        .repo
        .commit_meta_at_oid(&CoreOid::new(new_oid.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(squash.parents, [base.0], "one commit on top of the base");
    assert_eq!(
        squash.summary,
        format!("change (#{})", fixture.number),
        "the commit's own words; the queue's base merge is left out"
    );
    for path in ["README.md", "feature.txt"] {
        assert_eq!(
            fixture.repo.blob_oid_at_path(&new_oid, path).unwrap(),
            fixture.repo.blob_oid_at_path(&reviewed, path).unwrap(),
            "exactly the tested content at {path}"
        );
    }
    assert_eq!(
        fixture.repo.read_ref("refs/heads/main").unwrap().unwrap().0,
        new_oid
    );
    let record = fixture.record();
    assert_eq!(record.state, PrState::Merged);
    assert_eq!(record.merged_oid, Some(new_oid));
}

#[test]
fn a_queued_pull_request_knows_how_many_were_scheduled_before_it() {
    let fixture = pull_request_behind_its_base(("feature.txt", b"feature\n"));
    let record = fixture.request(&fixture.author).unwrap();
    assert_eq!(
        fixture
            .backend
            .auto_merge_ahead(&loc(), &record, &fixture.author)
            .unwrap(),
        Some(0),
        "first in line"
    );
    let unqueued = fixture.record();
    fixture
        .backend
        .cancel_auto_merge_with_operation(
            RepoActorContext::new(TENANT, REGION, REPO, &fixture.author).for_pr(fixture.number),
            &fixture.backend.fresh_operation_id().unwrap(),
        )
        .unwrap();
    assert_eq!(
        fixture
            .backend
            .auto_merge_ahead(&loc(), &fixture.record(), &fixture.author)
            .unwrap(),
        None,
        "not queued, so no place in line"
    );
    assert!(unqueued.auto_merge.is_some());
}

#[test]
fn a_sweep_brings_a_pull_request_left_behind_by_a_crash_onto_its_branch() {
    let fixture = pull_request_behind_its_base(("feature.txt", b"feature\n"));
    let recorded = fixture.record();
    // A branch update whose follow-up never ran, as after a crash between the two.
    let (moved, _, prior) = fixture
        .repo
        .build_file_commit(
            "refs/heads/feature",
            "more.txt",
            b"more\n",
            "more",
            "psn",
            AUTHOR_PSN,
        )
        .unwrap();
    fixture
        .repo
        .update_ref_cas(
            "refs/heads/feature",
            prior.as_ref(),
            Some(&moved),
            "push",
            AUTHOR_PSN,
        )
        .unwrap();
    assert_eq!(fixture.record().head_oid, recorded.head_oid);

    let heads = [myelin_git::pg_pr_store::FollowedHead {
        repo: REPO.into(),
        number: fixture.number,
        head_ref: recorded.head_ref.clone(),
        head_oid: recorded.head_oid.clone(),
    }];
    let follower = service_principal(TENANT, REGION, BRANCH_FOLLOWER_PRINCIPAL);
    let stopping = AtomicBool::new(false);
    assert_eq!(
        fixture
            .backend
            .settle_stale_heads(TENANT, REGION, &heads, &follower, &stopping),
        1
    );
    assert_eq!(fixture.record().head_oid, moved.0);
    let settled = [myelin_git::pg_pr_store::FollowedHead {
        head_oid: moved.0.clone(),
        ..heads[0].clone()
    }];
    assert_eq!(
        fixture
            .backend
            .settle_stale_heads(TENANT, REGION, &settled, &follower, &stopping),
        0,
        "a pull request already on its tip is left alone"
    );
}
