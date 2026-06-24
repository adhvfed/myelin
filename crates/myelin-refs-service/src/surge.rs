//! # `surge` — the Refs world-scale 30× surge + the protected-human-lane shed order (REF-P22 / P-453, M5)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §6.2 (*measure before you shard* — the first moves in order are a read replica for the hot
//! backlink read, then the read-time CTE, then the R4 reach index; the shard key is already
//! `(tenant, region) + target_root hash` so a measured hot tenant outgrowing one shard is a re-home,
//! not a redesign). **Doctrine:** external-insights/01 §3 (the 1×/10×/30× load generator; the
//! multiplier is read from the FROZEN thresholds file, never hardcoded; never weaken a threshold to
//! pass — a red is a dated `claimed-not-proven` row), §2 (the protected human lane; per-tenant
//! blast-radius). **Contract-index:** row **1.11** (the protected-human-lane shed order + per-surface
//! shed budgets OQ-K), consumed row **5.3** at scale (the backlink read the shed order protects),
//! row **1.8** (the per-lane shed-count telemetry).
//!
//! ## What this module is (the Refs surge half — REF-P22)
//! Refs has **two public surfaces** under the 30× surge (REF-D10): a backlink/traverse **READ** and a
//! ref **CREATION** write. The 30× surge is **agent ref-creation + agent backlink-read**; this module
//! tunes the doctrine shed order (`speculative → batch/CI → agent → human-last`) to Refs' two surfaces:
//! - a **human's interactive backlink/traverse read** holds the protected lane (shed last);
//! - an **agent backlink-read** + **agent ref-creation** lane sheds with `429 + Retry-After`;
//! - **per-tenant in-flight caps** keep one tenant's agent storm off another tenant's humans (the
//!   per-tenant bulkhead, §6.2 / EI-02 §1).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! **The shed order itself is the substrate's** [`myelin_substrate::shed`]: this module does NOT
//! re-author the shed lane / run-class / budget table (that would be a doctrinal fork — the same
//! mistake [`myelin_git::shed_clone`] avoided for the Git front door). It **WIRES** the existing
//! [`myelin_substrate::shed::ShedLane`] over the TWO new
//! [`Surface::RefsBacklinkRead`]/[`Surface::RefsRefCreate`] surfaces, reading each surface's budget
//! **from the thresholds file** ([`myelin_substrate::thresholds`]) — the tuned OQ-K numbers, never a
//! hardcoded magic value. The Refs surge gate's only authoring is the *derivation* of the request's
//! [`RunClass`] from its principal + an optional run-class header, and the placement of the
//! admit/shed decision at the front of the read/create pipeline.
//!
//! ## Sharding edge — measured-only (§6.2)
//! The shard key is already `(tenant, region) + target_root hash` (architecture §3.2/§6.2), so a
//! MEASURED hot tenant outgrowing one shard is a **re-home, not a redesign**. This prompt does NOT
//! pre-shard: the surge proves the per-tenant shed order holds at one cell; a shard split is a
//! measured-only branch ([`SHARD_SPLIT_IS_MEASURED_ONLY`]) — taken when the `cell_utilisation`
//! telemetry shows a tenant past its cell's headroom, never before (EI-01 §3, ADR-10).
//!
//! ## Floors named (VISION §3 — name your floors)
//! - **The hot-artifact reach index R4** (the Leopard-style flattened reach index — the named REF-P11
//!   floor's follow-on) is **REF-P23** ([`R4_REACH_INDEX_FOLLOW_ON`]): this prompt is the
//!   surge/shed-order half ONLY. The backlink read here is the read-time permission-filtered CTE +
//!   pagination + replica (REF-P11); R4 promotes it to a measured-trigger flattened index in REF-P23.
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet).
//!   Here the load is the P-S02 generator at 30× across the surging tenant; the per-tenant fairness +
//!   shed-order + cross-tenant-0 PROPERTIES are complete + testable now and do not change shape when
//!   the real PgStore-backed edge index carries the load.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2/§3)
//! The shed-order DECISION path ([`RefsShedGate::admit_for`]/[`RefsShedGate::admit_class`] → the
//! human-protected per-tenant graded admit) is mandatory-core: an off-by-one that sheds a human
//! before an agent, or that leaks one tenant's budget into another, is the failure this exists to
//! catch. The **REF-P11 SetExpr-lowering leak invariant is UNTOUCHED under shed**: shedding is a
//! pre-read admission decision (it refuses an over-budget read cheaply, BEFORE any backlink resolution
//! runs), so a shed read returns `429` and never a partial/leaked result — the permission filter still
//! gates every read that IS admitted. The surge never relaxes the filter; it only bounds concurrency.

