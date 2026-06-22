//! # The isolation-tier contract (12.5) — the Pool tier (the v1 floor) + Bridge/Dedicated on-demand
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! §7.1 (the isolation matrix — **Pool** = shared cell, logical/RLS isolation, the long tail = the
//! v1 floor; **Bridge** = DB-per-tenant within a shared cell; **Dedicated** = cell-per-tenant,
//! public-sector/high-assurance; the three classes map 1:1 to the isolation tier) and §4.1 (12.5 —
//! **the partition key `(tenant, region)` is identical at every tier**). Contract-index **row 12.5**
//! (the isolation-tier contract — `logical|schema|db|cell`; the partition key identical at every
//! tier).
//!
//! ## What this prompt (P-CP-10 / P-086) ships
//! 1. **The isolation-tier contract ([`IsolationTier`])** — the frozen `logical|schema|db|cell`
//!    enumeration of *how* a tenant is isolated within (or as) a cell. This is the **mechanism**
//!    contract (row 12.5), distinct from the cell-class [`crate::schema::IsolationKind`]
//!    (`Pool|Bridge|Dedicated`, the §7.1 sizing class): a cell *class* maps 1:1 to an isolation
//!    *tier* via [`IsolationTier::for_cell_class`].
//! 2. **The Pool tier as the v1 floor** — `Pool → logical/RLS`. The shared-cell, logical-isolation
//!    tier that the `residency-pin` lint + the OLTP RLS guard already enforce. This is the only tier
//!    a v1 cell actually provisions; Bridge/Dedicated are *declared* in the contract but provisioned
//!    on demand.
//! 3. **`resolve` ([`IsolationTier::resolve`]) — `requested_tier → IsolationTier`** — a `place`
//!    caller's `requested_tier` (a cell *class*) resolves to the isolation *tier* the assigned cell
//!    serves it at. v1 resolves Pool to the logical floor; Bridge/Dedicated resolve to their
//!    declared tiers (the higher tiers are a **provisioning** concern, not a redesign — the
//!    partition-key contract is identical across all three).
//! 4. **The partition key is identical at every tier ([`partition_key`])** — a store opens at the
//!    Pool tier with the **same** `(tenant, region)` [`PartitionKey`] that Bridge or Dedicated would
//!    use. The tier changes *where the bytes live* (a shared table behind RLS / a per-tenant DB / a
//!    per-tenant cell), it NEVER changes the partition key the RLS predicate filters on. This is the
//!    load-bearing §4.1 invariant: "scale this tenant out" (Pool→Bridge→Dedicated) and "EU data
//!    stays in EU" are the SAME mechanism, because the shard key does not move.
//!
//! ## Why the partition key is tier-invariant (the load-bearing distinction, §4.1)
//! The whole point of CP-M1 is that promoting a tenant from Pool to Bridge to Dedicated is a
//! **provisioning** change, never a **code** change: every consumer keys its store/stream/index/
//! cache on `(tenant, region)` regardless of the isolation tier (the harness injects the SAME
//! partition key, contract 12.1). At the Pool tier the RLS predicate `tenant_id = :partition.tenant`
//! filters a shared table; at Bridge the same key selects the per-tenant DB; at Dedicated the same
//! key selects the per-tenant cell — but the key the application code carries is byte-identical. A
//! design where the higher tiers used a *different* key (e.g. a schema name, a DB handle) would
//! force a code change to promote a tenant — exactly what 12.5 forbids. [`partition_key`] therefore
//! returns the SAME [`PartitionKey`] for every tier, asserted across all of `logical|schema|db|cell`
//! in the tests + the CDC.
//!
//! ## The isolation-tier leg (the GATE/DRILL — CI, no new runtime drill)
//! [`PoolStore::open`] opens a store at the **Pool tier** with the identical `(tenant, region)`
//! partition key Bridge/Dedicated would use; the RLS + `residency-pin` enforce logical isolation at
//! that tier (the RLS predicate filters on [`PartitionKey::tenant`]; the region pin holds on
//! [`PartitionKey::region`]). The Pool tier's cross-tenant-read *correctness* is the floor proven by
//! **CP-D2** (the gateway misroute-reject, P-CP-08) + the **four-layer enforcement** (P-CP-12) — NOT
//! re-proven here; this leg confirms the **partition key is tier-invariant** (§4.1) so promoting a
//! tenant across tiers never moves the shard key.
//!
//! ## Floor named (deferred bodies → filling prompt) — VISION §3 name-your-floors
//! - **The Pool tier is the v1 isolation floor.** It is the ONLY tier a v1 cell provisions (the
//!   shared-cell, logical/RLS isolation). The **Bridge** (DB-per-tenant) and **Dedicated**
//!   (cell-per-tenant) tiers are **declared in the contract here but provisioned ON DEMAND** —
//!   their concrete provisioning (a per-tenant DB / a per-tenant cell stood up at the higher tier)
//!   is the **enterprise / public-sector onboarding follow-on**: the Bridge-tier per-tenant index/DB
//!   provisioning is the `[OPEN → P6 (Search/Storage)]` residual the architecture names (§13), and
//!   the durable provisioning of a higher-tier cell rides the provisioning-gate path (P-CP-11) +
//!   the live-migration follow-on (P-CP-22). The contract shape ([`IsolationTier`] + the
//!   tier-invariant [`PartitionKey`]) is FROZEN now and does not change when the higher tiers are
//!   provisioned — promoting a tenant is a provisioning concern, not a redesign (the §4.1
//!   invariant). Recorded here in writing per VISION §3.

use myelin_tenancy::{Region, TenantId};

use crate::schema::IsolationKind;

/// **The isolation-tier contract (contract-index row 12.5; architecture §7.1).** The frozen
/// enumeration of *how* a tenant is isolated — `logical | schema | db | cell` — ordered from the
/// shared Pool floor (logical/RLS) to the fully-dedicated cell. This is the **mechanism** the
/// platform isolates a tenant by; the cell-class [`IsolationKind`] (`Pool|Bridge|Dedicated`, §7.1)
/// maps 1:1 onto it ([`IsolationTier::for_cell_class`]).
///
/// **The partition key `(tenant, region)` is identical at every tier (§4.1).** The tier changes
/// *where the bytes live*, never the shard key the application carries (see [`partition_key`]). The
/// higher tiers are a **provisioning** concern, not a redesign.
///
/// The `Logical` tier is the **v1 floor** (Pool). `Schema`, `Db`, and `Cell` are **declared** in the
/// contract but **provisioned on demand** (the named floor — enterprise / public-sector onboarding).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IsolationTier {
    /// **The v1 floor (Pool):** logical / RLS isolation in a shared cell. Many tenants share the
    /// cell's tables; the RLS predicate filters on the `(tenant, region)` partition key. This is the
    /// only tier a v1 cell provisions; it is the long tail (§7.1).
    Logical,
    /// **Schema-per-tenant** isolation within a shared cell (a declared-on-demand tier — a tenant
    /// gets its own schema/namespace; the partition key is unchanged).
    Schema,
    /// **DB-per-tenant (Bridge)** isolation within a shared cell (a declared-on-demand tier — a
    /// tenant gets its own database; the partition key is unchanged).
    Db,
    /// **Cell-per-tenant (Dedicated)** isolation (a declared-on-demand tier — a tenant gets its own
    /// cell; public-sector / high-assurance; the partition key is unchanged).
    Cell,
}

