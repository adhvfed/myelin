//! # `surge` — the Notif 30×-agent-surge shed budget (the F6 surge family; human-last lane)
//! + NOTIF-D5 (NOTIF-P25 / global P-467, M5)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/notifications.md` §5.2 (the fan-out scale axis +
//! the **agent-mention-storm shed budget**, C5 / OQ-K): the protected-human-lane shed order
//! (`speculative → batch/CI → agent → human-last`, ADR-16) concretised for Notif's storm profile —
//! a **per-tenant agent-run in-flight cap** (reserve/settle refuses over-cap), **humans never queue
//! behind agent runs** (a SEPARATE lane), the **agent-generated notification lane sheds first** with
//! `429 + Retry-After` (the agent runtime honours it, ADR-16.3), and a **human's interactive inbox
//! read is last-to-shed**; plus a **delivery-adapter bulkhead per provider** that bounds provider
//! load. **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §3 (the 1×/10×/30× load
//! generator; the multiplier read from the FROZEN thresholds file, never hardcoded; observability —
//! shed-counts + delivery-success — is part of the pass; never weaken a threshold to pass), §2 (the
//! protected human lane; per-tenant blast-radius). `02-platform-substrate.md` §5 (an unbounded lane
//! is the cascade). **Contract-index:** row **1.11** (the protected-human-lane shed order +
//! per-surface shed budgets OQ-K — Notif's agent-mention surface is one lane), row **1.8** (the
//! per-lane shed-count + delivery-success telemetry). **Drill source:**
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` NOTIF-D5 (30× agent-generated
//! notification surge → human inbox-read lane holds, agent sheds, delivery-adapter bulkhead bounds
//! provider load; shed-counts; delivery-success — part of the master M5 F6 surge family).
//!
//! ## What this module is (the Notif surge half — NOTIF-P25)
//! Notif is the **origin** of the agent-mention-storm surface ([`Surface::AgentMention`]): an agent
//! run @-mentioning a wide audience, or an agent fan-out posting notification-generating signals, is
//! the 30× surge NOTIF-D5 drives. This module tunes the doctrine shed order to Notif's two storm
//! profiles, both reading their budget from the FROZEN thresholds file:
//! - a **human's interactive inbox read** holds the protected lane ([`Surface::AgentMention`]'s
//!   reserved human fraction; shed LAST, latency within budget);
//! - the **agent-generated notification lane** sheds first with `429 + Retry-After` (honoured — our
//!   ResilientClient honours `Retry-After`, P-S17, so a shed is not a retry-storm amplifier);
//! - **per-tenant in-flight caps** keep one tenant's agent-mention storm off another tenant's humans
//!   (the per-tenant bulkhead, §5.2 / EI-02 §1);
//! - a **delivery-adapter bulkhead per provider** ([`ProviderBulkhead`]) bounds the off-cell provider
//!   load — the surge can never push unbounded concurrent sends at one delivery provider (NOTIF-D5's
//!   "delivery-adapter bulkhead bounds provider load" leg).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! **The shed order itself is the substrate's** [`myelin_substrate::shed`]: this module does NOT
//! re-author the shed lane / run-class / budget table (that would be a doctrinal fork — the same
//! mistake [`myelin_git::shed_clone`] / `myelin_refs_service::surge` / `myelin_search::surge` avoided
//! for their surfaces). It **WIRES** the existing [`ShedLane`] over the EXISTING
//! [`Surface::AgentMention`] surface — the §7.6 row already named for "humans never queue behind
//! agent runs" — reading its budget **from the thresholds file** ([`myelin_substrate::thresholds`]).
//! The **delivery-adapter bulkhead** is the substrate's [`myelin_substrate::shed::BoundedQueue`] (the
//! one bounded-everything primitive), not a second pool. The Notif surge gate's only authoring is the
//! *derivation* of the request's [`RunClass`] from its principal + an optional run-class header, the
//! placement of the admit/shed decision at the FRONT of the inbox-read / notification-emit pipeline,
//! and the per-provider bulkhead seam.
//!
//! ## The references-not-payloads / audit invariant is UNTOUCHED under shed (§3.9, EI-04 §5.3)
//! Shedding is a **pre-projection** admission decision: it refuses an over-budget agent notification
//! cheaply, BEFORE any inbox UPSERT / read materialisation runs. So a shed never drops an item from
//! the **audit/history** (Notif is a projection — storm-control suppresses DELIVERY and RANKING only,
//! never the durable history, EI-04 §5.3); the surge only bounds the *delivery/read* concurrency, it
//! never relaxes the projection. A human's inbox read that IS admitted still resolves refs per-viewer
//! (the NOTIF-1 invariant), so the surge changes throughput, never correctness.
//!
//! ## Floors named (VISION §3 — name your floors)
//! - **The EU-sovereign delivery provider** (the real provider swapped into the `DeliveryAdapter`
//!   trait, [OPEN — LEGAL]) is **NOTIF-P26** ([`EU_DELIVERY_PROVIDER_FOLLOW_ON`]): this prompt is the
//!   surge/shed-order half ONLY. The bulkhead here bounds the load at WHATEVER adapter is wired (the
//!   deterministic [`crate::MockAdapter`] today, the real EU provider after NOTIF-P26); the bounding
//!   DECISION does not change shape.
//! - **The off-cell-payload erasure residual** is **NOTIF-P27**.
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet,
//!   testing-strategy §4.1). Here the load is the P-S02 generator at 30× across the surging tenant;
//!   the per-tenant fairness + shed-order + cross-tenant-0 + bulkhead PROPERTIES are complete +
//!   testable now and do not change shape when the real index/provider carries the load.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2/§3)
//! The shed-order DECISION path ([`NotifShedGate::admit_for`] / [`NotifShedGate::admit_class`] → the
//! human-protected per-tenant graded admit) + the [`ProviderBulkhead`] admission are mandatory-core:
//! an off-by-one that sheds a human inbox read before an agent notification, that leaks one tenant's
//! budget into another, or that lets the bulkhead grow provider load unboundedly, is the failure this
//! exists to catch. **Floor: ≥ 80% line/branch mutation score on `surge.rs`** (measured with
//! `cargo mutants`; reported in the P-467 commit body).

use myelin_identity::Principal;
use myelin_substrate::shed::{
    BoundedQueue, RunClass, RunClassHeader, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// **The Notif surge default-to-beat multiplier (NOTIF-D5).** The 30× world-scale surge factor the
/// NOTIF-D5 drill drives at — read from the FROZEN thresholds file `[surge] multiplier` row (the
/// versioned source of truth, P-038) and asserted to equal this documented default-to-beat; a
/// divergence is a LOUD failure, never a silent weakening (EI-01 §3).
pub const NOTIF_SURGE_MULTIPLIER: u32 = 30;

/// **The EU-sovereign delivery provider follow-on (the named [OPEN — LEGAL] floor).** This prompt
/// (NOTIF-P25) is the surge/shed-order half ONLY; the real EU provider swapped into the
/// `DeliveryAdapter` trait — with its DPA / sub-processor posture, counsel/DPO ratified — is
/// **NOTIF-P26**. The bulkhead bounds load at whatever adapter is wired; the bounding shape is stable.
pub const EU_DELIVERY_PROVIDER_FOLLOW_ON: &str = "NOTIF-P26";

/// **The off-cell-payload erasure residual follow-on (the named structural-erase floor).** The
/// off-cell delivery payload erasure residual (X-7 / 10.9) is **NOTIF-P27**. Named here so the floor
/// is explicit, not implied.
pub const ERASURE_RESIDUAL_FOLLOW_ON: &str = "NOTIF-P27";

/// **The Notif agent-mention-storm surge surface (§5.2, C5 / OQ-K).** Notif is the ORIGIN of the
/// substrate's [`Surface::AgentMention`] storm profile — "humans never queue behind agent runs" is
/// literally Notif's row. The surge gate fronts THIS surface (it does not invent a new one); the
/// §7.6 budget for it already lives in the thresholds file.
pub const NOTIF_SURGE_SURFACE: Surface = Surface::AgentMention;

// ───────────────────────────── the Notif surge shed gate ─────────────────────────────────────────

/// **The protected-human-lane shed gate at the Notif agent-mention surface (NOTIF-P25 / OQ-K;
/// contract 1.11).**
///
/// A thin Notif wiring over the substrate's [`ShedLane`] for the ONE Notif storm surface
/// ([`Surface::AgentMention`]): it reads the surface's budget **from the thresholds file** and applies
/// the shed order `speculative → batch/CI → agent → human-last`, per-tenant. An inbox read / a
/// notification emit is admitted through [`NotifShedGate::admit_for`] (the run-class derived from the
/// verified principal); an over-budget non-human lane is shed with `429 + Retry-After`, while the
/// human inbox-read lane is protected (shed only in true saturation). The decision is a
/// pre-projection admission — a shed never drops the durable audit/history (EI-04 §5.3).
pub struct NotifShedGate {
    lane: ShedLane,
}

/// **Why a Notif inbox-read / notification-emit was refused at the shed gate** — the typed form the
/// transport maps to the wire `429`. A shed carries the `Retry-After` (seconds) the agent runtime
/// honours (ADR-16.3 — the no-amplification guarantee: a shed is not a retry-storm amplifier).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotifShedRejection {
    /// The lane that was shed (`speculative` / `batch_ci` / `agent` / `human`) — the contract-1.8
    /// per-lane shed-count signal keys on this.
    pub lane: RunClass,
    /// The `Retry-After` value in **seconds** (the frozen §2.10 unit) the transport sets on the
    /// `429 Too Many Requests` response.
    pub retry_after_secs: u64,
}

impl NotifShedGate {
    /// Open the Notif agent-mention surge gate, reading its budget **from the thresholds file** (the
    /// prompt's "the v1 budget numbers are in the thresholds file"). A missing row is a LOUD error
    /// (the gate refuses to open against a guessed budget — EI-01 §3), never a silent default.
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<NotifShedGate, String> {
        let budget = thresholds.shed_budget(NOTIF_SURGE_SURFACE).map_err(|e| {
            format!("Notif shed budget for {NOTIF_SURGE_SURFACE:?} unavailable: {e}")
        })?;
        Ok(NotifShedGate {
            lane: ShedLane::with_budget(NOTIF_SURGE_SURFACE, budget),
        })
    }

    /// Open the gate against an explicit budget (used by the surge drill to drive the boundary at a
    /// small, deterministic budget without editing the thresholds file).
    pub fn with_budget(budget: SurfaceBudget) -> NotifShedGate {
        NotifShedGate {
            lane: ShedLane::with_budget(NOTIF_SURGE_SURFACE, budget),
        }
    }

    /// **Admit a Notif request by its verified principal + an optional injected run-class header.** The
    /// run-class is DERIVED ([`RunClass::derive`]) from `principal.kind` (the kind sets the ceiling)
    /// and the header (which may only down-class) — a machine principal can NEVER up-class to the
    /// protected human inbox-read lane. Returns `Ok(class)` admitted (a slot was taken — release it on
    /// completion via [`NotifShedGate::release`]) or `Err(NotifShedRejection)` shed (`429 +
    /// Retry-After`). The decision is per-`principal.tenant`.
    pub fn admit_for(
        &mut self,
        principal: &Principal,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, NotifShedRejection> {
        let class = RunClass::derive(&principal.kind, header);
        self.admit_class(&principal.tenant, class).map(|()| class)
    }

    /// **Admit a request of a pre-derived [`RunClass`] for `tenant`.** The lower-level form the surge
    /// drill drives. Returns `Ok(())` admitted (a slot taken) or `Err(NotifShedRejection)` shed. The
    /// human inbox-read lane is protected: a human is shed ONLY when every slot (the reserved human
    /// fraction included) is full; the non-human lanes shed first, in the graded order
    /// `speculative → batch/CI → agent`.
    pub fn admit_class(
        &mut self,
        tenant: &TenantId,
        class: RunClass,
    ) -> Result<(), NotifShedRejection> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(NotifShedRejection {
                lane: class,
                retry_after_secs,
            }),
        }
    }

    /// Release a slot a prior admit took for `(tenant, class)` — call when the inbox read / emit
    /// completes so the lane recovers after the surge.
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

    /// The surface this gate fronts (always [`Surface::AgentMention`]).
    pub fn surface(&self) -> Surface {
        self.lane.surface()
    }
}

