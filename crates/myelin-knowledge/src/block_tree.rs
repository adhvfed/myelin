use myelin_query::field::{Jitter, OrderKey};
use std::collections::BTreeMap;

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct BlockId(pub String);

impl BlockId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct PageId(pub String);

impl PageId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRow {
    pub block_id: BlockId,
    pub page_id: PageId,
    pub parent_id: Option<BlockId>,
    pub order_key: OrderKey,
    pub block_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeError {
    NoSuchBlock(BlockId),
    DuplicateBlockId(BlockId),
    WouldCycle(BlockId),
    CrossPageParent { block: BlockId, parent: BlockId },
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TreeError::NoSuchBlock(b) => write!(f, "no such block in the tree: {}", b.as_str()),
            TreeError::DuplicateBlockId(b) => {
                write!(f, "block_id already live (ids mint once): {}", b.as_str())
            }
            TreeError::WouldCycle(b) => {
                write!(
                    f,
                    "move would make block {} its own ancestor (cycle)",
                    b.as_str()
                )
            }
            TreeError::CrossPageParent { block, parent } => write!(
                f,
                "block {} cannot parent under {} in a different page",
                block.as_str(),
                parent.as_str()
            ),
        }
    }
}

impl std::error::Error for TreeError {}

#[derive(Clone, Debug)]
pub struct BlockTree {
    page_id: PageId,
    rows: BTreeMap<BlockId, BlockRow>,
}

impl BlockTree {
    pub fn new(page_id: PageId) -> BlockTree {
        BlockTree {
            page_id,
            rows: BTreeMap::new(),
        }
    }

