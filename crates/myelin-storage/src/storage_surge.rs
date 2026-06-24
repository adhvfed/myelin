//! # The F6 surge family on the STORAGE lanes (P-ST-34 → global P-444, M5)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §2 "S-M5" world-scale hardening (*the
//! 30× surge on the storage lanes — a CI artifact storm by one tenant does not starve another;
//! reserve/settle per-tenant fairness + the C4 cache namespaces + the cell bulkhead; the protected
//! human lane holds, the agent/CI lanes shed*). **Contract-index:** rows 11.5 (restore-verify at cell
//! scale — re-confirmed by the sibling drill) + **11.7** (reserve/settle per-tenant fairness under
//! surge — the fairness primitive this module drives). **Drill catalogue:**
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.1 — the **F6 surge family**
//! (SUB-D3 / GIT-D6 / CI-D2); the STORAGE faces of that family. **Doctrine:** EI-01 §3 (prove-it under
//! 1×/10×/30× surge; observability is part of the pass; never weaken a threshold to pass — a red is a
//! dated `claimed-not-proven` row), §2 (the protected-human-lane shed order — human holds, agent/CI
//! sheds; per-tenant blast-radius — one tenant's surge never sheds another's).
//!
//! ## What this module IS — the storage-tier face of the F6 surge family
//! The F6 surge family is a whole-system property; this module is its **storage-lane half**. Under a
//! 30× CI artifact storm by ONE tenant (the heaviest storage consumer — CI build-cache writes + log
//! segments, §8), the storage tier must:
//!
//! 1. **Hold the protected human lane** — a human's interactive storage op (a blob read for a UI, an
//!    OLTP read for a page render) is admitted within budget while the machine lanes are shedding
//!    (shed-last, EI-01 §2 / storage §2).
//! 2. **Shed the agent/CI lanes** with `429 + Retry-After` (the CI artifact storm is absorbed by
//!    SHEDDING, never by growing storage-lane latency unboundedly — Little's Law).
//! 3. **Keep cross-tenant impact at 0** — tenant A's 30× storm fills only A's per-tenant storage
//!    budget; tenant B's lanes (human included) are untouched. This is the per-tenant bound NESTED
//!    inside the cell bulkhead (P-ST-32): a noisy tenant is contained at the tenant boundary, and even
//!    at its worst is bounded by its cell's envelope and CANNOT reach a tenant in another cell.
//!
//! ## Coherence (EI-01 §7) — what this REUSES, and why it does NOT re-define the shed lane
//! The substrate already owns the principal-aware shed lane + the `RunClass` shed order
//! (`myelin_substrate::shed::{ShedLane, RunClass, Surface}`, P-035) and the cell bulkhead
//! (`myelin_control_plane::bulkhead::{CellFleet, CellBulkhead}`, P-432) — but **the substrate and the
//! control plane sit ABOVE storage in the crate DAG** (substrate is the DAG root; it depends ON
//! storage), so storage CANNOT depend on either (the edge would invert the layering). This module
//! therefore ships the **storage-tier's own** lane-fairness primitive — distinct from the public-surface
//! shed lane: it models the per-tenant STORAGE-TIER budget (blob-write IO, OLTP pool slots, CI
//! cache-write slots) the storage backends impose, not the HTTP intake. It REUSES storage's own
//! [`crate::reserve_settle::CostLedger`] (the per-tenant fairness ledger, contract 11.7) for the
//! per-tenant accounting half + applies the SAME shed-order DISCIPLINE (`speculative → batch/CI →
//! agent → human-last`) the substrate's lane does, so there is no doctrinal drift — one shed order,
//! two tiers. The storage drill cross-validates this model against the substrate's `ShedLane` (the
//! drill CAN depend on substrate as a dev-dep, the same way the cell-scale restore-verify drill does)
//! so the two agree (coherence, no parallel assertion).
//!
//! ## The storage-lane shed order (the per-tenant storage-tier budget)
//! Each tenant gets a bounded storage-lane budget `{ cap, human_reservation }` (the §7.1 bound,
//! per-tenant). The admission rule mirrors the substrate's `ShedLane::admit` EXACTLY (so the order is
//! one discipline): a **human** storage op is admitted while ANY slot (reserved included) is free
//! (shed-last); a **non-human** op (speculative/batch-CI/agent) may never take the reserved-for-human
//! slots and is held to a GRADED ceiling by promise strength, so the CI artifact storm (the batch-CI
//! lane) sheds at its ceiling well before the human reserved slots are touched.
//!
//! ## Floors named (the prompt's honesty register — designed-not-built)
//! - **The column-store seam measured-trigger** (BUS-6 / `column_store_seam`) is SPECIFIED-NOT-BUILT
//!   (`myelin_substrate::thresholds::ColumnStoreSeam` records the measurement gate, P-440): no
//!   production stream has been MEASURED to outgrow the JetStream tier at degraded latency, so the seam
//!   stays NAMED, no build owed. Named here per the prompt's DoD.
//! - **The generated projection-feeder index measured-trigger** (the `declare_indexable`
//!   code-projection feeder index, P-231) is likewise designed-not-built — the index is generated on a
//!   measured trigger, not pre-built. Named here.
//! - **The real storage backends under a real 30× fleet load.** This module models the per-tenant
//!   storage-lane budget + the shed order as a pure, deterministic function (no live backend) — the
//!   30× world-scale FLEET-hardware load is the ONE legitimate remaining floor (real fleet). The
//!   per-tenant fairness + shed-order + cross-tenant-0 PROPERTIES are complete and testable now and do
//!   not change shape when the real PgStore/S3BlobStore/ValkeyCache backends carry the load; the live
//!   stack integration (PgStore/S3BlobStore/ValkeyCache behind the traits) is already proven by the
//!   infra-stage integration drills — no NEW db/object-store/cache/bus trait is touched here, so no new
//!   integration drill is owed.