// ─────────────────────── the delivery-adapter bulkhead (NOTIF-D5 leg) ─────────────────────────────

/// **A bounded delivery-adapter bulkhead per provider (NOTIF-D5; §5.2 "bounded delivery-adapter
/// concurrency a bulkhead per provider").**
///
/// The off-cell delivery path is the surge's amplification risk: a 30× agent-mention storm that
/// fanned out to email/push/web could push unbounded concurrent sends at ONE delivery provider,
/// overwhelming it (and tripping its rate limits — the retry-storm cascade, EI-02 §5). The bulkhead
/// bounds the concurrent in-flight sends PER PROVIDER: a send admits while the provider's bulkhead has
/// a free permit, and **sheds (fast-fails) once the provider is at its concurrency bound**, so the
/// provider load is bounded regardless of the surge size (Little's Law, §7.1). This is the substrate's
/// one [`BoundedQueue`] primitive keyed per provider — NOT a second pool implementation.
///
/// The bulkhead is per *provider channel* (`email` / `push` / `web` / …): a storm on one provider's
/// channel never starves another's permits (the per-provider isolation NOTIF-D5 names).
#[derive(Clone, Debug)]
pub struct ProviderBulkhead {
    provider: String,
    queue: BoundedQueue,
}

impl ProviderBulkhead {
    /// Open a bulkhead for `provider` bounding concurrent sends to `concurrency` permits (the per-
    /// provider §5.2 bound). A positive concurrency is required of a real provider; `0` is the
    /// degenerate always-shed (== provider unavailable) bulkhead — callers pass a measured bound.
    pub fn new(provider: impl Into<String>, concurrency: u32) -> ProviderBulkhead {
        ProviderBulkhead {
            provider: provider.into(),
            queue: BoundedQueue::new(concurrency),
        }
    }

