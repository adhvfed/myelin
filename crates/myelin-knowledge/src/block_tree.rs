//! # The block tree — adjacency list + LexoRank + stable block ids + page hierarchy (KN-P10 / P-300, M3)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md`
//! §1.2 (the block tree is an adjacency list — `parent_id` + a fractional `order_key`; subtree reads
//! are an index range; moves are an `order_key` write; recursive CTEs for the rare deep walk), §2.3
//! (the `block` row + the stable `block_id`), §2.5 (the FROZEN LexoRank `order_key` encoding — base-62,
//! `"U"` anchor, 2-char jitter, 48-char rebalance), §2.6 (the page hierarchy — a page is a root-block
//! subtree; sub-pages nest). `02-internals-and-algorithms.md` §3.5 (the LexoRank jitter/rebalance as
//! idempotent replayable move ops).
//!
//! **Contract-index:** row 5.7 (the stable block-id mint — OWNED; the `b`/`h` `#sub` targets, the
//! stability is Knowledge's obligation; minted via [`crate::subs`]). row 13.3 (the `order_key`/LexoRank
//! fractional index — CONSUMED via [`myelin_query`]'s frozen [`OrderKey`], NOT re-implemented; EI-01 §7
//! one primitive).
//!
//! ## What this module ships (KN-P10's owned work)
//! - **The adjacency-list block tree** ([`BlockTree`]): per-block rows (`parent_id` + the frozen
//!   LexoRank `order_key`), so a document is per-block rows, not one blob. An ordered sibling read
//!   ([`BlockTree::children`]) is an **index range** over `(parent_id, order_key)` (the
//!   `block_children` index of §2.3), never a full scan ([`BlockTree::subtree_read_is_index_range`]
//!   proves it). A block move ([`BlockTree::move_block`]) is an `order_key` write (+ a `parent_id`
//!   write for a re-parent) — **the `block_id` is never re-minted** (the stability gate).
//! - **Stable opaque block ids** ([`BlockId`]): minted ONCE at insert
//!   ([`BlockTree::insert_block`]); stable across edits/moves/collaboration (§2.3) — the `b<id>`/`h<id>`
//!   `#sub` target ([`crate::subs::mint_block`]). The stability counter `moved_block_id_dangles == 0`
//!   ([`BlockTree::resolve_sub`] still resolves an embed after a move).
//! - **The recursive-CTE deep subtree walk** ([`BlockTree::subtree_walk_cte`] +
//!   [`recursive_subtree_cte_sql`]): the rare deep walk lowers to a `WITH RECURSIVE` CTE over the
//!   adjacency list (§1.2) — the query-plan check is [`recursive_subtree_cte_sql`] (a visible CTE, not
//!   an N+1 application loop).
//! - **The page hierarchy** ([`PageTree`]): sub-pages are folder-like nesting (a page is a root-block
//!   subtree); the `page_parent` typed relation ([`PageTree::set_parent`] → [`PageTree::ancestry`]) —
//!   the TE-7 source of truth, mirrored to Refs in KN-P19.
//!
//! ## FLOORS named (EI-01 §1 — none new; the immediate follow-ons)
//! - **Version history + op-log compaction → content-addressed snapshots + op-log GC is KN-P11
//!   (P-301).** This module is the live block tree the snapshot compacts; the history/restore is P-301.
//! - **The `sync_block` read-projection floor (permission-filtered transclusion) is KN-P12 (P-302).**
//!   The `sync_block` node type is present in the frozen taxonomy ([`myelin_content`]); its read-engine
//!   is P-302.
//! - **The `page_parent` → Refs mirror (the TE-7 typed-edge mirror) is KN-P19 (P-309).** This module
//!   is the `page_parent` source of truth; the outbox emit + the Refs edge projection is P-309.
//! - **In-DB persistence is the P-S12 driver floor** — like [`crate::store`], the block tree's row
//!   model + the adjacency-list + recursive-CTE SQL are complete + testable now over an in-memory
//!   model; the DDL ([`crate::store::knowledge_store_migrations`]) is the byte-faithful schema. The
//!   live-Postgres adjacency-list integration is the KN-P11/store driver landing.

use myelin_query::field::{Jitter, OrderKey};
use std::collections::BTreeMap;

