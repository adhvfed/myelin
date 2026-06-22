//! The editor PRIMITIVES, shipped + unit-tested STANDALONE before the integrated
//! editor (KN-P08 / contract 13.1 WASM target; architecture
//! `04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md`
//! §8.2; design-system `08-design-system/02-components/block-editor.md` §2 the §8b.2
//! one-render-path law).
//!
//! Three primitives obey the day-one editor mandate (EI-05 §2, the "this isn't a real
//! editor" tells):
//!
//! 1. **The serializer** — `inline AST ↔ markdown-subset string`, the three structured
//!    nodes as U+FFFC placeholders + positional `inline_nodes`. ALREADY frozen in
//!    [`crate::inline`] ([`parse_inline`]/[`serialize_inline`]); this module REUSES it —
//!    there is no second renderer. The standalone KN-D2 leg over it lives in
//!    [`crate::corpus`].
//! 2. **The offset model** ([`offset`]) — **the caret is a character offset into the
//!    serialized markdown**, bridged to/from controlled-`contenteditable` DOM positions.
//!    A structured node is **exactly one caret position** (one U+FFFC code point). This is
//!    the "own the document model, reconcile the DOM, never read from it"
//!    ProseMirror-class lesson made testable.
//! 3. **The DOM-surgery** ([`surgery`]) — **Enter-splits-a-block** + **caret-placement-
//!    after-split** (caret lands at the START of the new block — the #1 "not a real
//!    editor" tell), plus **paste/IME normalisation** through the SAME serializer (paste
//!    is re-parsed + re-serialized into the canonical subset, never injected raw; an IME
//!    composition commit is a text insertion at the caret offset).
//!
//! ## WASM-clean (the one render path)
//! This module is `std`-only + reuses the frozen [`crate::inline`] core, so it compiles
//! native (server, structural undo/redo + op-replay) AND to `wasm32-unknown-unknown` (the
//! editor's controlled `contenteditable`) from ONE source — the offset model and the
//! Enter-split run on the IDENTICAL compiled code client + server (no drift).
//!
//! ## FLOOR (named): primitives ONLY — no integrated editor
//! These are the three primitives standalone. **No integrated editor, no transport, no
//! merge, no permissions, no React/`wasm-bindgen` shell.** The integrated single-doc
//! editor over these primitives + the KN-P07 transport is the IMMEDIATE follow-on
//! **KN-P09 (P-299)** — a green primitive here is NOT yet an editor. KN-P09 wraps the
//! [`offset`]/[`surgery`] free functions behind the TS/React `<BlockEditor>` shell
//! (design-system §1), drives them in a real browser (KN-D2 re-run integrated), and wires
//! the transport (KN-P07) the ops ride.

use crate::inline::{parse_inline, serialize_inline, InlineNode, OBJ};

pub mod offset;
pub mod surgery;

pub use offset::{
    caret_count, dom_to_offset, offset_to_dom, segments, CaretMap, DomPosition, Segment,
    SegmentKind,
};
pub use surgery::{normalize_paste, split_at, BlockSplit};

/// Re-parse + re-serialize a markdown-subset string through the ONE frozen render path,
/// carrying its positional structured-node array. This is the canonicalising pass the
/// editor runs "on serialize" (EI-05 §2: *normalise on serialise*) — the bridge from raw
/// controlled-`contenteditable` input back to the stored canonical form. It is the
/// identity on already-canonical input (the KN-D2 fixed-point) and the same code the
/// paste/IME normalisation in [`surgery`] funnels through.
pub fn canonicalize(md: &str, nodes: &[InlineNode]) -> (String, Vec<InlineNode>) {
    let inline = parse_inline(md, nodes);
    (serialize_inline(&inline), inline.nodes)
}

/// Count the structured ([`OBJ`]) nodes in a serialized markdown string — the seam the
/// offset model uses to assert "a structured node is exactly one caret position". Equal to
/// the length of the positional `inline_nodes` array for a well-formed line.
pub(crate) fn obj_count(md: &str) -> usize {
    md.chars().filter(|&c| c == OBJ).count()
}

/// Slice the positional structured-node array at a U+FFFC boundary. `before`/`after`
/// receive the nodes whose U+FFFC falls before/at-or-after `obj_split` (the count of
/// U+FFFC characters in the left half). Used by the Enter-split so each resulting block
/// carries exactly its own structured nodes.
pub(crate) fn split_nodes(
    nodes: &[InlineNode],
    obj_split: usize,
) -> (Vec<InlineNode>, Vec<InlineNode>) {
    let k = obj_split.min(nodes.len());
    (nodes[..k].to_vec(), nodes[k..].to_vec())
}
