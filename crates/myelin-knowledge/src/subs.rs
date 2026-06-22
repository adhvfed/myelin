//! # `subs` — Knowledge's `#sub` block/heading mints registered with Refs (KN-P10 / P-300, contract 5.7)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md`
//! §2.3 (the `block` row — `block_id` is the STABLE opaque id, the `#sub` anchor `b<id>` and the ref
//! target X-4, stable across edits/moves/collaboration). **Contract:**
//! `05-refined-shared-systems-architecture/contract-index.md` row 5.7 (the unified `#sub` grammar;
//! the stable opaque ids `b<opaqueid>` / `h<opaqueid>` are minted by each owner — Knowledge OWNS the
//! `b`/`h` mints; Refs owns the grammar + the 4-step tombstone ladder).
//!
//! ## The seam this module pins (Knowledge mints; Refs owns the grammar)
//! Refs ([`myelin_refs`]) owns ONE `#sub` URN grammar + ONE resolution ladder (recon X-4). Each
//! subsystem owns the **stable opaque mint** of its declared kinds. This module is Knowledge's side:
//!
//! - [`register_knowledge_sub_kinds`] — Knowledge's REGISTRATION with Refs: it DECLARES that
//!   Knowledge owns (mints + will resolve) the [`SubKind::Block`] (`b<opaqueid>`) and
//!   [`SubKind::Heading`] (`h<opaqueid>`) kinds. The registration is validated by Refs
//!   ([`SubKindRegistration::validate`]) and ACCEPTED.
//! - [`mint_block`] / [`mint_heading`] — Knowledge's typed `#sub` mints. They build Knowledge's
//!   **canonical page root** (`knowledge/page/<page_id>`) and attach the stable opaque sub-id
//!   through the one Refs codec ([`myelin_refs::mint`]), so every minted ref is grammatical **by
//!   construction** (0 ungrammatical — `mint` re-parses through the frozen grammar and rejects a
//!   malformed opaque body LOUDLY). Refs stores both the full sub-URN AND the
//!   [`myelin_refs::strip_sub`] root.
//!
//! ## The stability obligation is Knowledge's (architecture §2.3 / contract 5.7)
//! The `<opaqueid>` is the **immutable `block_id`** — a stable opaque id, never a positional index.
//! A reordered or edited block keeps its `block_id` and thus its `#sub` (see
//! [`crate::block_tree`] — the move is an `order_key` write that NEVER mints a new `block_id`), so an
//! embed of `b9 of page 7c2` does not dangle when the block is moved. Refs validates the grammar
//! SHAPE only; the opacity + stability of the id is Knowledge's contract. Refs stores the full
//! sub-URN AND the `#sub`-stripped root, so a broken sub-anchor still resolves to the parent page via
//! the one 4-step tombstone ladder (live / moved / outdated / gone).
//!
//! ## FLOORS named (EI-01 §1 — this is the REGISTRATION + the GRAMMAR + the stable mint CODEC)
//! Only the kind REGISTRATION + the grammatical mint codecs + the block-tree stability obligation
//! ship here (the stability gate lives in [`crate::block_tree`]). The named follow-ons:
//! - **The `b`/`h` `project(ref, viewer)` sub-anchor resolver** (the Refs 4-step ladder calls — the
//!   per-viewer permission-gated projection of a block, with `moved`/`outdated` states) is the
//!   **KN-P19 follow-on** (the Refs glue: `#sub` mints + the 4-step tombstone ladder + resolve/project).
//! - **The runtime mint SITE co-committed with the `knowledge.block.created` outbox event** rides the
//!   KN-P06 emit seam + the [`crate::block_tree`] insert path; this module is the mint CODEC the site
//!   calls.

use myelin_refs::{mint, ArtifactRef, ParseError, Sub, SubKind, SubKindRegistration};

/// The canonical Bus subsystem token Knowledge owns (§2 — `myelin://<tenant>/knowledge/<type>/<id>`).
pub const KNOWLEDGE_SUBSYSTEM: &str = "knowledge";

/// The `#sub` kinds Knowledge is the mint + resolver owner of (contract 5.7 / architecture §2.3): a
/// content block (`b<block_id>`) and a heading anchor (`h<block_id>`). Knowledge does NOT mint
/// `comment-`/`thread-`/`message-`/line-range/check/step kinds — those are Git/Chat/CI. `row-` and
/// `field-` (the flexible-database sub-anchors) are the KN-P17 follow-on, NOT this prompt's block-tree
/// scope.
pub const KNOWLEDGE_OWNED_SUB_KINDS: &[SubKind] = &[SubKind::Block, SubKind::Heading];

/// Knowledge's registration of its owned `#sub` kinds WITH Refs (contract 5.7, the 5.7 half of
/// KN-P10 / P-300). Returns the [`SubKindRegistration`] that Refs **accepts** (validated against the
/// frozen grammar + the Bus token table). This DECLARES the kinds Knowledge mints; it does NOT install
/// a resolver (the resolver is the named KN-P19 follow-on).
///
/// # Errors
/// Returns a [`myelin_refs::RegistrationError`] if the registration is not accepted — by construction
/// it always is (the subsystem token is canonical, the kinds are a non-empty, duplicate-free subset of
/// the frozen vocabulary); the fallible signature is the honest contract surface (Refs is the
/// authority that accepts, Knowledge does not get to assert acceptance).
pub fn register_knowledge_sub_kinds() -> Result<SubKindRegistration, myelin_refs::RegistrationError>
{
    SubKindRegistration {
        subsystem: KNOWLEDGE_SUBSYSTEM.to_string(),
        kinds: KNOWLEDGE_OWNED_SUB_KINDS.to_vec(),
    }
    .validate()
}

