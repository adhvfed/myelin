use std::collections::BTreeMap;

use myelin_substrate::{BoundedQueue, RunClass};
use myelin_tenancy::CellId;

pub const SURGE_MULTIPLIER: u32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellAdmission {
    Admitted,
    Shed {
        retry_after_secs: u64,
    },
    Faulted,
}

impl CellAdmission {
    pub fn is_admitted(self) -> bool {
        matches!(self, CellAdmission::Admitted)
    }

    pub fn is_shed(self) -> bool {
        matches!(self, CellAdmission::Shed { .. })
    }

    pub fn is_faulted(self) -> bool {
        matches!(self, CellAdmission::Faulted)
    }
}

#[derive(Clone, Debug)]
pub struct CellBulkhead {
    cell_id: CellId,
    human_lane: BoundedQueue,
    agent_lane: BoundedQueue,
    retry_after_secs: u64,
    faulted: bool,
    admitted: u64,
    shed: u64,
}

impl CellBulkhead {
    pub fn new(
        cell_id: CellId,
        human_lane_capacity: u32,
        agent_lane_capacity: u32,
        retry_after_secs: u64,
    ) -> CellBulkhead {
        CellBulkhead {
            cell_id,
            human_lane: BoundedQueue::new(human_lane_capacity),
            agent_lane: BoundedQueue::new(agent_lane_capacity),
            retry_after_secs,
            faulted: false,
            admitted: 0,
            shed: 0,
        }
    }

    pub fn cell_id(&self) -> &CellId {
        &self.cell_id
    }

    pub fn inject_fatal_fault(&mut self) {
        self.faulted = true;
    }

    pub fn recover(&mut self) {
        self.faulted = false;
    }

    pub fn is_faulted(&self) -> bool {
        self.faulted
    }

    pub fn offer(&mut self, lane: RunClass) -> CellAdmission {
        if self.faulted {
            return CellAdmission::Faulted;
        }
        let queue = if lane == RunClass::Human {
            &mut self.human_lane
        } else {
            &mut self.agent_lane
        };
        if queue.try_acquire() {
            self.admitted += 1;
            CellAdmission::Admitted
        } else {
            self.shed += 1;
            CellAdmission::Shed {
                retry_after_secs: self.retry_after_secs,
            }
        }
    }

    pub fn release(&mut self, lane: RunClass) {
        if lane == RunClass::Human {
            self.human_lane.release();
        } else {
            self.agent_lane.release();
        }
    }

    pub fn admitted(&self) -> u64 {
        self.admitted
    }

    pub fn shed(&self) -> u64 {
        self.shed
    }

    pub fn agent_lane_shed(&self) -> u64 {
        self.agent_lane.shed_count()
    }

