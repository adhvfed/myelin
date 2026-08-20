use super::*;
use myelin_storage::TenantScope;
use myelin_tenancy::{Region as IdentityRegion, TenantId as IdentityTenantId};
use std::collections::BTreeMap;

fn search_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "myelin-code-search-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn principal() -> Principal {
    Principal::new(
        IdentityTenantId("acme".into()),
        IdentityRegion("eu-west".into()),
        PrincipalId("u:searcher".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn serve(be: Arc<DurableGitBackend>, query: &str) -> Result<EdgeResponse, EdgeError> {
    let principal = principal();
    let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
    let params = BTreeMap::new();
    let request = EdgeRequest::new("GET", "/v1/git/search/code", query, vec![], vec![]);
    let identity = crate::catalogue::test_request_identity(&principal, &scope);
    DCodeSearch { be }.handle(&HandlerCtx {
        identity: &identity,
        principal: &principal,
        scope: &scope,
        params: &params,
        request: &request,
    })
}

#[test]
fn code_search_decodes_form_components_without_splitting_encoded_injection() {
    assert_eq!(
        parse_code_search_query("q=two%20words%20%26%20100%25%20%3D%20na%C3%AFve&repo=team%2Fcore")
            .unwrap(),
        ("two words & 100% = naïve".into(), Some("team/core".into()))
    );
    assert_eq!(
        parse_code_search_query("q=x%26limit%3D100").unwrap(),
        ("x&limit=100".into(), None),
        "an encoded ampersand stays inside q rather than becoming a limit parameter"
    );
    assert!(parse_code_search_query("q=x&limit=100").is_err());

    let maximum = format!(
        "q={}",
        "x".repeat(myelin_git::api::CODE_SEARCH_QUERY_MAX_BYTES)
    );
    assert!(parse_code_search_query(&maximum).is_ok());
}

#[test]
fn code_search_rejects_every_malformed_or_unbounded_coordinate() {
    for query in [
        "",
        "q",
        "=x",
        "repo=core",
        "q=",
        "q=+++",
        "q=x&q=y",
        "q=x&%71=y",
        "q=x&repo=core&repo=other",
        "q=x&unknown=value",
        "q=x&limit=100",
        "q=x&cursor=opaque",
        "q=%",
        "q=%0",
        "q=%GG",
        "q=%FF",
        "q=%00",
        "q=x&repo=",
        "q=x&repo=..%2Fsecret",
        "q=x&repo=team%2F%2Fcore",
        "q=x&repo=team%5Ccore",
    ] {
        assert!(
            matches!(
                parse_code_search_query(query),
                Err(EdgeError::BadRequest(_))
            ),
            "malformed code-search query should be a 400: {query:?}"
        );
    }

    let oversized_search = format!(
        "q={}",
        "x".repeat(myelin_git::api::CODE_SEARCH_QUERY_MAX_BYTES + 1)
    );
    assert!(parse_code_search_query(&oversized_search).is_err());
    let oversized_repo = format!(
        "q=x&repo={}",
        "r".repeat(myelin_git::api::CODE_SEARCH_REPO_MAX_BYTES + 1)
    );
    assert!(parse_code_search_query(&oversized_repo).is_err());
    let oversized_raw = format!("q=x&{}", "a".repeat(CODE_SEARCH_MAX_RAW_QUERY_BYTES));
    assert!(parse_code_search_query(&oversized_raw).is_err());
}

#[test]
fn code_search_reads_the_authorized_default_branch_without_repo_probing() {
    let root = search_root();
    let be = Arc::new(DurableGitBackend::rooted_inmem_for_test(&root));
    let principal = principal();
    be.create_repo_as("acme", "eu-west", "core", &principal)
        .unwrap();
    let repo = be
        .store
        .open_repo(&DurableGitBackend::loc("acme", "eu-west", "core"))
        .unwrap();
    let blob = repo.write_blob(b"first line\nneedle in code\n").unwrap();
    let tree = repo.write_tree(&[("app.rs", &blob)]).unwrap();
    let commit = repo
        .write_commit(&tree, &[], "seed", "searcher", "searcher")
        .unwrap();
    repo.update_ref_cas("refs/heads/main", None, Some(&commit), "seed", "searcher")
        .unwrap();

    let response = serve(be.clone(), "repo=core&q=needle").unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json_body().unwrap()["items"],
        json!([{
            "repo": "core",
            "ref": "refs/heads/main",
            "snapshot_oid": commit.as_str(),
            "path": "app.rs",
            "line": 2,
            "excerpt": "needle in code",
        }])
    );

    let missing = serve(be, "repo=missing-but-valid&q=needle").unwrap();
    assert_eq!(missing.status(), 200);
    assert_eq!(missing.json_body().unwrap()["items"], json!([]));
    std::fs::remove_dir_all(root).ok();
}
