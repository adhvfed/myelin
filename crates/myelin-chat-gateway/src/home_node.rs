//! # `home_node` — the CHAT-M5 measured-trigger-gated mega-channel channel-sharded home-node floor
//! (CHAT-P29 → global P-503; M5-C-S3; the named M4-C2 delivery floor, R-5).
//!
//! **Status note (DATED 2026-06-25; re-date on any change — a claim that outlives its verification
//! misleads the next agent, VISION §3 / EI-01 §1).** This module is a *gap-report*: it NAMES the
//! channel-sharded home-node escalation (M5-C-S3; the Phoenix/Discord guild model in Rust +
//! consistent-hash) and records whether its measured trigger has FIRED. Per VISION §3 (name-your-floors
//! — promotion is *triggered*, never premature), EI-04 §4/§5 ("don't add it before the volume is
//! *measured*"), and the chat architecture's own measure-before-shard mandate
//! (`05-hard-problems.md` §1 / §4; ADR-10), a promotion whose trigger has NOT fired stays a **named
//! floor — it is NOT built speculatively**, and its trigger status is recorded here, dated.
//!
//! ## Why a gap-report, not a build (the trigger has not fired)
//! Mega-channel live delivery is **firehose subject fan-out with per-view scope bounding** (the v1
//! seam, `05-hard-problems.md` §1 floor (b)): a post to a channel fans out over the bounded
//! `channel:<id>` firehose subject ([`crate::delivery::LiveDelivery`]), and each viewer's
//! subscription is per-view scope-bounded ([`crate::ChatGateway::subscribe`] via the `*`-rejecting
//! `chat_channel_scope` chokepoint). The **channel-sharded home-node** (the Phoenix/Discord guild
//! model in Rust + consistent-hash) is the **measured escalation (R-5)**, taken ONLY when a channel's
//! **measured subscriber count exceeds the subject-fan-out budget** (`05-hard-problems.md` §8 table;
//! roadmap §5 table row "Firehose subject fan-out … → Channel-sharded home-node").
//!
//! That signal has NOT been measured to cross the budget. The chat M5 surge family (CHAT-P26 / P-500,
//! [`crate::surge`]) drove the 30× **agent-message / connection storm** and measured the gateway
//! **SHED budgets** (the `ConnectionTier` / `AgentMention` lanes — the delivery-side *fairness* signal
//! under storm), NOT a per-channel **subscriber-count** reading crossing a subject-fan-out budget. The
//! 30× surge is a message/connection *rate* storm on bounded-membership channels, not a single
//! mega-channel whose subscriber fan-out blows the subject budget. No subscriber-count-vs-budget
//! measurement that crosses exists, so the v1 **firehose subject fan-out is RETAINED** and the
//! promotion **remains a named floor**. Building it now would be exactly the "add it before the volume
//! is measured" anti-pattern EI-04 §5 forbids and the "floor that masquerades as done" VISION §3
//! forbids.
//!
//! ## The trigger is a MEASURABLE predicate, not a hand-typed boolean
//! So the floor promotes on a *measured* signal (not a guess), the trigger is realised as a real,
//! evaluable predicate — [`SubjectFanOutBudget::exceeded_by`] over a measured
//! [`MeasuredFanOut::subscriber_count`]. When a cell's telemetry records a channel whose subscriber
//! count crosses the budget, `exceeded_by` returns `true` and the promotion is unblocked on *that*
//! reading — the same red-until-proven shape the Scylla hot-tier floor uses
//! ([`myelin_chat::scylla_followon`]). At this prompt's execution NO such reading exists, so the
//! status is [`TriggerStatus::NotFired`] and `built == false`.
//!
//! ## What is already BUILT — the seam the promotion swaps behind (no rewrite)
//! The escalation is a **gateway-process** change behind the **unchanged firehose resume-cursor
//! protocol** (contract 3.5) and the **unchanged cross-language harness shim** (contract 1.7) — NOT a
//! protocol redesign:
//! - the firehose `subscribe(stream, scope, cursor?)` / `resume(stream, scope, last_seq)` surface
//!   ([`myelin_events::Firehose`]) is identical whether the channel fans out over one subject or is
//!   sharded across home-node-owned subjects — the resume-cursor (`seq`) the CHAT-D1 drill pins is
//!   carried the same way (so CHAT-D1 was *written to survive the swap*, EI-01 §3);
//! - the per-view bounded `channel:<id>` scope (the `*`-rejecting chokepoint,
//!   [`myelin_chat::glue::chat_channel_scope`]) is unchanged — the home-node owns the channel's shard
//!   placement (consistent-hash), the scope still narrows a viewer to one channel slice;
//! - the gateway is bounded by the 1.7 harness shim ([`crate::te21_pin`]) — the home-node is a
//!   *gateway-process escalation* inside that boundary, not a new cross-language surface.
//!
//! So the home-node is a **construction-time delivery-topology swap behind the unchanged firehose
//! protocol**: the channel's subscribers are partitioned across home-node shards by consistent-hash, a
//! post fans out to the owning shard(s) instead of one flat subject, and CHAT-D1 is re-run across the
//! escalation (the resume-cursor backbone is invariant under the shard split).
//!
//! ## The BEAM/Phoenix-gateway SIBLING floor (CHAT-P9 / contract 1.7) — separately triggered
//! The home-node is the *mega-channel delivery* floor. It is DISTINCT from the **connection-tier
//! language** floor (`05-hard-problems.md` §1 floor (a)): the gateway is **Rust by default**; the
//! BEAM/Phoenix-gateway hatch is written-but-CLOSED, bounded by the 1.7 harness shim, opened ONLY if
//! CHAT-D3/D4 proved Rust presence-at-scale / tail-latency intractable (the TE-21 build-gate). That is
//! a *sibling, separately-triggered* floor: CHAT-D3/D4 are GREEN in Rust (the [`crate::surge`] family +
//! the deploy-herd drill), so the language floor's trigger has NOT fired either — Rust is retained.
//! Recorded below as [`BEAM_GATEWAY_SIBLING_FLOOR`] so both floors are explicit, not implied.
//!
//! ## The gap-report invariant (this prompt's gate)
//! Each follow-on below is a [`FloorFollowOn`] (the FROZEN chat floor vocabulary, reused from
//! [`myelin_chat`] — EI-01 §7, one shape, no second copy) with a NON-EMPTY trigger / follow-on /
//! preserved-contract and a dated [`TriggerStatus`]. [`home_node_floor_gap_report`] asserts **0
//! invisible gaps** AND the **honest-floor invariant**: while a trigger is `NotFired` the promotion
//! MUST stay a named floor (`built == false`). The dated gap-report row IS this prompt's "If NOT
//! triggered" branch (the prompt's GATE) — an honest named floor (EI-04 §4), machine-checked.

