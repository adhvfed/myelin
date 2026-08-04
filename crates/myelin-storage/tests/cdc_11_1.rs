use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{
    register_holder, OltpConfig, OltpPool, OltpStoreHolder, TenantQuery, TenantScope, TenantTable,
};
use myelin_tenancy::{Region, TenantId};

struct IssuesService {
    pool: OltpPool,
}

impl IssuesService {
    fn boot() -> IssuesService {
        let config = OltpConfig {
            max_pool_size: 16,
            statement_timeout_ms: 3_000,
            per_tenant_in_flight_cap: 4,
        };
        let pool = OltpPool::open(config).expect("issues OLTP pool opens through the harness");
        let receipt = register_holder("issue_oltp");
        assert_eq!(receipt.store, "issue_oltp");
        let _holder = OltpStoreHolder::new("issue_oltp");
        IssuesService { pool }
    }

    fn read_issue(&self, token: &Principal, region: Region) -> String {
        let _permit = self
            .pool
            .acquire(&token.tenant)
            .expect("a permit is available under the consumer's load");
        let scope = TenantScope::from_verified_token(token, region);
        let q = TenantQuery::for_table(scope, TenantTable::new("issue"));
        format!("{} -- binds={:?}", q.predicate_sql(), q.predicate_binds())
    }
}

#[test]
fn cdc_11_1_consumer_opens_oltp_client_through_the_harness() {
    let issues = IssuesService::boot();

    let token = Principal::stub(
        PrincipalId("u1".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    let sql = issues.read_issue(&token, Region("eu-west".into()));

    assert!(
        sql.contains("tenant = $1 AND region = $2"),
        "11.1 must scope to the verified (tenant, region) via bind placeholders: {sql}"
    );
    assert!(
        sql.contains("binds=[\"acme\", \"eu-west\"]"),
        "11.1 must carry the verified token's (tenant, region) as out-of-band binds: {sql}"
    );
    assert!(
        sql.starts_with("issue WHERE"),
        "11.1 scopes the consumer's own table: {sql}"
    );
}
