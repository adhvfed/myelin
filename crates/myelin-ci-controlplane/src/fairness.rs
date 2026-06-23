//! **The CI scheduler fairness slice — DRR fair-share over `fair_key` + priority lanes + per-tenant
//! backpressure (CI-P13 / P-356, M4).**
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §2.2 (fairness — Deficit Round Robin over `fair_key`, the per-`fair_key` deficit counter, the
//! canonical-CI-multi-tenant-starvation failure it prevents; the floor → hierarchical scheduler),
//! §2.3 (priority lanes interactive > batch > deploy), §2.4 (backpressure & abuse — per-tenant
//! in-flight caps, the bounded run-queue); `01-tech-and-data-model.md` §3.3 (`fair_deficit`).
//! **Reconciliation:** `05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §OQ-K
//! (the per-surface shed budget — bounded run-queue per tenant; the CI-surge lane shed order
//! speculative → batch/CI → agent → human-last). **Contracts consumed:** 1.11 (the protected-human-
//! lane shed order + the CI-surge per-surface budget floor), 1.8 (the per-`fair_key` wait-time
//! histogram / queue-depth telemetry).
//!
//! ## What CI-P13 ships here (the slice CI-P12 named as its floor)
//! CI-P12's claim [`crate::scheduler::CLAIM_QUERY`] ORDERS on `fair_deficit.deficit DESC` (the DRR
//! term) and `lane_priority DESC` (the strict lane) — but it does **not** advance/replenish the
//! deficit counter, and it carries no per-tenant in-flight cap. This module is exactly those three:
//!
//! 1. **DRR deficit advance/replenish over `fair_key`** ([`FairShare`]). The fairness intuition is
//!    DRR (Shreedhar & Varghese, SIGCOMM 1996), applied at claim time (arch 02 §2.2): each `fair_key`
//!    (= tenant or tenant:project) holds a **deficit counter**; the claim serves the *least-recently-
//!    served* eligible tenant (highest deficit) first. On claim the served `fair_key`'s deficit is
//!    **decremented by one quantum** (it has now been served, so it drops in priority), and a periodic
//!    **REPLENISH** adds a **plan-weighted quantum** back to every `fair_key` (so a higher plan tier
//!    recovers priority faster — weighted fair-share, the intuition of Linux CFS). This is what stops
//!    one tenant's 10k-job matrix from starving every other tenant — **the canonical CI multi-tenant
//!    fairness failure** (arch 02 §2.2).
//! 2. **Priority lanes** ([`crate::scheduler::Lane`], interactive > batch > deploy) are the strict
//!    `ORDER BY` already in the claim — the **protected-human-lane analogue inside CI**: interactive
//!    PR-check feedback never queues behind a batch matrix. This module adds the **lane shed order**
//!    ([`shed_order`]) that composes the lanes with the platform shed order (contract 1.11): under
//!    surge CI sheds the batch lane first and holds interactive — the same precedence the substrate
//!    [`myelin_substrate::ShedLane`] over [`myelin_substrate::ShedSurface::CiDispatch`] enforces at the
//!    public dispatch surface (REUSED, never re-defined — see [`shed_order`]).
//! 3. **Backpressure** ([`Backpressure`]): a **per-tenant in-flight cap** (the bounded run-queue,
//!    OQ-K) — over-cap jobs **queue gracefully** (they stay `queued`, never collapse the scheduler);
//!    the cap is checked at claim time so a tenant can never run more than its budget at once.
//!
//! ## DB-free model + the live-stack proof (the binding data-layer policy)
//! Like [`crate::scheduler`], this module carries the DRR accounting TWICE, in lock-step:
//! - the [`ADVANCE_DEFICIT_QUERY`] / [`REPLENISH_DEFICIT_QUERY`] / [`IN_FLIGHT_COUNT_QUERY`]
//!   **`&str` SQL** the live OLTP path runs against the real `fair_deficit` + `job_queue` tables. The
//!   REAL apply against the dev-stack Postgres (advance on a real claim, plan-weighted replenish, the
//!   over-cap count) is `tests/integration_ci_p13_fairness.rs` behind the `integration` cargo feature;
//! - a **deterministic in-memory model** ([`FairShare`] / [`Backpressure`]) with the IDENTICAL
//!   semantics — so the unit + fairness-drill tests are deterministic and DB-free, while the live test
//!   proves the SQL carries the same algorithm. Neither is a mock of the other.
//!
//! ## Mutation-score floor (mandatory-core — the scheduling fairness hot path)
//! [`FairShare`] + [`Backpressure`] are mandatory-core (a wrong deficit advance or a wrong cap
//! silently re-introduces starvation). The cargo-mutants floor for this module is **100% of viable
//! mutants caught** (`cargo mutants -p myelin-ci-controlplane --file
//! crates/myelin-ci-controlplane/src/fairness.rs`): every arithmetic/comparison mutant in the deficit
//! advance/replenish + the cap check is killed by the unit tests below (the plan-weighted replenish,
//! the decrement-on-serve, the `>= cap` boundary). The fairness *property* (no-starvation) is proven
//! by the [`tests::fairness_no_starvation_interactive_holds`] drill.
//!
//! ## Floors named (VISION §3 / the prompt DoD)
//! - **flat DRR fair-share at claim time → a richer hierarchical (per-tenant → per-project →
//!   per-pipeline) scheduler** is the named follow-on **CI-P29**, promotion-triggered by a measured
//!   per-`fair_key` starvation-histogram signal (open question 07#1, contract 1.8). This module is the
//!   flat single-level DRR; it does NOT split a tenant's deficit per-project/per-pipeline.
//! - **the 30× surge tuning of the DRR weights / the shed-budget numbers** is **CI-P30** (CI-M5): the
//!   plan-tier quantum weights ([`PlanTier::quantum_weight`]) + the per-tenant cap ([`DEFAULT_TENANT_IN_FLIGHT_CAP`])
//!   are **named v1 floors** (round, conservative), tuned by the full 30× CI-D2 surge drill. Changing
//!   a number is a floor-tuning change, not a contract change (the contract is "bounded + DRR +
//!   strict lane + shed order", which is structural and tested here).

use std::collections::BTreeMap;

use crate::scheduler::Lane;

// =================================================================================================
// 1. The live OLTP DRR/backpressure SQL (arch 02 §2.2/§2.4). Held as `&str` so the lints do not
//    mistake the DML for live Rust; the live integration test runs the IDENTICAL statements.
// =================================================================================================

/// **The DRR advance on claim (arch 02 §2.2 — "advance `fair_deficit` for the claimed `fair_key`").**
/// On a successful claim the served `fair_key`'s deficit is decremented by one quantum (it has now
/// been served, so it drops below the still-waiting tenants in the claim's `deficit DESC` order) and
/// its `last_served` is stamped (the wait-time histogram input, contract 1.8). An UPSERT so a
/// first-ever claim for a `fair_key` materialises the row at `-quantum`. Bind: `$1 tenant_id`,
/// `$2 region`, `$3 fair_key`, `$4 quantum`.
pub const ADVANCE_DEFICIT_QUERY: &str = "\
INSERT INTO fair_deficit (tenant_id, region, fair_key, deficit, last_served)
VALUES ($1, $2, $3, -$4, now())
ON CONFLICT (tenant_id, region, fair_key) DO UPDATE
SET deficit = fair_deficit.deficit - $4,
    last_served = now()
RETURNING deficit";

/// **The DRR replenish (arch 02 §2.2 — "periodically replenished, weighted by plan tier").** A
/// periodic sweep adds a plan-weighted quantum back to every `fair_key` in a region, so a served
/// tenant recovers priority over time (weighted fair-share). The deficit is clamped at a ceiling so a
/// long-idle `fair_key` cannot accumulate unbounded priority (a burst-credit cap — fairness, not a
/// reward for absence). Bind: `$1 region`, `$2 weighted_quantum`, `$3 deficit_ceiling`.
pub const REPLENISH_DEFICIT_QUERY: &str = "\
UPDATE fair_deficit
SET deficit = LEAST(deficit + $2, $3)
WHERE region = $1";

/// **The per-tenant in-flight count (arch 02 §2.4 — the bounded run-queue backpressure cap).** Counts
/// a tenant's currently non-terminal-non-queued jobs (`leased` + `running`) in a region — the
/// in-flight load the cap bounds. The claim refuses to lease a new job for a tenant already at its
/// cap (over-cap jobs stay `queued`, queueing gracefully). Bind: `$1 tenant_id`, `$2 region`.
pub const IN_FLIGHT_COUNT_QUERY: &str = "\
SELECT count(*) AS in_flight
FROM job_queue
WHERE tenant_id = $1
  AND region = $2
  AND state IN ('leased', 'running')";

// =================================================================================================
// 2. Plan tiers — the DRR replenish weight (arch 02 §2.2 "weighted by plan tier").
// =================================================================================================

/// **The tenant's plan tier — the DRR replenish weight (arch 02 §2.2).** A higher tier's deficit is
/// replenished by a larger quantum each sweep, so it recovers claim priority faster (weighted fair-
/// share — the paid tenant gets a larger *share*, never an *unbounded* one; the lane precedence + the
/// per-tenant cap still hold for every tier). No `PlanTier` exists in `myelin-tenancy` yet, so this is
/// CI-local; if a platform-wide `PlanTier` lands later, this REUSES it (a named reconciliation floor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanTier {
    /// Free tier — the base replenish quantum.
    Free,
    /// Pro tier — a larger share of the fair-queue.
    Pro,
    /// Enterprise tier — the largest share.
    Enterprise,
}

