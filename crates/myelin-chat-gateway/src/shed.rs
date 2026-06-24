//! # The Chat connection-tier shed gate — WIRES the substrate shed lane (CHAT-P10 / P-404).
//!
//! **Owning decisions:** `00-reconciliation-decisions.md` ADR-16 (backpressure + the protected
//! human lane + the shed order *speculative → batch/CI → agent → human-last*) + OQ-K (the
//! per-surface shed-budget table, named v1 FLOORS). Contract **1.11** (the shed order + the
//! per-surface budget floor — chat OWNS the connection-storm + agent-mention-storm SURFACES, not the
//! engine). Architecture `chat/architecture/02-internals-and-algorithms.md` §7 + §1.2 (the
//! firehose-ONLY live surfaces). VISION §3 (humans never queue behind agent runs).
//!
//! ## Coherence — REUSE, never re-author (EI-01 §7)
//! The shed lane, the run-class shed order, the per-surface budget table, and the `429 + Retry-After`
//! verdict are the SUBSTRATE's frozen primitive ([`myelin_substrate::shed`], P-S19 → P-035), already
//! wired by Git ([`GitFrontDoorShed`](myelin_git) over `Surface::GitFrontDoor`) and CI (the scheduler
//! fairness slice over `Surface::CiDispatch`). Chat does the SAME: it WIRES the existing
//! [`ShedLane`] over the existing [`Surface::ConnectionTier`] (presence/typing/read-state/human
//! message) + [`Surface::AgentMention`] (agent streaming partials) surfaces, reading the budget FROM
//! THE THRESHOLDS FILE ([`myelin_substrate::thresholds`]) — it re-authors NO lane, NO run-class, NO
//! budget table. Chat's ONLY authoring is the DERIVATION of a live frame's substrate
//! `(Surface, RunClass)` from the chat [`LiveSurface`] (the connection-tier-specific mapping) and the
//! placement of the admit/shed decision before the firehose publish.
//!
//! ## The chat connection-tier surfaces → substrate `(Surface, RunClass)`
//! The connection-storm + agent-mention-storm profiles (OQ-K's CHAT rows) map onto the substrate as:
//!
//! | chat [`LiveSurface`] | substrate [`Surface`] | substrate [`RunClass`] | shed rung |
//! |---|---|---|---|
//! | `Speculative` (presence) | `ConnectionTier` | `Speculative` | shed FIRST |
//! | `ReadState` (fine markers) | `ConnectionTier` | `Speculative` | shed first |
//! | `Typing` | `ConnectionTier` | `BatchCi` | shed after presence |
//! | `AgentPartial` (streaming) | `AgentMention` | `Agent` | the agent lane |
//! | `HumanMessage` (delivery) | `ConnectionTier` | `Human` | shed LAST (protected) |
//!
//! Presence/read-state are the lowest-promise ephemeral surfaces (shed first); typing is held to the
//! batch/CI rung (sheds after presence); the agent streaming partials ride the dedicated
//! `AgentMention` surface (humans never queue behind agent runs); the live human-message delivery is
//! the protected human lane on the connection tier (shed last). The substrate's graded run-class
//! ceilings then enforce *speculative → batch/CI → agent → human-last* with no second engine.
//!
//! ## FLOOR named
//! The per-surface budget NUMBERS are the OQ-K v1 FLOORS, read from `thresholds.toml`
//! (`[[shed_budgets]] surface = "ConnectionTier" / "AgentMention"`) — tuned by CHAT-D3/D4 in
//! M5-C-S1 / **CHAT-P26**, never edited green here. This module asserts the FLOOR PROPERTY (every
//! surface bounded, a reserved human lane, the shed order applied), not a tuned number.