use myelin_chat::{FloorFollowOn, TriggerStatus};

/// **The subject-fan-out budget — the measured trigger boundary for the home-node escalation.** A
/// channel whose MEASURED subscriber count crosses this budget is the mega-channel the home-node
/// escalation exists for (R-5). The budget is the per-channel subscriber count above which a flat
/// firehose subject fan-out is no longer the right delivery topology (the point the Phoenix/Discord
/// guild model wins). It is a MEASURED-not-predicted boundary: the value here is the *floor's named
/// trigger threshold* (the design boundary, `05-hard-problems.md` §1/§8), and the gate fires on a
/// telemetry reading crossing it — never on a guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubjectFanOutBudget {
    /// The per-channel subscriber count above which the flat subject fan-out is escalated to the
    /// channel-sharded home-node (R-5). A channel with `subscriber_count > max_subscribers_per_subject`
    /// is over budget.
    pub max_subscribers_per_subject: u64,
}

impl SubjectFanOutBudget {
    /// **The named trigger threshold (the design boundary, `05-hard-problems.md` §1 floor (b)).** The
    /// flat firehose subject fan-out is the v1 model up to this per-channel subscriber count; above
    /// it, the channel-sharded home-node is the measured escalation. This is the boundary the floor's
    /// trigger is *named against* — a real telemetry reading crossing it fires the promotion. The
    /// value is deliberately the architecture's stated guild-scale boundary (a Discord "guild" /
    /// mega-channel), not a number tuned to make a drill pass (EI-01 §3).
    pub const NAMED: SubjectFanOutBudget = SubjectFanOutBudget {
        // A mega-channel scale: the Discord guild model is the cited prior art for fan-out beyond a
        // flat subject (05-hard-problems §1). The boundary is the named design floor, refined when a
        // real cell measures the per-subject fan-out tail latency crossing budget.
        max_subscribers_per_subject: 100_000,
    };

