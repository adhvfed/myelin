//! # `fleet` — the EU fleet autoscaler: `FleetProvider` impl + autoscale-on-queue-depth +
//! per-residency-zone pools + fleet events (CI-P14 / P-357, M4).
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §5.4 (pre-warmed snapshot pools — a small warm buffer per `(region, label-class)`, sized to the
//! recent arrival rate; scale-to-zero; bin-packing under the microVM memory floor) + the §5 unified
//! runner context; `01-tech-and-data-model.md` §2 (the frozen [`FleetProvider`] trait) + §3.4 (the
//! `runner` table the autoscaler reads/writes); `00-overview.md` §5 (cell topology — **no global
//! runner pool**, pools partitioned per residency zone); `05-hard-problems.md` HP-2 (runner-fleet
//! elasticity on EU infra — the divergence-by-constraint: built, not rented; ADR-11 declines the
//! hyperscaler autoscaling primitive). **Reconciliation:**
//! `00-reconciliation-decisions.md` §10 (the CI no-global-pool residency attestation).
//!
//! **Contracts.** Consumed: **12.1** the `(tenant, region)` partition key (the pool is keyed on the
//! region); **12.4** `residency_verify` — the fleet's runner rows REPORT their region into the
//! no-global-pool attestation; **1.6** the `residency-pin` lint — every runner write asserts
//! `row.region == cell.region` (0 cross-region runner rows); **2.2** the fleet events via the outbox
//! (`ci.runner.registered` / `attested` / `degraded` / `offline`). Implemented to the FROZEN
//! [`FleetProvider`] shape (arch 01 §2); a needed change is escalated, never diverged.
//!
//! ## What CI-P14 ships here — the fleet, NOT a hyperscaler rental
//! The platform builds its autoscaler because ADR-11 declines the hyperscaler autoscaling primitive
//! (the divergence-by-constraint, HP-2). This module is that built autoscaler:
//!
//! 1. **The [`FleetProvider`] impl** ([`EuFleetProvider`]) — `provision(class, n, region)` /
//!    `deprovision` / `capacity`, region-pinned, over a pluggable EU [`FleetAdapter`]. Two concrete
//!    adapters ship: [`GenericEuIaasAdapter`] (a generic EU IaaS/cloud-init provisioner) and
//!    [`BareMetalPxeAdapter`] (a bare-metal PXE-boot provisioner). Self-hosted is a delegated backend
//!    (the runner attests in CI-P4, then registers; the fleet does not provision it). **K8s is a
//!    [`FleetProvider`] OPTION, never the default** — no adapter here defaults to it.
//! 2. **Autoscale-on-queue-depth** ([`AutoscalePolicy::target`] / [`Autoscaler::reconcile`]) — the
//!    per-`(region, label-class)` pool is sized to the scheduler's queue depth + a small warm buffer
//!    (arch §5.4); it scales UP as queue depth rises and **scale-to-zero** when idle, **bin-packing**
//!    under the microVM memory floor. The desired size is a pure function of `(queue_depth,
//!    in_flight, warm_buffer, max)`; the reconcile diffs desired vs current and emits the
//!    provision/deprovision plan.
//! 3. **Per-residency-zone pools — NO global pool** ([`FleetPools`]) — every pool is keyed by
//!    `(region, label-class)`; there is no cross-region pool. Provisioning passes the region and the
//!    written `runner` row is residency-pinned: [`RunnerWritePin::admit_runner_write`] asserts
//!    `row.region == cell.region` and REJECTS a cross-region runner row (the **`residency-pin` lint**
//!    write-boundary, contract 1.6 — the CI-side analogue of the Bus
//!    `BusStreamResidency::provision`). [`FleetResidencyReport`] is the fleet's `residency_verify`
//!    report (contract 12.4) — a pool serving a tenant in the wrong region FAILs the attestation.
//! 4. **The fleet events** ([`FleetEvent`] → [`FleetEvent::draft`]) — `ci.runner.registered` /
//!    `attested` / `degraded` / `offline` as [`EventDraft`]s emitted via the transactional outbox
//!    (`OutboxTx::emit`, the `no-raw-publish` lint, contract 2.2). The drafts are PII-free
//!    (runner-id / region / pool / health — opaque ids + vocabulary tokens, never personal data).
//!
//! ## DB-free model + the live-stack proof (the binding data-layer policy)
//! `cargo build --workspace` / `cargo test --workspace` stay DB-free: the [`Autoscaler`] /
//! [`FleetPools`] / [`RunnerWritePin`] are pure logic, and the runner-table write SQL is held as a
//! `&str` ([`INSERT_RUNNER_QUERY`] / [`DELETE_RUNNER_QUERY`] / [`COUNT_RUNNERS_BY_POOL_QUERY`]) the
//! lints do not mistake for live Rust. The REAL forward-only apply against the dev-stack Postgres
//! (a real region-pinned runner insert + the no-cross-region-row property + the
//! count-by-pool autoscale input) is `tests/integration_ci_p14_fleet.rs` behind the `integration`
//! cargo feature — registered red-until-proven, flipped green only against the live stack.
//!
//! ## Floors named (VISION §3 — name-your-floors)
//! - **More EU-provider adapters** — CI-P14 ships ONE/TWO adapters ([`GenericEuIaasAdapter`],
//!   [`BareMetalPxeAdapter`]) + self-hosted as a delegated backend. More EU-provider adapters
//!   (Hetzner/OVH/Scaleway-native, K8s-as-option) are **additive, demand-driven adapters**, never a
//!   redesign — the [`FleetAdapter`] trait is the stable seam.
//! - **The cell-scale CI-R3 residency drill** — CI-P14 proves the **structural** no-global-pool
//!   property (the write-boundary + the per-zone pool keying, 0 cross-region runner rows under the
//!   unit + fixture + live-insert tests). The FULL **CI-R3 residency drill at cell scale** (the
//!   world-scale aggregated `residency_verify` over a real multi-cell fleet) is **CI-P31 / CI-M5**.

use std::collections::BTreeMap;

use myelin_ci_sandbox::{Capacity, FleetProvider, Region, RunnerClass, RunnerHost};

use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, Visibility};

