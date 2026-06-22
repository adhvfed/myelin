//! # `comments` — KB-native comment threads over the shared `#thread-`/`#comment-` `#sub` grammar
//! (KN-P23 / P-313, M3 · Floor 4: one scheme, two stores with Chat)
//!
//! **Owning architecture doc (read in full before changing this):**
//! `04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md`
//! §1.5 (a comment is a sub-artifact `#comment-`/`#thread-` — **the same `#sub` grammar as Chat**,
//! OQ-L: one scheme, two stores; creating/resolving emits `knowledge.comment.created`/`.resolved`).
//! Also `04-views-cli-and-api.md` §1.5 (inline comments anchored to a text range or block; v1 ships a
//! **KB-native comment thread** — its own store — using the **shared design-system thread render
//! component**, CR-G: KB-native anchor data + shared thread render).
//!
//! **Reconciliation:** `00-reconciliation-decisions.md` OQ-L (the comments one-scheme-two-stores
//! with Chat; the consolidation onto the Chat threading primitive + firehose transport is the named
//! follow-on, post-M5, KQ-9).
//!
//! **Contracts:** `contract-index.md` row 5.7 (the unified `#sub` scheme — `comment-`/`thread-`
//! stable opaque ids minted by each owner; Refs owns the grammar + the 4-step tombstone ladder).
//!
//! ## What this module ships (the genuinely-new KN-P23 work)
//! The shared `#sub` grammar (the `Comment`/`Thread` kinds) ALREADY exists in [`myelin_refs`]; the
//! `knowledge.comment.created`/`.resolved` event tokens + the `CommentCreated`/`CommentResolved`
//! [`crate::emit::KnowledgeChange`] variants ALREADY exist (KN-P06 / P-296). This module is the
//! **KB-native comment STORE + the anchor model** that did not exist — reconciled-in-place with both
//! (EI-01 §7): it owns the `#comment-`/`#thread-` mint codecs through the ONE Refs grammar, the
//! [`CommentAnchor`] (anchored to a STABLE `block_id` or a `block_id`+text range — survives a block
//! move), the [`CommentThread`]/[`Comment`] model whose body is the [`myelin_content`] AST, and the
//! [`create_comment`]/[`resolve_comment`] ops that emit through the [`crate::emit`] outbox seam.
//!
//! ## The anchor-survives-a-move invariant (the comment-anchor gate)
//! A comment is anchored to the **stable opaque `block_id`** ([`crate::block_tree::BlockId`]), NEVER a
//! positional index. A block move ([`crate::block_tree::BlockTree::move_block`]) only rewrites the
//! block's `order_key` + `parent_id` — the `block_id` is **never re-minted** (the §2.3 stability
//! obligation, the same one [`crate::subs`] block `#sub` mints rely on). Therefore a comment anchored
//! to `b9` still resolves to `b9` after the block is reordered or re-parented: **0 dangling comments
//! across a move**. A text-range anchor additionally carries `(start, end)` character offsets within
//! that block's inline content; the offsets are interpreted against the block's CURRENT content (the
//! anchor binds to the block id, not to a snapshot), so the range follows the block, not the page
//! position. (Offset rebasing under concurrent edits inside the block is the CRDT-era follow-on — see
//! FLOORS; the block-granular anchor — the gate — holds regardless.)
//!
//! ## The two gates (the dated green artifact — CI; threshold 0, never softened)
//! - **The comment-anchor gate** (0 dangling across a move): a comment anchored to a block/range
//!   survives a real [`crate::block_tree::BlockTree::move_block`] — the `#sub` stable-id anchor holds.
//!   **GREEN 2026-06-22** (`comment_anchor_survives_a_block_move_zero_dangling`:
//!   `moved_block_comment_dangles == 0`).
//! - **The comment-event gate**: create/resolve emits `knowledge.comment.created`/`.resolved` through
//!   the OUTBOX (emit-iff-committed; subject = the `#comment-<id>` sub-URN; aggregate = the page) —
//!   the events KN-P22's notif rules consume. **GREEN 2026-06-22**
//!   (`create_comment_emits_comment_created_through_the_outbox` +
//!   `resolve_comment_emits_comment_resolved_through_the_outbox`).
//!
//! ## FLOOR 4 named (VISION §3 / EI-01 §1 — one scheme, two stores with Chat)
//! This is the **KB-native comment store** (its own store; the `#comment-`/`#thread-` grammar shared
//! with Chat). The **consolidation onto the Chat threading primitive + the firehose transport on the
//! real-time-presence trigger is the named follow-on (KQ-9, post-M5)** — a MERGE, not a rewrite:
//! KB comments and Chat threads already share the `#sub` grammar (here), the [`myelin_content`] AST
//! (the comment body), and Refs (the edge/anchor model), so the promotion swaps the store/transport,
//! not the data model. Named, not silently done.
//!
//! ## Other named floors
//! - **The live OLTP comment store + the per-viewer ABAC `check`** is the KN-P05 / KN-P16 store
//!   wiring (the SAME [`Comment`]/[`CommentThread`] shapes the live store hydrates — the model here
//!   is store-agnostic; the in-memory [`CommentStore`] is the floor). Named.
//! - **The intra-block text-offset REBASE under concurrent edits** (so a text-range anchor's
//!   `(start, end)` survives an edit INSIDE the anchored block, not just a move OF the block) is the
//!   CRDT-era follow-on (KQ-6/the collab merge). The block-granular anchor — the comment-anchor gate
//!   — holds without it (the comment stays on its block). Named.