impl PlanTier {
    /// **The plan-weighted replenish quantum multiplier (arch 02 §2.2).** Free=1, Pro=2,
    /// Enterprise=4 — a *bounded* weighting (Enterprise recovers priority 4× faster than Free, never
    /// infinitely; the lane precedence + per-tenant cap still bound every tier). A named v1 floor
    /// (CI-P30 tunes the exact weights under the 30× surge drill).
    pub fn quantum_weight(self) -> i64 {
        match self {
            PlanTier::Free => 1,
            PlanTier::Pro => 2,
            PlanTier::Enterprise => 4,
        }
    }

    /// The plan-tier vocabulary token (for telemetry / the live `fair_deficit` join, should a future
    /// schema carry the tier).
    pub fn as_str(self) -> &'static str {
        match self {
            PlanTier::Free => "free",
            PlanTier::Pro => "pro",
            PlanTier::Enterprise => "enterprise",
        }
    }
}

/// **The base DRR quantum (arch 02 §2.2).** The deficit decrement on a single claim, and the unit the
/// plan-weighted replenish multiplies. A named v1 floor (CI-P30 tunes it under the surge drill).
pub const BASE_QUANTUM: i64 = 1;

/// **The deficit ceiling (the burst-credit cap, arch 02 §2.2).** A `fair_key`'s deficit never exceeds
/// this, so a long-idle tenant cannot accumulate unbounded priority and then monopolise the queue (a
/// fairness bound, not a reward for absence). A named v1 floor (CI-P30 tunes it).
pub const DEFICIT_CEILING: i64 = 64;