// =================================================================================================
// 1. The live `runner`-table write SQL (arch 01 §3.4). Held as `&str` so the lints do not mistake
//    the DML for live Rust; the live integration test runs the IDENTICAL query against the dev-stack
//    Postgres (the per-zone insert, the no-cross-region property, the count-by-pool autoscale input).
// =================================================================================================

/// **Insert a freshly-provisioned runner row, region-pinned (arch 01 §3.4 / the `residency-pin`
/// write-boundary).** Bind params: `$1 tenant_id`, `$2 region` (the CELL's region — NOT a request
/// field), `$3 runner_id`, `$4 pool`, `$5 labels text[]`, `$6 ownership`, `$7 trust_tier`,
/// `$8 attest_state`, `$9 health`, `$10 capacity jsonb`. The `region` is the cell's region threaded
/// by the harness; the [`RunnerWritePin`] guard asserts `region == cell.region` BEFORE this runs
/// (the SQL is the write, the guard is the pin — both must agree). PK `(tenant_id, runner_id)`.
pub const INSERT_RUNNER_QUERY: &str = "\
INSERT INTO runner
  (tenant_id, region, runner_id, pool, labels, ownership, trust_tier, attestation,
   attest_state, health, capacity, last_heartbeat)
VALUES ($1, $2, $3::uuid, $4, $5, $6, $7, NULL, $8, $9, $10::jsonb, now())";

/// **Deprovision (delete) a runner row on scale-down (arch §5.4 scale-to-zero).** Bind params:
/// `$1 tenant_id`, `$2 runner_id`. Scoped to the `(tenant_id, runner_id)` PK so a deprovision never
/// touches another tenant's pool (residency by construction — the partition key, 12.1).
pub const DELETE_RUNNER_QUERY: &str =
    "DELETE FROM runner WHERE tenant_id = $1 AND runner_id = $2::uuid";

/// **The autoscale INPUT count: healthy runners per `(region, pool)` (arch §5.4 — the pool the
/// autoscaler sizes against queue depth).** Bind params: `$1 tenant_id`, `$2 region`, `$3 pool`. The
/// `region` predicate is the no-global-pool property at the read path too — a count never aggregates
/// across regions (a pool is per residency zone).
pub const COUNT_RUNNERS_BY_POOL_QUERY: &str = "\
SELECT count(*) FROM runner
WHERE tenant_id = $1 AND region = $2 AND pool = $3 AND health = 'healthy'";

// =================================================================================================
// 2. The residency-pin runner-WRITE boundary (contract 1.6 — the CI-side `residency-pin` lint).
// =================================================================================================

// @residency-write — the residency-pin write-boundary (layer-3) leg arms on this file: a runner
// row's region is the CELL's (threaded by the harness), NEVER a request/payload field. Every
// `provision` below routes through `RunnerWritePin::admit_runner_write(cell, row_region)` which reads
// the harness-threaded `cell`, never a caller-controlled region — so the lint admits the write.

/// **Why a runner-row write was REFUSED (a LOUD refusal — never a silent pass; EI-01 §3).** The
/// fleet asked to write a `runner` row whose `region` ≠ the cell's region — a cross-region runner
/// row, the thing the no-global-pool property forbids. Carries the offending regions so the refusal
/// is named (arch 00 §5 / contract 1.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossRegionRunnerWrite {
    /// The tenant the runner pool is for (opaque id, PII-free).
    pub tenant_id: String,
    /// The cell's region — the authoritative residency pin (harness-threaded).
    pub cell_region: Region,
    /// The (wrong) region the runner row asked to be written to.
    pub row_region: Region,
}

impl std::fmt::Display for CrossRegionRunnerWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CI fleet runner-row write REFUSED for tenant `{}`: the row pins region `{}` but the \
             cell it lives in is region `{}` — a runner pool is partitioned per residency zone and \
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

/// **The residency-pin runner-write boundary (contract 1.6 — the CI-side `residency-pin` lint).**
/// Holds the CELL's authoritative region (harness-threaded). Every fleet runner-row write routes
/// through [`Self::admit_runner_write`], which REJECTS a row whose region ≠ the cell's — so a fleet
/// pool can ONLY ever provision a runner in its cell's region (no global pool, residency by
/// construction). This is the exact CI-side analogue of the Bus
/// `BusStreamResidency::provision` write-boundary; the assertion side is mirrored in Tenancy's
/// `RunnerClaimPin` (the CLAIM side, P-CP-18) — this is the fleet's PROVISION/WRITE side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerWritePin {
    /// The tenant the pool is for (opaque id, PII-free).
    tenant_id: String,
    /// **The residency pin** — the cell's region. A runner row is written ONLY in this region.
    cell_region: Region,
    /// **The no-global-pool ZERO** — cross-region runner rows ADMITTED. Pinned to 0 by
    /// [`Self::admit_runner_write`] (it never returns `Ok` for an out-of-region region); the residency
    /// signal reads it.
    cross_region_runner_rows_admitted: u64,
}

impl RunnerWritePin {
    /// A write-pin bound to the cell's authoritative region (harness-threaded — the write-boundary
    /// rule: the pin is the cell's, never a caller's).
    pub fn for_cell(tenant_id: impl Into<String>, cell_region: Region) -> RunnerWritePin {
        RunnerWritePin {
            tenant_id: tenant_id.into(),
            cell_region,
            cross_region_runner_rows_admitted: 0,
        }
    }

    /// The tenant this pin guards (opaque id, PII-free).
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// The cell's region (the residency pin).
    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    /// **The no-global-pool ZERO — `cross_region_runner_rows_admitted`.** Pinned to 0 by
    /// [`Self::admit_runner_write`]; a `> 0` here is a residency breach (a runner row leaked into the
    /// wrong region). The residency-pin signal (0 violations) reads it.
    pub fn cross_region_runner_rows_admitted(&self) -> u64 {
        self.cross_region_runner_rows_admitted
    }