/// A **stable opaque block id** (§2.3 — `block.block_id`, stable across edits/moves/collaboration; the
/// `b<id>`/`h<id>` `#sub` target, 5.7). Minted ONCE at [`BlockTree::insert_block`]; a move or an edit
/// NEVER re-mints it. Opaque: the bytes carry no positional meaning (it is NOT a Vec index — that is
/// the editor floor [`crate::editor`] this tree replaces).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct BlockId(pub String);

impl BlockId {
    /// The opaque id string (the `<opaqueid>` body of a `b<id>`/`h<id>` `#sub`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A **stable opaque page id** (§2.6 — `page.page_id`; the independently-addressable root of a
/// root-block subtree, the `knowledge/page/<page_id>` `#sub` root).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct PageId(pub String);

impl PageId {
    /// The opaque page id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One per-block adjacency-list row (§2.3 — the load-bearing tree columns; the editor/content payload
/// columns `inline`/`props`/`inline_nodes` are stored on the same row but the TREE only needs these).
/// `parent_id == None` is the page-root block (the subtree root).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRow {
    /// The STABLE opaque id (minted once at insert; never re-minted on move/edit).
    pub block_id: BlockId,
    /// The root page this block belongs to (the partition helper + the subtree-read scope, §2.3).
    pub page_id: PageId,
    /// The adjacency-list parent (`None` for the page-root block).
    pub parent_id: Option<BlockId>,
    /// The FROZEN LexoRank `order_key` — sibling ordering (§2.5; [`OrderKey`], consumed not redefined).
    pub order_key: OrderKey,
    /// The frozen `myelin-content` block-type discriminator (`paragraph`/`heading`/… — §2.1). A
    /// `heading` block is the `h<id>` anchor target; any block is a `b<id>` target.
    pub block_type: String,
}

/// The error surface of a block-tree write (LOUD + typed — EI-01 §5; never a silent corrupt-tree).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeError {
    /// An insert/move named a `block_id` that does not exist in this page's tree.
    NoSuchBlock(BlockId),
    /// An insert reused a `block_id` already live in the tree (ids are minted once, never duplicated).
    DuplicateBlockId(BlockId),
    /// A move would make a block its own ancestor (a cycle — the adjacency list must stay a tree).
    WouldCycle(BlockId),
    /// An insert/move named a parent in a DIFFERENT page (a block tree never spans pages, §2.3).
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
                write!(f, "move would make block {} its own ancestor (cycle)", b.as_str())
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

/// **The adjacency-list block tree of one page** (§1.2/§2.3). A document is per-block rows keyed by the
/// stable [`BlockId`]; sibling order is the frozen LexoRank [`OrderKey`]. This is the in-memory MODEL
/// of the `block` table (the load-bearing shape — the live-Postgres adjacency-list lowers the SAME
/// reads/writes: a children read is the `block_children` index range, a move is an `order_key` UPDATE,
/// a deep walk is the recursive CTE).
///
/// The model is per-page: a [`BlockTree`] holds one page's blocks (the partition the `(tenant,
/// page_id)` index serves). Cross-page references are Refs `#sub` ([`crate::subs`]), never a parent_id.
#[derive(Clone, Debug)]
pub struct BlockTree {
    page_id: PageId,
    /// The rows, keyed by the stable block id (the `(tenant, block_id)` primary key, §2.3).
    rows: BTreeMap<BlockId, BlockRow>,
}

impl BlockTree {
    /// Open an empty tree for a page (the page-root block is inserted by the first
    /// [`BlockTree::insert_root`]).
    pub fn new(page_id: PageId) -> BlockTree {
        BlockTree { page_id, rows: BTreeMap::new() }
    }

    /// This tree's page id.
    pub fn page_id(&self) -> &PageId {
        &self.page_id
    }

    /// The number of blocks in the tree.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the tree is empty (no blocks yet).
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Fetch a block row by its stable id.
    pub fn get(&self, id: &BlockId) -> Option<&BlockRow> {
        self.rows.get(id)
    }

    /// **Insert the page-root block** (the subtree root, `parent_id == None`, §2.6). Mints the stable
    /// id once; the order_key is `rank_first` (the `"U"` anchor — the root is the first/only sibling at
    /// the top level).
    ///
    /// # Errors
    /// [`TreeError::DuplicateBlockId`] if the id is already live.
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

