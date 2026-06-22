//! Unit tests for KN-P23 KB-native comment threads (the comment-anchor gate + the comment-event gate).
//!
//! Two gates, both required green by the prompt:
//! 1. **comment-anchor gate** — a comment anchored to a block/range survives a block move (the `#sub`
//!    stable-id anchor holds; 0 dangling comments across a move).
//! 2. **comment-event gate** — creating/resolving a comment emits
//!    `knowledge.comment.created`/`.resolved` through the OUTBOX (the events KN-P22's rules consume).
//!
//! ## Mutation floor (EI-01 §3) — the anchoring is mandatory-core
//! The anchor-survives-a-move logic is the load-bearing comment invariant (a dangling comment IS the
//! failure). The model here keys the anchor on the STABLE [`BlockId`] and the move path
//! ([`crate::block_tree::BlockTree::move_block`]) is already mutation-floored at 100% in `block_tree`
//! (the `block_id` is never re-minted — the property this gate depends on). The comment store's own
//! load-bearing logic — the anchor `block_id()` projection (whole-block vs range), the grammatical
//! mint reject, the duplicate/degenerate-range guards, the resolve/reopen flag, and the
//! store-rejects-⇒-no-event atomicity — each has a test below a mutation flips. Stated, not a floor
//! waved past: this is core, the tests pin every branch.

use super::*;
use myelin_content::block::Block;
use myelin_content::inline::parse_inline;
use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use std::sync::Arc;

use crate::block_tree::{BlockId, BlockTree, PageId};
use myelin_query::field::Jitter;

fn jit() -> Jitter {
    Jitter::from_ranks(0, 0).expect("jitter ranks in 0..62")
}

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn body(text: &str) -> Vec<Block> {
    vec![Block::Paragraph {
        inline: parse_inline(text, &[]),
    }]
}