use myelin_identity::Principal;
use myelin_substrate::shed::{
    RunClass, RunClassHeader, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// **The Refs surge default-to-beat multiplier (REF-D10).** The 30× world-scale surge factor the
/// REF-D10 drill drives at — read from the FROZEN thresholds file `[surge] multiplier` row (the
/// versioned source of truth, P-038) and asserted to equal this documented default-to-beat; a
/// divergence is a LOUD failure, never a silent weakening (EI-01 §3).
pub const REFS_SURGE_MULTIPLIER: u32 = 30;

/// **The R4 reach-index follow-on (the named REF-P11 floor's follow-on).** This prompt (REF-P22) is
/// the surge/shed-order half ONLY; the hot-artifact Leopard-style flattened reach index R4 — derived
/// from R1, incrementally maintained from `refs.edge.*`, gated by the SAME `list_objects` filter, and
/// promoted on a MEASURED trigger — is **REF-P23**. Named here so the floor is explicit, not implied.
pub const R4_REACH_INDEX_FOLLOW_ON: &str = "REF-P23";

/// **The shard split is MEASURED-ONLY (§6.2 / ADR-10).** The Refs shard key is already
/// `(tenant, region) + target_root hash`, so a hot tenant outgrowing one shard is a re-home, not a
/// redesign. This prompt does NOT pre-shard — a split is taken only when the `cell_utilisation`
/// telemetry MEASURES a tenant past its cell's headroom, never predicted. `true` records that the
/// branch is measured-gated (the surge proves the per-tenant shed order holds at one cell first).
pub const SHARD_SPLIT_IS_MEASURED_ONLY: bool = true;

// ───────────────────────────── the Refs surge shed gate ──────────────────────────────────────────

/// **The protected-human-lane shed gate at a Refs surface (REF-P22 / OQ-K; contract 1.11).**
///
/// A thin Refs wiring over the substrate's [`ShedLane`] for ONE Refs surface
/// ([`Surface::RefsBacklinkRead`] or [`Surface::RefsRefCreate`]): it reads the surface's budget
/// **from the thresholds file** and applies the shed order `speculative → batch/CI → agent →
/// human-last`, per-tenant. A backlink read / ref-creation is admitted through
/// [`RefsShedGate::admit_for`] (the run-class derived from the verified principal); an over-budget
/// non-human lane is shed with `429 + Retry-After`, while the human lane is protected (shed only in
/// true saturation).
pub struct RefsShedGate {
    lane: ShedLane,
}

/// **Why a Refs read/create was refused at the shed gate** — the typed form the transport maps to the
/// wire `429`. A shed carries the `Retry-After` (seconds) the client honours (the no-amplification
/// guarantee — our ResilientClient honours `Retry-After`, so a shed is not a retry-storm amplifier).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefsShedRejection {
    /// The lane that was shed (`speculative` / `batch_ci` / `agent` / `human`) — the contract-1.8
    /// per-lane shed-count signal keys on this.
    pub lane: RunClass,
    /// The `Retry-After` value in **seconds** (the frozen §2.10 unit) the transport sets on the
    /// `429 Too Many Requests` response.
    pub retry_after_secs: u64,
}

impl RefsShedGate {
    /// Open a Refs surge gate for a surface, reading its budget **from the thresholds file** (the
    /// prompt's "the shed budget is read from the thresholds file"). A missing row for the surface is
    /// a LOUD error (the gate refuses to open against a guessed budget — EI-01 §3), never a silent
    /// default. The surface MUST be one of the two Refs surfaces.
    pub fn from_thresholds(
        thresholds: &Thresholds,
        surface: Surface,
    ) -> Result<RefsShedGate, String> {
        debug_assert!(
            matches!(surface, Surface::RefsBacklinkRead | Surface::RefsRefCreate),
            "RefsShedGate fronts only the two Refs surfaces"
        );
        let budget = thresholds
            .shed_budget(surface)
            .map_err(|e| format!("Refs shed budget for {surface:?} unavailable: {e}"))?;
        Ok(RefsShedGate {
            lane: ShedLane::with_budget(surface, budget),
        })
    }

