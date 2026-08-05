use std::collections::BTreeMap;

use myelin_ci_sandbox::{Capacity, FleetProvider, Region, RunnerClass, RunnerHost};

use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, Visibility};

pub const INSERT_RUNNER_QUERY: &str = "\
INSERT INTO runner
  (tenant_id, region, runner_id, pool, labels, ownership, trust_tier, attestation,
   attest_state, health, capacity, last_heartbeat)
VALUES ($1, $2, $3::uuid, $4, $5, $6, $7, NULL, $8, $9, $10::jsonb, now())";

pub const DELETE_RUNNER_QUERY: &str =
    "DELETE FROM runner WHERE tenant_id = $1 AND runner_id = $2::uuid";

pub const COUNT_RUNNERS_BY_POOL_QUERY: &str = "\
SELECT count(*) FROM runner
WHERE tenant_id = $1 AND region = $2 AND pool = $3 AND health = 'healthy'";

// @residency-write — the residency-pin write-boundary (layer-3) leg arms on this file: a runner
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossRegionRunnerWrite {
    pub tenant_id: String,
    pub cell_region: Region,
    pub row_region: Region,
}

impl std::fmt::Display for CrossRegionRunnerWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CI fleet runner-row write REFUSED for tenant `{}`: the row pins region `{}` but the \
             cell it lives in is region `{}` - a runner pool is partitioned per residency zone and \
             a runner row cannot exist outside its cell's region (the pin is the cell's, NOT the \
             caller's; arch 00 §5, contract 1.6). REFUSED (0 cross-region runner rows is the \
             no-global-pool green artifact).",
            self.tenant_id,
            self.row_region.as_str(),
            self.cell_region.as_str(),
        )
    }
}

