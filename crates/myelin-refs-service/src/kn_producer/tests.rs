use std::sync::Arc;

use myelin_content::events::{KNOWLEDGE_PAGE_CREATED, KNOWLEDGE_PAGE_MOVED};
use myelin_content::InlineNode;
use myelin_events::ArtifactRef;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::{mint, strip_sub, sub_kind, Sub};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

use super::*;
use crate::backlinks::{AuthzVisibleIndex, BacklinkRead};
use crate::edge_builder::{edge_id, EdgeProjection, EdgeRow, RelClass};
use crate::ladder::resolve_sub_outcome;
use crate::mirror::MirrorError;
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
fn real_kn_page_body_extracts_one_edge_per_structured_ref_node() {
    let producer = KnEdgeProducer;
    let source = KnEdgeProducer::page_root("acme-eu", "design-doc");

    let body = vec![
        InlineNode::Embed(ArtifactRef(
            "myelin://acme-eu/knowledge/page/sibling".into(),
        )),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme-eu/issue/issue/ENG-12".into())),
        InlineNode::Mention(viewer("alice", &tenant())),
    ];
    let edges = producer.kn_edges(&source, &body);
    assert_eq!(edges.len(), 3, "three structured ref nodes → three edges");
    for e in &edges {
        assert_eq!(e.source, source);
        assert_eq!(e.rel_class, RelClass::Reference);
    }
    assert_eq!(edges[0].target.0, "myelin://acme-eu/knowledge/page/sibling");
    assert_eq!(edges[0].rel.as_str(), "embeds");
    assert_eq!(edges[2].target.0, "myelin://acme-eu/identity/member/alice");
    assert_eq!(edges[2].rel.as_str(), "mentions");
}

#[test]
fn block_body_reference_sources_from_the_block_root() {
    let producer = KnEdgeProducer;
    let source = KnEdgeProducer::block_root("acme-eu", "blk-9");
    let body = vec![InlineNode::ArtifactRefNode(ArtifactRef(
        "myelin://acme-eu/issue/issue/ENG-7".into(),
    ))];
    let edges = producer.kn_edges(&source, &body);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].source.0, "myelin://acme-eu/knowledge/block/blk-9");
    assert_eq!(edges[0].target.0, "myelin://acme-eu/issue/issue/ENG-7");
}

fn block_ref(page: &str, block_id: &str) -> ArtifactRef {
    let root = KnEdgeProducer::page_root("acme-eu", page);
    mint(&root, Sub::Block(block_id.into())).expect("grammatical b<id> mint")
}

#[test]
fn stable_kn_block_resolves_live() {
    let owner = KnOwner::new();
    let ref_ = block_ref("design-doc", "b1");
    owner.record_anchor(&ref_, KnAnchorState::Live);
    match resolve_sub_outcome(&owner, &ref_) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, None, "a stable block is a clean LIVE"),
        other => panic!("expected LIVE, got {other:?}"),
    }
}

#[test]
fn edited_kn_block_resolves_outdated() {
    let owner = KnOwner::new();
    let ref_ = block_ref("design-doc", "b2");
    owner.record_anchor(&ref_, KnAnchorState::Edited);
    match resolve_sub_outcome(&owner, &ref_) {
        ProjectOutcome::Live(p) => {
            assert_eq!(p.flag, Some(crate::resolve::ProjectionFlag::Outdated));
            assert!(p.sub_anchor.is_some());
        }
        other => panic!("expected OUTDATED Live, got {other:?}"),
    }
}

#[test]
fn moved_kn_block_resolves_moved() {
    let owner = KnOwner::new();
    let ref_ = block_ref("design-doc", "b3");
    owner.record_anchor(&ref_, KnAnchorState::Moved);
    match resolve_sub_outcome(&owner, &ref_) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, Some(crate::resolve::ProjectionFlag::Moved)),
        other => panic!("expected MOVED Live, got {other:?}"),
    }
}

#[test]
fn deleted_kn_block_tombstones_carrying_the_root() {
    let owner = KnOwner::new();
    let ref_ = block_ref("design-doc", "b4");
    owner.record_anchor(&ref_, KnAnchorState::Deleted);

    let svc = kn_resolve_service(&owner);
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
    assert!(res.is_tombstone(), "deleted block → tombstone");
    assert_eq!(res.tombstone_reason(), Some(TombstoneReason::SubGone));
    if let crate::resolve::Resolution::Tombstone(t) = res {
        assert_eq!(t.root, strip_sub(&ref_));
        assert_eq!(t.root.0, "myelin://acme-eu/knowledge/page/design-doc");
    }
}

