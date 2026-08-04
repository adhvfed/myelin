use myelin_knowledge::block_tree::{BlockId, BlockTree, PageId};
use myelin_knowledge::{
    render_sync_block, AllowAll, DenyAll, ProjectionFreshness, SyncBlockRender, SyncSource,
    TombstoneReason, Viewer,
};
use myelin_query::field::Jitter;
use myelin_tenancy::ArtifactRef;

fn jit(a: usize, b: usize) -> Jitter {
    Jitter::from_ranks(a, b).expect("jitter ranks in 0..62")
}

fn bid(s: &str) -> BlockId {
    BlockId(s.to_string())
}

fn source_root() -> ArtifactRef {
    ArtifactRef("myelin://acme/knowledge/page/handbook".into())
}

fn source_tree() -> BlockTree {
    let mut t = BlockTree::new(PageId("handbook".into()));
    t.insert_root(bid("page-root"), "paragraph", jit(0, 0))
        .unwrap();
    t.insert_block(bid("head"), &bid("page-root"), "heading", jit(0, 1))
        .unwrap();
    t.insert_block(bid("policy"), &bid("head"), "paragraph", jit(0, 0))
        .unwrap();
    t
}

fn live_source<'a>(tree: &'a BlockTree) -> SyncSource<'a> {
    SyncSource {
        root: source_root(),
        tree: Some(tree),
        source_block: bid("head"),
        erased: false,
        freshness: ProjectionFreshness::Live,
    }
}

#[test]
fn provider_sync_block_render_is_permission_first() {
    let tree = source_tree();

    let denied = render_sync_block(&DenyAll, &Viewer("p-anon".into()), &live_source(&tree));
    match &denied {
        SyncBlockRender::Tombstone(t) => {
            assert_eq!(
                t.reason,
                TombstoneReason::Denied,
                "step-1 denied (permission-first)"
            );
            assert_eq!(
                t.root,
                source_root(),
                "the tombstone carries the root (§4.6)"
            );
        }
        SyncBlockRender::Projection(_) => panic!("a denied viewer must NEVER get a projection"),
    }
    assert_eq!(
        denied.leaked_blocks(),
        0,
        "sync_block_leak == 0 (the M3 leak gate)"
    );

    let allowed = render_sync_block(&AllowAll, &Viewer("p-alice".into()), &live_source(&tree));
    match allowed {
        SyncBlockRender::Projection(p) => {
            assert_eq!(p.source, source_root());
            let ids: Vec<&str> = p.subtree.iter().map(|r| r.block_id.as_str()).collect();
            assert_eq!(
                ids,
                vec!["head", "policy"],
                "the permitted viewer gets the live subtree"
            );
        }
        SyncBlockRender::Tombstone(_) => panic!("a permitted, resolvable source projects"),
    }
}

#[test]
fn provider_projection_is_live_reflects_source_edit() {
    let mut tree = source_tree();
    let first = render_sync_block(&AllowAll, &Viewer("p-alice".into()), &live_source(&tree));
    let first_ids: Vec<String> = match &first {
        SyncBlockRender::Projection(p) => p.subtree.iter().map(|r| r.block_id.0.clone()).collect(),
        SyncBlockRender::Tombstone(_) => panic!("projection"),
    };
    assert_eq!(first_ids, vec!["head", "policy"]);

    tree.insert_block(bid("addendum"), &bid("head"), "paragraph", jit(0, 1))
        .unwrap();

    let second = render_sync_block(&AllowAll, &Viewer("p-alice".into()), &live_source(&tree));
    let second_ids: Vec<String> = match &second {
        SyncBlockRender::Projection(p) => p.subtree.iter().map(|r| r.block_id.0.clone()).collect(),
        SyncBlockRender::Tombstone(_) => panic!("projection"),
    };
    assert_eq!(
        second_ids,
        vec!["head", "policy", "addendum"],
        "the source edit reflects in the projection (live, the reflect gate)"
    );
}

#[test]
fn consumer_renders_projection_or_degrades_tombstone() {
    fn render_for_display(outcome: &SyncBlockRender) -> String {
        match outcome {
            SyncBlockRender::Projection(p) => {
                let body: Vec<&str> = p.subtree.iter().map(|r| r.block_id.as_str()).collect();
                format!("[synced from {}] {}", p.source.0, body.join(","))
            }
            SyncBlockRender::Tombstone(t) => {
                format!(
                    "[referenced {} - unavailable: {}]",
                    t.root.0,
                    t.reason.label()
                )
            }
        }
    }

    let tree = source_tree();

    let denied = render_sync_block(&DenyAll, &Viewer("p-anon".into()), &live_source(&tree));
    let denied_view = render_for_display(&denied);
    assert_eq!(
        denied_view, "[referenced myelin://acme/knowledge/page/handbook - unavailable: denied]",
        "the consumer degrades to a content-free notice on deny"
    );
    assert!(
        !denied_view.contains("policy"),
        "the consumer NEVER renders source content on deny"
    );

    let allowed = render_sync_block(&AllowAll, &Viewer("p-alice".into()), &live_source(&tree));
    let allowed_view = render_for_display(&allowed);
    assert_eq!(
        allowed_view, "[synced from myelin://acme/knowledge/page/handbook] head,policy",
        "the consumer renders the live subtree on a projection"
    );
}

#[test]
fn consumer_sees_the_full_frozen_ladder_vocabulary() {
    let root_gone = render_sync_block(
        &AllowAll,
        &Viewer("p-alice".into()),
        &SyncSource {
            root: source_root(),
            tree: None,
            source_block: bid("head"),
            erased: false,
            freshness: ProjectionFreshness::Live,
        },
    );
    assert_eq!(
        root_gone.tombstone_reason(),
        Some(TombstoneReason::RootGone)
    );
    assert_eq!(root_gone.leaked_blocks(), 0);

    let tree = source_tree();
    let sub_gone = render_sync_block(
        &AllowAll,
        &Viewer("p-alice".into()),
        &SyncSource {
            root: source_root(),
            tree: Some(&tree),
            source_block: bid("deleted-block"),
            erased: false,
            freshness: ProjectionFreshness::Live,
        },
    );
    assert_eq!(sub_gone.tombstone_reason(), Some(TombstoneReason::SubGone));
    assert_eq!(sub_gone.leaked_blocks(), 0);

    let erased = render_sync_block(
        &AllowAll,
        &Viewer("p-alice".into()),
        &SyncSource {
            root: source_root(),
            tree: Some(&tree),
            source_block: bid("head"),
            erased: true,
            freshness: ProjectionFreshness::Live,
        },
    );
    assert_eq!(erased.tombstone_reason(), Some(TombstoneReason::Erased));
    assert_eq!(
        erased.leaked_blocks(),
        0,
        "an erased source renders NO content"
    );
}