use myelin_content::block::Block;
use myelin_events::{EventId, OutboxTx};
use myelin_refs::{mint, ArtifactRef, ParseError, Sub, SubKind, SubKindRegistration};
use myelin_tenancy::TenantId;
use std::collections::BTreeMap;

use crate::block_tree::BlockId;
use crate::emit::{emit_change, KnowledgeChange};
use crate::subs::KNOWLEDGE_SUBSYSTEM;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE #comment-/#thread- #sub MINTS (contract 5.7 — Knowledge's KB-native owner of these kinds)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The `#sub` kinds Knowledge's **KB-native comment store** is the mint owner of (contract 5.7 / OQ-L):
/// a comment node (`comment-<opaqueid>`) and a thread root (`thread-<opaqueid>`). These are the SAME
/// frozen kinds Chat owns for its own store (one scheme, two stores) — Refs owns the grammar; each
/// store mints stable opaque ids of the kind it persists. (Knowledge's block-tree owner side —
/// `b`/`h` — is [`crate::subs::KNOWLEDGE_OWNED_SUB_KINDS`]; this is the comment-store side.)
pub const KNOWLEDGE_COMMENT_SUB_KINDS: &[SubKind] = &[SubKind::Comment, SubKind::Thread];

/// Knowledge's KB-native comment-store registration of its `#comment-`/`#thread-` kinds WITH Refs
/// (contract 5.7, the 5.7 half of KN-P23). Returns the [`SubKindRegistration`] Refs **accepts**
/// (validated against the frozen grammar + the Bus token table). This DECLARES the comment/thread
/// kinds the KB store mints; Refs owns the grammar + the 4-step tombstone ladder.
///
/// # Errors
/// Returns a [`myelin_refs::RegistrationError`] if the registration is not accepted — by construction
/// it always is (the subsystem token is canonical, the kinds are a non-empty, duplicate-free subset of
/// the frozen vocabulary); the fallible signature is the honest contract surface (Refs is the
/// authority that accepts, the comment store does not get to assert acceptance).
pub fn register_knowledge_comment_kinds(
) -> Result<SubKindRegistration, myelin_refs::RegistrationError> {
    SubKindRegistration {
        subsystem: KNOWLEDGE_SUBSYSTEM.to_string(),
        kinds: KNOWLEDGE_COMMENT_SUB_KINDS.to_vec(),
    }
    .validate()
}

/// Build Knowledge's canonical **page root** `myelin://<tenant>/knowledge/page/<page_id>` — the root a
/// `#thread-`/`#comment-` sub attaches to (a comment lives on the page that holds the anchored block;
/// architecture 03 §1.5 — `subject` = `…/page/<id>#comment-<id>`).
fn page_root(tenant: &TenantId, page_id: &str) -> Result<ArtifactRef, ParseError> {
    myelin_refs::parse(&format!("myelin://{}/knowledge/page/{}", tenant.0, page_id))
}

/// Mint a **thread-root** sub-URN `…/knowledge/page/<page_id>#thread-<thread_id>` (contract 5.7, the
/// `thread-<opaqueid>` kind). The opaque body is the KB store's stable `thread_id`. Grammatical by
/// construction (it round-trips the one frozen Refs grammar); an empty `thread_id` is rejected loudly.
pub fn mint_thread(
    tenant: &TenantId,
    page_id: &str,
    thread_id: &str,
) -> Result<ArtifactRef, ParseError> {
    mint(&page_root(tenant, page_id)?, Sub::Thread(thread_id.to_string()))
}