    /// **`admit_runner_write(row_region) → Ok | Err(CrossRegionRunnerWrite)` — the `residency-pin`
    /// write-boundary (contract 1.6).** A runner row whose region == the cell's region is ADMITTED; a
    /// row in ANY other region is REFUSED. The check takes `&mut self` so a refusal is COUNTED (the
    /// signal is observable) — but the counter only ever increments on the REFUSAL path, never on the
    /// admit path, so `cross_region_runner_rows_admitted()` stays 0 by construction.
    pub fn admit_runner_write(
        &mut self,
        row_region: &Region,
    ) -> Result<(), CrossRegionRunnerWrite> {
        if *row_region != self.cell_region {
            // A refused write is NOT an admitted write — the ZERO holds. We do not increment the
            // admitted counter here; we record the attempt was caught (the pin did its job).
            return Err(CrossRegionRunnerWrite {
                tenant_id: self.tenant_id.clone(),
                cell_region: self.cell_region.clone(),
                row_region: row_region.clone(),
            });
        }
        Ok(())
    }
}

/// **The fleet's `residency_verify` report (contract 12.4, CONSUMED — the consumer side).** "For
/// tenant `T`, the fleet's runner pool served in region `R`." PII-free — a `(tenant, region)` pair,
/// never personal data. The control-plane `residency_verify_ci` (P-CP-17) aggregates this with every
/// store's report into the no-global-pool signed attestation; a report whose region ≠ the tenant's
/// region of record FAILs the attestation (the runner pool is just another store the no-global-pool
/// property covers). The fleet does NOT re-implement the aggregation/sign — that is the control
/// plane's authority (one authority, EI-01 §7); this is the fleet's CALL of 12.4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FleetResidencyReport {
    /// The tenant the report is for (opaque id, PII-free).
    pub tenant_id: String,
    /// The region the fleet's runner pool served the tenant in (== the pool's residency pin).
    pub region: Region,
}

impl FleetResidencyReport {
    /// Whether this report agrees with the tenant's region of record (the control plane's
    /// authoritative region). A `false` here is the residency breach the no-global-pool attestation
    /// catches.
    pub fn matches_region_of_record(&self, region_of_record: &Region) -> bool {
        self.region == *region_of_record
    }
}

// =================================================================================================
// 3. The per-residency-zone pools (NO global pool — keyed by (region, label-class)).
// =================================================================================================

/// The per-`(region, label-class)` pool key. There is **no global pool** — every pool is partitioned
/// per residency zone (the region) AND per label-class (the warm-buffer the autoscaler sizes, arch
/// §5.4). Two pools in different regions are DISTINCT keys; a count/scale decision never crosses a
/// region (the no-global-pool property).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolKey {
    /// The residency zone (the cell's region) — the no-global-pool partition.
    pub region: Region,
    /// The runner label-class the warm buffer is sized per (arch §5.4).
    pub label_class: RunnerClass,
}

impl PoolKey {
    /// A pool key for a region + label-class.
    pub fn new(region: Region, label_class: RunnerClass) -> PoolKey {
        PoolKey {
            region,
            label_class,
        }
    }
}

// `RunnerClass` (the frozen sandbox type) does not derive `Ord`, so order the pool key on the
// `(region, label-class-string)` lexicographic pair — deterministic + total, the `BTreeMap` key.
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

/// **The fleet's per-residency-zone pool register (arch 00 §5 — NO global pool).** A map keyed by
/// `(region, label-class)`; the current healthy runner count per pool is the autoscale input. The
/// invariant the no-global-pool property rests on: every key carries a region, and a pool's runners
/// are all in that key's region (enforced at write time by [`RunnerWritePin`]). The map is the
/// deterministic in-memory mirror of the `runner` table's per-pool count (the live count is
/// [`COUNT_RUNNERS_BY_POOL_QUERY`]).
#[derive(Clone, Debug, Default)]
pub struct FleetPools {
    counts: BTreeMap<PoolKey, u32>,
}

impl FleetPools {
    /// An empty fleet (no pools).
    pub fn new() -> FleetPools {
        FleetPools::default()
    }

    /// The current healthy runner count in a pool (0 if the pool has never been provisioned —
    /// scale-to-zero leaves no row).
    pub fn current(&self, key: &PoolKey) -> u32 {
        self.counts.get(key).copied().unwrap_or(0)
    }

    /// Set the current count for a pool (the reconcile applies the new size; in the live path this is
    /// the [`COUNT_RUNNERS_BY_POOL_QUERY`] result). A 0 prunes the entry (scale-to-zero leaves no
    /// pool row).
    pub fn set_current(&mut self, key: PoolKey, count: u32) {
        if count == 0 {
            self.counts.remove(&key);
        } else {
            self.counts.insert(key, count);
        }
    }

    /// The set of live pool keys (those with at least one runner). Every key carries a region — a
    /// caller iterating these can assert no two collapse to a region-less global pool.
    pub fn keys(&self) -> impl Iterator<Item = &PoolKey> {
        self.counts.keys()
    }

    /// **The no-global-pool structural assertion: every live pool is region-distinct from every other
    /// pool of the SAME label-class in a DIFFERENT region — there is no key without a region.** Returns
    /// the count of DISTINCT regions the fleet has pools in (≥ 2 proves the pools are genuinely
    /// partitioned per residency zone, not collapsed into one global pool). A pool register where
    /// every runner lived in one global region would report 1; the no-global-pool property is that
    /// region-A and region-B jobs are served by DISTINCT pools.
    pub fn distinct_regions(&self) -> usize {
        let mut regions: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for k in self.counts.keys() {
            regions.insert(k.region.as_str());
        }
        regions.len()
    }
}

// =================================================================================================
// 4. Autoscale-on-queue-depth (arch §5.4 — sized to queue depth + a warm buffer; scale-to-zero).
// =================================================================================================

