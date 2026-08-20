use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region as IdRegion, TenantId};

use super::*;
use crate::catalogue::test_request_identity;
use crate::repo_authz::GrantBackedRepos;
use crate::request::EdgeRequest;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const TENANT: &str = "tree-page-tenant";
const REGION: &str = "eu-north";

struct Fixture {
    root: PathBuf,
    be: DurableGitBackend,
    repo: DurableGitRepo,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "myelin-edge-tree-page-{label}-{}-{sequence}",
            std::process::id()
        ));
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let loc = DurableGitBackend::loc(TENANT, REGION, label);
        let repo = be.store.create_repo(&loc).expect("create repo");
        Self { root, be, repo }
    }

    fn commit_shared_files(
        &self,
        count: usize,
        parents: &[&CoreOid],
        message: &str,
    ) -> (CoreOid, CoreOid) {
        let blob = self.repo.write_blob(b"page\n").expect("blob");
        let names = (0..count)
            .map(|index| format!("file-{index:04}.txt"))
            .collect::<Vec<_>>();
        let entries = names
            .iter()
            .map(|name| (name.as_str(), &blob))
            .collect::<Vec<_>>();
        let tree = self.repo.write_tree(&entries).expect("tree");
        let commit = self
            .repo
            .write_commit(
                &tree,
                parents,
                message,
                "psn@tenant.noreply",
                "psn@tenant.noreply",
            )
            .expect("commit");
        (tree, commit)
    }

    fn commit_named_files(
        &self,
        files: &[(&str, &[u8])],
        parents: &[&CoreOid],
        message: &str,
    ) -> (CoreOid, CoreOid) {
        let blobs = files
            .iter()
            .map(|(name, bytes)| ((*name).to_string(), self.repo.write_blob(bytes).unwrap()))
            .collect::<Vec<_>>();
        let entries = blobs
            .iter()
            .map(|(name, oid)| (name.as_str(), oid))
            .collect::<Vec<_>>();
        let tree = self.repo.write_tree(&entries).expect("tree");
        let commit = self
            .repo
            .write_commit(
                &tree,
                parents,
                message,
                "psn@tenant.noreply",
                "psn@tenant.noreply",
            )
            .expect("commit");
        (tree, commit)
    }

    fn create_main(&self, commit: &CoreOid) {
        self.repo
            .update_ref_cas(
                "refs/heads/main",
                None,
                Some(commit),
                "create main",
                "psn@tenant.noreply",
            )
            .expect("create main");
    }

    fn move_main(&self, old: &CoreOid, new: &CoreOid) {
        self.repo
            .update_ref_cas(
                "refs/heads/main",
                Some(old),
                Some(new),
                "move main",
                "psn@tenant.noreply",
            )
            .expect("move main");
    }

    fn tree_json(&self, request: TreePageRequest) -> Result<Value, TreePageError> {
        self.be
            .tree_json(TENANT, REGION, self.label(), "refs/heads/main", "", request)
    }

    fn label(&self) -> &str {
        self.repo
            .path()
            .file_stem()
            .and_then(|name| name.to_str())
            .expect("repo slug")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

fn oid_bytes(oid: &CoreOid) -> [u8; 20] {
    let mut bytes = [0_u8; 20];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&oid.as_str()[index * 2..index * 2 + 2], 16).expect("hex oid");
    }
    bytes
}

