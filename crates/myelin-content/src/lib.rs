//! # `myelin-content` — block/inline taxonomy + the three platform-load-bearing inline nodes
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.5 (`myelin-content` — the substrate-relevant seam).
//!
//! **Contract-index cluster:** 13 — the shared crates' refined shapes
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` row 13.1
//! `myelin-content` taxonomy, frozen X-2/OQ-B).
//!
//! ## What crosses the crate boundary here (the substrate-relevant surface)
//! The three platform-load-bearing inline nodes — `mention(Principal)`,
//! `artifact_ref(ArtifactRef)`, `embed(ArtifactRef)` — are the **producers of
//! `refs.edge.created`** (emitted via the outbox). Inline content is a markdown-subset
//! string (KN-2/D10); the three structured nodes are stored structured. The crate has a
//! **WASM compile target** (C-8) so the one editor render path reuses the Rust core
//! client-side (`render(parse(md)) === md`).
//!
//! ## Floor named (this is a SUBSTRATE-RELEVANT SEAM, not the full taxonomy)
//! The full canonical block set + the ADF lossy-map (13.1/13.2, frozen X-2) is
//! **Knowledge's** deliverable (Knowledge leads; Chat/Issues consume strict subsets) —
//! NOT this prompt. P-001 ships only the three substrate-load-bearing inline-node TYPES
//! so subsystems have a stable surface to register against; the taxonomy + WASM target
//! land with the Knowledge roadmap. Bodies/variants beyond the three nodes are `todo!()`.

use myelin_events::ArtifactRef;
use myelin_identity::Principal;
use serde::{Deserialize, Serialize};

/// The three platform-load-bearing structured inline nodes (architecture §2.5). These
/// three uniformly produce `refs.edge.created` via the outbox (contract 5.4). The rest
/// of the inline taxonomy is a markdown-subset string (KN-2), owned by Knowledge (13.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InlineNode {
    /// @-mention of a principal.
    Mention(Principal),
    /// inline reference to an artifact.
    ArtifactRefNode(ArtifactRef),
    /// inline embed/unfurl of an artifact.
    Embed(ArtifactRef),
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    /// Compile-asserting test: the three substrate-load-bearing inline nodes exist with
    /// their frozen payload types — `Mention(Principal)`, `ArtifactRefNode(ArtifactRef)`,
    /// `Embed(ArtifactRef)` (architecture §2.5; these are the `refs.edge.created`
    /// producers). The full taxonomy is Knowledge's (13.1).
    #[test]
    fn three_load_bearing_inline_nodes_exist() {
        let m = InlineNode::Mention(Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, TenantId("acme".into())));
        let a = InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()));
        let e = InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/42".into()));
        assert!(matches!(m, InlineNode::Mention(_)));
        assert!(matches!(a, InlineNode::ArtifactRefNode(_)));
        assert!(matches!(e, InlineNode::Embed(_)));
    }
}