fn ctx_base() -> EmitContextBase {
    let principal = Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, tenant());
    EmitContextBase {
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(principal),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-22T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-22T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn store_and_minter() -> (OutboxStore, Arc<dyn IdMinter>) {
    (
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
    )
}

// ── the #sub mints ───────────────────────────────────────────────────────────────────────────────

/// Refs ACCEPTS Knowledge's KB-native comment-store registration of exactly the comment/thread kinds.
#[test]
fn refs_accepts_knowledge_comment_and_thread_registration() {
    let reg = register_knowledge_comment_kinds().expect("Refs accepts the comment registration");
    assert_eq!(reg.subsystem, "knowledge");
    assert_eq!(reg.kinds, vec![SubKind::Comment, SubKind::Thread]);
}

/// Every comment/thread mint is grammatical: it round-trips the one frozen Refs grammar and classifies
/// to the declared kind (0 ungrammatical — the SAME loud-reject contract `subs` block/heading uses).
#[test]
fn comment_and_thread_mints_are_grammatical_and_classify() {
    use myelin_refs::{format, strip_sub, sub_kind};
    let t = tenant();
    let c = mint_comment(&t, "7c2", "cabc").expect("comment mint grammatical");
    let th = mint_thread(&t, "7c2", "t9").expect("thread mint grammatical");

    assert_eq!(format(&c), "myelin://acme/knowledge/page/7c2#comment-cabc");
    assert_eq!(format(&th), "myelin://acme/knowledge/page/7c2#thread-t9");
    assert_eq!(sub_kind(&c).map(|s| s.kind()), Some(SubKind::Comment));
    assert_eq!(sub_kind(&th).map(|s| s.kind()), Some(SubKind::Thread));

    // The stripped root is Knowledge's canonical page root (the ladder degrades a dead sub to it).
    for r in [&c, &th] {
        let root = strip_sub(r);
        assert_eq!(format(&root), "myelin://acme/knowledge/page/7c2");
    }
}

/// An empty opaque id is rejected loudly by the mint codec (Knowledge does not author the grammar).
#[test]
fn empty_comment_or_thread_id_is_rejected_loudly() {
    let t = tenant();
    assert!(mint_comment(&t, "p1", "").is_err());
    assert!(mint_thread(&t, "p1", "").is_err());
}

// ── the anchor model ───────────────────────────────────────────────────────────────────────────────

/// A whole-block and a text-range anchor both expose the SAME stable block_id (the move key); the
/// range carries its `(start, end)` and rejects a degenerate range.
#[test]
fn anchor_exposes_the_stable_block_id_and_validates_a_range() {
    let b = BlockId("b9".into());
    let whole = CommentAnchor::block(b.clone());
    assert_eq!(whole.block_id(), &b);

    let r = CommentAnchor::range(b.clone(), 3, 10).expect("a well-formed range");
    assert_eq!(r.block_id(), &b);
    assert!(matches!(
        r,
        CommentAnchor::Range {
            start: 3,
            end: 10,
            ..
        }
    ));

    // A degenerate (empty/inverted) range is not a range.
    assert!(CommentAnchor::range(b.clone(), 5, 5).is_none());
    assert!(CommentAnchor::range(b, 9, 2).is_none());
}

// ── THE COMMENT-ANCHOR GATE (survives a move; 0 dangling) ──────────────────────────────────────────

/// **THE COMMENT-ANCHOR GATE.** A comment anchored to a block (and to a range within a block) survives
/// a block MOVE — the `#sub` stable-id anchor holds; 0 dangling comments across the move. We build a
/// real [`BlockTree`], anchor comments on a block, MOVE the block (re-parent + re-order), and assert
/// every comment still resolves to its block: `threads_on_block` returns the SAME threads, and the
/// comment's anchored block id still names a LIVE block in the tree.
#[test]
fn comment_anchor_survives_a_block_move_zero_dangling() {
    let t = tenant();
    let page = "7c2";

    // A real block tree: parent_a / parent_b at root; the anchored block `b9` under parent_a.
    let mut tree = BlockTree::new(PageId(page.into()));
    let pa = BlockId("pa".into());
    let pb = BlockId("pb".into());
    let b9 = BlockId("b9".into());
    tree.insert_root(pa.clone(), "paragraph", jit())
        .expect("insert pa");
    tree.insert_root(pb.clone(), "paragraph", jit())
        .expect("insert pb");
    tree.insert_block(b9.clone(), &pa, "paragraph", jit())
        .expect("insert b9 under pa");

    // Two comment threads anchored to b9: one whole-block, one a text range within it.
    let mut store = CommentStore::new();
    store
        .create_thread(
            &t,
            page,
            "tw".into(),
            "cw".into(),
            CommentAnchor::block(b9.clone()),
            body("looks good"),
        )
        .expect("create whole-block thread");
    store
        .create_thread(
            &t,
            page,
            "tr".into(),
            "cr".into(),
            CommentAnchor::range(b9.clone(), 0, 5).expect("range"),
            body("typo here"),
        )
        .expect("create range thread");

    // Before the move: both threads resolve to b9; b9 lives under pa.
    assert_eq!(
        store.threads_on_block(&b9).len(),
        2,
        "both threads anchor to b9 before the move"
    );
    assert_eq!(
        tree.get(&b9).and_then(|r| r.parent_id.clone()),
        Some(pa.clone())
    );

    // THE MOVE — re-parent b9 from pa to pb (a real move: order_key + parent rewrite, NOT a re-mint).
    tree.move_block(&b9, &pb, None, None, jit())
        .expect("move b9 under pb");

    // After the move: the block id is UNCHANGED, so EVERY comment still resolves to b9 (0 dangling).
    assert_eq!(
        tree.get(&b9).and_then(|r| r.parent_id.clone()),
        Some(pb),
        "b9 re-parented to pb"
    );
    let still_on_b9 = store.threads_on_block(&b9);
    assert_eq!(
        still_on_b9.len(),
        2,
        "0 dangling: both threads STILL anchor to b9 after the move"
    );

    // The dangling check: every comment's anchored block id is a LIVE block in the tree.
    let mut dangling = 0usize;
    for thread in store.threads.values() {
        if tree.get(thread.anchored_block()).is_none() {
            dangling += 1;
        }
    }
    assert_eq!(
        dangling, 0,
        "moved_block_comment_dangles == 0 (the comment-anchor gate)"
    );
}

// ── the store guards ───────────────────────────────────────────────────────────────────────────────

/// A duplicate thread id is rejected (thread ids mint once); a degenerate range never enters the store;
/// an ungrammatical (empty id) mint is rejected before any insert.
#[test]
fn store_rejects_duplicate_degenerate_and_ungrammatical() {
    let t = tenant();
    let mut store = CommentStore::new();
    let anchor = CommentAnchor::block(BlockId("b1".into()));
    store
        .create_thread(&t, "p", "t1".into(), "c1".into(), anchor.clone(), body("a"))
        .expect("first create");

    // Duplicate thread id.
    assert_eq!(
        store
            .create_thread(&t, "p", "t1".into(), "c2".into(), anchor, body("b"))
            .unwrap_err(),
        CommentError::DuplicateThread("t1".into())
    );

    // A range anchor cannot be built degenerate (the constructor guards), and create_thread also
    // guards if a Range is constructed directly.
    let degenerate = CommentAnchor::Range {
        block_id: BlockId("b2".into()),
        start: 7,
        end: 7,
    };
    assert_eq!(
        store
            .create_thread(&t, "p", "t2".into(), "c3".into(), degenerate, body("c"))
            .unwrap_err(),
        CommentError::DegenerateRange { start: 7, end: 7 }
    );

    // Ungrammatical: an empty comment id never enters the store.
    assert!(matches!(
        store
            .create_thread(
                &t,
                "p",
                "t3".into(),
                String::new(),
                CommentAnchor::block(BlockId("b3".into())),
                body("d")
            )
            .unwrap_err(),
        CommentError::Ungrammatical(_)
    ));
}

/// Resolve marks the thread settled and is reversible (reopen); a resolve on an unknown thread errors.
#[test]
fn resolve_is_reversible_and_guarded() {
    let t = tenant();
    let mut store = CommentStore::new();
    store
        .create_thread(
            &t,
            "p",
            "t1".into(),
            "c1".into(),
            CommentAnchor::block(BlockId("b1".into())),
            body("a"),
        )
        .expect("create");
    assert!(
        !store.thread("t1").unwrap().resolved,
        "fresh thread is unresolved"
    );

    store.resolve_thread("t1").expect("resolve");
    assert!(
        store.thread("t1").unwrap().resolved,
        "resolved after resolve"
    );
    store
        .resolve_thread("t1")
        .expect("resolve again (idempotent)");
    assert!(store.thread("t1").unwrap().resolved);

    store.reopen_thread("t1").expect("reopen");
    assert!(
        !store.thread("t1").unwrap().resolved,
        "reopen clears the flag (reversible)"
    );

    assert_eq!(
        store.resolve_thread("nope").unwrap_err(),
        CommentError::NoSuchThread("nope".into())
    );
}

// ── THE COMMENT-EVENT GATE (emit through the outbox; emit-iff-committed) ────────────────────────────

/// **THE COMMENT-EVENT GATE.** Creating a comment emits `knowledge.comment.created` through the OUTBOX,
/// in the SAME transaction as the store write (emit-iff-committed, KN-D7): an OPEN tx has buffered the
/// event (depth 0); a COMMIT makes the comment AND its event durable together; the event's type is the
/// frozen `knowledge.comment.created` token, its subject the `#comment-<id>` sub-URN, its aggregate the
/// page (per-doc ordering). A DROPPED tx writes NO event (0 ghost) — but the store mutation already
/// happened, so the gate's atomicity is: the store rejects ⇒ no event (tested separately); a committed
/// state ⇒ exactly its event.
#[test]
fn create_comment_emits_comment_created_through_the_outbox() {
    use myelin_content::events::KNOWLEDGE_COMMENT_CREATED;
    let (store_bus, minter) = store_and_minter();
    let mut cstore = CommentStore::new();
    let t = tenant();

    let mut tx = store_bus.begin(Arc::clone(&minter), ctx_base());
    let id = create_comment(
        &mut cstore,
        &mut tx,
        &t,
        "7c2",
        "t9".into(),
        "cabc".into(),
        CommentAnchor::block(BlockId("b9".into())),
        body("looks good"),
    )
    .expect("create_comment");

    assert_eq!(
        store_bus.outbox_depth(),
        0,
        "an OPEN tx has buffered the event (nothing durable yet)"
    );
    assert!(
        cstore.thread("t9").is_some(),
        "the thread is in the store (staged with the event)"
    );

    tx.commit()
        .expect("commit the comment + its event together");
    assert_eq!(
        store_bus.outbox_depth(),
        1,
        "after commit: exactly the comment.created event is durable"
    );

    let row = store_bus
        .row(&id)
        .expect("the committed comment.created row");
    assert_eq!(
        row.envelope.type_.0, KNOWLEDGE_COMMENT_CREATED,
        "the frozen comment.created token"
    );
    assert_eq!(
        row.envelope.subject.0, "myelin://acme/knowledge/page/7c2#comment-cabc",
        "subject = the #comment- sub-URN (the KN-P22 notif rules fire on this)"
    );
    assert_eq!(
        row.aggregate.0, "myelin://acme/knowledge/page/7c2",
        "aggregate = the page (per-doc order)"
    );
}

/// Resolving a comment emits `knowledge.comment.resolved` through the outbox, co-committed with the
/// store flag flip. The subject is the root comment's `#comment-` sub-URN (the same grammar).
#[test]
fn resolve_comment_emits_comment_resolved_through_the_outbox() {
    use myelin_content::events::KNOWLEDGE_COMMENT_RESOLVED;
    let (store_bus, minter) = store_and_minter();
    let mut cstore = CommentStore::new();
    let t = tenant();

    // Seed a live thread (committed) first.
    let mut tx0 = store_bus.begin(Arc::clone(&minter), ctx_base());
    create_comment(
        &mut cstore,
        &mut tx0,
        &t,
        "7c2",
        "t9".into(),
        "cabc".into(),
        CommentAnchor::block(BlockId("b9".into())),
        body("typo"),
    )
    .expect("seed");
    tx0.commit().expect("commit the seed");
    assert_eq!(store_bus.outbox_depth(), 1);

    // Resolve it: store flag + the resolved event co-commit.
    let mut tx = store_bus.begin(Arc::clone(&minter), ctx_base());
    let id = resolve_comment(&mut cstore, &mut tx, &t, "7c2", "t9", "cabc".into())
        .expect("resolve_comment");
    assert!(
        store_bus.row(&id).is_none(),
        "an OPEN tx has not made the resolved event durable yet"
    );
    tx.commit().expect("commit the resolve + its event");

    assert!(
        cstore.thread("t9").unwrap().resolved,
        "the thread is resolved in the store"
    );
    let row = store_bus
        .row(&id)
        .expect("the committed comment.resolved row");
    assert_eq!(
        row.envelope.type_.0, KNOWLEDGE_COMMENT_RESOLVED,
        "the frozen comment.resolved token"
    );
    assert_eq!(
        row.envelope.subject.0,
        "myelin://acme/knowledge/page/7c2#comment-cabc"
    );
}

/// **The gate's atomicity: a store REJECT buffers NO event (no event without its state).** A duplicate
/// thread create returns the store error AND leaves the outbox untouched.
#[test]
fn a_rejected_create_buffers_no_event() {
    let (store_bus, minter) = store_and_minter();
    let mut cstore = CommentStore::new();
    let t = tenant();

    let mut tx0 = store_bus.begin(Arc::clone(&minter), ctx_base());
    create_comment(
        &mut cstore,
        &mut tx0,
        &t,
        "p",
        "t1".into(),
        "c1".into(),
        CommentAnchor::block(BlockId("b1".into())),
        body("a"),
    )
    .expect("first create");
    tx0.commit().expect("commit");
    assert_eq!(store_bus.outbox_depth(), 1);

    // A duplicate create: the store rejects, the event is NOT buffered.
    let mut tx = store_bus.begin(Arc::clone(&minter), ctx_base());
    let err = create_comment(
        &mut cstore,
        &mut tx,
        &t,
        "p",
        "t1".into(),
        "c9".into(),
        CommentAnchor::block(BlockId("b1".into())),
        body("b"),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        CommentOpError::Store(CommentError::DuplicateThread(_))
    ));
    tx.commit().expect("commit the (empty) tx");
    assert_eq!(
        store_bus.outbox_depth(),
        1,
        "the rejected create buffered NO event (no event without state)"
    );
}