impl IsolationTier {
    /// **The cell-class → isolation-tier mapping (architecture §7.1 — the three classes map 1:1 to
    /// the isolation tier).** A `Pool` cell isolates logically (the v1 floor); a `Bridge` cell
    /// isolates by DB-per-tenant; a `Dedicated` cell isolates by cell-per-tenant. The `Schema` tier
    /// is the finer-grained declared-on-demand option between Pool and Bridge (it has no distinct
    /// *cell class* — it is a provisioning variant of Pool/Bridge; v1 never resolves to it from a
    /// cell class, so the mapping is total over the three §7.1 classes).
    pub fn for_cell_class(class: IsolationKind) -> IsolationTier {
        match class {
            IsolationKind::Pool => IsolationTier::Logical,
            IsolationKind::Bridge => IsolationTier::Db,
            IsolationKind::Dedicated => IsolationTier::Cell,
        }
    }

    /// **`resolve(requested_tier) → IsolationTier` (contract 12.5).** A `place` caller's
    /// `requested_tier` (a cell *class* — Pool/Bridge/Dedicated, the argument to
    /// [`crate::PlacementService::place`]) resolves to the isolation *tier* the assigned cell serves
    /// it at. This is the contract's resolution entry point: the requested cell class becomes the
    /// concrete isolation mechanism. v1 resolves `Pool → Logical` (the floor); `Bridge → Db` and
    /// `Dedicated → Cell` are the declared-on-demand tiers.
    pub fn resolve(requested_tier: IsolationKind) -> IsolationTier {
        IsolationTier::for_cell_class(requested_tier)
    }

