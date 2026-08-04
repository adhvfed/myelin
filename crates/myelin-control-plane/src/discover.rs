use std::sync::atomic::{AtomicU64, Ordering};

use myelin_substrate::{
    Answer, Clock, FailStatic, FailStaticError, Seconds, ServeError, StalenessBound, SystemClock,
};
use myelin_tenancy::{CellId, Region, TenantId};

use crate::registry::Registry;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiscoverKey {
    TenantId(TenantId),
    Slug(String),
}

impl DiscoverKey {
    pub fn kind(&self) -> &'static str {
        match self {
            DiscoverKey::TenantId(_) => "tenant_id",
            DiscoverKey::Slug(_) => "slug",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteTuple {
    pub cell_id: CellId,
    pub region: Region,
    pub cell_endpoint: String,
    pub ttl_seconds: Seconds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DiscoverySignals {
    pub discovery_cache_hit: u64,
    pub misroute_count: u64,
}

impl Registry {
    pub fn discover(&self, key: &DiscoverKey, ttl_seconds: Seconds) -> Option<RouteTuple> {
        let placement = match key {
            DiscoverKey::TenantId(tenant_id) => self.placement(tenant_id)?,
            DiscoverKey::Slug(slug) => self.placement_by_slug(slug)?,
        };
        let cell = self.cell(&placement.home_cell)?;
        Some(RouteTuple {
            cell_id: cell.cell_id.clone(),
            region: cell.region.clone(),
            cell_endpoint: cell.endpoint.clone(),
            ttl_seconds,
        })
    }
}

pub struct DiscoveryCache<C: Clock = SystemClock> {
    inner: FailStatic<DiscoverKey, RouteTuple, C>,
    discovery_cache_hit: AtomicU64,
    misroute_count: AtomicU64,
}

impl<C: Clock> std::fmt::Debug for DiscoveryCache<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryCache")
            .field(
                "discovery_cache_hit",
                &self.discovery_cache_hit.load(Ordering::SeqCst),
            )
            .field(
                "misroute_count",
                &self.misroute_count.load(Ordering::SeqCst),
            )
            .field("inner", &self.inner)
            .finish()
    }
}

impl DiscoveryCache<SystemClock> {
    pub fn try_new(
        fresh_ttl: Seconds,
        static_max: Seconds,
        bound: StalenessBound,
    ) -> Result<Self, FailStaticError> {
        Ok(DiscoveryCache {
            inner: FailStatic::try_new(fresh_ttl, static_max, bound)?,
            discovery_cache_hit: AtomicU64::new(0),
            misroute_count: AtomicU64::new(0),
        })
    }
}

impl<C: Clock> DiscoveryCache<C> {
    pub fn try_new_with_clock(
        fresh_ttl: Seconds,
        static_max: Seconds,
        bound: StalenessBound,
        clock: C,
    ) -> Result<Self, FailStaticError> {
        Ok(DiscoveryCache {
            inner: FailStatic::try_new_with_clock(fresh_ttl, static_max, bound, clock)?,
            discovery_cache_hit: AtomicU64::new(0),
            misroute_count: AtomicU64::new(0),
        })
    }

    pub fn clock(&self) -> &C {
        self.inner.clock()
    }

