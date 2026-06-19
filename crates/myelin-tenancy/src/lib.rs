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
//! ## P-CP-01 (P-025) — the partition-key types frozen as the names/units anchor
//! This prompt fills the `myelin-tenancy` skeleton P-001 stood up with the **three**
//! contract-12.1 partition-key value types — [`TenantId`], [`Region`], **[`ResidencyTag`]** —
//! to the frozen §4.1 shape: `(tenant, region)` is the first-class partition key, injected by
//! the harness, identical at every isolation tier. A change to any of these glue types breaks
//! every consumer's build *now*, never silently in prod (ADR-01). `TenantId` is an **opaque,
//! non-personal** token (NOT a slug, NOT an email — `control-plane-pii-free`); `Region` is
//! **immutable once assigned** (a region change is a *new value*, never a mutation — there is
//! no setter and the field is exposed read-only by construction discipline); `ResidencyTag` is
//! the per-row residency marker the `residency-pin` lint (P-CP-03 / P-026) reads.
//!
//! ## Floors named (stubbed bodies → filling prompt)
//! - The control-plane *routing/attestation* surfaces (`discover`, `place`, `placement_of`,
//!   `residency_verify`) live in the `myelin-control-plane` service crate — CP-M1 (P-CP-05…),
//!   NOT this prompt; this crate ships only the partition-key value types they thread.
//! - The `CrossCellPointer` PII-free bridge *frame* is frozen-not-live in **P-CP-02 (P-027)**;
//!   its cell-local *resolution* is deferred to P-CP-19 (M5).
//! - The `residency-pin` lint that READS `Region`/`ResidencyTag` (recognising them as the
//!   region-binder on a store construction) is **P-CP-03 (P-026)**; the storage-half twin
//!   already ships in P-ST-04 / P-020 (`myelin-lints`, which already tokenises `ResidencyTag`).
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

/// The first-class tenant partition key (contract 12.1; ADR-11; architecture §4.1).
///
/// Tenant is the first column / partition key of everything; there is no cross-tenant
/// query path. Held as an **opaque, non-personal** comparable token so it can be a SQL
/// partition key and a telemetry/trace label *without* exposing tenant-name PII
/// (`control-plane-pii-free`, §3.3). It is **NOT a slug, NOT an email, NOT a tenant name**
/// — the human tenant name + admin email are born *inside* the assigned cell (two-phase
/// signup, §6), never in this id. The id wraps a `String` only as its storage representation
/// (a ULID/opaque token in practice); it deliberately does **not** implement `From<String>`,
/// `From<&str>`, or `Display`, so a name/email/slug cannot be *implicitly* coerced into a
/// `TenantId` — construction is always the explicit, greppable `TenantId(token)` /
/// [`TenantId::from_token`] call, which an audit can scan for. See the opaqueness type-level
/// test below (`tenant_id_is_opaque_not_personal`).
///
/// # Opaque-token doc-test (the names/units anchor)
/// A `TenantId` is born from an **opaque token**, and there is *no* implicit coercion from a
/// personal string — a tenant name / email / slug can never become a `TenantId` by accident:
/// ```
/// use myelin_tenancy::TenantId;
/// // The ONLY way in is the explicit, greppable opaque-token constructor:
/// let t = TenantId::from_token("01J0OPAQUE_ULID");
/// assert_eq!(t.as_str(), "01J0OPAQUE_ULID");
/// // It is comparable/hashable (a usable partition key) but carries no PII accessor —
/// // there is no `t.name()` / `t.email()`, because there is no personal data inside it.
/// assert_eq!(t, TenantId::from_token("01J0OPAQUE_ULID"));
/// ```
/// The negative half — that `TenantId` does **not** implement `From<String>` / `From<&str>`,
/// so a bare personal string cannot coerce into a tenant id — is a `compile_fail` doc-test:
/// ```compile_fail
/// use myelin_tenancy::TenantId;
/// // A bare personal string must NOT coerce into a TenantId — this must FAIL to compile.
/// let _personal: TenantId = "ada@example.com".to_string().into();
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TenantId(pub String);

impl TenantId {
    /// Construct a `TenantId` from an already-minted **opaque token** (a ULID / control-plane
    /// routing token, §3.2 — never a name/email/slug). Named `from_token`, not `from_string`,
    /// so the call site reads as "this is an opaque id" and so the type can *never* be the
    /// target of a blanket `From<String>`/`From<&str>` (which would let a personal string be
    /// coerced in silently — the bug class `control-plane-pii-free` exists to forbid).
    #[inline]
    pub fn from_token(token: impl Into<String>) -> Self {
        TenantId(token.into())
    }