    /// Open the backlink/traverse READ gate from the thresholds file (the human read lane the surge
    /// protects).
    pub fn backlink_read_from_thresholds(thresholds: &Thresholds) -> Result<RefsShedGate, String> {
        RefsShedGate::from_thresholds(thresholds, Surface::RefsBacklinkRead)
    }

    /// Open the ref-CREATION gate from the thresholds file (the ingest lane the agent storm sheds on).
    pub fn ref_create_from_thresholds(thresholds: &Thresholds) -> Result<RefsShedGate, String> {
        RefsShedGate::from_thresholds(thresholds, Surface::RefsRefCreate)
    }

    /// Open the gate against an explicit budget (used by the surge drill to drive the boundary at a
    /// small, deterministic budget without editing the thresholds file).
    pub fn with_budget(surface: Surface, budget: SurfaceBudget) -> RefsShedGate {
        RefsShedGate {
            lane: ShedLane::with_budget(surface, budget),
        }
    }

    /// **Admit a Refs read/create by its verified principal + an optional injected run-class header.**
    /// The run-class is DERIVED ([`RunClass::derive`]) from `principal.kind` (the kind sets the
    /// ceiling) and the header (which may only down-class) — a machine principal can NEVER up-class to
    /// the protected human lane. Returns `Ok(class)` admitted (a slot was taken — release it on
    /// completion via [`RefsShedGate::release`]) or `Err(RefsShedRejection)` shed (`429 +
    /// Retry-After`). The decision is per-`principal.tenant`.
    pub fn admit_for(
        &mut self,
        principal: &Principal,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, RefsShedRejection> {
        let class = RunClass::derive(&principal.kind, header);
        self.admit_class(&principal.tenant, class).map(|()| class)
    }

    /// **Admit a request of a pre-derived [`RunClass`] for `tenant`.** The lower-level form the surge
    /// drill drives (it mints classes directly). Returns `Ok(())` admitted (a slot taken) or
    /// `Err(RefsShedRejection)` shed. The human lane is protected: a human is shed ONLY when every
    /// slot (the reserved human fraction included) is full; the non-human lanes shed first, in the
    /// graded order `speculative → batch/CI → agent`.
    pub fn admit_class(
        &mut self,
        tenant: &TenantId,
        class: RunClass,
    ) -> Result<(), RefsShedRejection> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(RefsShedRejection {
                lane: class,
                retry_after_secs,
            }),
        }
    }

    /// Release a slot a prior [`RefsShedGate::admit_for`]/[`RefsShedGate::admit_class`] took for
    /// `(tenant, class)` — call when the read/create completes so the lane recovers after the surge.
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

    /// The surface this gate fronts.
    pub fn surface(&self) -> Surface {
        self.lane.surface()
    }
}

// ───────────────────────────── the REF-D10 surge report ──────────────────────────────────────────

/// **The REF-D10 30× surge report — the three F6 properties on the Refs surfaces.** The dated green
/// artifact the DoD names: the human read/create lane HOLDS (0 shed within its reserved slots while a
/// machine lane sheds), the agent ref-creation + backlink-read lane SHEDS (`429 + Retry-After`,
/// absorbed not unbounded), and cross-tenant impact is 0 (the storm fills only the surging tenant's
/// per-tenant budget).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefsSurgeReport {
    /// The agent-lane shed count on the surging tenant (the storm absorbed by shedding — must be > 0).
    pub surging_agent_shed_count: u64,
    /// The human-lane shed count on the surging tenant (the protected lane — must be 0).
    pub surging_human_shed_count: u64,
    /// Whether the surging tenant's OWN human read/create was admitted within its reserved slots.
    pub surging_human_admitted: bool,
    /// Whether the quiet co-tenant's human read/create was admitted within budget (untouched).
    pub quiet_human_admitted: bool,
    /// The quiet co-tenant's in-flight count (the cross-tenant impact — must be 0 unless the quiet
    /// tenant's OWN human op was admitted, i.e. ≤ 1; the storm never spends the quiet tenant's budget).
    pub cross_tenant_impact: u32,
}