use myelin_substrate::shed::{RunClass, ShedDecision, ShedLane, Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// **A Chat connection-tier live-delivery surface — the rung a live frame rides in the shed order.**
/// Each variant maps to a substrate `(Surface, RunClass)` via [`LiveSurface::substrate_surface`] +
/// [`LiveSurface::run_class`]; the SUBSTRATE [`ShedLane`] then applies the protected-human-lane shed
/// order. The variant declaration order is the chat-side shed order (lowest-promise first), kept in
/// the type so a new surface MUST pick a rung.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LiveSurface {
    /// **Speculative / presence** (`chat.presence.*`, incl. agent-presence classes) — the
    /// lowest-promise ephemeral surface. Connection tier, `Speculative` run-class → shed FIRST.
    Speculative,
    /// **Fine-grained read-state** (`chat.read_state.viewed`) — ephemeral viewed markers, recovered
    /// from the durable coarse summary if lost. Connection tier, `Speculative` run-class.
    ReadState,
    /// **Typing** (`chat.typing.*`) — ephemeral, self-heals on TTL. Connection tier, `BatchCi`
    /// run-class → sheds after presence/read-state, before the agent lane.
    Typing,
    /// **Agent streaming partials** (`agent.message.partial`) — the AGENT lane. Rides the dedicated
    /// `AgentMention` surface with the `Agent` run-class → shed before humans (the final durable
    /// message is the truth even if every partial is lost; humans never queue behind agent runs).
    AgentPartial,
    /// **Human message delivery** (the live `chat.message.created` delivery frame) — the PROTECTED
    /// human lane. Connection tier, `Human` run-class → shed LAST, behind the reserved human slots.
    HumanMessage,
}

impl LiveSurface {
    /// Every chat live-delivery surface, lowest-promise (shed first) → highest (shed last).
    pub const ALL: [LiveSurface; 5] = [
        LiveSurface::Speculative,
        LiveSurface::ReadState,
        LiveSurface::Typing,
        LiveSurface::AgentPartial,
        LiveSurface::HumanMessage,
    ];

    /// The SUBSTRATE [`Surface`] this chat surface's budget lives on (OQ-K's CHAT rows): the live
    /// connection-tier frames ride `ConnectionTier`; the agent streaming partials ride the
    /// `AgentMention` storm surface (humans never queue behind agent runs).
    pub fn substrate_surface(self) -> Surface {
        match self {
            LiveSurface::AgentPartial => Surface::AgentMention,
            _ => Surface::ConnectionTier,
        }
    }

    /// The SUBSTRATE [`RunClass`] this chat surface derives to (the shed order
    /// *speculative → batch/CI → agent → human-last*). Presence/read-state are the lowest-promise
    /// `Speculative`; typing is `BatchCi`; agent partials are `Agent`; the human message lane is the
    /// protected `Human`. The run-class is DATA the substrate lane reads, not a chat-side branch.
    pub fn run_class(self) -> RunClass {
        match self {
            LiveSurface::Speculative | LiveSurface::ReadState => RunClass::Speculative,
            LiveSurface::Typing => RunClass::BatchCi,
            LiveSurface::AgentPartial => RunClass::Agent,
            LiveSurface::HumanMessage => RunClass::Human,
        }
    }

    /// `true` iff this is the PROTECTED human lane (the live human-message delivery, shed LAST).
    pub fn is_protected_human_lane(self) -> bool {
        matches!(self, LiveSurface::HumanMessage)
    }
}

/// **The shed verdict for a chat live frame** — the typed form the live-delivery surface turns into
/// a publish-or-drop. `Shed` carries the substrate `Retry-After` (the agent runtime honours it,
/// ADR-16.3; ephemeral surfaces drop silently).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShedVerdict {
    /// Deliver the frame — a slot was admitted on the substrate lane (release it on drain).
    Deliver,
    /// Shed the frame — the surface is over its budget under the shed order. Carries the
    /// `Retry-After` (seconds) the substrate advertised.
    Shed {
        /// The substrate `Retry-After` (seconds) — the agent runtime honours it (ADR-16.3).
        retry_after_secs: u64,
    },
}

impl ShedVerdict {
    /// `true` iff the frame is delivered (a slot was admitted).
    pub fn is_delivered(self) -> bool {
        matches!(self, ShedVerdict::Deliver)
    }
}