    pub fn resolve(
        &self,
        key: &DiscoverKey,
        discover_cp: impl Fn(&DiscoverKey) -> Result<Option<RouteTuple>, ServeError>,
    ) -> Answer<RouteTuple> {
        let reached_upstream = std::cell::Cell::new(false);
        let answer = self.inner.get(key.clone(), || match discover_cp(key) {
            Ok(Some(route)) => {
                reached_upstream.set(true);
                Ok(route)
            }
            Ok(None) => {
                reached_upstream.set(true);
                Err(ServeError(
                    "misroute: unknown tenant/slug (no route)".into(),
                ))
            }
            Err(e) => Err(e),
        });
        match &answer {
            Answer::Fresh(_) if !reached_upstream.get() => {
                self.discovery_cache_hit.fetch_add(1, Ordering::SeqCst);
            }
            Answer::Static(_) => {
                self.discovery_cache_hit.fetch_add(1, Ordering::SeqCst);
            }
            Answer::Closed if reached_upstream.get() => {
                self.misroute_count.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
        answer
    }

    pub fn signals(&self) -> DiscoverySignals {
        DiscoverySignals {
            discovery_cache_hit: self.discovery_cache_hit.load(Ordering::SeqCst),
            misroute_count: self.misroute_count.load(Ordering::SeqCst),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
    };
    use myelin_substrate::TestClock;

    fn drill_bound() -> StalenessBound {
        StalenessBound {
            revocation_sla_secs: 300,
            agent_token_ttl_secs: 60,
        }
    }

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
            endpoint: format!("cell.{region}.myelin.eu"),
        }
    }

    fn placed_registry() -> Registry {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.place_tenant(TenantPlacement {
            tenant_id: TenantId::from_token("01J0ACME"),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-1")],
        })
        .expect("the single-region placement is admitted");
        reg
    }

    #[test]
    fn discover_by_tenant_id_returns_the_routing_tuple() {
        let reg = placed_registry();
        let route = reg
            .discover(&DiscoverKey::TenantId(TenantId::from_token("01J0ACME")), 30)
            .expect("a placed tenant resolves to a route");
        assert_eq!(route.cell_id.as_str(), "cell-w-1");
        assert_eq!(route.region.as_str(), "eu-west");
        assert_eq!(route.cell_endpoint, "cell.eu-west.myelin.eu");
        assert_eq!(route.ttl_seconds, 30);
    }

    #[test]
    fn discover_by_slug_returns_the_same_route() {
        let reg = placed_registry();
        let by_slug = reg
            .discover(&DiscoverKey::Slug("acme".into()), 30)
            .expect("the non-personal slug resolves to a route");
        let by_id = reg
            .discover(&DiscoverKey::TenantId(TenantId::from_token("01J0ACME")), 30)
            .expect("the opaque id resolves to a route");
        assert_eq!(
            by_slug, by_id,
            "the slug and the tenant id resolve the SAME route"
        );
    }

    #[test]
    fn discover_unknown_key_returns_none() {
        let reg = placed_registry();
        assert!(reg
            .discover(
                &DiscoverKey::TenantId(TenantId::from_token("01J0GHOST")),
                30
            )
            .is_none());
        assert!(reg
            .discover(&DiscoverKey::Slug("ghost".into()), 30)
            .is_none());
    }

    #[test]
    fn discover_key_kind_is_pii_free() {
        assert_eq!(
            DiscoverKey::TenantId(TenantId::from_token("01J0ACME")).kind(),
            "tenant_id"
        );
        assert_eq!(DiscoverKey::Slug("acme".into()).kind(), "slug");
    }

    #[test]
    fn cache_serves_fresh_within_ttl_and_increments_cache_hit() {
        let reg = placed_registry();
        let key = DiscoverKey::TenantId(TenantId::from_token("01J0ACME"));
        let cache =
            DiscoveryCache::try_new_with_clock(30, 300, drill_bound(), TestClock::at(1_000))
                .expect("valid bound");

        let cp = |k: &DiscoverKey| Ok(reg.discover(k, 30));
        let a = cache.resolve(&key, cp);
        assert!(a.is_fresh());
        assert_eq!(
            cache.signals().discovery_cache_hit,
            0,
            "the first read is authoritative, not a hit"
        );

        clock_advance(&cache, 10);
        let unreachable = |_: &DiscoverKey| Err(ServeError("control plane unreachable".into()));
        let b = cache.resolve(&key, unreachable);
        assert!(b.is_fresh(), "within the TTL the cached route is fresh");
        assert_eq!(
            cache.signals().discovery_cache_hit,
            1,
            "a fresh-from-cache serve is a cache hit"
        );
    }

    #[test]
    fn cache_serves_fail_static_when_control_plane_unreachable() {
        let reg = placed_registry();
        let key = DiscoverKey::TenantId(TenantId::from_token("01J0ACME"));
        let cache = DiscoveryCache::try_new_with_clock(30, 300, drill_bound(), TestClock::at(0))
            .expect("valid bound");

        assert!(cache.resolve(&key, |k| Ok(reg.discover(k, 30))).is_fresh());

        clock_advance(&cache, 100);
        let unreachable = |_: &DiscoverKey| Err(ServeError("control plane hard-down".into()));
        let stale = cache.resolve(&key, unreachable);
        assert!(
            stale.is_degraded(),
            "a CP outage serves the route fail-static (degraded), not closed"
        );
        if let Answer::Static(route) = &stale {
            assert_eq!(
                route.cell_id.as_str(),
                "cell-w-1",
                "the last-known-good route is served"
            );
        }
        assert_eq!(
            cache.signals().discovery_cache_hit,
            1,
            "the fail-static serve is a cache hit"
        );

        clock_advance(&cache, 201);
        assert!(
            cache.resolve(&key, unreachable).is_closed(),
            "past the budget routing fails closed"
        );
    }

    #[test]
    fn cache_records_a_misroute_when_cp_says_unknown() {
        let reg = placed_registry();
        let key = DiscoverKey::TenantId(TenantId::from_token("01J0GHOST"));
        let cache = DiscoveryCache::try_new_with_clock(30, 300, drill_bound(), TestClock::at(0))
            .expect("valid bound");
        let answer = cache.resolve(&key, |k| Ok(reg.discover(k, 30)));
        assert!(
            answer.is_closed(),
            "an unknown tenant has no route (closed, never fabricated)"
        );
        assert_eq!(
            cache.signals().misroute_count,
            1,
            "the CP-said-unknown case is a misroute"
        );
        assert_eq!(
            cache.signals().discovery_cache_hit,
            0,
            "a misroute is not a cache hit"
        );
    }

    #[test]
    fn cold_start_during_outage_is_closed_but_not_a_misroute() {
        let key = DiscoverKey::TenantId(TenantId::from_token("01J0ACME"));
        let cache = DiscoveryCache::try_new_with_clock(30, 300, drill_bound(), TestClock::at(0))
            .expect("valid bound");
        let unreachable = |_: &DiscoverKey| Err(ServeError("cp down".into()));
        assert!(cache.resolve(&key, unreachable).is_closed());
        assert_eq!(
            cache.signals().misroute_count,
            0,
            "an unreachable CP is not a misroute"
        );
        assert_eq!(cache.signals().discovery_cache_hit, 0);
    }

    #[test]
    fn cache_rejects_a_staleness_budget_over_the_revocation_sla() {
        let err = DiscoveryCache::try_new(30, 301, drill_bound())
            .expect_err("static_max 301 > revocation SLA 300 is rejected");
        assert_eq!(
            err,
            FailStaticError::ExceedsRevocationSla {
                static_max: 301,
                revocation_sla: 300
            }
        );
    }

    fn clock_advance(cache: &DiscoveryCache<TestClock>, secs: u64) {
        cache.clock().advance(secs);
    }
}
