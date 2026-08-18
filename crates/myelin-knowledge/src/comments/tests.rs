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

#[test]
fn refs_accepts_knowledge_comment_and_thread_registration() {
    let reg = register_knowledge_comment_kinds().expect("Refs accepts the comment registration");
    assert_eq!(reg.subsystem, "knowledge");
    assert_eq!(reg.kinds, vec![SubKind::Comment, SubKind::Thread]);
}

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

    for r in [&c, &th] {
        let root = strip_sub(r);
        assert_eq!(format(&root), "page:7c2");
    }
}

#[test]
fn empty_comment_or_thread_id_is_rejected_loudly() {
    let t = tenant();
    assert!(mint_comment(&t, "p1", "").is_err());
    assert!(mint_thread(&t, "p1", "").is_err());
}

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

    assert!(CommentAnchor::range(b.clone(), 5, 5).is_none());
    assert!(CommentAnchor::range(b, 9, 2).is_none());
}

#[test]
fn comment_anchor_survives_a_block_move_zero_dangling() {
    let t = tenant();
    let page = "7c2";

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

    assert_eq!(
        store.threads_on_block(&b9).len(),
        2,
        "both threads anchor to b9 before the move"
    );
    assert_eq!(
        tree.get(&b9).and_then(|r| r.parent_id.clone()),
        Some(pa.clone())
    );

    tree.move_block(&b9, &pb, None, None, jit())
        .expect("move b9 under pb");

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

#[test]
fn store_rejects_duplicate_degenerate_and_ungrammatical() {
    let t = tenant();
    let mut store = CommentStore::new();
    let anchor = CommentAnchor::block(BlockId("b1".into()));
    store
        .create_thread(&t, "p", "t1".into(), "c1".into(), anchor.clone(), body("a"))
        .expect("first create");

    assert_eq!(
        store
            .create_thread(&t, "p", "t1".into(), "c2".into(), anchor, body("b"))
            .unwrap_err(),
        CommentError::DuplicateThread("t1".into())
    );

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
        row.aggregate.0, "page:7c2",
        "aggregate = the page (per-doc order)"
    );
}

#[test]
fn resolve_comment_emits_comment_resolved_through_the_outbox() {
    use myelin_content::events::KNOWLEDGE_COMMENT_RESOLVED;
    let (store_bus, minter) = store_and_minter();
    let mut cstore = CommentStore::new();
    let t = tenant();

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