/// **The Chat connection-tier shed governor — a thin wiring over TWO substrate [`ShedLane`]s
/// (CHAT-P10; ADR-16 / OQ-K / contract 1.11).** It composes the substrate lane over
/// [`Surface::ConnectionTier`] (presence/typing/read-state/human message) and
/// [`Surface::AgentMention`] (agent streaming partials), reading both budgets FROM THE THRESHOLDS
/// FILE. It re-authors NO lane / run-class / budget; it derives each chat [`LiveSurface`]'s substrate
/// `(Surface, RunClass)` and delegates the admit/shed decision to the substrate engine — per-tenant
/// (the blast-radius guarantee: one tenant's storm fills only that tenant's budget).
///
/// Holds NO durable state and publishes NOTHING — it is a pure admission decision the live-delivery
/// surface consults before a firehose publish (the placement of the shed gate, arch §7).
#[derive(Clone, Debug)]
pub struct ShedGovernor {
    /// The substrate lane over the chat connection tier (presence/typing/read-state/human message).
    connection_tier: ShedLane,
    /// The substrate lane over the chat agent-mention storm surface (agent streaming partials).
    agent_mention: ShedLane,
    /// `true` iff the tenant is under storm pressure (the connection-storm / frame-rate signal). The
    /// substrate lane is ALWAYS bounded (the §7.1 cap); the pressure flag drives whether the
    /// graded-shed thresholds engage early — modelled here by routing through the lane unconditionally
    /// (the lane's per-tenant cap is the always-on bound) while the flag is the observability signal a
    /// drill reads.
    under_pressure: bool,
}

impl ShedGovernor {
    /// **Open the chat shed governor with the OQ-K v1 FLOOR budgets** (the `ConnectionTier` +
    /// `AgentMention` rows of the substrate's frozen [`ShedBudgetTable::v1_floor`](myelin_substrate::shed::ShedBudgetTable::v1_floor)).
    /// The common constructor when the thresholds file is not threaded in (the floors ARE the file's
    /// seeds — same numbers); [`Self::from_thresholds`] is the production form that reads them from
    /// `thresholds.toml`.
    pub fn new() -> ShedGovernor {
        ShedGovernor {
            connection_tier: ShedLane::new(Surface::ConnectionTier),
            agent_mention: ShedLane::new(Surface::AgentMention),
            under_pressure: false,
        }
    }

    /// **Open the chat shed governor reading BOTH surface budgets from the thresholds file** (the
    /// prompt's "the shed budget is read from the thresholds file"). A missing `ConnectionTier` /
    /// `AgentMention` shed-budget row is a LOUD error (the gate refuses to open against a guessed
    /// budget — EI-01 §3), never a silent default. Mirrors `GitFrontDoorShed::from_thresholds`.
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<ShedGovernor, String> {
        let conn = thresholds
            .shed_budget(Surface::ConnectionTier)
            .map_err(|e| format!("chat ConnectionTier shed budget unavailable: {e}"))?;
        let agent = thresholds
            .shed_budget(Surface::AgentMention)
            .map_err(|e| format!("chat AgentMention shed budget unavailable: {e}"))?;
        Ok(ShedGovernor {
            connection_tier: ShedLane::with_budget(Surface::ConnectionTier, conn),
            agent_mention: ShedLane::with_budget(Surface::AgentMention, agent),
            under_pressure: false,
        })
    }

    /// Open the governor against EXPLICIT budgets (used by drills to drive the boundary at a small,
    /// deterministic budget without editing the thresholds file).
    pub fn with_budgets(
        connection_tier: SurfaceBudget,
        agent_mention: SurfaceBudget,
    ) -> ShedGovernor {
        ShedGovernor {
            connection_tier: ShedLane::with_budget(Surface::ConnectionTier, connection_tier),
            agent_mention: ShedLane::with_budget(Surface::AgentMention, agent_mention),
            under_pressure: false,
        }
    }

    /// Flip the per-tenant storm-pressure signal (the connection-storm / frame-rate threshold). An
    /// observability signal a drill reads; the substrate lane's per-tenant cap is the always-on bound.
    pub fn set_under_pressure(&mut self, under_pressure: bool) {
        self.under_pressure = under_pressure;
    }

    /// `true` iff the tenant is currently under storm pressure.
    pub fn under_pressure(&self) -> bool {
        self.under_pressure
    }