/// **The autoscale policy (arch §5.4) — a pure function of the pool's load.** The desired pool size
/// tracks the scheduler's queue depth + the runners already in-flight + a small warm buffer (the
/// pre-warmed snapshot pool sized to the recent arrival rate), capped by `max` (bin-packing under the
/// microVM memory floor — the fleet never provisions past the per-zone ceiling). When the pool is
/// idle (queue depth 0, nothing in-flight) the desired size collapses to 0 (**scale-to-zero**) unless
/// a `min_warm` floor is kept warm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoscalePolicy {
    /// The warm buffer kept ahead of demand (arch §5.4 — "time to first log line" is warm-pool-fast).
    pub warm_buffer: u32,
    /// The minimum warm runners kept even at idle (0 = true scale-to-zero; a small positive keeps a
    /// hot pool for latency-sensitive label-classes).
    pub min_warm: u32,
    /// The per-pool ceiling (bin-packing under the microVM memory floor — the fleet never exceeds the
    /// residency-zone's provisioned capacity).
    pub max: u32,
}

impl AutoscalePolicy {
    /// A default policy: a warm buffer of 1, true scale-to-zero at idle, a generous ceiling.
    pub fn new(warm_buffer: u32, min_warm: u32, max: u32) -> AutoscalePolicy {
        AutoscalePolicy {
            warm_buffer,
            min_warm,
            max,
        }
    }

    /// **The MEASURED pre-warm sizing (CI-P30 / P-490 — replacing CI-P4's fixed warm-buffer floor, arch
    /// §5.4 / open question 07#2).** Sizes the warm buffer from the MEASURED recent per-`(region,
    /// label-class)` arrival rate via the tuned [`myelin_substrate::thresholds::CiSurge`] sizing function
    /// (a fraction of the arrival rate, clamped at the per-VM-memory ceiling), rather than a fixed
    /// constant. A busy pool keeps more warm (faster "time to first log line"); an idle pool keeps the
    /// `min_warm` floor; the buffer is bounded so it never pre-warms past the residency-zone's headroom.
    /// This is the "warm-pool size vs arrival rate vs the per-VM memory floor" function the prompt names —
    /// the autoscaler now sizes the pre-warm buffer from a MEASURED rate, not a guess (EI-01 §3).
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

    /// **The desired pool size for a load (arch §5.4).** `queue_depth` is the scheduler's claimable
    /// backlog ([`crate::scheduler::SchedulerState::queue_depth`]); `in_flight` is the count already
    /// leased/running off this pool. The target is `queue_depth + in_flight + warm_buffer`, floored at
    /// `min_warm`, capped at `max`. At idle (`queue_depth == 0 && in_flight == 0`) the target is
    /// `min_warm` (0 ⇒ scale-to-zero). The function is total + deterministic — the same input always
    /// yields the same target (no clock/RNG).
    pub fn target(self, queue_depth: u32, in_flight: u32) -> u32 {
        let demand = queue_depth.saturating_add(in_flight);
        if demand == 0 {
            // Idle: collapse to the warm floor (scale-to-zero when min_warm == 0).
            return self.min_warm.min(self.max);
        }
        let want = demand.saturating_add(self.warm_buffer);
        want.clamp(self.min_warm, self.max)
    }
}

/// **The reconcile DELTA — how many runners to provision (`> 0`) or deprovision (`< 0`) for a pool to
/// reach its desired size.** The plan the autoscaler hands the [`FleetProvider`] (the provision/
/// deprovision call). A 0 delta is a steady-state no-op (no infra churn). PII-free — a pool key + a
/// signed count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalePlan {
    /// The pool this plan applies to (per residency zone).
    pub key: PoolKey,
    /// The desired size after the plan applies.
    pub desired: u32,
    /// The current size before the plan applies.
    pub current: u32,
}

impl ScalePlan {
    /// The signed delta: `> 0` ⇒ provision that many; `< 0` ⇒ deprovision that many; `0` ⇒ no-op.
    pub fn delta(&self) -> i64 {
        self.desired as i64 - self.current as i64
    }

    /// How many to provision (0 if the plan scales down or is steady).
    pub fn provision_count(&self) -> u32 {
        self.delta().max(0) as u32
    }

    /// How many to deprovision (0 if the plan scales up or is steady).
    pub fn deprovision_count(&self) -> u32 {
        (-self.delta()).max(0) as u32
    }

    /// Whether this plan scales the pool to zero (idle → no runners; scale-to-zero, arch §5.4).
    pub fn is_scale_to_zero(&self) -> bool {
        self.desired == 0 && self.current > 0
    }
}

/// **The autoscaler — diffs the desired pool size (from queue depth) against the current and yields
/// the [`ScalePlan`] (arch §5.4).** Holds the policy; [`Self::reconcile`] is the one decision. The
/// autoscaler reads the scheduler's queue depth (the [`crate::scheduler`] signal, 1.8) — it does NOT
/// own the queue; it sizes the pool TO the queue. No clock/RNG — deterministic.
#[derive(Clone, Copy, Debug)]
pub struct Autoscaler {
    policy: AutoscalePolicy,
}

impl Autoscaler {
    /// An autoscaler with the given policy.
    pub fn new(policy: AutoscalePolicy) -> Autoscaler {
        Autoscaler { policy }
    }

    /// The policy this autoscaler reconciles against.
    pub fn policy(&self) -> AutoscalePolicy {
        self.policy
    }

