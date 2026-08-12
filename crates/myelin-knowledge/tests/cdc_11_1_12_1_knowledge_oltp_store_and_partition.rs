use std::sync::Arc;

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::{knowledge_scope, KnowledgeStore, KnowledgeTable};
use myelin_storage::{FsBlobStore, OltpConfig, TenantScope};
use myelin_tenancy::{Region, TenantId};

fn principal(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn cfg() -> OltpConfig {
    OltpConfig {
        max_pool_size: 16,
        statement_timeout_ms: 3_000,
        per_tenant_in_flight_cap: 4,
    }
}

#[test]
fn consumer_knowledge_store_opens_its_own_bounded_oltp_pool() {
    let store = KnowledgeStore::open(cfg(), Arc::new(FsBlobStore::new()))
        .expect("the Knowledge store opens its OLTP pool");
    assert_eq!(store.pool().config(), cfg());
    let acme = TenantId("acme".into());
    let permit = store.pool().acquire(&acme).expect("acquire a permit");
    assert_eq!(store.pool().in_flight(), 1);
    drop(permit);
    assert_eq!(
        store.pool().in_flight(),
        0,
        "dropping the permit releases it"
    );
}

#[test]
fn consumer_every_knowledge_query_is_tenant_region_scoped() {
    let store = KnowledgeStore::open(cfg(), Arc::new(FsBlobStore::new())).expect("open");
    let scope = knowledge_scope(&principal("acme"), Region::new("fr-par"));
    for table in [
        KnowledgeTable::Block,
        KnowledgeTable::DbRow,
        KnowledgeTable::DocOp,
    ] {
        let q = store.query(scope.clone(), table);
        let sql = q.predicate_sql();
        assert!(
            sql.contains("tenant = $1 AND region = $2"),
            "{}: {sql}",
            table.name()
        );
        assert_eq!(
            q.predicate_binds(),
            vec!["acme".to_string(), "fr-par".to_string()],
            "{}",
            table.name()
        );
        assert_eq!(q.validate(), Ok(()));
    }
}

#[test]
fn provider_scope_is_minted_from_the_verified_token_only() {
    let scope: TenantScope = knowledge_scope(&principal("acme"), Region::new("fr-par"));
    assert_eq!(scope.tenant(), &TenantId("acme".into()));
    assert_eq!(scope.region(), &Region::new("fr-par"));
}

#[test]
fn drill_kn_d13_cross_tenant_path_spoof_is_rejected_zero_cross_tenant() {
    let scope = knowledge_scope(&principal("acme"), Region::new("fr-par"));

    let spoofed_path_tenant = TenantId("evil-corp".into());
    let resolved = KnowledgeStore::resolve_tenant(&scope, Some(&spoofed_path_tenant));

    assert_eq!(
        resolved.tenant,
        TenantId("acme".into()),
        "KN-D13: the effective tenant is the verified token's, never the spoofed path tenant"
    );
    assert!(
        !resolved.path_derived,
        "KN-D13: path_derived_tenant_count == 0 (the cross-tenant survival signal)"
    );
    assert!(
        resolved.attempted_path_mismatch,
        "KN-D13: the cross-tenant spoof attempt is flagged (the read stayed in the token tenant)"
    );

    let store = KnowledgeStore::open(cfg(), Arc::new(FsBlobStore::new())).expect("open");
    let q = store.query(scope, KnowledgeTable::Page);
    assert!(
        q.predicate_sql().contains("tenant = $1 AND region = $2")
            && q.predicate_binds() == vec!["acme".to_string(), "fr-par".to_string()]
            && !q.predicate_binds().iter().any(|b| b.contains("evil-corp")),
        "KN-D13: the resolved page query is pinned to the token tenant, never the spoofed path"
    );
}
