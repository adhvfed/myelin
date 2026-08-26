use std::sync::atomic::{AtomicU64, Ordering};

use myelin_substrate::{Clock, SystemClock};
use myelin_tenancy::{CellId, Region, TenantId};

use crate::registry::{PlacementError, Registry};
use crate::schema::{CellStatus, IsolationKind, PlacementStatus, TenantPlacement};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementAnswer {
    pub tenant_id: TenantId,
    pub home_cell: CellId,
    pub isolation_tier: IsolationKind,
    pub cell_endpoint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaceError {
    NoEligibleCell {
        region: Region,
        requested_tier: IsolationKind,
    },
    Invariant(PlacementError),
}

impl std::fmt::Display for PlaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaceError::NoEligibleCell {
                region,
                requested_tier,
            } => write!(
                f,
                "place REFUSED: no Active cell in region `{}` serves isolation tier `{:?}` with \
                 headroom (region-first → tier-second → capacity-third found no eligible cell). \
                 Signup degrades; it NEVER places into a different region (the region pin holds).",
                region.as_str(),
                requested_tier
            ),
            PlaceError::Invariant(e) => write!(f, "place REFUSED by the placement invariant: {e}"),
        }
    }
}

impl std::error::Error for PlaceError {}

impl From<PlacementError> for PlaceError {
    fn from(e: PlacementError) -> Self {
        PlaceError::Invariant(e)
    }
}

pub trait TokenMinter {
    fn mint(&self) -> TenantId;
}

#[derive(Debug, Default)]
pub struct CounterMinter {
    next: AtomicU64,
}

impl CounterMinter {
    pub fn new() -> CounterMinter {
        CounterMinter::default()
    }
}