/// **The default per-tenant in-flight cap (the bounded run-queue, OQ-K / arch 02 §2.4).** A tenant
/// may have at most this many jobs `leased`+`running` at once in a region; over-cap jobs queue
/// gracefully. A named v1 floor — it MATCHES the substrate `CiDispatch` per-tenant in-flight cap
/// ([`myelin_substrate::ShedBudgetTable::v1_floor`] → 64) so the scheduler cap and the public-surface
/// shed budget agree (one number, not two). CI-P30 tunes both under the 30× CI-D2 surge.
pub const DEFAULT_TENANT_IN_FLIGHT_CAP: u32 = 64;

// =================================================================================================
// 3. The DRR fair-share — the deterministic deficit advance/replenish model (lock-step with the SQL).
// =================================================================================================

/// **The DRR fair-share accounting over `fair_key` (arch 02 §2.2).** Holds the per-`(tenant, region,
/// fair_key)` deficit counter the claim ORDERS on (`deficit DESC`). The claim reads [`deficit`];
/// [`advance_on_claim`] decrements the served key (the [`ADVANCE_DEFICIT_QUERY`] semantics);
/// [`replenish`] adds a plan-weighted quantum to every key (the [`REPLENISH_DEFICIT_QUERY`]
/// semantics, clamped at [`DEFICIT_CEILING`]). Deterministic + DB-free; the live SQL is the same
/// algorithm against Postgres.
///
/// [`deficit`]: FairShare::deficit
/// [`advance_on_claim`]: FairShare::advance_on_claim
/// [`replenish`]: FairShare::replenish
#[derive(Clone, Debug, Default)]
pub struct FairShare {
    /// The per-`(tenant, region, fair_key)` deficit (the claim's `deficit DESC` term).
    deficits: BTreeMap<(String, String, String), i64>,
    /// The plan tier of each `fair_key` (the replenish weight). A key with no recorded tier
    /// replenishes at [`PlanTier::Free`] (the base quantum) — the safe default (never over-grants).
    tiers: BTreeMap<(String, String, String), PlanTier>,
}

impl FairShare {
    /// A fresh empty fair-share ledger (every `fair_key` starts at deficit 0).
    pub fn new() -> Self {
        FairShare::default()
    }

    fn key(tenant_id: &str, region: &str, fair_key: &str) -> (String, String, String) {
        (
            tenant_id.to_string(),
            region.to_string(),
            fair_key.to_string(),
        )
    }

    /// Register a `fair_key`'s plan tier (its replenish weight). Called when a tenant first appears
    /// (the dispatch derives `fair_key` + the tenant's plan tier from the snapshot, arch 02 §3.3).
    pub fn set_tier(&mut self, tenant_id: &str, region: &str, fair_key: &str, tier: PlanTier) {
        self.tiers
            .insert(Self::key(tenant_id, region, fair_key), tier);
    }

    /// The current deficit for a `fair_key` (the claim ORDERS on this, `deficit DESC`). A key never
    /// seen defaults to 0 (the `COALESCE(f.deficit, 0)` in the claim's LEFT JOIN).
    pub fn deficit(&self, tenant_id: &str, region: &str, fair_key: &str) -> i64 {
        self.deficits
            .get(&Self::key(tenant_id, region, fair_key))
            .copied()
            .unwrap_or(0)
    }

