use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use myelin_substrate::{Answer, Clock, MonotonicClock, ServeError, StalenessBound, SystemClock};
use myelin_tenancy::TenantId;

use crate::discover::{DiscoverKey, DiscoveryCache, RouteTuple};
use crate::place::{PlaceError, PlacementAnswer, PlacementService, TokenMinter};
use crate::placement_of::{CellGateway, GatewayReject, PlacementOf};
use crate::registry::Registry;
use crate::schema::IsolationKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegradeScope {
    None,
    SignupAndProvisioningOnly,
    DataPlaneCascaded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignupDegraded {
    pub reason: String,
}

impl core::fmt::Display for SignupDegraded {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "signup DEGRADED: {} - a new tenant cannot be placed while the control plane is down \
             (region-first placement writes the sticky tenant_placement row IN the control plane, \
             P-CP-07). Already-placed tenants are UNAFFECTED (they keep serving within their cells); \
             retry signup once the control plane is back. Degrade, not cascade (VISION §3).",
            self.reason
        )
    }
}

impl std::error::Error for SignupDegraded {}

#[derive(Clone, Default)]
pub struct ControlPlane {
    down: Arc<AtomicBool>,
}

impl ControlPlane {
    pub fn up() -> ControlPlane {
        ControlPlane {
            down: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn hard_down(&self) {
        self.down.store(true, Ordering::SeqCst);
    }

    pub fn restore(&self) {
        self.down.store(false, Ordering::SeqCst);
    }

    pub fn is_down(&self) -> bool {
        self.down.load(Ordering::SeqCst)
    }

    pub fn discover(
        &self,
        registry: &Registry,
        key: &DiscoverKey,
        ttl_seconds: myelin_substrate::Seconds,
    ) -> Result<Option<RouteTuple>, ServeError> {
        if self.is_down() {
            return Err(ServeError(
                "control plane hard-down (CP-D4 outage) - unreachable".into(),
            ));
        }
        Ok(registry.discover(key, ttl_seconds))
    }
}

impl core::fmt::Debug for ControlPlane {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ControlPlane")
            .field("down", &self.is_down())
            .finish()
    }
}

pub struct DataPlane<C: Clock = MonotonicClock> {
    gateway: CellGateway,
    cache: DiscoveryCache<C>,
    ttl_seconds: myelin_substrate::Seconds,
    placed_requests_served: Arc<AtomicU64>,
    placed_requests_failed: Arc<AtomicU64>,
}

#[derive(Clone, Debug)]
pub struct Served {
    pub placement: PlacementOf,
    pub via_fail_static: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServeFailure {
    Gateway(GatewayReject),
    NoRoute { tenant_id: TenantId },
}

impl core::fmt::Display for ServeFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ServeFailure::Gateway(r) => write!(f, "{r}"),
            ServeFailure::NoRoute { tenant_id } => write!(
                f,
                "no-route: tenant `{}` has no cached route AND the control plane is unreachable \
                 (cold-start-during-outage) - correctly fail-closed (never a fabricated route).",
                tenant_id.as_str()
            ),
        }
    }
}

impl std::error::Error for ServeFailure {}

impl<C: Clock> DataPlane<C> {
    pub fn new(
        gateway: CellGateway,
        cache: DiscoveryCache<C>,
        ttl_seconds: myelin_substrate::Seconds,
    ) -> DataPlane<C> {
        DataPlane {
            gateway,
            cache,
            ttl_seconds,
            placed_requests_served: Arc::new(AtomicU64::new(0)),
            placed_requests_failed: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn cache(&self) -> &DiscoveryCache<C> {
        &self.cache
    }

    pub fn serve(
        &self,
        control_plane: &ControlPlane,
        registry: &Registry,
        tenant: &TenantId,
    ) -> Result<Served, ServeFailure> {
        let key = DiscoverKey::TenantId(tenant.clone());

        let answer = self.cache.resolve(&key, |k| {
            control_plane.discover(registry, k, self.ttl_seconds)
        });
        let via_fail_static = matches!(answer, Answer::Static(_));
        if matches!(answer, Answer::Closed) {
            self.placed_requests_failed.fetch_add(1, Ordering::SeqCst);
            return Err(ServeFailure::NoRoute {
                tenant_id: tenant.clone(),
            });
        }

        match self.gateway.route(registry, tenant) {
            Ok(placement) => {
                self.placed_requests_served.fetch_add(1, Ordering::SeqCst);
                Ok(Served {
                    placement,
                    via_fail_static,
                })
            }
            Err(reject) => {
                self.placed_requests_failed.fetch_add(1, Ordering::SeqCst);
                Err(ServeFailure::Gateway(reject))
            }
        }
    }

    pub fn placed_requests_served(&self) -> u64 {
        self.placed_requests_served.load(Ordering::SeqCst)
    }

    pub fn placed_requests_failed(&self) -> u64 {
        self.placed_requests_failed.load(Ordering::SeqCst)
    }
}

impl<C: Clock> core::fmt::Debug for DataPlane<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DataPlane")
            .field("cell_id", &self.gateway.cell_id().as_str())
            .field("placed_requests_served", &self.placed_requests_served())
            .field("placed_requests_failed", &self.placed_requests_failed())
            .finish()
    }
}

