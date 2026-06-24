//! # `surge` — the Search world-scale 30× agent/CI query surge + protected-human-lane shed order
//! (SRCH-P25 / global P-460, M5)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/search-and-indexing.md` §6.3 (agent/CI load &
//! fairness — the query path runs under the principal-aware shed lane: a human's interactive search
//! holds the protected lane; agent/CI search sheds with `429 + Retry-After`; per-tenant in-flight caps
//! keep one tenant's agent storm off another's humans; "Search's query surface is one of [the §7.6
//! OQ-K surfaces]"). §6.1/§6.2 (in-cell, per-tenant, residency-pinned; measure before you shard).
//! **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §3 (the 1×/10×/30× load
//! generator; the multiplier is read from the FROZEN thresholds file, never hardcoded; never weaken a
//! threshold to pass — a red is a dated `claimed-not-proven` row), §2 (the protected human lane;
//! per-tenant blast-radius). **Contract-index:** row **1.11** (the protected-human-lane shed order +
//! per-surface shed budgets OQ-K — Search's query surface is one lane), row **1.8** (the per-lane
//! shed-count telemetry). **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-
//! catalogue.md` SRCH-D6 (30× agent/CI query surge → human search lane holds, agent sheds, others
//! unaffected).
//!
//! ## What this module is (the Search surge half — SRCH-P25)
//! Search has ONE public query surface under the 30× surge (SRCH-D6): the permission-filtered search
//! **QUERY**. The 30× surge is **agent/CI search** (an agent fan-out + a CI run-log query storm). This
//! module tunes the doctrine shed order (`speculative → batch/CI → agent → human-last`) to Search's
//! query surface:
//! - a **human's interactive search** holds the protected lane (shed last, latency within budget);
//! - an **agent/CI search** lane sheds with `429 + Retry-After` (honoured — our ResilientClient
//!   honours `Retry-After`, P-S17, so a shed is not a retry-storm amplifier);
//! - **per-tenant in-flight caps** keep one tenant's agent/CI query storm off another tenant's humans
//!   (the per-tenant bulkhead, §6.1/§6.2 / EI-02 §1).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! **The shed order itself is the substrate's** [`myelin_substrate::shed`]: this module does NOT
//! re-author the shed lane / run-class / budget table (that would be a doctrinal fork — the same
//! mistake [`myelin_git::shed_clone`]/[`myelin_refs_service::surge`] avoided for the Git front door /
//! Refs surfaces). It **WIRES** the existing [`myelin_substrate::shed::ShedLane`] over the ONE new
//! [`Surface::SearchQuery`] surface, reading the budget **from the thresholds file**
//! ([`myelin_substrate::thresholds`]) — the tuned OQ-K numbers, never a hardcoded magic value. The
//! Search surge gate's only authoring is the *derivation* of the request's [`RunClass`] from its
//! principal + an optional run-class header, and the placement of the admit/shed decision at the FRONT
//! of the query pipeline.
//!
//! ## The zero-escape invariant is UNTOUCHED under shed (§4.2)
//! Shedding is a **pre-query** admission decision: it refuses an over-budget query cheaply, BEFORE any
//! `list_objects` permission pre-filter resolution / index traversal runs. So a shed query returns a
//! `429` and never a partial/leaked result — the permission pre-filter still gates every query that IS
//! admitted (the §4.2 leak-free pre-filter is the crux, and the surge never relaxes it; it only bounds
//! concurrency). This mirrors the Refs `RefsShedGate` invariant exactly.
//!
//! ## Sharding edge — measured-only (§6.2)
//! Search is in-cell, per-tenant, residency-pinned (§6.1); "measure before you shard" (§6.2): the
//! first moves are the filter/result cache, then more embedded index nodes per cell, then a
//! per-subsystem index split for a hot tenant — a MEASURED-volume promotion, never pre-sharded. This
//! prompt does NOT pre-shard ([`SHARD_SPLIT_IS_MEASURED_ONLY`]): the surge proves the per-tenant shed
//! order holds at one cell; a split is taken only when measured, never predicted (EI-01 §3).
//!
//! ## Floors named (VISION §3 — name your floors)
//! - **The tuned filtered-ANN strategy + the HNSW↔IVF-PQ promotion** (the vector hot path's
//!   memory-pressure upgrade) is **SRCH-P26** ([`FILTERED_ANN_FOLLOW_ON`]): this prompt is the
//!   surge/shed-order half ONLY.
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet,
//!   testing-strategy §4.1). Here the load is the P-S02 generator at 30× across the surging tenant; the
//!   per-tenant fairness + shed-order + cross-tenant-0 PROPERTIES are complete + testable now and do
//!   not change shape when the real index carries the load.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2/§3)
//! The shed-order DECISION path ([`SearchShedGate::admit_for`]/[`SearchShedGate::admit_class`] → the
//! human-protected per-tenant graded admit) is mandatory-core: an off-by-one that sheds a human before
//! an agent, or that leaks one tenant's budget into another, is the failure this exists to catch.