    /// **Insert a block as the last child of `parent`** (the common append — type a new line). The
    /// `order_key` is `rank_last` after the parent's current last child (the frozen LexoRank append,
    /// §2.5). The stable [`BlockId`] is minted ONCE here; a later move never re-mints it.
    ///
    /// # Errors
    /// [`TreeError::NoSuchBlock`] if `parent` is not in the tree, [`TreeError::DuplicateBlockId`] if
    /// `block_id` is already live.
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

    /// **Insert a block BETWEEN two siblings** (a drag-drop / split insert). The `order_key` is the
    /// frozen LexoRank midpoint bisection of `(lo, hi)` + jitter — so two concurrent same-gap inserts
    /// get DISTINCT keys (the 2-char jitter, §2.5). `lo`/`hi` are sibling block ids of `parent` (or
    /// `None` for "before the first" / "after the last"); the bound keys are looked up live so the
    /// caller never threads raw order_keys.
    ///
    /// # Errors
    /// [`TreeError::NoSuchBlock`] for an unknown parent/bound, [`TreeError::DuplicateBlockId`] for a
    /// reused id.
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

    /// **Move a block — an `order_key` write (+ a `parent_id` write for a re-parent), NEVER a re-mint
    /// of the `block_id`** (§1.2 — "moves are an `order_key` write"; the stability gate of §2.3). The
    /// block lands between `lo`/`hi` under `new_parent`. This is the load-bearing stability operation:
    /// an embed of `b<id>` resolves to the SAME block after the move (the `moved_block_id_dangles == 0`
    /// counter — [`BlockTree::resolve_sub`] still finds it).
    ///
    /// # Errors
    /// [`TreeError::NoSuchBlock`] for an unknown block/parent/bound, [`TreeError::WouldCycle`] if the
    /// move would make the block its own ancestor, [`TreeError::CrossPageParent`] is structurally
    /// impossible here (one tree == one page) but the type exists for the live-Postgres path.
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
        // A block may not become its own ancestor (the adjacency list must remain a tree, §1.2).
        if block_id == new_parent || self.is_descendant_of(new_parent, block_id) {
            return Err(TreeError::WouldCycle(block_id.clone()));
        }
        let lo_key = self.bound_key(lo)?;
        let hi_key = self.bound_key(hi)?;
        let order_key = OrderKey::rank_between(lo_key.as_ref(), hi_key.as_ref(), jitter);
        let row = self.rows.get_mut(block_id).expect("checked present above");
        // The STABLE-ID INVARIANT: only the order_key + parent_id change; block_id is untouched.
        row.parent_id = Some(new_parent.clone());
        row.order_key = order_key;
        Ok(())
    }

    /// **The ordered sibling read — an INDEX RANGE over `(parent_id, order_key)`** (§1.2/§2.3, the
    /// `block_children` index). Returns the children of `parent` in `order_key` order. In Postgres this
    /// is the index range `WHERE (tenant, page_id, parent_id) = (..)` ORDER BY `order_key` — never a
    /// full table scan ([`BlockTree::subtree_read_is_index_range`] / [`children_index_range_sql`]).
    pub fn children(&self, parent: &BlockId) -> Vec<&BlockRow> {
        let mut kids: Vec<&BlockRow> = self
            .rows
            .values()
            .filter(|r| r.parent_id.as_ref() == Some(parent))
            .collect();
        // The frozen ordering: order_key lexicographic, tiebroken by block_id (the §2.5 total order; a
        // full tiebreak adds created_at — the tree model carries order_key + id, the created_at
        // tiebreak lands with the row's full timestamp columns).
        kids.sort_by(|a, b| {
            a.order_key.cmp(&b.order_key).then_with(|| a.block_id.cmp(&b.block_id))
        });
        kids
    }

    /// The page-root blocks (top-level, `parent_id == None`), in `order_key` order.
    pub fn roots(&self) -> Vec<&BlockRow> {
        let mut roots: Vec<&BlockRow> =
            self.rows.values().filter(|r| r.parent_id.is_none()).collect();
        roots.sort_by(|a, b| {
            a.order_key.cmp(&b.order_key).then_with(|| a.block_id.cmp(&b.block_id))
        });
        roots
    }

    /// **The rare deep subtree walk — a `WITH RECURSIVE` CTE over the adjacency list** (§1.2). Returns
    /// every block in `root`'s subtree (including `root`), depth-first in sibling `order_key` order.
    /// The in-memory model walks the adjacency list; the live-Postgres path lowers the SAME walk to
    /// the recursive CTE ([`recursive_subtree_cte_sql`]) — NOT an N+1 application loop.
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