pub struct SignupPlane<M: TokenMinter, C: Clock = SystemClock> {
    service: PlacementService<M, C>,
    signups_degraded: Arc<AtomicU64>,
}

impl<M: TokenMinter, C: Clock> SignupPlane<M, C> {
    pub fn new(service: PlacementService<M, C>) -> SignupPlane<M, C> {
        SignupPlane {
            service,
            signups_degraded: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn signup(
        &self,
        control_plane: &ControlPlane,
        registry: &mut Registry,
        region: &myelin_tenancy::Region,
        requested_tier: IsolationKind,
        slug: &str,
    ) -> Result<PlacementAnswer, SignupDegraded> {
        if control_plane.is_down() {
            self.signups_degraded.fetch_add(1, Ordering::SeqCst);
            return Err(SignupDegraded {
                reason: "control plane hard-down (CP-D4 outage) - unreachable".into(),
            });
        }
        match self.service.place(registry, region, requested_tier, slug) {
            Ok(answer) => Ok(answer),
            Err(PlaceError::NoEligibleCell {
                region,
                requested_tier,
            }) => {
                self.signups_degraded.fetch_add(1, Ordering::SeqCst);
                Err(SignupDegraded {
                    reason: format!(
                        "no eligible cell in region `{}` for tier `{requested_tier:?}` (capacity \
                         degrade - provisioning needed)",
                        region.as_str()
                    ),
                })
            }
            Err(PlaceError::Invariant(e)) => {
                self.signups_degraded.fetch_add(1, Ordering::SeqCst);
                Err(SignupDegraded {
                    reason: format!("placement invariant rejected the write: {e}"),
                })
            }
        }
    }

    pub fn signups_degraded(&self) -> u64 {
        self.signups_degraded.load(Ordering::SeqCst)
    }

    pub fn service(&self) -> &PlacementService<M, C> {
        &self.service
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpOutageReport {
    pub placed_requests_served: u64,
    pub placed_requests_failed: u64,
    pub signups_degraded: u64,
    pub serving_uptime_pct: u8,
    pub degrade_scope: DegradeScope,
}

impl CpOutageReport {
    pub fn compute(
        placed_requests_served: u64,
        placed_requests_failed: u64,
        signups_degraded: u64,
    ) -> CpOutageReport {
        let total = placed_requests_served + placed_requests_failed;
        let serving_uptime_pct = (placed_requests_served * 100)
            .checked_div(total)
            .unwrap_or(100) as u8;
        let degrade_scope = if placed_requests_failed > 0 {
            DegradeScope::DataPlaneCascaded
        } else if signups_degraded > 0 {
            DegradeScope::SignupAndProvisioningOnly
        } else {
            DegradeScope::None
        };
        CpOutageReport {
            placed_requests_served,
            placed_requests_failed,
            signups_degraded,
            serving_uptime_pct,
            degrade_scope,
        }
    }

    pub fn is_cp_d4_win(&self) -> bool {
        self.placed_requests_failed == 0
            && self.serving_uptime_pct == 100
            && self.degrade_scope == DegradeScope::SignupAndProvisioningOnly
    }
}

pub fn cp_outage_bound() -> StalenessBound {
    StalenessBound {
        revocation_sla_secs: 300,
        agent_token_ttl_secs: 60,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::CounterMinter;
    use crate::schema::{
        Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
    };
    use myelin_substrate::TestClock;
    use myelin_tenancy::{CellId, Region};

    fn cell(id: &str, region: &str) -> Cell {
        Cell {
            cell_id: CellId::from_token(id),
            region: Region::new(region),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 10,
            version: 1,
            endpoint: format!("cell.{region}.{id}.myelin.eu"),
        }
    }

    fn placed_registry() -> (Registry, TenantId) {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        let tenant = TenantId::from_token("01J0ACME");
        reg.place_tenant(TenantPlacement {
            tenant_id: tenant.clone(),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-1")],
        })
        .expect("the single-region placement is admitted");
        (reg, tenant)
    }

    fn data_plane(clock: TestClock) -> DataPlane<TestClock> {
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let cache = DiscoveryCache::try_new_with_clock(30, 300, cp_outage_bound(), clock)
            .expect("valid bound");
        DataPlane::new(gw, cache, 30)
    }

    #[test]
    fn cp_hard_down_keeps_placed_tenant_serving_signup_only_degrades() {
        let (mut reg, tenant) = placed_registry();
        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(0));
        let signup = SignupPlane::new(PlacementService::new(CounterMinter::new()));

        let s0 = dp
            .serve(&cp, &reg, &tenant)
            .expect("CP up: the placed tenant serves");
        assert!(
            !s0.via_fail_static,
            "with the CP up the route is fresh, not fail-static"
        );
        assert_eq!(s0.placement.home_cell.as_str(), "cell-w-1");

        cp.hard_down();
        assert!(cp.is_down());

        dp.cache().clock().advance(100);
        let s1 = dp
            .serve(&cp, &reg, &tenant)
            .expect("CP down: the placed tenant KEEPS SERVING");
        assert!(
            s1.via_fail_static,
            "with the CP hard-down the route is served fail-static"
        );
        assert_eq!(
            s1.placement.home_cell.as_str(),
            "cell-w-1",
            "served within its cell"
        );

        let degraded = signup
            .signup(
                &cp,
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "newco",
            )
            .expect_err("CP down: signup DEGRADES");
        assert!(
            degraded.to_string().contains("control plane hard-down"),
            "loud: {degraded}"
        );
        assert_eq!(signup.signups_degraded(), 1, "exactly one signup degraded");
        assert_eq!(
            signup.service().signals().placement_count,
            0,
            "nothing was placed while CP down"
        );

        assert_eq!(
            dp.placed_requests_served(),
            2,
            "both placed-tenant requests served"
        );
        assert_eq!(
            dp.placed_requests_failed(),
            0,
            "0 placed-tenant requests failed (the CP-D4 zero)"
        );

        cp.restore();
        let placed = signup
            .signup(
                &cp,
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "newco",
            )
            .expect("CP restored: signup works again");
        assert!(
            placed.tenant_id.as_str().starts_with("01J0CP-"),
            "a new tenant is placed PII-free"
        );

        let report = CpOutageReport::compute(
            dp.placed_requests_served(),
            dp.placed_requests_failed(),
            signup.signups_degraded(),
        );
        assert!(report.is_cp_d4_win(), "the CP-D4 win: {report:?}");
        assert_eq!(report.serving_uptime_pct, 100);
        assert_eq!(
            report.degrade_scope,
            DegradeScope::SignupAndProvisioningOnly
        );
    }

    #[test]
    fn signups_degraded_counts_each_degrade() {
        let (mut reg, _tenant) = placed_registry();
        let cp = ControlPlane::up();
        let signup = SignupPlane::new(PlacementService::new(CounterMinter::new()));

        cp.hard_down();
        for _ in 0..2 {
            assert!(
                signup
                    .signup(
                        &cp,
                        &mut reg,
                        &Region::new("eu-west"),
                        IsolationKind::Pool,
                        "newco"
                    )
                    .is_err(),
                "CP down: signup degrades"
            );
        }
        assert_eq!(
            signup.signups_degraded(),
            2,
            "each degrade is counted (not a constant)"
        );
    }

    #[test]
    fn cached_route_serves_without_touching_the_control_plane() {
        let (reg, tenant) = placed_registry();
        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(1_000));

        dp.serve(&cp, &reg, &tenant).expect("primes the cache");
        cp.hard_down();
        dp.cache().clock().advance(10);
        let s = dp
            .serve(&cp, &reg, &tenant)
            .expect("a fresh cached route serves without the CP");
        assert!(
            !s.via_fail_static,
            "within the TTL the cached route is fresh (the CP was not needed)"
        );
        assert!(
            dp.cache().signals().discovery_cache_hit >= 1,
            "served from the cache"
        );
    }

    #[test]
    fn past_staleness_budget_fails_closed_and_reads_red() {
        let (reg, tenant) = placed_registry();
        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(0));

        dp.serve(&cp, &reg, &tenant).expect("primes the cache");
        cp.hard_down();
        dp.cache().clock().advance(301);
        let fail = dp
            .serve(&cp, &reg, &tenant)
            .expect_err("past the budget routing fails closed");
        assert!(
            matches!(fail, ServeFailure::NoRoute { .. }),
            "no route, correctly fail-closed: {fail}"
        );
        assert_eq!(
            dp.placed_requests_failed(),
            1,
            "the past-budget request failed"
        );

        let report =
            CpOutageReport::compute(dp.placed_requests_served(), dp.placed_requests_failed(), 0);
        assert!(
            !report.is_cp_d4_win(),
            "a failed placed request is NOT the CP-D4 win"
        );
        assert_eq!(report.degrade_scope, DegradeScope::DataPlaneCascaded);
        assert!(
            report.serving_uptime_pct < 100,
            "uptime dropped below 100: {report:?}"
        );
    }

    #[test]
    fn misroute_during_outage_is_a_gateway_reject_not_a_cascade() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        let beta = TenantId::from_token("01J0BETA");
        reg.place_tenant(TenantPlacement {
            tenant_id: beta.clone(),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-2"),
            isolation_tier: IsolationKind::Pool,
            slug: "beta".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-2")],
        })
        .expect("placed");

        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(0));
        let fail = dp
            .serve(&cp, &reg, &beta)
            .expect_err("cell-w-1 does not home BETA → a gateway reject (layer 4), not a serve");
        let ServeFailure::Gateway(GatewayReject::Misroute(_)) = fail else {
            panic!("expected a misroute gateway reject, got {fail}");
        };
    }

