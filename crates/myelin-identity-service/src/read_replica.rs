use myelin_identity::{Consistency, ConsistencyMode};
use myelin_storage::{OltpStoreHolder, TenantQuery, TenantScope, TenantTable};
use myelin_substrate::Seconds;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const S5_TABLE: &str = "authz_read_replica";

pub const S5_HOLDER: &str = "identity_authz_read_replica";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaRow {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct PartKey {
    tenant: String,
    region: String,
}

#[derive(Default)]
struct Partition {
    rows: BTreeMap<String, ReplicaRow>,
    applied_offset: u64,
}

#[derive(Default)]
struct Inner {
    partitions: BTreeMap<PartKey, Partition>,
}

#[derive(Clone)]
pub struct AuthzReadReplica {
    inner: Arc<Mutex<Inner>>,
    holder: OltpStoreHolder,
    telemetry: Arc<ReplicaTelemetry>,
}

impl Default for AuthzReadReplica {
    fn default() -> Self {
        AuthzReadReplica::new()
    }
}

impl AuthzReadReplica {
    pub const DEFAULT_MAX_REPLICATION_LAG_SECS: Seconds = 30;

    pub fn new() -> AuthzReadReplica {
        let holder = OltpStoreHolder::new(S5_HOLDER);
        let _receipt = holder.register();
        AuthzReadReplica {
            inner: Arc::new(Mutex::new(Inner::default())),
            holder,
            telemetry: Arc::new(ReplicaTelemetry::default()),
        }
    }

    pub fn holder(&self) -> &OltpStoreHolder {
        &self.holder
    }

    pub fn telemetry(&self) -> &ReplicaTelemetry {
        &self.telemetry
    }

    pub fn route(&self, at: &Consistency) -> ReadRoute {
        match at.mode {
            ConsistencyMode::Strong => {
                self.telemetry.observe_primary();
                ReadRoute::Primary
            }
            ConsistencyMode::BoundedStale => {
                self.telemetry.observe_replica();
                ReadRoute::Replica
            }
        }
    }

    pub fn read(&self, scope: &TenantScope, key: &str) -> Option<ReplicaRow> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S5_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
        };
        self.lock()
            .partitions
            .get(&pk)
            .and_then(|p| p.rows.get(key).cloned())
    }

    pub fn replicate(&self, scope: &TenantScope, op: &str, row: ReplicaRow, offset: u64) {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S5_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
        };
        let mut inner = self.lock();
        let partition = inner.partitions.entry(pk).or_default();
        match op {
            "add" => {
                partition.rows.insert(row.key.clone(), row);
            }
            "remove" => {
                partition.rows.remove(&row.key);
            }
            _ => {}
        }
        if offset > partition.applied_offset {
            partition.applied_offset = offset;
        }
    }

    pub fn applied_offset(&self, scope: &TenantScope) -> u64 {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S5_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
        };
        self.lock()
            .partitions
            .get(&pk)
            .map(|p| p.applied_offset)
            .unwrap_or(0)
    }

    pub fn row_count(&self, scope: &TenantScope) -> usize {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S5_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
        };
        self.lock()
            .partitions
            .get(&pk)
            .map(|p| p.rows.len())
            .unwrap_or(0)
    }

    pub fn reject_write(&self) -> Result<std::convert::Infallible, ReplicaWriteRejected> {
        Err(ReplicaWriteRejected)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadRoute {
    Replica,
    Primary,
}

impl ReadRoute {
    pub fn is_replica(&self) -> bool {
        matches!(self, ReadRoute::Replica)
    }

    pub fn is_primary(&self) -> bool {
        matches!(self, ReadRoute::Primary)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplicaWriteRejected;

impl core::fmt::Display for ReplicaWriteRejected {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "S5 is a read-only replica (architecture §2): there is no consumer write path; the only \
             mutator is replication from the primary (AuthzReadReplica::replicate)"
        )
    }
}

impl std::error::Error for ReplicaWriteRejected {}

#[derive(Debug, Default)]
pub struct ReplicaTelemetry {
    served_from_replica: AtomicU64,
    routed_to_primary: AtomicU64,
}

impl ReplicaTelemetry {
    fn observe_replica(&self) {
        self.served_from_replica.fetch_add(1, Ordering::Relaxed);
    }

    fn observe_primary(&self) {
        self.routed_to_primary.fetch_add(1, Ordering::Relaxed);
    }

    pub fn served_from_replica(&self) -> u64 {
        self.served_from_replica.load(Ordering::Relaxed)
    }

    pub fn routed_to_primary(&self) -> u64 {
        self.routed_to_primary.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind, Zookie};
    use myelin_tenancy::{Region, TenantId};

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn strong() -> Consistency {
        Consistency {
            at_least: Zookie("zk-00000000000000000005".into()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn bounded_stale() -> Consistency {
        Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::BoundedStale,
        }
    }

    fn row(key: &str, value: &str) -> ReplicaRow {
        ReplicaRow {
            key: key.into(),
            value: value.into(),
        }
    }

    #[test]
    fn default_consistency_read_is_served_from_s5() {
        let s5 = AuthzReadReplica::new();
        let acme = scope("acme");
        s5.replicate(&acme, "add", row("p:alice", "active"), 5);

        let route = s5.route(&bounded_stale());
        assert_eq!(
            route,
            ReadRoute::Replica,
            "a default-consistency read is served from S5"
        );
        assert!(route.is_replica());
        assert_eq!(
            s5.read(&acme, "p:alice"),
            Some(row("p:alice", "active")),
            "the replicated row is served from the stale-tolerant replica"
        );
    }

    #[test]
    fn strong_read_bypasses_s5_to_the_primary() {
        let s5 = AuthzReadReplica::new();
        let acme = scope("acme");
        s5.replicate(&acme, "add", row("p:alice", "STALE"), 1);

        let route = s5.route(&strong());
        assert_eq!(
            route,
            ReadRoute::Primary,
            "a zookie-stamped read bypasses S5"
        );
        assert!(route.is_primary());
    }

    #[test]
    fn s5_is_read_only_a_write_attempt_errors() {
        let s5 = AuthzReadReplica::new();
        let r = s5.reject_write();
        assert!(
            r.is_err(),
            "a direct write to S5 is rejected (read-only replica)"
        );
        assert_eq!(r.unwrap_err(), ReplicaWriteRejected);
    }

    #[test]
    fn s5_follows_the_primary_replication_only() {
        let s5 = AuthzReadReplica::new();
        let acme = scope("acme");
        assert_eq!(s5.row_count(&acme), 0);
        assert_eq!(s5.read(&acme, "p:alice"), None, "no row before replication");

        s5.replicate(&acme, "add", row("p:alice", "active"), 5);
        assert_eq!(s5.row_count(&acme), 1);
        assert_eq!(s5.read(&acme, "p:alice"), Some(row("p:alice", "active")));

        s5.replicate(&acme, "remove", row("p:alice", "active"), 6);
        assert_eq!(
            s5.read(&acme, "p:alice"),
            None,
            "a removed grant is gone from the replica"
        );
    }

    #[test]
    fn replication_is_idempotent_and_offset_is_monotone() {
        let s5 = AuthzReadReplica::new();
        let acme = scope("acme");
        s5.replicate(&acme, "add", row("p:alice", "active"), 5);
        s5.replicate(&acme, "add", row("p:alice", "active"), 5);
        assert_eq!(s5.row_count(&acme), 1, "a re-add is idempotent (one row)");
        assert_eq!(
            s5.applied_offset(&acme),
            5,
            "the offset is at the latest applied delta"
        );

        s5.replicate(&acme, "add", row("p:bob", "active"), 7);
        assert_eq!(s5.applied_offset(&acme), 7);
        s5.replicate(&acme, "add", row("p:carol", "active"), 3);
        assert_eq!(
            s5.applied_offset(&acme),
            7,
            "an older redelivery never regresses the offset"
        );
    }

    #[test]
    fn zero_cross_tenant_replica_rows() {
        let s5 = AuthzReadReplica::new();
        let acme = scope("acme");
        let globex = scope("globex");
        s5.replicate(&acme, "add", row("p:alice", "active"), 5);

        assert_eq!(s5.row_count(&globex), 0, "0 cross-tenant replica rows");
        assert_eq!(
            s5.read(&globex, "p:alice"),
            None,
            "no cross-tenant replica read path"
        );
        assert_eq!(
            s5.applied_offset(&globex),
            0,
            "globex's offset is untouched by acme's replication"
        );
        assert_eq!(s5.row_count(&acme), 1);
    }

    #[test]
    fn s5_auto_registers_as_a_personal_data_holder() {
        let s5 = AuthzReadReplica::new();
        assert_eq!(
            s5.holder().store,
            S5_HOLDER,
            "S5 registered under its holder name"
        );
        let receipt = s5.holder().register();
        assert_eq!(receipt.store, S5_HOLDER);
    }

    #[test]
    fn route_telemetry_records_the_scaling_split() {
        let s5 = AuthzReadReplica::new();
        let _ = s5.route(&bounded_stale());
        let _ = s5.route(&bounded_stale());
        let _ = s5.route(&strong());
        let t = s5.telemetry();
        assert_eq!(
            t.served_from_replica(),
            2,
            "two default-consistency reads served from S5"
        );
        assert_eq!(
            t.routed_to_primary(),
            1,
            "one zookie read bypassed to the primary"
        );
    }

    #[test]
    fn world_scale_lag_tunable_is_the_named_default_to_beat() {
        assert_eq!(
            AuthzReadReplica::DEFAULT_MAX_REPLICATION_LAG_SECS,
            30,
            "the replication-lag staleness budget is the engineering seed (re-measured at P-ID-31)"
        );
    }
}
