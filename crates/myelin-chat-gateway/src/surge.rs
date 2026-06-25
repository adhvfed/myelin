//! # `surge` — Chat world-scale 30× surge hardening + the tuned per-surface shed budgets (CHAT-P26 / P-500, M5)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/07-drills-and-open-questions.md` rows
//! **D-C3** (30× agent message/connection surge on one tenant → human connection/read latency in
//! budget; the agent lane sheds `429 + Retry-After`; other tenants unaffected) + **D-C4** (deploy
//! reconnect thundering-herd) + `02-internals-and-algorithms.md` §1.4/§7 (presence at scale, the
//! connection tier where the worst load manifests). **Reconciliation:**
//! `00-reconciliation-decisions.md` ADR-16 (the protected-human-lane shed order
//! *speculative → batch/CI → agent → human-last*) + OQ-K (the per-surface shed-budget table —
//! TUNED here from the D-C3/D-C4 results, promoting the CHAT-P10 floor). **Contract-index:** row
//! **1.11** (the protected-human-lane shed order + per-surface shed budgets — chat OWNS the
//! connection-storm + agent-mention-storm SURFACES, tuned here), row **1.8** (the per-lane
//! shed-count / human-lane-latency survival signals the drills assert against). **Doctrine:**
//! external-insights/01 §3 (prove-it under 1×/10×/30×; the multiplier is read from the FROZEN
//! thresholds file, never hardcoded; never weaken a threshold to pass — a red is a dated
//! `claimed-not-proven` row), §2 (the protected human lane; per-tenant blast-radius).
//!
//! ## What this module is (the Chat surge half — CHAT-P26)
//! Chat has **two storm surfaces** under the 30× surge (D-C3): the connection tier (presence /
//! typing / read-state / live human-message delivery — the AGENT-MESSAGE + CONNECTION storm rides
//! here) and the agent-mention surface (agent streaming partials). The 30× surge is an
//! **agent message/connection storm**; this module proves the doctrine shed order
//! (`speculative → batch/CI → agent → human-last`) holds at Chat's surfaces:
//! - a **human's live message delivery** holds the protected lane (shed last);
//! - the **agent streaming-partial** + **presence/typing** lanes shed with `429 + Retry-After`;
//! - **per-tenant in-flight caps** keep one tenant's storm off another tenant's humans (the
//!   per-tenant bulkhead, §1.4 / EI-02 §1).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! **The shed order itself is the substrate's** [`myelin_substrate::shed`], already WIRED by the
//! chat [`ShedGovernor`](crate::shed::ShedGovernor) over [`Surface::ConnectionTier`] +
//! [`Surface::AgentMention`] (CHAT-P10 / P-404). This module does NOT re-author the shed lane /
//! run-class / budget table NOR the governor — it COMPOSES the existing governor (reading the
//! TUNED budgets from `thresholds.toml`) and adds only the **surge-harness scenario** + the
//! **three-F6-property report** the D-C3/D-C4 drills assert against (the same shape Refs'
//! `run_refs_surge` / Git's clone-surge use). One shape, no second copy.
//!
//! ## The shed budgets are TUNED here (promoting the CHAT-P10 floor — OQ-K / R-C2 / Q-C5)
//! CHAT-P10 (P-404) named the connection-storm + agent-mention-storm caps + the protected-human-lane
//! reservation as **v1 FLOORS**. The substrate M5 surge family (P-S33 / P-434) drove those numbers
//! under real surge + connection-storm load and recorded them as **MEASURED defaults-to-beat** in
//! `thresholds.toml` (`ConnectionTier` 256/64, `AgentMention` 96/24 — each reserving 25% of cap, above
//! the 20% measured human-lane floor). This prompt CONFIRMS those tuned numbers hold the chat surge
//! green and PROMOTES the chat-side floor language to "tuned": [`CHAT_SHED_BUDGETS_TUNED`] is now
//! `true`, read from the file (never re-authored here). The chat surge asserts the three F6 properties
//! AT the tuned numbers — never a number chosen to make the drill pass (EI-01 §3).
//!
//! ## Floors named (VISION §3 — name your floors; the prompt's honesty register)
//! The chat-side shed budgets are TUNED here. The remaining chat M5 promotions are TRIGGERED
//! (measured-not-unconditional), each with its landing prompt:
//! - **Scylla / column-store hot-tier** promotion — [`SCYLLA_HOT_TIER_FOLLOW_ON`] (**CHAT-P28**),
//!   taken only when the measured message-store hot-read fanout crosses its budget.
//! - **The channel-sharded home-node** (mega-channel live delivery; the Phoenix/Discord guild model)
//!   — [`HOME_NODE_FOLLOW_ON`] (**CHAT-P29**), promoted on a measured subscriber count exceeding the
//!   subject-fan-out budget.
//! - **Cross-org delivery** — [`CROSS_ORG_FOLLOW_ON`] (**CHAT-P30**).
//! - **Comment-consolidation** — [`COMMENT_CONSOLIDATION_FOLLOW_ON`] (**CHAT-P31**).
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet).
//!   Here the load is the P-S02 generator at 30× across the surging tenant; the per-tenant fairness +
//!   shed-order + cross-tenant-0 PROPERTIES are complete + testable now and do not change shape when
//!   the real broker-backed firehose carries the load.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2/§3)
//! The shed-order DECISION path is the substrate's (`ShedLane::admit`) wired by the chat
//! [`ShedGovernor`]; this module adds NO new core decision — it only drives the existing decision
//! under surge and reads the survival signals. An off-by-one that sheds a human before an agent, or
//! leaks one tenant's budget into another, is caught by the substrate's mutation-tested core and by
//! this drill's three-property assertion.