    /// Whether this tier is the **v1 floor** (the Pool / logical tier — the only tier a v1 cell
    /// actually provisions). `Schema`/`Db`/`Cell` are declared-on-demand (the named floor).
    pub fn is_v1_floor(self) -> bool {
        self == IsolationTier::Logical
    }

    /// Whether this tier is **declared-on-demand** (provisioned only for enterprise / public-sector
    /// onboarding — the named floor). Every tier above the Pool floor is on-demand.
    pub fn is_declared_on_demand(self) -> bool {
        !self.is_v1_floor()
    }

    /// The frozen contract token for this tier (`logical|schema|db|cell`) — the row-12.5 wire name.
    /// Kept in lock-step with the enum by [`IsolationTier::ALL`] + the contract-token test.
    pub fn as_contract_token(self) -> &'static str {
        match self {
            IsolationTier::Logical => "logical",
            IsolationTier::Schema => "schema",
            IsolationTier::Db => "db",
            IsolationTier::Cell => "cell",
        }
    }

    /// The full frozen tier set the contract enumerates (`logical|schema|db|cell`, in tier order).
    /// The tests assert the partition key is identical across EVERY entry here (tier-invariance).
    pub const ALL: [IsolationTier; 4] = [
        IsolationTier::Logical,
        IsolationTier::Schema,
        IsolationTier::Db,
        IsolationTier::Cell,
    ];
}

/// **The `(tenant, region)` partition key (contract 12.1) — identical at every isolation tier
/// (contract 12.5 / architecture §4.1).** This is the first-class shard key the harness injects into
/// every store/stream/index/cache; the RLS predicate filters on its [`PartitionKey::tenant`] and the
/// residency pin holds on its [`PartitionKey::region`]. It is constructed [`PartitionKey::for_tier`]
/// IDENTICALLY for `logical|schema|db|cell` — the tier changes where the bytes live, never the key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartitionKey {
    /// The tenant column (the first-class partition key, contract 12.1) — the RLS predicate filters
    /// on this at EVERY tier.
    pub tenant: TenantId,
    /// The residency region (immutable, contract 12.1) — the region pin holds on this at EVERY tier.
    pub region: Region,
}

impl PartitionKey {
    /// Build the `(tenant, region)` partition key for a store opening at `_tier`. The `_tier` is
    /// **deliberately ignored**: the partition key is identical at every isolation tier (§4.1). The
    /// argument is present to make the tier-invariance *explicit at the call site* — a future change
    /// that tried to vary the key by tier would have to thread the tier into the key, which this
    /// contract forbids (and the tests catch). The key is `(tenant, region)` for Pool exactly as it
    /// is for Bridge/Dedicated.
    pub fn for_tier(tenant: TenantId, region: Region, _tier: IsolationTier) -> PartitionKey {
        PartitionKey { tenant, region }
    }
}

/// **`partition_key(tenant, region, tier)` — the tier-invariant partition key (contract 12.5 /
/// §4.1).** A free-function entry point over [`PartitionKey::for_tier`]: it returns the SAME
/// `(tenant, region)` key for EVERY `tier ∈ {logical, schema, db, cell}`. The tests assert the
/// returned key is byte-identical across all four tiers — promoting a tenant from Pool to
/// Bridge/Dedicated never moves the shard key (the load-bearing §4.1 invariant).
pub fn partition_key(tenant: TenantId, region: Region, tier: IsolationTier) -> PartitionKey {
    PartitionKey::for_tier(tenant, region, tier)
}