    pub fn page_id(&self) -> &PageId {
        &self.page_id
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn get(&self, id: &BlockId) -> Option<&BlockRow> {
        self.rows.get(id)
    }

    pub fn insert_root(
        &mut self,
        block_id: BlockId,
        block_type: &str,
        jitter: Jitter,
    ) -> Result<(), TreeError> {
        if self.rows.contains_key(&block_id) {
            return Err(TreeError::DuplicateBlockId(block_id));
        }
        let order_key = OrderKey::rank_first(jitter);
        self.rows.insert(
            block_id.clone(),
            BlockRow {
                block_id,
                page_id: self.page_id.clone(),
                parent_id: None,
                order_key,
                block_type: block_type.to_string(),
            },
        );
        Ok(())
    }

    pub fn insert_block(
        &mut self,
        block_id: BlockId,
        parent: &BlockId,
        block_type: &str,
        jitter: Jitter,
    ) -> Result<(), TreeError> {
        if !self.rows.contains_key(parent) {
            return Err(TreeError::NoSuchBlock(parent.clone()));
        }
        if self.rows.contains_key(&block_id) {
            return Err(TreeError::DuplicateBlockId(block_id));
        }
        let last = self.last_child_key(parent);
        let order_key = OrderKey::rank_last(last.as_ref(), jitter);
        self.rows.insert(
            block_id.clone(),
            BlockRow {
                block_id,
                page_id: self.page_id.clone(),
                parent_id: Some(parent.clone()),
                order_key,
                block_type: block_type.to_string(),
            },
        );
        Ok(())
    }

    pub fn insert_between(
        &mut self,
        block_id: BlockId,
        parent: &BlockId,
        lo: Option<&BlockId>,
        hi: Option<&BlockId>,
        block_type: &str,
        jitter: Jitter,
    ) -> Result<(), TreeError> {
        if !self.rows.contains_key(parent) {
            return Err(TreeError::NoSuchBlock(parent.clone()));
        }
        if self.rows.contains_key(&block_id) {
            return Err(TreeError::DuplicateBlockId(block_id));
        }
        let lo_key = self.bound_key(lo)?;
        let hi_key = self.bound_key(hi)?;
        let order_key = OrderKey::rank_between(lo_key.as_ref(), hi_key.as_ref(), jitter);
        self.rows.insert(
            block_id.clone(),
            BlockRow {
                block_id,
                page_id: self.page_id.clone(),
                parent_id: Some(parent.clone()),
                order_key,
                block_type: block_type.to_string(),
            },
        );
        Ok(())
    }

    pub fn move_block(
        &mut self,
        block_id: &BlockId,
        new_parent: &BlockId,
        lo: Option<&BlockId>,
        hi: Option<&BlockId>,
        jitter: Jitter,
    ) -> Result<(), TreeError> {
        if !self.rows.contains_key(block_id) {
            return Err(TreeError::NoSuchBlock(block_id.clone()));
        }
        if !self.rows.contains_key(new_parent) {
            return Err(TreeError::NoSuchBlock(new_parent.clone()));
        }
        if block_id == new_parent || self.is_descendant_of(new_parent, block_id) {
            return Err(TreeError::WouldCycle(block_id.clone()));
        }
        let lo_key = self.bound_key(lo)?;
        let hi_key = self.bound_key(hi)?;
        let order_key = OrderKey::rank_between(lo_key.as_ref(), hi_key.as_ref(), jitter);
        let row = self.rows.get_mut(block_id).expect("checked present above");
        row.parent_id = Some(new_parent.clone());
        row.order_key = order_key;
        Ok(())
    }

    pub fn children(&self, parent: &BlockId) -> Vec<&BlockRow> {
        let mut kids: Vec<&BlockRow> = self
            .rows
            .values()
            .filter(|r| r.parent_id.as_ref() == Some(parent))
            .collect();
        kids.sort_by(|a, b| {
            a.order_key
                .cmp(&b.order_key)
                .then_with(|| a.block_id.cmp(&b.block_id))
        });
        kids
    }

    pub fn roots(&self) -> Vec<&BlockRow> {
        let mut roots: Vec<&BlockRow> = self
            .rows
            .values()
            .filter(|r| r.parent_id.is_none())
            .collect();
        roots.sort_by(|a, b| {
            a.order_key
                .cmp(&b.order_key)
                .then_with(|| a.block_id.cmp(&b.block_id))
        });
        roots
    }

    pub fn subtree_walk_cte(&self, root: &BlockId) -> Vec<&BlockRow> {
        let mut out = Vec::new();
        if let Some(r) = self.rows.get(root) {
            out.push(r);
            self.walk_children(root, &mut out);
        }
        out
    }

    fn walk_children<'a>(&'a self, parent: &BlockId, out: &mut Vec<&'a BlockRow>) {
        for child in self.children(parent) {
            out.push(child);
            self.walk_children(&child.block_id, out);
        }
    }

    pub fn resolve_sub(&self, block_id: &BlockId) -> Option<&BlockRow> {
        self.rows.get(block_id)
    }

    fn is_descendant_of(&self, maybe_descendant: &BlockId, ancestor: &BlockId) -> bool {
        let mut cur = self
            .rows
            .get(maybe_descendant)
            .and_then(|r| r.parent_id.clone());
        while let Some(p) = cur {
            if &p == ancestor {
                return true;
            }
            cur = self.rows.get(&p).and_then(|r| r.parent_id.clone());
        }
        false
    }

    fn last_child_key(&self, parent: &BlockId) -> Option<OrderKey> {
        self.children(parent).last().map(|r| r.order_key.clone())
    }

    fn bound_key(&self, bound: Option<&BlockId>) -> Result<Option<OrderKey>, TreeError> {
        match bound {
            None => Ok(None),
            Some(b) => self
                .rows
                .get(b)
                .map(|r| Some(r.order_key.clone()))
                .ok_or_else(|| TreeError::NoSuchBlock(b.clone())),
        }
    }

    pub fn subtree_read_is_index_range(parent: &BlockId) -> String {
        children_index_range_sql(parent)
    }
}