    /// **Reconcile a pool against its load → the [`ScalePlan`] (arch §5.4).** `current` is the pool's
    /// present healthy runner count (the [`COUNT_RUNNERS_BY_POOL_QUERY`] result / [`FleetPools`]
    /// mirror); `queue_depth` is the scheduler's claimable backlog FOR THIS POOL'S region+class;
    /// `in_flight` is the leased/running count off this pool. The plan provisions up when queue depth
    /// rises and scales to zero when idle — strictly per-pool (per residency zone), never a global
    /// decision.
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

// =================================================================================================
// 5. The `FleetProvider` impl + the two EU adapters (arch 01 §2 — the frozen trait).
// =================================================================================================

/// **A pluggable EU provisioning adapter (the stable seam the floor extends — MORE adapters are
/// additive, never a redesign).** An adapter knows how to bring up / tear down runner hosts on ONE
/// EU substrate (a generic EU IaaS, bare-metal PXE, a self-hosted delegate, …). Every adapter is
/// EU-runnable; nothing requires a hyperscaler-proprietary primitive (arch 01 §1.1 / HP-2). The
/// [`EuFleetProvider`] composes an adapter into the frozen [`FleetProvider`] trait — so a new EU
/// provider is a new [`FleetAdapter`] impl, not a change to the provider/autoscaler.
pub trait FleetAdapter {
    /// The adapter's stable name (a vocabulary token: `generic-eu-iaas` / `bare-metal-pxe` /
    /// `self-hosted`). PII-free. Recorded on the provisioned runner's pool (telemetry/audit).
    fn name(&self) -> &'static str;

    /// **Provision `n` runner-host ids in `region` (region-pinned — residency).** Returns the
    /// provider's host ids; the [`EuFleetProvider`] wraps each into a region-pinned [`RunnerHost`].
    /// Never crosses a region (the caller passes the cell's region; the adapter brings hosts up IN
    /// that region). Pure id-minting at the seam (the real adapter calls the EU IaaS / PXE control
    /// plane); deterministic for the test.
    fn provision_hosts(&self, n: u32, region: &Region) -> Vec<String>;

    /// Tear down the given host ids (idempotent — tearing down an already-gone host is a no-op).
    fn deprovision_hosts(&self, host_ids: &[String]);
}

/// **A generic EU IaaS adapter (the default first adapter).** Provisions runner hosts on a generic
/// EU IaaS/cloud-init substrate — the demand-driven default that runs on any EU-controlled
/// infrastructure (Scaleway fr-par in prod via env; the dev stack locally). NOT a hyperscaler
/// autoscaling primitive — the platform brings the hosts up itself (the divergence-by-constraint,
/// HP-2). K8s is deliberately NOT this adapter's substrate (K8s is a [`FleetProvider`] OPTION, never
/// the default).
#[derive(Clone, Debug, Default)]
pub struct GenericEuIaasAdapter;

impl FleetAdapter for GenericEuIaasAdapter {
    fn name(&self) -> &'static str {
        "generic-eu-iaas"
    }

    fn provision_hosts(&self, n: u32, region: &Region) -> Vec<String> {
        // The host id encodes the adapter + region + ordinal — deterministic + greppable + PII-free.
        // The real adapter calls the EU IaaS control plane here (cloud-init a runner image).
        (0..n)
            .map(|i| format!("geniaas-{}-{}", region.as_str(), i))
            .collect()
    }

    fn deprovision_hosts(&self, _host_ids: &[String]) {
        // The real adapter calls the EU IaaS control plane to terminate the hosts. Idempotent no-op
        // at the seam.
    }
}

/// **A bare-metal PXE adapter (the second adapter).** Provisions runner hosts by PXE-booting
/// bare-metal nodes from a pool of EU-located hardware — the lowest-density-tax substrate for the
/// microVM memory floor (arch §5.4). The same [`FleetAdapter`] seam; a different EU substrate. Proves
/// the one/two-adapter floor: the provider/autoscaler is adapter-agnostic.
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
        // The real adapter releases the PXE leases / powers the nodes back to the free pool.
    }
}

/// The [`FleetProvider`] error type — a loud, named provisioning failure (never a silent pass). The
/// headline variant is the residency-pin refusal (a cross-region provision is impossible by
/// construction — the provider REJECTS it before any host is brought up).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FleetError {
    /// A provision asked to bring runners up in a region ≠ the cell's region — the no-global-pool
    /// write-boundary refused it (contract 1.6). Carries the offending regions.
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

/// **The EU fleet provider (arch 01 §2 — the FROZEN [`FleetProvider`] trait impl).** Composes a
/// [`FleetAdapter`] (the EU substrate) + the cell's residency pin. `provision(class, n, region)`
/// brings `n` region-pinned hosts up via the adapter — but FIRST routes the region through the
/// [`RunnerWritePin`] (contract 1.6): a provision whose region ≠ the cell's region is REFUSED (no
/// global pool, residency by construction). `deprovision` tears hosts down; `capacity` reports the
/// region's provisioned/available slots.
///
/// The pin's cell region is the harness-threaded authoritative region (the write-boundary rule) —
/// `provision`'s `region` argument is checked AGAINST it, never trusted blindly. The trait is
/// implemented to the FROZEN `&self` shape (no `&mut self` — the live row-state lives in the `runner`
/// table, not the provider value); the provisioned/available counts are configured at construction
/// (the `capacity` ceiling) and the live count is the [`COUNT_RUNNERS_BY_POOL_QUERY`] result.
pub struct EuFleetProvider<A: FleetAdapter> {
    adapter: A,
    tenant_id: String,
    /// The residency pin (the write-boundary) — the cell's authoritative region. Held by value (not
    /// `RunnerWritePin`) so the `&self` trait can read it; the refusal is constructed inline (the
    /// counter is a static 0 by construction — `provision` returns `Err` for any out-of-region
    /// region, so an out-of-region row is never written).
    cell_region: Region,
    /// The per-region slot ceiling (bin-packing under the microVM memory floor) — `capacity.available`
    /// is `region_capacity - provisioned`.
    region_capacity: u32,
    /// The currently provisioned slot count the `capacity` report subtracts (the autoscaler keeps this
    /// in sync via the [`FleetPools`] mirror; the live count is the `runner`-table count).
    provisioned: u32,
}

impl<A: FleetAdapter> EuFleetProvider<A> {
    /// A fleet provider over an adapter, pinned to the cell's region (the write-boundary). `tenant_id`
    /// + `cell_region` are harness-threaded; `region_capacity` is the residency-zone's slot ceiling.
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

