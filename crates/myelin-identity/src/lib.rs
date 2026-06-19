//! # `myelin-identity` — the one polymorphic `Principal`, capability types, authz client
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.2 (`myelin-identity` — `Principal`, capability types, the authz client).
//!
//! **Contract-index cluster:** 4 — Identity & access
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` rows 4.2 `check`,
//! 4.3 `list_objects`, 4.5 `delegation`; `Consistency`/zookie is row 4.10).
//!
//! ## What crosses the crate boundary here (the frozen surface)
//! - `Principal` / `PrincipalKind` — the ONE polymorphic principal (Human / Agent /
//!   Service; kind is data, not a code branch; EI-02 §2). Every service links the authz
//!   client; no service re-implements the check (ADR-13.3).
//! - `AuthzClient::check` (4.2, fail-closed per-action gate, optional `CaveatContext`),
//!   `list_objects` (4.3, the leak-free pre-filter → `Ids | Filter{set_expr, zookie}`),
//!   `delegation` (4.5, `agent ∩ delegation ∩ tenant`, monotone).
//! - `Consistency` — the zookie / read-your-writes token; also the input to fail-static
//!   (§8). A zookie-stamped read bypasses the fail-static cache (4.10).
//!
//! ## DAG position (§2.9): identity is a SINK
//! Identity depends on **`myelin-tenancy` only** (for `TenantId` / `Region` /
//! `ArtifactRef`). It does NOT depend on `myelin-events` — see the `myelin-tenancy`
//! crate-level DEVIATION note: `ArtifactRef`'s value type was moved to the sink so
//! `AuthzClient::check`'s `&ArtifactRef` parameter does not force an identity → events
//! back-edge. This keeps "identity depends on nothing" true (the `no-cross-sync-cycle`
//! lint, P-S10, enforces the same on the service call graph).
//!
//! ## Floors named (stubbed bodies → filling prompt)
//! Every method body is `todo!()`. The trait SHAPES are frozen here (P-001 ships the
//! skeleton); the Identity roadmap fills them:
//! - `authenticate` + machine-identity resolution (4.1) → Identity M1.
//! - `check` + `CaveatContext` ABAC (4.2) → Identity M1.
//! - `list_objects` + the `SetExpr` push-down / authz reverse index (4.3) → Identity M1
//!   (the single most load-bearing inter-system contract; the `SetExpr` algebra is
//!   Identity-owned, deliberately NOT pinned in the substrate skeleton).
//! - `delegation` (4.5), `write_tuples`/zookie (4.6), `mint_run_token` (4.7) → Identity M1.
//! - The fail-static cache wiring (`Consistency` is the input) lands in `myelin-substrate`
//!   `FailStatic<T>` (P-S18) + Identity M1 (the proven Id-hiccup drill, P-S25 / SUB-D4).

use myelin_tenancy::{ArtifactRef, Region, TenantId};
use serde::{Deserialize, Serialize};

/// The region a resolved principal is pinned to (contract 4.1: `authenticate` returns
/// `Principal{tenant, region, ...}`). Re-exported from `myelin-tenancy` so the
/// envelope's `actor`/`region` threading reads `myelin_identity::PrincipalRegion`; the
/// value type lives in the DAG sink. Wired into `authenticate` at Identity M1.
pub type PrincipalRegion = Region;

/// Opaque principal id (contract 4.1). PII-free routing id, never a name/email
/// (`control-plane-pii-free`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrincipalId(pub String);

/// Opaque reference to an agent runtime instance (architecture §2.2 — `RuntimeRef`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuntimeRef(pub String);

/// The ONE polymorphic principal (architecture §2.2; EI-02 §2; ADR-13.3).
///
/// Kind is *data*, not a code branch. `tenant` is first-class (never optional). The
/// `…` in the architecture snippet (e.g. `region`, `data_role`, `status`) lands with
/// the Identity-M1 `authenticate` impl (contract 4.1); P-001 freezes the load-bearing
/// cross-crate fields `id`, `kind`, `tenant`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub id: PrincipalId,
    pub kind: PrincipalKind,
    pub tenant: TenantId,
}

/// The principal kind (architecture §2.2). Human / Agent / Service — an Agent carries
/// its `runtime_ref` and an optional `on_behalf_of` (ADR-13.3 delegation).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrincipalKind {
    Human,
    Agent {
        runtime_ref: RuntimeRef,
        on_behalf_of: Option<PrincipalId>,
    },
    Service,
}

/// A permission token (capability) checked against an object (contract 4.2). The
/// permission/relation algebra is Identity's ReBAC fragment surface (4.9); the substrate
/// freezes only the opaque carrier the call shape needs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Permission(pub String);

/// The object type for a `list_objects` pre-filter (contract 4.3).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectType(pub String);

/// The consistency token ("zookie", Zanzibar) for read-your-writes (contract 4.10).
///
/// Also the input to fail-static (§8): a zookie-stamped read bypasses the fail-static
/// cache and the authz reverse index honours the zookie revision watermark.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consistency {
    pub at_least: Zookie,
    pub mode: ConsistencyMode,
}

/// The opaque consistency watermark (Zanzibar zookie).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zookie(pub String);

/// Strong (read-your-writes) vs bounded-stale (fail-static-eligible) (§8, contract 4.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsistencyMode {
    Strong,
    BoundedStale,
}

/// The field/transition ABAC context for `check` (contract 4.2, `CaveatContext`),
/// evaluated off the hot `list_objects` path (OQ-E). `None` for the common case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaveatContext {
    pub object: ArtifactRef,
    pub field: Option<String>,
    pub transition: Option<String>,
}

/// The per-action decision (contract 4.2). Fail-closed (ADR-03).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Allow,
    Deny,
    Conditional,
}

/// The leak-free pre-filter result (contract 4.3) — a materialised id set OR a
/// pushdownable `Filter`. The `SetExpr` algebra is **Identity's** deliverable (OQ-E);
/// the substrate pins only the call shape, so the variant payloads are opaque stubs here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListObjectsResult {
    /// A materialised id set with its consistency zookie.
    Ids { ids: Vec<ArtifactRef>, zookie: Zookie },
    /// A pushdownable filter (the `SetExpr` algebra lives in Identity's doc).
    Filter { set_expr: SetExpr, zookie: Zookie },
}

/// Placeholder for the consumer-composable set algebra (contract 4.3).
///
/// **Floor:** the real `SetExpr` (All/None/Ids/NotIds/InRelation/Union/Intersect/
/// Difference/TupleSet, lowered to a SQL predicate over the consumer's id column via the
/// authz reverse index) is Identity-M1-owned and deliberately NOT designed in the
/// substrate skeleton. P-001 reserves the type name on the frozen call shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetExpr(pub String);

/// The effective policy of an agent run after delegation intersection (contract 4.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectivePolicy(pub String);

/// Placeholder error type for the skeleton. The real error taxonomy lands with the
/// Identity-M1 impl; P-001 only needs a `Result` shape on the frozen signatures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthzError(pub String);

/// `Result` alias for the authz surface.
pub type Result<T> = core::result::Result<T, AuthzError>;

/// The authz client every service links (contract 4.2/4.3/4.5; ADR-13.3).
///
/// All methods flow through the resilient client (§6) and are the canonical fail-static
/// surface (§8) — that wiring is the substrate's, not implemented here. Bodies are
/// `todo!()` (frozen shape only; Identity M1 fills them).
pub trait AuthzClient {
    /// Per-action gate, fail-closed (contract 4.2; ADR-03). `caveat` carries field/
    /// transition ABAC evaluated here, off the hot `list_objects` path (OQ-E).
    fn check(
        &self,
        subject: &Principal,
        perm: Permission,
        object: &ArtifactRef,
        at: Consistency,
        caveat: Option<CaveatContext>,
    ) -> Result<Decision>;

    /// The leak-free pre-filter (contract 4.3; ADR-03). Returns a materialised id set OR
    /// a pushdownable `Filter`. The single most load-bearing inter-system contract.
    fn list_objects(
        &self,
        subject: &Principal,
        perm: Permission,
        ty: ObjectType,
        at: Consistency,
    ) -> Result<ListObjectsResult>;

    /// `agent ∩ delegation ∩ tenant` (contract 4.5; ADR-08.3, monotone intersection).
    fn delegation(
        &self,
        agent: &Principal,
        trigger_actor: &Principal,
    ) -> Result<EffectivePolicy>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-asserting test: the frozen public surface exists with the frozen field
    /// names (contract 4.2/4.3/4.5/4.10). `Principal{id,kind,tenant}`, `PrincipalKind`'s
    /// three variants, `Consistency{at_least, mode}`, and the `AuthzClient` trait shape.
    /// Drift from the §2.2 anchor stops compilation.
    #[test]
    fn surface_principal_and_consistency_exist() {
        let p = Principal {
            id: PrincipalId("p1".into()),
            kind: PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt".into()),
                on_behalf_of: Some(PrincipalId("human".into())),
            },
            tenant: TenantId("acme".into()),
        };
        assert!(matches!(p.kind, PrincipalKind::Agent { .. }));
        let c = Consistency {
            at_least: Zookie("z".into()),
            mode: ConsistencyMode::Strong,
        };
        assert_eq!(c.mode, ConsistencyMode::Strong);
    }

    /// A stub implementer proves the `AuthzClient` trait is object-safe-shaped and its
    /// three frozen methods take the frozen parameter types (incl. `&ArtifactRef`, the
    /// type the DAG-deviation keeps reachable here without an events back-edge).
    #[test]
    fn authz_client_trait_shape_is_frozen() {
        struct Stub;
        impl AuthzClient for Stub {
            fn check(
                &self,
                _s: &Principal,
                _p: Permission,
                _o: &ArtifactRef,
                _at: Consistency,
                _cav: Option<CaveatContext>,
            ) -> Result<Decision> {
                Ok(Decision::Deny) // fail-closed default; real body is Identity M1.
            }
            fn list_objects(
                &self,
                _s: &Principal,
                _p: Permission,
                _ty: ObjectType,
                _at: Consistency,
            ) -> Result<ListObjectsResult> {
                todo!("list_objects + SetExpr push-down lands in Identity M1 (contract 4.3)")
            }
            fn delegation(
                &self,
                _a: &Principal,
                _t: &Principal,
            ) -> Result<EffectivePolicy> {
                todo!("delegation lands in Identity M1 (contract 4.5)")
            }
        }
        let stub = Stub;
        let p = Principal {
            id: PrincipalId("p".into()),
            kind: PrincipalKind::Service,
            tenant: TenantId("t".into()),
        };
        let d = stub.check(
            &p,
            Permission("read".into()),
            &ArtifactRef("myelin://t/issue/issue/PROJ-1".into()),
            Consistency { at_least: Zookie("z".into()), mode: ConsistencyMode::Strong },
            None,
        );
        assert_eq!(d, Ok(Decision::Deny));
    }
}