pub fn children_index_range_sql(parent: &BlockId) -> String {
    format!(
        "SELECT block_id, parent_id, order_key, block_type \
           FROM block \
          WHERE tenant = $1 AND page_id = $2 AND parent_id = '{}' \
          ORDER BY order_key, block_id",
        parent.as_str()
    )
}

pub fn recursive_subtree_cte_sql(root: &BlockId) -> String {
    format!(
        "WITH RECURSIVE subtree AS ( \
            SELECT block_id, parent_id, order_key, 0 AS depth \
              FROM block \
             WHERE tenant = $1 AND page_id = $2 AND block_id = '{}' \
            UNION ALL \
            SELECT c.block_id, c.parent_id, c.order_key, s.depth + 1 \
              FROM block c \
              JOIN subtree s ON c.parent_id = s.block_id \
             WHERE c.tenant = $1 AND c.page_id = $2 \
         ) SELECT * FROM subtree ORDER BY depth, order_key, block_id",
        root.as_str()
    )
}

#[derive(Clone, Debug, Default)]
pub struct PageTree {
    parent: BTreeMap<PageId, PageId>,
    order: BTreeMap<PageId, OrderKey>,
}

impl PageTree {
    pub fn new() -> PageTree {
        PageTree::default()
    }

    pub fn set_parent(
        &mut self,
        page: PageId,
        parent: PageId,
        order_key: OrderKey,
    ) -> Result<(), TreeError> {
        if page == parent || self.is_ancestor(&page, &parent) {
            return Err(TreeError::WouldCycle(BlockId(page.0)));
        }
        self.parent.insert(page.clone(), parent);
        self.order.insert(page, order_key);
        Ok(())
    }

    pub fn parent_of(&self, page: &PageId) -> Option<&PageId> {
        self.parent.get(page)
    }

    pub fn ancestry(&self, page: &PageId) -> Vec<PageId> {
        let mut chain = Vec::new();
        let mut cur = self.parent.get(page).cloned();
        while let Some(p) = cur {
            chain.push(p.clone());
            cur = self.parent.get(&p).cloned();
        }
        chain.reverse();
        chain
    }

    pub fn sub_pages(&self, parent: &PageId) -> Vec<PageId> {
        let mut kids: Vec<PageId> = self
            .parent
            .iter()
            .filter(|(_, p)| *p == parent)
            .map(|(c, _)| c.clone())
            .collect();
        kids.sort_by(|a, b| {
            let ka = self.order.get(a);
            let kb = self.order.get(b);
            ka.cmp(&kb).then_with(|| a.cmp(b))
        });
        kids
    }

