use myelin_tenancy::{Region, TenantId};

use crate::schema::IsolationKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IsolationTier {
    Logical,
    Schema,
    Db,
    Cell,
}

impl IsolationTier {
    pub fn for_cell_class(class: IsolationKind) -> IsolationTier {
        match class {
            IsolationKind::Pool => IsolationTier::Logical,
            IsolationKind::Bridge => IsolationTier::Db,
            IsolationKind::Dedicated => IsolationTier::Cell,
        }
    }

    pub fn resolve(requested_tier: IsolationKind) -> IsolationTier {
        IsolationTier::for_cell_class(requested_tier)
    }

    pub fn is_v1_floor(self) -> bool {
        self == IsolationTier::Logical
    }

    pub fn is_declared_on_demand(self) -> bool {
        !self.is_v1_floor()
    }

    pub fn as_contract_token(self) -> &'static str {
        match self {
            IsolationTier::Logical => "logical",
            IsolationTier::Schema => "schema",
            IsolationTier::Db => "db",
            IsolationTier::Cell => "cell",
        }
    }

    pub const ALL: [IsolationTier; 4] = [
        IsolationTier::Logical,
        IsolationTier::Schema,
        IsolationTier::Db,
        IsolationTier::Cell,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartitionKey {
    pub tenant: TenantId,
    pub region: Region,
}

impl PartitionKey {
    pub fn for_tier(tenant: TenantId, region: Region, _tier: IsolationTier) -> PartitionKey {
        PartitionKey { tenant, region }
    }
}

pub fn partition_key(tenant: TenantId, region: Region, tier: IsolationTier) -> PartitionKey {
    PartitionKey::for_tier(tenant, region, tier)
}

#[derive(Clone, Debug)]
pub struct PoolStore {
    tier: IsolationTier,
    partition: PartitionKey,
}

impl PoolStore {
    pub fn open(tenant: TenantId, region: Region) -> PoolStore {
        let tier = IsolationTier::Logical;
        let partition = partition_key(tenant, region, tier);
        PoolStore { tier, partition }
    }

    pub fn tier(&self) -> IsolationTier {
        self.tier
    }

    pub fn partition(&self) -> &PartitionKey {
        &self.partition
    }

    pub fn rls_tenant(&self) -> &TenantId {
        &self.partition.tenant
    }

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
        assert_eq!(IsolationTier::ALL.len(), 4);
    }

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
        assert_eq!(
            IsolationTier::for_cell_class(IsolationKind::Pool),
            IsolationTier::resolve(IsolationKind::Pool)
        );
    }

    #[test]
    fn partition_key_is_identical_at_every_tier() {
        let floor_key = partition_key(tenant(), region(), IsolationTier::Logical);
        for tier in IsolationTier::ALL {
            let key = partition_key(tenant(), region(), tier);
            assert_eq!(
                key, floor_key,
                "the `(tenant, region)` partition key MUST be identical at the `{}` tier as at the \
                 Pool floor - the tier changes where bytes live, NEVER the shard key (§4.1)",
                tier.as_contract_token()
            );
            assert_eq!(key.tenant, tenant());
            assert_eq!(key.region, region());
        }
    }

    #[test]
    fn pool_store_opens_with_the_tier_invariant_partition_key() {
        let store = PoolStore::open(tenant(), region());
        assert_eq!(store.tier(), IsolationTier::Logical);
        assert!(store.tier().is_v1_floor());

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

    #[test]
    fn cdc_12_5_store_opens_at_pool_tier_with_partition_key() {
        struct SharedSystemStore {
            partition: PartitionKey,
        }
        impl SharedSystemStore {
            fn open_at(tenant: TenantId, region: Region, tier: IsolationTier) -> SharedSystemStore {
                SharedSystemStore {
                    partition: partition_key(tenant, region, tier),
                }
            }
            fn rls_tenant(&self) -> &TenantId {
                &self.partition.tenant
            }
        }

        let pool = PoolStore::open(tenant(), region());
        assert!(pool.tier().is_v1_floor());

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

        for higher in [IsolationTier::Db, IsolationTier::Cell] {
            let promoted = SharedSystemStore::open_at(tenant(), region(), higher);
            assert_eq!(
                promoted.partition,
                consumer_pool.partition,
                "promoting the tenant to the `{}` tier MUST NOT move the partition key - it is a \
                 provisioning change, not a code change (§4.1)",
                higher.as_contract_token()
            );
        }
    }

    #[test]
    fn the_tier_mechanism_is_distinct_from_the_cell_class() {
        let tier = IsolationTier::for_cell_class(IsolationKind::Dedicated);
        assert_eq!(tier, IsolationTier::Cell);
        assert_eq!(tier.as_contract_token(), "cell");
        assert_eq!(
            IsolationTier::for_cell_class(IsolationKind::Pool),
            IsolationTier::Logical
        );
    }
}