#[test]
fn kn_heading_row_field_anchors_degrade_through_the_ladder() {
    let owner = KnOwner::new();
    let page = KnEdgeProducer::page_root("acme-eu", "db-page");
    let heading = mint(&page, Sub::Heading("h7".into())).expect("h<id>");
    let row = mint(&page, Sub::Row("r3".into())).expect("row-<id>");
    let field = mint(&page, Sub::Field("status".into())).expect("field-<id>");

    owner.record_anchor(&heading, KnAnchorState::Edited);
    owner.record_anchor(&row, KnAnchorState::Deleted);
    owner.record_anchor(&field, KnAnchorState::Live);

    match resolve_sub_outcome(&owner, &heading) {
        ProjectOutcome::Live(p) => {
            assert_eq!(p.flag, Some(crate::resolve::ProjectionFlag::Outdated))
        }
        other => panic!("edited heading → OUTDATED, got {other:?}"),
    }
    assert_eq!(resolve_sub_outcome(&owner, &row), ProjectOutcome::SubGone);
    match resolve_sub_outcome(&owner, &field) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, None),
        other => panic!("live field → LIVE, got {other:?}"),
    }
}

#[test]
fn erased_kn_block_is_an_erased_tombstone() {
    let owner = KnOwner::new();
    let ref_ = block_ref("design-doc", "b-erased");
    owner.record_anchor(&ref_, KnAnchorState::Erased);
    assert_eq!(resolve_sub_outcome(&owner, &ref_), ProjectOutcome::Erased);
}