    /// **Try to admit one send to this provider.** Returns `true` if a permit was taken (the send may
    /// proceed — release it on the provider ack via [`ProviderBulkhead::release`]); `false` if the
    /// provider is at its concurrency bound — the send has **fast-failed (shed)**, bounding provider
    /// load rather than growing latency unboundedly. The shed is counted (the bounded-everything
    /// signal feeds the NOTIF-D5 delivery-load artifact).
    pub fn try_send(&mut self) -> bool {
        self.queue.try_acquire()
    }

    /// Release a permit a prior [`ProviderBulkhead::try_send`] took (call on the provider ack /
    /// completion) so the bulkhead recovers after the surge passes. Saturating — a stray release never
    /// wraps.
    pub fn release(&mut self) {
        self.queue.release();
    }

    /// The provider this bulkhead fronts (the per-provider isolation key).
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The concurrent in-flight sends to this provider (admitted not yet released) — never exceeds the
    /// bound (the NOTIF-D5 "bulkhead bounds provider load" assertion reads this).
    pub fn in_flight(&self) -> u32 {
        self.queue.in_flight()
    }

    /// The provider concurrency bound (the §5.2 per-provider cap).
    pub fn concurrency(&self) -> u32 {
        self.queue.capacity()
    }

    /// The cumulative count of sends the bulkhead shed (the provider was already at its bound) — the
    /// proof the bulkhead fast-failed rather than buffering an unbounded queue at the provider.
    pub fn shed_count(&self) -> u64 {
        self.queue.shed_count()
    }
}