/// Mint a **comment** sub-URN `…/knowledge/page/<page_id>#comment-<comment_id>` (contract 5.7, the
/// `comment-<opaqueid>` kind). The opaque body is the KB store's stable `comment_id`. Grammatical by
/// construction; an empty `comment_id` is rejected loudly. This is the `subject` the
/// `knowledge.comment.created`/`.resolved` events carry ([`crate::emit::KnowledgeChange`]).
pub fn mint_comment(
    tenant: &TenantId,
    page_id: &str,
    comment_id: &str,
) -> Result<ArtifactRef, ParseError> {
    mint(&page_root(tenant, page_id)?, Sub::Comment(comment_id.to_string()))
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE ANCHOR MODEL (anchored to a STABLE block_id or a block_id + text range — survives a move)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **Where a comment thread is anchored in a page (architecture 04 §1.5 — a text range or a block).**
/// The anchor binds to the **stable opaque [`BlockId`]**, never a positional index, so it survives a
/// block move (the `block_id` is never re-minted; [`crate::block_tree::BlockTree::move_block`]). This
/// is the load-bearing invariant the comment-anchor gate proves (0 dangling across a move).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommentAnchor {
    /// Anchored to a whole block (`block_id`). The comment pins to the block wherever it moves.
    Block { block_id: BlockId },
    /// Anchored to a `(start, end)` character range WITHIN a block's inline content (`start < end`,
    /// `start`-inclusive / `end`-exclusive). The offsets are interpreted against the block's CURRENT
    /// content (the anchor binds to the block id, not a content snapshot), so the range moves with the
    /// block. Intra-block offset REBASE under concurrent edits is the named CRDT-era follow-on.
    Range {
        block_id: BlockId,
        /// 0-based, inclusive start character offset within the block's inline content.
        start: usize,
        /// 0-based, exclusive end character offset (`end > start`).
        end: usize,
    },
}

impl CommentAnchor {
    /// The stable [`BlockId`] this anchor binds to (the move-survival key — the same for a whole-block
    /// and a text-range anchor). The ONE field a move can never invalidate.
    pub fn block_id(&self) -> &BlockId {
        match self {
            CommentAnchor::Block { block_id } | CommentAnchor::Range { block_id, .. } => block_id,
        }
    }

    /// Construct a whole-block anchor.
    pub fn block(block_id: BlockId) -> Self {
        CommentAnchor::Block { block_id }
    }

    /// Construct a text-range anchor, validating `start < end` (an empty/inverted range is not a
    /// range). Returns `None` if the bounds are degenerate — a comment must anchor to ≥ 1 character.
    pub fn range(block_id: BlockId, start: usize, end: usize) -> Option<Self> {
        (start < end).then_some(CommentAnchor::Range { block_id, start, end })
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE KB-NATIVE COMMENT / THREAD MODEL (its own store; body is the myelin-content AST)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **A single KB-native comment — a node in a [`CommentThread`].** The body is the [`myelin_content`]
/// AST (a `Vec<Block>`, the SAME content model a page block carries — so a comment can hold the full
/// rich-text taxonomy + mention/`artifact_ref` inline nodes that emit `refs.edge.created`, X-2). The
/// `comment_id` is the KB store's stable opaque mint (the `#comment-<id>` body).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    /// The KB store's stable opaque comment id (the `#comment-<id>` body, [`mint_comment`]).
    pub comment_id: String,
    /// The comment body in the [`myelin_content`] AST (rich text + inline reference nodes).
    pub body: Vec<Block>,
}

/// **A KB-native comment THREAD — the anchored discussion (architecture 04 §1.5).** A thread roots at
/// a [`CommentAnchor`] (a stable block / text range) and holds an ordered sequence of [`Comment`]s
/// (the root comment + replies). The `thread_id` is the stable `#thread-<id>` mint. `resolved` marks
/// the thread settled (the `knowledge.comment.resolved` lifecycle); a resolved thread is hidden from
/// the active gutter but kept for history (reversible).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentThread {
    /// The KB store's stable opaque thread id (the `#thread-<id>` body, [`mint_thread`]).
    pub thread_id: String,
    /// Where the thread is anchored — the stable-block-id binding that survives a move.
    pub anchor: CommentAnchor,
    /// The ordered comments (the root + replies). Non-empty for a live thread (a thread is created
    /// with its root comment).
    pub comments: Vec<Comment>,
    /// Whether the thread is resolved (settled). Reversible — a resolved thread is kept for history.
    pub resolved: bool,
}

impl CommentThread {
    /// The stable [`BlockId`] this thread is anchored to (delegates to the anchor — the move key).
    pub fn anchored_block(&self) -> &BlockId {
        self.anchor.block_id()
    }
}

