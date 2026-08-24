use std::sync::Arc;

use myelin_content::InlineNode;
use myelin_events::ArtifactRef;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::{strip_sub, sub_kind};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

use super::*;
use crate::backlinks::{AuthzVisibleIndex, BacklinkRead};
use crate::edge_builder::{edge_id, EdgeProjection, EdgeRow, RelClass};
use crate::ladder::{resolve_sub_outcome, MintedLineRange};
use crate::resolve::{bounded_stale, ProjectOutcome, ResolveMode, ResolveService, TombstoneReason};

fn tenant() -> TenantId {
    TenantId("acme-eu".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn cell() -> CellId {
    CellId::from_token("cell-fr-par-1")
}
fn viewer(id: &str, t: &TenantId) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, t.clone())
}
fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    }
}
fn authz() -> Arc<FailStaticAuthz> {
    Arc::new(FailStaticAuthz::try_new(300, &threshold()).expect("valid bound"))
}

#[test]
fn real_git_pr_body_extracts_one_edge_per_structured_ref_node() {
    let producer = GitEdgeProducer;
    let source = GitEdgeProducer::pr_root("acme-eu", "repo7", 4291);

    let body = vec![
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme-eu/issue/issue/ENG-12".into())),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme-eu/git/pr/repo7:4288".into())),
        InlineNode::Mention(viewer("alice", &tenant())),
    ];
    let edges = producer.git_edges(&source, &body);
    assert_eq!(edges.len(), 3, "three structured ref nodes → three edges");
    for e in &edges {
        assert_eq!(e.source, source);
        assert_eq!(e.rel_class, RelClass::Reference);
    }
    assert_eq!(edges[0].target.0, "myelin://acme-eu/issue/issue/ENG-12");
    assert_eq!(edges[0].rel.as_str(), "links");
    assert_eq!(edges[2].target.0, "myelin://acme-eu/identity/member/alice");
    assert_eq!(edges[2].rel.as_str(), "mentions");
}

#[test]
fn commit_trailer_reference_sources_from_the_commit_root() {
    let producer = GitEdgeProducer;
    let source = GitEdgeProducer::commit_root("acme-eu", "repo7", "abc123def");
    let body = vec![InlineNode::ArtifactRefNode(ArtifactRef(
        "myelin://acme-eu/issue/issue/ENG-7".into(),
    ))];
    let edges = producer.git_edges(&source, &body);
    assert_eq!(edges.len(), 1);
    assert_eq!(
        edges[0].source.0,
        "myelin://acme-eu/git/commit/repo7:abc123def"
    );
    assert_eq!(edges[0].target.0, "myelin://acme-eu/issue/issue/ENG-7");
}

#[test]
fn force_pushed_line_range_resolves_moved_with_shifted_anchor() {
    let owner = GitOwner::new();
    let ref_ = myelin_git::subs::mint_blob_line_range(
        "acme-eu",
        "repo7",
        "refs/heads/main",
        "src/lib.rs",
        42,
        44,
    )
    .expect("grammatical line-range mint");

    let original = [
        "// header",
        "use std::fmt;",
        "",
        "fn answer() -> u32 {",
        "    let x = 40;",
        "    x + 2",
    ];
    let mut padded: Vec<&str> = (0..41).map(|_| "// pad").collect();
    padded.extend_from_slice(&original[3..6]);
    let minted = MintedLineRange::mint("oid-original", &padded, 42, 44);

    let mut rewritten: Vec<&str> = vec!["// new banner", "// added", "// added 2"];
    rewritten.extend_from_slice(&padded);
    owner.record_line_range(&ref_, minted, "oid-forcepushed", &rewritten);

    let outcome = resolve_sub_outcome(&owner, &ref_);
    match outcome {
        ProjectOutcome::Live(p) => {
            assert_eq!(
                p.flag,
                Some(crate::resolve::ProjectionFlag::Moved),
                "a rebased range is MOVED"
            );
            assert_eq!(
                p.sub_anchor.as_deref().unwrap(),
                "myelin://acme-eu/git/blob/repo7:refs%2Fheads%2Fmain:src%2Flib%2Ers#L45-L47"
            );
        }
        other => panic!("expected MOVED Live, got {other:?}"),
    }
}

