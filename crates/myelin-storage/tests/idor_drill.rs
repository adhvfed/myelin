use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{OltpConfig, OltpError, OltpPool, TenantQuery, TenantScope, TenantTable};
use myelin_tenancy::{Region, TenantId};

fn principal(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

#[test]
fn idor_drill_zero_path_derived_tenants() {
    let mut signals = SignalSource::new();

    let token = principal("acme");
    let scope = TenantScope::from_verified_token(&token, Region("eu-west".into()));
    let attacker_path_tenant = TenantId("evil-corp".into());

    let mut path_derived_tenant_count: i64 = 0;
    let mut cross_tenant_reads: i64 = 0;
    const BATCH: usize = 64;
    for _ in 0..BATCH {
        let resolved = scope.resolve(Some(&attacker_path_tenant));
        assert_eq!(resolved.tenant, TenantId("acme".into()));
        if resolved.path_derived {
            path_derived_tenant_count += 1;
        }
        if resolved.tenant != token.tenant {
            cross_tenant_reads += 1;
        }
        let q = TenantQuery::for_table(scope.clone(), TenantTable::new("issue"));
        assert!(q.predicate_sql().contains("tenant = $1 AND region = $2"));
        assert_eq!(
            q.predicate_binds().first().map(String::as_str),
            Some("acme")
        );
    }

    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads);
    assert_eq!(
        path_derived_tenant_count, 0,
        "path_derived_tenant_count must be 0 - no tenant is ever taken from the path (§1.1)"
    );

    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-007 DRILL GREEN 2026-06-19] scoped-IDOR: batch={BATCH} reads, \
         token_tenant=acme path_tenant=evil-corp → CrossTenantCount=0, \
         path_derived_tenant_count=0 (storage §1.1 IDOR floor held)"
    );
}

#[test]
fn bounded_pool_saturation_fast_fails_and_signals() {
    let mut signals = SignalSource::new();

    let config = OltpConfig {
        max_pool_size: 4,
        statement_timeout_ms: 5_000,
        per_tenant_in_flight_cap: 2,
    };
    let pool = OltpPool::open(config).expect("valid config opens");

    let _a1 = pool.acquire(&TenantId("acme".into())).unwrap();
    let _a2 = pool.acquire(&TenantId("acme".into())).unwrap();
    let _b1 = pool.acquire(&TenantId("beta".into())).unwrap();
    let _b2 = pool.acquire(&TenantId("beta".into())).unwrap();
    assert_eq!(pool.in_flight(), 4);

    let rejected = pool.acquire(&TenantId("gamma".into()));
    assert!(
        matches!(rejected, Err(OltpError::PoolSaturated)),
        "a saturated bounded pool must reject immediately, never block (§1.1); got {rejected:?}"
    );

    signals.set_labelled(
        SignalName::PoolSaturation,
        vec![myelin_harness::telemetry::Label::new("pool", "oltp")],
        pool.saturation_rejections() as i64,
    );
    signals
        .assert_labelled(
            SignalName::PoolSaturation,
            vec![myelin_harness::telemetry::Label::new("pool", "oltp")],
            Predicate::Gte(1),
        )
        .expect_green();

    println!(
        "[P-007 DRILL GREEN 2026-06-19] bounded-pool: max_pool_size=4, filled=4, \
         next acquire → PoolSaturated (fast-fail, not blocked), \
         PoolSaturation{{pool=oltp}} rejections={} (>= 1)",
        pool.saturation_rejections()
    );
}