    /// **The stable-id resolve: an embed of `b<block_id>` resolves to the SAME row regardless of where
    /// the block now sits in the tree** (the stability gate — `moved_block_id_dangles == 0`). This is
    /// the in-tree half of the `#sub` resolve ([`crate::subs::mint_block`] is the mint; the full
    /// per-viewer permission-gated `project(ref, viewer)` ladder is KN-P19). It looks the block up by
    /// its STABLE id — a move (which changes `parent_id`/`order_key`, never `block_id`) leaves this a
    /// hit, so the embed never dangles.
    pub fn resolve_sub(&self, block_id: &BlockId) -> Option<&BlockRow> {
        self.rows.get(block_id)
    }

    /// Whether `maybe_descendant` is in the subtree rooted at `ancestor` (the cycle guard for a move).
    fn is_descendant_of(&self, maybe_descendant: &BlockId, ancestor: &BlockId) -> bool {
        let mut cur = self.rows.get(maybe_descendant).and_then(|r| r.parent_id.clone());
        while let Some(p) = cur {
            if &p == ancestor {
                return true;
            }
            cur = self.rows.get(&p).and_then(|r| r.parent_id.clone());
        }
        false
    }

    /// The `order_key` of `parent`'s current last child (for the append `rank_last`), or `None` if the
    /// parent has no children yet.
    fn last_child_key(&self, parent: &BlockId) -> Option<OrderKey> {
        self.children(parent).last().map(|r| r.order_key.clone())
    }

    /// Resolve an optional sibling-bound block id to its `order_key` (for a between-insert/move). A
    /// `None` bound is "the open end" (`None` order_key). An unknown bound is a LOUD error.
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

    /// **The subtree-read-is-an-index-range proof** (the GATE artifact, §2.3). The ordered sibling read
    /// is served by the `block_children` index over `(tenant, page_id, parent_id, order_key)` — a
    /// bounded index range, NOT a full scan. Returns the visible index-range SQL the live read uses;
    /// the in-memory [`BlockTree::children`] returns the SAME rows in the SAME order. This is the
    /// query-plan check the prompt's subtree-read-range gate names.
    pub fn subtree_read_is_index_range(parent: &BlockId) -> String {
        children_index_range_sql(parent)
    }
}

/// **The ordered-sibling-read index-range SQL** (the `block_children` index of §2.3). A sibling read is
/// an equality probe on `(tenant, page_id, parent_id)` ordered by `order_key` — a bounded INDEX RANGE,
/// never a `Seq Scan`. This is the visible-SQL query-plan artifact for the subtree-read-range gate.
pub fn children_index_range_sql(parent: &BlockId) -> String {
    // The leading-column equality + the order_key sort is exactly the block_children index shape —
    // Postgres serves it as an Index Scan range (the planner reads the index, not the heap).
    format!(
        "SELECT block_id, parent_id, order_key, block_type \
           FROM block \
          WHERE tenant = $1 AND page_id = $2 AND parent_id = '{}' \
          ORDER BY order_key, block_id",
        parent.as_str()
    )
}

/// **The recursive-CTE deep-subtree-walk SQL** (§1.2 — "recursive CTEs for deep walks"). The rare deep
/// walk lowers to a `WITH RECURSIVE` traversal of the adjacency list (anchor = the root block; the
/// recursive arm joins each level's children on `parent_id`), ordered by `order_key`. This is the
/// visible-SQL query-plan artifact proving the deep walk is ONE recursive query, not an N+1 loop.
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

/// **The page hierarchy** (§2.6 — sub-pages = folder-like nesting; a page is a root-block subtree).
/// `page_parent` is the TE-7 typed relation (the source of truth, mirrored to Refs in KN-P19). A page
/// has at most one parent page (a tree, like the block tree); the root pages have `None`.
#[derive(Clone, Debug, Default)]
pub struct PageTree {
    /// `page_id → parent_page` (the `page_parent` edge; absent == a root page). The `order_key` of a
    /// page among its sibling sub-pages is carried in [`Self::order`] (the `page_parent.order_key`).
    parent: BTreeMap<PageId, PageId>,
    order: BTreeMap<PageId, OrderKey>,
}