/// Build Knowledge's canonical **page root** `myelin://<tenant>/knowledge/page/<page_id>`
/// (architecture §2.6 — a page is the independently-addressable root-block subtree; a block `#sub`
/// anchors onto the page that holds it). This is the root a `b`/`h` sub attaches to.
fn page_root(tenant: &str, page_id: &str) -> Result<ArtifactRef, ParseError> {
    myelin_refs::parse(&format!("myelin://{tenant}/knowledge/page/{page_id}"))
}

/// Mint a **content-block** sub-URN `…/knowledge/page/<page_id>#b<block_id>` (contract 5.7, the
/// `b<opaqueid>` kind). The opaque body is Knowledge's **immutable `block_id`** (the stability
/// obligation is Knowledge's, §2.3 — the id does not change when the block is edited or reordered, so
/// the `#sub` survives moves; see [`crate::block_tree::BlockTree::move_block`]). The result is
/// grammatical by construction (it round-trips the frozen grammar); an empty `block_id` is rejected
/// LOUDLY.
///
/// **FLOOR:** this MINTS the stable ref; the runtime mint SITE (co-committed with
/// `knowledge.block.created`) rides KN-P06; the `project(ref, viewer)` resolver is KN-P19.
pub fn mint_block(tenant: &str, page_id: &str, block_id: &str) -> Result<ArtifactRef, ParseError> {
    let root = page_root(tenant, page_id)?;
    mint(&root, Sub::Block(block_id.to_string()))
}

/// Mint a **heading-anchor** sub-URN `…/knowledge/page/<page_id>#h<block_id>` (contract 5.7, the
/// `h<opaqueid>` kind). The opaque body is the `block_id` of the heading block (a `heading` block in
/// the frozen `myelin-content` taxonomy). Like [`mint_block`] the result is grammatical by
/// construction; an empty `block_id` is rejected LOUDLY.
pub fn mint_heading(
    tenant: &str,
    page_id: &str,
    block_id: &str,
) -> Result<ArtifactRef, ParseError> {
    let root = page_root(tenant, page_id)?;
    mint(&root, Sub::Heading(block_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_refs::{format, strip_sub, sub_kind};

    /// Refs ACCEPTS Knowledge's registration of exactly the `b`/`h` kinds (the deliverable's 5.7
    /// registration half).
    #[test]
    fn refs_accepts_knowledge_block_and_heading_registration() {
        let reg =
            register_knowledge_sub_kinds().expect("Refs accepts Knowledge's #sub registration");
        assert_eq!(reg.subsystem, "knowledge");
        assert_eq!(reg.kinds, vec![SubKind::Block, SubKind::Heading]);
    }

    /// Knowledge mints ONLY its own block/heading kinds — never a foreign kind (comment/thread/row/…).
    #[test]
    fn knowledge_registers_only_block_and_heading() {
        let reg = register_knowledge_sub_kinds().expect("registration accepted");
        for k in &reg.kinds {
            assert!(
                matches!(k, SubKind::Block | SubKind::Heading),
                "Knowledge registered a non-Knowledge-owned #sub kind `{k:?}`"
            );
        }
        assert!(!reg.kinds.contains(&SubKind::Comment));
        assert!(!reg.kinds.contains(&SubKind::Row));
        assert!(!reg.kinds.contains(&SubKind::Field));
    }

    /// Every block/heading mint is grammatical: it round-trips the one frozen Refs grammar, and Refs
    /// classifies it to the declared kind (0 ungrammatical).
    #[test]
    fn block_and_heading_mints_are_grammatical_and_classify() {
        let b = mint_block("acme-eu", "7c2", "b9").expect("block mint is grammatical");
        let h = mint_heading("acme-eu", "7c2", "hdr1").expect("heading mint is grammatical");

        // Re-parse through the one codec (a non-canonical ref would not re-parse).
        let rb = myelin_refs::parse(&format(&b)).expect("block ref is canonical");
        let rh = myelin_refs::parse(&format(&h)).expect("heading ref is canonical");
        assert_eq!(format(&rb), format(&b));
        assert_eq!(format(&rh), format(&h));
        assert_eq!(sub_kind(&rb).map(|s| s.kind()), Some(SubKind::Block));
        assert_eq!(sub_kind(&rh).map(|s| s.kind()), Some(SubKind::Heading));

        // Refs strips every mint back to Knowledge's canonical page root (the full sub-URN + the
        // stripped root is what Refs stores, contract 5.7).
        for r in [&b, &h] {
            let root = strip_sub(r);
            assert!(
                !format(&root).contains('#'),
                "stripped root carries a #sub: {}",
                format(&root)
            );
            assert!(
                myelin_refs::parse(&format(&root)).is_ok(),
                "stripped root must itself parse as a canonical root"
            );
        }
    }

    /// The mint codec REJECTS a malformed (empty opaque body) mint LOUDLY — Knowledge does not get to
    /// author the grammar; an empty `block_id` is never a sub-URN.
    #[test]
    fn empty_block_id_is_rejected_loudly() {
        assert!(mint_block("acme", "p1", "").is_err());
        assert!(mint_heading("acme", "p1", "").is_err());
    }
}