#[test]
fn partially_edited_line_range_resolves_outdated() {
    let owner = GitOwner::new();
    let ref_ =
        myelin_git::subs::mint_blob_line_range("acme-eu", "repo7", "refs/heads/main", "f.rs", 1, 3)
            .expect("grammatical");
    let original = vec!["line A", "line B", "line C"];
    let minted = MintedLineRange::mint("oid-1", &original, 1, 3);
    let edited = vec!["line A", "line B", "line C REWRITTEN", "tail"];
    owner.record_line_range(&ref_, minted, "oid-2", &edited);

    let outcome = resolve_sub_outcome(&owner, &ref_);
    match outcome {
        ProjectOutcome::Live(p) => {
            assert_eq!(p.flag, Some(crate::resolve::ProjectionFlag::Outdated));
            assert_eq!(
                p.sub_anchor.as_deref().unwrap(),
                "myelin://acme-eu/git/blob/repo7:refs%2Fheads%2Fmain:f%2Ers#L1-L2"
            );
        }
        other => panic!("expected OUTDATED Live, got {other:?}"),
    }
}

#[test]
fn content_gone_line_range_tombstones_carrying_the_root() {
    let owner = GitOwner::new();
    let ref_ =
        myelin_git::subs::mint_blob_line_range("acme-eu", "repo7", "refs/heads/main", "g.rs", 1, 2)
            .expect("grammatical");
    let original = vec!["secret line 1", "secret line 2"];
    let minted = MintedLineRange::mint("oid-1", &original, 1, 2);
    let rewritten = vec!["// file is now empty-ish", "// nothing here"];
    owner.record_line_range(&ref_, minted, "oid-2", &rewritten);

    let svc = git_resolve_service(&owner);
    let v = viewer("insider", &tenant());
    owner.grant_view(&tenant(), &region(), &v, &strip_sub(&ref_));
    let res = svc.resolve(
        &tenant(),
        &region(),
        &ref_,
        &strip_sub(&ref_),
        &v,
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(res.is_tombstone(), "content_gone → tombstone");
    assert_eq!(res.tombstone_reason(), Some(TombstoneReason::SubGone));
    if let crate::resolve::Resolution::Tombstone(t) = res {
        assert_eq!(t.root, strip_sub(&ref_));
        assert_eq!(
            t.root.0,
            "myelin://acme-eu/git/blob/repo7:refs%2Fheads%2Fmain:g%2Ers"
        );
    }
}

#[test]
fn exact_line_range_resolves_live() {
    let owner = GitOwner::new();
    let ref_ =
        myelin_git::subs::mint_blob_line_range("acme-eu", "repo7", "refs/heads/main", "h.rs", 1, 1)
            .expect("grammatical");
    let lines = vec!["fn main() {}"];
    let minted = MintedLineRange::mint("oid-X", &lines, 1, 1);
    owner.record_line_range(&ref_, minted, "oid-X", &lines);
    match resolve_sub_outcome(&owner, &ref_) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, None, "an exact range is a clean LIVE"),
        other => panic!("expected LIVE, got {other:?}"),
    }
}

#[test]
fn git_comment_and_thread_anchors_degrade_through_the_ladder() {
    let owner = GitOwner::new();
    let comment =
        myelin_git::subs::mint_pr_comment("acme-eu", "repo7", 4291, "cAbc").expect("grammatical");
    let thread =
        myelin_git::subs::mint_pr_thread("acme-eu", "repo7", 4291, "tXyz").expect("grammatical");
    let deleted =
        myelin_git::subs::mint_pr_comment("acme-eu", "repo7", 4291, "cDel").expect("grammatical");

    owner.record_comment(&comment, CommentState::Moved);
    owner.record_comment(&thread, CommentState::Resolved);
    owner.record_comment(&deleted, CommentState::Gone);

    match resolve_sub_outcome(&owner, &comment) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, Some(crate::resolve::ProjectionFlag::Moved)),
        other => panic!("comment moved → MOVED Live, got {other:?}"),
    }
    match resolve_sub_outcome(&owner, &thread) {
        ProjectOutcome::Live(p) => {
            assert_eq!(p.flag, Some(crate::resolve::ProjectionFlag::Outdated))
        }
        other => panic!("thread resolved → OUTDATED, got {other:?}"),
    }
    assert_eq!(
        resolve_sub_outcome(&owner, &deleted),
        ProjectOutcome::SubGone
    );
}