    /// **The trigger predicate: is this measured subscriber count OVER the subject-fan-out budget?**
    /// Returns `true` iff `subscriber_count` strictly exceeds [`Self::max_subscribers_per_subject`] —
    /// the measured signal that fires the home-node promotion (R-5). A reading at-or-below the budget
    /// keeps the flat subject fan-out (the v1 seam) — the trigger has NOT fired.
    pub fn exceeded_by(&self, subscriber_count: u64) -> bool {
        subscriber_count > self.max_subscribers_per_subject
    }
}

/// **A measured per-channel fan-out reading — the telemetry the trigger evaluates against.** This is
/// the *shape* a cell's connection-tier telemetry would emit (the live-subscriber count on a channel's
/// firehose subject). The promotion fires when a recorded reading's [`Self::subscriber_count`] crosses
/// the [`SubjectFanOutBudget`]. No such crossing reading exists at this prompt's execution (the surge
/// family measured shed budgets, not subscriber fan-out — see the module note), so the floor stays
/// named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasuredFanOut {
    /// The measured live-subscriber count on a single channel's firehose subject.
    pub subscriber_count: u64,
}

impl MeasuredFanOut {
    /// Does this measured reading fire the home-node trigger against the given budget? Returns `true`
    /// iff the subscriber count exceeds the budget — the measured signal the promotion rides.
    pub fn fires(&self, budget: &SubjectFanOutBudget) -> bool {
        budget.exceeded_by(self.subscriber_count)
    }
}

/// **The mega-channel channel-sharded home-node — the ONE measured-trigger-gated chat-M5 delivery
/// floor this prompt (CHAT-P29 / P-503) names.** Stays a named floor until a channel's measured
/// subscriber count crosses the subject-fan-out budget; the gap-report enforces no premature
/// promotion.
pub const HOME_NODE_FLOOR: FloorFollowOn = FloorFollowOn {
    id: "channel-sharded-home-node",
    what: "a channel-sharded home-node for mega-channel live delivery (the Phoenix/Discord guild \
           model in Rust + consistent-hash): a mega-channel's subscribers are partitioned across \
           home-node shards by consistent-hash, and a post fans out to the owning shard(s) instead of \
           one flat firehose subject — the measured escalation (R-5) of the v1 firehose subject \
           fan-out with per-view scope bounding",
    built_seam: "the firehose resume-cursor protocol (contract 3.5, myelin_events::Firehose: \
                 subscribe/resume/scope) + the per-view bounded channel:<id> scope (the *-rejecting \
                 chat_channel_scope chokepoint) + the 1.7 cross-language harness shim (te21_pin): the \
                 home-node is a GATEWAY-PROCESS delivery-topology swap behind these UNCHANGED \
                 surfaces, never a protocol redesign — the resume-cursor seq CHAT-D1 pins is carried \
                 the same way across the shard split",
    preserved_contract: "3.5 — the firehose resume-cursor protocol (subscribe/resume/scope) unchanged \
                         across the escalation: 0 lost / 0 dup on resume holds whether a channel fans \
                         out over one subject or is sharded; 1.7 — the cross-language harness shim \
                         bounds the gateway-process escalation. No wire-protocol divergence: only the \
                         delivery topology behind the protocol changes",
    trigger: "a channel's MEASURED subscriber count exceeding the subject-fan-out budget (R-5; the \
              measure-before-shard mandate, ADR-10) — SubjectFanOutBudget::exceeded_by over a \
              MeasuredFanOut::subscriber_count reading; not before measured",
    status: TriggerStatus::NotFired {
        as_of: "2026-06-25: NO measured per-channel subscriber-count reading crossing the \
                subject-fan-out budget exists. The chat M5 surge family (CHAT-P26 / P-500) measured \
                the gateway SHED budgets (ConnectionTier/AgentMention lanes — the delivery-side \
                fairness signal under a 30x agent-message/connection RATE storm), NOT a single \
                mega-channel whose subscriber fan-out crosses the subject budget. The v1 firehose \
                subject fan-out with per-view scope bounding is correct at the measured load; the \
                home-node promotion remains a named floor (measured-not-predicted: the \
                subscriber-count-vs-budget measurement is owed).",
    },
    follow_on: "build the channel-sharded home-node in myelin-chat-gateway (consistent-hash a \
                mega-channel's subscribers across home-node shards; fan a post to the owning shard(s); \
                the Phoenix/Discord guild model in Rust) behind the unchanged firehose protocol; re-run \
                CHAT-D1 across the escalation",
    promotion_gate: "CHAT-D1 (resume recovers the gap, 0 lost / 0 dup) re-run GREEN across the \
                     home-node escalation — the lost/dup signal = 0 post-escalation (the drill was \
                     written to survive the swap) — SCHED",
    built: false,
};

