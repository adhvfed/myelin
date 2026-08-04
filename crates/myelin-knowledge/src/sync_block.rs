use crate::block_tree::{BlockId, BlockRow, BlockTree};
use myelin_tenancy::ArtifactRef;

pub trait SourceReadCheck {
    fn can_read_source(&self, viewer: &Viewer, source: &ArtifactRef) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Viewer(pub String);

impl Viewer {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAll;

impl SourceReadCheck for DenyAll {
    fn can_read_source(&self, _viewer: &Viewer, _source: &ArtifactRef) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAll;

impl SourceReadCheck for AllowAll {
    fn can_read_source(&self, _viewer: &Viewer, _source: &ArtifactRef) -> bool {
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    Denied,
    RootGone,
    SubGone,
    Erased,
}

impl TombstoneReason {
    pub fn label(&self) -> &'static str {
        match self {
            TombstoneReason::Denied => "denied",
            TombstoneReason::RootGone => "root_gone",
            TombstoneReason::SubGone => "sub_gone",
            TombstoneReason::Erased => "erased",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    pub root: ArtifactRef,
    pub reason: TombstoneReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionFreshness {
    Live,
    Moved,
    Outdated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncBlockProjection {
    pub source: ArtifactRef,
    pub source_block: BlockId,
    pub subtree: Vec<BlockRow>,
    pub freshness: ProjectionFreshness,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncBlockRender {
    Projection(SyncBlockProjection),
    Tombstone(Tombstone),
}

impl SyncBlockRender {
    pub fn leaked_blocks(&self) -> usize {
        match self {
            SyncBlockRender::Projection(_) => 0,
            SyncBlockRender::Tombstone(_) => 0,
        }
    }

    pub fn tombstone_reason(&self) -> Option<TombstoneReason> {
        match self {
            SyncBlockRender::Projection(_) => None,
            SyncBlockRender::Tombstone(t) => Some(t.reason),
        }
    }
}

pub struct SyncSource<'a> {
    pub root: ArtifactRef,
    pub tree: Option<&'a BlockTree>,
    pub source_block: BlockId,
    pub erased: bool,
    pub freshness: ProjectionFreshness,
}

pub fn render_sync_block<C: SourceReadCheck>(
    check: &C,
    viewer: &Viewer,
    source: &SyncSource<'_>,
) -> SyncBlockRender {
    if !check.can_read_source(viewer, &source.root) {
        return SyncBlockRender::Tombstone(Tombstone {
            root: source.root.clone(),
            reason: TombstoneReason::Denied,
        });
    }

    if source.erased {
        return SyncBlockRender::Tombstone(Tombstone {
            root: source.root.clone(),
            reason: TombstoneReason::Erased,
        });
    }

    let tree = match source.tree {
        Some(t) => t,
        None => {
            return SyncBlockRender::Tombstone(Tombstone {
                root: source.root.clone(),
                reason: TombstoneReason::RootGone,
            })
        }
    };

    match tree.resolve_sub(&source.source_block) {
        Some(_present) => {
            let subtree: Vec<BlockRow> = tree
                .subtree_walk_cte(&source.source_block)
                .into_iter()
                .cloned()
                .collect();
            SyncBlockRender::Projection(SyncBlockProjection {
                source: source.root.clone(),
                source_block: source.source_block.clone(),
                subtree,
                freshness: source.freshness,
            })
        }
        None => SyncBlockRender::Tombstone(Tombstone {
            root: source.root.clone(),
            reason: TombstoneReason::SubGone,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_tree::PageId;
    use myelin_query::field::Jitter;

    fn jit(a: usize, b: usize) -> Jitter {
        Jitter::from_ranks(a, b).expect("jitter ranks in 0..62")
    }

    fn bid(s: &str) -> BlockId {
        BlockId(s.to_string())
    }

    fn source_tree() -> BlockTree {
        let mut t = BlockTree::new(PageId("src-page".into()));
        t.insert_root(bid("page-root"), "paragraph", jit(0, 0))
            .unwrap();
        t.insert_block(bid("head"), &bid("page-root"), "heading", jit(0, 1))
            .unwrap();
        t.insert_block(bid("a"), &bid("head"), "paragraph", jit(0, 0))
            .unwrap();
        t.insert_block(bid("b"), &bid("head"), "paragraph", jit(0, 1))
            .unwrap();
        t
    }

    fn source_root() -> ArtifactRef {
        ArtifactRef("myelin://acme/knowledge/page/src-page".into())
    }

    fn viewer() -> Viewer {
        Viewer("p-bob".into())
    }

    fn live_source<'a>(tree: &'a BlockTree, head: &str) -> SyncSource<'a> {
        SyncSource {
            root: source_root(),
            tree: Some(tree),
            source_block: bid(head),
            erased: false,
            freshness: ProjectionFreshness::Live,
        }
    }

    #[test]
    fn denied_viewer_gets_tombstone_never_content() {
        let tree = source_tree();
        let src = live_source(&tree, "head");
        let render = render_sync_block(&DenyAll, &viewer(), &src);

        match &render {
            SyncBlockRender::Tombstone(t) => {
                assert_eq!(t.reason, TombstoneReason::Denied, "step-1 denied");
                assert_eq!(t.reason.label(), "denied");
                assert_eq!(
                    t.root,
                    source_root(),
                    "the tombstone carries the root (§4.6)"
                );
            }
            SyncBlockRender::Projection(_) => panic!("a denied viewer must NEVER get a projection"),
        }
        assert_eq!(
            render.leaked_blocks(),
            0,
            "sync_block_leak == 0 (no content to a denied viewer)"
        );
        assert!(
            matches!(render, SyncBlockRender::Tombstone(_)),
            "a denied render is structurally a tombstone (carries no BlockRow)"
        );
    }

    #[test]
    fn source_edit_reflects_in_projection() {
        let mut tree = source_tree();

        let first = render_sync_block(&AllowAll, &viewer(), &live_source(&tree, "head"));
        let first_ids: Vec<String> = match &first {
            SyncBlockRender::Projection(p) => {
                p.subtree.iter().map(|r| r.block_id.0.clone()).collect()
            }
            SyncBlockRender::Tombstone(_) => panic!("a permitted viewer gets a projection"),
        };
        assert_eq!(
            first_ids,
            vec!["head", "a", "b"],
            "the initial live subtree"
        );

        tree.insert_block(bid("c"), &bid("head"), "paragraph", jit(0, 2))
            .unwrap();

        let second = render_sync_block(&AllowAll, &viewer(), &live_source(&tree, "head"));
        let second_ids: Vec<String> = match &second {
            SyncBlockRender::Projection(p) => {
                p.subtree.iter().map(|r| r.block_id.0.clone()).collect()
            }
            SyncBlockRender::Tombstone(_) => panic!("still a projection"),
        };
        assert_eq!(
            second_ids,
            vec!["head", "a", "b", "c"],
            "the source edit (appended `c`) REFLECTS in the projection (live, not stale)"
        );
    }

    #[test]
    fn permitted_viewer_gets_live_projection() {
        let tree = source_tree();
        let render = render_sync_block(&AllowAll, &viewer(), &live_source(&tree, "head"));
        match render {
            SyncBlockRender::Projection(p) => {
                assert_eq!(p.source, source_root());
                assert_eq!(p.source_block, bid("head"));
                assert_eq!(p.freshness, ProjectionFreshness::Live);
                let ids: Vec<&str> = p.subtree.iter().map(|r| r.block_id.as_str()).collect();
                assert_eq!(ids, vec!["head", "a", "b"], "the live source subtree");
            }
            SyncBlockRender::Tombstone(_) => panic!("a permitted, resolvable source projects"),
        }
    }

    #[test]
    fn root_gone_is_a_tombstone() {
        let src = SyncSource {
            root: source_root(),
            tree: None,
            source_block: bid("head"),
            erased: false,
            freshness: ProjectionFreshness::Live,
        };
        let render = render_sync_block(&AllowAll, &viewer(), &src);
        assert_eq!(render.tombstone_reason(), Some(TombstoneReason::RootGone));
        assert_eq!(render.leaked_blocks(), 0);
    }

    #[test]
    fn sub_gone_is_a_tombstone() {
        let tree = source_tree();
        let src = live_source(&tree, "ghost-block");
        let render = render_sync_block(&AllowAll, &viewer(), &src);
        match &render {
            SyncBlockRender::Tombstone(t) => {
                assert_eq!(
                    t.reason,
                    TombstoneReason::SubGone,
                    "the block is gone, the root resolves"
                );
                assert_eq!(t.root, source_root(), "the root is still carried");
            }
            SyncBlockRender::Projection(_) => panic!("a gone source block is a tombstone"),
        }
        assert_eq!(render.leaked_blocks(), 0);
    }

    #[test]
    fn erased_source_is_a_tombstone_even_for_permitted_viewer() {
        let tree = source_tree();
        let src = SyncSource {
            root: source_root(),
            tree: Some(&tree),
            source_block: bid("head"),
            erased: true,
            freshness: ProjectionFreshness::Live,
        };
        let render = render_sync_block(&AllowAll, &viewer(), &src);
        assert_eq!(render.tombstone_reason(), Some(TombstoneReason::Erased));
        assert_eq!(
            render.leaked_blocks(),
            0,
            "an erased source renders NO content"
        );
    }

    #[test]
    fn moved_source_projects_live_flagged_moved() {
        let mut tree = source_tree();
        tree.move_block(&bid("head"), &bid("page-root"), None, None, jit(2, 2))
            .unwrap();
        let src = SyncSource {
            root: source_root(),
            tree: Some(&tree),
            source_block: bid("head"),
            erased: false,
            freshness: ProjectionFreshness::Moved,
        };
        let render = render_sync_block(&AllowAll, &viewer(), &src);
        match render {
            SyncBlockRender::Projection(p) => {
                assert_eq!(
                    p.freshness,
                    ProjectionFreshness::Moved,
                    "the move is flagged, not lost"
                );
                let ids: Vec<&str> = p.subtree.iter().map(|r| r.block_id.as_str()).collect();
                assert_eq!(
                    ids,
                    vec!["head", "a", "b"],
                    "the moved source still renders its subtree"
                );
            }
            SyncBlockRender::Tombstone(_) => {
                panic!("a moved source LIVES (stable block_id), it is not a tombstone")
            }
        }
    }

    #[test]
    fn per_viewer_two_answers_from_one_source() {
        let tree = source_tree();
        let denied = render_sync_block(&DenyAll, &viewer(), &live_source(&tree, "head"));
        let allowed = render_sync_block(&AllowAll, &viewer(), &live_source(&tree, "head"));
        assert!(
            matches!(denied, SyncBlockRender::Tombstone(_)),
            "denied viewer → tombstone"
        );
        assert!(
            matches!(allowed, SyncBlockRender::Projection(_)),
            "permitted viewer → projection"
        );
        assert_eq!(denied.leaked_blocks(), 0);
        assert_eq!(allowed.leaked_blocks(), 0);
    }

    #[test]
    fn tombstone_reason_labels_are_the_frozen_vocabulary() {
        assert_eq!(TombstoneReason::Denied.label(), "denied");
        assert_eq!(TombstoneReason::RootGone.label(), "root_gone");
        assert_eq!(TombstoneReason::SubGone.label(), "sub_gone");
        assert_eq!(TombstoneReason::Erased.label(), "erased");
    }
}