use myelin_identity::Principal;
use myelin_substrate::shed::{
    RunClass, RunClassHeader, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// **The Search surge default-to-beat multiplier (SRCH-D6).** The 30× world-scale surge factor the
/// SRCH-D6 drill drives at — read from the FROZEN thresholds file `[surge] multiplier` row (the
/// versioned source of truth, P-038) and asserted to equal this documented default-to-beat; a
/// divergence is a LOUD failure, never a silent weakening (EI-01 §3).
pub const SEARCH_SURGE_MULTIPLIER: u32 = 30;

/// **The filtered-ANN / HNSW↔IVF-PQ promotion follow-on (the named vector hot-path floor).** This
/// prompt (SRCH-P25) is the surge/shed-order half ONLY; the tuned filter-during-traversal strategy +
/// the measured HNSW↔IVF-PQ promotion point (SRCH-D8 recall@k) is **SRCH-P26**. Named here so the
/// floor is explicit, not implied.
pub const FILTERED_ANN_FOLLOW_ON: &str = "SRCH-P26";

/// **The shard split is MEASURED-ONLY (§6.2 / ADR-10).** Search is per-tenant in-cell; a hot tenant is
/// promoted by the §6.2 measure-before-you-shard ladder (cache → more index nodes → per-subsystem
/// split), never pre-sharded. This prompt does NOT pre-shard — a split is taken only when the
/// utilisation telemetry MEASURES a tenant past its cell's headroom. `true` records that the branch is
/// measured-gated (the surge proves the per-tenant shed order holds at one cell first).
pub const SHARD_SPLIT_IS_MEASURED_ONLY: bool = true;

// ───────────────────────────── the Search surge shed gate ────────────────────────────────────────

/// **The protected-human-lane shed gate at the Search query surface (SRCH-P25 / OQ-K; contract
/// 1.11).**
///
/// A thin Search wiring over the substrate's [`ShedLane`] for the ONE Search surface
/// ([`Surface::SearchQuery`]): it reads the surface's budget **from the thresholds file** and applies
/// the shed order `speculative → batch/CI → agent → human-last`, per-tenant. A search query is
/// admitted through [`SearchShedGate::admit_for`] (the run-class derived from the verified principal);
/// an over-budget non-human lane is shed with `429 + Retry-After`, while the human lane is protected
/// (shed only in true saturation). The decision is a pre-query admission — a shed query never runs a
/// `list_objects` resolution, so it cannot leak (§4.2).
pub struct SearchShedGate {
    lane: ShedLane,
}

/// **Why a search query was refused at the shed gate** — the typed form the transport maps to the wire
/// `429`. A shed carries the `Retry-After` (seconds) the client honours (the no-amplification
/// guarantee — our ResilientClient honours `Retry-After`, so a shed is not a retry-storm amplifier).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchShedRejection {
    /// The lane that was shed (`speculative` / `batch_ci` / `agent` / `human`) — the contract-1.8
    /// per-lane shed-count signal keys on this.
    pub lane: RunClass,
    /// The `Retry-After` value in **seconds** (the frozen §2.10 unit) the transport sets on the
    /// `429 Too Many Requests` response.
    pub retry_after_secs: u64,
}