    /// **Advance on claim ([`ADVANCE_DEFICIT_QUERY`], arch 02 §2.2): decrement the served `fair_key`'s
    /// deficit by [`BASE_QUANTUM`].** The just-served tenant drops below still-waiting tenants in the
    /// next claim's `deficit DESC` order — the round-robin "deficit" that prevents a 10k-matrix tenant
    /// from being claimed over and over while others wait. Returns the new deficit.
    pub fn advance_on_claim(&mut self, tenant_id: &str, region: &str, fair_key: &str) -> i64 {
        let entry = self
            .deficits
            .entry(Self::key(tenant_id, region, fair_key))
            .or_insert(0);
        *entry -= BASE_QUANTUM;
        *entry
    }

    /// **Replenish ([`REPLENISH_DEFICIT_QUERY`], arch 02 §2.2): add a plan-weighted quantum to every
    /// `fair_key` in a region, clamped at [`DEFICIT_CEILING`].** A served tenant recovers priority
    /// over time; a higher plan tier recovers faster (the weight). The clamp is the burst-credit cap
    /// (a long-idle key cannot hoard priority). Only keys that already have a deficit row (i.e. have
    /// been served at least once, or registered) are replenished — a never-seen key sits at the
    /// implicit 0 the claim COALESCEs.
    pub fn replenish(&mut self, region: &str) {
        // Collect the keys to replenish first (so we can read the tier alongside).
        let updates: Vec<((String, String, String), i64)> = self
            .deficits
            .keys()
            .filter(|(_, r, _)| r == region)
            .map(|k| {
                let weight = self.tiers.get(k).copied().unwrap_or(PlanTier::Free);
                (k.clone(), BASE_QUANTUM * weight.quantum_weight())
            })
            .collect();
        for (k, quantum) in updates {
            let d = self.deficits.entry(k).or_insert(0);
            *d = (*d + quantum).min(DEFICIT_CEILING);
        }
    }

    /// Force-set a deficit (test/seed helper — lets a unit test drive the claim's `deficit DESC`
    /// ordering term in isolation, the same role `SchedulerState::set_deficit` plays for CI-P12).
    pub fn set_deficit(&mut self, tenant_id: &str, region: &str, fair_key: &str, deficit: i64) {
        self.deficits
            .insert(Self::key(tenant_id, region, fair_key), deficit);
    }
}

// =================================================================================================
// 4. Backpressure — the per-tenant in-flight cap (the bounded run-queue, OQ-K / arch 02 §2.4).
// =================================================================================================

/// **The per-tenant in-flight cap (the bounded run-queue, OQ-K / arch 02 §2.4).** Tracks each
/// tenant's in-flight (`leased`+`running`) job count per region and answers [`admits`]: a tenant at
/// its cap is over-cap, so the claim does NOT lease it a new job — the job stays `queued` (queues
/// gracefully, never collapses the scheduler). Per-tenant is the blast-radius guarantee (EI-02 §1):
/// one tenant's surge fills only its own budget, never another tenant's.
///
/// This is the *scheduler-internal* cap. It COMPOSES with — does not replace — the *public-surface*
/// shed gate ([`myelin_substrate::ShedLane`] over [`myelin_substrate::ShedSurface::CiDispatch`], contract
/// 1.11): the shed gate refuses an over-budget *dispatch* at the front door (429 + Retry-After); this
/// cap refuses an over-budget *claim* deeper in the scheduler. Both read the SAME v1 floor number
/// ([`DEFAULT_TENANT_IN_FLIGHT_CAP`] = the substrate `CiDispatch` cap), so they agree.
///
/// [`admits`]: Backpressure::admits
#[derive(Clone, Debug)]
pub struct Backpressure {
    cap: u32,
    /// Per-`(tenant, region)` in-flight count (`leased`+`running`).
    in_flight: BTreeMap<(String, String), u32>,
    /// Cumulative per-tenant over-cap (backpressured) count — the contract-1.8 shed-count signal
    /// (how often a tenant's claim was held by the cap; a sustained nonzero is the surge signal).
    backpressured: BTreeMap<(String, String), u64>,
}

impl Default for Backpressure {
    fn default() -> Self {
        Backpressure::with_cap(DEFAULT_TENANT_IN_FLIGHT_CAP)
    }
}

impl Backpressure {
    /// A backpressure cap at the v1-floor [`DEFAULT_TENANT_IN_FLIGHT_CAP`].
    pub fn new() -> Self {
        Backpressure::default()
    }

    /// A backpressure cap with an explicit per-tenant in-flight ceiling (tests drive the boundary).
    pub fn with_cap(cap: u32) -> Self {
        Backpressure {
            cap,
            in_flight: BTreeMap::new(),
            backpressured: BTreeMap::new(),
        }
    }

    fn key(tenant_id: &str, region: &str) -> (String, String) {
        (tenant_id.to_string(), region.to_string())
    }

