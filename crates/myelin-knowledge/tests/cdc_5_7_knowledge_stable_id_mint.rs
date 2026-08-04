use myelin_knowledge::block_tree::{BlockId, BlockTree, PageId};
use myelin_knowledge::subs::{
    mint_block, mint_heading, register_knowledge_sub_kinds, KNOWLEDGE_OWNED_SUB_KINDS,
};
use myelin_query::field::Jitter;
use myelin_refs::{format, strip_sub, sub_kind, ArtifactRef, SubKind};

fn jit(a: usize, b: usize) -> Jitter {
    Jitter::from_ranks(a, b).expect("jitter ranks in 0..62")
}

fn provider_mints() -> Vec<(SubKind, ArtifactRef)> {
    vec![
        (
            SubKind::Block,
            mint_block("acme-eu", "7c2", "b9").expect("block mint is grammatical"),
        ),
        (
            SubKind::Heading,
            mint_heading("acme-eu", "7c2", "hdr1").expect("heading mint is grammatical"),
        ),
    ]
}

fn consumer_classifies(r: &ArtifactRef) -> Option<SubKind> {
    let reparsed = myelin_refs::parse(&format(r)).ok()?;
    assert_eq!(format(&reparsed), format(r), "minted ref must be canonical");
    sub_kind(&reparsed).map(|s| s.kind())
}

#[test]
fn cdc_5_7_knowledge_provider_mints_consumer_accepts_and_classifies_every_kind() {
    let reg =
        register_knowledge_sub_kinds().expect("Refs must ACCEPT Knowledge's #sub registration");
    assert_eq!(reg.subsystem, "knowledge");
    assert_eq!(reg.kinds, KNOWLEDGE_OWNED_SUB_KINDS.to_vec());

    for (declared, minted) in provider_mints() {
        assert_eq!(
            consumer_classifies(&minted),
            Some(declared),
            "Refs wrongly classified Knowledge's mint `{}` (declared {declared:?})",
            format(&minted)
        );
        let root = strip_sub(&minted);
        assert!(
            !format(&root).contains('#'),
            "stripped root still carries a `#sub`: `{}`",
            format(&root)
        );
        assert!(
            myelin_refs::parse(&format(&root)).is_ok(),
            "stripped root `{}` must itself be a parseable canonical root",
            format(&root)
        );
    }
}

#[test]
fn cdc_5_7_consumer_rejects_a_malformed_knowledge_mint_loudly() {
    assert!(mint_block("acme", "p1", "").is_err());
    assert!(mint_heading("acme", "p1", "").is_err());
}

#[test]
fn cdc_5_7_knowledge_registers_only_its_own_kinds() {
    let reg = register_knowledge_sub_kinds().expect("registration accepted");
    for k in &reg.kinds {
        assert!(
            matches!(k, SubKind::Block | SubKind::Heading),
            "Knowledge registered a non-Knowledge-owned #sub kind `{k:?}`"
        );
    }
    assert!(!reg.kinds.contains(&SubKind::Comment));
    assert!(!reg.kinds.contains(&SubKind::Message));
    assert!(!reg.kinds.contains(&SubKind::Check));
}

#[test]
fn cdc_5_7_minted_sub_is_stable_across_a_block_move_zero_dangles() {
    let tenant = "acme-eu";
    let page = "7c2";
    let bid = BlockId("nested".into());

    let mut tree = BlockTree::new(PageId(page.into()));
    tree.insert_root(BlockId("root".into()), "paragraph", jit(0, 0))
        .unwrap();
    tree.insert_block(
        BlockId("c1".into()),
        &BlockId("root".into()),
        "paragraph",
        jit(0, 1),
    )
    .unwrap();
    tree.insert_block(
        BlockId("c2".into()),
        &BlockId("root".into()),
        "paragraph",
        jit(0, 2),
    )
    .unwrap();
    tree.insert_block(bid.clone(), &BlockId("c1".into()), "paragraph", jit(0, 0))
        .unwrap();

    let sub_before = mint_block(tenant, page, bid.as_str()).expect("mint before move");
    let row_before = tree
        .resolve_sub(&bid)
        .expect("embed resolves before move")
        .clone();

    tree.move_block(&bid, &BlockId("c2".into()), None, None, jit(3, 3))
        .unwrap();

    let row_after = tree
        .resolve_sub(&bid)
        .expect("embed STILL resolves after move");
    assert_eq!(
        row_after.block_id, row_before.block_id,
        "block_id stable across the move"
    );
    assert_ne!(
        row_after.order_key, row_before.order_key,
        "the move rewrote the order_key"
    );

    let sub_after = mint_block(tenant, page, bid.as_str()).expect("mint after move");
    assert_eq!(
        format(&sub_before),
        format(&sub_after),
        "the b<id> #sub is byte-identical across a move (moved_block_id_dangles == 0)"
    );
    assert_eq!(consumer_classifies(&sub_after), Some(SubKind::Block));
}