/// **The in-memory KB-native comment store (the FLOOR — the live OLTP store is KN-P05/P16).** Keyed by
/// `thread_id`. This is the store-agnostic MODEL the live Postgres store hydrates; the per-viewer ABAC
/// `check` (a comment is never more visible than its page) rides the KN-P16 wiring. The store NEVER
/// re-mints a block id, so a comment's anchor is stable by construction.
#[derive(Debug, Default)]
pub struct CommentStore {
    threads: BTreeMap<String, CommentThread>,
}

/// An error from a comment-store op.
#[derive(Debug, PartialEq, Eq)]
pub enum CommentError {
    /// A `thread_id`/`comment_id` was minted that is ungrammatical (empty opaque body) — the mint
    /// codec rejected it loudly (it never reaches the store).
    Ungrammatical(String),
    /// A `create` reused a `thread_id` already live in the store (thread ids mint once).
    DuplicateThread(String),
    /// A `resolve`/`reply` named a `thread_id` that does not exist in the store.
    NoSuchThread(String),
    /// A `create` named a degenerate text range (`start >= end`).
    DegenerateRange { start: usize, end: usize },
}

impl std::fmt::Display for CommentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommentError::Ungrammatical(s) => write!(f, "ungrammatical comment/thread mint: {s}"),
            CommentError::DuplicateThread(t) => write!(f, "thread_id already live: {t}"),
            CommentError::NoSuchThread(t) => write!(f, "no such thread: {t}"),
            CommentError::DegenerateRange { start, end } => {
                write!(f, "degenerate text-range anchor: start {start} >= end {end}")
            }
        }
    }
}

impl std::error::Error for CommentError {}

impl CommentStore {
    /// A fresh, empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a thread by id.
    pub fn thread(&self, thread_id: &str) -> Option<&CommentThread> {
        self.threads.get(thread_id)
    }

    /// The threads anchored to a given block (the gutter for one block — the move-survival read: this
    /// query returns the SAME threads before and after a move, because it keys on the stable
    /// `block_id`, never the block's position).
    pub fn threads_on_block(&self, block_id: &BlockId) -> Vec<&CommentThread> {
        self.threads.values().filter(|t| t.anchored_block() == block_id).collect()
    }

    /// **Create a KB-native comment thread anchored to a block/range, with its root comment** — the
    /// store half of [`create_comment`]. Validates the mints are grammatical + the range is well-formed
    /// + the thread id is fresh, then inserts a live (unresolved) thread. Returns the inserted thread.
    ///
    /// # Errors
    /// [`CommentError::Ungrammatical`] if `tenant`+`page_id`+id fail the Refs grammar (an empty id);
    /// [`CommentError::DuplicateThread`] if the id is already live; [`CommentError::DegenerateRange`]
    /// if a range anchor's bounds are inverted/empty.
    pub fn create_thread(
        &mut self,
        tenant: &TenantId,
        page_id: &str,
        thread_id: String,
        comment_id: String,
        anchor: CommentAnchor,
        body: Vec<Block>,
    ) -> Result<&CommentThread, CommentError> {
        // The mint codec is the grammar authority: a thread/comment URN must be grammatical or it
        // never enters the store (0 ungrammatical, the same loud-reject contract as `subs`).
        mint_thread(tenant, page_id, &thread_id)
            .map_err(|e| CommentError::Ungrammatical(e.to_string()))?;
        mint_comment(tenant, page_id, &comment_id)
            .map_err(|e| CommentError::Ungrammatical(e.to_string()))?;
        if let CommentAnchor::Range { start, end, .. } = &anchor {
            if start >= end {
                return Err(CommentError::DegenerateRange { start: *start, end: *end });
            }
        }
        if self.threads.contains_key(&thread_id) {
            return Err(CommentError::DuplicateThread(thread_id));
        }
        let thread = CommentThread {
            thread_id: thread_id.clone(),
            anchor,
            comments: vec![Comment { comment_id, body }],
            resolved: false,
        };
        Ok(self.threads.entry(thread_id).or_insert(thread))
    }

    /// **Resolve a thread (settle it) — the store half of [`resolve_comment`].** Idempotent-safe: a
    /// second resolve is a no-op on an already-resolved thread (still returns the thread). A resolved
    /// thread is kept for history (reversible via [`Self::reopen_thread`]).
    ///
    /// # Errors
    /// [`CommentError::NoSuchThread`] if the id is not in the store.
    pub fn resolve_thread(&mut self, thread_id: &str) -> Result<&CommentThread, CommentError> {
        let thread = self
            .threads
            .get_mut(thread_id)
            .ok_or_else(|| CommentError::NoSuchThread(thread_id.to_string()))?;
        thread.resolved = true;
        Ok(thread)
    }