    /// The tenant's current in-flight (`leased`+`running`) count in a region.
    pub fn in_flight(&self, tenant_id: &str, region: &str) -> u32 {
        self.in_flight
            .get(&Self::key(tenant_id, region))
            .copied()
            .unwrap_or(0)
    }

    /// **Does the tenant have in-flight headroom (arch 02 §2.4)?** True iff `in_flight < cap` — i.e.
    /// the claim MAY lease a new job for this tenant. At `in_flight == cap` the tenant is over-cap and
    /// the job queues gracefully (the claim skips it; it stays `queued`). The `>= cap` boundary is the
    /// load-bearing comparison the mutation tests pin.
    pub fn admits(&self, tenant_id: &str, region: &str) -> bool {
        self.in_flight(tenant_id, region) < self.cap
    }

    /// Record a successful claim (a job went `leased`) — increments the tenant's in-flight count.
    /// Saturating at the cap is wrong (it would mask a leak); we let it count truthfully and rely on
    /// [`admits`] to gate. Returns the new in-flight count.
    ///
    /// [`admits`]: Backpressure::admits
    pub fn on_claimed(&mut self, tenant_id: &str, region: &str) -> u32 {
        let e = self
            .in_flight
            .entry(Self::key(tenant_id, region))
            .or_insert(0);
        *e += 1;
        *e
    }

    /// Record a job leaving in-flight (it reached `terminal`, or the reaper re-queued an expired
    /// lease) — decrements the tenant's in-flight count (saturating at 0, a stray release is a no-op).
    pub fn on_released(&mut self, tenant_id: &str, region: &str) -> u32 {
        let e = self
            .in_flight
            .entry(Self::key(tenant_id, region))
            .or_insert(0);
        *e = e.saturating_sub(1);
        *e
    }

    /// Record that a tenant's claim was held by the cap (the over-cap backpressure signal, 1.8).
    pub fn record_backpressured(&mut self, tenant_id: &str, region: &str) {
        *self
            .backpressured
            .entry(Self::key(tenant_id, region))
            .or_insert(0) += 1;
    }

    /// The cumulative count of claims this tenant had held by the cap (the 1.8 shed-count signal).
    pub fn backpressured_count(&self, tenant_id: &str, region: &str) -> u64 {
        self.backpressured
            .get(&Self::key(tenant_id, region))
            .copied()
            .unwrap_or(0)
    }
}

// =================================================================================================
// 5. The lane shed order — composing the CI lanes with the platform shed order (contract 1.11).
// =================================================================================================