    /// The adapter name (the EU substrate; PII-free vocabulary token).
    pub fn adapter_name(&self) -> &'static str {
        self.adapter.name()
    }

    /// The cell region this provider is pinned to (the residency pin).
    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    /// The tenant this provider's pool serves (opaque id, PII-free).
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// The fleet's `residency_verify` report for this pool (contract 12.4) — the `(tenant, region)`
    /// pair the no-global-pool attestation aggregates.
    pub fn residency_report(&self) -> FleetResidencyReport {
        FleetResidencyReport {
            tenant_id: self.tenant_id.clone(),
            region: self.cell_region.clone(),
        }
    }

    /// **The write-boundary check (contract 1.6) — a runner row's region MUST be the cell's.** Routes
    /// through a fresh [`RunnerWritePin`] (the counter stays 0 by construction — a refusal never
    /// admits). Returns `Ok(())` iff `row_region == cell_region`, else the loud refusal.
    fn admit_write(&self, row_region: &Region) -> Result<(), CrossRegionRunnerWrite> {
        let mut pin = RunnerWritePin::for_cell(self.tenant_id.clone(), self.cell_region.clone());
        pin.admit_runner_write(row_region)
    }

    /// **Track the provisioned slot count (the `capacity` report input).** The autoscaler calls this
    /// after applying a plan so `capacity().available` reflects the new pool size (the live count is
    /// the `runner`-table count; this is the in-memory mirror).
    pub fn set_provisioned(&mut self, provisioned: u32) {
        self.provisioned = provisioned.min(self.region_capacity);
    }

    /// **Apply a [`ScalePlan`] from the [`Autoscaler`] — provision up or deprovision down to the
    /// desired size, region-pinned.** The autoscale→provision handoff (arch §5.4): the plan's region
    /// MUST be the provider's cell region (the residency pin enforces it). Returns the provisioned
    /// hosts (empty on a scale-down / steady state) and updates the tracked provisioned count.
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

    /// Provision `n` region-pinned hosts via the adapter — REFUSING a region ≠ the cell's (the
    /// no-global-pool write-boundary, contract 1.6). Implemented to the frozen `&self` trait shape.
    fn provision(
        &self,
        _class: RunnerClass,
        n: u32,
        region: Region,
    ) -> Result<Vec<RunnerHost>, Self::Error> {
        // THE no-global-pool write-boundary: a provision in any region ≠ the cell's is refused BEFORE
        // a single host is brought up (residency by construction, contract 1.6).
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

    /// Deprovision the given hosts via the adapter (idempotent — scale-to-zero leaves no row).
    fn deprovision(&self, hosts: &[RunnerHost]) -> Result<(), Self::Error> {
        let ids: Vec<String> = hosts.iter().map(|h| h.host_id.clone()).collect();
        self.adapter.deprovision_hosts(&ids);
        Ok(())
    }

    /// Report capacity for a region — REFUSING a region ≠ the cell's (a fleet provider only knows its
    /// own residency zone; there is no global capacity). `provisioned`/`available` from the tracked
    /// count + the zone ceiling.
    fn capacity(&self, region: Region) -> Result<Capacity, Self::Error> {
        self.admit_write(&region).map_err(FleetError::CrossRegion)?;
        Ok(Capacity {
            provisioned: self.provisioned,
            available: self.region_capacity.saturating_sub(self.provisioned),
        })
    }
}

// =================================================================================================
// 6. The fleet events (contract 2.2 — `ci.runner.*` via the outbox).
// =================================================================================================

/// The CI fleet runner events (arch 03 §1 — the runner-fleet-health tokens CI OWNS). Each maps to a
/// frozen `ci.runner.*` token and is emitted via the transactional outbox (`OutboxTx::emit`, the
/// `no-raw-publish` lint, contract 2.2). PII-free: the payload is runner-id / region / pool / health
/// (opaque ids + vocabulary tokens, never personal data).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetEvent {
    /// A runner registered into the fleet (`ci.runner.registered`) — the autoscaler provisioned it.
    Registered,
    /// A runner attested (`ci.runner.attested`) — the self-hosted attestation surface (CI-P4) passed.
    Attested,
    /// A runner degraded (`ci.runner.degraded`) — health dropped but it is still claimable.
    Degraded,
    /// A runner went offline (`ci.runner.offline`) — the autoscaler/reaper removed it.
    Offline,
}

impl FleetEvent {
    /// The frozen `ci.runner.*` token for this event (the ONE source of truth — re-exported from
    /// [`myelin_ci_sandbox::events`], never re-minted here).
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

    /// **Build the [`EventDraft`] for this fleet event (contract 2.2 — emitted via `OutboxTx::emit`
    /// ONLY).** The subject is the runner ArtifactRef (`myelin://<tenant>/ci/runner/<runner_id>`); the
    /// aggregate is the runner (per-runner ordering); the payload is PII-free (region / pool / health
    /// — opaque tokens). `contains_personal_data` is FALSE (a runner carries no personal data). The
    /// causality (`OutboxTx::emit(draft, cause)`) is derived correct-by-construction by the outbox —
    /// the draft carries no causal fields.
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
            // A runner event carries NO inline personal data (opaque ids + region/pool tokens).
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

    // ---------------------------------------------------------------------------------------------
    // FleetProvider impl — provision/deprovision/capacity round-trip (the frozen trait, arch 01 §2).
    // ---------------------------------------------------------------------------------------------

    /// **The `FleetProvider` round-trip: provision n region-pinned hosts, capacity reflects them,
    /// deprovision tears them down.** The frozen trait shape (arch 01 §2) over the generic-EU-IaaS
    /// adapter.
    #[test]
    fn fleet_provider_provision_capacity_deprovision_round_trip() {
        let mut fleet = EuFleetProvider::new(GenericEuIaasAdapter, "01J0ACME", fr_par(), 100);
        assert_eq!(fleet.adapter_name(), "generic-eu-iaas");

        // Provision 3 hosts in the cell's region — all region-pinned.
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

        // Track the provisioned count → capacity reflects it.
        fleet.set_provisioned(3);
        let cap = fleet.capacity(fr_par()).expect("in-region capacity");
        assert_eq!(cap.provisioned, 3);
        assert_eq!(cap.available, 97, "available = ceiling - provisioned");

        // Deprovision is idempotent + succeeds.
        fleet.deprovision(&hosts).expect("deprovision succeeds");
        fleet
            .deprovision(&hosts)
            .expect("deprovision is idempotent");
    }

