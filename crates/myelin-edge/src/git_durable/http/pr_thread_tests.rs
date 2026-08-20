use super::*;
use crate::repo_authz::GrantBackedRepos;
use crate::request::EdgeRequest;
use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region as IdRegion, TenantId};
use std::collections::BTreeMap;

const TENANT: &str = "acme";
const REGION: &str = "eu-west";
const SLUG: &str = "core";

fn temp_root(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "myelin-prthread-{tag}-{}-{nanos}",
        std::process::id()
    ))
}

fn human(id: &str) -> Principal {
    Principal::new(
        TenantId(TENANT.into()),
        IdRegion(REGION.into()),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn serve(
    handler: &dyn Handler,
    method: &str,
    viewer: &Principal,
    params: &[(&str, &str)],
    body: Value,
) -> Result<Value, EdgeError> {
    let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
    let pmap: BTreeMap<String, String> = params
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let bytes = if body.is_null() {
        vec![]
    } else {
        serde_json::to_vec(&body).unwrap()
    };
    let headers = if method == "GET" {
        vec![]
    } else {
        let mut retry_key = blake3::Hasher::new();
        retry_key.update(method.as_bytes());
        for (name, value) in params {
            retry_key.update(name.as_bytes());
            retry_key.update(value.as_bytes());
        }
        retry_key.update(&bytes);
        vec![(
            "idempotency-key".into(),
            format!("pr-thread-test-{}", retry_key.finalize().to_hex()),
        )]
    };
    let req = EdgeRequest::new(method, "/v1/git/x", "", headers, bytes);
    let identity = crate::catalogue::test_request_identity(viewer, &scope);
    let ctx = HandlerCtx {
        identity: &identity,
        principal: viewer,
        scope: &scope,
        params: &pmap,
        request: &req,
    };
    handler.handle(&ctx).map(|r| r.json_body().expect("json"))
}

fn setup(tag: &str, head_oid: &str) -> (Arc<DurableGitBackend>, Principal, Principal) {
    let root = temp_root(tag);
    let authz = GrantBackedRepos::new()
        .grant_write("u:writer", TENANT, SLUG)
        .grant_read("u:reader", TENANT, SLUG);
    let writer = human("u:writer");
    let reader = human("u:reader");
    let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
    be.create_repo_as(TENANT, REGION, SLUG, &writer).unwrap();
    let body = json!({
        "title": "R3.3 flagship", "base_ref": "refs/heads/main",
        "head_ref": "refs/heads/feature", "head_oid": head_oid, "draft": false,
    });
    be.open_pr(TENANT, REGION, SLUG, &body, &writer).unwrap();
    (Arc::new(be), writer, reader)
}

#[test]
fn thread_write_admits_requested_reviewer_or_repo_pusher_only() {
    let (be, writer, reader) = setup("authz", &"0".repeat(40));
    let list = guarded(
        &be,
        RepoPermission::Pull,
        Arc::new(DPrThreads { be: be.clone() }),
    );
    let v = serve(
        &*list,
        "GET",
        &reader,
        &[("repo", SLUG), ("n", "1")],
        Value::Null,
    )
    .expect("reader may read threads");
    assert!(v["threads"].is_array());
    let create = pr_review_guarded(&be, Arc::new(DPrThreadCreate { be: be.clone() }));
    let err = serve(
        &*create,
        "POST",
        &reader,
        &[("repo", SLUG), ("n", "1")],
        json!({ "body_md": "hi" }),
    )
    .expect_err("an unrelated read-only viewer must be forbidden from commenting");
    assert!(matches!(err, EdgeError::Forbidden(_)), "got {err:?}");

    serve(
        &*create,
        "POST",
        &writer,
        &[("repo", SLUG), ("n", "1")],
        json!({ "body_md": "writer comment" }),
    )
    .expect("a repo pusher may review");

    let loc = DurableGitBackend::loc(TENANT, REGION, SLUG);
    be.prs
        .update(&loc, 1, |record| {
            record.reviews.push(ReviewRecord {
                reviewer_pseudonym: DurableGitBackend::pseudonym(TENANT, &reader),
                state: ReviewState::Requested,
                is_agent: false,
            });
            Ok(())
        })
        .expect("request review");
    serve(
        &*create,
        "POST",
        &reader,
        &[("repo", SLUG), ("n", "1")],
        json!({ "body_md": "requested-reviewer comment" }),
    )
    .expect("a directly requested reviewer may review without repo Push");
}

#[test]
fn opening_a_pull_request_records_unique_requested_reviewers() {
    let (be, writer, reader) = setup("requested-reviewers", &"0".repeat(40));
    let opened = be
        .open_pr(
            TENANT,
            REGION,
            SLUG,
            &json!({
                "title": "Request review while opening",
                "base_ref": "refs/heads/main",
                "head_ref": "refs/heads/second-feature",
                "head_oid": "1".repeat(40),
                "reviewers": ["u:reader", "u:reader", "u:writer"],
            }),
            &writer,
        )
        .expect("open pull request");

    assert_eq!(
        opened.reviews.len(),
        1,
        "duplicates and the author are omitted"
    );
    assert_eq!(
        opened.reviews[0].reviewer_pseudonym,
        "u:reader@acme.noreply"
    );
    assert_eq!(opened.reviews[0].state, ReviewState::Requested);
    assert!(
        be.authorize_pr_review(TENANT, REGION, SLUG, opened.number, &reader),
        "a requested reader may participate in the review"
    );

    let malformed = be
        .open_pr(
            TENANT,
            REGION,
            SLUG,
            &json!({
                "title": "Reject malformed reviewer",
                "base_ref": "refs/heads/main",
                "head_ref": "refs/heads/third-feature",
                "head_oid": "2".repeat(40),
                "reviewers": [" u:reader"],
            }),
            &writer,
        )
        .expect_err("reviewer ids must be canonical");
    assert!(matches!(malformed, DurableError::Git(message) if message.contains("reviewer ids")));
}

#[test]
fn oversized_comment_is_rejected_before_conversation_storage() {
    let (be, writer, _reader) = setup("body-limit", &"0".repeat(40));
    let create = pr_review_guarded(&be, Arc::new(DPrThreadCreate { be: be.clone() }));

    let error = serve(
        &*create,
        "POST",
        &writer,
        &[("repo", SLUG), ("n", "1")],
        json!({
            "body_md": "x".repeat(myelin_git::pr_threads::MAX_COMMENT_BODY_BYTES + 1),
        }),
    )
    .expect_err("oversized comment must fail before persistence");

    assert!(matches!(error, EdgeError::BadRequest(_)), "got {error:?}");
    assert!(be
        .threads
        .load(&DurableGitBackend::loc(TENANT, REGION, SLUG), "pr:core:1")
        .unwrap()
        .threads
        .is_empty());
}

#[test]
fn pending_comment_is_private_and_submit_replays_its_receipt() {
    let (be, writer, reader) = setup("pending", &"0".repeat(40));
    let threads = Arc::new(DPrThreads { be: be.clone() });
    let start = Arc::new(DPrReviewStart { be: be.clone() });
    let pending = Arc::new(DPrReviewComment { be: be.clone() });
    let submit = Arc::new(DPrReviewSubmit { be: be.clone() });

    let batch = serve(
        &*start,
        "POST",
        &reader,
        &[("repo", SLUG), ("n", "1")],
        Value::Null,
    )
    .unwrap();
    let rid = batch["applied"]["review"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    serve(
        &*pending,
        "POST",
        &reader,
        &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
        json!({ "body_md": "draft note" }),
    )
    .unwrap();

    let seen = serve(
        &*threads,
        "GET",
        &writer,
        &[("repo", SLUG), ("n", "1")],
        Value::Null,
    )
    .unwrap();
    assert_eq!(
        seen["threads"].as_array().unwrap().len(),
        0,
        "pending comment is private"
    );
    assert_eq!(
        seen["reviews"].as_array().unwrap().len(),
        0,
        "draft batch is hidden"
    );

    let first = serve(
        &*submit,
        "POST",
        &reader,
        &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
        json!({ "verdict": "commented" }),
    )
    .unwrap();
    assert_eq!(first["applied"]["result"]["emitted"], true);
    let again = serve(
        &*submit,
        "POST",
        &reader,
        &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
        json!({ "verdict": "commented" }),
    )
    .unwrap();
    assert_eq!(again, first, "a retry returns the first durable receipt");

    let seen = serve(
        &*threads,
        "GET",
        &writer,
        &[("repo", SLUG), ("n", "1")],
        Value::Null,
    )
    .unwrap();
    assert_eq!(
        seen["threads"].as_array().unwrap().len(),
        1,
        "submit makes it public"
    );
}

#[test]
fn a_changes_requested_batch_blocks_the_gate() {
    let (be, _writer, reader) = setup("blockgate", &"0".repeat(40));
    let start = Arc::new(DPrReviewStart { be: be.clone() });
    let submit = Arc::new(DPrReviewSubmit { be: be.clone() });
    let checks = Arc::new(DPrChecks { be: be.clone() });

    let batch = serve(
        &*start,
        "POST",
        &reader,
        &[("repo", SLUG), ("n", "1")],
        Value::Null,
    )
    .unwrap();
    let rid = batch["applied"]["review"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    serve(
        &*submit,
        "POST",
        &reader,
        &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
        json!({ "verdict": "changes_requested" }),
    )
    .unwrap();
    let ck = serve(
        &*checks,
        "GET",
        &reader,
        &[("repo", SLUG), ("n", "1")],
        Value::Null,
    )
    .unwrap();
    assert_eq!(
        ck["changes_requested"], true,
        "the gate ingests changes_requested"
    );
    assert_eq!(
        ck["gate_admitted"], false,
        "a live request-changes blocks the merge"
    );
}

#[test]
fn a_blocked_merge_returns_409_with_rerendered_checks() {
    let root = temp_root("merge409");
    let authz = GrantBackedRepos::new().grant_admin("u:writer", TENANT, SLUG);
    let writer = human("u:writer");
    let be = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz)),
    );
    be.create_repo_as(TENANT, REGION, SLUG, &writer).unwrap();
    be.open_pr(
        TENANT,
        REGION,
        SLUG,
        &json!({ "title": "N6", "base_ref": "refs/heads/main", "head_ref": "refs/heads/feature",
                     "head_oid": "0".repeat(40), "draft": false }),
        &writer,
    )
    .unwrap();
    let merge = guarded(
        &be,
        RepoPermission::ProtectedPush,
        Arc::new(DMerge { be: be.clone() }),
    );
    let resp = serve(
        &*merge,
        "POST",
        &writer,
        &[("repo", SLUG), ("n", "1")],
        Value::Null,
    )
    .expect("merge handler returns a body (409 is an Ok EdgeResponse, not an Err)");
    assert_eq!(resp["error"]["code"], "merge_blocked");
    assert_eq!(
        resp["checks"]["gate_admitted"], false,
        "the 409 carries the fresh gate state"
    );
}

fn setup_diff(tag: &str) -> (Arc<DurableGitBackend>, Principal, String) {
    let root = temp_root(tag);
    let authz = GrantBackedRepos::new()
        .grant_write("u:writer", TENANT, SLUG)
        .grant_read("u:reader", TENANT, SLUG);
    let writer = human("u:writer");
    let reader = human("u:reader");
    let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
    be.create_repo_as(TENANT, REGION, SLUG, &writer).unwrap();
    let loc = DurableGitBackend::loc(TENANT, REGION, SLUG);
    let repo = be.store.open_repo(&loc).unwrap();
    let b0 = repo.write_blob(b"a\nb\nc\n").unwrap();
    let t0 = repo.write_tree(&[("file.txt", &b0)]).unwrap();
    let base = repo
        .write_commit(&t0, &[], "base", "psn@acme.noreply", "psn@acme.noreply")
        .unwrap();
    repo.update_ref_cas(
        "refs/heads/main",
        None,
        Some(&base),
        "c",
        "psn@acme.noreply",
    )
    .unwrap();
    let bh = repo.write_blob(b"a\nB\nc\nd\n").unwrap();
    let th = repo.write_tree(&[("file.txt", &bh)]).unwrap();
    let head = repo
        .write_commit(
            &th,
            &[&base],
            "head",
            "psn@acme.noreply",
            "psn@acme.noreply",
        )
        .unwrap();
    be.open_pr(
        TENANT,
        REGION,
        SLUG,
        &json!({ "title": "diff pr", "base_ref": "refs/heads/main",
                     "head_ref": "refs/heads/feature", "head_oid": head.0, "draft": false }),
        &writer,
    )
    .unwrap();
    (Arc::new(be), reader, head.0)
}

#[test]
fn line_anchors_are_strictly_validated_and_revision_bound() {
    let (be, reviewer, head) = setup_diff("anchors");
    let new_side = be
        .create_thread(
            RepoActorContext::new(TENANT, REGION, SLUG, &reviewer).for_pr(1),
            "new-side-anchor",
            &json!({
                "body_md": "new-side note",
                "anchor": { "path": "file.txt", "line": 4, "side": "new" },
            }),
        )
        .expect("a displayed new-side line resolves");
    assert_eq!(new_side["anchor"]["side"], "new");
    assert_eq!(new_side["anchor"]["head_oid"], head);
    assert_eq!(new_side["anchor"]["base_oid"].as_str().unwrap().len(), 40);

    let old_side = be
        .create_thread(
            RepoActorContext::new(TENANT, REGION, SLUG, &reviewer).for_pr(1),
            "old-side-anchor",
            &json!({
                "body_md": "old-side note",
                "anchor": { "path": "file.txt", "line": 2, "side": "old" },
            }),
        )
        .expect("a displayed old-side line resolves");
    assert_eq!(old_side["anchor"]["side"], "old");

    for (index, invalid) in [
            json!({ "body_md": "missing side", "anchor": { "path": "file.txt", "line": 2 } }),
            json!({ "body_md": "stale line", "anchor": { "path": "file.txt", "line": 99, "side": "new" } }),
            json!({ "body_md": "unsafe path", "anchor": { "path": "../secret", "line": 1, "side": "new" } }),
        ]
        .into_iter()
        .enumerate()
        {
            let error = be
                .create_thread(
                    RepoActorContext::new(TENANT, REGION, SLUG, &reviewer).for_pr(1),
                    &format!("invalid-anchor-{index}"),
                    &invalid,
                )
                .expect_err("malformed or stale anchor must be rejected");
            assert!(error.to_string().contains("anchor"), "got {error:?}");
        }

    let stored = be
        .threads
        .load(&DurableGitBackend::loc(TENANT, REGION, SLUG), "pr:core:1")
        .unwrap();
    assert_eq!(stored.threads.len(), 2, "invalid anchors persisted nothing");
}

#[test]
fn pr_diff_is_pull_guarded_zero_leak_and_three_dot() {
    let (be, reader, head) = setup_diff("diffauthz");
    let guard = guarded(
        &be,
        RepoPermission::Pull,
        Arc::new(DPrDiff { be: be.clone() }),
    );
    let v = serve(
        &*guard,
        "GET",
        &reader,
        &[("repo", SLUG), ("n", "1")],
        Value::Null,
    )
    .expect("a reader may view the PR diff");
    assert_eq!(v["number"], 1);
    assert_eq!(
        v["three_dot"], true,
        "durable repos are libgit2-backed → merge-base"
    );
    assert_eq!(v["total_files"], 1);
    assert_eq!(v["files"][0]["path"], "file.txt");
    assert_eq!(v["files"][0]["status"], "M");
    assert_eq!(v["files"][0]["kind"], "text");
    let new_blob_oid = v["files"][0]["new_blob_oid"]
        .as_str()
        .expect("visible text files carry their immutable new-side blob oid");
    assert_eq!(new_blob_oid.len(), 40);
    assert_ne!(
        new_blob_oid, head,
        "the blob oid must never be the PR head commit oid"
    );
    let lines = v["files"][0]["hunks"][0]["lines"].as_array().unwrap();
    assert!(lines
        .iter()
        .any(|l| l["origin"] == "+" && l["content"] == "d" && l["new_no"] == 4));
    assert_eq!(
        v["restricted_files"], 0,
        "count-only; 0 under the repo-level Pull guard"
    );
    assert!(v["restricted_files"].is_number());

    let stranger = human("u:stranger");
    let err = serve(
        &*guard,
        "GET",
        &stranger,
        &[("repo", SLUG), ("n", "1")],
        Value::Null,
    )
    .expect_err("a stranger must not view the diff");
    assert!(
        matches!(err, EdgeError::NotFound(_)),
        "0-leak 404, got {err:?}"
    );
}

#[test]
fn pr_diff_absent_pr_is_not_found() {
    let (be, reader, _head) = setup_diff("diffabsent");
    let guard = guarded(
        &be,
        RepoPermission::Pull,
        Arc::new(DPrDiff { be: be.clone() }),
    );
    let err = serve(
        &*guard,
        "GET",
        &reader,
        &[("repo", SLUG), ("n", "999")],
        Value::Null,
    )
    .expect_err("absent PR");
    assert!(matches!(err, EdgeError::NotFound(_)), "got {err:?}");
}