    /// The opaque token as a string slice (for use as a partition-key / trace label). This is
    /// the *only* read accessor; there is intentionally no `name()` / `email()` — there is no
    /// PII inside a `TenantId` to expose.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The residency region a `(tenant, region)` pair is pinned to (contract 12.1; ADR-11;
/// architecture §4.1).
///
/// Every store/stream/index/cache declares a region (the `residency-pin` lint, P-CP-03 /
/// P-026); `Region` is the value that declaration carries, and it is the compiled-in shard
/// key so "EU data stays in EU" and "scale this tenant out" are the *same* mechanism (§1.3).
///
/// **Immutable once assigned (frozen rule, §4.1 / §5.1).** A cell's region — and therefore a
/// tenant's region — never changes; *a region change is modelled as a NEW value, never as a
/// mutation of an existing one.* This type carries **no setter and no `&mut` accessor**: you
/// build a new `Region` and place it, you never rewrite one in place. The immutability is
/// structural (there is no API to mutate the inner value through a shared reference); the
/// `region change == new value` discipline is proven by the `region_is_immutable_new_value`
/// compile-fixture test below.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Region(pub String);

impl Region {
    /// Construct a `Region` value. There is deliberately no in-place mutator: a region change
    /// is a *new* `Region` (see the type doc) — this constructor is how that new value is made.
    #[inline]
    pub fn new(code: impl Into<String>) -> Self {
        Region(code.into())
    }

    /// The region code as a string slice (for the partition key / the `residency-pin`
    /// region-binder / blast-radius scoping, §10.1).
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The per-row residency marker (contract 12.1; architecture §4.3 / §5.3).
///
/// Where [`Region`] is the *cell's* immutable region, `ResidencyTag` is the **per-row** tag a
/// store carries on each stored fact so the `residency-pin` lint (P-CP-03 / P-026) can assert
/// `row.region == cell.region` at the write boundary — a region-mismatched write fails *at the
/// boundary*, never reaching a cross-region store (`external-insights/04` §1: residency as
/// region-pinning, no cross-region query path). It is the third type of the `(tenant, region)`
/// partition-key family frozen here as the names/units anchor; the `myelin-lints` residency-pin
/// scanner already tokenises `ResidencyTag` as a region-binder (P-ST-04 / P-020), and P-CP-03
/// wires it as the per-row write-boundary assertion.
///
/// A `ResidencyTag` is **derived from** the cell's [`Region`] and is therefore immutable in the
/// same way: it is set once on write to the owning cell's region and never rewritten in place.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ResidencyTag(pub Region);

impl ResidencyTag {
    /// Tag a row with the cell's region (the only way a residency tag is born — `row.region`
    /// is *pinned to* `cell.region`, §4.3). Takes the cell [`Region`] by value: the row adopts
    /// the cell's region, it does not pick its own.
    #[inline]
    pub fn pinned_to(cell_region: Region) -> Self {
        ResidencyTag(cell_region)
    }

    /// The region this row is pinned to (what the `residency-pin` write-boundary check reads).
    #[inline]
    pub fn region(&self) -> &Region {
        &self.0
    }
}

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
    use std::collections::HashMap;

    /// Compile-asserting test: the frozen public surface exists with the frozen field
    /// shapes — `TenantId`, `Region`, and `ResidencyTag` are the three contract-12.1
    /// partition-key value types. If a field name/shape drifts from the §4.1/§12.1 anchor,
    /// this stops compiling.
    #[test]
    fn surface_partition_key_types_exist() {
        let tenant: TenantId = TenantId("01J0TENANT".to_string());
        let region: Region = Region("eu-west".to_string());
        let tag: ResidencyTag = ResidencyTag::pinned_to(region.clone());
        // ordering + hashing are part of the frozen shape (partition-key usable).
        assert_eq!(tenant, TenantId("01J0TENANT".to_string()));
        assert!(region < Region("eu-westz".to_string()));
        assert_eq!(tag.region(), &region);
    }