// ─────────────────────────────── the NOTIF-D5 surge report ────────────────────────────────────────

/// **The NOTIF-D5 30× surge report — the four properties of the agent-mention-storm surge.** The dated
/// green artifact the DoD names: the human inbox-read lane HOLDS (0 shed within its reserved slots
/// while the agent notification lane sheds), the agent-generated notification lane SHEDS (`429 +
/// Retry-After`, absorbed not unbounded), other tenants are UNAFFECTED (the storm fills only the
/// surging tenant's per-tenant budget), and the delivery-adapter bulkhead BOUNDS provider load (the
/// concurrent provider sends never exceed the per-provider bound, regardless of surge size).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifSurgeReport {
    /// The agent-lane shed count on the surging tenant (the agent-generated notification storm
    /// absorbed by shedding — must be > 0).
    pub surging_agent_shed_count: u64,
    /// The CI/batch-lane shed count on the surging tenant (a batch/CI notification storm — must be > 0).
    pub surging_ci_shed_count: u64,
    /// The human inbox-read shed count on the surging tenant (the protected lane — must be 0).
    pub surging_human_shed_count: u64,
    /// Whether the surging tenant's OWN human inbox read was admitted within its reserved slots.
    pub surging_human_admitted: bool,
    /// Whether the quiet co-tenant's human inbox read was admitted within budget (untouched).
    pub quiet_human_admitted: bool,
    /// The quiet co-tenant's in-flight count BEFORE its own human read (the cross-tenant impact — must
    /// be 0; the storm never spends the quiet tenant's budget).
    pub cross_tenant_impact: u32,
    /// The peak concurrent provider sends the delivery-adapter bulkhead allowed (must be ≤ the
    /// per-provider concurrency bound — the bulkhead bounded provider load under the surge).
    pub provider_peak_in_flight: u32,
    /// The per-provider concurrency bound the bulkhead enforced (the §5.2 cap the peak must respect).
    pub provider_bound: u32,
    /// The number of sends the bulkhead shed (the provider was at its bound — must be > 0 under a
    /// surge larger than the bound, proving the bulkhead fast-failed rather than buffering unbounded).
    pub provider_bulkhead_shed: u64,
}