#[test]
fn ci_check_and_step_sub_anchors_resolve_through_the_same_ladder() {
    use myelin_refs::{mint, Sub};
    let owner = GitOwner::new();
    let commit_root = ArtifactRef("myelin://acme-eu/git/commit/repo7:abc".into());
    let check = mint(&commit_root, Sub::Check("ci/build".into())).expect("check- mint");
    let step = mint(&commit_root, Sub::Step(7)).expect("step- mint");

    owner.record_check(&check, CommentState::Live);
    owner.record_check(&step, CommentState::Gone);

    assert!(matches!(
        resolve_sub_outcome(&owner, &check),
        ProjectOutcome::Live(_)
    ));
    assert_eq!(resolve_sub_outcome(&owner, &step), ProjectOutcome::SubGone);
    assert!(sub_kind(&check).is_some());
    assert!(sub_kind(&step).is_some());
}

#[test]
fn ref_d1_denied_viewer_of_a_git_pr_is_tombstoned_never_leaked() {
    let owner = GitOwner::new();
    let pr = GitEdgeProducer::pr_root("acme-eu", "secret-repo", 1);
    let outsider = viewer("outsider", &tenant());
    let svc = git_resolve_service(&owner);
    let res = svc.resolve(
        &tenant(),
        &region(),
        &pr,
        &pr,
        &outsider,
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(res.is_tombstone(), "a denied viewer is tombstoned");
    assert_eq!(res.tombstone_reason(), Some(TombstoneReason::Denied));
    if let crate::resolve::Resolution::Tombstone(t) = res {
        assert_eq!(t.root, pr);
    }
}

#[test]
fn ref_d2_cross_tenant_git_backlink_read_returns_nothing() {
    use myelin_identity::ListObjectsResult;

    let tenant_a = TenantId("tenantA".into());
    let edges = EdgeProjection::new();
    let pr = "myelin://tenantA/git/pr/repo:9";
    let issue = "myelin://tenantA/issue/issue/ENG-1";
    let id = edge_id(&tenant_a, pr, issue, "links");
    edges.upsert(
        &tenant_a,
        &region(),
        EdgeRow {
            edge_id: id.clone(),
            source: ArtifactRef(pr.into()),
            source_root: strip_sub(&ArtifactRef(pr.into())),
            target: ArtifactRef(issue.into()),
            target_root: strip_sub(&ArtifactRef(issue.into())),
            rel: "links".into(),
            rel_class: RelClass::Reference,
            origin_event: format!("evt-{id}"),
            origin_actor: "git-pseudonym-1".into(),
            zookie: Some("zk-1".into()),
            tombstoned: false,
        },
    );

    let read = BacklinkRead::new(edges, AuthzVisibleIndex::new());
    let tenant_b = TenantId("tenantB".into());
    let viewer_b = viewer("attacker", &tenant_b);
    let page = read
        .backlinks(
            &tenant_b,
            &region(),
            &ArtifactRef(issue.into()),
            &viewer_b,
            &ListObjectsResult::Filter {
                set_expr: myelin_identity::SetExpr::All,
                zookie: myelin_identity::Zookie("zk-1".into()),
            },
            &bounded_stale(),
            10,
        )
        .expect("backlink read");
    assert_eq!(
        page.edges.len(),
        0,
        "no cross-tenant Git backlink is visible (REF-D2)"
    );
}

#[test]
fn git_fragment_flows_through_list_objects_leak_free() {
    use myelin_identity::ListObjectsResult;

    let t = tenant();
    let edges = EdgeProjection::new();
    let issue = "myelin://acme-eu/issue/issue/ENG-1";
    for pr in [
        "myelin://acme-eu/git/pr/repo:public",
        "myelin://acme-eu/git/pr/repo:secret",
    ] {
        let id = edge_id(&t, pr, issue, "links");
        edges.upsert(
            &t,
            &region(),
            EdgeRow {
                edge_id: id.clone(),
                source: ArtifactRef(pr.into()),
                source_root: strip_sub(&ArtifactRef(pr.into())),
                target: ArtifactRef(issue.into()),
                target_root: strip_sub(&ArtifactRef(issue.into())),
                rel: "links".into(),
                rel_class: RelClass::Reference,
                origin_event: format!("evt-{id}"),
                origin_actor: "git-pseudonym".into(),
                zookie: Some("zk-1".into()),
                tombstoned: false,
            },
        );
    }
    let read = BacklinkRead::new(edges, AuthzVisibleIndex::new());
    let v = viewer("dev", &t);
    let allowed = ListObjectsResult::Ids {
        ids: vec![myelin_identity::ObjectId(
            "myelin://acme-eu/git/pr/repo:public".into(),
        )],
        zookie: myelin_identity::Zookie("zk-1".into()),
    };
    let page = read
        .backlinks(
            &t,
            &region(),
            &ArtifactRef(issue.into()),
            &v,
            &allowed,
            &bounded_stale(),
            10,
        )
        .expect("backlink read");
    assert_eq!(
        page.edges.len(),
        1,
        "only the repo->pull-admitted PR is visible"
    );
    assert_eq!(
        page.edges[0].source.0,
        "myelin://acme-eu/git/pr/repo:public"
    );
}

#[test]
fn git_replay_scope_is_sub_artifact_granular() {
    assert_eq!(
        git_replay_scope(GitReplayGrain::Repo("core".into())),
        "repo:core"
    );
    assert_eq!(
        git_replay_scope(GitReplayGrain::Blob {
            repo: "core".into(),
            oid: "abc".into()
        }),
        "blob:core/abc"
    );
    assert_eq!(
        git_replay_scope(GitReplayGrain::Pr {
            repo: "core".into(),
            number: 42
        }),
        "pr:core/42"
    );
    assert_eq!(GIT_OWNER_TOKEN, "git");
}

#[test]
fn ref_d4_git_edge_reindex_byte_parity() {
    let t = tenant();
    let corpus = [
        (
            "myelin://acme-eu/git/pr/repo:9",
            "myelin://acme-eu/issue/issue/ENG-1",
            "links",
        ),
        (
            "myelin://acme-eu/git/commit/repo:abc",
            "myelin://acme-eu/issue/issue/ENG-2",
            "links",
        ),
        (
            "myelin://acme-eu/git/blob/repo:main:src%2Flib.rs#L1-L9",
            "myelin://acme-eu/git/blob/repo:main:src%2Flib.rs",
            "embeds",
        ),
    ];
    let build = || {
        let edges = EdgeProjection::new();
        for (source, target, rel) in corpus {
            let id = edge_id(&t, source, target, rel);
            edges.upsert(
                &t,
                &region(),
                EdgeRow {
                    edge_id: id.clone(),
                    source: ArtifactRef(source.into()),
                    source_root: strip_sub(&ArtifactRef(source.into())),
                    target: ArtifactRef(target.into()),
                    target_root: strip_sub(&ArtifactRef(target.into())),
                    rel: rel.into(),
                    rel_class: RelClass::Reference,
                    origin_event: format!("evt-{id}"),
                    origin_actor: "git-pseudonym".into(),
                    zookie: Some("zk-1".into()),
                    tombstoned: false,
                },
            );
        }
        edges
    };
    let live = build();
    let live_hash = live.parity_hash(&t, &region());
    live.wipe_partition(&t, &region());
    assert_eq!(live.live_count(&t, &region()), 0, "partition wiped");
    let cold = build();
    assert_eq!(
        cold.parity_hash(&t, &region()),
        live_hash,
        "cold == live (byte-identical Git-edge reindex parity)"
    );
}

fn git_resolve_service(owner: &GitOwner) -> ResolveService {
    ResolveService::new(
        authz(),
        Arc::new(crate::resolve::NoOpCacheRead),
        Arc::new(owner.clone()),
        cell(),
    )
}
