use super::*;
use crate::repo_authz::GrantBackedRepos;
use crate::request::EdgeRequest;
use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region as IdRegion, TenantId};
use std::collections::BTreeMap;

const TENANT: &str = "acme";
const REGION: &str = "eu-west";

fn temp_root(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "myelin-prlist-{tag}-{}-{nanos}",
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

fn open_pr(be: &DurableGitBackend, slug: &str, title: &str, opener: &Principal) {
    be.create_repo_as(TENANT, REGION, slug, opener).ok();
    let body = json!({
        "title": title,
        "base_ref": "refs/heads/main",
        "head_ref": "refs/heads/feature",
        "head_oid": "0".repeat(40),
        "draft": false,
    });
    be.open_pr(TENANT, REGION, slug, &body, opener)
        .unwrap_or_else(|e| panic!("open PR in {slug}: {e:?}"));
}

fn repo_pr_bytes(be: &DurableGitBackend, slug: &str, viewer: &Principal) -> usize {
    let loc = DurableGitBackend::loc(TENANT, REGION, slug);
    let records = be.pr_list(&loc, viewer).expect("read seeded PR records");
    serialized_pr_records_bytes(&records).expect("measure seeded PR records")
}

fn serve(handler: &dyn Handler, viewer: &Principal, repo: Option<&str>, query: &str) -> Value {
    serve_result(handler, viewer, repo, query).unwrap_or_else(|e| panic!("handler errored: {e:?}"))
}

fn serve_result(
    handler: &dyn Handler,
    viewer: &Principal,
    repo: Option<&str>,
    query: &str,
) -> Result<Value, EdgeError> {
    let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
    let mut params = BTreeMap::new();
    if let Some(r) = repo {
        params.insert("repo".to_string(), r.to_string());
    }
    let req = EdgeRequest::new("GET", "/v1/git/prs", query, vec![], vec![]);
    let identity = crate::catalogue::test_request_identity(viewer, &scope);
    let ctx = HandlerCtx {
        identity: &identity,
        principal: viewer,
        scope: &scope,
        params: &params,
        request: &req,
    };
    handler
        .handle(&ctx)
        .map(|response| response.json_body().expect("json body"))
}

#[test]
fn forged_or_over_cap_pr_list_cursor_is_a_clean_bad_request() {
    let root = temp_root("forged-cursor");
    let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
    let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
    let viewer = human("u:viewer");
    open_pr(&be, "core", "Only PR", &viewer);
    let handler = DRepoPrList { be: Arc::new(be) };
    for cursor in [
        usize::MAX.to_string(),
        "10001".into(),
        "01".into(),
        "+1".into(),
    ] {
        let error = serve_result(
            &handler,
            &viewer,
            Some("core"),
            &format!("state=all&cursor={cursor}"),
        )
        .expect_err("a noncanonical or over-cap cursor must be rejected");
        assert_eq!(
            error,
            EdgeError::BadRequest("invalid pull request cursor".into())
        );
    }
    for query in [
        "state=unknown",
        "sort=oldest",
        "state=open&state=all",
        "sort=updated&sort=created",
        "cursor=1&cursor=2",
        "limit=1&limit=2",
        "unknown=value",
        "limit",
        "=value",
        "limit=01",
        "limit=0",
        "limit=101",
        "limit=%ZZ",
        "cursor=",
    ] {
        assert!(matches!(
            serve_result(&handler, &viewer, Some("core"), query),
            Err(EdgeError::BadRequest(_))
        ));
    }
    let oversized = format!("state=open&x={}", "a".repeat(PR_LIST_QUERY_MAX_BYTES));
    assert!(matches!(
        serve_result(&handler, &viewer, Some("core"), &oversized),
        Err(EdgeError::BadRequest(_))
    ));
    let body = serve(&handler, &viewer, Some("core"), "state=all&cursor=10000");
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        0,
        "the capped out-of-range coordinate is an empty page"
    );
    assert_eq!(body["counts"]["all"], 1, "empty pages retain exact badges");
    assert_eq!(body["page"]["total"], 1);
    assert!(
        body["page"]["next_cursor"].is_null(),
        "no next past the end"
    );

    let capped = repo_pr_list_envelope(EnrichedPrSlice {
        rows: Vec::new(),
        counts: PrListCounts::default(),
        total: PR_LIST_OFFSET_MAX + 1,
        offset: PR_LIST_OFFSET_MAX,
        limit: 100,
        next_cursor: None,
        prev_cursor: None,
    });
    assert!(
        capped["page"]["next_cursor"].is_null(),
        "the transitional ceiling never emits a cursor its strict parser will reject"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn repository_candidate_scan_stops_before_unbounded_materialization() {
    let root = temp_root("repo-scan-bound");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    let viewer = human("u:viewer");
    for slug in ["alpha", "beta"] {
        be.create_repo_as(TENANT, REGION, slug, &viewer).unwrap();
    }

    let error = be
        .scan_repo_slugs_bounded(TENANT, REGION, 1)
        .expect_err("the second repository must trip the candidate ceiling");
    assert!(matches!(
        error,
        DurableError::Git(message)
            if message == "browse response limit exceeded: repository candidate count"
    ));
    assert_eq!(
        be.scan_repo_slugs_bounded(TENANT, REGION, 2).unwrap(),
        vec!["alpha".to_string(), "beta".to_string()]
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn oversized_title_is_rejected_at_create() {
    let root = temp_root("title-cap");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    let author = human("u:author");
    be.create_repo_as(TENANT, REGION, "core", &author).unwrap();
    let body = json!({
        "title": "x".repeat(513),
        "base_ref": "refs/heads/main",
        "head_ref": "refs/heads/feature",
        "head_oid": "0".repeat(40),
    });
    let err = be.open_pr(TENANT, REGION, "core", &body, &author);
    assert!(err.is_err(), "513-byte title must be rejected");
    let ok_body = json!({
        "title": "x".repeat(512),
        "base_ref": "refs/heads/main",
        "head_ref": "refs/heads/feature",
        "head_oid": "0".repeat(40),
    });
    assert!(be
        .open_pr(TENANT, REGION, "core", &ok_body, &author)
        .is_ok());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn cross_repo_bucket_never_leaks_a_forbidden_repos_pr() {
    let root = temp_root("cross-leak");
    let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "alpha");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    let viewer = human("u:viewer");
    open_pr(&be, "alpha", "Alpha change", &viewer);
    open_pr(&be, "beta", "Beta change (forbidden repo)", &viewer);
    let be = be.with_repo_authorizer(Arc::new(authz));

    let handler = DMyPrs { be: Arc::new(be) };
    let body = serve(&handler, &viewer, None, "bucket=yours");
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "only the visible repo's PR is listed");
    assert_eq!(items[0]["repo"], "alpha");
    assert_eq!(items[0]["title"], "Alpha change");
    assert_eq!(body["counts"]["bucket"], 1);
    assert_eq!(body["page"]["total"], 1);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn cross_repo_query_is_strict_and_fs_order_has_repo_tie_breaker() {
    let root = temp_root("cross-strict-query");
    let viewer = human("u:viewer");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    open_pr(&be, "beta", "Beta", &viewer);
    open_pr(&be, "alpha", "Alpha", &viewer);
    let authz = GrantBackedRepos::new()
        .grant_read("u:viewer", TENANT, "alpha")
        .grant_read("u:viewer", TENANT, "beta");
    let handler = DMyPrs {
        be: Arc::new(be.with_repo_authorizer(Arc::new(authz))),
    };
    for query in [
        "bucket=unknown",
        "bucket=yours&bucket=needs-review",
        "sort=oldest",
        "sort=updated&sort=created",
        "cursor=01",
        "cursor=10001",
        "cursor=1&cursor=2",
        "limit=0",
        "limit=01",
        "limit=101",
        "limit=1&limit=2",
        "unknown=value",
        "limit",
        "limit=%ZZ",
    ] {
        assert!(matches!(
            serve_result(&handler, &viewer, None, query),
            Err(EdgeError::BadRequest(_))
        ));
    }

    let first = serve(&handler, &viewer, None, "bucket=yours&sort=created&limit=1");
    assert_eq!(first["counts"]["bucket"], 2);
    assert_eq!(first["items"][0]["repo"], "alpha");
    let next = first["page"]["next_cursor"].as_str().unwrap();
    assert!(next.starts_with(PR_LIST_CURSOR_PREFIX));
    let second = serve(&handler, &viewer, None, &format!("cursor={next}"));
    assert_eq!(second["items"][0]["repo"], "beta");
    assert!(second["page"]["next_cursor"].is_null());

    let capped = cross_pr_list_envelope(EnrichedCrossPrSlice {
        rows: Vec::new(),
        total: PR_LIST_OFFSET_MAX + 1,
        offset: PR_LIST_OFFSET_MAX,
        limit: 100,
        next_cursor: None,
        prev_cursor: None,
    });
    assert!(capped["page"]["next_cursor"].is_null());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn empty_visible_cross_repo_set_is_exact_empty() {
    let root = temp_root("cross-empty-visible");
    let viewer = human("u:viewer");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    open_pr(&be, "hidden", "Hidden", &viewer);
    let handler = DMyPrs {
        be: Arc::new(be.with_repo_authorizer(Arc::new(GrantBackedRepos::new()))),
    };
    let body = serve(&handler, &viewer, None, "bucket=yours");
    assert!(body["items"].as_array().unwrap().is_empty());
    assert_eq!(body["counts"]["bucket"], 0);
    assert_eq!(body["page"]["total"], 0);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn cross_repo_record_ceiling_applies_across_visible_repositories() {
    let root = temp_root("cross-record-cap");
    let viewer = human("u:viewer");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    for slug in ["alpha", "beta"] {
        open_pr(&be, slug, &format!("{slug} one"), &viewer);
        open_pr(&be, slug, &format!("{slug} two"), &viewer);
    }
    let authz = GrantBackedRepos::new()
        .grant_read("u:viewer", TENANT, "alpha")
        .grant_read("u:viewer", TENANT, "beta");
    let be = be.with_repo_authorizer(Arc::new(authz));

    let error = be
        .list_prs_cross_bounded(
            TENANT,
            REGION,
            &viewer,
            CrossPrListLimits {
                maximum_records: 3,
                maximum_bytes: usize::MAX,
            },
        )
        .err()
        .expect("four collectively visible PRs must exceed a three-record request cap");
    assert!(matches!(
        &error,
        DurableError::Git(message)
            if message == "pull request list limit exceeded: cross-repository record count"
    ));
    assert_eq!(map_durable_err(error).status(), 413);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn cross_repo_byte_ceiling_is_exact_and_aggregate() {
    let root = temp_root("cross-byte-cap");
    let viewer = human("u:viewer");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    open_pr(&be, "alpha", "Alpha payload", &viewer);
    open_pr(&be, "beta", "Beta payload", &viewer);
    let authz = GrantBackedRepos::new()
        .grant_read("u:viewer", TENANT, "alpha")
        .grant_read("u:viewer", TENANT, "beta");
    let be = be.with_repo_authorizer(Arc::new(authz));
    let exact_bytes = repo_pr_bytes(&be, "alpha", &viewer)
        .checked_add(repo_pr_bytes(&be, "beta", &viewer))
        .unwrap();

    let error = be
        .list_prs_cross_bounded(
            TENANT,
            REGION,
            &viewer,
            CrossPrListLimits {
                maximum_records: 2,
                maximum_bytes: exact_bytes - 1,
            },
        )
        .err()
        .expect("the aggregate byte limit applies across repositories");
    assert!(matches!(
        error,
        DurableError::Git(message)
            if message == "pull request list limit exceeded: cross-repository serialized bytes"
    ));

    let exact = be
        .list_prs_cross_bounded(
            TENANT,
            REGION,
            &viewer,
            CrossPrListLimits {
                maximum_records: 2,
                maximum_bytes: exact_bytes,
            },
        )
        .expect("exactly-at-cap records and bytes remain available");
    assert_eq!(exact.len(), 2);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn forbidden_oversized_repo_contributes_neither_work_nor_capacity() {
    let root = temp_root("cross-forbidden-cap");
    let viewer = human("u:viewer");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    open_pr(&be, "alpha", "Visible", &viewer);
    for index in 0..3 {
        open_pr(&be, "hidden", &format!("Hidden {index}"), &viewer);
    }
    let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "alpha");
    let be = be.with_repo_authorizer(Arc::new(authz));
    let visible_bytes = repo_pr_bytes(&be, "alpha", &viewer);

    let rows = be
        .list_prs_cross_bounded(
            TENANT,
            REGION,
            &viewer,
            CrossPrListLimits {
                maximum_records: 1,
                maximum_bytes: visible_bytes,
            },
        )
        .expect("an oversized forbidden repository is excluded before PR reads");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].repo_slug.as_deref(), Some("alpha"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn cross_repo_capacity_accounting_is_overflow_safe() {
    assert_eq!(
        checked_cross_pr_list_total(7, 3, 10, "cross-repository record count").unwrap(),
        10,
        "the exact ceiling is admitted"
    );
    assert!(matches!(
        checked_cross_pr_list_total(
            usize::MAX,
            1,
            usize::MAX,
            "cross-repository record count",
        ),
        Err(DurableError::Git(message))
            if message == "pull request list limit exceeded: cross-repository record count"
    ));
}

#[test]
fn per_repo_list_rows_titles_and_counts() {
    let root = temp_root("per-repo");
    let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
    let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
    let viewer = human("u:viewer");
    open_pr(&be, "core", "First PR", &viewer);
    open_pr(&be, "core", "Second PR", &viewer);

    let handler = DRepoPrList { be: Arc::new(be) };
    let body = serve(&handler, &viewer, Some("core"), "");
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let titles: Vec<&str> = items.iter().map(|i| i["title"].as_str().unwrap()).collect();
    assert!(titles.contains(&"First PR") && titles.contains(&"Second PR"));
    assert_eq!(body["counts"]["open"], 2);
    assert_eq!(body["counts"]["all"], 2);
    assert_eq!(body["counts"]["merged"], 0);
    assert_eq!(body["counts"]["yours"], 2, "the viewer authored both");
    let merged = serve(&handler, &viewer, Some("core"), "state=merged");
    assert_eq!(merged["items"].as_array().unwrap().len(), 0);
    assert_eq!(
        merged["counts"]["open"], 2,
        "the Open badge still reads 2 on the Merged tab"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn per_repo_list_cursor_is_stable_and_bidirectional() {
    let root = temp_root("cursor");
    let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
    let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
    let viewer = human("u:viewer");
    for i in 1..=5 {
        open_pr(&be, "core", &format!("PR {i}"), &viewer);
    }
    let handler = DRepoPrList { be: Arc::new(be) };

    let p1 = serve(&handler, &viewer, Some("core"), "state=all&limit=2");
    assert_eq!(p1["items"].as_array().unwrap().len(), 2);
    assert_eq!(p1["page"]["total"], 5);
    assert!(p1["page"]["prev_cursor"].is_null(), "head has no Newer");
    let c2 = p1["page"]["next_cursor"].as_str().unwrap();
    assert!(c2.starts_with(PR_LIST_CURSOR_PREFIX));

    let p2 = serve(&handler, &viewer, Some("core"), &format!("cursor={c2}"));
    let c1 = p2["page"]["prev_cursor"].as_str().unwrap();
    let c3 = p2["page"]["next_cursor"].as_str().unwrap();
    assert!(c1.starts_with(PR_LIST_CURSOR_PREFIX));
    assert!(c3.starts_with(PR_LIST_CURSOR_PREFIX));
    let back = serve(&handler, &viewer, Some("core"), &format!("cursor={c1}"));
    assert_eq!(back["items"], p1["items"], "Newer returns the prior page");

    let p3 = serve(&handler, &viewer, Some("core"), &format!("cursor={c3}"));
    assert_eq!(p3["items"].as_array().unwrap().len(), 1);
    assert!(p3["page"]["next_cursor"].is_null(), "tail has no Older");

    let mut seen: Vec<u64> = Vec::new();
    for pg in [&p1, &p2, &p3] {
        for it in pg["items"].as_array().unwrap() {
            seen.push(it["number"].as_u64().unwrap());
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, vec![1, 2, 3, 4, 5]);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn live_keyset_survives_anchor_removal_and_newer_insert_but_does_not_claim_snapshot_history() {
    let root = temp_root("cursor-live-mutation");
    let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
    let be = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz)),
    );
    let viewer = human("u:viewer");
    for i in 1..=5 {
        open_pr(&be, "core", &format!("PR {i}"), &viewer);
    }
    let handler = DRepoPrList { be: be.clone() };
    let first = serve(
        &handler,
        &viewer,
        Some("core"),
        "state=open&sort=created&limit=2",
    );
    let cursor = first["page"]["next_cursor"].as_str().unwrap().to_string();

    be.prs
        .update(
            &DurableGitBackend::loc(TENANT, REGION, "core"),
            4,
            |record| {
                record.state = PrState::Closed;
                Ok(())
            },
        )
        .unwrap();
    open_pr(&be, "core", "PR 6", &viewer);
    let second = serve(&handler, &viewer, Some("core"), &format!("cursor={cursor}"));
    assert_eq!(
        second["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["number"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [3, 2]
    );
    assert_eq!(
        second["page"]["total"], 5,
        "total is the current live total"
    );

    for number in [1_u64, 2, 3, 5, 6] {
        be.prs
            .update(
                &DurableGitBackend::loc(TENANT, REGION, "core"),
                number,
                |record| {
                    record.updated_at = Some((number * 10) as i64);
                    Ok(())
                },
            )
            .unwrap();
    }
    let updated_first = serve(
        &handler,
        &viewer,
        Some("core"),
        "state=open&sort=updated&limit=2",
    );
    let updated_cursor = updated_first["page"]["next_cursor"]
        .as_str()
        .unwrap()
        .to_string();
    be.prs
        .update(
            &DurableGitBackend::loc(TENANT, REGION, "core"),
            2,
            |record| {
                record.updated_at = Some(i64::MAX - 1);
                Ok(())
            },
        )
        .unwrap();
    let repeated = serve(
        &handler,
        &viewer,
        Some("core"),
        &format!("cursor={updated_cursor}"),
    );
    assert!(repeated["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["number"] != 2));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn cursor_scope_replay_and_cross_visible_set_changes_are_typed() {
    let root = temp_root("cursor-scopes");
    let viewer = human("u:viewer");
    let authz = GrantBackedRepos::new()
        .grant_read("u:viewer", TENANT, "alpha")
        .grant_read("u:viewer", TENANT, "beta");
    let be = Arc::new(DurableGitBackend::rooted_inmem_for_test(&root));
    open_pr(&be, "alpha", "Alpha", &viewer);
    open_pr(&be, "alpha", "Alpha two", &viewer);
    open_pr(&be, "beta", "Beta", &viewer);
    let be = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz)),
    );

    let repo_handler = DRepoPrList { be: be.clone() };
    let alpha = serve(
        &repo_handler,
        &viewer,
        Some("alpha"),
        "state=all&sort=created&limit=1",
    );
    let repo_cursor = alpha["page"]["next_cursor"].as_str().unwrap();
    assert!(matches!(
        serve_result(
            &repo_handler,
            &viewer,
            Some("beta"),
            &format!("cursor={repo_cursor}")
        ),
        Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
    ));

    let cross_handler = DMyPrs { be: be.clone() };
    let first = serve(
        &cross_handler,
        &viewer,
        None,
        "bucket=yours&sort=created&limit=1",
    );
    let cross_cursor = first["page"]["next_cursor"].as_str().unwrap();
    let narrowed = DMyPrs {
        be: Arc::new(
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(
                GrantBackedRepos::new().grant_read("u:viewer", TENANT, "alpha"),
            )),
        ),
    };
    assert!(matches!(
        serve_result(&narrowed, &viewer, None, &format!("cursor={cross_cursor}")),
        Err(EdgeError::Conflict(message)) if message.contains("stale")
    ));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn pull_denial_precedes_pr_list_cursor_decoding() {
    let root = temp_root("cursor-auth-order");
    let be = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_authorizer(Arc::new(GrantBackedRepos::new())),
    );
    let guarded_handler = guarded(
        &be,
        RepoPermission::Pull,
        Arc::new(DRepoPrList { be: be.clone() }),
    );
    let error = serve_result(
        guarded_handler.as_ref(),
        &human("u:denied"),
        Some("hidden"),
        "cursor=pl1_malformed",
    )
    .unwrap_err();
    assert_eq!(error, EdgeError::NotFound("repository not found".into()));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn row_vm_title_null_and_checks_unavailable_are_honest() {
    let pr = myelin_git::lifecycle::PullRequest::open(
        9,
        "refs/heads/main",
        "refs/heads/feature",
        "psn:old@acme",
        false,
    );
    let rec = PrRecord::open(&pr, "abc");
    assert_eq!(rec.title, "");
    let enriched = EnrichedPr {
        rec,
        summary: ChecksSummary::unavailable(),
        you_requested: false,
        repo_slug: Some("core".into()),
    };
    let row = DurableGitBackend::pr_list_row_json(&enriched);
    assert!(
        row["title"].is_null(),
        "empty title → null (the #number fallback is honest)"
    );
    assert_eq!(row["number"], 9);
    assert_eq!(
        row["checks_summary"]["verdict"], "unavailable",
        "fails static, still lists"
    );
    assert_eq!(row["updated_at"], Value::Null);
}

#[test]
fn f3_clone_url_is_http_wire_shape_never_ssh() {
    let root = temp_root("f3-clone-url");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    let url = be.clone_url(TENANT, REGION, "widgets");
    assert!(
        url.ends_with("/acme/eu-west/widgets.git"),
        "the wire path grammar is /{{tenant}}/{{region}}/{{repo}}.git - got {url}"
    );
    assert!(
        !url.contains("ssh://"),
        "no ssh scheme (there is no SSH server): {url}"
    );
    assert!(!url.contains("git@myelin"), "no fabricated ssh host: {url}");

    let author = human("u:author");
    be.create_repo_as(TENANT, REGION, "widgets", &author)
        .unwrap();
    let home = be
        .repo_home_json(TENANT, REGION, "widgets")
        .expect("repository home");
    let advertised = home["clone_url"].as_str().expect("clone URL");
    assert!(
        advertised.ends_with("/acme/eu-west/widgets.git"),
        "got {advertised}"
    );
    assert!(
        !advertised.contains("ssh://"),
        "no ssh in the projection: {advertised}"
    );

    let namespaced = be.clone_url(TENANT, REGION, "team/widgets");
    assert!(
        namespaced.ends_with("/acme/eu-west/team%2Fwidgets.git"),
        "a namespaced slug stays in the wire route's repo segment: {namespaced}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn pr_number_allocation_never_resets_or_wraps() {
    assert_eq!(DurableGitBackend::next_pr_number_after(None).unwrap(), 1);
    assert_eq!(
        DurableGitBackend::next_pr_number_after(Some(41)).unwrap(),
        42
    );
    let err = DurableGitBackend::next_pr_number_after(Some(u64::MAX))
        .expect_err("an exhausted namespace must fail instead of wrapping");
    assert!(err.to_string().contains("number space exhausted"));
}

#[test]
fn f8_open_pr_resolves_head_oid_from_head_ref_tip() {
    let root = temp_root("f8-resolve-head");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    let author = human("u:author");
    be.create_repo_as(TENANT, REGION, "core", &author).unwrap();

    let loc = DurableGitBackend::loc(TENANT, REGION, "core");
    let repo = be.store.open_repo(&loc).expect("open repo");
    let blob = repo.write_blob(b"hello\n").expect("blob");
    let tree = repo.write_tree(&[("f.txt", &blob)]).expect("tree");
    let tip = repo
        .write_commit(&tree, &[], "seed", "psn@acme.noreply", "psn@acme.noreply")
        .expect("commit");
    repo.update_ref_cas(
        "refs/heads/feature",
        None,
        Some(&tip),
        "create",
        "psn@acme.noreply",
    )
    .expect("create feature ref");

    let body = json!({
        "title": "resolve my head",
        "base_ref": "refs/heads/main",
        "head_ref": "refs/heads/feature",
    });
    let rec = be
        .open_pr(TENANT, REGION, "core", &body, &author)
        .expect("open PR");
    assert_eq!(
        rec.head_oid, tip.0,
        "F8: an omitted head_oid is resolved from head_ref's current tip"
    );

    let body_bare = json!({ "title": "bare head_ref", "head_ref": "feature" });
    let rec2 = be
        .open_pr(TENANT, REGION, "core", &body_bare, &author)
        .expect("open PR");
    assert_eq!(
        rec2.head_oid, tip.0,
        "F8: a bare branch name also resolves to the tip"
    );

    let bad = json!({ "title": "ghost branch", "head_ref": "refs/heads/does-not-exist" });
    let err = be
        .open_pr(TENANT, REGION, "core", &bad, &author)
        .expect_err("must refuse");
    assert_eq!(
        map_durable_err(err).status(),
        400,
        "F8: a non-existent head_ref is a 400 at open, not a merge-time surprise"
    );
    let oversized = be.raw_response_bounded(
        TENANT,
        REGION,
        "core",
        "refs/heads/feature",
        "f.txt",
        RawResponseOptions {
            attachment: true,
            maximum_bytes: 1,
        },
    );
    assert!(matches!(oversized, Err(error) if error.status() == 413));
    assert_eq!(
        read_text_blob_at_snapshot_bounded(&repo, &tip, "f.txt", 1).unwrap(),
        None,
        "an oversized README-style preview must stop at the object header",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn blob_view_stops_at_the_object_header_for_oversized_previews() {
    let root = temp_root("blob-preview-bound");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    let author = human("u:author");
    be.create_repo_as(TENANT, REGION, "core", &author).unwrap();
    let loc = DurableGitBackend::loc(TENANT, REGION, "core");
    let repo = be.store.open_repo(&loc).expect("open repo");
    let blob = repo.write_blob(b"hello\n").expect("blob");
    let tree = repo.write_tree(&[("large.txt", &blob)]).expect("tree");
    let tip = repo
        .write_commit(&tree, &[], "seed", "psn@acme.noreply", "psn@acme.noreply")
        .expect("commit");
    repo.update_ref_cas(
        "refs/heads/main",
        None,
        Some(&tip),
        "create",
        "psn@acme.noreply",
    )
    .expect("create main");

    let metadata = be
        .blob_json_bounded(
            TENANT,
            REGION,
            "core",
            "main",
            "large.txt",
            BlobViewOptions {
                maximum_preview_bytes: 1,
                maximum_transfer_bytes: 4,
            },
        )
        .expect("metadata-only blob view");
    assert_eq!(metadata["contents"], "");
    assert_eq!(metadata["base_oid"], blob.as_str());
    assert_eq!(metadata["size_bytes"], 6);
    assert_eq!(metadata["preview_unavailable"], true);
    assert_eq!(metadata["download_available"], false);
    assert_eq!(metadata["viewer_may_edit"], false);

    let inline = be
        .blob_json_bounded(
            TENANT,
            REGION,
            "core",
            "main",
            "large.txt",
            BlobViewOptions {
                maximum_preview_bytes: 6,
                maximum_transfer_bytes: 6,
            },
        )
        .expect("inline blob view");
    assert_eq!(inline["contents"], "hello\n");
    assert_eq!(inline["preview_unavailable"], false);
    assert_eq!(inline["download_available"], true);
    assert_eq!(inline["viewer_may_edit"], false);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn oversized_interactive_reads_map_to_bounded_public_responses() {
    for (private, public) in [
        (
            "browse response limit exceeded: private repository detail",
            "repository view exceeds the interactive browse limit",
        ),
        (
            "pr diff computation limit exceeded: private repository detail",
            "pull request diff exceeds the interactive file limit",
        ),
        (
            "commit diff computation limit exceeded: private repository detail",
            "commit diff exceeds the interactive content limit",
        ),
        (
            "pull request list limit exceeded: private repository detail",
            "pull request list exceeds the interactive record limit",
        ),
        (
            "pull request record limit exceeded: private repository detail",
            "pull request record exceeds the storage limit",
        ),
        (
            "branch protection limit exceeded: private repository detail",
            "branch protection policy exceeds the storage limit",
        ),
        (
            "wire ref limit exceeded: private repository detail",
            "repository exceeds the smart-HTTP ref limit",
        ),
    ] {
        let mapped = map_durable_err(DurableError::Git(private.into()));
        assert_eq!(mapped.status(), 413);
        assert_eq!(
            mapped.to_string(),
            format!("413 (payload_too_large): {public}")
        );
    }
}