impl SearchShedGate {
    /// Open the Search query surge gate, reading its budget **from the thresholds file** (the prompt's
    /// "the shed budget is read from the thresholds file"). A missing row is a LOUD error (the gate
    /// refuses to open against a guessed budget — EI-01 §3), never a silent default.
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<SearchShedGate, String> {
        let budget = thresholds
            .shed_budget(Surface::SearchQuery)
            .map_err(|e| format!("Search shed budget for SearchQuery unavailable: {e}"))?;
        Ok(SearchShedGate {
            lane: ShedLane::with_budget(Surface::SearchQuery, budget),
        })
    }

    /// Open the gate against an explicit budget (used by the surge drill to drive the boundary at a
    /// small, deterministic budget without editing the thresholds file).
    pub fn with_budget(budget: SurfaceBudget) -> SearchShedGate {
        SearchShedGate {
            lane: ShedLane::with_budget(Surface::SearchQuery, budget),
        }
    }

    /// **Admit a search query by its verified principal + an optional injected run-class header.** The
    /// run-class is DERIVED ([`RunClass::derive`]) from `principal.kind` (the kind sets the ceiling)
    /// and the header (which may only down-class) — a machine principal can NEVER up-class to the
    /// protected human lane. Returns `Ok(class)` admitted (a slot was taken — release it on completion
    /// via [`SearchShedGate::release`]) or `Err(SearchShedRejection)` shed (`429 + Retry-After`). The
    /// decision is per-`principal.tenant`.
    pub fn admit_for(
        &mut self,
        principal: &Principal,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, SearchShedRejection> {
        let class = RunClass::derive(&principal.kind, header);
        self.admit_class(&principal.tenant, class).map(|()| class)
    }

    /// **Admit a query of a pre-derived [`RunClass`] for `tenant`.** The lower-level form the surge
    /// drill drives. Returns `Ok(())` admitted (a slot taken) or `Err(SearchShedRejection)` shed. The
    /// human lane is protected: a human is shed ONLY when every slot (the reserved human fraction
    /// included) is full; the non-human lanes shed first, in the graded order
    /// `speculative → batch/CI → agent`.
    pub fn admit_class(
        &mut self,
        tenant: &TenantId,
        class: RunClass,
    ) -> Result<(), SearchShedRejection> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(SearchShedRejection {
                lane: class,
                retry_after_secs,
            }),
        }
    }

    /// Release a slot a prior admit took for `(tenant, class)` — call when the query completes so the
    /// lane recovers after the surge.
    pub fn release(&mut self, tenant: &TenantId, class: RunClass) {
        self.lane.release(tenant, class);
    }

    /// The cumulative shed count for a lane (the contract-1.8 `shed-count per lane` survival signal —
    /// the surge-drill green artifact: `human-lane == 0 shed`, `agent-lane > 0 shed`).
    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.lane.shed_count(class)
    }

    /// The per-tenant in-flight count (admitted not yet released) — for the blast-radius assertions.
    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.lane.in_flight(tenant)
    }

    /// The surface this gate fronts (always [`Surface::SearchQuery`]).
    pub fn surface(&self) -> Surface {
        self.lane.surface()
    }
}

// ───────────────────────────── the SRCH-D6 surge report ──────────────────────────────────────────

/// **The SRCH-D6 30× surge report — the three properties on the Search query surface.** The dated
/// green artifact the DoD names: the human search lane HOLDS (0 shed within its reserved slots while a
/// machine lane sheds), the agent/CI query lane SHEDS (`429 + Retry-After`, absorbed not unbounded),
/// and other tenants are UNAFFECTED (the storm fills only the surging tenant's per-tenant budget).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchSurgeReport {
    /// The agent-lane shed count on the surging tenant (the storm absorbed by shedding — must be > 0).
    pub surging_agent_shed_count: u64,
    /// The CI/batch-lane shed count on the surging tenant (the CI run-log query storm — must be > 0).
    pub surging_ci_shed_count: u64,
    /// The human-lane shed count on the surging tenant (the protected lane — must be 0).
    pub surging_human_shed_count: u64,
    /// Whether the surging tenant's OWN human search was admitted within its reserved slots.
    pub surging_human_admitted: bool,
    /// Whether the quiet co-tenant's human search was admitted within budget (untouched).
    pub quiet_human_admitted: bool,
    /// The quiet co-tenant's in-flight count BEFORE its own human query (the cross-tenant impact — must
    /// be 0; the storm never spends the quiet tenant's budget).
    pub cross_tenant_impact: u32,
}