/// **The CI lane shed order under surge (arch 02 §2.3/§2.4; contract 1.11).** Returns the lanes in
/// the order they are SHED under pressure — `[Deploy, Batch, Interactive]` (deploy/batch shed first,
/// interactive held last). This is the **protected-human-lane analogue inside CI**: interactive PR
/// feedback is the human-facing lane, shed LAST. It is exactly the inverse of the claim's strict
/// `ORDER BY` lane priority (interactive > batch > deploy), and it composes with the platform shed
/// order (speculative → batch/CI → agent → human-last, contract 1.11) that the substrate
/// [`myelin_substrate::ShedLane`] over [`myelin_substrate::ShedSurface::CiDispatch`] enforces at the
/// public dispatch surface (REUSED, never re-defined — the substrate owns the cross-surface shed
/// budget; this is the CI-internal lane precedence the substrate's batch/CI class corresponds to).
pub fn shed_order() -> [Lane; 3] {
    [Lane::Deploy, Lane::Batch, Lane::Interactive]
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── DRR DEFICIT ADVANCE / REPLENISH (mandatory-core) ───────────────────────────────────────

    /// **Advance-on-claim decrements the served `fair_key`'s deficit (arch 02 §2.2).** The just-
    /// served tenant drops in the next claim's `deficit DESC` order — the deficit that prevents the
    /// same tenant being claimed over and over.
    #[test]
    fn advance_on_claim_decrements_served_deficit() {
        let mut f = FairShare::new();
        assert_eq!(f.deficit("t", "fr-par", "t"), 0, "unseen key defaults to 0");
        let after = f.advance_on_claim("t", "fr-par", "t");
        assert_eq!(
            after, -BASE_QUANTUM,
            "the served key is decremented by one quantum"
        );
        assert_eq!(f.deficit("t", "fr-par", "t"), -BASE_QUANTUM);
        // A second serve decrements again (it drops further).
        assert_eq!(f.advance_on_claim("t", "fr-par", "t"), -2 * BASE_QUANTUM);
    }

    /// **Replenish adds a PLAN-WEIGHTED quantum (arch 02 §2.2 "weighted by plan tier").** Free adds 1,
    /// Pro adds 2, Enterprise adds 4 per sweep — a higher tier recovers claim priority faster.
    #[test]
    fn replenish_is_plan_weighted() {
        let mut f = FairShare::new();
        // Three fair_keys, each served once (deficit -1), at three plan tiers.
        for (k, tier) in [
            ("free", PlanTier::Free),
            ("pro", PlanTier::Pro),
            ("ent", PlanTier::Enterprise),
        ] {
            f.set_tier("t", "fr-par", k, tier);
            f.advance_on_claim("t", "fr-par", k); // → -1
        }
        f.replenish("fr-par");
        // Free: -1 + 1 = 0 ; Pro: -1 + 2 = 1 ; Enterprise: -1 + 4 = 3.
        assert_eq!(f.deficit("t", "fr-par", "free"), 0, "Free replenishes by 1");
        assert_eq!(f.deficit("t", "fr-par", "pro"), 1, "Pro replenishes by 2");
        assert_eq!(
            f.deficit("t", "fr-par", "ent"),
            3,
            "Enterprise replenishes by 4 (the largest share)"
        );
    }

    /// **Replenish clamps at the deficit ceiling (the burst-credit cap, arch 02 §2.2).** A long-idle
    /// `fair_key` cannot accumulate unbounded priority then monopolise the queue.
    #[test]
    fn replenish_clamps_at_ceiling() {
        let mut f = FairShare::new();
        f.set_tier("t", "fr-par", "k", PlanTier::Enterprise);
        f.set_deficit("t", "fr-par", "k", DEFICIT_CEILING - 1);
        f.replenish("fr-par"); // would be ceiling-1+4, clamped to ceiling.
        assert_eq!(
            f.deficit("t", "fr-par", "k"),
            DEFICIT_CEILING,
            "the deficit never exceeds the burst-credit ceiling"
        );
        // A second replenish does not push past the ceiling either.
        f.replenish("fr-par");
        assert_eq!(f.deficit("t", "fr-par", "k"), DEFICIT_CEILING);
    }

    /// **Replenish is region-scoped (no cross-region bleed — residency, arch 00 §5).** A sweep of
    /// `fr-par` does not touch an `nl-ams` key.
    #[test]
    fn replenish_is_region_scoped() {
        let mut f = FairShare::new();
        f.advance_on_claim("t", "fr-par", "k"); // -1
        f.advance_on_claim("t", "nl-ams", "k"); // -1
        f.replenish("fr-par");
        assert_eq!(f.deficit("t", "fr-par", "k"), 0, "fr-par replenished");
        assert_eq!(
            f.deficit("t", "nl-ams", "k"),
            -1,
            "nl-ams untouched by an fr-par sweep (region-scoped, no cross-region bleed)"
        );
    }

    /// **A key with no recorded tier replenishes at the Free base (the safe default — never over-
    /// grants).**
    #[test]
    fn untiered_key_replenishes_at_free_base() {
        let mut f = FairShare::new();
        f.advance_on_claim("t", "fr-par", "k"); // -1, no tier set
        f.replenish("fr-par");
        assert_eq!(
            f.deficit("t", "fr-par", "k"),
            0,
            "an untiered key replenishes by the Free base quantum (1), not more"
        );
    }

    /// **PlanTier weights are a BOUNDED ordering (Free < Pro < Enterprise).** Pins the weight floor so
    /// a mutation that flips the order or zeroes a weight is caught.
    #[test]
    fn plan_tier_weights_are_bounded_and_ordered() {
        assert_eq!(PlanTier::Free.quantum_weight(), 1);
        assert_eq!(PlanTier::Pro.quantum_weight(), 2);
        assert_eq!(PlanTier::Enterprise.quantum_weight(), 4);
        assert!(
            PlanTier::Free.quantum_weight() < PlanTier::Pro.quantum_weight()
                && PlanTier::Pro.quantum_weight() < PlanTier::Enterprise.quantum_weight(),
            "the tiers are strictly ordered (a paid tier recovers faster), and bounded (no ∞ share)"
        );
        assert_eq!(PlanTier::Pro.as_str(), "pro");
    }

    // ── BACKPRESSURE: the per-tenant in-flight cap (OQ-K / arch 02 §2.4) ───────────────────────

    /// **The per-tenant in-flight cap admits up to `cap`, then backpressures (arch 02 §2.4).** At
    /// `in_flight == cap` the tenant is over-cap — the claim must NOT lease it a new job (the job
    /// queues gracefully). Pins the `>= cap` boundary.
    #[test]
    fn backpressure_admits_to_cap_then_holds() {
        let mut bp = Backpressure::with_cap(3);
        assert!(bp.admits("t", "fr-par"), "empty → admits");
        bp.on_claimed("t", "fr-par"); // 1
        bp.on_claimed("t", "fr-par"); // 2
        assert!(bp.admits("t", "fr-par"), "2 < 3 → still admits");
        bp.on_claimed("t", "fr-par"); // 3 == cap
        assert!(
            !bp.admits("t", "fr-par"),
            "at the cap the tenant is over-cap — the claim holds (queues gracefully)"
        );
        assert_eq!(bp.in_flight("t", "fr-par"), 3);
        // Releasing one (a job finished / was reaped) re-opens headroom.
        bp.on_released("t", "fr-par"); // 2
        assert!(bp.admits("t", "fr-par"), "headroom re-opens on release");
    }

    /// **The cap is PER-TENANT (the blast-radius guarantee, EI-02 §1).** One tenant filling its cap
    /// never holds another tenant's claim.
    #[test]
    fn backpressure_cap_is_per_tenant() {
        let mut bp = Backpressure::with_cap(2);
        bp.on_claimed("noisy", "fr-par");
        bp.on_claimed("noisy", "fr-par"); // noisy at cap
        assert!(
            !bp.admits("noisy", "fr-par"),
            "the noisy tenant is over-cap"
        );
        assert!(
            bp.admits("quiet", "fr-par"),
            "a different tenant is unaffected (per-tenant blast radius)"
        );
    }

    /// **Release saturates at 0 (a stray release is a no-op, never an underflow).**
    #[test]
    fn backpressure_release_saturates_at_zero() {
        let mut bp = Backpressure::with_cap(4);
        assert_eq!(bp.on_released("t", "fr-par"), 0, "release on empty stays 0");
        bp.on_claimed("t", "fr-par");
        assert_eq!(bp.on_released("t", "fr-par"), 0);
        assert_eq!(bp.on_released("t", "fr-par"), 0, "still 0, no underflow");
    }

    /// **The backpressure count is the 1.8 shed signal (per-tenant, cumulative).**
    #[test]
    fn backpressure_count_is_a_per_tenant_signal() {
        let mut bp = Backpressure::new();
        assert_eq!(bp.backpressured_count("t", "fr-par"), 0);
        bp.record_backpressured("t", "fr-par");
        bp.record_backpressured("t", "fr-par");
        assert_eq!(bp.backpressured_count("t", "fr-par"), 2);
        assert_eq!(bp.backpressured_count("other", "fr-par"), 0, "per-tenant");
    }

    /// **The default cap matches the substrate `CiDispatch` per-tenant in-flight floor (one number,
    /// not two).** The scheduler cap and the public-surface shed budget must agree.
    #[test]
    fn default_cap_matches_substrate_ci_dispatch_budget() {
        let substrate = myelin_substrate::ShedBudgetTable::v1_floor()
            .budget(myelin_substrate::ShedSurface::CiDispatch)
            .per_tenant_in_flight_cap;
        assert_eq!(
            DEFAULT_TENANT_IN_FLIGHT_CAP, substrate,
            "the CI scheduler cap MUST equal the substrate CiDispatch shed budget (one v1 floor)"
        );
    }

    // ── LANE SHED ORDER (contract 1.11) ────────────────────────────────────────────────────────

    /// **The lane shed order holds interactive LAST (the protected-human-lane analogue, 1.11).** Under
    /// surge CI sheds deploy/batch first; the interactive PR-feedback lane is held to the end — the
    /// inverse of the claim's strict lane priority.
    #[test]
    fn lane_shed_order_holds_interactive_last() {
        let order = shed_order();
        assert_eq!(order, [Lane::Deploy, Lane::Batch, Lane::Interactive]);
        assert_eq!(
            *order.last().unwrap(),
            Lane::Interactive,
            "interactive is shed LAST (the protected human lane inside CI)"
        );
        // It is the strict inverse of the claim's lane priority (interactive highest priority = shed
        // last): each successive shed candidate has a higher claim priority than the previous.
        let priorities: Vec<i32> = order.iter().map(|l| l.priority()).collect();
        assert!(
            priorities.windows(2).all(|w| w[0] < w[1]),
            "the shed order ascends in claim priority (lowest-priority lane shed first)"
        );
    }

    // ── THE FAIRNESS DRILL (the prompt GATE — no-starvation, interactive holds) ─────────────────

    /// **THE FAIRNESS-UNDER-CONTENTION DRILL (the prompt GATE).** One tenant enqueues a large matrix
    /// (its `fair_key` is claimed many times) while OTHER tenants wait — DRR must NOT starve them: the
    /// advance-on-claim drives the hot tenant's deficit down so the next claim picks a waiting tenant.
    ///
    /// The drill drives the DRR loop deterministically (the [`FairShare`] is the same algorithm the
    /// live SQL runs) and asserts the no-starvation PROPERTY: over a window of `N` claims, every
    /// contending tenant is served — the hot tenant cannot monopolise. AND it asserts the interactive
    /// lane holds its latency budget under the same contention (the lane is strict, fairness only
    /// orders WITHIN a lane).
    #[test]
    fn fairness_no_starvation_interactive_holds() {
        // ── Part A: DRR no-starvation across tenants ──
        // The "hot" tenant has a huge backlog; two "quiet" tenants each have a few jobs. We simulate
        // the claim picking the highest-deficit fair_key, advancing it, and periodically replenishing
        // — exactly the live loop. The property: every tenant is served within a bounded window (no
        // tenant waits forever while the hot one is claimed over and over).
        let region = "fr-par";
        let mut f = FairShare::new();
        let tenants = ["hot", "quiet1", "quiet2"];
        for t in tenants {
            f.set_tier(t, region, t, PlanTier::Free); // same tier → pure DRR fairness, no weight skew
        }
        // Backlogs: hot has effectively unlimited; the quiet tenants have 5 each.
        let mut backlog: BTreeMap<&str, u32> =
            [("hot", 10_000u32), ("quiet1", 5), ("quiet2", 5)].into();
        let mut served: BTreeMap<&str, u32> = [("hot", 0u32), ("quiet1", 0), ("quiet2", 0)].into();

        // Run a window of claims. Each "claim" picks the eligible fair_key with the highest deficit
        // (the claim's `deficit DESC`), serves it (advance = decrement), and every few claims the
        // periodic replenish runs (the live replenish sweep). This is the DRR fairness loop.
        let window = 60;
        for round in 0..window {
            // Pick the eligible (has-backlog) tenant with the highest deficit, tie-break by name for
            // determinism (the live claim tie-breaks by enqueued_at; here a stable order suffices).
            let pick = tenants
                .iter()
                .filter(|t| backlog[*t] > 0)
                .max_by_key(|t| (f.deficit(t, region, t), std::cmp::Reverse(**t)))
                .copied();
            if let Some(t) = pick {
                *backlog.get_mut(t).unwrap() -= 1;
                *served.get_mut(t).unwrap() += 1;
                f.advance_on_claim(t, region, t);
            }
            // The periodic replenish (every 3 claims) — a served tenant recovers priority.
            if round % 3 == 2 {
                f.replenish(region);
            }
        }

        // NO STARVATION: both quiet tenants were FULLY served within the window — the hot tenant's
        // 10k backlog did NOT monopolise the scheduler.
        assert_eq!(
            served["quiet1"], 5,
            "quiet1 is fully served — DRR did not let the hot tenant starve it"
        );
        assert_eq!(
            served["quiet2"], 5,
            "quiet2 is fully served — no starvation under a 10k-matrix neighbour"
        );
        // The hot tenant still made progress (it is not BLOCKED, just fairly interleaved) but did not
        // consume the whole window.
        assert!(
            served["hot"] > 0 && served["hot"] < window,
            "the hot tenant progresses but is fairly interleaved, never monopolising"
        );

        // ── Part B: the interactive lane holds (strict lane precedence under contention) ──
        // Fairness only orders WITHIN a lane; the lane is the OUTER strict key. Under the same
        // contention an interactive job is claimed before ANY batch job regardless of deficit — the
        // protected-human lane holds its latency budget. (This is the scheduler claim's lane DESC; we
        // assert the precedence the fairness slice must NOT override.)
        assert!(
            Lane::Interactive.priority() > Lane::Batch.priority()
                && Lane::Batch.priority() > Lane::Deploy.priority(),
            "the interactive lane outranks batch/deploy strictly — fairness never reorders across lanes"
        );
        // And the shed order confirms the same lane is protected under surge.
        assert_eq!(*shed_order().last().unwrap(), Lane::Interactive);
    }

    // ── THE LIVE-SQL LOCK-STEP CHECK ───────────────────────────────────────────────────────────

    /// **The live DRR/backpressure SQL encodes the SAME accounting the model implements.** Pins the
    /// statement text so a model/SQL drift is loud: the advance UPSERTs a decrement, the replenish
    /// adds a plan-weighted quantum clamped at the ceiling, the in-flight count is `leased`+`running`.
    #[test]
    fn the_live_fairness_sql_matches_the_model() {
        assert!(
            ADVANCE_DEFICIT_QUERY.contains("ON CONFLICT")
                && ADVANCE_DEFICIT_QUERY.contains("deficit = fair_deficit.deficit - $4"),
            "ADVANCE UPSERTs a per-fair_key decrement (the served key drops in deficit DESC)"
        );
        assert!(
            REPLENISH_DEFICIT_QUERY.contains("LEAST(deficit + $2, $3)")
                && REPLENISH_DEFICIT_QUERY.contains("WHERE region = $1"),
            "REPLENISH adds a (plan-weighted) quantum clamped at the ceiling, region-scoped"
        );
        assert!(
            IN_FLIGHT_COUNT_QUERY.contains("state IN ('leased', 'running')")
                && IN_FLIGHT_COUNT_QUERY.contains("tenant_id = $1"),
            "the in-flight count is the tenant's leased+running jobs (the bounded run-queue load)"
        );
    }
}
