//! # `myelin-tenancy` — tenant / region / residency types (the DAG root)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.5 (`myelin-tenancy` — the substrate-relevant seam) and §2.9 (the dependency root).
//!
//! **Contract-index cluster:** 12 — Tenancy & control plane
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` rows 12.1, 12.5).
//!
//! ## What crosses the crate boundary here (the frozen surface)
//! `TenantId` and `Region` are the `(tenant, region)` first-class partition key
//! (contract 12.1; ADR-11). Tenant is the first column / partition key of *everything*
//! and comes from the verified token, never the URL path (architecture §0.1). This crate
//! is the **sink** of the dependency DAG (§2.9): it depends on nothing above it, so a
//! cycle cannot form. The `no-cross-sync-cycle` lint (P-S10) enforces the same property
//! on the *service* call graph; the `crate-graph-acyclic` test in this workspace
//! (`myelin-substrate`) enforces it on the *crate* graph.
//!
//! ## Floors named (stubbed bodies → filling prompt)
//! - Residency-tag / cell-routing client types (`ResidencyTag`, `discover`, `place`,
//!   `placement_of`, `residency_verify`, `CrossCellPointer`) are the **control plane's**
//!   deliverable (contract 12.2/12.3/12.4/12.6), NOT this prompt. P-001 ships only the
//!   `TenantId` / `Region` value types the envelope and every query path thread.
//! - The `ResidencyTag` body is a placeholder; the residency-pin lint (P-S11) and the
//!   tenancy roadmap fill the routing/attestation surfaces.
//!
//! ## DEVIATION FROM A FROZEN SHAPE (EI-01 §1 — code wins, write it down)
//! The architecture (§2.1) sites the `ArtifactRef` *type* in `myelin-events`, and the
//! P-001 prompt lists `ArtifactRef` under `myelin-events`. BUT the frozen DAG (§2.9) has
//! `myelin-identity` upstream of `myelin-events` (identity is a sink with only tenancy
//! below it), and `AuthzClient::check(..., object: &ArtifactRef, ...)` (contract 4.2,
//! §2.2) takes an `ArtifactRef`. Putting the type in `myelin-events` would force
//! identity → events, a back-edge that VIOLATES root-last ordering and breaks
//! "identity depends on nothing above tenancy".
//!
//! Resolution: the `ArtifactRef` **value newtype** (`pub struct ArtifactRef(String)` —
//! pure data; parse/format/resolve stay in `myelin-refs`, REF-3) is defined HERE in the
//! DAG sink and **re-exported as `myelin_events::ArtifactRef`** so the frozen public
//! path the envelope + the prompt name is preserved byte-for-byte for every consumer.
//! No signature changes; only the definition site moves down to keep the DAG acyclic.
//! This is the minimal change that satisfies BOTH "ArtifactRef is the envelope's
//! `subject` type in events" AND "identity is a sink". Flagged in the P-001 report.

use serde::{Deserialize, Serialize};

/// The first-class tenant partition key (contract 12.1; ADR-11).
///
/// Tenant is the first column / partition key of everything; there is no cross-tenant
/// query path. Held as an opaque, comparable id so it can be a SQL partition key and a
/// telemetry/trace label without exposing tenant-name PII (`control-plane-pii-free`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TenantId(pub String);

/// The residency region a `(tenant, region)` pair is pinned to (contract 12.1; ADR-11).
///
/// Every store/stream/index/cache declares a region (the `residency-pin` lint, P-S11);
/// `Region` is the value that declaration carries. RFC-3339-UTC timestamps and these
/// region tags together let blast-radius scoping be mechanical (architecture §10.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Region(pub String);

/// The canonical artifact reference (contract 5.1; ADR-13.1).
///
/// `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]`. This is the **value type only** —
/// `parse`/`format`/`resolve` live in `myelin-refs` (REF-3), which must not be
/// re-implemented per service. The `#sub` sub-anchor grammar is frozen in Refs
/// (contract 5.7, X-4).
///
/// **Definition-site note (see crate-level DEVIATION):** the architecture sites this
/// type in `myelin-events`, but the DAG (§2.9) puts `myelin-identity` above events and
/// `AuthzClient::check` needs `ArtifactRef`. So the value type lives here in the sink
/// and is re-exported as `myelin_events::ArtifactRef`; the frozen path is preserved and
/// the DAG stays acyclic with identity-as-sink.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ArtifactRef(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-asserting test: the frozen public surface exists with the frozen field
    /// shapes — `TenantId` and `Region` are constructible newtypes over a stable id
    /// (contract 12.1). If a field name/shape drifts from the §2.5/§12.1 anchor, this
    /// stops compiling.
    #[test]
    fn surface_tenant_and_region_exist() {
        let tenant: TenantId = TenantId("acme".to_string());
        let region: Region = Region("eu-west".to_string());
        // ordering + hashing are part of the frozen shape (partition-key usable).
        assert_eq!(tenant, TenantId("acme".to_string()));
        assert!(region < Region("eu-westz".to_string()));
    }
}