/// **The BEAM/Phoenix-gateway SIBLING floor (CHAT-P9 / contract 1.7) — the connection-tier LANGUAGE
/// floor, separately triggered.** Distinct from the mega-channel *delivery* home-node above: the
/// gateway is Rust by default; the BEAM/Phoenix hatch is opened ONLY if CHAT-D3/D4 prove Rust
/// presence-at-scale / tail-latency intractable (the TE-21 build-gate). CHAT-D3/D4 are GREEN in Rust
/// (the surge family + deploy-herd drill), so this trigger has NOT fired either — Rust is retained.
/// Named here so both floors are explicit (the prompt's DoD: state the BEAM-gateway sibling floor).
pub const BEAM_GATEWAY_SIBLING_FLOOR: FloorFollowOn = FloorFollowOn {
    id: "beam-phoenix-gateway",
    what: "a BEAM/Phoenix-Channels connection-tier gateway (the Discord Elixir-gateway split): \
           per-connection preemptive scheduling + Phoenix.PubSub/Presence — the written-but-closed \
           hatch for the connection-tier language (TE-21), a gateway-process swap NOT a platform \
           rewrite",
    built_seam: "the 1.7 cross-language harness shim (te21_pin, currently a NO-OP in the all-Rust \
                 default) + the firehose resume-cursor protocol (3.5) the BEAM gateway would speak on \
                 the wire: the gateway is stateless (sockets + presence + resume cursors only), so the \
                 swap is bounded by the frozen shim, never a rewrite of the correctness-critical Rust \
                 services it calls by RPC",
    preserved_contract: "1.7 — the cross-language harness shim (three-surface, liveness!=readiness, \
                         resilient-client + Retry-After, no fire-and-forget emit, the survival \
                         signals, the protected-human-lane shed order, forward-only migrations); 3.5 \
                         — the firehose resume-cursor protocol the BEAM gateway rides unchanged",
    trigger: "CHAT-D3/D4 proving the Rust connection tier intractable at presence-at-scale / \
              tail-latency (the TE-21 build-gate) — a SEPARATE trigger from the mega-channel \
              subscriber-count budget; not before that intractability is measured",
    status: TriggerStatus::NotFired {
        as_of: "2026-06-25: CHAT-D3/D4 are GREEN in Rust (the surge family run_chat_surge holds the \
                protected human lane under the 30x storm; the deploy-herd reconnect drill holds). The \
                Rust connection tier is NOT intractable, so the BEAM/Phoenix hatch stays \
                written-but-CLOSED — Rust is the connection-tier language (TE-21 default). Floor \
                remains named (the hatch is the honesty hedge, not a planned build).",
    },
    follow_on: "swap the gateway PROCESS to BEAM/Phoenix-Channels behind the 1.7 harness shim + the \
                3.5 firehose protocol (Phoenix.PubSub/Presence for the live tier; the Rust services \
                called by RPC for everything correctness-critical) — a gateway-process swap, not a \
                platform rewrite",
    promotion_gate: "the 1.7 harness shim re-proven against the BEAM gateway + CHAT-D1/D3/D4 GREEN on \
                     the swapped process (0 lost/dup; the protected human lane holds) — only if the \
                     Rust-intractability trigger fires",
    built: false,
};

/// Every floor follow-on this gap-report accounts for: the mega-channel home-node (this prompt's
/// named floor) + the BEAM-gateway sibling floor (the prompt's DoD requires it be stated). A `slice`
/// so the manifest extends uniformly if a future gateway-M5 promotion is named.
pub const GATEWAY_MEASURED_TRIGGER_FLOORS: &[FloorFollowOn] =
    &[HOME_NODE_FLOOR, BEAM_GATEWAY_SIBLING_FLOOR];