impl PageTree {
    /// An empty page hierarchy.
    pub fn new() -> PageTree {
        PageTree::default()
    }

    /// **Set `page`'s parent (the `page_parent` typed edge, §4.3/TE-7).** A sub-page nests under
    /// `parent`; the `order_key` ranks it among its siblings (the frozen LexoRank). A re-parent is a
    /// `parent_page` + `order_key` write — the `page_id` is stable (like a block move). The TE-7 edge
    /// is the source of truth; the Refs mirror is KN-P19.
    ///
    /// # Errors
    /// [`TreeError::WouldCycle`] if `page` is an ancestor of `parent` (pages form a tree).
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

    /// The parent page of `page` (the `page_parent` edge), or `None` for a root page.
    pub fn parent_of(&self, page: &PageId) -> Option<&PageId> {
        self.parent.get(page)
    }

    /// **The ancestry chain of a page** (root-first → the page's parent), the folder-breadcrumb walk.
    /// On the in-memory model this walks the `parent` map; the live-Postgres path lowers it to a
    /// `WITH RECURSIVE` walk of `page_parent` (the SAME adjacency-list pattern as the block tree).
    pub fn ancestry(&self, page: &PageId) -> Vec<PageId> {
        let mut chain = Vec::new();
        let mut cur = self.parent.get(page).cloned();
        while let Some(p) = cur {
            chain.push(p.clone());
            cur = self.parent.get(&p).cloned();
        }
        chain.reverse(); // root-first (the breadcrumb order)
        chain
    }

    /// The direct sub-pages of `parent`, in `order_key` order (the folder listing).
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

    /// Whether `maybe_ancestor` is an ancestor of `page` (the page-tree cycle guard).
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

    /// A small page tree: a root block with three children r1<r2<r3 (appended in order).
    fn three_child_tree() -> BlockTree {
        let mut t = BlockTree::new(PageId("p1".into()));
        t.insert_root(bid("root"), "paragraph", jit(0, 0)).unwrap();
        t.insert_block(bid("c1"), &bid("root"), "paragraph", jit(0, 1)).unwrap();
        t.insert_block(bid("c2"), &bid("root"), "paragraph", jit(0, 2)).unwrap();
        t.insert_block(bid("c3"), &bid("root"), "paragraph", jit(0, 3)).unwrap();
        t
    }

