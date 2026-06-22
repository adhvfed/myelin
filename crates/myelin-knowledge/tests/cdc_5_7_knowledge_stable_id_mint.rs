//! # The CDC pair for contract 5.7 — Knowledge's owned stable block-id `#sub` mints (KN-P10 / P-300)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 5.7
//! (the unified `#sub` sub-artifact scheme — ONE grammar, stable opaque ids minted by each owner;
//! Refs stores the full sub-URN + the stripped root). **Reconciliation:**
//! `00-reconciliation-decisions.md` X-4 (the frozen `#sub` grammar + the one resolution ladder).
//! Owning architecture: Knowledge
//! `04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md` §2.3 (the
//! `block_id` is the STABLE opaque id, the `b<id>`/`h<id>` `#sub` anchor + the ref target X-4, stable
//! across edits/moves/collaboration); Refs owns the grammar + the ladder.
//!
//! ## The seam this pair pins (Knowledge mints; Refs owns the grammar)
//! Row 5.7 is the seam between the PROVIDER that MINTS stable opaque sub-ids of its declared kinds
//! (here Knowledge — [`myelin_knowledge::subs`]) and the CONSUMER that owns the ONE grammar + accepts
//! the registration + validates every minted sub-URN (Refs — [`myelin_refs`]). The frozen behaviour
//! both sides agree on:
//!
//! - the PROVIDER (Knowledge) REGISTERS the `#sub` kinds it owns (`b<id>` / `h<id>`) and MINTS
//!   grammatical sub-URNs of those kinds — every minted ref round-trips the one grammar (0
//!   ungrammatical), and Refs can [`strip_sub`](myelin_refs::strip_sub) each back to Knowledge's
//!   canonical page root;
//! - the CONSUMER (Refs) ACCEPTS Knowledge's registration (the kinds are the frozen vocabulary + the
//!   owner is a canonical Bus token) and would REJECT a malformed mint LOUDLY — Knowledge does not
//!   author the grammar;
//! - the STABILITY obligation is Knowledge's: a reordered/re-parented block keeps its `block_id`, so
//!   the minted `#sub` is IDENTICAL before and after a move (the `moved_block_id_dangles == 0`
//!   property — proven against the [`myelin_knowledge::BlockTree`] move path here).
//!
//! (No cargo-mutants floor: this is REGISTRATION + grammatical mints over the already-proven Refs
//! grammar + the order_key tree-write whose mutation floor is the frozen `myelin_query::OrderKey`
//! module's own — not new load-bearing resolution logic. The `b`/`h` `project(ref, viewer)` resolver
//! mutation floors land with KN-P19.)

use myelin_knowledge::block_tree::{BlockId, BlockTree, PageId};
use myelin_knowledge::subs::{
    mint_block, mint_heading, register_knowledge_sub_kinds, KNOWLEDGE_OWNED_SUB_KINDS,
};
use myelin_query::field::Jitter;
use myelin_refs::{format, strip_sub, sub_kind, ArtifactRef, SubKind};

fn jit(a: usize, b: usize) -> Jitter {
    Jitter::from_ranks(a, b).expect("jitter ranks in 0..62")
}

/// **PROVIDER side of 5.7** — Knowledge registers the `#sub` kinds it OWNS and returns its grammatical
/// mints. The provider's promise: every sub-URN it puts on a ref is one of its declared kinds and is
/// grammar-conformant by construction.
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

/// **CONSUMER side of 5.7** — Refs, the grammar owner, classifies a minted sub-URN through the one
/// frozen grammar (and round-trips it byte-identical). The consumer's promise: it never silently
/// admits an ungrammatical sub-URN.
fn consumer_classifies(r: &ArtifactRef) -> Option<SubKind> {
    let reparsed = myelin_refs::parse(&format(r)).ok()?;
    assert_eq!(format(&reparsed), format(r), "minted ref must be canonical");
    sub_kind(&reparsed).map(|s| s.kind())
}