impl NotifSurgeReport {
    /// **The NOTIF-D5 GREEN predicate (the four properties — all measured, none weakened).** The
    /// agent and CI machine lanes shed (absorbed by shedding), the human inbox-read lane held (0 shed
    /// on the surging tenant plus its own human admitted), the quiet co-tenant's human held,
    /// cross-tenant impact is 0, and the delivery-adapter bulkhead bounded provider load (peak ≤ bound,
    /// and it shed the over-bound sends rather than buffering them).
    pub fn is_notif_d5_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.surging_ci_shed_count > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
            && self.provider_bound > 0
            && self.provider_peak_in_flight <= self.provider_bound
            && self.provider_bulkhead_shed > 0
    }

    /// A one-line summary for the dated green-artifact log row (observability is part of the pass —
    /// shed-counts + the delivery-load bound, NOTIF-D5's named signals).
    pub fn summary(&self) -> String {
        format!(
            "NOTIF-D5: surging agent_shed={} ci_shed={} human_shed={} surging_human_admitted={} \
             quiet_human_admitted={} cross_tenant_impact={} | provider_peak={}/{} bulkhead_shed={} → {}",
            self.surging_agent_shed_count,
            self.surging_ci_shed_count,
            self.surging_human_shed_count,
            self.surging_human_admitted,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
            self.provider_peak_in_flight,
            self.provider_bound,
            self.provider_bulkhead_shed,
            if self.is_notif_d5_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

/// **Drive the NOTIF-D5 30× agent-mention surge on the Notif shed gate + the delivery bulkhead.**
///
/// Spreads `storm_agent_ops` agent-generated notification ops + `storm_ci_ops` CI/batch notification
/// ops on the surging tenant — the machine lanes fill then shed — while every admitted notification
/// attempts a delivery through the per-provider [`ProviderBulkhead`] (the surge pushes more concurrent
/// sends than the provider bound, so the bulkhead sheds the excess, bounding provider load). It then
/// proves the surging tenant's OWN human inbox read is still admitted (shed-last) and a quiet
/// co-tenant's human read is admitted within its independent per-tenant budget. Returns the
/// [`NotifSurgeReport`] (the four properties).
///
/// The `multiplier` is the surge factor (read from the FILE by the caller; passed through for the log
/// row), not used to scale here — the storm-op counts are already the derived 30× storm-op counts.
pub fn run_notif_surge(
    gate: &mut NotifShedGate,
    bulkhead: &mut ProviderBulkhead,
    surging: &TenantId,
    quiet: &TenantId,
    storm_agent_ops: u64,
    storm_ci_ops: u64,
    _multiplier: u32,
) -> NotifSurgeReport {
    let mut provider_peak: u32 = 0;

    // Drive the CI/batch notification storm SUSTAINED — admitted ops KEEP their in-flight slot so the
    // storm PRESSURES the per-tenant cap and the lane sheds (the surge is sustained, not a one-shot
    // exhaustion). The CI lane is held to a TIGHTER graded ceiling than the agent lane, so the
    // batch/CI notification storm sheds first (speculative → batch/CI → agent → human-last). Each
    // admitted op also attempts a delivery through the per-provider bulkhead (the off-cell load).
    for _ in 0..storm_ci_ops {
        if gate.admit_class(surging, RunClass::BatchCi).is_ok() {
            attempt_delivery(bulkhead, &mut provider_peak);
        }
    }
    // Drive the agent-generated notification storm SUSTAINED — the agent lane fills its non-reserved
    // budget then sheds (429 + Retry-After), absorbed by shedding, never unbounded.
    for _ in 0..storm_agent_ops {
        if gate.admit_class(surging, RunClass::Agent).is_ok() {
            attempt_delivery(bulkhead, &mut provider_peak);
        }
    }

    // The surging tenant's OWN human inbox read is STILL admitted — the protected lane, shed last (a
    // human uses the reserved slots the agent/CI storm could never take).
    let surging_human_admitted = gate.admit_class(surging, RunClass::Human).is_ok();

    // The quiet co-tenant is UNTOUCHED: its human inbox read is admitted within its independent
    // per-tenant budget (the storm never spent the quiet tenant's slots).
    let quiet_in_flight_before = gate.in_flight(quiet);
    let quiet_human_admitted = gate.admit_class(quiet, RunClass::Human).is_ok();

    NotifSurgeReport {
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_ci_shed_count: gate.shed_count(RunClass::BatchCi),
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        quiet_human_admitted,
        cross_tenant_impact: quiet_in_flight_before,
        provider_peak_in_flight: provider_peak,
        provider_bound: bulkhead.concurrency(),
        provider_bulkhead_shed: bulkhead.shed_count(),
    }
}

/// Attempt ONE delivery through the per-provider bulkhead, tracking the peak concurrent in-flight. The
/// surge drives more concurrent sends than the provider bound: each send takes a permit and HOLDS it
/// (a sustained concurrent burst — the off-cell send is in-flight at the provider), so concurrency
/// climbs to the bound and every further send `try_send`s `false` (sheds), bounding provider load
/// rather than buffering an unbounded queue at the provider (Little's Law, §7.1). The peak in-flight
/// NEVER exceeds the bound (the NOTIF-D5 assertion); the over-bound sends are shed, not buffered.
fn attempt_delivery(bulkhead: &mut ProviderBulkhead, peak: &mut u32) {
    // A permit is HELD across the surge (the concurrent in-flight send); a `false` is a shed — the
    // over-bound send fast-failed, never queued. We never release inside the storm: the surge is a
    // sustained concurrent burst, so the bulkhead bound is the hard ceiling on provider load.
    let _ = bulkhead.try_send();
    *peak = (*peak).max(bulkhead.in_flight());
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
            retry_after_secs: 10,
        }
    }

    // ───────────────────────── the shed budget is read from the file ─────────────────────────

    /// **The Notif shed budget is read from the thresholds file** (the prompt's explicit requirement:
    /// "the v1 budget numbers are in the thresholds file"). The gate opens against the canonical
    /// `thresholds.toml` `[[shed_budgets]]` row for `AgentMention` — Notif's storm surface — not a
    /// hardcoded number. A missing row would have been a loud error.
    #[test]
    fn the_notif_shed_budget_is_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let gate =
            NotifShedGate::from_thresholds(&thresholds).expect("AgentMention budget present");
        assert_eq!(gate.surface(), Surface::AgentMention);
        assert_eq!(NOTIF_SURGE_SURFACE, Surface::AgentMention);

        let b = thresholds
            .shed_budget(Surface::AgentMention)
            .expect("present");
        assert!(
            b.per_tenant_in_flight_cap > 0,
            "AgentMention bounded (§7.1)"
        );
        assert!(
            b.human_lane_reservation > 0,
            "AgentMention reserves a human inbox-read lane (humans never queue behind agent runs)"
        );
        // the surge multiplier in the file matches the documented default-to-beat (never hardcoded).
        assert_eq!(thresholds.surge.multiplier, NOTIF_SURGE_MULTIPLIER);
    }

    /// **The lane separation: a human inbox read is SERVED while the agent notification lane SHEDS
    /// (NOTIF-D5):** the human read holds the protected lane while the agent-generated notification
    /// lane sheds (`429 + Retry-After`). This is the prompt's required "a human read is served while
    /// the agent lane is shedding" unit test.
    #[test]
    fn a_human_read_is_served_while_the_agent_lane_sheds() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        let a = agent("acme");
        let h = human("acme");

        // an agent notification storm fills the non-human budget (cap-reserved = 4) then sheds.
        for _ in 0..4 {
            assert!(
                gate.admit_for(&a, None).is_ok(),
                "agent notification admitted under budget"
            );
        }
        let shed = gate.admit_for(&a, None).expect_err("the agent storm sheds");
        assert_eq!(shed.lane, RunClass::Agent);
        assert_eq!(
            shed.retry_after_secs, 10,
            "the shed carries a Retry-After the agent runtime honours (ADR-16.3)"
        );

        // THE GATE: the HUMAN's interactive inbox read is STILL SERVED (shed last).
        assert_eq!(
            gate.admit_for(&h, None)
                .expect("the human is served while the agent lane sheds"),
            RunClass::Human
        );
        assert_eq!(
            gate.shed_count(RunClass::Human),
            0,
            "the human inbox-read lane: 0 shed"
        );
        assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
    }

    /// **The per-tenant in-flight cap: over-cap → reserve/settle refuses (the prompt's required unit
    /// test).** An agent notification storm fills the per-tenant cap, then the next over-cap agent op
    /// is REFUSED (shed) — the reserve/settle refuses over-cap at dispatch (§5.2, contract 11.7).
    #[test]
    fn the_per_tenant_in_flight_cap_refuses_over_cap() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        let t = tenant("acme");
        // non-human budget is cap(6) - reserved(2) = 4: fill it, then over-cap refuses.
        for _ in 0..4 {
            gate.admit_class(&t, RunClass::Agent)
                .expect("agent admitted under the per-tenant cap");
        }
        assert_eq!(
            gate.in_flight(&t),
            4,
            "the per-tenant in-flight is at the cap"
        );
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "over-cap → reserve/settle refuses (the per-tenant in-flight cap bites)"
        );
    }

    /// **The full shed PRIORITY order: speculative → batch/CI → agent → human-last** (the batch/CI
    /// notification lane sheds before the agent lane sheds before the human inbox read).
    #[test]
    fn shed_priority_is_speculative_then_batch_then_agent_then_human() {
        let mut gate = NotifShedGate::with_budget(small_budget());
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
            "batch/CI notification lane sheds next"
        );
        gate.admit_class(&t, RunClass::Agent)
            .expect("agent admitted"); // non_human → 4
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "agent sheds before the human"
        );
        gate.admit_class(&t, RunClass::Human)
            .expect("human inbox read served — shed last");

        assert_eq!(gate.shed_count(RunClass::Speculative), 1);
        assert_eq!(gate.shed_count(RunClass::BatchCi), 1);
        assert_eq!(gate.shed_count(RunClass::Agent), 1);
        assert_eq!(gate.shed_count(RunClass::Human), 0);
    }

    /// **Cross-tenant isolation: tenant A's surge does not affect tenant B's human-read latency (the
    /// prompt's required cross-tenant test).** One tenant's agent notification storm NEVER sheds
    /// another tenant's human inbox read (the per-tenant blast-radius bulkhead).
    #[test]
    fn one_tenants_surge_never_sheds_anothers_human() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        let noisy = agent("noisy");
        let quiet_human = human("quiet");

        for _ in 0..4 {
            gate.admit_for(&noisy, None).expect("noisy agent admitted");
        }
        assert!(
            gate.admit_for(&noisy, None).is_err(),
            "noisy agent notification lane sheds"
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
            "the noisy storm must NEVER shed another tenant's human inbox read"
        );
    }

    /// **A machine principal can NEVER up-class to the human inbox-read lane** (structurally
    /// unspoofable — there is no `Human` run-class header).
    #[test]
    fn a_machine_principal_cannot_spoof_the_human_lane() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        let a = agent("acme");
        assert_eq!(gate.admit_for(&a, None).expect("admitted"), RunClass::Agent);
        let h = human("acme");
        assert_eq!(
            gate.admit_for(&h, Some(RunClassHeader::Speculative))
                .expect("admitted"),
            RunClass::Speculative,
            "a human-issued prefetch read may down-class itself"
        );
    }

    /// Release frees a slot so the lane recovers after the surge passes.
    #[test]
    fn release_frees_a_slot_after_the_surge() {
        let mut gate = NotifShedGate::with_budget(SurfaceBudget {
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

    // ───────────────────────── the delivery-adapter bulkhead ─────────────────────────

    /// **The delivery-adapter bulkhead bounds provider load — concurrent sends never exceed the bound,
    /// and the over-bound sends shed (NOTIF-D5 leg).**
    #[test]
    fn the_provider_bulkhead_bounds_provider_load() {
        let mut bh = ProviderBulkhead::new("email", 2);
        assert_eq!(bh.provider(), "email");
        assert_eq!(bh.concurrency(), 2);
        assert!(bh.try_send(), "first send under the bound");
        assert!(bh.try_send(), "second send under the bound");
        assert!(
            !bh.try_send(),
            "a third concurrent send SHEDS — the provider load is bounded (Little's Law)"
        );
        assert_eq!(
            bh.in_flight(),
            2,
            "provider in-flight never exceeds the bound"
        );
        assert_eq!(
            bh.shed_count(),
            1,
            "the over-bound send was shed, not buffered"
        );
        bh.release();
        assert!(
            bh.try_send(),
            "a released permit is reusable after the surge"
        );
    }

    /// **Per-provider isolation: one provider's bulkhead is independent of another's.** A storm on the
    /// `email` provider never spends the `push` provider's permits.
    #[test]
    fn provider_bulkheads_are_per_provider_isolated() {
        let mut email = ProviderBulkhead::new("email", 1);
        let mut push = ProviderBulkhead::new("push", 1);
        assert!(email.try_send());
        assert!(!email.try_send(), "email at its bound");
        // push is untouched — its independent bound is free.
        assert!(push.try_send(), "push provider's bulkhead is independent");
        assert_eq!(email.in_flight(), 1);
        assert_eq!(push.in_flight(), 1);
    }

    // ───────────────────────── the NOTIF-D5 surge report ─────────────────────────

    /// **The NOTIF-D5 surge report is GREEN under a real storm** (the four properties: human held,
    /// agent + CI shed, cross-tenant 0, bulkhead bounds provider load).
    #[test]
    fn run_notif_surge_is_green() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        // the provider bound (2) is TIGHTER than the gate's non-human budget (4), so the admitted
        // deliveries overflow the bulkhead — it sheds, bounding provider load below the lane budget.
        let mut bh = ProviderBulkhead::new("email", 2);
        let surging = tenant("noisy");
        let quiet = tenant("quiet");
        // a storm well past the non-human budget (4) AND past the provider bound (2) so both the
        // machine lanes AND the bulkhead must shed.
        let report = run_notif_surge(
            &mut gate,
            &mut bh,
            &surging,
            &quiet,
            50,
            50,
            NOTIF_SURGE_MULTIPLIER,
        );
        assert!(report.is_notif_d5_green(), "{}", report.summary());
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(
            report.surging_ci_shed_count > 0,
            "CI notification lane shed"
        );
        assert_eq!(report.surging_human_shed_count, 0, "human inbox-read held");
        assert!(report.surging_human_admitted, "surging tenant's human held");
        assert!(report.quiet_human_admitted, "quiet co-tenant's human held");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
        assert!(
            report.provider_peak_in_flight <= report.provider_bound,
            "the bulkhead bounded provider load (peak ≤ bound)"
        );
        assert!(
            report.provider_bulkhead_shed > 0,
            "the bulkhead shed the over-bound sends (fast-fail, not buffer)"
        );
    }

    /// **The surge gate is NOT vacuous — an UNBOUNDED lane (no shed) reads RED.**
    #[test]
    fn an_unbounded_lane_reads_red() {
        let huge = SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 200_000,
            retry_after_secs: 10,
        };
        let mut gate = NotifShedGate::with_budget(huge);
        // a huge bulkhead too — nothing sheds anywhere.
        let mut bh = ProviderBulkhead::new("email", 1_000_000);
        let report = run_notif_surge(
            &mut gate,
            &mut bh,
            &tenant("noisy"),
            &tenant("quiet"),
            100,
            100,
            NOTIF_SURGE_MULTIPLIER,
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "the unbounded lane swallowed the storm"
        );
        assert!(
            !report.is_notif_d5_green(),
            "an unbounded lane MUST read RED"
        );
    }

    /// **An unbounded delivery provider (no bulkhead shed) ALSO reads RED** — the bulkhead leg is not
    /// vacuous: if the provider load is never bounded, NOTIF-D5 is not green even with a shedding lane.
    #[test]
    fn an_unbounded_provider_reads_red() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        // a provider bound large enough that NO send ever sheds (unbounded provider load).
        let mut bh = ProviderBulkhead::new("email", 1_000_000);
        let report = run_notif_surge(
            &mut gate,
            &mut bh,
            &tenant("noisy"),
            &tenant("quiet"),
            50,
            50,
            NOTIF_SURGE_MULTIPLIER,
        );
        // the lanes still shed (the gate budget is small), but the provider bulkhead did NOT.
        assert!(report.surging_agent_shed_count > 0, "the lane still sheds");
        assert_eq!(
            report.provider_bulkhead_shed, 0,
            "the unbounded provider never shed"
        );
        assert!(
            !report.is_notif_d5_green(),
            "an unbounded provider (no bulkhead bound) MUST read RED"
        );
    }

    /// **Each NOTIF-D5 green condition is load-bearing — the predicate flips RED if ANY one is
    /// violated in isolation (the boundary the mutation floor pins).** Starting from a fully-green
    /// report, each single-field regression (agent not shedding, CI not shedding, a human shed, the
    /// surging/quiet human not admitted, cross-tenant impact, the bulkhead not shedding, the peak over
    /// the bound) must read RED. This catches a `>`→`>=` / `==`→`!=` boundary mutant in
    /// [`NotifSurgeReport::is_notif_d5_green`].
    #[test]
    fn each_notif_d5_condition_is_load_bearing() {
        let green = NotifSurgeReport {
            surging_agent_shed_count: 5,
            surging_ci_shed_count: 5,
            surging_human_shed_count: 0,
            surging_human_admitted: true,
            quiet_human_admitted: true,
            cross_tenant_impact: 0,
            provider_peak_in_flight: 2,
            provider_bound: 2,
            provider_bulkhead_shed: 3,
        };
        assert!(green.is_notif_d5_green(), "the baseline is green");

        // agent lane MUST shed (> 0): 0 → RED (catches `>`→`>=` at the agent boundary).
        assert!(!NotifSurgeReport {
            surging_agent_shed_count: 0,
            ..green.clone()
        }
        .is_notif_d5_green());
        // CI lane MUST shed (> 0): 0 → RED (catches `>`→`>=` at the CI boundary).
        assert!(!NotifSurgeReport {
            surging_ci_shed_count: 0,
            ..green.clone()
        }
        .is_notif_d5_green());
        // a human shed (> 0) → RED (catches `==`→`!=` at the human boundary).
        assert!(!NotifSurgeReport {
            surging_human_shed_count: 1,
            ..green.clone()
        }
        .is_notif_d5_green());
        // the surging tenant's human NOT admitted → RED.
        assert!(!NotifSurgeReport {
            surging_human_admitted: false,
            ..green.clone()
        }
        .is_notif_d5_green());
        // the quiet co-tenant's human NOT admitted → RED.
        assert!(!NotifSurgeReport {
            quiet_human_admitted: false,
            ..green.clone()
        }
        .is_notif_d5_green());
        // any cross-tenant impact → RED.
        assert!(!NotifSurgeReport {
            cross_tenant_impact: 1,
            ..green.clone()
        }
        .is_notif_d5_green());
        // the bulkhead MUST shed the excess (> 0): 0 → RED (catches `>`→`>=` at the bulkhead boundary).
        assert!(!NotifSurgeReport {
            provider_bulkhead_shed: 0,
            ..green.clone()
        }
        .is_notif_d5_green());
        // the provider peak OVER the bound → RED (the bulkhead must bound load).
        assert!(!NotifSurgeReport {
            provider_peak_in_flight: 3,
            ..green.clone()
        }
        .is_notif_d5_green());
        // an unbounded provider (bound 0) → RED.
        assert!(!NotifSurgeReport {
            provider_bound: 0,
            provider_peak_in_flight: 0,
            ..green.clone()
        }
        .is_notif_d5_green());
    }

    /// **The summary names every measured signal (observability is part of the pass).** A mutant that
    /// replaces the summary with an empty / fixed string is caught: the GREEN summary must contain the
    /// shed-counts, the cross-tenant impact, the provider bound, and the GREEN verdict.
    #[test]
    fn the_summary_carries_the_measured_signals() {
        let report = NotifSurgeReport {
            surging_agent_shed_count: 7,
            surging_ci_shed_count: 9,
            surging_human_shed_count: 0,
            surging_human_admitted: true,
            quiet_human_admitted: true,
            cross_tenant_impact: 0,
            provider_peak_in_flight: 2,
            provider_bound: 2,
            provider_bulkhead_shed: 4,
        };
        let s = report.summary();
        assert!(
            s.contains("agent_shed=7"),
            "names the agent shed count: {s}"
        );
        assert!(s.contains("ci_shed=9"), "names the CI shed count: {s}");
        assert!(
            s.contains("human_shed=0"),
            "names the human shed count: {s}"
        );
        assert!(
            s.contains("cross_tenant_impact=0"),
            "names cross-tenant impact: {s}"
        );
        assert!(
            s.contains("bulkhead_shed=4"),
            "names the bulkhead shed count: {s}"
        );
        assert!(s.contains("GREEN"), "names the verdict: {s}");
        assert!(s.starts_with("NOTIF-D5:"), "the artifact is labelled: {s}");
    }

    /// The floors are named (NOTIF-P26 EU provider + NOTIF-P27 erasure residual + the surge surface).
    #[test]
    fn the_floors_are_named() {
        assert_eq!(EU_DELIVERY_PROVIDER_FOLLOW_ON, "NOTIF-P26");
        assert_eq!(ERASURE_RESIDUAL_FOLLOW_ON, "NOTIF-P27");
        assert_eq!(NOTIF_SURGE_SURFACE, Surface::AgentMention);
        assert_eq!(NOTIF_SURGE_MULTIPLIER, 30);
    }
}