    /// Reopen a resolved thread (reversibility over confirmation, architecture 04 — a resolve is not a
    /// delete). Does NOT re-emit a create event (the thread already exists); the UI reflects the flag.
    ///
    /// # Errors
    /// [`CommentError::NoSuchThread`] if the id is not in the store.
    pub fn reopen_thread(&mut self, thread_id: &str) -> Result<&CommentThread, CommentError> {
        let thread = self
            .threads
            .get_mut(thread_id)
            .ok_or_else(|| CommentError::NoSuchThread(thread_id.to_string()))?;
        thread.resolved = false;
        Ok(thread)
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. THE OPS — store mutation + the outbox event co-commit (the comment-event gate)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **Create a KB-native comment thread AND emit `knowledge.comment.created` through the outbox, in the
/// SAME transaction (the comment-event gate; emit-iff-committed, KN-D7).** The store mutation is
/// staged and the event is BUFFERED into `tx` via the ONE sanctioned emit seam ([`emit_change`] →
/// [`OutboxTx::emit`]); both co-commit when the caller commits `tx`. The event's `subject` is the
/// `#comment-<id>` sub-URN ([`mint_comment`]); KN-P22's notif rules fire on this event.
///
/// Returns the `EventId` of the buffered `knowledge.comment.created` event.
///
/// # Errors
/// Propagates [`CommentError`] from the store half (the event is NOT buffered if the store rejects);
/// propagates the [`myelin_events`] bus error if the emit itself fails.
#[allow(clippy::too_many_arguments)]
pub fn create_comment(
    store: &mut CommentStore,
    tx: &mut dyn OutboxTx,
    tenant: &TenantId,
    page_id: &str,
    thread_id: String,
    comment_id: String,
    anchor: CommentAnchor,
    body: Vec<Block>,
) -> Result<EventId, CommentOpError> {
    // (1) the store mutation (validated mints + a fresh thread). If this fails, NO event is buffered.
    store
        .create_thread(tenant, page_id, thread_id, comment_id.clone(), anchor, body)
        .map_err(CommentOpError::Store)?;
    // (2) the event, BUFFERED into the caller's open tx (co-commits with the store write).
    let change =
        KnowledgeChange::CommentCreated { page_id: page_id.to_string(), comment_id };
    emit_change(tx, tenant, &change, None).map_err(CommentOpError::Bus)
}

/// **Resolve a KB-native comment thread AND emit `knowledge.comment.resolved` through the outbox, in
/// the SAME transaction (the comment-event gate).** Mirrors [`create_comment`]: store mutation +
/// buffered event co-commit. The `comment_id` named is the thread's ROOT comment id (the `#comment-`
/// subject the resolved event carries — the same subject grammar as the created event).
///
/// Returns the `EventId` of the buffered `knowledge.comment.resolved` event.
///
/// # Errors
/// Propagates [`CommentError::NoSuchThread`] (no event buffered if the thread is unknown); propagates
/// the bus error if the emit fails.
pub fn resolve_comment(
    store: &mut CommentStore,
    tx: &mut dyn OutboxTx,
    tenant: &TenantId,
    page_id: &str,
    thread_id: &str,
    root_comment_id: String,
) -> Result<EventId, CommentOpError> {
    store.resolve_thread(thread_id).map_err(CommentOpError::Store)?;
    let change = KnowledgeChange::CommentResolved {
        page_id: page_id.to_string(),
        comment_id: root_comment_id,
    };
    emit_change(tx, tenant, &change, None).map_err(CommentOpError::Bus)
}

/// The error of a comment OP (the store-mutation + outbox-emit pair): either the store rejected the
/// mutation (no event buffered) or the bus emit failed. Distinct from [`CommentError`] (store-only) so
/// a caller can tell a domain reject from a transport failure.
#[derive(Debug)]
pub enum CommentOpError {
    /// The store rejected the mutation — NO event was buffered (the gate's atomicity: no event
    /// without its state).
    Store(CommentError),
    /// The outbox emit failed (a bus/transport error).
    Bus(myelin_events::OutboxError),
}

impl std::fmt::Display for CommentOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommentOpError::Store(e) => write!(f, "comment store rejected: {e}"),
            CommentOpError::Bus(e) => write!(f, "comment event emit failed: {e:?}"),
        }
    }
}

impl std::error::Error for CommentOpError {}

#[cfg(test)]
mod tests;
