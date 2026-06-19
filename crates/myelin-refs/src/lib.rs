//! # `myelin-refs` — `ArtifactRef` parse/format/resolve + the edge/backlink client
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.3 (`myelin-refs` — URN parse/format/resolve + edge client).
//!
//! **Contract-index cluster:** 5 — `ArtifactRef`, refs & projection
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` rows 5.1
//! `ArtifactRef` parse/format, 5.3 `backlinks/edges`, 5.7 the unified `#sub` grammar).
//!
//! ## What crosses the crate boundary here (the frozen surface)
//! `Refs` — the one library every service links for `parse` / `format` / `edges` /
//! `backlinks`; services must NOT re-implement URN handling (REF-3). `parse` rejects
//! scope-less/ambiguous refs and **never guesses scope**. `backlinks` is
//! permission-filtered (REF-1) — it threads a `viewer: &Principal` through `list_objects`.
//! The `ArtifactRef` *value type* is owned by `myelin-tenancy` / re-exported by
//! `myelin-events`; this crate owns its *behaviour*.
//!
//! ## Floors named (stubbed bodies → filling prompt)
//! All method bodies are `todo!()`. The trait SHAPE is frozen here (P-001 skeleton); the
//! Refs roadmap fills them:
//! - `parse`/`format` (5.1) + the unified `#sub` grammar + the 4-step tombstone ladder
//!   (5.7, X-4/OQ-D) → Refs M-stage.
//! - `edges`/`backlinks` over the recursive-CTE walk + `list_objects` pre-filter (5.3) →
//!   Refs M-stage. Edge creation is `refs.edge.created` emission via the outbox (5.4) —
//!   no standalone edge-write API.

use myelin_identity::Principal;
use serde::{Deserialize, Serialize};

/// Re-export the frozen `ArtifactRef` value type so callers of this crate read
/// `myelin_refs::ArtifactRef`.
pub use myelin_events::ArtifactRef;

/// An outbound or inbound edge between two artifacts (architecture §2.3; contract 5.3/5.4).
/// The typed-edge taxonomy (`closes/blocks/depends_on/parent/...`) is the TE-7 mirror
/// (5.5); the skeleton carries an opaque edge so the trait shape compiles.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: ArtifactRef,
    pub to: ArtifactRef,
    pub kind: String,
}

/// Placeholder error for the skeleton. The real refs error taxonomy (ambiguous-ref,
/// scope-less-ref, tombstone) lands with the impl.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefError(pub String);

/// `Result` alias for the refs surface.
pub type Result<T> = core::result::Result<T, RefError>;

/// The one refs library (architecture §2.3; contract 5.1/5.3; REF-3). `parse`/`format`
/// are associated (no `&self`) — they are the canonical URN codec. `edges`/`backlinks`
/// take `&self` (they read the edge projection). `backlinks` is permission-filtered via
/// the viewer (REF-1).
///
/// **Floor:** every body is `todo!()` (frozen shape only); the Refs roadmap implements
/// parse/format/`#sub` (5.1/5.7) and the edge walk (5.3).
pub trait Refs {
    /// Rejects ambiguity; never guesses scope (REF-3).
    fn parse(s: &str) -> Result<ArtifactRef>;
    fn format(r: &ArtifactRef) -> String;
    /// outbound edges.
    fn edges(&self, r: &ArtifactRef) -> Result<Vec<Edge>>;
    /// permission-filtered inbound edges (REF-1).
    fn backlinks(&self, r: &ArtifactRef, viewer: &Principal) -> Result<Vec<Edge>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    /// Compile-asserting test: the `Refs` trait shape is frozen (contract 5.1/5.3) —
    /// `parse`/`format` associated fns, `edges`/`backlinks(&self, …, viewer)` methods.
    /// A stub implementer proves the signatures; bodies are deferred to the Refs roadmap.
    #[test]
    fn refs_trait_shape_is_frozen() {
        struct Stub;
        impl Refs for Stub {
            fn parse(_s: &str) -> Result<ArtifactRef> {
                todo!("URN parse + #sub grammar lands in the Refs roadmap (5.1/5.7)")
            }
            fn format(r: &ArtifactRef) -> String {
                r.0.clone()
            }
            fn edges(&self, _r: &ArtifactRef) -> Result<Vec<Edge>> {
                todo!("edge walk lands in the Refs roadmap (5.3)")
            }
            fn backlinks(&self, _r: &ArtifactRef, _viewer: &Principal) -> Result<Vec<Edge>> {
                todo!("permission-filtered backlinks land in the Refs roadmap (5.3)")
            }
        }
        let r = ArtifactRef("myelin://acme/issues/issue/PROJ-1".into());
        assert_eq!(Stub::format(&r), "myelin://acme/issues/issue/PROJ-1");
        let _viewer = Principal {
            id: PrincipalId("p".into()),
            kind: PrincipalKind::Human,
            tenant: TenantId("acme".into()),
        };
    }
}