    #[test]
    fn report_compute_is_exact() {
        let win = CpOutageReport::compute(10, 0, 3);
        assert_eq!(win.serving_uptime_pct, 100);
        assert_eq!(win.degrade_scope, DegradeScope::SignupAndProvisioningOnly);
        assert!(win.is_cp_d4_win());

        let red = CpOutageReport::compute(9, 1, 3);
        assert_eq!(red.serving_uptime_pct, 90);
        assert_eq!(red.degrade_scope, DegradeScope::DataPlaneCascaded);
        assert!(!red.is_cp_d4_win());

        let none = CpOutageReport::compute(10, 0, 0);
        assert_eq!(none.degrade_scope, DegradeScope::None);
        assert!(
            !none.is_cp_d4_win(),
            "no outage exercised is not a CP-D4 win"
        );
    }

    #[test]
    fn cdc_fail_static_discovery_degrade_provider_consumer() {
        struct GatewayDuringOutage<'a> {
            data_plane: &'a DataPlane<TestClock>,
            control_plane: &'a ControlPlane,
            registry: &'a Registry,
        }
        impl GatewayDuringOutage<'_> {
            fn serve_during_outage(
                &self,
                tenant: &TenantId,
            ) -> Result<(bool, String), ServeFailure> {
                let served = self
                    .data_plane
                    .serve(self.control_plane, self.registry, tenant)?;
                Ok((
                    served.via_fail_static,
                    served.placement.home_cell.as_str().to_string(),
                ))
            }
        }

        let (reg, tenant) = placed_registry();
        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(0));
        dp.serve(&cp, &reg, &tenant)
            .expect("primes the cache (CP up)");
        cp.hard_down();
        dp.cache().clock().advance(100);

        let consumer = GatewayDuringOutage {
            data_plane: &dp,
            control_plane: &cp,
            registry: &reg,
        };
        let (via_fail_static, home_cell) = consumer
            .serve_during_outage(&tenant)
            .expect("the placed tenant serves while the CP is down");
        assert!(
            via_fail_static,
            "the route is served fail-static while the CP is down"
        );
        assert_eq!(home_cell, "cell-w-1", "served entirely within its cell");

        dp.cache().clock().advance(300);
        let cold = consumer.serve_during_outage(&tenant);
        assert!(
            matches!(cold, Err(ServeFailure::NoRoute { .. })),
            "past-budget serve fails closed: {cold:?}"
        );
    }

    #[test]
    fn data_plane_debug_is_pii_free() {
        let (reg, tenant) = placed_registry();
        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(0));
        dp.serve(&cp, &reg, &tenant).expect("served");
        let dbg = format!("{dp:?}");
        assert!(dbg.contains("cell-w-1"), "shows the cell id: {dbg}");
        assert!(
            dbg.contains("placed_requests_served"),
            "shows the aggregate: {dbg}"
        );
        assert!(!dbg.contains("01J0ACME"), "leaks no tenant id: {dbg}");
    }
}