    fn is_ancestor(&self, maybe_ancestor: &PageId, page: &PageId) -> bool {
        let mut cur = self.parent.get(page).cloned();
        while let Some(p) = cur {
            if &p == maybe_ancestor {
                return true;
            }
            cur = self.parent.get(&p).cloned();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jit(a: usize, b: usize) -> Jitter {
        Jitter::from_ranks(a, b).expect("jitter ranks in 0..62")
    }

    fn bid(s: &str) -> BlockId {
        BlockId(s.to_string())
    }

    fn three_child_tree() -> BlockTree {
        let mut t = BlockTree::new(PageId("p1".into()));
        t.insert_root(bid("root"), "paragraph", jit(0, 0)).unwrap();
        t.insert_block(bid("c1"), &bid("root"), "paragraph", jit(0, 1))
            .unwrap();
        t.insert_block(bid("c2"), &bid("root"), "paragraph", jit(0, 2))
            .unwrap();
        t.insert_block(bid("c3"), &bid("root"), "paragraph", jit(0, 3))
            .unwrap();
        t
    }

    #[test]
    fn append_children_are_order_key_sorted() {
        let t = three_child_tree();
        let kids: Vec<&str> = t
            .children(&bid("root"))
            .iter()
            .map(|r| r.block_id.as_str())
            .collect();
        assert_eq!(
            kids,
            vec!["c1", "c2", "c3"],
            "appended children read back in insert order"
        );
        let keys: Vec<&str> = t
            .children(&bid("root"))
            .iter()
            .map(|r| r.order_key.as_str())
            .collect();
        assert!(
            keys.windows(2).all(|w| w[0] < w[1]),
            "order_keys strictly increase: {keys:?}"
        );
    }

    #[test]
    fn between_insert_lands_in_the_gap() {
        let mut t = three_child_tree();
        t.insert_between(
            bid("c1_5"),
            &bid("root"),
            Some(&bid("c1")),
            Some(&bid("c2")),
            "paragraph",
            jit(1, 1),
        )
        .unwrap();
        let kids: Vec<&str> = t
            .children(&bid("root"))
            .iter()
            .map(|r| r.block_id.as_str())
            .collect();
        assert_eq!(
            kids,
            vec!["c1", "c1_5", "c2", "c3"],
            "the between-insert lands in the gap"
        );
    }

    #[test]
    fn concurrent_same_gap_inserts_distinct() {
        let t = three_child_tree();
        let lo = t.get(&bid("c1")).unwrap().order_key.clone();
        let hi = t.get(&bid("c2")).unwrap().order_key.clone();
        let a = OrderKey::rank_between(Some(&lo), Some(&hi), jit(5, 5));
        let b = OrderKey::rank_between(Some(&lo), Some(&hi), jit(6, 6));
        assert_ne!(
            a.as_str(),
            b.as_str(),
            "the 2-char jitter makes same-gap inserts distinct"
        );
        assert!(lo < a && a < hi, "A in the gap");
        assert!(lo < b && b < hi, "B in the gap");
    }

    #[test]
    fn moved_block_keeps_its_id_zero_dangles() {
        let mut t = three_child_tree();
        t.insert_block(bid("nested"), &bid("c1"), "paragraph", jit(0, 0))
            .unwrap();

        let before = t
            .resolve_sub(&bid("nested"))
            .expect("embed resolves before move")
            .clone();
        let id_before = before.block_id.clone();
        let key_before = before.order_key.clone();
        let parent_before = before.parent_id.clone();

        t.move_block(&bid("nested"), &bid("c3"), None, None, jit(2, 2))
            .unwrap();

        let after = t
            .resolve_sub(&bid("nested"))
            .expect("embed STILL resolves after move");
        assert_eq!(
            after.block_id, id_before,
            "the block_id is STABLE across the move (0 dangles)"
        );
        assert_eq!(after.parent_id, Some(bid("c3")), "the parent changed");
        assert_ne!(after.parent_id, parent_before, "the parent actually moved");
        assert_ne!(
            after.order_key, key_before,
            "the order_key was rewritten by the move"
        );
        let c3_kids: Vec<&str> = t
            .children(&bid("c3"))
            .iter()
            .map(|r| r.block_id.as_str())
            .collect();
        assert_eq!(c3_kids, vec!["nested"], "nested is now under c3");
        assert!(t.children(&bid("c1")).is_empty(), "nested left c1");
    }

    #[test]
    fn move_into_own_subtree_is_refused() {
        let mut t = three_child_tree();
        t.insert_block(bid("grandchild"), &bid("c1"), "paragraph", jit(0, 0))
            .unwrap();
        let err = t
            .move_block(&bid("c1"), &bid("grandchild"), None, None, jit(0, 0))
            .unwrap_err();
        assert_eq!(err, TreeError::WouldCycle(bid("c1")));
        assert_eq!(
            t.move_block(&bid("c1"), &bid("c1"), None, None, jit(0, 0))
                .unwrap_err(),
            TreeError::WouldCycle(bid("c1"))
        );
    }

    #[test]
    fn recursive_subtree_walk_returns_whole_subtree() {
        let mut t = three_child_tree();
        t.insert_block(bid("g1"), &bid("c1"), "paragraph", jit(0, 0))
            .unwrap();
        t.insert_block(bid("gg1"), &bid("g1"), "paragraph", jit(0, 0))
            .unwrap();

        let walk: Vec<&str> = t
            .subtree_walk_cte(&bid("root"))
            .iter()
            .map(|r| r.block_id.as_str())
            .collect();
        assert_eq!(walk, vec!["root", "c1", "g1", "gg1", "c2", "c3"]);

        let sql = recursive_subtree_cte_sql(&bid("root"));
        assert!(
            sql.contains("WITH RECURSIVE"),
            "the deep walk is a recursive CTE: {sql}"
        );
        assert!(
            sql.contains("JOIN subtree"),
            "the recursive arm joins on parent_id: {sql}"
        );
        let sub: Vec<&str> = t
            .subtree_walk_cte(&bid("c1"))
            .iter()
            .map(|r| r.block_id.as_str())
            .collect();
        assert_eq!(sub, vec!["c1", "g1", "gg1"]);
    }

    #[test]
    fn subtree_read_is_an_index_range_not_a_scan() {
        let sql = BlockTree::subtree_read_is_index_range(&bid("root"));
        assert!(
            sql.contains("parent_id = 'root'"),
            "equality on parent_id (index range): {sql}"
        );
        assert!(
            sql.contains("ORDER BY order_key"),
            "ordered by the index's order_key column: {sql}"
        );
        assert!(
            sql.contains("tenant = $1 AND page_id = $2"),
            "leading partition columns pinned: {sql}"
        );
        assert!(
            !sql.contains("WHERE TRUE"),
            "the read is bounded, never a full scan"
        );
        let t = three_child_tree();
        let ordered: Vec<&str> = t
            .children(&bid("root"))
            .iter()
            .map(|r| r.block_id.as_str())
            .collect();
        assert_eq!(
            ordered,
            vec!["c1", "c2", "c3"],
            "the children read is order_key-ordered"
        );
    }

    #[test]
    fn page_hierarchy_nests_and_walks_ancestry() {
        let mut pages = PageTree::new();
        pages
            .set_parent(
                PageId("team".into()),
                PageId("root".into()),
                OrderKey::rank_first(jit(0, 0)),
            )
            .unwrap();
        pages
            .set_parent(
                PageId("project".into()),
                PageId("team".into()),
                OrderKey::rank_first(jit(0, 0)),
            )
            .unwrap();
        pages
            .set_parent(
                PageId("wiki".into()),
                PageId("team".into()),
                OrderKey::rank_last(None, jit(1, 0)),
            )
            .unwrap();

        assert_eq!(
            pages.parent_of(&PageId("project".into())),
            Some(&PageId("team".into()))
        );
        assert_eq!(
            pages.ancestry(&PageId("project".into())),
            vec![PageId("root".into()), PageId("team".into())]
        );
        let subs: Vec<String> = pages
            .sub_pages(&PageId("team".into()))
            .iter()
            .map(|p| p.0.clone())
            .collect();
        assert_eq!(subs, vec!["project".to_string(), "wiki".to_string()]);
    }

    #[test]
    fn page_cycle_is_refused() {
        let mut pages = PageTree::new();
        pages
            .set_parent(
                PageId("b".into()),
                PageId("a".into()),
                OrderKey::rank_first(jit(0, 0)),
            )
            .unwrap();
        let err = pages
            .set_parent(
                PageId("a".into()),
                PageId("b".into()),
                OrderKey::rank_first(jit(0, 0)),
            )
            .unwrap_err();
        assert!(matches!(err, TreeError::WouldCycle(_)));
    }

    #[test]
    fn duplicate_and_unknown_parent_refused() {
        let mut t = three_child_tree();
        assert_eq!(
            t.insert_block(bid("c1"), &bid("root"), "paragraph", jit(0, 0))
                .unwrap_err(),
            TreeError::DuplicateBlockId(bid("c1"))
        );
        assert_eq!(
            t.insert_block(bid("new"), &bid("ghost"), "paragraph", jit(0, 0))
                .unwrap_err(),
            TreeError::NoSuchBlock(bid("ghost"))
        );
    }
}