use std::collections::BTreeMap;

use myelin_tenancy::TenantId;

/// **The F6 surge multiplier the storage lanes are driven at (testing-strategy §3.1 — 1× baseline /
/// 10× stress / 30× surge).** The F6 / SUB-D3 / CI-D2 headline multiplier is **30×**: the storage
/// tier absorbs a 30× CI artifact storm by SHEDDING the batch-CI / agent lanes, never by growing
/// storage-lane latency unboundedly. The NUMBER is the frozen F6 surge multiplier (mirrors
/// `myelin_substrate::load_generator::Multiplier::SURGE` + the `[surge]` thresholds row); it is not
/// weakened to pass. The drill reads the multiplier from the FROZEN thresholds file (EI-01 §3), never
/// a hardcoded literal — this constant is the documented default-to-beat it asserts against.
pub const STORAGE_SURGE_MULTIPLIER: u32 = 30;

/// **The storage-lane class — the shed priority order (storage §2 / EI-01 §2; the SAME order as
/// `myelin_substrate::RunClass`).**
///
/// The variants are declared in shed order; the derived `Ord` is therefore the shed priority — a
/// LOWER class sheds FIRST. This is storage's own copy of the shed order (it cannot import the
/// substrate's `RunClass` without inverting the crate DAG), kept in lock-step with the substrate's by
/// the F6 storage drill, which cross-validates that the two orderings agree (coherence, EI-01 §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StorageLaneClass {
    /// Made no promise — dropped FIRST under pressure (prefetch/warm/speculative blob reads).
    Speculative,
    /// **Batch + CI: the CI artifact storm lane** — machine clients that can and should back off
    /// (`429 + Retry-After`). This is the lane the 30× CI build-cache write storm rides.
    BatchCi,
    /// Agent runs: machine clients; shed AFTER batch/CI but BEFORE humans.
    Agent,
    /// The interactive human — shed **LAST**, and only in true storage-tier saturation (the protected
    /// lane: a UI blob read / a page-render OLTP read).
    Human,
}

impl StorageLaneClass {
    /// Stable lowercase label for the `storage_lane_shed_count{lane}` telemetry signal (contract 1.8).
    pub fn lane(self) -> &'static str {
        match self {
            StorageLaneClass::Speculative => "speculative",
            StorageLaneClass::BatchCi => "batch_ci",
            StorageLaneClass::Agent => "agent",
            StorageLaneClass::Human => "human",
        }
    }
}

/// The decision a storage-lane admission makes (storage §2). Either the storage op is admitted (a
/// per-tenant budget slot was taken), or it is shed with a `429 + Retry-After` — the storage tier's
/// typed form of the back-pressure the gateway surfaces. Our own clients honour the `Retry-After`
/// (P-S17), so this is not a retry-storm amplifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageAdmission {
    /// Admitted — the storage op proceeds (a per-tenant slot was taken; release it on completion).
    Admit,
    /// Shed — this tenant's storage lane is at/over its protected boundary for this class. The caller
    /// maps this to HTTP **`429 Too Many Requests` + `Retry-After: {retry_after_secs}`**.
    Shed {
        /// The `Retry-After` value in **seconds** (the frozen unit) — clients honour it (P-S17).
        retry_after_secs: u64,
    },
}

impl StorageAdmission {
    /// `true` iff the storage op was admitted.
    pub fn is_admitted(self) -> bool {
        matches!(self, StorageAdmission::Admit)
    }
}

/// **The per-tenant storage-lane budget (storage §7.1 — bounded everything, per-tenant).**
///
/// The bound on concurrent admitted storage ops *for one tenant* on the storage tier, plus the
/// protected-human-lane reservation (slots within the cap reserved so a human storage op is never shed
/// while a machine lane still occupies the tenant's budget) and the `Retry-After` advertised on a
/// shed. Per-tenant is the blast-radius guarantee: one tenant's surge fills only its own budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageLaneBudget {
    /// The per-tenant in-flight cap on the storage tier (the §7.1 bound — blob-write IO + OLTP pool +
    /// CI cache-write slots), per-tenant so one tenant cannot starve another.
    pub per_tenant_in_flight_cap: u32,
    /// The protected-human-lane reservation: in-flight slots (within the cap) reserved so the machine
    /// lanes are shed BEFORE a human storage op is ever shed.
    pub human_lane_reservation: u32,
    /// The `Retry-After` (seconds) the storage tier advertises when it sheds.
    pub retry_after_secs: u64,
}