impl std::error::Error for CrossRegionRunnerWrite {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerWritePin {
    tenant_id: String,
    cell_region: Region,
    cross_region_runner_rows_admitted: u64,
}

impl RunnerWritePin {
    pub fn for_cell(tenant_id: impl Into<String>, cell_region: Region) -> RunnerWritePin {
        RunnerWritePin {
            tenant_id: tenant_id.into(),
            cell_region,
            cross_region_runner_rows_admitted: 0,
        }
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    pub fn cross_region_runner_rows_admitted(&self) -> u64 {
        self.cross_region_runner_rows_admitted
    }

    pub fn admit_runner_write(
        &mut self,
        row_region: &Region,
    ) -> Result<(), CrossRegionRunnerWrite> {
        if *row_region != self.cell_region {
            return Err(CrossRegionRunnerWrite {
                tenant_id: self.tenant_id.clone(),
                cell_region: self.cell_region.clone(),
                row_region: row_region.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FleetResidencyReport {
    pub tenant_id: String,
    pub region: Region,
}

impl FleetResidencyReport {
    pub fn matches_region_of_record(&self, region_of_record: &Region) -> bool {
        self.region == *region_of_record
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolKey {
    pub region: Region,
    pub label_class: RunnerClass,
}

impl PoolKey {
    pub fn new(region: Region, label_class: RunnerClass) -> PoolKey {
        PoolKey {
            region,
            label_class,
        }
    }
}

impl PartialOrd for PoolKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PoolKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.region.as_str(), self.label_class.0.as_str())
            .cmp(&(other.region.as_str(), other.label_class.0.as_str()))
    }
}

#[derive(Clone, Debug, Default)]
pub struct FleetPools {
    counts: BTreeMap<PoolKey, u32>,
}

impl FleetPools {
    pub fn new() -> FleetPools {
        FleetPools::default()
    }

    pub fn current(&self, key: &PoolKey) -> u32 {
        self.counts.get(key).copied().unwrap_or(0)
    }

    pub fn set_current(&mut self, key: PoolKey, count: u32) {
        if count == 0 {
            self.counts.remove(&key);
        } else {
            self.counts.insert(key, count);
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &PoolKey> {
        self.counts.keys()
    }

    pub fn distinct_regions(&self) -> usize {
        let mut regions: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for k in self.counts.keys() {
            regions.insert(k.region.as_str());
        }
        regions.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoscalePolicy {
    pub warm_buffer: u32,
    pub min_warm: u32,
    pub max: u32,
}

impl AutoscalePolicy {
    pub fn new(warm_buffer: u32, min_warm: u32, max: u32) -> AutoscalePolicy {
        AutoscalePolicy {
            warm_buffer,
            min_warm,
            max,
        }
    }

    pub fn from_measured_arrival_rate(
        ci_surge: &myelin_substrate::thresholds::CiSurge,
        arrival_rate: u32,
        min_warm: u32,
        max: u32,
    ) -> AutoscalePolicy {
        AutoscalePolicy {
            warm_buffer: ci_surge.prewarm_buffer_for(arrival_rate),
            min_warm,
            max,
        }
    }

    pub fn target(self, queue_depth: u32, in_flight: u32) -> u32 {
        let demand = queue_depth.saturating_add(in_flight);
        if demand == 0 {
            return self.min_warm.min(self.max);
        }
        let want = demand.saturating_add(self.warm_buffer);
        want.clamp(self.min_warm, self.max)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalePlan {
    pub key: PoolKey,
    pub desired: u32,
    pub current: u32,
}

impl ScalePlan {
    pub fn delta(&self) -> i64 {
        self.desired as i64 - self.current as i64
    }

    pub fn provision_count(&self) -> u32 {
        self.delta().max(0) as u32
    }

    pub fn deprovision_count(&self) -> u32 {
        (-self.delta()).max(0) as u32
    }

    pub fn is_scale_to_zero(&self) -> bool {
        self.desired == 0 && self.current > 0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Autoscaler {
    policy: AutoscalePolicy,
}

impl Autoscaler {
    pub fn new(policy: AutoscalePolicy) -> Autoscaler {
        Autoscaler { policy }
    }

    pub fn policy(&self) -> AutoscalePolicy {
        self.policy
    }

    pub fn reconcile(
        &self,
        key: PoolKey,
        current: u32,
        queue_depth: u32,
        in_flight: u32,
    ) -> ScalePlan {
        let desired = self.policy.target(queue_depth, in_flight);
        ScalePlan {
            key,
            desired,
            current,
        }
    }
}

pub trait FleetAdapter {
    fn name(&self) -> &'static str;

    fn provision_hosts(&self, n: u32, region: &Region) -> Vec<String>;

    fn deprovision_hosts(&self, host_ids: &[String]);
}

#[derive(Clone, Debug, Default)]
pub struct GenericEuIaasAdapter;

impl FleetAdapter for GenericEuIaasAdapter {
    fn name(&self) -> &'static str {
        "generic-eu-iaas"
    }

    fn provision_hosts(&self, n: u32, region: &Region) -> Vec<String> {
        (0..n)
            .map(|i| format!("geniaas-{}-{}", region.as_str(), i))
            .collect()
    }

    fn deprovision_hosts(&self, _host_ids: &[String]) {
    }
}

#[derive(Clone, Debug, Default)]
pub struct BareMetalPxeAdapter;

impl FleetAdapter for BareMetalPxeAdapter {
    fn name(&self) -> &'static str {
        "bare-metal-pxe"
    }

    fn provision_hosts(&self, n: u32, region: &Region) -> Vec<String> {
        (0..n)
            .map(|i| format!("pxe-{}-{}", region.as_str(), i))
            .collect()
    }

    fn deprovision_hosts(&self, _host_ids: &[String]) {
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FleetError {
    CrossRegion(CrossRegionRunnerWrite),
}

impl std::fmt::Display for FleetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FleetError::CrossRegion(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FleetError {}

pub struct EuFleetProvider<A: FleetAdapter> {
    adapter: A,
    tenant_id: String,
    cell_region: Region,
    region_capacity: u32,
    provisioned: u32,
}

impl<A: FleetAdapter> EuFleetProvider<A> {
    pub fn new(
        adapter: A,
        tenant_id: impl Into<String>,
        cell_region: Region,
        region_capacity: u32,
    ) -> EuFleetProvider<A> {
        EuFleetProvider {
            adapter,
            tenant_id: tenant_id.into(),
            cell_region,
            region_capacity,
            provisioned: 0,
        }
    }

    pub fn adapter_name(&self) -> &'static str {
        self.adapter.name()
    }

    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub fn residency_report(&self) -> FleetResidencyReport {
        FleetResidencyReport {
            tenant_id: self.tenant_id.clone(),
            region: self.cell_region.clone(),
        }
    }

    fn admit_write(&self, row_region: &Region) -> Result<(), CrossRegionRunnerWrite> {
        let mut pin = RunnerWritePin::for_cell(self.tenant_id.clone(), self.cell_region.clone());
        pin.admit_runner_write(row_region)
    }

    pub fn set_provisioned(&mut self, provisioned: u32) {
        self.provisioned = provisioned.min(self.region_capacity);
    }

    pub fn apply(&mut self, plan: &ScalePlan) -> Result<Vec<RunnerHost>, FleetError> {
        let up = plan.provision_count();
        let down = plan.deprovision_count();
        if up > 0 {
            let hosts =
                self.provision(plan.key.label_class.clone(), up, plan.key.region.clone())?;
            self.set_provisioned(self.provisioned.saturating_add(up));
            return Ok(hosts);
        }
        if down > 0 {
            self.set_provisioned(self.provisioned.saturating_sub(down));
        }
        Ok(Vec::new())
    }
}

impl<A: FleetAdapter> FleetProvider for EuFleetProvider<A> {
    type Error = FleetError;

    fn provision(
        &self,
        _class: RunnerClass,
        n: u32,
        region: Region,
    ) -> Result<Vec<RunnerHost>, Self::Error> {
        self.admit_write(&region).map_err(FleetError::CrossRegion)?;
        let host_ids = self.adapter.provision_hosts(n, &region);
        Ok(host_ids
            .into_iter()
            .map(|host_id| RunnerHost {
                host_id,
                region: region.clone(),
            })
            .collect())
    }

    fn deprovision(&self, hosts: &[RunnerHost]) -> Result<(), Self::Error> {
        let ids: Vec<String> = hosts.iter().map(|h| h.host_id.clone()).collect();
        self.adapter.deprovision_hosts(&ids);
        Ok(())
    }

    fn capacity(&self, region: Region) -> Result<Capacity, Self::Error> {
        self.admit_write(&region).map_err(FleetError::CrossRegion)?;
        Ok(Capacity {
            provisioned: self.provisioned,
            available: self.region_capacity.saturating_sub(self.provisioned),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetEvent {
    Registered,
    Attested,
    Degraded,
    Offline,
}

impl FleetEvent {
    pub fn token(self) -> &'static str {
        use myelin_ci_sandbox::events::{
            CI_RUNNER_ATTESTED, CI_RUNNER_DEGRADED, CI_RUNNER_OFFLINE, CI_RUNNER_REGISTERED,
        };
        match self {
            FleetEvent::Registered => CI_RUNNER_REGISTERED,
            FleetEvent::Attested => CI_RUNNER_ATTESTED,
            FleetEvent::Degraded => CI_RUNNER_DEGRADED,
            FleetEvent::Offline => CI_RUNNER_OFFLINE,
        }
    }

    pub fn draft(
        self,
        tenant_id: &str,
        runner_id: &str,
        region: &Region,
        pool: &str,
    ) -> EventDraft {
        EventDraft {
            type_: EventType(self.token().to_string()),
            subject: ArtifactRef(format!("myelin://{tenant_id}/ci/runner/{runner_id}")),
            aggregate: AggregateKey(format!("runner:{runner_id}")),
            payload: serde_json::json!({
                "runner_id": runner_id,
                "region": region.as_str(),
                "pool": pool,
                "event": self.token(),
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::validate_event_type;

    fn fr_par() -> Region {
        Region::new("fr-par")
    }
    fn eu_north() -> Region {
        Region::new("eu-north")
    }
    fn linux_class() -> RunnerClass {
        RunnerClass("linux-x64".into())
    }

    #[test]
    fn fleet_provider_provision_capacity_deprovision_round_trip() {
        let mut fleet = EuFleetProvider::new(GenericEuIaasAdapter, "01J0ACME", fr_par(), 100);
        assert_eq!(fleet.adapter_name(), "generic-eu-iaas");

        let hosts = fleet
            .provision(linux_class(), 3, fr_par())
            .expect("an in-region provision succeeds");
        assert_eq!(hosts.len(), 3, "provisioned exactly n hosts");
        for h in &hosts {
            assert_eq!(
                &h.region,
                &fr_par(),
                "every host is region-pinned to the cell"
            );
            assert!(h.host_id.starts_with("geniaas-fr-par-"), "the adapter id");
        }

        fleet.set_provisioned(3);
        let cap = fleet.capacity(fr_par()).expect("in-region capacity");
        assert_eq!(cap.provisioned, 3);
        assert_eq!(cap.available, 97, "available = ceiling - provisioned");

        fleet.deprovision(&hosts).expect("deprovision succeeds");
        fleet
            .deprovision(&hosts)
            .expect("deprovision is idempotent");
    }

    #[test]
    fn the_bare_metal_pxe_adapter_satisfies_the_same_trait() {
        let fleet = EuFleetProvider::new(BareMetalPxeAdapter, "01J0ACME", fr_par(), 50);
        assert_eq!(fleet.adapter_name(), "bare-metal-pxe");
        let hosts = fleet
            .provision(linux_class(), 2, fr_par())
            .expect("provision");
        assert!(
            hosts[0].host_id.starts_with("pxe-fr-par-"),
            "the PXE adapter id"
        );
    }

    #[test]
    fn a_region_a_pool_refuses_to_provision_in_region_b() {
        let fleet = EuFleetProvider::new(GenericEuIaasAdapter, "01J0ACME", fr_par(), 100);
        let err = fleet
            .provision(linux_class(), 1, eu_north())
            .expect_err("a cross-region provision MUST be refused (no global pool)");
        assert_eq!(
            err,
            FleetError::CrossRegion(CrossRegionRunnerWrite {
                tenant_id: "01J0ACME".into(),
                cell_region: fr_par(),
                row_region: eu_north(),
            })
        );
        assert!(
            err.to_string().contains("the pin is the cell's"),
            "loud reason: {err}"
        );
        let ok = fleet
            .provision(linux_class(), 1, fr_par())
            .expect("the in-region provision still succeeds");
        assert_eq!(ok.len(), 1, "the cell-region provision is admitted");
    }

    #[test]
    fn the_residency_pin_red_green_fixture() {
        let mut pin = RunnerWritePin::for_cell("01J0ACME", fr_par());

        pin.admit_runner_write(&fr_par())
            .expect("an in-region runner write is admitted");

        let err = pin
            .admit_runner_write(&eu_north())
            .expect_err("a cross-region runner write is refused");
        assert_eq!(err.cell_region, fr_par());
        assert_eq!(err.row_region, eu_north());

        assert_eq!(
            pin.cross_region_runner_rows_admitted(),
            0,
            "0 cross-region runner rows admitted - the no-global-pool green artifact"
        );
    }

    #[test]
    fn the_pools_are_partitioned_per_residency_zone() {
        let mut pools = FleetPools::new();
        let fr = PoolKey::new(fr_par(), linux_class());
        let eu = PoolKey::new(eu_north(), linux_class());
        pools.set_current(fr.clone(), 4);
        pools.set_current(eu.clone(), 2);

        assert_eq!(pools.current(&fr), 4);
        assert_eq!(pools.current(&eu), 2);
        assert_ne!(fr, eu, "a region-A pool key is distinct from region-B");
        assert_eq!(
            pools.distinct_regions(),
            2,
            "the fleet has pools in two distinct residency zones - not one global pool"
        );

        pools.set_current(eu.clone(), 0);
        assert_eq!(pools.current(&eu), 0);
        assert_eq!(
            pools.distinct_regions(),
            1,
            "the eu pool scaled to zero - pruned"
        );
    }

    #[test]
    fn autoscale_tracks_queue_depth_and_scales_to_zero_at_idle() {
        let policy =
            AutoscalePolicy::new( 2,  0,  20);

        assert_eq!(policy.target(0, 0), 0, "idle → 0 (scale-to-zero)");

        assert_eq!(policy.target(5, 0), 7, "5 queued + 2 warm buffer");
        assert_eq!(
            policy.target(5, 3),
            10,
            "5 queued + 3 in-flight + 2 warm buffer"
        );

        assert_eq!(policy.target(100, 0), 20, "capped at max=20");

        let warm = AutoscalePolicy::new(1,  2, 20);
        assert_eq!(warm.target(0, 0), 2, "idle but min_warm=2 keeps 2 hot");
    }

    #[test]
    fn prewarm_buffer_is_sized_from_the_measured_arrival_rate() {
        let ci_surge = myelin_substrate::thresholds::CiSurge::default();

        let busy =
            AutoscalePolicy::from_measured_arrival_rate(&ci_surge,  100, 0, 200);
        assert_eq!(
            busy.warm_buffer, 10,
            "10% of a 100-arrival rate = a 10-VM warm buffer"
        );
        assert_eq!(
            busy.target(5, 0),
            15,
            "5 queued + 10 warm (sized to the arrival rate)"
        );

        let idle =
            AutoscalePolicy::from_measured_arrival_rate(&ci_surge,  0, 0, 200);
        assert_eq!(
            idle.warm_buffer, 0,
            "an idle pool pre-warms nothing (scale-to-zero ready)"
        );

        let burst = AutoscalePolicy::from_measured_arrival_rate(
            &ci_surge,  100_000, 0, 200,
        );
        assert_eq!(
            burst.warm_buffer, 16,
            "the warm buffer is clamped at the per-VM-memory ceiling"
        );
    }

    #[test]
    fn autoscaler_reconcile_yields_the_scale_plan() {
        let auto = Autoscaler::new(AutoscalePolicy::new(1, 0, 50));
        let key = PoolKey::new(fr_par(), linux_class());

        let up = auto.reconcile(
            key.clone(),
             2,
             9,
             0,
        );
        assert_eq!(up.desired, 10, "9 queued + 1 warm buffer");
        assert_eq!(up.delta(), 8, "provision 8 more");
        assert_eq!(up.provision_count(), 8);
        assert_eq!(up.deprovision_count(), 0);
        assert!(!up.is_scale_to_zero());

        let down = auto.reconcile(
            key.clone(),
             10,
             0,
             0,
        );
        assert_eq!(down.desired, 0, "idle → scale-to-zero");
        assert_eq!(down.delta(), -10, "deprovision all 10");
        assert_eq!(down.deprovision_count(), 10);
        assert!(down.is_scale_to_zero(), "idle drains to zero");

        let steady = auto.reconcile(
            key,  6,  5,  0,
        );
        assert_eq!(steady.desired, 6, "5 queued + 1 warm = 6 == current");
        assert_eq!(steady.delta(), 0, "steady state is a no-op");
    }

    #[test]
    fn applying_a_scale_plan_provisions_region_pinned_hosts() {
        let mut fleet = EuFleetProvider::new(GenericEuIaasAdapter, "01J0ACME", fr_par(), 100);
        let auto = Autoscaler::new(AutoscalePolicy::new(1, 0, 50));

        let plan = auto.reconcile(
            PoolKey::new(fr_par(), linux_class()),
             0,
             4,
             0,
        );
        let hosts = fleet.apply(&plan).expect("apply the in-region plan");
        assert_eq!(hosts.len(), 5, "provisioned 5 hosts (4 queued + 1 warm)");
        for h in &hosts {
            assert_eq!(&h.region, &fr_par(), "region-pinned");
        }
        assert_eq!(fleet.capacity(fr_par()).unwrap().provisioned, 5);

        let cross = auto.reconcile(PoolKey::new(eu_north(), linux_class()), 0, 4, 0);
        assert!(
            matches!(fleet.apply(&cross), Err(FleetError::CrossRegion(_))),
            "a cross-region plan is refused"
        );
    }

    #[test]
    fn the_fleet_reports_its_region_into_residency_verify() {
        let fleet = EuFleetProvider::new(GenericEuIaasAdapter, "01J0ACME", fr_par(), 100);
        let report = fleet.residency_report();
        assert_eq!(report.tenant_id, "01J0ACME");
        assert_eq!(report.region, fr_par());
        assert!(
            report.matches_region_of_record(&fr_par()),
            "in-region: green"
        );
        assert!(
            !report.matches_region_of_record(&eu_north()),
            "a mismatch is the residency breach"
        );
    }

    #[test]
    fn the_fleet_events_build_grammatical_pii_free_drafts() {
        let cases = [
            (FleetEvent::Registered, "ci.runner.registered"),
            (FleetEvent::Attested, "ci.runner.attested"),
            (FleetEvent::Degraded, "ci.runner.degraded"),
            (FleetEvent::Offline, "ci.runner.offline"),
        ];
        for (ev, token) in cases {
            assert_eq!(ev.token(), token, "the frozen token");
            validate_event_type(token).expect("a grammatical ci.runner.* token");

            let draft = ev.draft("01J0ACME", "01J0RUNNER", &fr_par(), "linux-x64");
            assert_eq!(draft.type_.0, token);
            assert_eq!(
                draft.subject.0, "myelin://01J0ACME/ci/runner/01J0RUNNER",
                "the runner subject ArtifactRef"
            );
            assert_eq!(
                draft.aggregate.0, "runner:01J0RUNNER",
                "per-runner ordering"
            );
            assert!(
                !draft.contains_personal_data,
                "a fleet event is PII-free (opaque ids + region/pool tokens)"
            );
            assert!(draft.pii_key_ref.is_none());
            assert_eq!(draft.payload["region"], "fr-par");
            assert_eq!(draft.payload["pool"], "linux-x64");
        }
    }

    #[test]
    fn the_fleet_write_sql_is_region_pinned() {
        assert!(
            INSERT_RUNNER_QUERY.contains("region"),
            "the runner insert carries the region column"
        );
        assert!(
            COUNT_RUNNERS_BY_POOL_QUERY.contains("region = $2"),
            "the autoscale count filters on region (per residency zone, not a global count)"
        );
        assert!(
            DELETE_RUNNER_QUERY.contains("tenant_id = $1 AND runner_id = $2"),
            "deprovision is PK-scoped (never crosses a tenant/region)"
        );
    }
}