#[test]
fn ref_d1_denied_viewer_of_a_kn_page_is_tombstoned_never_leaked() {
    let owner = KnOwner::new();
    let page = KnEdgeProducer::page_root("acme-eu", "confidential-roadmap");
    let outsider = viewer("outsider", &tenant());
    let svc = kn_resolve_service(&owner);
    let res = svc.resolve(
        &tenant(),
        &region(),
        &page,
        &page,
        &outsider,
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(res.is_tombstone(), "a denied viewer is tombstoned");
    assert_eq!(res.tombstone_reason(), Some(TombstoneReason::Denied));
    if let crate::resolve::Resolution::Tombstone(t) = res {
        assert_eq!(t.root, page);
    }
}

#[test]
fn ref_d2_cross_tenant_kn_backlink_read_returns_nothing() {
    use myelin_identity::ListObjectsResult;

    let tenant_a = TenantId("tenantA".into());
    let edges = EdgeProjection::new();
    let page = "myelin://tenantA/knowledge/page/parent";
    let target = "myelin://tenantA/knowledge/page/child";
    let id = edge_id(&tenant_a, page, target, "embeds");
    edges.upsert(
        &tenant_a,
        &region(),
        EdgeRow {
            edge_id: id.clone(),
            source: ArtifactRef(page.into()),
            source_root: strip_sub(&ArtifactRef(page.into())),
            target: ArtifactRef(target.into()),
            target_root: strip_sub(&ArtifactRef(target.into())),
            rel: "embeds".into(),
            rel_class: RelClass::Reference,
            origin_event: format!("evt-{id}"),
            origin_actor: "kn-pseudonym-1".into(),
            zookie: Some("zk-1".into()),
            tombstoned: false,
        },
    );

    let read = BacklinkRead::new(edges, AuthzVisibleIndex::new());
    let tenant_b = TenantId("tenantB".into());
    let viewer_b = viewer("attacker", &tenant_b);
    let result_page = read
        .backlinks(
            &tenant_b,
            &region(),
            &ArtifactRef(target.into()),
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
        result_page.edges.len(),
        0,
        "no cross-tenant KN backlink is visible (REF-D2)"
    );
}

#[test]
fn kn_fragment_flows_through_list_objects_leak_free() {
    use myelin_identity::ListObjectsResult;

    let t = tenant();
    let edges = EdgeProjection::new();
    let target = "myelin://acme-eu/knowledge/page/shared";
    for page in [
        "myelin://acme-eu/knowledge/page/public",
        "myelin://acme-eu/knowledge/page/secret",
    ] {
        let id = edge_id(&t, page, target, "embeds");
        edges.upsert(
            &t,
            &region(),
            EdgeRow {
                edge_id: id.clone(),
                source: ArtifactRef(page.into()),
                source_root: strip_sub(&ArtifactRef(page.into())),
                target: ArtifactRef(target.into()),
                target_root: strip_sub(&ArtifactRef(target.into())),
                rel: "embeds".into(),
                rel_class: RelClass::Reference,
                origin_event: format!("evt-{id}"),
                origin_actor: "kn-pseudonym".into(),
                zookie: Some("zk-1".into()),
                tombstoned: false,
            },
        );
    }
    let read = BacklinkRead::new(edges, AuthzVisibleIndex::new());
    let v = viewer("reader", &t);
    let allowed = ListObjectsResult::Ids {
        ids: vec![myelin_identity::ObjectId(
            "myelin://acme-eu/knowledge/page/public".into(),
        )],
        zookie: myelin_identity::Zookie("zk-1".into()),
    };
    let page = read
        .backlinks(
            &t,
            &region(),
            &ArtifactRef(target.into()),
            &v,
            &allowed,
            &bounded_stale(),
            10,
        )
        .expect("backlink read");
    assert_eq!(
        page.edges.len(),
        1,
        "only the page-tree-admitted page is visible"
    );
    assert_eq!(
        page.edges[0].source.0,
        "myelin://acme-eu/knowledge/page/public"
    );
}

fn page_parent_event(parent: &str, child: &str, trigger: &str) -> PageParentEvent {
    PageParentEvent {
        parent: KnEdgeProducer::page_root("acme-eu", parent),
        child: KnEdgeProducer::page_root("acme-eu", child),
        origin_event_id: format!("evt-{parent}-{child}"),
        origin_event_type: trigger.into(),
        origin_actor: "kn-pseudonym".into(),
        zookie: Some("zk-1".into()),
    }
}

#[test]
fn page_parent_mirror_projects_both_inverse_paired_lifecycle_edges() {
    let ev = page_parent_event("root", "section", KNOWLEDGE_PAGE_CREATED);
    let rows = mirror_page_parent(&tenant(), &ev).expect("recognised lifecycle trigger");
    assert_eq!(rows.len(), 2, "parent + the frozen inverse child edge");

    let fwd = rows
        .iter()
        .find(|r| r.rel == "parent")
        .expect("a parent edge");
    assert_eq!(fwd.source.0, "myelin://acme-eu/knowledge/page/root");
    assert_eq!(fwd.target.0, "myelin://acme-eu/knowledge/page/section");
    assert_eq!(
        fwd.rel_class,
        RelClass::Lifecycle,
        "a mirror edge is ALWAYS lifecycle-class"
    );

    let inv = rows
        .iter()
        .find(|r| r.rel == "child")
        .expect("the inverse child edge");
    assert_eq!(inv.source.0, "myelin://acme-eu/knowledge/page/section");
    assert_eq!(inv.target.0, "myelin://acme-eu/knowledge/page/root");
    assert_eq!(inv.rel_class, RelClass::Lifecycle);
}

#[test]
fn page_parent_mirror_accepts_the_move_reparent_trigger() {
    let ev = page_parent_event("newroot", "moved-page", KNOWLEDGE_PAGE_MOVED);
    let rows = mirror_page_parent(&tenant(), &ev).expect("move is a recognised trigger");
    assert_eq!(rows.len(), 2);
}

#[test]
fn page_parent_mirror_rejects_an_unrecognised_trigger() {
    let ev = page_parent_event("a", "b", "knowledge.page.archived");
    let err = mirror_page_parent(&tenant(), &ev).expect_err("not a re-parent trigger");
    assert_eq!(
        err,
        MirrorError::UnknownRel("knowledge.page.archived".into())
    );
}

#[test]
fn page_parent_mirror_is_idempotent_on_replay() {
    let proj = EdgeProjection::new();
    let ev = page_parent_event("root", "section", KNOWLEDGE_PAGE_CREATED);
    let ids1 = project_page_parent(&proj, &tenant(), &region(), &ev).expect("project");
    let ids2 = project_page_parent(&proj, &tenant(), &region(), &ev).expect("re-project");
    assert_eq!(ids1, ids2, "the same deterministic edge_id pair on replay");
    let child = KnEdgeProducer::page_root("acme-eu", "section");
    let inbound = proj.inbound_live(&tenant(), &region(), &child);
    let parent_edges: Vec<_> = inbound.iter().filter(|r| r.rel == "parent").collect();
    assert_eq!(
        parent_edges.len(),
        1,
        "idempotent - one parent edge inbound to the child"
    );
}

#[test]
fn page_parent_reconverges_to_the_typed_table_typed_wins() {
    let proj = EdgeProjection::new();
    let t = tenant();
    let r = region();

    let drift = page_parent_event("old-root", "section", KNOWLEDGE_PAGE_CREATED);
    project_page_parent(&proj, &t, &r, &drift).expect("project drift");

    let truth = page_parent_event("new-root", "section", KNOWLEDGE_PAGE_MOVED);
    let section = KnEdgeProducer::page_root("acme-eu", "section");
    let (reprojected, tombstoned) = reconverge_page_tree(
        &proj,
        &t,
        &r,
        &[truth],
        std::slice::from_ref(&section),
        "evt-reindex-1",
    )
    .expect("reconverge");
    assert_eq!(
        reprojected, 2,
        "the typed truth's parent+child pair re-projected"
    );
    assert!(
        tombstoned >= 1,
        "the drifted old-root parent edge is tombstoned (typed wins)"
    );

    let inbound = proj.inbound_live(&t, &r, &section);
    let parents: Vec<&str> = inbound
        .iter()
        .filter(|r| r.rel == "parent")
        .map(|r| r.source.0.as_str())
        .collect();
    assert_eq!(
        parents,
        vec!["myelin://acme-eu/knowledge/page/new-root"],
        "typed table wins"
    );
}

#[test]
fn reconverge_rejects_a_non_trigger_snapshot_event() {
    let proj = EdgeProjection::new();
    let bad = page_parent_event("a", "b", "knowledge.page.archived");
    let err = reconverge_page_tree(&proj, &tenant(), &region(), &[bad], &[], "evt-x")
        .expect_err("non-trigger snapshot event");
    assert_eq!(
        err,
        MirrorError::UnknownRel("knowledge.page.archived".into())
    );
}

#[test]
fn kn_replay_scope_is_sub_artifact_granular() {
    assert_eq!(
        kn_replay_scope(KnReplayGrain::Page("home".into())),
        "page:home"
    );
    assert_eq!(
        kn_replay_scope(KnReplayGrain::Block {
            page: "home".into(),
            id: "b7".into()
        }),
        "block:home/b7"
    );
    assert_eq!(
        kn_replay_scope(KnReplayGrain::Subtree("root".into())),
        "subtree:root"
    );
    assert_eq!(KN_OWNER_TOKEN, "knowledge");
}

#[test]
fn ref_d4_kn_corpus_reindex_byte_parity_incl_page_parent_mirror() {
    let t = tenant();
    let r = region();
    let ref_corpus = [
        (
            "myelin://acme-eu/knowledge/page/a",
            "myelin://acme-eu/knowledge/page/b",
            "embeds",
        ),
        (
            "myelin://acme-eu/knowledge/block/blk-1",
            "myelin://acme-eu/issue/issue/ENG-1",
            "links",
        ),
    ];
    let parent_ev = page_parent_event("root", "child", KNOWLEDGE_PAGE_CREATED);

    let build = || {
        let edges = EdgeProjection::new();
        for (source, target, rel) in ref_corpus {
            let id = edge_id(&t, source, target, rel);
            edges.upsert(
                &t,
                &r,
                EdgeRow {
                    edge_id: id.clone(),
                    source: ArtifactRef(source.into()),
                    source_root: strip_sub(&ArtifactRef(source.into())),
                    target: ArtifactRef(target.into()),
                    target_root: strip_sub(&ArtifactRef(target.into())),
                    rel: rel.into(),
                    rel_class: RelClass::Reference,
                    origin_event: format!("evt-{id}"),
                    origin_actor: "kn-pseudonym".into(),
                    zookie: Some("zk-1".into()),
                    tombstoned: false,
                },
            );
        }
        project_page_parent(&edges, &t, &r, &parent_ev).expect("project page_parent");
        edges
    };

    let live = build();
    let live_hash = live.parity_hash(&t, &r);
    live.wipe_partition(&t, &r);
    assert_eq!(live.live_count(&t, &r), 0, "partition wiped");
    let cold = build();
    assert_eq!(
        cold.parity_hash(&t, &r),
        live_hash,
        "cold == live (byte-identical KN-corpus reindex parity, incl. the page_parent mirror)"
    );
}

fn kn_resolve_service(owner: &KnOwner) -> ResolveService {
    ResolveService::new(
        authz(),
        Arc::new(crate::resolve::NoOpCacheRead),
        Arc::new(owner.clone()),
        cell(),
    )
}

#[test]
fn kn_block_ref_classifies_through_the_one_grammar() {
    let ref_ = block_ref("design-doc", "b1");
    assert!(sub_kind(&ref_).is_some());
    assert_eq!(
        strip_sub(&ref_).0,
        "myelin://acme-eu/knowledge/page/design-doc"
    );
}