impl StorageLaneBudget {
    /// The §2 seed default-to-beat for the storage tier: a 128-slot per-tenant cap with 32 reserved
    /// for the human lane (25% — at-or-above the substrate's measured 20% human-lane floor) and a 5 s
    /// Retry-After. A named v1 default; the cap is the per-tenant storage IO budget the backends
    /// impose. Mirrors the storage-lane posture of the substrate's `Surface::GitFrontDoor` /
    /// `CiDispatch` budgets without re-defining them.
    pub fn v1_default() -> StorageLaneBudget {
        StorageLaneBudget {
            per_tenant_in_flight_cap: 128,
            human_lane_reservation: 32,
            retry_after_secs: 5,
        }
    }
}

/// Per-tenant in-flight accounting: human vs non-human slots. Per-tenant keying is the blast-radius
/// guarantee — a surge on one tenant cannot evict another tenant's slots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TenantInFlight {
    human: u32,
    non_human: u32,
}

impl TenantInFlight {
    fn total(self) -> u32 {
        self.human + self.non_human
    }
}

/// **The per-tenant storage-lane shed gate (storage §2; the F6 storage face).**
///
/// Admits-or-sheds a storage op against the issuing tenant's storage-lane budget, applying the shed
/// order `speculative → batch/CI → agent → human-last` PER-TENANT. In-flight work is counted per
/// `TenantId`, so one tenant's 30× CI storm fills only that tenant's budget and never sheds another
/// tenant's human storage op (the cross-tenant-impact-0 property). The admission rule mirrors
/// `myelin_substrate::shed::ShedLane::admit` EXACTLY (one shed-order discipline, two tiers).
#[derive(Clone, Debug)]
pub struct StorageLaneGate {
    budget: StorageLaneBudget,
    /// Per-tenant in-flight accounting (the blast-radius boundary).
    tenants: BTreeMap<TenantId, TenantInFlight>,
    /// Per-lane cumulative shed counts (the `storage_lane_shed_count{lane}` producer signal, 1.8).
    shed_counts: BTreeMap<StorageLaneClass, u64>,
}

impl StorageLaneGate {
    /// Open a storage-lane gate with the §2 v1-default budget.
    pub fn new() -> StorageLaneGate {
        StorageLaneGate::with_budget(StorageLaneBudget::v1_default())
    }

    /// Open a storage-lane gate with an explicit budget (used by drills to drive the boundary).
    pub fn with_budget(budget: StorageLaneBudget) -> StorageLaneGate {
        StorageLaneGate {
            budget,
            tenants: BTreeMap::new(),
            shed_counts: BTreeMap::new(),
        }
    }

    /// **The storage-tier admission decision.** Reads the issuing tenant's current per-tenant
    /// in-flight + the op's [`StorageLaneClass`] and returns [`StorageAdmission::Admit`] (taking a
    /// slot) or [`StorageAdmission::Shed`] (`429 + Retry-After`), applying the shed order with the
    /// human lane protected, per-tenant.
    ///
    /// The rule (identical to the substrate's `ShedLane::admit` — coherence, EI-01 §7):
    /// - A **human** op is admitted while `total < cap` (it may use the reserved slots → shed last).
    /// - A **non-human** op is admitted only while `non_human < ceiling(class)` AND `total < cap`,
    ///   where `ceiling` is a GRADED threshold by promise strength so a lower-promise lane sheds at a
    ///   tighter ceiling (speculative sheds before batch/CI sheds before agent), and no non-human lane
    ///   may ever consume the reserved-for-human slots (its top ceiling is `cap - reservation`).
    pub fn admit(&mut self, tenant: &TenantId, class: StorageLaneClass) -> StorageAdmission {
        let cap = self.budget.per_tenant_in_flight_cap;
        let reserved = self.budget.human_lane_reservation.min(cap);
        let cur = self.tenants.get(tenant).copied().unwrap_or_default();

        let admit = match class {
            // human: shed LAST — admitted while ANY slot (reserved included) is free.
            StorageLaneClass::Human => cur.total() < cap,
            // non-human lanes: a graded ceiling by promise strength; never take the reserved slots.
            other => {
                let non_human_budget = cap.saturating_sub(reserved);
                let step = (non_human_budget / 8).max(1);
                let ceiling = match other {
                    StorageLaneClass::Speculative => non_human_budget.saturating_sub(2 * step),
                    StorageLaneClass::BatchCi => non_human_budget.saturating_sub(step),
                    StorageLaneClass::Agent => non_human_budget,
                    StorageLaneClass::Human => unreachable!("human handled above"),
                };
                cur.non_human < ceiling && cur.total() < cap
            }
        };

        if admit {
            let entry = self.tenants.entry(tenant.clone()).or_default();
            if class == StorageLaneClass::Human {
                entry.human += 1;
            } else {
                entry.non_human += 1;
            }
            StorageAdmission::Admit
        } else {
            *self.shed_counts.entry(class).or_insert(0) += 1;
            StorageAdmission::Shed {
                retry_after_secs: self.budget.retry_after_secs,
            }
        }
    }