fn forge_cursor_oids(cursor: &str, snapshot: &CoreOid, tree: &CoreOid) -> String {
    let encoded = cursor.strip_prefix("gt1_").expect("tree cursor prefix");
    let mut frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .expect("tree cursor frame");
    frame[1..21].copy_from_slice(&oid_bytes(snapshot));
    frame[21..41].copy_from_slice(&oid_bytes(tree));
    format!(
        "gt1_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
    )
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

fn serve_tree(
    handler: &dyn Handler,
    viewer: &Principal,
    slug: &str,
    query: &str,
) -> Result<EdgeResponse, EdgeError> {
    let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
    let params = BTreeMap::from([
        ("repo".to_string(), slug.to_string()),
        ("ref".to_string(), "refs/heads/main".to_string()),
        ("path".to_string(), String::new()),
    ]);
    let request = EdgeRequest::new(
        "GET",
        "/v1/git/repos/core/tree/refs%2Fheads%2Fmain/",
        query,
        vec![],
        vec![],
    );
    let identity = test_request_identity(viewer, &scope);
    handler.handle(&HandlerCtx {
        identity: &identity,
        principal: viewer,
        scope: &scope,
        params: &params,
        request: &request,
    })
}

#[test]
fn tree_pull_guard_denies_before_malformed_query_or_cursor_parsing() {
    let fixture = Fixture::new("guard-order");
    let tree = fixture.repo.write_tree(&[]).expect("empty tree");
    let commit = fixture
        .repo
        .write_commit(
            &tree,
            &[],
            "guarded tree",
            "psn@tenant.noreply",
            "psn@tenant.noreply",
        )
        .expect("commit");
    fixture.create_main(&commit);

    let reader = human("u:reader");
    let stranger = human("u:stranger");
    let authorizer = GrantBackedRepos::new().grant_read("u:reader", TENANT, fixture.label());
    let be = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(&fixture.root)
            .with_repo_authorizer(Arc::new(authorizer)),
    );
    let handler = guarded(
        &be,
        RepoPermission::Pull,
        Arc::new(DTree { be: be.clone() }),
    );

    for query in ["cursor=%", "cursor=not-a-tree-cursor"] {
        let denied = match serve_tree(&*handler, &stranger, fixture.label(), query) {
            Err(error) => error,
            Ok(_) => panic!("the pull guard must deny before DTree parses the query"),
        };
        assert!(
            matches!(denied, EdgeError::NotFound(_)) && denied.status() == 404,
            "ungranted malformed tree request must be 0-leak 404: {denied:?}"
        );

        let admitted = match serve_tree(&*handler, &reader, fixture.label(), query) {
            Err(error) => error,
            Ok(_) => panic!("a granted reader must reach DTree's strict parser"),
        };
        assert!(
            matches!(admitted, EdgeError::BadRequest(_)) && admitted.status() == 400,
            "granted malformed tree request must reach DTree and return 400: {admitted:?}"
        );
    }
}

