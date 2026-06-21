//! # The CDC pair for contract 5.7 — chat's owned `#sub` mints (CHAT-P2 / P-244)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 5.7 (the
//! unified `#sub` sub-artifact scheme — ONE grammar, stable opaque ids minted by each owner; Refs
//! stores the full sub-URN + the stripped root). **Reconciliation:**
//! `00-reconciliation-decisions.md` X-4 (the frozen `#sub` grammar + the one resolution ladder —
//! `message-`/`thread-` are the Chat kinds). Owning architecture: chat
//! `04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md` §2 (the `#sub`
//! mints chat owns + the canonical root grammar); Refs owns the grammar + the ladder.
//!
//! ## The seam this pair pins (chat mints; Refs owns the grammar)
//! Row 5.7 is the seam between the PROVIDER that MINTS stable opaque sub-ids of its declared kinds
//! (here chat — [`myelin_chat::subs`]) and the CONSUMER that owns the ONE grammar + accepts the
//! registration + validates every minted sub-URN (Refs — [`myelin_refs`]). The frozen behaviour both
//! sides agree on:
//!
//! - the PROVIDER (chat) REGISTERS the `#sub` kinds it owns (`message-` / `thread-`) and MINTS
//!   grammatical sub-URNs of those kinds — every minted ref round-trips the one grammar (0
//!   ungrammatical), and Refs can [`strip_sub`](myelin_refs::strip_sub) each back to chat's canonical
//!   root. The `<opaqueid>` is the immutable `message_id` / `thread_root_id` ULID (the stability
//!   obligation is chat's, §2 — the `#sub` survives edits);
//! - the CONSUMER (Refs) ACCEPTS chat's registration (the kinds are the frozen vocabulary + the owner
//!   is a canonical Bus token) and would REJECT a malformed mint LOUDLY — chat does not author the
//!   grammar.
//!
//! This is the dedicated 5.7 provider+consumer pair the CHAT-P2 TESTS field names; the focused
//! per-mint round-trip fixtures live in `myelin_chat::subs::tests`. (No cargo-mutants floor: this is
//! REGISTRATION + grammatical mints over the already-proven Refs grammar, not new load-bearing
//! resolution logic — the resolver mutation floors land with the Chat M4 projection spine.)

use myelin_chat::subs::{mint_message, mint_thread, register_chat_sub_kinds, CHAT_OWNED_SUB_KINDS};
use myelin_refs::{format, strip_sub, sub_kind, ArtifactRef, SubKind};

/// **PROVIDER side of 5.7** — chat registers the `#sub` kinds it OWNS and returns its grammatical
/// mints. The provider's promise: every sub-URN it puts on a ref is one of its declared kinds and is
/// grammar-conformant by construction.
fn provider_mints() -> Vec<(SubKind, ArtifactRef)> {
    vec![
        (
            SubKind::Message,
            mint_message("acme-eu", "01J0MSGULID").expect("message mint is grammatical"),
        ),
        (
            SubKind::Thread,
            mint_thread("acme-eu", "01J0THRROOT").expect("thread mint is grammatical"),
        ),
    ]
}

/// **CONSUMER side of 5.7** — Refs, the grammar owner, classifies a minted sub-URN through the one
/// frozen grammar (and round-trips it byte-identical). The consumer's promise: it never silently
/// admits an ungrammatical sub-URN.
fn consumer_classifies(r: &ArtifactRef) -> Option<SubKind> {
    // Re-parse the minted ref through the one codec, then classify — a non-canonical ref would not
    // re-parse, proving the mint is grammatical (not merely a string chat built).
    let reparsed = myelin_refs::parse(&format(r)).ok()?;
    assert_eq!(format(&reparsed), format(r), "minted ref must be canonical");
    sub_kind(&reparsed).map(|s| s.kind())
}

/// The 5.7 pair, end-to-end: the PROVIDER (chat) registers its owned kinds + mints, and the CONSUMER
/// (Refs) ACCEPTS the registration and classifies EVERY mint to the declared kind — 0 ungrammatical.
/// This is the dated green artifact the CHAT-P2 GATE names (the #sub-grammar signal = 0).
#[test]
fn cdc_5_7_chat_provider_mints_consumer_accepts_and_classifies_every_kind() {
    // The registration is accepted by Refs (the one grammar owner).
    let reg = register_chat_sub_kinds().expect("Refs must ACCEPT chat's #sub kind registration");
    assert_eq!(reg.subsystem, "chat");
    assert_eq!(reg.kinds, CHAT_OWNED_SUB_KINDS.to_vec());

    // Every mint is grammatical: Refs classifies it to the declared kind (0 ungrammatical).
    for (declared, minted) in provider_mints() {
        assert_eq!(
            consumer_classifies(&minted),
            Some(declared),
            "Refs wrongly classified chat's mint `{}` (declared {declared:?})",
            format(&minted)
        );
        // Refs can strip every mint back to chat's canonical ROOT (the full sub-URN + stripped root
        // is what Refs stores, contract 5.7).
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

/// The CONSUMER (Refs) REJECTS a malformed chat-shaped mint LOUDLY — chat does NOT get to author the
/// grammar. The negative half of the seam: an empty opaque id never becomes a sub-URN; the mint
/// itself fails (it re-parses through the one grammar).
#[test]
fn cdc_5_7_consumer_rejects_a_malformed_chat_mint_loudly() {
    // an empty opaque message id (the stable-id obligation is chat's; the grammar still refuses)
    assert!(mint_message("acme", "").is_err());
    // an empty opaque thread id
    assert!(mint_thread("acme", "").is_err());
}

/// The PROVIDER registers ONLY its own kinds — chat owns `message-`/`thread-`, NOT the
/// Git/Issues/Knowledge/CI-owned kinds (architecture §2). The no-foreign-kind invariant, pinned at the
/// contract seam. (The `thread-` kind is SHARED with Git review threads, OQ-L — both are legitimate
/// owners of `thread-` mints; the seam is that chat does not claim `comment-`/line-range/block/etc.)
#[test]
fn cdc_5_7_chat_registers_only_its_own_kinds() {
    let reg = register_chat_sub_kinds().expect("registration accepted");
    for k in &reg.kinds {
        assert!(
            matches!(k, SubKind::Message | SubKind::Thread),
            "chat registered a non-chat-owned #sub kind `{k:?}`"
        );
    }
    for foreign in [
        SubKind::Comment,
        SubKind::LineRange,
        SubKind::Block,
        SubKind::Heading,
        SubKind::Row,
        SubKind::Field,
        SubKind::Check,
        SubKind::Step,
    ] {
        assert!(
            !reg.kinds.contains(&foreign),
            "chat must not register the foreign kind {foreign:?}"
        );
    }
}

/// The `#sub` is STABLE across edits because the id is immutable (the stability obligation, §2): a
/// re-mint with the same `message_id` yields the IDENTICAL sub-URN, so embeds/references don't dangle
/// when a message is edited. This pins chat's half of the contract-5.7 stability promise.
#[test]
fn cdc_5_7_chat_sub_is_stable_across_edits() {
    let before = mint_message("acme", "01J0STABLE").expect("mint");
    let after = mint_message("acme", "01J0STABLE").expect("re-mint after an edit");
    assert_eq!(
        before, after,
        "the #sub must be stable across edits (the message_id is immutable, §2)"
    );
}