impl TokenMinter for CounterMinter {
    fn mint(&self) -> TenantId {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        TenantId::from_token(format!("01J0CP-{n:020}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PlacementSignals {
    pub placement_count: u64,
    pub provision_latency_secs: u64,
}

pub struct PlacementService<M: TokenMinter = CounterMinter, C: Clock = SystemClock> {
    minter: M,
    clock: C,
    placement_count: AtomicU64,
    provision_latency_secs: AtomicU64,
}

impl<M: TokenMinter> PlacementService<M, SystemClock> {
    pub fn new(minter: M) -> PlacementService<M, SystemClock> {
        PlacementService {
            minter,
            clock: SystemClock,
            placement_count: AtomicU64::new(0),
            provision_latency_secs: AtomicU64::new(0),
        }
    }
}

impl<M: TokenMinter, C: Clock> PlacementService<M, C> {
    pub fn with_clock(minter: M, clock: C) -> PlacementService<M, C> {
        PlacementService {
            minter,
            clock,
            placement_count: AtomicU64::new(0),
            provision_latency_secs: AtomicU64::new(0),
        }
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    pub fn place(
        &self,
        registry: &mut Registry,
        region: &Region,
        requested_tier: IsolationKind,
        slug: &str,
    ) -> Result<PlacementAnswer, PlaceError> {
        let started = self.clock.now_secs();

        let home_cell = registry
            .assign_cell(region, requested_tier)
            .ok_or_else(|| PlaceError::NoEligibleCell {
                region: region.clone(),
                requested_tier,
            })?;
        let cell_endpoint = registry
            .cell(&home_cell)
            .expect("assign_cell returned a registered cell")
            .endpoint
            .clone();

        let tenant_id = self.minter.mint();
        registry.place_tenant(TenantPlacement {
            tenant_id: tenant_id.clone(),
            region: region.clone(),
            home_cell: home_cell.clone(),
            isolation_tier: requested_tier,
            slug: slug.to_string(),
            status: PlacementStatus::Pending,
            member_cells: vec![home_cell.clone()],
        })?;

        let elapsed = self.clock.now_secs().saturating_sub(started);
        self.provision_latency_secs
            .fetch_add(elapsed, Ordering::SeqCst);
        self.placement_count.fetch_add(1, Ordering::SeqCst);

        Ok(PlacementAnswer {
            tenant_id,
            home_cell,
            isolation_tier: requested_tier,
            cell_endpoint,
        })
    }

    pub fn answer_for(&self, registry: &Registry, tenant_id: &TenantId) -> Option<PlacementAnswer> {
        let row = registry.placement(tenant_id)?;
        let cell_endpoint = registry.cell(&row.home_cell)?.endpoint.clone();
        Some(PlacementAnswer {
            tenant_id: tenant_id.clone(),
            home_cell: row.home_cell.clone(),
            isolation_tier: row.isolation_tier,
            cell_endpoint,
        })
    }

    pub fn signals(&self) -> PlacementSignals {
        PlacementSignals {
            placement_count: self.placement_count.load(Ordering::SeqCst),
            provision_latency_secs: self.provision_latency_secs.load(Ordering::SeqCst),
        }
    }
}

impl<M: TokenMinter, C: Clock> std::fmt::Debug for PlacementService<M, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlacementService")
            .field(
                "placement_count",
                &self.placement_count.load(Ordering::SeqCst),
            )
            .field(
                "provision_latency_secs",
                &self.provision_latency_secs.load(Ordering::SeqCst),
            )
            .finish()
    }
}

impl Registry {
    pub fn assign_cell(&self, region: &Region, requested_tier: IsolationKind) -> Option<CellId> {
        self.cells_in_assignment_order(region, requested_tier)
            .into_iter()
            .next()
            .map(|c| c.cell_id.clone())
    }

    pub fn cells_in_assignment_order(
        &self,
        region: &Region,
        requested_tier: IsolationKind,
    ) -> Vec<crate::schema::Cell> {
        let mut eligible: Vec<crate::schema::Cell> = self
            .cells_iter()
            .filter(|c| &c.region == region)
            .filter(|c| c.status == CellStatus::Active)
            .filter(|c| Self::serves_tier(c.isolation_kind, requested_tier))
            .filter(|c| c.utilisation < 100)
            .collect();
        eligible.sort_by(|a, b| {
            a.utilisation
                .cmp(&b.utilisation)
                .then_with(|| a.cell_id.as_str().cmp(b.cell_id.as_str()))
        });
        eligible
    }

    pub fn serves_tier(cell_kind: IsolationKind, requested_tier: IsolationKind) -> bool {
        cell_kind == requested_tier
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Capacity, Cell};

    fn cell(id: &str, region: &str, kind: IsolationKind, util: u8, status: CellStatus) -> Cell {
        Cell {
            cell_id: CellId::from_token(id),
            region: Region::new(region),
            status,
            isolation_kind: kind,
            capacity: Capacity {
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: util,
            version: 1,
            endpoint: format!("cell.{region}.{id}.myelin.eu"),
        }
    }

    fn registry_with(cells: impl IntoIterator<Item = Cell>) -> Registry {
        let mut reg = Registry::new();
        for c in cells {
            reg.insert_cell(c);
        }
        reg
    }

    #[test]
    fn place_mints_pii_free_and_writes_a_sticky_placement() {
        let mut reg = registry_with([cell(
            "cell-w-1",
            "eu-west",
            IsolationKind::Pool,
            5,
            CellStatus::Active,
        )]);
        let svc = PlacementService::new(CounterMinter::new());

        let answer = svc
            .place(
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "acme",
            )
            .expect("a placeable region+tier mints + places");

        assert!(
            answer.tenant_id.as_str().starts_with("01J0CP-"),
            "opaque minted id"
        );
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        assert_eq!(answer.isolation_tier, IsolationKind::Pool);
        assert_eq!(answer.cell_endpoint, "cell.eu-west.cell-w-1.myelin.eu");

        let row = reg
            .placement(&answer.tenant_id)
            .expect("the placement is stored");
        assert_eq!(row.region.as_str(), "eu-west");
        assert_eq!(row.home_cell.as_str(), "cell-w-1");
        assert_eq!(
            row.status,
            PlacementStatus::Pending,
            "phase 2 (in-cell identity) not yet done"
        );

        assert_eq!(
            svc.signals().placement_count,
            1,
            "placement_count increments on a placement"
        );
    }

    #[test]
    fn placement_persists_the_requested_tenant_slug() {
        let mut reg = registry_with([cell(
            "cell-w-1",
            "eu-west",
            IsolationKind::Pool,
            5,
            CellStatus::Active,
        )]);
        let svc = PlacementService::new(CounterMinter::new());
        let answer = svc
            .place(
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "acme",
            )
            .expect("placed");
        let row = reg.placement(&answer.tenant_id).expect("stored");
        assert_eq!(row.slug, "acme");
    }

    #[test]
    fn assignment_is_region_first_tier_second_capacity_third_stability_always() {
        let reg = registry_with([
            cell(
                "cell-n-1",
                "eu-north",
                IsolationKind::Pool,
                1,
                CellStatus::Active,
            ),
            cell(
                "cell-w-ded",
                "eu-west",
                IsolationKind::Dedicated,
                2,
                CellStatus::Active,
            ),
            cell(
                "cell-w-hi",
                "eu-west",
                IsolationKind::Pool,
                80,
                CellStatus::Active,
            ),
            cell(
                "cell-w-lo",
                "eu-west",
                IsolationKind::Pool,
                20,
                CellStatus::Active,
            ),
            cell(
                "cell-w-tie",
                "eu-west",
                IsolationKind::Pool,
                20,
                CellStatus::Active,
            ),
        ]);

        let chosen = reg
            .assign_cell(&Region::new("eu-west"), IsolationKind::Pool)
            .expect("an eligible cell exists");
        assert_eq!(
            chosen.as_str(),
            "cell-w-lo",
            "lowest-util in-region right-tier, tie by opaque id"
        );

        let order: Vec<String> = reg
            .cells_in_assignment_order(&Region::new("eu-west"), IsolationKind::Pool)
            .iter()
            .map(|c| c.cell_id.as_str().to_string())
            .collect();
        assert_eq!(order, vec!["cell-w-lo", "cell-w-tie", "cell-w-hi"]);
    }

    #[test]
    fn place_refuses_rather_than_cross_region() {
        let mut reg = registry_with([cell(
            "cell-n-1",
            "eu-north",
            IsolationKind::Pool,
            1,
            CellStatus::Active,
        )]);
        let svc = PlacementService::new(CounterMinter::new());
        let err = svc
            .place(
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "acme",
            )
            .expect_err("no eligible in-region cell ⇒ refused, never cross-region");
        assert_eq!(
            err,
            PlaceError::NoEligibleCell {
                region: Region::new("eu-west"),
                requested_tier: IsolationKind::Pool,
            }
        );
        assert_eq!(svc.signals().placement_count, 0);
        assert!(
            err.to_string()
                .contains("NEVER places into a different region"),
            "loud: {err}"
        );
    }

    #[test]
    fn place_skips_a_fully_utilised_cell() {
        let mut reg = registry_with([cell(
            "cell-w-full",
            "eu-west",
            IsolationKind::Pool,
            100,
            CellStatus::Active,
        )]);
        let svc = PlacementService::new(CounterMinter::new());
        let err = svc
            .place(
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "acme",
            )
            .expect_err("a fully-utilised cell has no headroom");
        assert!(matches!(err, PlaceError::NoEligibleCell { .. }));
        assert!(
            reg.cells_in_assignment_order(&Region::new("eu-west"), IsolationKind::Pool)
                .is_empty(),
            "a 100%-utilised cell is excluded by capacity-third"
        );

        reg.insert_cell(cell(
            "cell-w-99",
            "eu-west",
            IsolationKind::Pool,
            99,
            CellStatus::Active,
        ));
        let chosen = reg.assign_cell(&Region::new("eu-west"), IsolationKind::Pool);
        assert_eq!(
            chosen.as_ref().map(|c| c.as_str()),
            Some("cell-w-99"),
            "a 99% cell still has headroom"
        );
    }

    #[test]
    fn place_skips_a_non_active_cell() {
        let mut reg = registry_with([cell(
            "cell-w-prov",
            "eu-west",
            IsolationKind::Pool,
            1,
            CellStatus::Provisioning,
        )]);
        let svc = PlacementService::new(CounterMinter::new());
        let err = svc
            .place(
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "acme",
            )
            .expect_err("a Provisioning cell accepts no placements");
        assert!(matches!(err, PlaceError::NoEligibleCell { .. }));
    }

    #[test]
    fn placement_is_a_sticky_stored_fact() {
        let mut reg = registry_with([cell(
            "cell-w-hi",
            "eu-west",
            IsolationKind::Pool,
            70,
            CellStatus::Active,
        )]);
        let svc = PlacementService::new(CounterMinter::new());
        let answer = svc
            .place(
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "acme",
            )
            .expect("placed on the only cell");
        assert_eq!(answer.home_cell.as_str(), "cell-w-hi");

        reg.insert_cell(cell(
            "cell-w-lo",
            "eu-west",
            IsolationKind::Pool,
            1,
            CellStatus::Active,
        ));
        let sticky = svc
            .answer_for(&reg, &answer.tenant_id)
            .expect("the placement is sticky");
        assert_eq!(
            sticky.home_cell.as_str(),
            "cell-w-hi",
            "the placed cell is sticky, never re-hashed"
        );
        assert_eq!(
            sticky, answer,
            "the same routing answer is returned for the placed tenant"
        );
        assert_eq!(
            svc.signals().placement_count,
            1,
            "answer_for does not re-place"
        );
    }

    #[test]
    fn each_place_mints_a_unique_id() {
        let mut reg = registry_with([cell(
            "cell-w-1",
            "eu-west",
            IsolationKind::Pool,
            5,
            CellStatus::Active,
        )]);
        let svc = PlacementService::new(CounterMinter::new());
        let a = svc
            .place(
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "acme",
            )
            .expect("a");
        let b = svc
            .place(
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "beta",
            )
            .expect("b");
        assert_ne!(
            a.tenant_id, b.tenant_id,
            "each placement mints a unique opaque id"
        );
        assert_eq!(svc.signals().placement_count, 2);
    }

    #[test]
    fn provision_latency_is_recorded() {
        struct SteppingClock {
            now: AtomicU64,
            step: u64,
        }
        impl Clock for SteppingClock {
            fn now_secs(&self) -> u64 {
                self.now.fetch_add(self.step, Ordering::SeqCst)
            }
        }

        let mut reg = registry_with([
            cell(
                "cell-w-1",
                "eu-west",
                IsolationKind::Pool,
                5,
                CellStatus::Active,
            ),
            cell(
                "cell-w-2",
                "eu-west",
                IsolationKind::Pool,
                6,
                CellStatus::Active,
            ),
        ]);
        let clock = SteppingClock {
            now: AtomicU64::new(1_000),
            step: 3,
        };
        let svc = PlacementService::with_clock(CounterMinter::new(), clock);

        svc.place(
            &mut reg,
            &Region::new("eu-west"),
            IsolationKind::Pool,
            "acme",
        )
        .expect("placed");
        assert_eq!(
            svc.signals().provision_latency_secs,
            3,
            "one place records a 3s span"
        );

        svc.place(
            &mut reg,
            &Region::new("eu-west"),
            IsolationKind::Pool,
            "beta",
        )
        .expect("placed");
        assert_eq!(
            svc.signals().provision_latency_secs,
            6,
            "the span sums across placements"
        );
    }

    #[test]
    fn placement_service_debug_is_pii_free_and_aggregate() {
        let mut reg = registry_with([cell(
            "cell-w-1",
            "eu-west",
            IsolationKind::Pool,
            5,
            CellStatus::Active,
        )]);
        let svc = PlacementService::new(CounterMinter::new());
        let answer = svc
            .place(
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "acme",
            )
            .expect("placed");
        let dbg = format!("{svc:?}");
        assert!(
            dbg.contains("placement_count"),
            "the Debug shows the aggregate count: {dbg}"
        );
        assert!(
            dbg.contains("provision_latency_secs"),
            "the Debug shows the latency aggregate: {dbg}"
        );
        assert!(
            !dbg.contains(answer.tenant_id.as_str()),
            "the Debug leaks no tenant id: {dbg}"
        );
    }

    #[test]
    fn serves_tier_is_exact_match_in_v1() {
        assert!(Registry::serves_tier(
            IsolationKind::Pool,
            IsolationKind::Pool
        ));
        assert!(!Registry::serves_tier(
            IsolationKind::Pool,
            IsolationKind::Dedicated
        ));
        assert!(Registry::serves_tier(
            IsolationKind::Dedicated,
            IsolationKind::Dedicated
        ));
    }

    #[test]
    fn cdc_12_3_place_provider_consumer() {
        struct SignupEdge<'a, M: TokenMinter, C: Clock> {
            svc: &'a PlacementService<M, C>,
        }
        impl<M: TokenMinter, C: Clock> SignupEdge<'_, M, C> {
            fn signup_phase1(
                &self,
                registry: &mut Registry,
                region: &Region,
                tier: IsolationKind,
                slug: &str,
            ) -> Result<PlacementAnswer, PlaceError> {
                self.svc.place(registry, region, tier, slug)
            }
            fn signup_phase2_in_cell<'b>(&self, answer: &'b PlacementAnswer) -> &'b str {
                &answer.cell_endpoint
            }
        }

        let mut reg = registry_with([cell(
            "cell-w-1",
            "eu-west",
            IsolationKind::Pool,
            5,
            CellStatus::Active,
        )]);
        let svc = PlacementService::new(CounterMinter::new());
        let edge = SignupEdge { svc: &svc };

        let answer = edge
            .signup_phase1(
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "acme",
            )
            .expect("the signup edge places the tenant PII-free");
        let in_cell_endpoint = edge.signup_phase2_in_cell(&answer);
        assert_eq!(in_cell_endpoint, "cell.eu-west.cell-w-1.myelin.eu");
        let row = reg
            .placement(&answer.tenant_id)
            .expect("the routing record is stored");
        assert_eq!(
            row.status,
            PlacementStatus::Pending,
            "identity capture is phase 2, in-cell"
        );
        assert_eq!(row.region.as_str(), "eu-west");
    }
}