#[test]
fn repo_home_pages_more_than_one_thousand_rows_with_its_qualified_continuation_ref() {
    let fixture = Fixture::new("wide");
    let (_, commit) = fixture.commit_shared_files(1_001, &[], "wide root");
    fixture.create_main(&commit);

    let home = fixture
        .be
        .repo_home_json(TENANT, REGION, fixture.label())
        .expect("repo home");
    assert_eq!(home["state"], "populated");
    assert_eq!(home["ref"], format!("myelin://{TENANT}/git/repo/wide"));
    assert_eq!(home["entries"].as_array().unwrap().len(), 100);
    assert_eq!(home["entries_page"]["limit"], 100);
    assert_eq!(home["entries_page"]["ref"], "refs/heads/main");
    assert_eq!(home["entries_page"]["snapshot_oid"], commit.as_str());
    let mut names = home["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let mut cursor = home["entries_page"]["next_cursor"]
        .as_str()
        .map(str::to_string);
    while let Some(next) = cursor {
        let page = fixture
            .tree_json(TreePageRequest {
                limit: 100,
                cursor: Some(next),
                ..TreePageRequest::default()
            })
            .expect("qualified continuation");
        names.extend(
            page["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["name"].as_str().unwrap().to_string()),
        );
        cursor = page["page"]["next_cursor"].as_str().map(str::to_string);
    }
    assert_eq!(names.len(), 1_001);
    assert_eq!(names.first().unwrap(), "file-0000.txt");
    assert_eq!(names.last().unwrap(), "file-1000.txt");
}

#[test]
fn unborn_repo_home_exposes_the_same_canonical_identity_it_will_keep_after_first_push() {
    let fixture = Fixture::new("unborn");
    let home = fixture
        .be
        .repo_home_json(TENANT, REGION, fixture.label())
        .expect("empty repo home");

    assert_eq!(home["state"], "empty");
    assert_eq!(home["ref"], format!("myelin://{TENANT}/git/repo/unborn"));
}

#[test]
fn branch_movement_is_a_typed_stale_tree_cursor() {
    let fixture = Fixture::new("stale");
    let (_, first) = fixture.commit_shared_files(3, &[], "first");
    fixture.create_main(&first);
    let first_page = fixture
        .tree_json(TreePageRequest {
            limit: 1,
            ..TreePageRequest::default()
        })
        .expect("first page");
    let cursor = first_page["page"]["next_cursor"]
        .as_str()
        .expect("cursor")
        .to_string();
    let (_, second) = fixture.commit_shared_files(4, &[&first], "second");
    fixture.move_main(&first, &second);

    let error = fixture
        .tree_json(TreePageRequest {
            limit: 1,
            cursor: Some(cursor),
            ..TreePageRequest::default()
        })
        .expect_err("moved branch must stale");
    assert_eq!(error, TreePageError::CursorStale);
    assert_eq!(map_tree_page_err(error).status(), 409);
}

#[test]
fn forged_cursor_oids_cannot_select_unreachable_tree_objects() {
    let fixture = Fixture::new("forged");
    let (_, visible) = fixture.commit_shared_files(3, &[], "visible");
    fixture.create_main(&visible);
    let first_page = fixture
        .tree_json(TreePageRequest {
            limit: 1,
            ..TreePageRequest::default()
        })
        .expect("first page");
    let cursor = first_page["page"]["next_cursor"].as_str().unwrap();
    let (secret_tree, secret_commit) = fixture.commit_named_files(
        &[("secret.txt", b"unreachable secret\n")],
        &[],
        "unreachable",
    );
    let forged = forge_cursor_oids(cursor, &secret_commit, &secret_tree);

    let error = fixture
        .tree_json(TreePageRequest {
            limit: 1,
            cursor: Some(forged),
            ..TreePageRequest::default()
        })
        .expect_err("forged object ids are consistency-only");
    assert_eq!(error, TreePageError::CursorStale);
}

#[test]
fn readme_is_present_only_on_the_first_unfiltered_tree_page() {
    let fixture = Fixture::new("readme");
    let (_, commit) = fixture.commit_named_files(
        &[("README.md", b"# snapshot readme\n"), ("z.txt", b"z\n")],
        &[],
        "readme",
    );
    fixture.create_main(&commit);
    let first = fixture
        .tree_json(TreePageRequest {
            limit: 1,
            ..TreePageRequest::default()
        })
        .expect("first page");
    assert_eq!(first["readme"], "# snapshot readme\n");
    let cursor = first["page"]["next_cursor"].as_str().unwrap().to_string();
    let continuation = fixture
        .tree_json(TreePageRequest {
            limit: 1,
            cursor: Some(cursor),
            ..TreePageRequest::default()
        })
        .expect("continuation");
    assert!(continuation.get("readme").is_none());
    let search = fixture
        .tree_json(TreePageRequest {
            query: Some("readme".into()),
            ..TreePageRequest::default()
        })
        .expect("search");
    assert!(search.get("readme").is_none());
}

#[test]
fn committed_empty_tree_is_populated_and_exposes_a_terminal_entries_page() {
    let fixture = Fixture::new("empty-tree");
    let tree = fixture.repo.write_tree(&[]).expect("empty tree");
    let commit = fixture
        .repo
        .write_commit(
            &tree,
            &[],
            "empty snapshot",
            "psn@tenant.noreply",
            "psn@tenant.noreply",
        )
        .expect("empty commit");
    fixture.create_main(&commit);

    let home = fixture
        .be
        .repo_home_json(TENANT, REGION, fixture.label())
        .expect("repo home");
    assert_eq!(home["state"], "populated");
    assert_eq!(home["entries"], json!([]));
    assert!(home["entries_page"]["next_cursor"].is_null());
}

#[test]
fn repo_and_tree_entry_metadata_share_the_selected_snapshot() {
    let fixture = Fixture::new("snapshot-meta");
    let (_, first) = fixture.commit_named_files(&[("file.txt", b"first\n")], &[], "first");
    fixture.create_main(&first);
    let (_, second) = fixture.commit_named_files(&[("file.txt", b"second\n")], &[&first], "second");
    fixture.move_main(&first, &second);

    let tree = fixture
        .tree_json(TreePageRequest::default())
        .expect("tree page");
    assert_eq!(tree["snapshot_oid"], second.as_str());
    assert_eq!(tree["entries"][0]["latest_commit"]["oid"], second.as_str());
    let home = fixture
        .be
        .repo_home_json(TENANT, REGION, fixture.label())
        .expect("repo home");
    assert_eq!(home["snapshot_oid"], second.as_str());
    assert_eq!(home["entries_page"]["snapshot_oid"], second.as_str());
    assert_eq!(home["latest_commit"]["oid"], second.as_str());
    assert_eq!(home["entries"][0]["latest_commit"]["oid"], second.as_str());
}