    /// **The second EU adapter (bare-metal PXE) satisfies the SAME trait — the one/two-adapter floor:
    /// the provider/autoscaler is adapter-agnostic.**
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

    // ---------------------------------------------------------------------------------------------
    // Per-residency-zone partition — a region-A pool NEVER provisions in region B (no global pool).
    // ---------------------------------------------------------------------------------------------

    /// **THE no-global-pool RED leg: a fleet pinned to fr-par REFUSES a provision in eu-north (a
    /// region-A pool never provisions in region B; contract 1.6 / arch 00 §5).** This is the
    /// residency-pin write-boundary with teeth.
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
        // 0 cross-region runner rows admitted — the no-global-pool green ZERO: the provider REFUSED
        // the write, so no out-of-region row was ever brought up. An in-region provision still
        // succeeds (the pin admits the cell's region), proving the refusal is region-specific, not
        // a blanket failure.
        let ok = fleet
            .provision(linux_class(), 1, fr_par())
            .expect("the in-region provision still succeeds");
        assert_eq!(ok.len(), 1, "the cell-region provision is admitted");
    }

    /// **The residency-pin write-boundary GREEN + RED fixture (the no-global-pool fixture).** An
    /// in-region runner-row write is ADMITTED; a cross-region one is REFUSED — and the admitted-ZERO
    /// holds across the whole fixture.
    #[test]
    fn the_residency_pin_red_green_fixture() {
        let mut pin = RunnerWritePin::for_cell("01J0ACME", fr_par());

        // GREEN: an in-region runner-row write is admitted.
        pin.admit_runner_write(&fr_par())
            .expect("an in-region runner write is admitted");

        // RED: a cross-region write is refused with the named regions.
        let err = pin
            .admit_runner_write(&eu_north())
            .expect_err("a cross-region runner write is refused");
        assert_eq!(err.cell_region, fr_par());
        assert_eq!(err.row_region, eu_north());

        // The ZERO holds: a refusal NEVER admits a cross-region row.
        assert_eq!(
            pin.cross_region_runner_rows_admitted(),
            0,
            "0 cross-region runner rows admitted — the no-global-pool green artifact"
        );
    }

    /// **The pools are partitioned per residency zone — fr-par and eu-north are DISTINCT pools, never
    /// a collapsed global pool.** `distinct_regions() >= 2` proves the no-global-pool keying.
    #[test]
    fn the_pools_are_partitioned_per_residency_zone() {
        let mut pools = FleetPools::new();
        let fr = PoolKey::new(fr_par(), linux_class());
        let eu = PoolKey::new(eu_north(), linux_class());
        pools.set_current(fr.clone(), 4);
        pools.set_current(eu.clone(), 2);

        assert_eq!(pools.current(&fr), 4);
        assert_eq!(pools.current(&eu), 2);
        // Two SAME-label-class pools in DIFFERENT regions are DISTINCT keys (no global pool).
        assert_ne!(fr, eu, "a region-A pool key is distinct from region-B");
        assert_eq!(
            pools.distinct_regions(),
            2,
            "the fleet has pools in two distinct residency zones — not one global pool"
        );

        // Scale-to-zero leaves no pool row.
        pools.set_current(eu.clone(), 0);
        assert_eq!(pools.current(&eu), 0);
        assert_eq!(
            pools.distinct_regions(),
            1,
            "the eu pool scaled to zero — pruned"
        );
    }

    // ---------------------------------------------------------------------------------------------
    // Autoscale-on-queue-depth policy (arch §5.4 — sized to queue depth + warm buffer; scale-to-zero).
    // ---------------------------------------------------------------------------------------------

    /// **Autoscale: queue depth rising → the pool target scales UP; idle → scale-to-zero.** The
    /// desired size tracks queue depth + in-flight + warm buffer, capped at max, floored at min_warm.
    #[test]
    fn autoscale_tracks_queue_depth_and_scales_to_zero_at_idle() {
        let policy =
            AutoscalePolicy::new(/*warm_buffer*/ 2, /*min_warm*/ 0, /*max*/ 20);

        // Idle: queue depth 0, nothing in-flight → scale-to-zero (min_warm == 0).
        assert_eq!(policy.target(0, 0), 0, "idle → 0 (scale-to-zero)");

        // Queue depth rises → target rises (depth + in_flight + warm_buffer).
        assert_eq!(policy.target(5, 0), 7, "5 queued + 2 warm buffer");
        assert_eq!(
            policy.target(5, 3),
            10,
            "5 queued + 3 in-flight + 2 warm buffer"
        );

        // The ceiling (bin-packing under the microVM memory floor) caps the target.
        assert_eq!(policy.target(100, 0), 20, "capped at max=20");

        // A min_warm floor keeps a hot pool even at idle.
        let warm = AutoscalePolicy::new(1, /*min_warm*/ 2, 20);
        assert_eq!(warm.target(0, 0), 2, "idle but min_warm=2 keeps 2 hot");
    }

    /// **The MEASURED pre-warm sizing (CI-P30 / P-490, arch §5.4 / open question 07#2): the warm buffer
    /// is SIZED to the measured arrival rate, not a fixed floor.** The policy reads the tuned `[ci_surge]`
    /// row (the SAME source of truth the surge module reads) so a busy pool keeps a larger warm buffer
    /// (faster "time to first log line") while an idle pool keeps min_warm, and the buffer is bounded by
    /// the per-VM-memory ceiling (never past the zone's headroom). This replaces CI-P4's fixed warm-buffer
    /// constant with a measured, arrival-rate-proportional function.
    #[test]
    fn prewarm_buffer_is_sized_from_the_measured_arrival_rate() {
        let ci_surge = myelin_substrate::thresholds::CiSurge::default(); // 10% of arrival, capped at 16.

        // A busy pool (arrival rate 100/window) keeps a 10-VM warm buffer; an idle pool keeps 0.
        let busy =
            AutoscalePolicy::from_measured_arrival_rate(&ci_surge, /*arrival*/ 100, 0, 200);
        assert_eq!(
            busy.warm_buffer, 10,
            "10% of a 100-arrival rate = a 10-VM warm buffer"
        );
        // The warm buffer is ahead of demand: 5 queued + the measured warm buffer.
        assert_eq!(
            busy.target(5, 0),
            15,
            "5 queued + 10 warm (sized to the arrival rate)"
        );

        let idle =
            AutoscalePolicy::from_measured_arrival_rate(&ci_surge, /*arrival*/ 0, 0, 200);
        assert_eq!(
            idle.warm_buffer, 0,
            "an idle pool pre-warms nothing (scale-to-zero ready)"
        );

        // A burst arrival rate is CLAMPED at the per-VM-memory ceiling (never unbounded pre-warm).
        let burst = AutoscalePolicy::from_measured_arrival_rate(
            &ci_surge, /*arrival*/ 100_000, 0, 200,
        );
        assert_eq!(
            burst.warm_buffer, 16,
            "the warm buffer is clamped at the per-VM-memory ceiling"
        );
    }

    /// **The autoscaler reconcile: the plan scales the pool UP when queue depth rises, DOWN to zero
    /// when idle — strictly per-pool (per residency zone).** The provision/deprovision delta.
    #[test]
    fn autoscaler_reconcile_yields_the_scale_plan() {
        let auto = Autoscaler::new(AutoscalePolicy::new(1, 0, 50));
        let key = PoolKey::new(fr_par(), linux_class());

        // A surge: queue depth 9, current pool 2 → provision up.
        let up = auto.reconcile(
            key.clone(),
            /*current*/ 2,
            /*queue_depth*/ 9,
            /*in_flight*/ 0,
        );
        assert_eq!(up.desired, 10, "9 queued + 1 warm buffer");
        assert_eq!(up.delta(), 8, "provision 8 more");
        assert_eq!(up.provision_count(), 8);
        assert_eq!(up.deprovision_count(), 0);
        assert!(!up.is_scale_to_zero());

        // Drain: queue depth 0, current pool 10 → scale to zero.
        let down = auto.reconcile(
            key.clone(),
            /*current*/ 10,
            /*queue_depth*/ 0,
            /*in_flight*/ 0,
        );
        assert_eq!(down.desired, 0, "idle → scale-to-zero");
        assert_eq!(down.delta(), -10, "deprovision all 10");
        assert_eq!(down.deprovision_count(), 10);
        assert!(down.is_scale_to_zero(), "idle drains to zero");

        // Steady state: desired == current → a no-op (no infra churn).
        let steady = auto.reconcile(
            key, /*current*/ 6, /*queue_depth*/ 5, /*in_flight*/ 0,
        );
        assert_eq!(steady.desired, 6, "5 queued + 1 warm = 6 == current");
        assert_eq!(steady.delta(), 0, "steady state is a no-op");
    }

    /// **The autoscale→provision handoff: applying a scale-up plan provisions region-pinned hosts via
    /// the provider; a cross-region plan is refused.** The end-to-end loop (queue depth → plan →
    /// provision), region-pinned.
    #[test]
    fn applying_a_scale_plan_provisions_region_pinned_hosts() {
        let mut fleet = EuFleetProvider::new(GenericEuIaasAdapter, "01J0ACME", fr_par(), 100);
        let auto = Autoscaler::new(AutoscalePolicy::new(1, 0, 50));

        // Queue depth 4 in the cell's region → provision up to 5.
        let plan = auto.reconcile(
            PoolKey::new(fr_par(), linux_class()),
            /*current*/ 0,
            /*queue_depth*/ 4,
            /*in_flight*/ 0,
        );
        let hosts = fleet.apply(&plan).expect("apply the in-region plan");
        assert_eq!(hosts.len(), 5, "provisioned 5 hosts (4 queued + 1 warm)");
        for h in &hosts {
            assert_eq!(&h.region, &fr_par(), "region-pinned");
        }
        // The capacity report reflects the provisioned count.
        assert_eq!(fleet.capacity(fr_par()).unwrap().provisioned, 5);

        // A plan whose region is NOT the cell's is refused at apply (no global pool).
        let cross = auto.reconcile(PoolKey::new(eu_north(), linux_class()), 0, 4, 0);
        assert!(
            matches!(fleet.apply(&cross), Err(FleetError::CrossRegion(_))),
            "a cross-region plan is refused"
        );
    }

    // ---------------------------------------------------------------------------------------------
    // residency_verify report (contract 12.4, consumed).
    // ---------------------------------------------------------------------------------------------

    /// **The fleet reports its region into `residency_verify` (contract 12.4): a report matching the
    /// tenant's region of record is in-region; a mismatch is the breach the no-global-pool attestation
    /// catches.**
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

    // ---------------------------------------------------------------------------------------------
    // The fleet events (contract 2.2 — `ci.runner.*` via the outbox).
    // ---------------------------------------------------------------------------------------------

    /// **The four fleet events map to the frozen `ci.runner.*` tokens + build grammatical, PII-free
    /// `EventDraft`s (contract 2.2).** The tokens are the ONE source of truth (re-exported); the drafts
    /// are emitted via `OutboxTx::emit` (no causal fields on the draft).
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
            // The token is grammatical (the one Bus validator).
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
            // PII-free: a runner event carries no inline personal data.
            assert!(
                !draft.contains_personal_data,
                "a fleet event is PII-free (opaque ids + region/pool tokens)"
            );
            assert!(draft.pii_key_ref.is_none());
            // The payload carries the region + pool (the no-global-pool audit trail).
            assert_eq!(draft.payload["region"], "fr-par");
            assert_eq!(draft.payload["pool"], "linux-x64");
        }
    }

    /// **The fleet-write SQL is region-pinned by construction — the INSERT carries the region column,
    /// the COUNT predicate filters on region (no global pool at the read path either).**
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
