//! P-ST-01 (global P-007) GATE / DRILLS — dated green artifacts.
//!
//! These are the prompt's two quantified drills, run against the failure-injection
//! harness's telemetry-assertion library (the contract-1.8 survival-signal set), exactly
//! as the harness self-test (P-S04) does. `myelin-harness` is a DEV-dependency only — it
//! never enters the storage crate's production DAG (the same posture a service's drills
//! have).
//!
//! 1. **The scoped IDOR drill (the §1.1 IDOR floor).** A read whose token-tenant ≠
//!    path-tenant resolves to the token-tenant; `path_derived_tenant_count == 0`
//!    (the `CrossTenantCount == 0` survival signal — the single most load-bearing zero in
//!    the platform, telemetry.rs).
//! 2. **The bounded-pool saturation drill (the §3.1 / §1.1 cascade bound).** Driving the
//!    bounded pool to its bound fast-fails (rejects, does not block unboundedly), and the
//!    `PoolSaturation` USE signal records the rejection.
//!
//! Both call `Assertion::expect_green()` — a red aborts the test LOUDLY with the signal +
//! predicate + observed value (EI-01 §3: loud, never swallowed; the threshold is NOT
//! weakened to pass).

use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{OltpConfig, OltpError, OltpPool, TenantQuery, TenantScope, TenantTable};
use myelin_tenancy::{Region, TenantId};

fn principal(tenant: &str) -> Principal {
    Principal {
        id: PrincipalId("p".into()),
        kind: PrincipalKind::Human,
        tenant: TenantId(tenant.into()),
    }
}

/// **DRILL 1 — the scoped IDOR drill (storage §1.1 IDOR floor).**
///
/// Drive a batch of reads whose URL path asserts a DIFFERENT (attacker) tenant than the
/// verified token. Every read must resolve to the TOKEN's tenant, and the
/// `path_derived_tenant_count` survival signal must read `== 0` (the `CrossTenantCount`
/// zero). A non-zero would mean a tenant was taken from the path — the IDOR the floor
/// forbids — and the drill would abort loudly.
#[test]
fn idor_drill_zero_path_derived_tenants() {
    let mut signals = SignalSource::new();

    // The verified token: tenant = acme. The path on every request asserts "evil-corp"
    // (the IDOR attempt). We run a batch (the 1x load unit) of such reads.
    let token = principal("acme");
    let scope = TenantScope::from_verified_token(&token, Region("eu-west".into()));
    let attacker_path_tenant = TenantId("evil-corp".into());

    let mut path_derived_tenant_count: i64 = 0;
    let mut cross_tenant_reads: i64 = 0;
    const BATCH: usize = 64;
    for _ in 0..BATCH {
        let resolved = scope.resolve(Some(&attacker_path_tenant));
        // The effective tenant is ALWAYS the token's — never the attacker's path tenant.
        assert_eq!(resolved.tenant, TenantId("acme".into()));
        if resolved.path_derived {
            path_derived_tenant_count += 1;
        }
        if resolved.tenant != token.tenant {
            cross_tenant_reads += 1;
        }
        // And the actual query carries the (tenant, region) predicate pinned to the token.
        let q = TenantQuery::for_table(scope.clone(), TenantTable::new("issue"));
        assert!(q.predicate_sql().contains("tenant = 'acme'"));
    }

    // Record the survival signals (the producer side exports these on the metrics-health
    // port at P-S13; here the drill rig records what the guard produced).
    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads);
    // path_derived_tenant_count is the IDOR-specific projection; assert it directly too.
    assert_eq!(
        path_derived_tenant_count, 0,
        "path_derived_tenant_count must be 0 — no tenant is ever taken from the path (§1.1)"
    );

    // THE green artifact: 0 cross-tenant reads. expect_green() panics loudly on red.
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-007 DRILL GREEN 2026-06-19] scoped-IDOR: batch={BATCH} reads, \
         token_tenant=acme path_tenant=evil-corp → CrossTenantCount=0, \
         path_derived_tenant_count=0 (storage §1.1 IDOR floor held)"
    );
}

/// **DRILL 2 — the bounded-pool saturation drill (storage §3.1 / §1.1 cascade bound).**
///
/// Drive the bounded pool past its bound. The next acquire must be REJECTED immediately
/// (fast-fail), never block unboundedly, and the `PoolSaturation` USE signal must record
/// the rejection (`>= 1`). A pool that blocked instead of rejecting would turn one slow
/// query into a whole-pool stall (the cascade §1.1 forbids).
#[test]
fn bounded_pool_saturation_fast_fails_and_signals() {
    let mut signals = SignalSource::new();

    let config = OltpConfig {
        max_pool_size: 4,
        statement_timeout_ms: 5_000,
        per_tenant_in_flight_cap: 2,
    };
    let pool = OltpPool::open(config).expect("valid config opens");

    // Fill the pool to its global bound with two tenants holding their cap (2 each = 4).
    let _a1 = pool.acquire(&TenantId("acme".into())).unwrap();
    let _a2 = pool.acquire(&TenantId("acme".into())).unwrap();
    let _b1 = pool.acquire(&TenantId("beta".into())).unwrap();
    let _b2 = pool.acquire(&TenantId("beta".into())).unwrap();
    assert_eq!(pool.in_flight(), 4);

    // The next acquire (a third tenant) must FAST-FAIL with PoolSaturated — not block.
    let rejected = pool.acquire(&TenantId("gamma".into()));
    assert!(
        matches!(rejected, Err(OltpError::PoolSaturated)),
        "a saturated bounded pool must reject immediately, never block (§1.1); got {rejected:?}"
    );

    // The PoolSaturation USE signal records the rejection.
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