/// **A store opened at the Pool (logical) tier (the v1 floor) — the isolation-tier leg.** It carries
/// the identical `(tenant, region)` [`PartitionKey`] that a Bridge/Dedicated store would carry; the
/// RLS predicate filters on [`PartitionKey::tenant`] and the residency pin holds on
/// [`PartitionKey::region`]. This is the §7.1 Pool floor: a shared cell, logical/RLS isolation.
///
/// On this floor the store is an in-process stand-in keyed by the partition key (the concrete OLTP
/// pool + the RLS predicate are Storage-owned, P-ST-01 — this confirms the *partition key the RLS
/// filters on* is the tier-invariant `(tenant, region)`, the thing this prompt owns). A real OLTP
/// store opens through the SAME [`PartitionKey`] (the harness injects it) regardless of tier.
#[derive(Clone, Debug)]
pub struct PoolStore {
    /// The isolation tier this store is opened at (the Pool floor = [`IsolationTier::Logical`]).
    tier: IsolationTier,
    /// The `(tenant, region)` partition key the RLS predicate filters on (identical at every tier).
    partition: PartitionKey,
}

impl PoolStore {
    /// **Open a store at the Pool (logical) tier with the `(tenant, region)` partition key.** The
    /// key is built [`partition_key`] for the Pool floor — byte-identical to the key a Bridge or
    /// Dedicated store would use. The RLS predicate this store filters on reads
    /// [`PartitionKey::tenant`]; the region pin reads [`PartitionKey::region`].
    pub fn open(tenant: TenantId, region: Region) -> PoolStore {
        let tier = IsolationTier::Logical; // the v1 floor.
        let partition = partition_key(tenant, region, tier);
        PoolStore { tier, partition }
    }

    /// The isolation tier this store is opened at ([`IsolationTier::Logical`] — the Pool floor).
    pub fn tier(&self) -> IsolationTier {
        self.tier
    }

    /// The `(tenant, region)` partition key the RLS predicate filters on (identical at every tier —
    /// the §4.1 invariant this whole prompt confirms).
    pub fn partition(&self) -> &PartitionKey {
        &self.partition
    }

    /// **The RLS predicate's tenant binder** — the tenant the shared-cell RLS filters on at the Pool
    /// tier. This is byte-identical to the tenant a Bridge/Dedicated store would carry: the RLS
    /// predicate `tenant_id = :tenant` filters on the SAME partition-key tenant at every tier (only
    /// *where the rows live* differs). The cross-tenant-read correctness this enables is the floor
    /// proven by CP-D2 (P-CP-08) + the four-layer enforcement (P-CP-12).
    pub fn rls_tenant(&self) -> &TenantId {
        &self.partition.tenant
    }

    /// **The residency-pin region binder** — the region this Pool-tier store is pinned to (identical
    /// at every tier; the region pin holds on the partition-key region).
    pub fn pinned_region(&self) -> &Region {
        &self.partition.region
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId::from_token("01J0ACME")
    }

    fn region() -> Region {
        Region::new("eu-west")
    }

    /// **The isolation-tier contract enumerates `logical|schema|db|cell` (contract-index row 12.5).**
    /// All four tiers exist, in tier order, with the frozen wire tokens.
    #[test]
    fn isolation_tier_contract_enumerates_logical_schema_db_cell() {
        let tokens: Vec<&str> = IsolationTier::ALL
            .iter()
            .map(|t| t.as_contract_token())
            .collect();
        assert_eq!(
            tokens,
            ["logical", "schema", "db", "cell"],
            "the frozen 12.5 tier set"
        );
        // The set is exactly four (a fifth tier would change the frozen contract — caught here).
        assert_eq!(IsolationTier::ALL.len(), 4);
    }