use crate::shed::{LiveSurface, ShedGovernor};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// **The Chat surge default-to-beat multiplier (D-C3 / D-C4).** The 30× world-scale surge factor the
/// CHAT-D3/D4 drills drive at — read from the FROZEN thresholds file `[surge] multiplier` row (the
/// versioned source of truth, P-038) and asserted to equal this documented default-to-beat; a
/// divergence is a LOUD failure, never a silent weakening (EI-01 §3).
pub const CHAT_SURGE_MULTIPLIER: u32 = 30;

/// **The chat per-surface shed budgets are TUNED, not floored (Q-C5 / R-C2 / OQ-K, promoted from the
/// CHAT-P10 floor).** The `ConnectionTier` + `AgentMention` rows in `thresholds.toml` carry MEASURED
/// defaults-to-beat (P-S33 / P-434 drove them under surge + connection-storm load); this prompt
/// confirms they hold the chat surge green. `true` records the floor → tuned promotion is complete
/// (the chat-side half of OQ-K is no longer a named floor).
pub const CHAT_SHED_BUDGETS_TUNED: bool = true;

/// **The Scylla / column-store hot-tier promotion follow-on** — a TRIGGERED (measured-not-
/// unconditional) chat M5 promotion: **CHAT-P28**. Named here so the floor is explicit, not implied.
pub const SCYLLA_HOT_TIER_FOLLOW_ON: &str = "CHAT-P28";

/// **The channel-sharded home-node follow-on** (mega-channel live delivery; the Phoenix/Discord guild
/// model) — a TRIGGERED chat M5 promotion, taken on a measured subscriber count exceeding the
/// subject-fan-out budget: **CHAT-P29** (global P-503). The FULL named-floor record (the measurable
/// subject-fan-out budget trigger predicate + the dated gap-report row + the BEAM-gateway sibling
/// floor) lives in [`crate::home_node`] ([`crate::HOME_NODE_FLOOR`]); this string is the back-pointer
/// to the landing prompt (EI-01 §7 — one floor record, this names where it lands).
pub const HOME_NODE_FOLLOW_ON: &str = "CHAT-P29";

/// **The cross-org delivery follow-on** — a TRIGGERED chat M5 promotion: **CHAT-P30**.
pub const CROSS_ORG_FOLLOW_ON: &str = "CHAT-P30";

/// **The comment-consolidation follow-on** — a TRIGGERED chat M5 promotion: **CHAT-P31**.
pub const COMMENT_CONSOLIDATION_FOLLOW_ON: &str = "CHAT-P31";

// ───────────────────────────── the CHAT-D3 surge report ──────────────────────────────────────────