    /// The substrate lane backing a chat surface (the `ConnectionTier` lane for everything except the
    /// agent partials, which ride the `AgentMention` lane).
    fn lane_mut(&mut self, surface: LiveSurface) -> &mut ShedLane {
        match surface.substrate_surface() {
            Surface::AgentMention => &mut self.agent_mention,
            _ => &mut self.connection_tier,
        }
    }

    fn lane(&self, surface: LiveSurface) -> &ShedLane {
        match surface.substrate_surface() {
            Surface::AgentMention => &self.agent_mention,
            _ => &self.connection_tier,
        }
    }

    /// **Admit a live frame on `surface` for `tenant` (the protected-human-lane shed order).** Derives
    /// the substrate `(Surface, RunClass)` and delegates to the substrate [`ShedLane::admit`] — the
    /// engine applies the shed order *speculative → batch/CI → agent → human-last*, per-tenant, with
    /// the human lane protected (a `Human` frame is shed only in true saturation; a non-human lane is
    /// shed first). Returns [`ShedVerdict::Deliver`] (a slot taken — release via [`Self::on_drained`])
    /// or [`ShedVerdict::Shed`].
    pub fn admit(&mut self, tenant: &TenantId, surface: LiveSurface) -> ShedVerdict {
        let class = surface.run_class();
        match self.lane_mut(surface).admit(tenant, class) {
            ShedDecision::Admit => ShedVerdict::Deliver,
            ShedDecision::Shed { retry_after_secs } => ShedVerdict::Shed { retry_after_secs },
        }
    }

    /// Release a slot a prior [`Self::admit`] took for `(tenant, surface)` — call when the connection
    /// pumps the frame to its socket / the consumer acks it, so the lane recovers after the storm.
    pub fn on_drained(&mut self, tenant: &TenantId, surface: LiveSurface) {
        let class = surface.run_class();
        self.lane_mut(surface).release(tenant, class);
    }

    /// The current per-tenant in-flight depth on a chat surface (admitted not yet drained).
    pub fn in_flight(&self, tenant: &TenantId, surface: LiveSurface) -> u32 {
        self.lane(surface).in_flight(tenant)
    }

    /// The cumulative shed count for a chat surface's run-class (the contract-1.8 `shed-count per
    /// lane` survival signal — the drill green artifact: `human-lane == 0 shed`, presence/agent `> 0`).
    pub fn shed_count(&self, surface: LiveSurface) -> u64 {
        self.lane(surface).shed_count(surface.run_class())
    }
}

