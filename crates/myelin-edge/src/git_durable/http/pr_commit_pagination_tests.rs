use super::*;
use crate::request::EdgeRequest;
use base64::Engine as _;
use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_tenancy::{Region as IdentityRegion, TenantId};
use std::collections::BTreeMap;

const TENANT: &str = "acme";
const REGION: &str = "eu-west";
const REPO: &str = "core";

struct Fixture {
    root: PathBuf,
    be: DurableGitBackend,
    viewer: Principal,
    base: CoreOid,
    second: CoreOid,
    third: CoreOid,
    head: CoreOid,
}

fn temp_root(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "myelin-pr-commit-pages-{tag}-{}-{nanos}",
        std::process::id()
    ))
}

fn human(tenant: &str, region: &str, id: &str) -> Principal {
    Principal::new(
        TenantId(tenant.into()),
        IdentityRegion(region.into()),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn fixture(tag: &str) -> Fixture {
    let root = temp_root(tag);
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    let viewer = human(TENANT, REGION, "u:viewer");
    be.create_repo_as(TENANT, REGION, REPO, &viewer).unwrap();
    let loc = DurableGitBackend::loc(TENANT, REGION, REPO);
    let repo = be.store.open_repo(&loc).unwrap();
    let blob = repo.write_blob(b"snapshot\n").unwrap();
    let tree = repo.write_tree(&[("file.txt", &blob)]).unwrap();
    let base = repo
        .write_commit(&tree, &[], "base", "psn@acme.noreply", "psn@acme.noreply")
        .unwrap();
    repo.update_ref_cas(
        "refs/heads/main",
        None,
        Some(&base),
        "base",
        "psn@acme.noreply",
    )
    .unwrap();
    let second = repo
        .write_commit(
            &tree,
            &[&base],
            "second",
            "psn@acme.noreply",
            "psn@acme.noreply",
        )
        .unwrap();
    let third = repo
        .write_commit(
            &tree,
            &[&second],
            "third",
            "psn@acme.noreply",
            "psn@acme.noreply",
        )
        .unwrap();
    let head = repo
        .write_commit(
            &tree,
            &[&third],
            "head",
            "psn@acme.noreply",
            "psn@acme.noreply",
        )
        .unwrap();
    be.open_pr(
        TENANT,
        REGION,
        REPO,
        &json!({
            "title": "Snapshot PR",
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            "head_oid": head.0,
        }),
        &viewer,
    )
    .unwrap();
    Fixture {
        root,
        be,
        viewer,
        base,
        second,
        third,
        head,
    }
}

fn serve(
    handler: &dyn Handler,
    viewer: &Principal,
    repo: &str,
    number: u64,
    query: &str,
) -> Result<EdgeResponse, EdgeError> {
    let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
    let params = BTreeMap::from([
        ("repo".to_string(), repo.to_string()),
        ("n".to_string(), number.to_string()),
    ]);
    let request = EdgeRequest::new(
        "GET",
        format!("/v1/git/repos/{repo}/prs/{number}/commits"),
        query,
        vec![],
        vec![],
    );
    let identity = crate::catalogue::test_request_identity(viewer, &scope);
    handler.handle(&HandlerCtx {
        identity: &identity,
        principal: viewer,
        scope: &scope,
        params: &params,
        request: &request,
    })
}

fn json(response: EdgeResponse) -> Value {
    response.json_body().expect("JSON response")
}

fn cursor_from(body: &Value) -> String {
    body["page"]["next_cursor"]
        .as_str()
        .expect("next cursor")
        .to_string()
}

fn mutate_cursor(cursor: &str, mutation: impl FnOnce(&mut [u8])) -> String {
    let encoded = cursor.strip_prefix("pc1_").unwrap();
    let mut frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .unwrap();
    mutation(&mut frame);
    format!(
        "pc1_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
    )
}

#[test]
fn strict_query_accepts_only_canonical_bounded_parameters() {
    let cursor = PrCommitCursor::new([4; 32], Some(&"1".repeat(40)), &"2".repeat(40), 1)
        .unwrap()
        .encode();
    for valid in [
        "".to_string(),
        "limit=1".to_string(),
        format!("cursor={cursor}"),
        format!("limit=100&cursor={cursor}"),
        format!("cursor={cursor}&limit=2"),
    ] {
        assert!(
            parse_pr_commit_query(&valid).is_ok(),
            "valid query: {valid}"
        );
    }

    let wrong_version = mutate_cursor(&cursor, |frame| frame[0] = 2);
    let overflow = mutate_cursor(&cursor, |frame| {
        frame[74..78].copy_from_slice(&u32::MAX.to_be_bytes())
    });
    for malformed in [
        "limit".to_string(),
        "cursor".to_string(),
        "limit=".to_string(),
        "cursor=".to_string(),
        "limit=0".to_string(),
        "limit=01".to_string(),
        "limit=101".to_string(),
        "limit=1&limit=2".to_string(),
        format!("cursor={cursor}&cursor={cursor}"),
        "unknown=x".to_string(),
        "limit=1&".to_string(),
        "=1".to_string(),
        format!("cursor={cursor}="),
        format!("cursor={wrong_version}"),
        format!("cursor={overflow}"),
        format!("cursor=pc1_{}", "a".repeat(256)),
        "x".repeat(PR_COMMIT_QUERY_MAX_BYTES + 1),
    ] {
        assert!(
            matches!(
                parse_pr_commit_query(&malformed),
                Err(EdgeError::BadRequest(_))
            ),
            "malformed query must be rejected: {malformed}"
        );
    }
}

#[test]
fn capacity_errors_are_a_sanitized_payload_too_large_response() {
    assert!(matches!(
        map_pr_commit_page_error(PrCommitPageError::CapacityExceeded),
        EdgeError::PayloadTooLarge(message)
            if message == "pull request commit history exceeds the interactive walk limit"
    ));
}

#[test]
fn exact_pages_repeat_deterministically_and_keep_the_pinned_snapshot() {
    let fixture = fixture("pages");
    let handler = DPrCommits {
        be: Arc::new(fixture.be),
    };
    let first = json(serve(&handler, &fixture.viewer, REPO, 1, "limit=1").unwrap());
    assert_eq!(
        first
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["items", "page"]
    );
    assert_eq!(
        first["page"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["limit", "next_cursor"]
    );
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["oid"], fixture.head.0);
    assert_eq!(first["page"]["limit"], 1);
    let first_cursor = cursor_from(&first);
    assert!(first_cursor.starts_with("pc1_"));

    let second_query = format!("cursor={first_cursor}&limit=1");
    let second = json(serve(&handler, &fixture.viewer, REPO, 1, &second_query).unwrap());
    let repeated = json(serve(&handler, &fixture.viewer, REPO, 1, &second_query).unwrap());
    assert_eq!(second, repeated, "the same cursor is deterministic");
    let second_oid = second["items"][0]["oid"].as_str().unwrap().to_string();

    let loc = DurableGitBackend::loc(TENANT, REGION, REPO);
    let repo = handler.be.store.open_repo(&loc).unwrap();
    repo.update_ref_cas(
        "refs/heads/main",
        Some(&fixture.base),
        Some(&fixture.second),
        "advance live base",
        "psn@acme.noreply",
    )
    .unwrap();
    let live_head = repo
        .write_commit(
            &repo.write_tree(&[]).unwrap(),
            &[&fixture.head],
            "new live head",
            "psn@acme.noreply",
            "psn@acme.noreply",
        )
        .unwrap();
    handler
        .be
        .prs
        .update(&loc, 1, |record| {
            record.head_oid = live_head.0.clone();
            Ok(())
        })
        .unwrap();

    let still_second = json(serve(&handler, &fixture.viewer, REPO, 1, &second_query).unwrap());
    assert_eq!(still_second["items"][0]["oid"], second_oid);
    let third_cursor = cursor_from(&second);
    let third = json(
        serve(
            &handler,
            &fixture.viewer,
            REPO,
            1,
            &format!("limit=1&cursor={third_cursor}"),
        )
        .unwrap(),
    );
    let third_oid = third["items"][0]["oid"].as_str().unwrap().to_string();
    assert!(third["page"]["next_cursor"].is_null());
    assert_eq!(
        std::collections::BTreeSet::from([fixture.head.0.clone(), second_oid, third_oid,]),
        std::collections::BTreeSet::from([fixture.head.0, fixture.third.0, fixture.second.0,]),
        "the pinned pages contain each PR-owned commit exactly once"
    );
    std::fs::remove_dir_all(&fixture.root).ok();
}

#[test]
fn scope_replay_and_expired_snapshots_fail_cleanly() {
    let fixture = fixture("scope");
    let handler = DPrCommits {
        be: Arc::new(fixture.be),
    };
    handler
        .be
        .open_pr(
            TENANT,
            REGION,
            REPO,
            &json!({
                "title": "Second PR",
                "base_ref": "refs/heads/main",
                "head_ref": "refs/heads/other",
                "head_oid": fixture.head.0,
            }),
            &fixture.viewer,
        )
        .unwrap();
    let first = json(serve(&handler, &fixture.viewer, REPO, 1, "limit=1").unwrap());
    let cursor = cursor_from(&first);

    assert!(matches!(
        serve(
            &handler,
            &fixture.viewer,
            REPO,
            2,
            &format!("cursor={cursor}"),
        ),
        Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
    ));

    let other_repo = "other";
    handler
        .be
        .create_repo_as(TENANT, REGION, other_repo, &fixture.viewer)
        .unwrap();
    handler
        .be
        .open_pr(
            TENANT,
            REGION,
            other_repo,
            &json!({
                "title": "Other repo",
                "base_ref": "refs/heads/main",
                "head_ref": "refs/heads/feature",
                "head_oid": "0".repeat(40),
            }),
            &fixture.viewer,
        )
        .unwrap();
    assert!(matches!(
        serve(
            &handler,
            &fixture.viewer,
            other_repo,
            1,
            &format!("cursor={cursor}"),
        ),
        Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
    ));

    for (tenant, region) in [("other-tenant", REGION), (TENANT, "other-region")] {
        let viewer = human(tenant, region, "u:viewer");
        handler
            .be
            .create_repo_as(tenant, region, REPO, &viewer)
            .unwrap();
        handler
            .be
            .open_pr(
                tenant,
                region,
                REPO,
                &json!({
                    "title": "Other scope",
                    "base_ref": "refs/heads/main",
                    "head_ref": "refs/heads/feature",
                    "head_oid": "0".repeat(40),
                }),
                &viewer,
            )
            .unwrap();
        assert!(matches!(
            serve(&handler, &viewer, REPO, 1, &format!("cursor={cursor}")),
            Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
        ));
    }

    let expired = PrCommitCursor::new(
        pr_commit_cursor_scope(TENANT, REGION, REPO, 1),
        None,
        &"f".repeat(40),
        1,
    )
    .unwrap()
    .encode();
    assert!(matches!(
        serve(
            &handler,
            &fixture.viewer,
            REPO,
            1,
            &format!("cursor={expired}"),
        ),
        Err(EdgeError::Conflict(message)) if message == "pull request commit cursor expired"
    ));
    let expired_base = PrCommitCursor::new(
        pr_commit_cursor_scope(TENANT, REGION, REPO, 1),
        Some(&"e".repeat(40)),
        &fixture.head.0,
        1,
    )
    .unwrap()
    .encode();
    assert!(matches!(
        serve(
            &handler,
            &fixture.viewer,
            REPO,
            1,
            &format!("cursor={expired_base}"),
        ),
        Err(EdgeError::Conflict(message)) if message == "pull request commit cursor expired"
    ));
    std::fs::remove_dir_all(&fixture.root).ok();
}

#[test]
fn pull_denial_precedes_malformed_cursor_parsing() {
    let fixture = fixture("guard-order");
    let denied = Arc::new(fixture.be.with_repo_authorizer(Arc::new(DenyAllRepos)));
    let handler = guarded(
        &denied,
        RepoPermission::Pull,
        Arc::new(DPrCommits { be: denied.clone() }),
    );
    assert!(matches!(
        serve(&*handler, &fixture.viewer, REPO, 1, "cursor=malformed"),
        Err(EdgeError::NotFound(message)) if message == "repository not found"
    ));
    std::fs::remove_dir_all(&fixture.root).ok();
}