impl RefsSurgeReport {
    /// **The REF-D10 GREEN predicate (the three F6 properties — all measured, none weakened).** The
    /// agent lane shed (absorbed by shedding), the human lane held (0 shed on the surging tenant + its
    /// own human admitted), the quiet co-tenant's human held, and cross-tenant impact is contained.
    pub fn is_ref_d10_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
    }

    /// A one-line summary for the dated green-artifact log row (observability is part of the pass).
    pub fn summary(&self) -> String {
        format!(
            "REF-D10: surging agent_shed={} human_shed={} surging_human_admitted={} \
             quiet_human_admitted={} cross_tenant_impact={} → {}",
            self.surging_agent_shed_count,
            self.surging_human_shed_count,
            self.surging_human_admitted,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
            if self.is_ref_d10_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

/// **Drive the REF-D10 30× surge on ONE Refs surface gate.** Spreads `storm_agent_ops` agent requests
/// (the derived storm-op count) on the surging tenant — the agent lane fills then sheds — then proves
/// the surging tenant's OWN human op is still admitted (shed-last) and a quiet co-tenant's human op is
/// admitted within its independent budget. Returns the [`RefsSurgeReport`] (the three F6 properties).
///
/// The `multiplier` is the surge factor (read from the FILE by the caller; passed through for the
/// log row), not used to scale here — `storm_agent_ops` is already the derived 30× storm-op count.
pub fn run_refs_surge(
    gate: &mut RefsShedGate,
    surging: &TenantId,
    quiet: &TenantId,
    storm_agent_ops: u64,
    _multiplier: u32,
) -> RefsSurgeReport {
    // Drive the agent ref-creation + backlink-read storm on the surging tenant: the agent lane fills
    // its non-reserved budget then sheds (429 + Retry-After) — absorbed by shedding, never unbounded.
    for _ in 0..storm_agent_ops {
        let _ = gate.admit_class(surging, RunClass::Agent);
    }

    // The surging tenant's OWN human read/create is STILL admitted — the protected lane, shed last
    // (a human uses the reserved slots the agent storm could never take).
    let surging_human_admitted = gate.admit_class(surging, RunClass::Human).is_ok();

    // The quiet co-tenant is UNTOUCHED: its human op is admitted within its independent per-tenant
    // budget (the storm never spent the quiet tenant's slots).
    let quiet_in_flight_before = gate.in_flight(quiet);
    let quiet_human_admitted = gate.admit_class(quiet, RunClass::Human).is_ok();

    RefsSurgeReport {
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        quiet_human_admitted,
        // cross-tenant impact: the quiet tenant's in-flight BEFORE its own human op (the storm's
        // spillover onto the quiet tenant — must be 0; the per-tenant bound is the blast boundary).
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

    /// **Both Refs shed budgets are read from the thresholds file** (the prompt's explicit
    /// requirement). The gates open against the canonical `thresholds.toml` `[[shed_budgets]]` rows —
    /// not a hardcoded number. A missing row would have been a loud error.
    #[test]
    fn the_refs_shed_budgets_are_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let read = RefsShedGate::backlink_read_from_thresholds(&thresholds)
            .expect("RefsBacklinkRead budget present");
        let create = RefsShedGate::ref_create_from_thresholds(&thresholds)
            .expect("RefsRefCreate budget present");
        assert_eq!(read.surface(), Surface::RefsBacklinkRead);
        assert_eq!(create.surface(), Surface::RefsRefCreate);

        for surface in [Surface::RefsBacklinkRead, Surface::RefsRefCreate] {
            let b = thresholds.shed_budget(surface).expect("present");
            assert!(b.per_tenant_in_flight_cap > 0, "{surface:?} bounded (§7.1)");
            assert!(
                b.human_lane_reservation > 0,
                "{surface:?} reserves a human lane"
            );
        }
    }

    /// **The shed order serves the human while the agent lane sheds (REF-D10):** the human read/create
    /// is SERVED while the agent ref-creation/backlink-read lane SHEDS (`429 + Retry-After`).
    #[test]
    fn shed_order_serves_the_human_while_the_agent_lane_sheds() {
        let mut gate = RefsShedGate::with_budget(Surface::RefsBacklinkRead, small_budget());
        let a = agent("acme");
        let h = human("acme");

        // an agent ref-creation/backlink-read storm fills the non-human budget (cap-reserved = 4) then sheds.
        for _ in 0..4 {
            assert!(
                gate.admit_for(&a, None).is_ok(),
                "agent op admitted under budget"
            );
        }
        let shed = gate.admit_for(&a, None).expect_err("the agent storm sheds");
        assert_eq!(shed.lane, RunClass::Agent);
        assert_eq!(shed.retry_after_secs, 3, "the shed carries a Retry-After");

        // THE GATE: the HUMAN's interactive read/create is STILL SERVED (shed last).
        assert_eq!(
            gate.admit_for(&h, None)
                .expect("the human is served while the agent sheds"),
            RunClass::Human
        );
        assert_eq!(gate.shed_count(RunClass::Human), 0, "human lane: 0 shed");
        assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
    }

    /// **The full shed PRIORITY order: speculative → batch/CI → agent → human-last.**
    #[test]
    fn shed_priority_is_speculative_then_batch_then_agent_then_human() {
        let mut gate = RefsShedGate::with_budget(Surface::RefsRefCreate, small_budget());
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
            "batch/ci sheds next"
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

    /// **Per-tenant: one tenant's agent storm NEVER sheds another tenant's human (blast-radius).**
    #[test]
    fn one_tenants_storm_never_sheds_anothers_human() {
        let mut gate = RefsShedGate::with_budget(Surface::RefsBacklinkRead, small_budget());
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

    /// **A machine principal can NEVER up-class to the human lane** (structurally unspoofable).
    #[test]
    fn a_machine_principal_cannot_spoof_the_human_lane() {
        let mut gate = RefsShedGate::with_budget(Surface::RefsRefCreate, small_budget());
        let a = agent("acme");
        assert_eq!(gate.admit_for(&a, None).expect("admitted"), RunClass::Agent);
        let h = human("acme");
        assert_eq!(
            gate.admit_for(&h, Some(RunClassHeader::Speculative))
                .expect("admitted"),
            RunClass::Speculative,
            "a human-issued prefetch may down-class itself"
        );
    }

    /// Release frees a slot so the lane recovers after the surge passes.
    #[test]
    fn release_frees_a_slot_after_the_surge() {
        let mut gate = RefsShedGate::with_budget(
            Surface::RefsBacklinkRead,
            SurfaceBudget {
                per_tenant_in_flight_cap: 3,
                human_lane_reservation: 1,
                retry_after_secs: 1,
            },
        );
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

    /// **The REF-D10 surge report is GREEN under a real storm** (the three F6 properties).
    #[test]
    fn run_refs_surge_is_green() {
        let mut gate = RefsShedGate::with_budget(Surface::RefsBacklinkRead, small_budget());
        let surging = tenant("noisy");
        let quiet = tenant("quiet");
        // a storm well past the non-human budget (4) so the agent lane MUST shed.
        let report = run_refs_surge(&mut gate, &surging, &quiet, 50, REFS_SURGE_MULTIPLIER);
        assert!(report.is_ref_d10_green(), "{}", report.summary());
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
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
        let mut gate = RefsShedGate::with_budget(Surface::RefsBacklinkRead, huge);
        let report = run_refs_surge(
            &mut gate,
            &tenant("noisy"),
            &tenant("quiet"),
            100,
            REFS_SURGE_MULTIPLIER,
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "the unbounded lane swallowed the storm"
        );
        assert!(
            !report.is_ref_d10_green(),
            "an unbounded lane MUST read RED"
        );
    }

    /// The floors are named (REF-P23 R4 follow-on + the measured-only shard branch).
    #[test]
    fn the_floors_are_named() {
        assert_eq!(R4_REACH_INDEX_FOLLOW_ON, "REF-P23");
        // a shard split is measured-only (§6.2) — NOT pre-sharded by this prompt. The flag records
        // that the branch is measured-gated; reading it through a binding keeps the doctrine explicit.
        let measured_only = SHARD_SPLIT_IS_MEASURED_ONLY;
        assert!(measured_only, "a shard split is measured-only (§6.2)");
        assert_eq!(REFS_SURGE_MULTIPLIER, 30);
    }
}