    /// **The Pool tier is the v1 floor; Bridge/Dedicated (and Schema) are declared-on-demand.** The
    /// logical tier is the only v1-provisioned tier; every higher tier is on-demand (the named floor).
    #[test]
    fn pool_logical_is_the_v1_floor_others_on_demand() {
        assert!(
            IsolationTier::Logical.is_v1_floor(),
            "Pool/logical is the v1 floor"
        );
        assert!(!IsolationTier::Logical.is_declared_on_demand());
        for higher in [
            IsolationTier::Schema,
            IsolationTier::Db,
            IsolationTier::Cell,
        ] {
            assert!(!higher.is_v1_floor(), "{higher:?} is NOT the v1 floor");
            assert!(
                higher.is_declared_on_demand(),
                "{higher:?} is declared-on-demand (the floor)"
            );
        }
    }

    /// **`resolve(requested_tier)` resolves a requested cell class to a tier (contract 12.5).** Pool
    /// resolves to the logical floor; Bridge → db; Dedicated → cell — the three §7.1 classes map 1:1.
    #[test]
    fn resolve_maps_requested_cell_class_to_a_tier() {
        assert_eq!(
            IsolationTier::resolve(IsolationKind::Pool),
            IsolationTier::Logical
        );
        assert_eq!(
            IsolationTier::resolve(IsolationKind::Bridge),
            IsolationTier::Db
        );
        assert_eq!(
            IsolationTier::resolve(IsolationKind::Dedicated),
            IsolationTier::Cell
        );
        // for_cell_class is the same mapping (resolve is its `place`-facing alias).
        assert_eq!(
            IsolationTier::for_cell_class(IsolationKind::Pool),
            IsolationTier::resolve(IsolationKind::Pool)
        );
    }

    /// **THE LOAD-BEARING INVARIANT (architecture §4.1): the partition key `(tenant, region)` is
    /// IDENTICAL at every isolation tier.** `partition_key` returns a byte-identical
    /// `(tenant, region)` key for EVERY tier in `logical|schema|db|cell` — promoting a tenant from
    /// Pool to Bridge/Dedicated never moves the shard key.
    #[test]
    fn partition_key_is_identical_at_every_tier() {
        let floor_key = partition_key(tenant(), region(), IsolationTier::Logical);
        for tier in IsolationTier::ALL {
            let key = partition_key(tenant(), region(), tier);
            assert_eq!(
                key, floor_key,
                "the `(tenant, region)` partition key MUST be identical at the `{}` tier as at the \
                 Pool floor — the tier changes where bytes live, NEVER the shard key (§4.1)",
                tier.as_contract_token()
            );
            // The key is exactly the `(tenant, region)` pair — no tier component leaks in.
            assert_eq!(key.tenant, tenant());
            assert_eq!(key.region, region());
        }
    }

    /// **The isolation-tier leg (the GATE/DRILL): a store opens at the Pool tier with the identical
    /// `(tenant, region)` partition key as Bridge/Dedicated would use; the RLS + residency-pin
    /// enforce logical isolation at the Pool tier.** The store's RLS tenant binder + pinned region
    /// are the partition key's `(tenant, region)` — the same key a higher-tier store would carry.
    #[test]
    fn pool_store_opens_with_the_tier_invariant_partition_key() {
        let store = PoolStore::open(tenant(), region());
        // Opened at the Pool floor (logical/RLS isolation, §7.1).
        assert_eq!(store.tier(), IsolationTier::Logical);
        assert!(store.tier().is_v1_floor());

        // The RLS predicate filters on the partition-key tenant; the region pin holds on its region.
        assert_eq!(
            store.rls_tenant(),
            &tenant(),
            "the RLS predicate filters on the partition tenant"
        );
        assert_eq!(
            store.pinned_region(),
            &region(),
            "the residency pin holds on the partition region"
        );

        // The Pool-tier key is byte-identical to the key Bridge/Dedicated would carry (tier-invariant).
        let bridge_key = partition_key(tenant(), region(), IsolationTier::Db);
        let dedicated_key = partition_key(tenant(), region(), IsolationTier::Cell);
        assert_eq!(
            store.partition(),
            &bridge_key,
            "Pool key == Bridge key (the partition is tier-invariant)"
        );
        assert_eq!(
            store.partition(),
            &dedicated_key,
            "Pool key == Dedicated key (the partition is tier-invariant)"
        );
    }