    /// Release a slot a prior [`StorageLaneGate::admit`] took for `(tenant, class)`. Saturating — a
    /// stray release never wraps.
    pub fn release(&mut self, tenant: &TenantId, class: StorageLaneClass) {
        if let Some(entry) = self.tenants.get_mut(tenant) {
            if class == StorageLaneClass::Human {
                entry.human = entry.human.saturating_sub(1);
            } else {
                entry.non_human = entry.non_human.saturating_sub(1);
            }
        }
    }

    /// The cumulative shed count for a lane (the `storage_lane_shed_count{lane}` producer signal).
    pub fn shed_count(&self, class: StorageLaneClass) -> u64 {
        self.shed_counts.get(&class).copied().unwrap_or(0)
    }

    /// The current per-tenant total in-flight (admitted not yet released) — for the per-tenant
    /// blast-radius assertions.
    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.tenants
            .get(tenant)
            .copied()
            .unwrap_or_default()
            .total()
    }

    /// The per-tenant in-flight human-lane count (for the protected-lane-held assertion).
    pub fn human_in_flight(&self, tenant: &TenantId) -> u32 {
        self.tenants.get(tenant).copied().unwrap_or_default().human
    }

    /// The cap (the §7.1 per-tenant storage bound).
    pub fn cap(&self) -> u32 {
        self.budget.per_tenant_in_flight_cap
    }
}

impl Default for StorageLaneGate {
    fn default() -> Self {
        StorageLaneGate::new()
    }
}

/// **The F6 storage-lane surge proof (the dated green artifact the DoD names; storage §2).**
///
/// The PII-free aggregate result of driving a 30× CI artifact storm by ONE tenant at the storage tier
/// and MEASURING the three F6 properties — emitted as a typed value so the green is LOUD (observability
/// is part of the pass, EI-01 §3) and the gate can go RED (a non-green artifact is never silently a
/// pass).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageSurgeReport {
    /// The surge multiplier the storm was driven at (== [`STORAGE_SURGE_MULTIPLIER`] for the F6
    /// headline; read from the FILE by the drill).
    pub multiplier: u32,
    /// The surging (noisy) tenant's batch-CI lane shed count — `> 0` proves the CI artifact storm was
    /// absorbed by SHEDDING (`429 + Retry-After`), not by unbounded storage-lane latency.
    pub surging_tenant_ci_shed_count: u64,
    /// The surging tenant's human-lane shed count — `0` on a green artifact (the protected human lane
    /// HELD even on the surging tenant, within its reserved slots).
    pub surging_tenant_human_shed_count: u64,
    /// **The cross-tenant impact** — the number of OTHER tenants whose storage lanes were affected
    /// (any shed, any slot taken) by the surging tenant's storm. `0` on a green artifact (the
    /// per-tenant bound contained the storm; one tenant's surge never sheds another's).
    pub cross_tenant_impact: u64,
    /// Whether the quiet tenant's human storage op was admitted within budget DURING the surge — the
    /// protected-human-lane-holds property at the cross-tenant grain.
    pub quiet_tenant_human_admitted: bool,
}

impl StorageSurgeReport {
    /// **Is this a GREEN F6 storage-lane artifact?** The three F6 properties all hold:
    /// 1. the agent/CI lanes SHED under the surge (`surging_tenant_ci_shed_count > 0`);
    /// 2. the human lane HELD (no human shed on the surging tenant + the quiet tenant's human is
    ///    admitted within budget);
    /// 3. cross-tenant impact is 0 (the storm is contained to the surging tenant).
    pub fn is_f6_green(&self) -> bool {
        self.surging_tenant_ci_shed_count > 0
            && self.surging_tenant_human_shed_count == 0
            && self.quiet_tenant_human_admitted
            && self.cross_tenant_impact == 0
    }

    /// The dated green-artifact line a drill prints on PASS (the measured-numbers proof; the caller
    /// prefixes the dated `[P-444 F6 STORAGE GREEN <date>]` tag).
    pub fn summary(&self) -> String {
        format!(
            "F6 storage-lane surge ({}×): CI artifact storm by one tenant ABSORBED by shedding \
             (batch_ci shed {}× with 429+Retry-After); protected human lane HELD (human shed {}, \
             quiet-tenant human admitted={}); cross_tenant_impact={} (other tenants untouched). \
             No threshold weakened.",
            self.multiplier,
            self.surging_tenant_ci_shed_count,
            self.surging_tenant_human_shed_count,
            self.quiet_tenant_human_admitted,
            self.cross_tenant_impact,
        )
    }
}