/// The 5.7 pair, end-to-end: the PROVIDER (Knowledge) registers its owned kinds + mints, and the
/// CONSUMER (Refs) ACCEPTS the registration and classifies EVERY mint to the declared kind — 0
/// ungrammatical. This is the dated green artifact the KN-P10 TESTS field names.
#[test]
fn cdc_5_7_knowledge_provider_mints_consumer_accepts_and_classifies_every_kind() {
    // The registration is accepted by Refs (the one grammar owner).
    let reg = register_knowledge_sub_kinds().expect("Refs must ACCEPT Knowledge's #sub registration");
    assert_eq!(reg.subsystem, "knowledge");
    assert_eq!(reg.kinds, KNOWLEDGE_OWNED_SUB_KINDS.to_vec());

    // Every mint is grammatical: Refs classifies it to the declared kind (0 ungrammatical).
    for (declared, minted) in provider_mints() {
        assert_eq!(
            consumer_classifies(&minted),
            Some(declared),
            "Refs wrongly classified Knowledge's mint `{}` (declared {declared:?})",
            format(&minted)
        );
        // Refs can strip every mint back to Knowledge's canonical page ROOT (the full sub-URN + the
        // stripped root is what Refs stores, contract 5.7).
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

/// The CONSUMER (Refs) REJECTS a malformed Knowledge-shaped mint LOUDLY — Knowledge does NOT get to
/// author the grammar. The negative half of the seam: an empty opaque block-id never becomes a sub-URN.
#[test]
fn cdc_5_7_consumer_rejects_a_malformed_knowledge_mint_loudly() {
    assert!(mint_block("acme", "p1", "").is_err());
    assert!(mint_heading("acme", "p1", "").is_err());
}

/// The PROVIDER registers ONLY its own kinds — Knowledge owns `b<id>`/`h<id>`, NOT the Git/Chat-owned
/// `comment-`/`thread-`/`message-`/line-range kinds nor the CI `check-`/`step-`. The no-foreign-kind
/// invariant, pinned at the contract seam.
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

/// **THE STABLE-ID PROPERTY GATE (the headline 5.7 obligation): a block reordered/moved keeps its
/// `block_id`, so the minted `#sub` is byte-IDENTICAL before and after the move — an embed of `b<id>`
/// resolves to the same block (`moved_block_id_dangles == 0`).** This proves the stability is real
/// against the actual [`BlockTree`] move path, not merely asserted on a string.
#[test]
fn cdc_5_7_minted_sub_is_stable_across_a_block_move_zero_dangles() {
    let tenant = "acme-eu";
    let page = "7c2";
    let bid = BlockId("nested".into());

    // Build a tree: root → c1 → nested, plus a sibling c2 to move `nested` under.
    let mut tree = BlockTree::new(PageId(page.into()));
    tree.insert_root(BlockId("root".into()), "paragraph", jit(0, 0)).unwrap();
    tree.insert_block(BlockId("c1".into()), &BlockId("root".into()), "paragraph", jit(0, 1)).unwrap();
    tree.insert_block(BlockId("c2".into()), &BlockId("root".into()), "paragraph", jit(0, 2)).unwrap();
    tree.insert_block(bid.clone(), &BlockId("c1".into()), "paragraph", jit(0, 0)).unwrap();

    // The editor stored a `#sub` embed of `nested` (the b<id> mint) BEFORE any move.
    let sub_before = mint_block(tenant, page, bid.as_str()).expect("mint before move");
    let row_before = tree.resolve_sub(&bid).expect("embed resolves before move").clone();

    // MOVE `nested` from under c1 to under c2 (an order_key + parent_id write; NEVER an id re-mint).
    tree.move_block(&bid, &BlockId("c2".into()), None, None, jit(3, 3)).unwrap();

    // The same stable id resolves AFTER the move (0 dangles).
    let row_after = tree.resolve_sub(&bid).expect("embed STILL resolves after move");
    assert_eq!(row_after.block_id, row_before.block_id, "block_id stable across the move");
    assert_ne!(row_after.order_key, row_before.order_key, "the move rewrote the order_key");

    // And the minted `#sub` is byte-IDENTICAL before and after — the embed never dangles.
    let sub_after = mint_block(tenant, page, bid.as_str()).expect("mint after move");
    assert_eq!(
        format(&sub_before),
        format(&sub_after),
        "the b<id> #sub is byte-identical across a move (moved_block_id_dangles == 0)"
    );
    // Refs still classifies + strips the post-move mint (the consumer half holds across the move).
    assert_eq!(consumer_classifies(&sub_after), Some(SubKind::Block));
}
