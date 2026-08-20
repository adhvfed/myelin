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
        "myelin-repo-summary-{tag}-{}-{nanos}",
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
    viewer: &Principal,
    query: &str,
) -> Result<EdgeResponse, EdgeError> {
    let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
    let params = BTreeMap::new();
    let request = EdgeRequest::new("GET", "/v1/git/repos", query, vec![], vec![]);
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

fn create_repo(be: &DurableGitBackend, slug: &str, creator: &Principal) {
    be.create_repo_as(TENANT, REGION, slug, creator)
        .expect("create repository");
}

fn add_non_commit_main_target(be: &DurableGitBackend, slug: &str) {
    let loc = DurableGitBackend::loc(TENANT, REGION, slug);
    let repo = be.store.open_repo(&loc).expect("open repository");
    let tree = repo.write_tree(&[]).expect("write bare tree object");
    repo.update_ref_cas(
        "refs/heads/main",
        None,
        Some(&tree),
        "create deliberately non-commit main target",
        "psn@tenant.noreply",
    )
    .expect("create direct branch target");
}

#[test]
fn list_rows_have_exact_shapes_without_materializing_repository_homes() {
    let root = temp_root("exact-rows");
    let viewer = human("u:viewer");
    let authz = GrantBackedRepos::new()
        .grant_read("u:viewer", TENANT, "empty")
        .grant_read("u:viewer", TENANT, "populated");
    let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
    create_repo(&be, "empty", &viewer);
    create_repo(&be, "populated", &viewer);
    add_non_commit_main_target(&be, "populated");
    let handler = DRepoList { be: Arc::new(be) };

    let body = json(serve(&handler, &viewer, "").expect("repository list response"));
    assert_eq!(
        body,
        json!({
            "items": [
                {
                    "state": "populated",
                    "slug": "acme/populated",
                    "clone_url": "/acme/eu-west/populated.git",
                },
                { "state": "empty", "slug": "acme/empty" },
            ],
            "page": { "next_cursor": null, "limit": DEFAULT_PAGE_LIMIT },
        })
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn newest_first_paging_authorizes_before_slicing_without_gaps_or_duplicates() {
    let root = temp_root("auth-before-page");
    let viewer = human("u:viewer");
    let authz = GrantBackedRepos::new()
        .grant_read("u:viewer", TENANT, "alpha")
        .grant_read("u:viewer", TENANT, "gamma")
        .grant_read("u:viewer", TENANT, "omega");
    let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
    for slug in ["alpha", "beta", "gamma", "omega"] {
        create_repo(&be, slug, &viewer);
    }
    let handler = DRepoList { be: Arc::new(be) };

    let mut query = "limit=1".to_string();
    let mut seen = Vec::new();
    loop {
        let body = json(serve(&handler, &viewer, &query).expect("repository list page"));
        let items = body["items"].as_array().expect("items");
        seen.extend(
            items
                .iter()
                .map(|item| item["slug"].as_str().expect("slug").to_string()),
        );
        let Some(cursor) = body["page"]["next_cursor"].as_str() else {
            break;
        };
        assert!(cursor.starts_with(REPO_LIST_CURSOR_PREFIX));
        query = format!("limit=1&cursor={cursor}");
    }
    assert_eq!(seen, ["acme/omega", "acme/gamma", "acme/alpha"]);
    assert!(!seen.iter().any(|slug| slug == "acme/beta"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn repository_list_query_is_strict_and_limits_are_canonical_and_bounded() {
    assert_eq!(parse_repo_list_query("").unwrap().limit, DEFAULT_PAGE_LIMIT);
    assert_eq!(parse_repo_list_query("%6cimit=100").unwrap().limit, 100);
    for query in [
        "limit",
        "view=summary",
        "limit=1&limit=1",
        "unknown=x",
        "unknown=%GG",
        "limit=100%0A",
        "limit=0",
        "limit=01",
        "limit=101",
        "cursor=x",
    ] {
        assert!(
            matches!(parse_repo_list_query(query), Err(EdgeError::BadRequest(_))),
            "query should be rejected: {query}"
        );
    }
    assert!(matches!(
        parse_repo_list_query(&format!(
            "cursor=rl2_{}",
            "a".repeat(REPO_LIST_CURSOR_MAX_BYTES)
        )),
        Err(EdgeError::BadRequest(_))
    ));
}

#[test]
fn cursor_is_canonical_bounded_and_scoped_to_verified_tenant_region() {
    let cursor = RepoListCursor::legacy(repo_list_cursor_scope(TENANT, REGION), "alpha")
        .unwrap()
        .encode();
    let parsed = parse_repo_list_cursor(&cursor, TENANT, REGION).expect("canonical cursor");
    assert_eq!(parsed.last_slug(), "alpha");
    for malformed in [
        "rl2_".to_string(),
        "rl2_not-base64!".to_string(),
        format!("{cursor}="),
    ] {
        assert!(matches!(
            parse_repo_list_cursor(&malformed, TENANT, REGION),
            Err(EdgeError::BadRequest(_))
        ));
    }
    assert!(matches!(
        parse_repo_list_cursor(&cursor, "other-tenant", REGION),
        Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
    ));
    assert!(matches!(
        parse_repo_list_cursor(&cursor, TENANT, "other-region"),
        Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
    ));

    let root = temp_root("empty-scope");
    let viewer = human("u:viewer");
    let handler = DRepoList {
        be: Arc::new(DurableGitBackend::rooted_inmem_for_test(&root)),
    };
    let empty = json(serve(&handler, &viewer, "").expect("empty repository list"));
    assert_eq!(empty["items"], json!([]));
    let wrong_scope =
        RepoListCursor::legacy(repo_list_cursor_scope("other-tenant", REGION), "alpha")
            .unwrap()
            .encode();
    assert!(matches!(
        serve(
            &handler,
            &viewer,
            &format!("cursor={wrong_scope}")
        ),
        Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
    ));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn repository_list_response_and_candidate_cardinality_are_bounded() {
    let small = page_envelope(json!([]), None, DEFAULT_PAGE_LIMIT);
    assert_eq!(repo_list_response(&small).unwrap().status(), 200);
    assert!(matches!(
        repo_list_response(&json!({
            "items": ["x".repeat(REPO_LIST_RESPONSE_MAX_BYTES)],
            "page": { "next_cursor": null, "limit": 1 },
        })),
        Err(EdgeError::PayloadTooLarge(_))
    ));

    let root = temp_root("candidate-cap");
    let be = DurableGitBackend::rooted_inmem_for_test(&root);
    let viewer = human("u:viewer");
    for slug in ["alpha", "beta"] {
        create_repo(&be, slug, &viewer);
    }
    assert!(matches!(
        be.scan_repo_slugs_bounded(TENANT, REGION, 1),
        Err(DurableError::Git(message))
            if message == "browse response limit exceeded: repository candidate count"
    ));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn repository_list_capacity_errors_use_catalogue_specific_sanitized_text() {
    let mapped = map_repo_list_durable_err(DurableError::Git(
        "wire ref limit exceeded: private branch detail".into(),
    ));
    assert_eq!(mapped.status(), 413);
    assert_eq!(
        mapped.to_string(),
        "413 (payload_too_large): repository catalogue exceeds the interactive list limit"
    );

    let delegated = map_repo_list_durable_err(DurableError::NotFound("missing".into()));
    assert_eq!(
        delegated,
        map_durable_err(DurableError::NotFound("missing".into())),
        "non-capacity errors retain the shared durable mapping"
    );
    assert_eq!(
        map_durable_err(DurableError::Git(
            "wire ref limit exceeded: private wire detail".into()
        ))
        .to_string(),
        "413 (payload_too_large): repository exceeds the smart-HTTP ref limit",
        "actual wire callers retain the established sanitized message"
    );
}