impl Default for ShedGovernor {
    fn default() -> Self {
        ShedGovernor::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    /// **The chat live surfaces map onto the substrate `(Surface, RunClass)` in the shed order
    /// speculative → batch/CI → agent → human-last (the REUSED substrate engine, no second order).**
    #[test]
    fn surfaces_map_to_substrate_in_shed_order() {
        // presence/read-state → ConnectionTier + Speculative (shed first).
        assert_eq!(
            LiveSurface::Speculative.substrate_surface(),
            Surface::ConnectionTier
        );
        assert_eq!(LiveSurface::Speculative.run_class(), RunClass::Speculative);
        assert_eq!(LiveSurface::ReadState.run_class(), RunClass::Speculative);
        // typing → batch/CI rung.
        assert_eq!(LiveSurface::Typing.run_class(), RunClass::BatchCi);
        // agent partials ride the AgentMention surface + the Agent lane.
        assert_eq!(
            LiveSurface::AgentPartial.substrate_surface(),
            Surface::AgentMention
        );
        assert_eq!(LiveSurface::AgentPartial.run_class(), RunClass::Agent);
        // the human message lane is the protected Human lane on the connection tier.
        assert_eq!(
            LiveSurface::HumanMessage.substrate_surface(),
            Surface::ConnectionTier
        );
        assert_eq!(LiveSurface::HumanMessage.run_class(), RunClass::Human);
        assert!(LiveSurface::HumanMessage.is_protected_human_lane());

        // the substrate run-class order IS the shed order (a lower class sheds first).
        assert!(RunClass::Speculative < RunClass::BatchCi);
        assert!(RunClass::BatchCi < RunClass::Agent);
        assert!(RunClass::Agent < RunClass::Human);
    }

    /// **Under storm, presence sheds BEFORE the human lane; the human lane holds (0 human-lane drops)
    /// — the substrate engine applied through the chat surfaces.** A small ConnectionTier budget
    /// drives the boundary.
    #[test]
    fn presence_sheds_first_human_lane_holds() {
        // cap 5, reserve 2 for humans → non-human budget 3.
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 5,
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
        let t = tenant();

        // fill the connection-tier non-human budget with presence frames until presence sheds.
        let mut presence_shed = false;
        for _ in 0..8 {
            match gov.admit(&t, LiveSurface::Speculative) {
                ShedVerdict::Deliver => {}
                ShedVerdict::Shed { .. } => {
                    presence_shed = true;
                    break;
                }
            }
        }
        assert!(presence_shed, "presence sheds under pressure");

        // the HUMAN message lane STILL delivers (it uses the reserved slots — shed last).
        assert!(
            gov.admit(&t, LiveSurface::HumanMessage).is_delivered(),
            "the human lane holds while presence sheds"
        );
        assert_eq!(
            gov.shed_count(LiveSurface::HumanMessage),
            0,
            "0 human-lane drops"
        );
        assert!(
            gov.shed_count(LiveSurface::Speculative) > 0,
            "presence shed > 0"
        );
    }

    /// **The agent lane sheds before the human lane (humans never queue behind agent runs).** The
    /// agent partials ride the dedicated AgentMention surface; saturate it → it sheds; the human
    /// message on the connection tier still delivers.
    #[test]
    fn agent_lane_sheds_before_human_lane() {
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 5,
            human_lane_reservation: 2,
            retry_after_secs: 3,
        };
        let agent = SurfaceBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 0,
            retry_after_secs: 10,
        };
        let mut gov = ShedGovernor::with_budgets(conn, agent);
        gov.set_under_pressure(true);
        let t = tenant();

        // saturate the agent-mention surface → the agent lane sheds.
        let mut agent_shed = false;
        for _ in 0..6 {
            if let ShedVerdict::Shed { .. } = gov.admit(&t, LiveSurface::AgentPartial) {
                agent_shed = true;
                break;
            }
        }
        assert!(agent_shed, "the agent lane sheds when over budget");
        // the human message lane (a DIFFERENT substrate surface, untouched) still delivers.
        assert!(
            gov.admit(&t, LiveSurface::HumanMessage).is_delivered(),
            "the human lane holds while the agent lane sheds"
        );
    }

    /// **Per-tenant: one tenant's storm never sheds another tenant's human (the substrate
    /// blast-radius guarantee, inherited by the chat wiring).**
    #[test]
    fn shedding_is_per_tenant() {
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 1,
            retry_after_secs: 3,
        };
        let agent = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 0,
            retry_after_secs: 10,
        };
        let mut gov = ShedGovernor::with_budgets(conn, agent);
        gov.set_under_pressure(true);
        let noisy = TenantId("noisy".into());
        let quiet = TenantId("quiet".into());

        // saturate the noisy tenant's connection tier.
        for _ in 0..8 {
            let _ = gov.admit(&noisy, LiveSurface::Speculative);
        }
        // the quiet tenant's human is completely unaffected.
        assert_eq!(gov.in_flight(&quiet, LiveSurface::HumanMessage), 0);
        assert!(
            gov.admit(&quiet, LiveSurface::HumanMessage).is_delivered(),
            "one tenant's storm must NEVER shed another tenant's human"
        );
    }

    /// **The v1 FLOOR governor (from the substrate frozen budget table) opens with both chat surfaces
    /// bounded.** The common `new()` constructor reads the substrate `v1_floor` budgets.
    #[test]
    fn v1_floor_governor_opens_with_bounded_surfaces() {
        let mut gov = ShedGovernor::new();
        let t = tenant();
        // both surfaces admit at least one frame (bounded + non-degenerate).
        assert!(gov.admit(&t, LiveSurface::HumanMessage).is_delivered());
        assert!(gov.admit(&t, LiveSurface::AgentPartial).is_delivered());
        assert!(gov.admit(&t, LiveSurface::Speculative).is_delivered());
    }
}
