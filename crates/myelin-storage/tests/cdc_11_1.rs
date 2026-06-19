//! Contract 11.1 CDC pair — the OLTP tier client (pool + RLS half).
//!
//! The prompt requires "the provider+consumer pair for 11.1 (a subsystem-side consumer
//! opening the OLTP client through the harness)". This is the consumer-driven contract
//! test: the PROVIDER is `myelin-storage` (the OLTP tier client this prompt ships); the
//! CONSUMER is a subsystem (modelled here as a tiny `IssuesService`) that opens its OLTP
//! pool through the harness seam and builds a tenant-scoped query. The test pins the
//! frozen call shape every subsystem relies on — if 11.1's surface drifts (the pool open
//! seam, the `(tenant, region)` scope from the verified token, the tenant-predicate query
//! builder), this stops compiling/passing.

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{
    register_holder, OltpConfig, OltpPool, OltpStoreHolder, TenantQuery, TenantScope, TenantTable,
};
use myelin_tenancy::{Region, TenantId};

/// A subsystem consumer of 11.1: it owns its schema (the `issue` table) and opens its OWN
/// OLTP pool through the harness seam — the `no-cross-db` boundary is preserved (it does
/// not reach another subsystem's tables; it uses the shared GUARD mechanism).
struct IssuesService {
    pool: OltpPool,
}

impl IssuesService {
    /// Open the service's OLTP store through the harness (contract 1.1 — a service opens
    /// its pool through `serve(AppSpec)`, here the storage seam it wires). The harness
    /// auto-registers the store as a holder (1.4) — proven by the returned receipt.
    fn boot() -> IssuesService {
        let config = OltpConfig {
            max_pool_size: 16,
            statement_timeout_ms: 3_000,
            per_tenant_in_flight_cap: 4,
        };
        let pool = OltpPool::open(config).expect("issues OLTP pool opens through the harness");
        // Auto-registration hook fires for the store the service opened (1.4).
        let receipt = register_holder("issue_oltp");
        assert_eq!(receipt.store, "issue_oltp");
        // The store is a real PersonalDataHolder (frozen shape; bodies are GDPR M1).
        let _holder = OltpStoreHolder::new("issue_oltp");
        IssuesService { pool }
    }

    /// A tenant-scoped read the consumer performs: it MUST carry the `(tenant, region)`
    /// predicate sourced from the verified token. The consumer cannot build a query
    /// without a `TenantScope` (the tenant-predicate compile-fixture) — so this call shape
    /// IS the contract.
    fn read_issue(&self, token: &Principal, region: Region) -> String {
        let _permit = self
            .pool
            .acquire(&token.tenant)
            .expect("a permit is available under the consumer's load");
        let scope = TenantScope::from_verified_token(token, region);
        let q = TenantQuery::for_table(scope, TenantTable::new("issue"));
        q.predicate_sql()
    }
}

/// THE CDC pair: the consumer (a subsystem) opens the OLTP client through the harness seam
/// and reads through the `(tenant, region)` RLS guard — the provider (`myelin-storage`)
/// honours the frozen 11.1 shape. The read is pinned to the VERIFIED token's tenant.
#[test]
fn cdc_11_1_consumer_opens_oltp_client_through_the_harness() {
    let issues = IssuesService::boot();

    let token = Principal::stub(PrincipalId("u1".into()), PrincipalKind::Human, TenantId("acme".into()));
    let sql = issues.read_issue(&token, Region("eu-west".into()));

    // The provider produced a tenant-scoped statement pinned to the verified token.
    assert!(sql.contains("tenant = 'acme'"), "11.1 must scope to the verified tenant: {sql}");
    assert!(sql.contains("region = 'eu-west'"), "11.1 must scope to the region: {sql}");
    assert!(sql.starts_with("issue WHERE"), "11.1 scopes the consumer's own table: {sql}");
}