    pub fn human_lane_shed(&self) -> u64 {
        self.human_lane.shed_count()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellFleetReport {
    pub surged_cell_agent_shed: u64,
    pub surged_cell_human_held: bool,
    pub cross_cell_impact: usize,
    pub other_cells_unaffected: usize,
}

impl CellFleetReport {
    pub fn is_cp_d5_win(&self) -> bool {
        self.cross_cell_impact == 0
            && self.surged_cell_human_held
            && self.surged_cell_agent_shed > 0
    }
}

#[derive(Clone, Debug, Default)]
pub struct CellFleet {
    cells: BTreeMap<CellId, CellBulkhead>,
}

impl CellFleet {
    pub fn new() -> CellFleet {
        CellFleet::default()
    }

    pub fn insert(&mut self, cell: CellBulkhead) {
        self.cells.insert(cell.cell_id().clone(), cell);
    }

    pub fn cell(&self, id: &CellId) -> Option<&CellBulkhead> {
        self.cells.get(id)
    }

    pub fn cell_mut(&mut self, id: &CellId) -> Option<&mut CellBulkhead> {
        self.cells.get_mut(id)
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn run_surge(&mut self, target: &CellId, surge_requests: u32) -> CellFleetReport {
        let other_ids: Vec<CellId> = self
            .cells
            .keys()
            .filter(|id| *id != target)
            .cloned()
            .collect();
        let before: BTreeMap<CellId, u64> = other_ids
            .iter()
            .map(|id| (id.clone(), self.cells[id].human_lane_shed()))
            .collect();

        {
            let tgt = self
                .cells
                .get_mut(target)
                .expect("the surge target cell exists");
            let _ = tgt.offer(RunClass::Human);
            tgt.release(RunClass::Human);
            for _ in 0..surge_requests {
                let _ = tgt.offer(RunClass::Agent);
            }
        }

        for id in &other_ids {
            let other = self.cells.get_mut(id).expect("an other cell exists");
            let _ = other.offer(RunClass::Human);
            other.release(RunClass::Human);
        }

        let cross_cell_impact = self.cross_cell_impact(target, &before);

        let tgt = &self.cells[target];
        CellFleetReport {
            surged_cell_agent_shed: tgt.agent_lane_shed(),
            surged_cell_human_held: tgt.human_lane_shed() == 0,
            cross_cell_impact,
            other_cells_unaffected: other_ids.len() - cross_cell_impact,
        }
    }

    pub fn cross_cell_impact(&self, target: &CellId, before: &BTreeMap<CellId, u64>) -> usize {
        self.cells
            .iter()
            .filter(|(id, _)| *id != target)
            .filter(|(id, cell)| {
                let human_shed_before = before.get(*id).copied().unwrap_or(0);
                cell.human_lane_shed() > human_shed_before
            })
            .count()
    }

    pub fn shared_queue_impact(
        shared_capacity: u32,
        surge_requests: u32,
        other_cells: usize,
    ) -> usize {
        if surge_requests > shared_capacity {
            other_cells
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_tenancy::Region;

    fn cell_id(s: &str) -> CellId {
        CellId::from_token(s)
    }

    fn three_cell_fleet() -> CellFleet {
        let mut fleet = CellFleet::new();
        for id in ["cell-w-1", "cell-w-2", "cell-w-3"] {
            fleet.insert(CellBulkhead::new(cell_id(id), 100, 10, 5));
        }
        let _ = Region::new("eu-west");
        fleet
    }

    #[test]
    fn surge_in_one_cell_sheds_its_agents_holds_humans_others_unaffected() {
        let mut fleet = three_cell_fleet();
        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);

        assert!(
            report.surged_cell_agent_shed > 0,
            "the agent lane shed the surge: {report:?}"
        );
        assert_eq!(
            report.surged_cell_agent_shed, 20,
            "30 agent offers, lane cap 10 → 20 shed"
        );
        assert!(
            report.surged_cell_human_held,
            "the human lane held within budget"
        );
        assert_eq!(
            report.cross_cell_impact, 0,
            "cross-cell impact 0 (the CP-D5 zero)"
        );
        assert_eq!(report.other_cells_unaffected, 2, "both other cells served");
        assert!(report.is_cp_d5_win(), "the CP-D5 win: {report:?}");

        for id in ["cell-w-2", "cell-w-3"] {
            let c = fleet.cell(&cell_id(id)).unwrap();
            assert_eq!(
                c.shed(),
                0,
                "{id} shed nothing - the surge did not reach it"
            );
            assert!(c.admitted() >= 1, "{id} kept serving its baseline");
        }
    }

    #[test]
    fn fatal_fault_in_one_cell_is_contained_to_that_cell() {
        let mut fleet = three_cell_fleet();
        {
            let target = fleet.cells.get_mut(&cell_id("cell-w-1")).unwrap();
            target.inject_fatal_fault();
            assert!(target.offer(RunClass::Human).is_faulted());
            assert!(target.offer(RunClass::Agent).is_faulted());
            assert!(target.is_faulted());
        }
        for id in ["cell-w-2", "cell-w-3"] {
            let other = fleet.cells.get_mut(&cell_id(id)).unwrap();
            assert!(!other.is_faulted(), "{id} is not faulted");
            assert!(
                other.offer(RunClass::Human).is_admitted(),
                "{id} keeps serving humans"
            );
            assert!(
                other.offer(RunClass::Agent).is_admitted(),
                "{id} keeps serving agents"
            );
        }
    }

    #[test]
    fn noisy_tenant_is_contained_to_its_cell() {
        let mut fleet = three_cell_fleet();
        let report = fleet.run_surge(&cell_id("cell-w-2"), 50);
        assert!(
            report.surged_cell_agent_shed > 0,
            "the noisy tenant's surge shed its own cell's agent lane"
        );
        assert_eq!(
            report.cross_cell_impact, 0,
            "the noisy tenant is contained to its cell"
        );
        assert!(report.is_cp_d5_win());
    }

    #[test]
    fn offer_admits_to_the_bound_then_sheds() {
        let mut cell = CellBulkhead::new(cell_id("c"), 2, 2, 7);
        assert!(cell.offer(RunClass::Human).is_admitted());
        assert!(cell.offer(RunClass::Human).is_admitted());
        let shed = cell.offer(RunClass::Human);
        assert!(shed.is_shed(), "third human offer sheds (lane full)");
        assert_eq!(
            shed,
            CellAdmission::Shed {
                retry_after_secs: 7
            },
            "sheds with the cell's Retry-After"
        );
        cell.release(RunClass::Human);
        assert!(
            cell.offer(RunClass::Human).is_admitted(),
            "freed slot admits"
        );
    }

    #[test]
    fn faulted_cell_refuses_every_request() {
        let mut cell = CellBulkhead::new(cell_id("c"), 100, 100, 5);
        cell.inject_fatal_fault();
        assert_eq!(cell.offer(RunClass::Human), CellAdmission::Faulted);
        assert_eq!(cell.offer(RunClass::Agent), CellAdmission::Faulted);
        cell.recover();
        assert!(
            cell.offer(RunClass::Human).is_admitted(),
            "recovered cell admits"
        );
    }

    #[test]
    fn human_lane_holds_while_agent_lane_sheds_under_surge() {
        let mut cell = CellBulkhead::new(cell_id("c"), 100, 5, 5);
        for _ in 0..10 {
            assert!(cell.offer(RunClass::Human).is_admitted());
            cell.release(RunClass::Human);
        }
        for _ in 0..30 {
            let _ = cell.offer(RunClass::Agent);
        }
        assert_eq!(cell.human_lane_shed(), 0, "the human lane held (0 shed)");
        assert_eq!(cell.agent_lane_shed(), 25, "the agent lane shed 25 of 30");
    }

    #[test]
    fn cross_cell_impact_measured_against_pre_surge_baseline() {
        let mut fleet = three_cell_fleet();
        {
            let c2 = fleet.cells.get_mut(&cell_id("cell-w-2")).unwrap();
            for _ in 0..20 {
                let _ = c2.offer(RunClass::Agent);
            }
            assert!(c2.shed() >= 10, "cell-w-2 has pre-surge sheds");
        }
        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
        assert_eq!(
            report.cross_cell_impact, 0,
            "pre-surge sheds in another cell are not cross-cell impact (measured against baseline)"
        );
    }

    #[test]
    fn shared_queue_model_shows_non_zero_cross_cell_impact() {
        let impact = CellFleet::shared_queue_impact(10, SURGE_MULTIPLIER, 2);
        assert_eq!(
            impact, 2,
            "a shared queue saturated by the surge sheds BOTH other cells (cross-cell impact 2 - RED)"
        );
        assert!(impact > 0, "the shared-queue anti-pattern is NOT contained");

        let mut fleet = three_cell_fleet();
        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
        assert_eq!(
            report.cross_cell_impact, 0,
            "the independent bulkhead contains the SAME surge (cross-cell impact 0 - GREEN)"
        );
    }

    #[test]
    fn is_cp_d5_win_requires_all_conjuncts() {
        assert!(CellFleetReport {
            surged_cell_agent_shed: 20,
            surged_cell_human_held: true,
            cross_cell_impact: 0,
            other_cells_unaffected: 2,
        }
        .is_cp_d5_win());
        assert!(!CellFleetReport {
            surged_cell_agent_shed: 20,
            surged_cell_human_held: true,
            cross_cell_impact: 1,
            other_cells_unaffected: 1,
        }
        .is_cp_d5_win());
        assert!(!CellFleetReport {
            surged_cell_agent_shed: 20,
            surged_cell_human_held: false,
            cross_cell_impact: 0,
            other_cells_unaffected: 2,
        }
        .is_cp_d5_win());
        assert!(!CellFleetReport {
            surged_cell_agent_shed: 0,
            surged_cell_human_held: true,
            cross_cell_impact: 0,
            other_cells_unaffected: 2,
        }
        .is_cp_d5_win());
    }

    #[test]
    fn cross_cell_impact_counts_an_affected_other_cell() {
        let mut fleet = CellFleet::new();
        fleet.insert(CellBulkhead::new(cell_id("cell-w-1"), 100, 10, 5));
        fleet.insert(CellBulkhead::new(cell_id("cell-w-2"), 1, 10, 5));
        fleet.insert(CellBulkhead::new(cell_id("cell-w-3"), 1, 10, 5));
        let before: BTreeMap<CellId, u64> =
            [(cell_id("cell-w-2"), 0u64), (cell_id("cell-w-3"), 0u64)]
                .into_iter()
                .collect();
        assert_eq!(fleet.cross_cell_impact(&cell_id("cell-w-1"), &before), 0);

        {
            let c2 = fleet.cell_mut(&cell_id("cell-w-2")).unwrap();
            assert!(c2.offer(RunClass::Human).is_admitted());
            assert!(c2.offer(RunClass::Human).is_shed());
            assert_eq!(c2.human_lane_shed(), 1, "cell-w-2's human lane shed once");
        }
        assert_eq!(
            fleet.cross_cell_impact(&cell_id("cell-w-1"), &before),
            1,
            "exactly the cell whose human lane rose above baseline is counted"
        );

        {
            let t = fleet.cell_mut(&cell_id("cell-w-1")).unwrap();
            for _ in 0..200 {
                let _ = t.offer(RunClass::Human);
            }
            assert!(t.human_lane_shed() > 0, "the target's human lane shed");
        }
        assert_eq!(
            fleet.cross_cell_impact(&cell_id("cell-w-1"), &before),
            1,
            "the target cell is excluded from its own cross-cell impact (still just cell-w-2)"
        );
    }

    #[test]
    fn run_surge_unaffected_is_other_count_minus_impact() {
        let mut fleet = three_cell_fleet();
        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
        assert_eq!(report.cross_cell_impact, 0);
        assert_eq!(report.other_cells_unaffected, 2);
        assert_eq!(
            report.other_cells_unaffected + report.cross_cell_impact,
            2,
            "unaffected + impact == the other-cell count (the difference is exact)"
        );
    }

    #[test]
    fn run_surge_reports_difference_when_an_other_cell_is_affected() {
        let mut fleet = CellFleet::new();
        fleet.insert(CellBulkhead::new(cell_id("cell-w-1"), 100, 10, 5));
        let mut c2 = CellBulkhead::new(cell_id("cell-w-2"), 1, 10, 5);
        assert!(c2.offer(RunClass::Human).is_admitted());
        fleet.insert(c2);
        fleet.insert(CellBulkhead::new(cell_id("cell-w-3"), 100, 10, 5));

        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
        assert_eq!(
            report.cross_cell_impact, 1,
            "the saturated other cell's human lane shed → impact 1"
        );
        assert_eq!(
            report.other_cells_unaffected, 1,
            "unaffected = other_count(2) - impact(1) = 1 (the subtraction, not a sum)"
        );
        assert!(
            !report.is_cp_d5_win(),
            "an affected other cell is NOT the CP-D5 win"
        );
    }

    #[test]
    fn counters_return_real_values_not_constants() {
        let mut cell = CellBulkhead::new(cell_id("c"), 1, 1, 5);
        assert_eq!(cell.admitted(), 0, "a fresh cell admitted nothing");
        assert!(cell.offer(RunClass::Human).is_admitted());
        assert!(cell.offer(RunClass::Human).is_shed());
        assert!(cell.offer(RunClass::Human).is_shed());
        assert_eq!(cell.human_lane_shed(), 2, "two human-lane sheds");
        assert_eq!(
            cell.admitted(),
            1,
            "one admitted (not a constant 1 by luck - fresh was 0)"
        );
    }

    #[test]
    fn is_empty_reflects_contents() {
        let empty = CellFleet::new();
        assert!(empty.is_empty(), "a fresh fleet is empty");
        assert_eq!(empty.len(), 0);
        let populated = three_cell_fleet();
        assert!(!populated.is_empty(), "a populated fleet is not empty");
        assert_eq!(populated.len(), 3);
    }

    #[test]
    fn admission_predicates_discriminate_variants() {
        let admitted = CellAdmission::Admitted;
        let shed = CellAdmission::Shed {
            retry_after_secs: 5,
        };
        let faulted = CellAdmission::Faulted;
        assert!(admitted.is_admitted());
        assert!(!shed.is_admitted());
        assert!(!faulted.is_admitted());
        assert!(shed.is_shed());
        assert!(!admitted.is_shed());
        assert!(!faulted.is_shed());
        assert!(faulted.is_faulted());
        assert!(!admitted.is_faulted());
        assert!(!shed.is_faulted());
    }

    #[test]
    fn shared_queue_impact_boundary_is_strict() {
        assert_eq!(
            CellFleet::shared_queue_impact(30, 30, 2),
            0,
            "surge == cap → 0"
        );
        assert_eq!(
            CellFleet::shared_queue_impact(30, 31, 2),
            2,
            "surge > cap → impact"
        );
    }

    #[test]
    fn cdc_cell_bulkhead_containment_provider_consumer() {
        struct OpsSurgeDrill;
        impl OpsSurgeDrill {
            fn read_verdict(report: &CellFleetReport) -> (bool, usize) {
                (report.is_cp_d5_win(), report.cross_cell_impact)
            }
        }

        let mut fleet = three_cell_fleet();
        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);

        let (contained, impact) = OpsSurgeDrill::read_verdict(&report);
        assert!(contained, "the bulkhead held (CP-D5 win)");
        assert_eq!(impact, 0, "cross-cell impact 0");

        let shared_impact = CellFleet::shared_queue_impact(10, SURGE_MULTIPLIER, 2);
        assert!(
            shared_impact > 0,
            "a shared queue is NOT contained (the consumer reads it RED)"
        );
    }
}