impl SearchSurgeReport {
    /// **The SRCH-D6 GREEN predicate (the three properties — all measured, none weakened).** The
    /// agent + CI machine lanes shed (absorbed by shedding), the human search lane held (0 shed on the
    /// surging tenant + its own human admitted), the quiet co-tenant's human held, and cross-tenant
    /// impact is 0.
    pub fn is_srch_d6_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.surging_ci_shed_count > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
    }

    /// A one-line summary for the dated green-artifact log row (observability is part of the pass).
    pub fn summary(&self) -> String {
        format!(
            "SRCH-D6: surging agent_shed={} ci_shed={} human_shed={} surging_human_admitted={} \
             quiet_human_admitted={} cross_tenant_impact={} → {}",
            self.surging_agent_shed_count,
            self.surging_ci_shed_count,
            self.surging_human_shed_count,
            self.surging_human_admitted,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
            if self.is_srch_d6_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

/// **Drive the SRCH-D6 30× agent/CI query surge on the Search query gate.** Spreads `storm_agent_ops`
/// agent search queries + `storm_ci_ops` CI/batch search queries on the surging tenant — the machine
/// lanes fill then shed — then proves the surging tenant's OWN human search is still admitted
/// (shed-last) and a quiet co-tenant's human search is admitted within its independent budget. Returns
/// the [`SearchSurgeReport`] (the three properties).
///
/// The `multiplier` is the surge factor (read from the FILE by the caller; passed through for the log
/// row), not used to scale here — the storm-op counts are already the derived 30× storm-op counts.
pub fn run_search_surge(
    gate: &mut SearchShedGate,
    surging: &TenantId,
    quiet: &TenantId,
    storm_agent_ops: u64,
    storm_ci_ops: u64,
    _multiplier: u32,
) -> SearchSurgeReport {
    // Drive the agent + CI search-query storm on the surging tenant: the machine lanes fill their
    // non-reserved budget then shed (429 + Retry-After) — absorbed by shedding, never unbounded. The
    // CI (batch) lane is held to a TIGHTER graded ceiling than the agent lane, so the CI run-log query
    // storm sheds first (speculative → batch/CI → agent → human-last).
    for _ in 0..storm_ci_ops {
        let _ = gate.admit_class(surging, RunClass::BatchCi);
    }
    for _ in 0..storm_agent_ops {
        let _ = gate.admit_class(surging, RunClass::Agent);
    }

    // The surging tenant's OWN human search is STILL admitted — the protected lane, shed last (a human
    // uses the reserved slots the agent/CI storm could never take).
    let surging_human_admitted = gate.admit_class(surging, RunClass::Human).is_ok();

    // The quiet co-tenant is UNTOUCHED: its human search is admitted within its independent per-tenant
    // budget (the storm never spent the quiet tenant's slots).
    let quiet_in_flight_before = gate.in_flight(quiet);
    let quiet_human_admitted = gate.admit_class(quiet, RunClass::Human).is_ok();

    SearchSurgeReport {
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_ci_shed_count: gate.shed_count(RunClass::BatchCi),
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        quiet_human_admitted,
        cross_tenant_impact: quiet_in_flight_before,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef};
    use myelin_tenancy::Region;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.to_string())
    }

    fn human(tenant_slug: &str) -> Principal {
        Principal::new(
            tenant(tenant_slug),
            Region("fr-par".into()),
            PrincipalId(format!("h-{tenant_slug}")),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn agent(tenant_slug: &str) -> Principal {
        Principal::new(
            tenant(tenant_slug),
            Region("fr-par".into()),
            PrincipalId(format!("a-{tenant_slug}")),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt".into()),
                on_behalf_of: None,
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    /// cap 6, reserve 2 → non-human budget 4; step = max(4/8,1)=1 → speculative ceiling 2, batch 3,
    /// agent 4. A small deterministic budget so the graded thresholds are easy to reach.
    fn small_budget() -> SurfaceBudget {
        SurfaceBudget {
            per_tenant_in_flight_cap: 6,
            human_lane_reservation: 2,
            retry_after_secs: 3,
        }
    }

    // ───────────────────────── the shed budget is read from the file ─────────────────────────

    /// **The Search shed budget is read from the thresholds file** (the prompt's explicit
    /// requirement). The gate opens against the canonical `thresholds.toml` `[[shed_budgets]]` row for
    /// `SearchQuery` — not a hardcoded number. A missing row would have been a loud error.
    #[test]
    fn the_search_shed_budget_is_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let gate =
            SearchShedGate::from_thresholds(&thresholds).expect("SearchQuery budget present");
        assert_eq!(gate.surface(), Surface::SearchQuery);

        let b = thresholds
            .shed_budget(Surface::SearchQuery)
            .expect("present");
        assert!(b.per_tenant_in_flight_cap > 0, "SearchQuery bounded (§7.1)");
        assert!(
            b.human_lane_reservation > 0,
            "SearchQuery reserves a human lane"
        );
        // the surge multiplier in the file matches the documented default-to-beat (never hardcoded).
        assert_eq!(thresholds.surge.multiplier, SEARCH_SURGE_MULTIPLIER);
    }

    /// **The shed order serves the human while the agent/CI lane sheds (SRCH-D6):** the human search is
    /// SERVED while the agent search lane SHEDS (`429 + Retry-After`).
    #[test]
    fn shed_order_serves_the_human_while_the_agent_lane_sheds() {
        let mut gate = SearchShedGate::with_budget(small_budget());
        let a = agent("acme");
        let h = human("acme");

        // an agent search-query storm fills the non-human budget (cap-reserved = 4) then sheds.
        for _ in 0..4 {
            assert!(
                gate.admit_for(&a, None).is_ok(),
                "agent query admitted under budget"
            );
        }
        let shed = gate.admit_for(&a, None).expect_err("the agent storm sheds");
        assert_eq!(shed.lane, RunClass::Agent);
        assert_eq!(shed.retry_after_secs, 3, "the shed carries a Retry-After");

        // THE GATE: the HUMAN's interactive search is STILL SERVED (shed last).
        assert_eq!(
            gate.admit_for(&h, None)
                .expect("the human is served while the agent sheds"),
            RunClass::Human
        );
        assert_eq!(gate.shed_count(RunClass::Human), 0, "human lane: 0 shed");
        assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
    }

    /// **The full shed PRIORITY order: speculative → batch/CI → agent → human-last** (the CI run-log
    /// query lane sheds before the agent lane sheds before the human).
    #[test]
    fn shed_priority_is_speculative_then_batch_then_agent_then_human() {
        let mut gate = SearchShedGate::with_budget(small_budget());
        let t = tenant("acme");
        for _ in 0..2 {
            gate.admit_class(&t, RunClass::Agent)
                .expect("agent admitted");
        }
        assert!(
            gate.admit_class(&t, RunClass::Speculative).is_err(),
            "speculative sheds first"
        );
        gate.admit_class(&t, RunClass::BatchCi)
            .expect("batch admitted"); // non_human → 3
        assert!(
            gate.admit_class(&t, RunClass::BatchCi).is_err(),
            "batch/CI (the CI run-log query lane) sheds next"
        );
        gate.admit_class(&t, RunClass::Agent)
            .expect("agent admitted"); // non_human → 4
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "agent sheds before the human"
        );
        gate.admit_class(&t, RunClass::Human)
            .expect("human served — shed last");

        assert_eq!(gate.shed_count(RunClass::Speculative), 1);
        assert_eq!(gate.shed_count(RunClass::BatchCi), 1);
        assert_eq!(gate.shed_count(RunClass::Agent), 1);
        assert_eq!(gate.shed_count(RunClass::Human), 0);
    }

    /// **Per-tenant: one tenant's agent query storm NEVER sheds another tenant's human (blast-radius).**
    #[test]
    fn one_tenants_storm_never_sheds_anothers_human() {
        let mut gate = SearchShedGate::with_budget(small_budget());
        let noisy = agent("noisy");
        let quiet_human = human("quiet");

        for _ in 0..4 {
            gate.admit_for(&noisy, None).expect("noisy agent admitted");
        }
        assert!(
            gate.admit_for(&noisy, None).is_err(),
            "noisy agent lane sheds"
        );
        assert_eq!(gate.in_flight(&tenant("noisy")), 4, "noisy has 4 in-flight");
        assert_eq!(
            gate.in_flight(&tenant("quiet")),
            0,
            "the quiet tenant's budget is independent"
        );
        assert_eq!(
            gate.admit_for(&quiet_human, None)
                .expect("the quiet human is served"),
            RunClass::Human,
            "the noisy storm must NEVER shed another tenant's human"
        );
    }

    /// **A machine principal can NEVER up-class to the human search lane** (structurally unspoofable).
    #[test]
    fn a_machine_principal_cannot_spoof_the_human_lane() {
        let mut gate = SearchShedGate::with_budget(small_budget());
        let a = agent("acme");
        assert_eq!(gate.admit_for(&a, None).expect("admitted"), RunClass::Agent);
        let h = human("acme");
        assert_eq!(
            gate.admit_for(&h, Some(RunClassHeader::Speculative))
                .expect("admitted"),
            RunClass::Speculative,
            "a human-issued prefetch search may down-class itself"
        );
    }

    /// Release frees a slot so the lane recovers after the surge passes.
    #[test]
    fn release_frees_a_slot_after_the_surge() {
        let mut gate = SearchShedGate::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 1,
            retry_after_secs: 1,
        });
        let t = tenant("acme");
        gate.admit_class(&t, RunClass::Agent).expect("admitted");
        gate.admit_class(&t, RunClass::Agent).expect("admitted"); // non_human 2 == cap-reserved
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "agent sheds"
        );
        gate.release(&t, RunClass::Agent);
        gate.admit_class(&t, RunClass::Agent)
            .expect("a released slot is reusable");
    }

    /// **The SRCH-D6 surge report is GREEN under a real storm** (the three properties).
    #[test]
    fn run_search_surge_is_green() {
        let mut gate = SearchShedGate::with_budget(small_budget());
        let surging = tenant("noisy");
        let quiet = tenant("quiet");
        // a storm well past the non-human budget (4) so both machine lanes MUST shed.
        let report = run_search_surge(&mut gate, &surging, &quiet, 50, 50, SEARCH_SURGE_MULTIPLIER);
        assert!(report.is_srch_d6_green(), "{}", report.summary());
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(
            report.surging_ci_shed_count > 0,
            "CI run-log query lane shed"
        );
        assert_eq!(report.surging_human_shed_count, 0, "human lane held");
        assert!(report.surging_human_admitted, "surging tenant's human held");
        assert!(report.quiet_human_admitted, "quiet co-tenant's human held");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    }

    /// **The surge gate is NOT vacuous — an UNBOUNDED lane (no shed) reads RED.**
    #[test]
    fn an_unbounded_lane_reads_red() {
        let huge = SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 200_000,
            retry_after_secs: 3,
        };
        let mut gate = SearchShedGate::with_budget(huge);
        let report = run_search_surge(
            &mut gate,
            &tenant("noisy"),
            &tenant("quiet"),
            100,
            100,
            SEARCH_SURGE_MULTIPLIER,
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "the unbounded lane swallowed the storm"
        );
        assert!(
            !report.is_srch_d6_green(),
            "an unbounded lane MUST read RED"
        );
    }

    /// The floors are named (SRCH-P26 filtered-ANN follow-on + the measured-only shard branch).
    #[test]
    fn the_floors_are_named() {
        assert_eq!(FILTERED_ANN_FOLLOW_ON, "SRCH-P26");
        let measured_only = SHARD_SPLIT_IS_MEASURED_ONLY;
        assert!(measured_only, "a shard split is measured-only (§6.2)");
        assert_eq!(SEARCH_SURGE_MULTIPLIER, 30);
    }
}
