//! # `myelin-content` — the FROZEN v1 block + inline taxonomy (contract 13.1, X-2/OQ-B)
//!
//! **Owning architecture docs:**
//! `04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md`
//! §2.1 (the canonical [`Block`] taxonomy) + §2.2 (the markdown-subset inline grammar +
//! the three structured nodes) and `02-internals-and-algorithms.md` §8 (the ONE editor
//! render path; the WASM target; why a markdown-subset string not inline-range JSON).
//!
//! **Contract-index 13.1** (`05-refined-shared-systems-architecture/contract-index.md`
//! row 13.1, **SHARPENED → frozen**, recon §X-2): Knowledge **leads and freezes** this
//! canonical taxonomy; Chat and Issues consume strict **subsets** (neither adds a node
//! type, X-2). The three inline ref nodes ([`InlineNode::Mention`] /
//! [`InlineNode::ArtifactRef`] / [`InlineNode::Embed`]) produce `refs.edge.created`
//! **uniformly** across Chat, Issues, and Knowledge (5.4).
//!
//! ## The one render path (KN-D2, the frozen correctness bar)
//! [`parse_inline`]/[`serialize_inline`] are ONE implementation, compiled native
//! (server) and to `wasm32-unknown-unknown` (client editor) from this single source —
//! eliminating the two-divergent-renderers trap structurally (EI-01 §7). The frozen gate
//! is the round-trip invariant `serialize_inline(parse_inline(md)) == md` over the frozen
//! [`corpus`] (KN-D2: 100% round-trip, 0 regressions). See [`corpus::corpus_pass_rate`].
//!
//! ## NO Knowledge feature ships here — this is a FREEZE
//! This crate ships ONLY the shared shapes that Chat / Issues / Search / Refs compile
//! against. No store, no service, no editor. The block tree, OLTP store, collab
//! transport, and editor land in the Knowledge M3 prompts (KN-P04+).
//!
//! ## Named floors
//! - **`sync_block` engine** ([`Block::SyncBlock`]): the node TYPE is frozen here so the
//!   taxonomy is complete, but its v1 engine is a read-projection FLOOR (renders like
//!   `embed`, §2.4) shipped in **KN-P12**; the editable-in-place multi-home follow-on is
//!   post-M5 (KQ-6, designed against the CRDT).
//! - **`db_view.view`**: RESOLVED in **KN-P02 (P-235)** — the KN-P01 `ViewHandle` floor is
//!   gone; the `db_view` block now carries the frozen `myelin_query::ViewSpec` (13.3, X-3)
//!   directly. The **ADF → `myelin-content` lossy-map (13.2)** also lands here ([`adf`]):
//!   Knowledge freezes the conversion table; Issues consumes it at import.
//! - **WASM-artifact green** (KN-D2 second leg): the crate is structured `std`-only and
//!   dependency-clean so it builds for `wasm32-unknown-unknown` from this one source; the
//!   `build-wasm.sh` script + the `wasm-render-path` integration test gate it. On a host
//!   without the `wasm32-unknown-unknown` std component the artifact build is a NAMED
//!   FLOOR (red-until-proven on real CI with the target installed) — the round-trip gate
//!   itself is proven green natively against the identical single source.

#![forbid(unsafe_code)]

pub mod adf;
pub mod block;
pub mod corpus;
pub mod inline;

pub use adf::{AdfMapping, AdfNode, AdfTarget, ImportReport, Loss, LossyConversion, MAP};
pub use block::{
    Block, CalloutTone, Cell, Column, EmbedDisplay, HeadingLevel, ListItem, TaskItem,
};
pub use inline::{parse_inline, serialize_inline, Inline, InlineNode, Mark, Span, OBJ};

/// The WASM render-path export surface (contract 13.1 WASM target). These free functions
/// are the stable C-ABI-friendly entry points the editor's WASM glue calls; they operate
/// on the SAME [`parse_inline`]/[`serialize_inline`] core as the server, so there is no
/// second renderer. They are plain `pub` Rust (no `wasm-bindgen` macro dependency in the
/// frozen crate) — the editor crate (KN-P08, the TS/React shell) wraps these behind its
/// own `wasm-bindgen` boundary, keeping this freeze crate toolchain-light.
pub mod wasm {
    use crate::inline::{parse_inline, serialize_inline, Inline, InlineNode};

    /// Parse a markdown-subset string + its positional structured-node array into the
    /// inline AST. The WASM client and the native server call THIS one function.
    pub fn render_parse(md: &str, nodes: &[InlineNode]) -> Inline {
        parse_inline(md, nodes)
    }

    /// Serialize the inline AST back to the canonical markdown-subset string. The frozen
    /// round-trip invariant `render_serialize(render_parse(md)) == md` holds on the SAME
    /// compiled code native and on `wasm32-unknown-unknown` (KN-D2).
    pub fn render_serialize(inline: &Inline) -> String {
        serialize_inline(inline)
    }
}

// The contract-index 13.1 provider/consumer CDC pair lives in
// `tests/cdc_13_1_taxonomy.rs` (the file the contract-coverage manifest names): Knowledge
// PROVIDES the frozen taxonomy; Chat/Issues/Search CONSUME strict subsets (X-2).
