use std::collections::BTreeMap;

use myelin_events::Timestamp;
use myelin_identity::Zookie;
use myelin_tenancy::{CellId, CrossCellPointer, OpaqueSubjectId, Region, TenantId};

use crate::cross_cell_bridge::{BridgeMode, BridgeResolution, CrossCellBridge, ViewerId};
use crate::registry::{PlacementError, Registry};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellDsrReceipt {
    pub cell: CellId,
    pub subject: OpaqueSubjectId,
    pub receipt: String,
}

pub trait CellLocalEraser: Send + Sync {
    fn erase_in_cell(
        &self,
        tenant: &TenantId,
        subject: &OpaqueSubjectId,
        now: &Timestamp,
    ) -> CellDsrReceipt;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCellDsrReceiptSet {
    pub subject: OpaqueSubjectId,
    pub tenant: TenantId,
    pub fan_out_cells: Vec<CellId>,
    pub receipts: Vec<CellDsrReceipt>,
    pub ran_at: Timestamp,
}

impl MultiCellDsrReceiptSet {
    pub fn cells_missed(&self) -> usize {
        self.fan_out_cells
            .iter()
            .filter(|c| !self.receipts.iter().any(|r| &r.cell == *c))
            .count()
    }

    pub fn is_complete(&self) -> bool {
        self.cells_missed() == 0 && self.receipts.len() == self.fan_out_cells.len()
    }

    pub fn summary(&self) -> String {
        format!(
            "GA-D8 cross-cell DSR fan-out [{}]: subject={} tenant={} fan_out_cells={} receipts={} \
             cells_missed={} -> {}",
            self.ran_at.0,
            self.subject.artifact_ref().0,
            self.tenant.as_str(),
            self.fan_out_cells.len(),
            self.receipts.len(),
            self.cells_missed(),
            if self.is_complete() { "GREEN" } else { "RED" },
        )
    }
}

#[derive(Default)]
pub struct CrossCellDsrFanOut {
    erasers: BTreeMap<CellId, std::sync::Arc<dyn CellLocalEraser>>,
}

impl CrossCellDsrFanOut {
    pub fn new() -> CrossCellDsrFanOut {
        CrossCellDsrFanOut::default()
    }

    pub fn register(&mut self, cell: CellId, eraser: std::sync::Arc<dyn CellLocalEraser>) {
        self.erasers.insert(cell, eraser);
    }

    pub fn fan_out(
        &self,
        subject: &OpaqueSubjectId,
        tenant: &TenantId,
        home_cell: &CellId,
        member_cells: &[CellId],
        now: Timestamp,
    ) -> MultiCellDsrReceiptSet {
        let mut fan_out_cells: Vec<CellId> = Vec::new();
        for c in std::iter::once(home_cell).chain(member_cells.iter()) {
            if !fan_out_cells.contains(c) {
                fan_out_cells.push(c.clone());
            }
        }
        let mut receipts = Vec::with_capacity(fan_out_cells.len());
        for cell in &fan_out_cells {
            if let Some(eraser) = self.erasers.get(cell) {
                receipts.push(eraser.erase_in_cell(tenant, subject, &now));
            }
        }
        MultiCellDsrReceiptSet {
            subject: subject.clone(),
            tenant: tenant.clone(),
            fan_out_cells,
            receipts,
            ran_at: now,
        }
    }
}

pub const ZOOKIE_STALENESS_BUDGET_SECS: u64 = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZookieStaleness {
    WithinBound {
        home_zookie: Zookie,
        observed_staleness_secs: u64,
    },
    PastBound {
        home_zookie: Zookie,
        observed_staleness_secs: u64,
    },
}

impl ZookieStaleness {
    pub fn is_within_bound(&self) -> bool {
        matches!(self, ZookieStaleness::WithinBound { .. })
    }