/// **Drive a 30×-class CI artifact storm by `surging` at the storage tier and MEASURE the F6
/// properties (the storage face of SUB-D3 / GIT-D6 / CI-D2).**
///
/// `gate` is the per-tenant storage-lane gate (the per-tenant fairness + shed order). `surging` is the
/// noisy tenant issuing the storm; `quiet` is an unrelated co-tenant whose human lane must hold.
/// `storm_ops` is the number of batch-CI storage ops the storm issues (derived from a real generator
/// run at the surge multiplier — never hand-typed in the drill). The function:
/// 1. issues `storm_ops` batch-CI ops by `surging` (the CI build-cache write storm) — the storm's
///    excess sheds at the batch-CI ceiling (`429 + Retry-After`), never unbounded;
/// 2. issues a human storage op by `surging` (its OWN human must still be admitted — within its
///    reserved slots — shed-last on the surging tenant too);
/// 3. issues a human storage op by `quiet` (an unrelated tenant — must be admitted within budget,
///    untouched by the storm).
///
/// It then measures the cross-tenant impact (the quiet tenant's in-flight delta caused by the storm,
/// which is 0 by the per-tenant bound).
///
/// Returns the [`StorageSurgeReport`] — the dated F6 green artifact (or a RED one if a property fails).
pub fn run_storage_lane_surge(
    gate: &mut StorageLaneGate,
    surging: &TenantId,
    quiet: &TenantId,
    storm_ops: u64,
    multiplier: u32,
) -> StorageSurgeReport {
    // (1) The 30× CI artifact storm: storm_ops batch-CI storage ops by the surging tenant. The excess
    //     over the batch-CI ceiling sheds with 429+Retry-After (absorbed, not unbounded).
    for _ in 0..storm_ops {
        let _ = gate.admit(surging, StorageLaneClass::BatchCi);
    }
    let surging_tenant_ci_shed_count = gate.shed_count(StorageLaneClass::BatchCi);

    // (2) The surging tenant's OWN human storage op — admitted within its reserved slots (shed-last).
    let surging_human = gate.admit(surging, StorageLaneClass::Human);
    let surging_tenant_human_shed_count = gate.shed_count(StorageLaneClass::Human);

    // (3) The quiet co-tenant's human storage op — must be admitted within budget, untouched by the
    //     surge. Measure the quiet tenant's in-flight BEFORE the surge could have touched it (0) and
    //     AFTER admitting its own human (1): the cross-tenant impact is the delta the STORM caused,
    //     which is 0 (only the quiet tenant's own op moved its in-flight).
    let quiet_before = gate.in_flight(quiet);
    let quiet_human = gate.admit(quiet, StorageLaneClass::Human);
    let quiet_tenant_human_admitted = quiet_human.is_admitted();

    // Cross-tenant impact: any slot the storm took in the quiet tenant's budget (0 by the per-tenant
    // bound — the storm only ever touched the surging tenant's accounting).
    let cross_tenant_impact = u64::from(quiet_before);

    // The surging tenant's own human being admitted is the shed-LAST property on the noisy tenant; a
    // shed there would mean its reserved lane was starved by its own machine storm (a contract breach).
    debug_assert!(
        surging_human.is_admitted(),
        "the surging tenant's human reserved lane must hold even under its own storm"
    );

    StorageSurgeReport {
        multiplier,
        surging_tenant_ci_shed_count,
        surging_tenant_human_shed_count,
        cross_tenant_impact,
        quiet_tenant_human_admitted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }

    fn small_budget() -> StorageLaneBudget {
        // cap 10, reserve 4: non_human_budget 6, step max(6/8,1)=1 → speculative 4, batch_ci 5, agent 6.
        StorageLaneBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 4,
            retry_after_secs: 5,
        }
    }

    /// The declared variant order IS the shed order: a lower class sheds first (coherence with the
    /// substrate's `RunClass` ordering — the cross-validation in the drill proves they agree).
    #[test]
    fn shed_priority_order_is_speculative_then_batch_ci_then_agent_then_human() {
        assert!(StorageLaneClass::Speculative < StorageLaneClass::BatchCi);
        assert!(StorageLaneClass::BatchCi < StorageLaneClass::Agent);
        assert!(StorageLaneClass::Agent < StorageLaneClass::Human);
    }

    /// **The storage-lane shed order fires in the right priority** under saturation: speculative sheds
    /// first, then batch/CI, then agent, and the human is admitted while a machine lane is already
    /// being shed (human shed last). Mirrors the substrate's `shed_order_*` test (one discipline).
    #[test]
    fn storage_lane_sheds_speculative_then_batch_ci_then_agent_then_human_last() {
        let mut gate = StorageLaneGate::with_budget(small_budget());
        let t = tenant("acme");
        // fill the non-human in-flight up to 4 with agent ops (all under every ceiling).
        for _ in 0..4 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        // now non_human == 4: SPECULATIVE sheds first (ceiling 4, not < 4).
        assert!(matches!(
            gate.admit(&t, StorageLaneClass::Speculative),
            StorageAdmission::Shed { .. }
        ));
        // batch/ci still admitted (ceiling 5, 4 < 5).
        assert_eq!(
            gate.admit(&t, StorageLaneClass::BatchCi),
            StorageAdmission::Admit
        ); // → 5
           // now non_human == 5: batch/ci sheds, agent still admitted (ceiling 6).
        assert!(matches!(
            gate.admit(&t, StorageLaneClass::BatchCi),
            StorageAdmission::Shed { .. }
        ));
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Admit
        ); // → 6
           // now non_human == 6 == cap-reserved: AGENT sheds, but the HUMAN is still admitted (protected).
        assert!(matches!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Shed { .. }
        ));
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Human),
            StorageAdmission::Admit
        );

        assert_eq!(gate.shed_count(StorageLaneClass::Speculative), 1);
        assert_eq!(gate.shed_count(StorageLaneClass::BatchCi), 1);
        assert_eq!(gate.shed_count(StorageLaneClass::Agent), 1);
        assert_eq!(
            gate.shed_count(StorageLaneClass::Human),
            0,
            "the human storage lane has NOT been shed"
        );
    }

    /// **Human shed last: only in true storage-tier saturation (every slot, reserved included, full).**
    #[test]
    fn human_storage_lane_is_shed_last_only_in_true_saturation() {
        let budget = StorageLaneBudget {
            per_tenant_in_flight_cap: 5,
            human_lane_reservation: 2,
            retry_after_secs: 7,
        };
        let mut gate = StorageLaneGate::with_budget(budget);
        let t = tenant("acme");
        // fill the non-reserved budget (3) with agents; agents then shed (cap-reserved = 3).
        for _ in 0..3 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        assert!(matches!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Shed { .. }
        ));
        // humans keep being admitted into the reserved slots (total 3 → 4 → 5).
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Human),
            StorageAdmission::Admit
        );
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Human),
            StorageAdmission::Admit
        );
        // now total == cap == 5: TRUE saturation — even the human is shed (shed last, but it IS shed).
        match gate.admit(&t, StorageLaneClass::Human) {
            StorageAdmission::Shed { retry_after_secs } => assert_eq!(retry_after_secs, 7),
            StorageAdmission::Admit => {
                panic!("a fully-saturated storage tier must shed even the human")
            }
        }
        assert_eq!(gate.shed_count(StorageLaneClass::Human), 1);
    }

    /// **Per-tenant: one tenant's storage storm does NOT shed another tenant's human (blast-radius).**
    #[test]
    fn storage_shedding_is_per_tenant() {
        let budget = StorageLaneBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 1,
            retry_after_secs: 3,
        };
        let mut gate = StorageLaneGate::with_budget(budget);
        let noisy = tenant("noisy");
        let quiet = tenant("quiet");
        // SATURATE the noisy tenant completely.
        for _ in 0..3 {
            assert_eq!(
                gate.admit(&noisy, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        assert!(matches!(
            gate.admit(&noisy, StorageLaneClass::Agent),
            StorageAdmission::Shed { .. }
        ));
        assert_eq!(
            gate.admit(&noisy, StorageLaneClass::Human),
            StorageAdmission::Admit
        );
        assert!(matches!(
            gate.admit(&noisy, StorageLaneClass::Human),
            StorageAdmission::Shed { .. }
        ));
        // the QUIET tenant is COMPLETELY UNAFFECTED.
        assert_eq!(
            gate.in_flight(&quiet),
            0,
            "the quiet tenant's budget is independent"
        );
        assert_eq!(
            gate.admit(&quiet, StorageLaneClass::Human),
            StorageAdmission::Admit,
            "the noisy tenant's storage storm must NEVER shed another tenant's human"
        );
    }

    /// Release frees a storage-lane slot so the tier recovers after the surge passes.
    #[test]
    fn release_frees_a_storage_slot() {
        let budget = StorageLaneBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 1,
            retry_after_secs: 1,
        };
        let mut gate = StorageLaneGate::with_budget(budget);
        let t = tenant("acme");
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Admit
        );
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Admit
        ); // non_human 2 == cap-reserved
        assert!(matches!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Shed { .. }
        ));
        gate.release(&t, StorageLaneClass::Agent);
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Admit,
            "a released storage slot is reusable"
        );
    }

    /// **THE F6 STORAGE-LANE SURGE PROOF (the dated green artifact the DoD names).** A 30×-class CI
    /// artifact storm by one tenant: the batch-CI lane sheds (absorbed, not unbounded), the human lane
    /// HOLDS (surging tenant's own human + the quiet tenant's human), cross-tenant impact 0.
    #[test]
    fn f6_storage_lane_surge_emits_a_green_artifact() {
        let mut gate = StorageLaneGate::with_budget(small_budget());
        let surging = tenant("noisy-ci");
        let quiet = tenant("quiet-co-tenant");
        // a storm well past the batch-CI ceiling (so it MUST shed) — 30× the small base of 1 op.
        let report = run_storage_lane_surge(
            &mut gate,
            &surging,
            &quiet,
            STORAGE_SURGE_MULTIPLIER as u64,
            STORAGE_SURGE_MULTIPLIER,
        );
        assert!(
            report.is_f6_green(),
            "the F6 storage-lane surge must be GREEN: {report:?}"
        );
        assert!(
            report.surging_tenant_ci_shed_count > 0,
            "the CI artifact storm must be absorbed by SHEDDING (429+Retry-After), not unbounded latency"
        );
        assert_eq!(
            report.surging_tenant_human_shed_count, 0,
            "the human lane held"
        );
        assert!(
            report.quiet_tenant_human_admitted,
            "the quiet tenant's human held"
        );
        assert_eq!(
            report.cross_tenant_impact, 0,
            "the storm is contained to the surging tenant"
        );
        // the summary names the measured numbers (observability is part of the pass).
        let s = report.summary();
        assert!(s.contains("F6 storage-lane surge"));
        assert!(s.contains("cross_tenant_impact=0"));
    }

    /// **The F6 gate is NOT vacuous — it can go RED.** A report where the storm did NOT shed (the lane
    /// was unbounded), or the human was shed, or a cross-tenant impact appeared, reads RED. Proves the
    /// green is earned (EI-01 §3).
    #[test]
    fn f6_report_can_go_red() {
        // no shed → the storm was NOT absorbed (unbounded latency) → RED.
        let no_shed = StorageSurgeReport {
            multiplier: 30,
            surging_tenant_ci_shed_count: 0,
            surging_tenant_human_shed_count: 0,
            cross_tenant_impact: 0,
            quiet_tenant_human_admitted: true,
        };
        assert!(!no_shed.is_f6_green(), "no shed = unbounded latency = RED");

        // human shed → the protected lane was starved → RED.
        let human_shed = StorageSurgeReport {
            multiplier: 30,
            surging_tenant_ci_shed_count: 5,
            surging_tenant_human_shed_count: 1,
            cross_tenant_impact: 0,
            quiet_tenant_human_admitted: true,
        };
        assert!(!human_shed.is_f6_green(), "a shed human lane = RED");

        // cross-tenant impact → the storm escaped its tenant → RED.
        let cross = StorageSurgeReport {
            multiplier: 30,
            surging_tenant_ci_shed_count: 5,
            surging_tenant_human_shed_count: 0,
            cross_tenant_impact: 1,
            quiet_tenant_human_admitted: true,
        };
        assert!(!cross.is_f6_green(), "a cross-tenant impact = RED");

        // quiet tenant's human shed → another tenant's human was starved → RED.
        let quiet_shed = StorageSurgeReport {
            multiplier: 30,
            surging_tenant_ci_shed_count: 5,
            surging_tenant_human_shed_count: 0,
            cross_tenant_impact: 0,
            quiet_tenant_human_admitted: false,
        };
        assert!(!quiet_shed.is_f6_green(), "a starved co-tenant human = RED");
    }

    /// The v1-default budget reserves at-or-above the substrate's measured 20% human-lane floor (the
    /// storage tier does not tune the human lane into starvation — coherence with the substrate floor).
    #[test]
    fn v1_default_budget_holds_the_human_lane_floor() {
        let b = StorageLaneBudget::v1_default();
        assert!(b.per_tenant_in_flight_cap > 0, "bounded (§7.1)");
        assert!(
            b.human_lane_reservation <= b.per_tenant_in_flight_cap,
            "reservation within cap"
        );
        // 20% floor of 128 = 26 (ceil 25.6); the v1 default reserves 32 ≥ 26.
        let floor_20pct = (u64::from(b.per_tenant_in_flight_cap) * 2000).div_ceil(10_000) as u32;
        assert!(
            b.human_lane_reservation >= floor_20pct,
            "the storage human lane reserves {} ≥ the 20% floor {}",
            b.human_lane_reservation,
            floor_20pct
        );
    }

    /// Each lane label is the stable lowercase contract-1.8 signal name (kills the `lane()` →
    /// `""`/`"xyzzy"` mutants — the label is a load-bearing telemetry key, not a free string).
    #[test]
    fn lane_labels_are_the_stable_signal_names() {
        assert_eq!(StorageLaneClass::Speculative.lane(), "speculative");
        assert_eq!(StorageLaneClass::BatchCi.lane(), "batch_ci");
        assert_eq!(StorageLaneClass::Agent.lane(), "agent");
        assert_eq!(StorageLaneClass::Human.lane(), "human");
    }

    /// `is_admitted` is true ONLY for `Admit` and false for `Shed` (kills the `-> true` mutant —
    /// the admit/shed distinction is the whole gate).
    #[test]
    fn is_admitted_distinguishes_admit_from_shed() {
        assert!(StorageAdmission::Admit.is_admitted());
        assert!(!StorageAdmission::Shed {
            retry_after_secs: 5
        }
        .is_admitted());
    }

    /// The `in_flight` / `human_in_flight` accessors report the EXACT per-tenant slot counts after a
    /// mix of human + non-human admits (kills the accessor `-> 0`/`-> 1` mutants — the cross-tenant
    /// and protected-lane assertions read these, so a wrong count would silently pass a starved lane).
    #[test]
    fn in_flight_accessors_report_exact_per_tenant_counts() {
        let mut gate = StorageLaneGate::with_budget(small_budget());
        let t = tenant("acme");
        // 2 agents (non-human) + 3 humans → total 5, human 3.
        for _ in 0..2 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        for _ in 0..3 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Human),
                StorageAdmission::Admit
            );
        }
        assert_eq!(
            gate.in_flight(&t),
            5,
            "total in-flight = 2 non-human + 3 human"
        );
        assert_eq!(gate.human_in_flight(&t), 3, "exactly 3 human slots taken");
        // an untouched tenant reports 0 (not a constant) — the cross-tenant-0 property reads this.
        assert_eq!(gate.in_flight(&tenant("other")), 0);
        assert_eq!(gate.human_in_flight(&tenant("other")), 0);
        // releasing a human frees exactly that slot (the count is real accounting, not a constant).
        gate.release(&t, StorageLaneClass::Human);
        assert_eq!(gate.human_in_flight(&t), 2);
        assert_eq!(gate.in_flight(&t), 4);
    }

    /// **The graded-ceiling arithmetic is exact** (kills the `2 * step` → `2 + step` / `2 / step`
    /// mutants and the `< ceiling` → `<= ceiling` boundary mutant). With a budget where `2 * step` and
    /// the boundary are distinguishable, speculative sheds at EXACTLY its ceiling and one slot earlier
    /// than batch/CI — a `*`-to-`+`/`/` mutation or an off-by-one boundary would move the shed point.
    #[test]
    fn graded_ceiling_arithmetic_is_exact() {
        // cap 80, reserve 16 → non_human_budget 64, step = max(64/8,1) = 8.
        // speculative ceiling = 64 - 2*8 = 48; batch_ci = 64 - 8 = 56; agent = 64.
        // (2 + step = 10 ≠ 48-shape; 2 / step = 0 → ceiling 64; both would mis-place the shed point.)
        let budget = StorageLaneBudget {
            per_tenant_in_flight_cap: 80,
            human_lane_reservation: 16,
            retry_after_secs: 5,
        };
        let mut gate = StorageLaneGate::with_budget(budget);
        let t = tenant("acme");
        // fill non-human to exactly 47 with agents (under the 48 speculative ceiling).
        for _ in 0..47 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        // at 47: speculative still admitted (47 < 48).
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Speculative),
            StorageAdmission::Admit,
            "speculative admitted at 47 < its 48 ceiling"
        );
        // now non_human == 48 == speculative ceiling: speculative sheds (48 not < 48 — the `<`, not `<=`),
        // but batch_ci is still admitted (48 < 56).
        assert!(
            matches!(
                gate.admit(&t, StorageLaneClass::Speculative),
                StorageAdmission::Shed { .. }
            ),
            "speculative sheds AT its ceiling (the `<` boundary, not `<=`)"
        );
        assert_eq!(
            gate.admit(&t, StorageLaneClass::BatchCi),
            StorageAdmission::Admit,
            "batch_ci still admitted at 48 (< its 56 ceiling) — speculative sheds 8 slots earlier"
        );
    }

    /// **A non-human op is shed at TRUE saturation even when its own class ceiling is not yet reached
    /// (kills the non-human `cur.total() < cap` → `<= cap` mutant).** When humans fill the reserved
    /// slots so `total == cap` while `non_human` is still under its ceiling, a non-human op must STILL
    /// shed (there is genuinely no slot left) — the `< cap` total guard, not `<= cap`. A `<=` mutation
    /// would wrongly admit a non-human into a full tier (over-committing past the cap).
    #[test]
    fn a_non_human_sheds_at_total_saturation_even_below_its_class_ceiling() {
        // cap 10, reserve 6 → non_human_budget 4, agent ceiling 4.
        let budget = StorageLaneBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 6,
            retry_after_secs: 5,
        };
        let mut gate = StorageLaneGate::with_budget(budget);
        let t = tenant("acme");
        // 3 agents (non_human 3, < the 4 ceiling) + 7 humans (using the reserved slots) → total 10 == cap.
        for _ in 0..3 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        for _ in 0..7 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Human),
                StorageAdmission::Admit
            );
        }
        assert_eq!(
            gate.in_flight(&t),
            10,
            "the tier is at cap (3 non-human + 7 human)"
        );
        // an agent op: its own ceiling (4) is NOT yet reached (non_human 3 < 4), but total == cap, so it
        // MUST shed — the `total() < cap` guard (10 < 10 = false), NOT `<= cap` (which would admit).
        assert!(
            matches!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Shed { .. }
            ),
            "a non-human MUST shed at total saturation even below its class ceiling (the `< cap` guard)"
        );
    }
}