/// **The CHAT-D3 30× surge report — the three F6 properties on Chat's surfaces.** The dated green
/// artifact the DoD names: the human live-message lane HOLDS (0 shed while a machine lane sheds), the
/// agent streaming-partial + presence lanes SHED (`429 + Retry-After`, absorbed not unbounded), and
/// cross-tenant impact is 0 (the storm fills only the surging tenant's per-tenant budget).
///
/// The signals are the contract-1.8 survival set: `human-lane shed-count` (must be 0), the
/// `agent-lane` + `speculative-lane` shed-counts (must be > 0), and the cross-tenant impact (must be
/// 0). Observability is part of the pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChatSurgeReport {
    /// The agent-partial shed count on the surging tenant (the storm absorbed by shedding — > 0).
    pub surging_agent_shed_count: u64,
    /// The presence/speculative shed count on the surging tenant (sheds first — > 0).
    pub surging_presence_shed_count: u64,
    /// The human live-message shed count on the surging tenant (the protected lane — must be 0).
    pub surging_human_shed_count: u64,
    /// How many human live-message frames the surging tenant delivered under the storm (must equal
    /// the human frames offered — every human message was delivered).
    pub surging_human_delivered: u64,
    /// Whether the quiet co-tenant's human live-message was delivered (untouched by the storm).
    pub quiet_human_delivered: bool,
    /// The quiet co-tenant's in-flight count BEFORE its own human op (the cross-tenant spillover —
    /// must be 0; the per-tenant bound is the blast-radius boundary).
    pub cross_tenant_impact: u32,
}