    pub fn observed_staleness_secs(&self) -> u64 {
        match self {
            ZookieStaleness::WithinBound {
                observed_staleness_secs,
                ..
            }
            | ZookieStaleness::PastBound {
                observed_staleness_secs,
                ..
            } => *observed_staleness_secs,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CrossCellZookieReader;

impl CrossCellZookieReader {
    pub fn new() -> CrossCellZookieReader {
        CrossCellZookieReader
    }

    pub fn read_through(
        &self,
        home_zookie: &Zookie,
        home_minted_at_secs: u64,
        member_observed_at_secs: u64,
    ) -> ZookieStaleness {
        let observed_staleness_secs = home_minted_at_secs.saturating_sub(member_observed_at_secs);
        if observed_staleness_secs <= ZOOKIE_STALENESS_BUDGET_SECS {
            ZookieStaleness::WithinBound {
                home_zookie: home_zookie.clone(),
                observed_staleness_secs,
            }
        } else {
            ZookieStaleness::PastBound {
                home_zookie: home_zookie.clone(),
                observed_staleness_secs,
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebalanceReceipt {
    pub tenant: TenantId,
    pub from_cell: CellId,
    pub to_cell: CellId,
    pub region: Region,
    pub member_cells_after: Vec<CellId>,
}

impl Registry {
    pub fn add_member_cell(
        &mut self,
        tenant_id: &TenantId,
        new_member: CellId,
    ) -> Result<Vec<CellId>, PlacementError> {
        let Some(current) = self.placement(tenant_id) else {
            return Err(PlacementError::UnknownCell {
                tenant: tenant_id.clone(),
                cell: new_member,
            });
        };
        let mut proposed = current.clone();
        if !proposed.member_cells.contains(&new_member) {
            proposed.member_cells.push(new_member);
        }
        self.check_placement_invariant(&proposed)?;
        let after = proposed.member_cells.clone();
        self.place_tenant(proposed)?;
        Ok(after)
    }

    pub fn rebalance_member_cell(
        &mut self,
        tenant_id: &TenantId,
        from_cell: &CellId,
        to_cell: CellId,
    ) -> Result<RebalanceReceipt, PlacementError> {
        let Some(current) = self.placement(tenant_id) else {
            return Err(PlacementError::UnknownCell {
                tenant: tenant_id.clone(),
                cell: to_cell,
            });
        };
        let mut member_cells: Vec<CellId> = current
            .member_cells
            .iter()
            .filter(|c| *c != from_cell)
            .cloned()
            .collect();
        if !member_cells.contains(&to_cell) {
            member_cells.push(to_cell.clone());
        }
        let mut proposed = current.clone();
        proposed.member_cells = member_cells.clone();
        self.check_placement_invariant(&proposed)?;
        let region = proposed.region.clone();
        self.place_tenant(proposed)?;
        Ok(RebalanceReceipt {
            tenant: tenant_id.clone(),
            from_cell: from_cell.clone(),
            to_cell,
            region,
            member_cells_after: member_cells,
        })
    }
}

pub fn resolve_across_member_cells(
    bridge: &CrossCellBridge,
    pointers: &[CrossCellPointer],
    viewer: &ViewerId,
    mode: BridgeMode,
) -> Vec<BridgeResolution> {
    pointers
        .iter()
        .map(|p| bridge.resolve(p, viewer, mode))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
    };
    use myelin_tenancy::ArtifactRef;
    use std::sync::Arc;

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

    fn subject(s: &str) -> OpaqueSubjectId {
        OpaqueSubjectId::from_ref(ArtifactRef(s.into()))
    }

    struct CellEraser {
        cell: CellId,
        receipted: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }
    impl CellEraser {
        fn new(cell: &str) -> CellEraser {
            CellEraser {
                cell: CellId::from_token(cell),
                receipted: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
    }
    impl CellLocalEraser for CellEraser {
        fn erase_in_cell(
            &self,
            tenant: &TenantId,
            subject: &OpaqueSubjectId,
            _now: &Timestamp,
        ) -> CellDsrReceipt {
            self.receipted
                .lock()
                .unwrap()
                .push((tenant.as_str().into(), subject.artifact_ref().0.clone()));
            CellDsrReceipt {
                cell: self.cell.clone(),
                subject: subject.clone(),
                receipt: format!(
                    "receipt:{}:{}",
                    self.cell.as_str(),
                    subject.artifact_ref().0
                ),
            }
        }
    }

    #[test]
    fn dsr_fan_out_iterates_all_member_cells_and_misses_zero() {
        let mut fanout = CrossCellDsrFanOut::new();
        let b = CellEraser::new("cell-b");
        let c = CellEraser::new("cell-c");
        let d = CellEraser::new("cell-d");
        let b_seen = b.receipted.clone();
        fanout.register(CellId::from_token("cell-b"), Arc::new(b));
        fanout.register(CellId::from_token("cell-c"), Arc::new(c));
        fanout.register(CellId::from_token("cell-d"), Arc::new(d));

        let set = fanout.fan_out(
            &subject("p1"),
            &TenantId::from_token("01J0ACME"),
            &CellId::from_token("cell-b"),
            &[CellId::from_token("cell-c"), CellId::from_token("cell-d")],
            Timestamp("2026-06-24T00:00:00Z".into()),
        );
        assert_eq!(set.fan_out_cells.len(), 3);
        assert_eq!(set.receipts.len(), 3);
        assert_eq!(set.cells_missed(), 0, "0 cells missed (the GA-D8 zero)");
        assert!(set.is_complete(), "the merged receipt set is COMPLETE");
        assert_eq!(
            b_seen.lock().unwrap().as_slice(),
            &[("01J0ACME".to_string(), "p1".to_string())]
        );
        assert!(set.summary().contains("GREEN"));
        assert!(set.summary().contains("cells_missed=0"));
    }

    #[test]
    fn fan_out_always_includes_the_home_cell() {
        let mut fanout = CrossCellDsrFanOut::new();
        fanout.register(
            CellId::from_token("cell-b"),
            Arc::new(CellEraser::new("cell-b")),
        );
        fanout.register(
            CellId::from_token("cell-c"),
            Arc::new(CellEraser::new("cell-c")),
        );
        let set = fanout.fan_out(
            &subject("p1"),
            &TenantId::from_token("t"),
            &CellId::from_token("cell-b"),
            &[CellId::from_token("cell-c")],
            Timestamp("t0".into()),
        );
        assert!(set.fan_out_cells.contains(&CellId::from_token("cell-b")));
        assert_eq!(set.cells_missed(), 0);
    }

    #[test]
    fn fan_out_deduplicates_the_home_cell_in_member_cells() {
        let mut fanout = CrossCellDsrFanOut::new();
        fanout.register(
            CellId::from_token("cell-b"),
            Arc::new(CellEraser::new("cell-b")),
        );
        let set = fanout.fan_out(
            &subject("p1"),
            &TenantId::from_token("t"),
            &CellId::from_token("cell-b"),
            &[CellId::from_token("cell-b")],
            Timestamp("t0".into()),
        );
        assert_eq!(set.fan_out_cells.len(), 1, "deduplicated to one cell");
        assert_eq!(set.receipts.len(), 1, "erased once, not twice");
        assert!(set.is_complete());
    }

    #[test]
    fn an_unreachable_member_cell_is_a_missed_cell_not_silently_dropped() {
        let mut fanout = CrossCellDsrFanOut::new();
        fanout.register(
            CellId::from_token("cell-b"),
            Arc::new(CellEraser::new("cell-b")),
        );
        let set = fanout.fan_out(
            &subject("p1"),
            &TenantId::from_token("t"),
            &CellId::from_token("cell-b"),
            &[CellId::from_token("cell-c")],
            Timestamp("t0".into()),
        );
        assert_eq!(
            set.fan_out_cells.len(),
            2,
            "both cells are in the fan-out set"
        );
        assert_eq!(
            set.receipts.len(),
            1,
            "only the reachable cell returned a receipt"
        );
        assert_eq!(
            set.cells_missed(),
            1,
            "the unreachable cell is MISSED (not dropped)"
        );
        assert!(!set.is_complete(), "an incomplete set is RED");
        assert!(set.summary().contains("RED"));
        assert!(set.summary().contains("cells_missed=1"));
    }

    #[test]
    fn zookie_within_budget_is_admitted_bounded_stale() {
        let reader = CrossCellZookieReader::new();
        let z = Zookie("home-snap-100".into());
        let v = reader.read_through(&z, 1000, 940);
        assert!(v.is_within_bound(), "60s ≤ 300s budget → admitted");
        assert_eq!(v.observed_staleness_secs(), 60);
        let ZookieStaleness::WithinBound { home_zookie, .. } = v else {
            unreachable!()
        };
        assert_eq!(home_zookie, z);
    }

    #[test]
    fn member_at_or_after_home_snapshot_observes_zero_staleness() {
        let reader = CrossCellZookieReader::new();
        let z = Zookie("home-snap-100".into());
        let v = reader.read_through(&z, 1000, 1100);
        assert!(v.is_within_bound());
        assert_eq!(v.observed_staleness_secs(), 0);
    }

    #[test]
    fn zookie_past_bound_is_refused_never_a_stale_serve() {
        let reader = CrossCellZookieReader::new();
        let z = Zookie("home-snap-100".into());
        let v = reader.read_through(&z, 1000, 400);
        assert!(!v.is_within_bound(), "600s > 300s budget → REFUSED");
        assert_eq!(v.observed_staleness_secs(), 600);
        assert!(matches!(v, ZookieStaleness::PastBound { .. }));
    }

    #[test]
    fn zookie_budget_boundary_is_inclusive() {
        let reader = CrossCellZookieReader::new();
        let z = Zookie("z".into());
        assert!(reader
            .read_through(&z, ZOOKIE_STALENESS_BUDGET_SECS, 0)
            .is_within_bound());
        assert!(!reader
            .read_through(&z, ZOOKIE_STALENESS_BUDGET_SECS + 1, 0)
            .is_within_bound());
    }

    fn registry_three_cells() -> Registry {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        reg.insert_cell(cell("cell-w-3", "eu-west"));
        reg.insert_cell(cell("cell-n-1", "eu-north"));
        reg.place_tenant(TenantPlacement {
            tenant_id: TenantId::from_token("01J0ACME"),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-1")],
        })
        .expect("single-region placement admitted");
        reg
    }

    #[test]
    fn member_cells_promoted_to_multi_element_same_region() {
        let mut reg = registry_three_cells();
        let after = reg
            .add_member_cell(
                &TenantId::from_token("01J0ACME"),
                CellId::from_token("cell-w-2"),
            )
            .expect("a same-region member cell is admitted");
        assert!(after.contains(&CellId::from_token("cell-w-2")));
        let placement = reg.placement_of(&TenantId::from_token("01J0ACME")).unwrap();
        assert_eq!(
            placement.member_cells.len(),
            2,
            "member_cells is now multi-element"
        );
        assert!(placement
            .member_cells
            .contains(&CellId::from_token("cell-w-1")));
        assert!(placement
            .member_cells
            .contains(&CellId::from_token("cell-w-2")));
    }

    #[test]
    fn cross_region_member_cell_add_is_rejected() {
        let mut reg = registry_three_cells();
        let e = reg
            .add_member_cell(
                &TenantId::from_token("01J0ACME"),
                CellId::from_token("cell-n-1"),
            )
            .expect_err("a cross-region member cell is rejected");
        assert!(matches!(e, PlacementError::CrossRegionMemberCell { .. }));
        let placement = reg.placement_of(&TenantId::from_token("01J0ACME")).unwrap();
        assert_eq!(
            placement.member_cells.len(),
            1,
            "still single-element after a rejected add"
        );
    }

    #[test]
    fn rebalance_moves_workload_across_member_cells_same_region() {
        let mut reg = registry_three_cells();
        reg.add_member_cell(
            &TenantId::from_token("01J0ACME"),
            CellId::from_token("cell-w-2"),
        )
        .unwrap();
        let receipt = reg
            .rebalance_member_cell(
                &TenantId::from_token("01J0ACME"),
                &CellId::from_token("cell-w-2"),
                CellId::from_token("cell-w-3"),
            )
            .expect("a same-region rebalance is admitted");
        assert_eq!(receipt.region.as_str(), "eu-west");
        assert!(receipt
            .member_cells_after
            .contains(&CellId::from_token("cell-w-3")));
        assert!(
            !receipt
                .member_cells_after
                .contains(&CellId::from_token("cell-w-2")),
            "moved away from cell-w-2"
        );
        let placement = reg.placement_of(&TenantId::from_token("01J0ACME")).unwrap();
        assert!(placement
            .member_cells
            .contains(&CellId::from_token("cell-w-3")));
        assert!(!placement
            .member_cells
            .contains(&CellId::from_token("cell-w-2")));
    }

    #[test]
    fn cross_region_rebalance_is_rejected() {
        let mut reg = registry_three_cells();
        reg.add_member_cell(
            &TenantId::from_token("01J0ACME"),
            CellId::from_token("cell-w-2"),
        )
        .unwrap();
        let e = reg
            .rebalance_member_cell(
                &TenantId::from_token("01J0ACME"),
                &CellId::from_token("cell-w-2"),
                CellId::from_token("cell-n-1"),
            )
            .expect_err("a cross-region rebalance is rejected");
        assert!(matches!(e, PlacementError::CrossRegionMemberCell { .. }));
        let placement = reg.placement_of(&TenantId::from_token("01J0ACME")).unwrap();
        assert!(
            placement
                .member_cells
                .contains(&CellId::from_token("cell-w-2")),
            "still on cell-w-2"
        );
        assert!(!placement
            .member_cells
            .contains(&CellId::from_token("cell-n-1")));
    }

    #[test]
    fn is_complete_rejects_a_duplicate_receipt() {
        let dup = MultiCellDsrReceiptSet {
            subject: subject("p1"),
            tenant: TenantId::from_token("t"),
            fan_out_cells: vec![CellId::from_token("cell-b")],
            receipts: vec![
                CellDsrReceipt {
                    cell: CellId::from_token("cell-b"),
                    subject: subject("p1"),
                    receipt: "r1".into(),
                },
                CellDsrReceipt {
                    cell: CellId::from_token("cell-b"),
                    subject: subject("p1"),
                    receipt: "r2".into(),
                },
            ],
            ran_at: Timestamp("t0".into()),
        };
        assert_eq!(dup.cells_missed(), 0, "every fan-out cell has a receipt");
        assert_eq!(dup.receipts.len(), 2);
        assert_eq!(dup.fan_out_cells.len(), 1);
        assert!(
            !dup.is_complete(),
            "a duplicate receipt is NOT a complete set (the len clause is load-bearing)"
        );
    }

    #[test]
    fn resolve_across_member_cells_resolves_each_pointer() {
        use crate::cross_cell_bridge::{
            BridgeProjection, BridgeResolution, BridgeTombstone, BridgeTombstoneReason,
            CellLocalResolver, CellResolverRegistry, CrossCellBridge,
        };
        use myelin_tenancy::{ArtifactType, CorrelationId};

        struct Resolver;
        impl CellLocalResolver for Resolver {
            fn resolve_in_cell(
                &self,
                pointer: &CrossCellPointer,
                viewer: &ViewerId,
                _mode: BridgeMode,
            ) -> BridgeResolution {
                if viewer.as_str() == "v-ok" {
                    BridgeResolution::Projection(BridgeProjection {
                        subject: pointer.subject().clone(),
                        title: "t".into(),
                        state: "open".into(),
                        icon: "i".into(),
                    })
                } else {
                    BridgeResolution::Tombstone(BridgeTombstone {
                        subject: pointer.subject().clone(),
                        reason: BridgeTombstoneReason::Denied,
                    })
                }
            }
        }
        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(Resolver));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);
        let mk = |s: &str| {
            CrossCellPointer::new(
                subject(s),
                ArtifactType::Issue,
                CorrelationId("c".into()),
                CellId::from_token("cell-b"),
            )
        };
        let pointers = [mk("p1"), mk("p2")];
        let out = resolve_across_member_cells(
            &bridge,
            &pointers,
            &ViewerId::from_token("v-ok"),
            BridgeMode::Live,
        );
        assert_eq!(out.len(), 2, "one resolution per pointer (non-empty)");
        assert!(out.iter().all(|r| r.is_projection()));
        let denied = resolve_across_member_cells(
            &bridge,
            &pointers,
            &ViewerId::from_token("v-no"),
            BridgeMode::Live,
        );
        assert_eq!(denied.len(), 2);
        assert!(denied.iter().all(|r| r.is_tombstone()));
    }

    #[test]
    fn add_member_cell_to_unknown_tenant_is_fail_closed() {
        let mut reg = registry_three_cells();
        let e = reg
            .add_member_cell(
                &TenantId::from_token("01J0GHOST"),
                CellId::from_token("cell-w-2"),
            )
            .expect_err("an unknown tenant has no placement to extend");
        assert!(matches!(e, PlacementError::UnknownCell { .. }));
    }
}