    /// **CDC pair for 12.5 (provider + consumer).** The PROVIDER is this module's isolation-tier
    /// contract + the tier-invariant [`partition_key`]: a store opening at the Pool tier with the
    /// `(tenant, region)` partition key ([`PoolStore`]). The CONSUMER stands in for **any shared
    /// system** (every consumer keys its store on the harness-injected partition key, regardless of
    /// tier): it opens a store at the Pool tier, reads back ONLY the `(tenant, region)` partition key
    /// it filters by, and — load-bearing — gets the SAME key when the tenant is later promoted to a
    /// higher tier (Bridge/Dedicated). If the partition key drifted with the tier, the consumer would
    /// read a different key after promotion — the assertion below would fail. If the tier-contract
    /// shape drifts (a field added/removed/retyped on [`PartitionKey`], a tier removed), the consumer
    /// stops compiling — the point of a glue-crate CDC.
    #[test]
    fn cdc_12_5_store_opens_at_pool_tier_with_partition_key() {
        /// A stand-in **shared-system store handle** consumer (the shape every real store —
        /// OLTP/blob/index/cache — opens through). It is parameterised by the `(tenant, region)`
        /// partition key the harness injects; it does NOT know or care about the isolation tier (the
        /// tier is a provisioning concern). It filters every read by the partition tenant (RLS) within
        /// the pinned region.
        struct SharedSystemStore {
            partition: PartitionKey,
        }
        impl SharedSystemStore {
            /// Open against the partition key the contract hands it at a given tier — the consumer
            /// takes the key, never the tier (the key is tier-invariant).
            fn open_at(tenant: TenantId, region: Region, tier: IsolationTier) -> SharedSystemStore {
                SharedSystemStore {
                    partition: partition_key(tenant, region, tier),
                }
            }
            /// The RLS tenant the store filters every read by (the partition tenant — tier-invariant).
            fn rls_tenant(&self) -> &TenantId {
                &self.partition.tenant
            }
        }

        // PROVIDER: a store opens at the Pool (v1 floor) tier with the `(tenant, region)` key.
        let pool = PoolStore::open(tenant(), region());
        assert!(pool.tier().is_v1_floor());

        // CONSUMER: a shared system opens its store at the Pool tier with the SAME partition key.
        let consumer_pool = SharedSystemStore::open_at(tenant(), region(), IsolationTier::Logical);
        assert_eq!(
            consumer_pool.partition,
            *pool.partition(),
            "consumer keys on the same partition"
        );
        assert_eq!(
            consumer_pool.rls_tenant(),
            pool.rls_tenant(),
            "same RLS tenant binder"
        );

        // PROMOTION: the tenant is later promoted to Bridge then Dedicated (a PROVISIONING change).
        // The consumer's partition key is byte-identical — the shard key did not move (§4.1).
        for higher in [IsolationTier::Db, IsolationTier::Cell] {
            let promoted = SharedSystemStore::open_at(tenant(), region(), higher);
            assert_eq!(
                promoted.partition,
                consumer_pool.partition,
                "promoting the tenant to the `{}` tier MUST NOT move the partition key — it is a \
                 provisioning change, not a code change (§4.1)",
                higher.as_contract_token()
            );
        }
    }

    /// `Cell` (the contract tier name) is distinct from [`crate::schema::IsolationKind`] (the cell
    /// class). The two are bridged by [`IsolationTier::for_cell_class`]; this documents the seam so a
    /// reader does not conflate the `logical|schema|db|cell` mechanism with the `Pool|Bridge|Dedicated`
    /// sizing class.
    #[test]
    fn the_tier_mechanism_is_distinct_from_the_cell_class() {
        // The cell-class Dedicated maps to the `Cell` isolation tier — they are NOT the same type.
        let tier = IsolationTier::for_cell_class(IsolationKind::Dedicated);
        assert_eq!(tier, IsolationTier::Cell);
        assert_eq!(tier.as_contract_token(), "cell");
        // The Pool cell class maps to the logical floor (NOT a `Pool` tier — there is none).
        assert_eq!(
            IsolationTier::for_cell_class(IsolationKind::Pool),
            IsolationTier::Logical
        );
    }
}