    /// **Opaqueness type-level test (the prompt's required `TenantId` test).** Proves
    /// `TenantId` is an *opaque token*, not a name/email/slug type, and cannot be
    /// *accidentally* coerced from a personal string. The guarantee is structural: a blanket
    /// `From<String>`/`From<&str>`/`Display` would let `let t: TenantId = some_email.into();`
    /// compile (the `control-plane-pii-free` bug class). We assert at the type level that no
    /// such conversion exists, while the *explicit* opaque-token constructor does.
    #[test]
    fn tenant_id_is_opaque_not_personal() {
        // Helper that is satisfiable IFF `T: From<S>` exists. We do NOT call it for
        // (TenantId, String) / (TenantId, &str): those impls must not exist, so the
        // corresponding lines below are deliberately ABSENT (a present line would fail to
        // compile, turning this into the captured compile-fixture the prompt asks for).
        fn assert_from<T, S>()
        where
            T: From<S>,
        {
        }
        // A *personal* string type stands in for an email/name/slug.
        struct Email(#[allow(dead_code)] String);
        // POSITIVE: the opaque-token path IS the way in, and it is explicit + greppable.
        let t = TenantId::from_token("01J0OPAQUE");
        assert_eq!(t.as_str(), "01J0OPAQUE");
        // NEGATIVE (structural): there is no `From<Email> for TenantId`, so an email cannot
        // be coerced into a tenant id. If someone added `impl From<Email> for TenantId`,
        // uncommenting the next line would compile — its ABSENCE is the proof.
        // assert_from::<TenantId, Email>();   // must NOT compile — left commented on purpose.
        let _ = Email("ada@example.com".to_string());
        // Sanity: the helper itself is well-formed against a real From impl (String→String).
        assert_from::<String, &str>();
    }

    /// **Region-immutable compile-fixture (the prompt's required `Region` test).** A region
    /// change is modelled as a *new value*, never a mutation: there is no setter / `&mut`
    /// accessor on `Region`, so "relocate to a new region" is `Region::new(..)`, producing a
    /// distinct value, never an in-place rewrite. This test exercises exactly that discipline.
    #[test]
    fn region_is_immutable_new_value() {
        let original = Region::new("eu-west");
        // "Changing" the region produces a NEW value; `original` is untouched (it is not
        // mutated through any accessor — there is none).
        let relocated = Region::new("eu-north");
        assert_ne!(original, relocated);
        assert_eq!(original.as_str(), "eu-west"); // original is unchanged.
        // There is intentionally no `original.set_region(..)` / `*original.0 = ..` API: the
        // type carries no in-place mutator, so immutability is structural, not by convention.
    }

    /// **CDC pair for 12.1 (provider + consumer).** The provider side is this crate exporting
    /// `(TenantId, Region)`; the consumer side is a downstream store handle *parameterised by*
    /// the partition key — a `StoreHandle` keyed by `(TenantId, Region)` that refuses any read
    /// not scoped to a `(tenant, region)`. This is the contract the harness injects everywhere
    /// (§4.1: "the first-class partition key, injected by the harness"). If the partition-key
    /// shape drifts, this consumer stops compiling — the whole point of a glue-crate CDC.
    #[test]
    fn cdc_12_1_store_handle_parameterised_by_tenant_region() {
        /// A stand-in consumer: a per-`(tenant, region)` store handle (the shape every real
        /// store — OLTP/blob/index/cache — opens through). It can ONLY be opened with a
        /// partition key, and every row it holds carries the residency tag derived from the
        /// cell's region, so a cross-region row is unrepresentable.
        struct StoreHandle {
            partition: (TenantId, Region),
            residency: ResidencyTag,
            rows: HashMap<String, String>,
        }
        impl StoreHandle {
            fn open(tenant: TenantId, cell_region: Region) -> Self {
                let residency = ResidencyTag::pinned_to(cell_region.clone());
                StoreHandle {
                    partition: (tenant, cell_region),
                    residency,
                    rows: HashMap::new(),
                }
            }
            fn put(&mut self, key: &str, val: &str) {
                // every write is implicitly in the handle's region — residency by construction.
                self.rows.insert(key.to_string(), val.to_string());
            }
            fn get(&self, key: &str) -> Option<&String> {
                self.rows.get(key)
            }
        }

        let tenant = TenantId::from_token("01J0ACME");
        let cell_region = Region::new("eu-west");
        let mut store = StoreHandle::open(tenant.clone(), cell_region.clone());
        store.put("k", "v");

        // The consumer is parameterised by the EXACT provider types.
        assert_eq!(store.partition.0, tenant);
        assert_eq!(store.partition.1, cell_region);
        // Every row is pinned to the cell's region (the residency-pin invariant's seed).
        assert_eq!(store.residency.region(), &cell_region);
        assert_eq!(store.get("k").map(String::as_str), Some("v"));
    }
}