/// **The gap-report gate (CHAT-P29 / P-503 — the "If NOT triggered" branch's dated row).** Asserts,
/// over every named gateway floor: (1) it is FULLY recorded (0 invisible gaps), and (2) the
/// honest-floor invariant holds (a promotion is built ONLY once its trigger has fired). Returns
/// `Ok(())` when the gap-report is honest; an `Err` names the offending floor. This is the
/// machine-checked equivalent of the dated gap-report row the prompt requires for the
/// not-triggered branch. Reuses the FROZEN chat floor vocabulary (EI-01 §7 — one shape, no second
/// copy), evaluating it in-crate over the gateway's own floors.
pub fn home_node_floor_gap_report() -> Result<(), String> {
    gap_report_over(GATEWAY_MEASURED_TRIGGER_FLOORS)
}

/// The gap-report verdict over an ARBITRARY slice of floors (so the invariant can be checked against a
/// deliberately-broken row in a test — the verdict's two failure modes are then load-bearing, not
/// vacuously `Ok`). An invisible gap OR a premature promotion is an `Err` naming the floor. The
/// predicates are the FROZEN chat-floor ones ([`FloorFollowOn::is_fully_recorded`] /
/// [`FloorFollowOn::honours_no_premature_promotion`]) — reused, never re-authored.
fn gap_report_over(floors: &[FloorFollowOn]) -> Result<(), String> {
    for f in floors {
        if !f.is_fully_recorded() {
            return Err(format!(
                "gateway floor follow-on `{}` is an invisible gap (a must-be-non-empty field is empty)",
                f.id
            ));
        }
        if !f.honours_no_premature_promotion() {
            return Err(format!(
                "gateway floor follow-on `{}` is a PREMATURE promotion — built with an unfired \
                 trigger (EI-04 §5 forbids adding it before the measurement)",
                f.id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The subject-fan-out budget trigger predicate is real + load-bearing.** A subscriber count at
    /// or below the budget does NOT fire (the v1 flat subject fan-out is retained); a count strictly
    /// above DOES fire (the mega-channel the home-node exists for). The boundary is exclusive (`>`).
    #[test]
    fn the_subject_fan_out_budget_predicate_is_load_bearing() {
        let budget = SubjectFanOutBudget::NAMED;
        let cap = budget.max_subscribers_per_subject;
        // at/below budget → NOT fired (flat subject fan-out is correct).
        assert!(
            !budget.exceeded_by(0),
            "an empty channel never fires the trigger"
        );
        assert!(
            !budget.exceeded_by(cap),
            "exactly-at-budget does NOT fire (`>`, not `>=`)"
        );
        assert!(
            !MeasuredFanOut {
                subscriber_count: cap
            }
            .fires(&budget),
            "a measured reading at budget does not fire the home-node promotion"
        );
        // above budget → FIRED (the mega-channel escalation).
        assert!(
            budget.exceeded_by(cap + 1),
            "one over budget fires the trigger"
        );
        assert!(
            MeasuredFanOut {
                subscriber_count: cap + 1
            }
            .fires(&budget),
            "a measured reading over budget fires the home-node promotion (R-5)"
        );
    }

    /// **The home-node floor is recorded with no invisible gap, and its trigger has NOT fired at this
    /// prompt's execution** (no measured subscriber-count crossing exists — the surge family measured
    /// shed budgets, not subscriber fan-out). So it MUST stay a named floor (`built == false`).
    #[test]
    fn the_home_node_floor_is_an_honest_named_floor() {
        let floor = std::hint::black_box(HOME_NODE_FLOOR);
        assert!(
            floor.is_fully_recorded(),
            "the home-node floor must be fully recorded (no must-be-non-empty field empty)"
        );
        assert!(
            !floor.status.has_fired(),
            "the measured subscriber-count-vs-budget trigger has NOT fired at this prompt's execution"
        );
        assert!(
            !floor.built,
            "the channel-sharded home-node is a NAMED FLOOR — not built speculatively (ADR-10)"
        );
        assert!(
            floor.honours_no_premature_promotion(),
            "the honest-floor invariant holds: ¬fired ⇒ ¬built"
        );
    }

    /// **The home-node trigger names the MEASURED signal (subscriber count vs the subject-fan-out
    /// budget) + the measure-before-shard mandate; the dated status is non-empty.** And the preserved
    /// contract names the firehose resume-cursor protocol (3.5) the escalation rides unchanged + the
    /// 1.7 harness shim.
    #[test]
    fn the_home_node_trigger_names_the_measured_signal_and_is_dated() {
        let floor = std::hint::black_box(HOME_NODE_FLOOR);
        assert!(floor.trigger.contains("subscriber count"));
        assert!(floor.trigger.contains("subject-fan-out budget"));
        assert!(floor.trigger.contains("ADR-10"));
        assert!(!floor.status.dated().is_empty());
        // the escalation rides the UNCHANGED firehose protocol (3.5) + is bounded by the 1.7 shim.
        assert!(floor.preserved_contract.contains("3.5"));
        assert!(floor.preserved_contract.contains("1.7"));
        // and the promotion gate is CHAT-D1 re-run across the escalation (the prompt's gate).
        assert!(floor.promotion_gate.contains("CHAT-D1"));
    }

    /// **The BEAM/Phoenix-gateway SIBLING floor is named, separately triggered, and unfired** (CHAT-D3/D4
    /// are green in Rust → Rust is retained). The prompt's DoD requires this be stated.
    #[test]
    fn the_beam_gateway_sibling_floor_is_named_and_unfired() {
        let floor = std::hint::black_box(BEAM_GATEWAY_SIBLING_FLOOR);
        assert!(
            floor.is_fully_recorded(),
            "the BEAM sibling floor is fully recorded"
        );
        assert!(
            !floor.status.has_fired(),
            "CHAT-D3/D4 are GREEN in Rust — the Rust connection tier is NOT intractable, the hatch \
             stays closed"
        );
        assert!(
            !floor.built,
            "the BEAM/Phoenix gateway is a NAMED FLOOR — not built"
        );
        // it is the LANGUAGE floor (TE-21 build-gate), distinct from the mega-channel delivery floor.
        assert!(floor.trigger.contains("CHAT-D3/D4"));
        assert!(floor.trigger.contains("SEPARATE trigger"));
        assert!(floor.preserved_contract.contains("1.7"));
    }

    /// **The gateway gap-report is GREEN over the real manifest** (both floors fully recorded + no
    /// premature promotion), and its verdict is load-bearing: `Err` (naming the floor) over an
    /// invisible-gap row and over a premature-promotion row.
    #[test]
    fn the_gap_report_is_honest_and_its_verdict_is_load_bearing() {
        // Ok over the real (honest) manifest — both the home-node + BEAM sibling floors.
        home_node_floor_gap_report().expect("the gateway gap-report is honest — 0 invisible gaps");
        let ids: Vec<&str> = GATEWAY_MEASURED_TRIGGER_FLOORS
            .iter()
            .map(|f| f.id)
            .collect();
        assert_eq!(
            ids,
            vec!["channel-sharded-home-node", "beam-phoenix-gateway"]
        );

        // Err (invisible gap) when a row has an empty must-be-non-empty field — naming the floor.
        let invisible_gap = FloorFollowOn {
            id: "gap-row",
            what: "x",
            built_seam: "x",
            preserved_contract: "x",
            trigger: "", // the gap
            status: TriggerStatus::NotFired { as_of: "x" },
            follow_on: "x",
            promotion_gate: "x",
            built: false,
        };
        let err = gap_report_over(&[invisible_gap]).expect_err("an invisible gap is an Err");
        assert!(err.contains("gap-row") && err.contains("invisible gap"));

        // Err (premature promotion) when a row is built with an unfired trigger — naming the floor.
        let premature = FloorFollowOn {
            id: "premature-row",
            what: "x",
            built_seam: "x",
            preserved_contract: "x",
            trigger: "x",
            status: TriggerStatus::NotFired { as_of: "x" },
            follow_on: "x",
            promotion_gate: "x",
            built: true, // premature: built with an unfired trigger
        };
        let err = gap_report_over(&[premature]).expect_err("a premature promotion is an Err");
        assert!(err.contains("premature-row") && err.contains("PREMATURE"));
    }
}