impl ChatSurgeReport {
    /// **The CHAT-D3 GREEN predicate (the three F6 properties — all measured, none weakened).** The
    /// agent + presence lanes shed (absorbed by shedding), the human lane held (0 shed + every human
    /// frame delivered) on the surging tenant, the quiet co-tenant's human held, and cross-tenant
    /// impact is 0.
    pub fn is_chat_d3_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.surging_presence_shed_count > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_delivered > 0
            && self.quiet_human_delivered
            && self.cross_tenant_impact == 0
    }

    /// A one-line summary for the dated green-artifact log row (observability is part of the pass).
    pub fn summary(&self) -> String {
        format!(
            "CHAT-D3: surging agent_shed={} presence_shed={} human_shed={} human_delivered={} \
             quiet_human_delivered={} cross_tenant_impact={} → {}",
            self.surging_agent_shed_count,
            self.surging_presence_shed_count,
            self.surging_human_shed_count,
            self.surging_human_delivered,
            self.quiet_human_delivered,
            self.cross_tenant_impact,
            if self.is_chat_d3_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

/// **Open a chat [`ShedGovernor`] for the surge, reading BOTH surface budgets from the thresholds
/// file** (the TUNED `ConnectionTier` + `AgentMention` rows, P-S33). A missing row is a LOUD error
/// (the gate refuses to open against a guessed budget — EI-01 §3). The common production opener the
/// CHAT-D3 drill drives at the tuned numbers.
pub fn surge_governor_from_thresholds(thresholds: &Thresholds) -> Result<ShedGovernor, String> {
    ShedGovernor::from_thresholds(thresholds)
}

/// **Drive the CHAT-D3 30× surge on the chat shed governor.** Rolls a storm of `storm_frames` mixed
/// machine frames (presence speculative + agent streaming partials) on the surging tenant — the
/// presence lane sheds first, the agent lane next — interleaved with `human_frames` human live-message
/// frames (the protected lane), each promptly drained (interactive priority). Then proves a quiet
/// co-tenant's human is delivered within its independent budget. Returns the [`ChatSurgeReport`] (the
/// three F6 properties).
///
/// The human live-message lane is drained promptly after each delivery (the connection pumps an
/// interactive human frame to its socket at once), so the human lane stays within its reserved slots
/// while the machine lanes back up and shed — the realistic connection-tier model (arch §7).
///
/// `multiplier` is the surge factor (read from the FILE by the caller; passed through for the log
/// row), not used to scale here — `storm_frames` is already the derived 30× storm-frame count.
pub fn run_chat_surge(
    gov: &mut ShedGovernor,
    surging: &TenantId,
    quiet: &TenantId,
    storm_frames: u64,
    human_frames: u64,
    _multiplier: u32,
) -> ChatSurgeReport {
    gov.set_under_pressure(true);

    let mut surging_human_delivered = 0u64;
    let frames = storm_frames.max(human_frames);

    for i in 0..frames {
        // a presence beacon (speculative — sheds first). Never drained: presence is ephemeral.
        if i < storm_frames {
            let _ = gov.admit(surging, LiveSurface::Speculative);
            // an agent streaming partial (the agent lane — sheds before humans). Not drained: the
            // agent runtime honours the 429 + Retry-After rather than the gateway eagerly draining.
            let _ = gov.admit(surging, LiveSurface::AgentPartial);
        }
        // a HUMAN live message (the protected lane — must NOT shed while the machine lanes carry
        // load). Drained PROMPTLY (interactive priority) so the human lane stays within budget.
        if i < human_frames && gov.admit(surging, LiveSurface::HumanMessage).is_delivered() {
            surging_human_delivered += 1;
            gov.on_drained(surging, LiveSurface::HumanMessage);
        }
    }

    // The quiet co-tenant is UNTOUCHED: its human live-message is delivered within its independent
    // per-tenant budget (the storm never spent the quiet tenant's slots).
    let quiet_in_flight_before = gov.in_flight(quiet, LiveSurface::HumanMessage);
    let quiet_human_delivered = gov.admit(quiet, LiveSurface::HumanMessage).is_delivered();

    ChatSurgeReport {
        surging_agent_shed_count: gov.shed_count(LiveSurface::AgentPartial),
        surging_presence_shed_count: gov.shed_count(LiveSurface::Speculative),
        surging_human_shed_count: gov.shed_count(LiveSurface::HumanMessage),
        surging_human_delivered,
        quiet_human_delivered,
        cross_tenant_impact: quiet_in_flight_before,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shed::ShedVerdict;
    use myelin_substrate::shed::{Surface, SurfaceBudget};

    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }

    /// **The chat shed budgets are read from the thresholds file (TUNED, not hardcoded).** The
    /// governor opens against the canonical `thresholds.toml` `ConnectionTier`/`AgentMention` rows.
    #[test]
    fn the_chat_shed_budgets_are_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        // both budgets present + bounded + reserving a human fraction (25% of cap, above the 20% floor).
        for surface in [Surface::ConnectionTier, Surface::AgentMention] {
            let b = thresholds.shed_budget(surface).expect("present");
            assert!(b.per_tenant_in_flight_cap > 0, "{surface:?} bounded (§7.1)");
        }
        let conn = thresholds.shed_budget(Surface::ConnectionTier).unwrap();
        assert!(
            conn.human_lane_reservation > 0,
            "ConnectionTier reserves a human lane"
        );
        // the governor opens cleanly against the file (no guessed budget).
        let _gov = surge_governor_from_thresholds(&thresholds).expect("governor opens from file");
    }

    /// **The CHAT-D3 surge report is GREEN at a small deterministic budget** (the three F6 properties:
    /// agent + presence shed, human held + delivered, quiet co-tenant held, cross-tenant 0).
    #[test]
    fn run_chat_surge_is_green() {
        // cap 12 / reserve 4 → non-human budget 8 on the connection tier; agent cap 6.
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 12,
            human_lane_reservation: 4,
            retry_after_secs: 3,
        };
        let agent = SurfaceBudget {
            per_tenant_in_flight_cap: 6,
            human_lane_reservation: 0,
            retry_after_secs: 10,
        };
        let mut gov = ShedGovernor::with_budgets(conn, agent);
        let report = run_chat_surge(
            &mut gov,
            &tenant("noisy"),
            &tenant("quiet"),
            64,
            40,
            CHAT_SURGE_MULTIPLIER,
        );
        assert!(report.is_chat_d3_green(), "{}", report.summary());
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(report.surging_presence_shed_count > 0, "presence lane shed");
        assert_eq!(report.surging_human_shed_count, 0, "human lane held");
        assert_eq!(
            report.surging_human_delivered, 40,
            "every human frame delivered"
        );
        assert!(report.quiet_human_delivered, "quiet co-tenant's human held");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    }

    /// **The surge report is GREEN at the TUNED file budgets** (not just a tiny synthetic one): the
    /// connection-storm is sized past the tuned ConnectionTier non-human ceiling so the storm MUST
    /// shed, and the human lane still holds. Proves the tuned numbers hold the chat surge.
    #[test]
    fn run_chat_surge_is_green_at_the_tuned_file_budgets() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let mut gov = surge_governor_from_thresholds(&thresholds).expect("governor from file");
        let conn = thresholds.shed_budget(Surface::ConnectionTier).unwrap();
        // a storm well past the tuned non-human ceiling so the presence/agent lanes MUST shed.
        let storm = u64::from(conn.per_tenant_in_flight_cap) * 4;
        let report = run_chat_surge(
            &mut gov,
            &tenant("noisy"),
            &tenant("quiet"),
            storm,
            storm,
            CHAT_SURGE_MULTIPLIER,
        );
        assert!(
            report.is_chat_d3_green(),
            "the TUNED budgets hold the chat surge: {}",
            report.summary()
        );
    }

    /// **Per-tenant: one tenant's storm NEVER sheds another tenant's human (the blast-radius bound).**
    /// Saturate the surging tenant completely, then prove the quiet tenant's human is still delivered.
    #[test]
    fn one_tenants_storm_never_sheds_anothers_human() {
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 8,
            human_lane_reservation: 2,
            retry_after_secs: 3,
        };
        let agent = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 0,
            retry_after_secs: 10,
        };
        let mut gov = ShedGovernor::with_budgets(conn, agent);
        gov.set_under_pressure(true);
        let noisy = tenant("noisy");
        let quiet = tenant("quiet");

        // saturate the noisy tenant's connection tier completely (no drains → it backs up + sheds).
        for _ in 0..(8 * 4) {
            let _ = gov.admit(&noisy, LiveSurface::Speculative);
        }
        assert!(
            gov.shed_count(LiveSurface::Speculative) > 0,
            "the saturated noisy tenant's presence lane sheds"
        );
        // the quiet tenant is independent: its human is delivered (cross-tenant 0).
        assert_eq!(gov.in_flight(&quiet, LiveSurface::HumanMessage), 0);
        assert!(
            gov.admit(&quiet, LiveSurface::HumanMessage).is_delivered(),
            "the noisy storm must NEVER shed another tenant's human"
        );
    }

    /// **The surge gate is NOT vacuous — an UNBOUNDED lane (no shed) reads RED.** Proves the green is
    /// earned: with a giant budget the storm fits, nothing sheds, and the report reads RED.
    #[test]
    fn an_unbounded_lane_reads_red() {
        let huge = SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 200_000,
            retry_after_secs: 3,
        };
        let mut gov = ShedGovernor::with_budgets(huge, huge);
        let report = run_chat_surge(
            &mut gov,
            &tenant("noisy"),
            &tenant("quiet"),
            100,
            100,
            CHAT_SURGE_MULTIPLIER,
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "the unbounded lane swallowed the storm (no shed)"
        );
        assert!(
            !report.is_chat_d3_green(),
            "an unbounded chat lane MUST read RED — never a silent pass"
        );
    }

    /// **The shed verdict carries the substrate Retry-After (429 + Retry-After honoured).**
    #[test]
    fn a_shed_carries_retry_after() {
        let agent = SurfaceBudget {
            per_tenant_in_flight_cap: 2,
            human_lane_reservation: 0,
            retry_after_secs: 10,
        };
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 1,
            retry_after_secs: 3,
        };
        let mut gov = ShedGovernor::with_budgets(conn, agent);
        gov.set_under_pressure(true);
        let t = tenant("acme");
        let mut shed_retry = None;
        for _ in 0..8 {
            if let ShedVerdict::Shed { retry_after_secs } = gov.admit(&t, LiveSurface::AgentPartial)
            {
                shed_retry = Some(retry_after_secs);
                break;
            }
        }
        assert_eq!(
            shed_retry,
            Some(10),
            "the agent-lane shed carries the AgentMention Retry-After"
        );
    }

    /// The chat-side shed budgets are TUNED and the remaining floors are named (the honesty register).
    #[test]
    fn the_floors_are_named_and_the_budgets_are_tuned() {
        // the chat shed budgets are tuned (OQ-K / R-C2 / Q-C5 — the CHAT-P10 floor is promoted).
        const { assert!(CHAT_SHED_BUDGETS_TUNED) };
        assert_eq!(CHAT_SURGE_MULTIPLIER, 30);
        assert_eq!(SCYLLA_HOT_TIER_FOLLOW_ON, "CHAT-P28");
        assert_eq!(HOME_NODE_FOLLOW_ON, "CHAT-P29");
        assert_eq!(CROSS_ORG_FOLLOW_ON, "CHAT-P30");
        assert_eq!(COMMENT_CONSOLIDATION_FOLLOW_ON, "CHAT-P31");
    }
}