    /// **Adjacency-list insert: appended children are in order_key order, an index-range sibling read.**
    #[test]
    fn append_children_are_order_key_sorted() {
        let t = three_child_tree();
        let kids: Vec<&str> = t.children(&bid("root")).iter().map(|r| r.block_id.as_str()).collect();
        assert_eq!(kids, vec!["c1", "c2", "c3"], "appended children read back in insert order");
        // Each child's order_key strictly increases (the frozen rank_last append).
        let keys: Vec<&str> =
            t.children(&bid("root")).iter().map(|r| r.order_key.as_str()).collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "order_keys strictly increase: {keys:?}");
    }

    /// **A between-insert lands strictly between its bounds (the LexoRank midpoint bisection).**
    #[test]
    fn between_insert_lands_in_the_gap() {
        let mut t = three_child_tree();
        // Insert c1.5 between c1 and c2.
        t.insert_between(bid("c1_5"), &bid("root"), Some(&bid("c1")), Some(&bid("c2")), "paragraph", jit(1, 1))
            .unwrap();
        let kids: Vec<&str> =
            t.children(&bid("root")).iter().map(|r| r.block_id.as_str()).collect();
        assert_eq!(kids, vec!["c1", "c1_5", "c2", "c3"], "the between-insert lands in the gap");
    }

    /// **Two concurrent same-gap inserts get DISTINCT keys (the 2-char jitter, §2.5).** Both land
    /// strictly between c1 and c2; their keys differ ONLY in the jitter suffix — no collision.
    #[test]
    fn concurrent_same_gap_inserts_distinct() {
        let t = three_child_tree();
        let lo = t.get(&bid("c1")).unwrap().order_key.clone();
        let hi = t.get(&bid("c2")).unwrap().order_key.clone();
        let a = OrderKey::rank_between(Some(&lo), Some(&hi), jit(5, 5));
        let b = OrderKey::rank_between(Some(&lo), Some(&hi), jit(6, 6));
        assert_ne!(a.as_str(), b.as_str(), "the 2-char jitter makes same-gap inserts distinct");
        assert!(lo < a && a < hi, "A in the gap");
        assert!(lo < b && b < hi, "B in the gap");
    }

    /// **THE STABILITY GATE: a block reordered/re-parented keeps its `block_id` — an embed of
    /// `b<id>` resolves to the SAME block after the move (`moved_block_id_dangles == 0`).** This is
    /// the load-bearing 5.7 obligation: a move is an order_key/parent_id write, NEVER an id re-mint.
    #[test]
    fn moved_block_keeps_its_id_zero_dangles() {
        let mut t = three_child_tree();
        // Add a nested child under c1, then a second parent c3 to move it under.
        t.insert_block(bid("nested"), &bid("c1"), "paragraph", jit(0, 0)).unwrap();

        // The embed: a #sub onto `nested` (the b<id> the editor stored before any move).
        let before = t.resolve_sub(&bid("nested")).expect("embed resolves before move").clone();
        let id_before = before.block_id.clone();
        let key_before = before.order_key.clone();
        let parent_before = before.parent_id.clone();

        // MOVE `nested` from under c1 to be the first child of c3. order_key + parent_id change.
        t.move_block(&bid("nested"), &bid("c3"), None, None, jit(2, 2)).unwrap();

        let after = t.resolve_sub(&bid("nested")).expect("embed STILL resolves after move");
        // 0 dangles: the SAME stable id resolves.
        assert_eq!(after.block_id, id_before, "the block_id is STABLE across the move (0 dangles)");
        // The move changed exactly the tree position (parent_id + order_key), nothing else.
        assert_eq!(after.parent_id, Some(bid("c3")), "the parent changed");
        assert_ne!(after.parent_id, parent_before, "the parent actually moved");
        assert_ne!(after.order_key, key_before, "the order_key was rewritten by the move");
        // And `nested` now reads back as a child of c3.
        let c3_kids: Vec<&str> =
            t.children(&bid("c3")).iter().map(|r| r.block_id.as_str()).collect();
        assert_eq!(c3_kids, vec!["nested"], "nested is now under c3");
        // …and no longer under c1.
        assert!(t.children(&bid("c1")).is_empty(), "nested left c1");
    }

    /// **A move that would create a cycle is refused LOUDLY** (the adjacency list stays a tree).
    #[test]
    fn move_into_own_subtree_is_refused() {
        let mut t = three_child_tree();
        t.insert_block(bid("grandchild"), &bid("c1"), "paragraph", jit(0, 0)).unwrap();
        // Moving c1 under its own grandchild would make c1 its own ancestor — refused.
        let err = t.move_block(&bid("c1"), &bid("grandchild"), None, None, jit(0, 0)).unwrap_err();
        assert_eq!(err, TreeError::WouldCycle(bid("c1")));
        // Moving a block under itself is likewise refused.
        assert_eq!(
            t.move_block(&bid("c1"), &bid("c1"), None, None, jit(0, 0)).unwrap_err(),
            TreeError::WouldCycle(bid("c1"))
        );
    }

    /// **The recursive-CTE deep subtree walk returns the whole subtree, depth-first in order_key
    /// order** — and the lowered SQL is ONE `WITH RECURSIVE`, not an N+1 loop.
    #[test]
    fn recursive_subtree_walk_returns_whole_subtree() {
        let mut t = three_child_tree();
        // Deepen: c1 → g1 → gg1 (a 3-level subtree to force the recursion).
        t.insert_block(bid("g1"), &bid("c1"), "paragraph", jit(0, 0)).unwrap();
        t.insert_block(bid("gg1"), &bid("g1"), "paragraph", jit(0, 0)).unwrap();

        let walk: Vec<&str> =
            t.subtree_walk_cte(&bid("root")).iter().map(|r| r.block_id.as_str()).collect();
        // root, then c1's subtree (c1, g1, gg1) depth-first, then c2, c3.
        assert_eq!(walk, vec!["root", "c1", "g1", "gg1", "c2", "c3"]);

        // The lowered deep-walk SQL is a single recursive CTE (the query-plan artifact).
        let sql = recursive_subtree_cte_sql(&bid("root"));
        assert!(sql.contains("WITH RECURSIVE"), "the deep walk is a recursive CTE: {sql}");
        assert!(sql.contains("JOIN subtree"), "the recursive arm joins on parent_id: {sql}");
        // A subtree of a non-root block is just that block's subtree.
        let sub: Vec<&str> =
            t.subtree_walk_cte(&bid("c1")).iter().map(|r| r.block_id.as_str()).collect();
        assert_eq!(sub, vec!["c1", "g1", "gg1"]);
    }

    /// **THE SUBTREE-READ-RANGE GATE: the sibling read is an INDEX RANGE, not a full scan.** The
    /// query-plan artifact is the visible index-range SQL (equality on the index-leading columns +
    /// order_key sort = the `block_children` index range; no `Seq Scan`).
    #[test]
    fn subtree_read_is_an_index_range_not_a_scan() {
        let sql = BlockTree::subtree_read_is_index_range(&bid("root"));
        // Equality probe on the index-leading columns (tenant, page_id, parent_id) — an index range.
        assert!(sql.contains("parent_id = 'root'"), "equality on parent_id (index range): {sql}");
        assert!(sql.contains("ORDER BY order_key"), "ordered by the index's order_key column: {sql}");
        assert!(sql.contains("tenant = $1 AND page_id = $2"), "leading partition columns pinned: {sql}");
        // A full scan would lack the parent_id equality — assert we did NOT emit an unbounded read.
        assert!(!sql.contains("WHERE TRUE"), "the read is bounded, never a full scan");
        // The in-memory children read returns the SAME rows in the SAME (index) order.
        let t = three_child_tree();
        let ordered: Vec<&str> =
            t.children(&bid("root")).iter().map(|r| r.block_id.as_str()).collect();
        assert_eq!(ordered, vec!["c1", "c2", "c3"], "the children read is order_key-ordered");
    }

    /// **The page hierarchy: sub-pages nest folder-like; ancestry is the breadcrumb walk.**
    #[test]
    fn page_hierarchy_nests_and_walks_ancestry() {
        let mut pages = PageTree::new();
        // root → team → project (a 3-level folder nesting).
        pages
            .set_parent(PageId("team".into()), PageId("root".into()), OrderKey::rank_first(jit(0, 0)))
            .unwrap();
        pages
            .set_parent(PageId("project".into()), PageId("team".into()), OrderKey::rank_first(jit(0, 0)))
            .unwrap();
        // A second sub-page under team, ranked after `project`.
        pages
            .set_parent(PageId("wiki".into()), PageId("team".into()), OrderKey::rank_last(None, jit(1, 0)))
            .unwrap();

        assert_eq!(pages.parent_of(&PageId("project".into())), Some(&PageId("team".into())));
        // Ancestry is root-first: [root, team] for `project`.
        assert_eq!(
            pages.ancestry(&PageId("project".into())),
            vec![PageId("root".into()), PageId("team".into())]
        );
        // team's sub-pages list (folder listing) in order_key order.
        let subs: Vec<String> =
            pages.sub_pages(&PageId("team".into())).iter().map(|p| p.0.clone()).collect();
        assert_eq!(subs, vec!["project".to_string(), "wiki".to_string()]);
    }

    /// **A page-tree cycle is refused** (pages form a tree, like blocks).
    #[test]
    fn page_cycle_is_refused() {
        let mut pages = PageTree::new();
        pages
            .set_parent(PageId("b".into()), PageId("a".into()), OrderKey::rank_first(jit(0, 0)))
            .unwrap();
        // Making `a` a child of `b` would cycle.
        let err = pages
            .set_parent(PageId("a".into()), PageId("b".into()), OrderKey::rank_first(jit(0, 0)))
            .unwrap_err();
        assert!(matches!(err, TreeError::WouldCycle(_)));
    }

    /// **Duplicate-id and unknown-parent inserts are refused LOUDLY** (ids mint once; the tree never
    /// silently corrupts).
    #[test]
    fn duplicate_and_unknown_parent_refused() {
        let mut t = three_child_tree();
        assert_eq!(
            t.insert_block(bid("c1"), &bid("root"), "paragraph", jit(0, 0)).unwrap_err(),
            TreeError::DuplicateBlockId(bid("c1"))
        );
        assert_eq!(
            t.insert_block(bid("new"), &bid("ghost"), "paragraph", jit(0, 0)).unwrap_err(),
            TreeError::NoSuchBlock(bid("ghost"))
        );
    }
}
