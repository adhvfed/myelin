use myelin_chat::{FloorFollowOn, TriggerStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubjectFanOutBudget {
    pub max_subscribers_per_subject: u64,
}

impl SubjectFanOutBudget {
    pub const NAMED: SubjectFanOutBudget = SubjectFanOutBudget {
        max_subscribers_per_subject: 100_000,
    };

    pub fn exceeded_by(&self, subscriber_count: u64) -> bool {
        subscriber_count > self.max_subscribers_per_subject
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasuredFanOut {
    pub subscriber_count: u64,
}

impl MeasuredFanOut {
    pub fn fires(&self, budget: &SubjectFanOutBudget) -> bool {
        budget.exceeded_by(self.subscriber_count)
    }
}

pub const HOME_NODE_FLOOR: FloorFollowOn = FloorFollowOn {
    id: "channel-sharded-home-node",
    what: "a channel-sharded home-node for mega-channel live delivery (the Phoenix/Discord guild \
           model in Rust + consistent-hash): a mega-channel's subscribers are partitioned across \
           home-node shards by consistent-hash, and a post fans out to the owning shard(s) instead of \
           one flat firehose subject - the measured escalation (R-5) of the v1 firehose subject \
           fan-out with per-view scope bounding",
    built_seam: "the firehose resume-cursor protocol (contract 3.5, myelin_events::Firehose: \
                 subscribe/resume/scope) + the per-view bounded channel:<id> scope (the *-rejecting \
                 chat_channel_scope chokepoint) + the 1.7 cross-language harness shim (te21_pin): the \
                 home-node is a GATEWAY-PROCESS delivery-topology swap behind these UNCHANGED \
                 surfaces, never a protocol redesign - the resume-cursor seq CHAT-D1 pins is carried \
                 the same way across the shard split",
    preserved_contract: "3.5 - the firehose resume-cursor protocol (subscribe/resume/scope) unchanged \
                         across the escalation: 0 lost / 0 dup on resume holds whether a channel fans \
                         out over one subject or is sharded; 1.7 - the cross-language harness shim \
                         bounds the gateway-process escalation. No wire-protocol divergence: only the \
                         delivery topology behind the protocol changes",
    trigger: "a channel's MEASURED subscriber count exceeding the subject-fan-out budget (R-5; the \
              measure-before-shard mandate, ADR-10) - SubjectFanOutBudget::exceeded_by over a \
              MeasuredFanOut::subscriber_count reading; not before measured",
    status: TriggerStatus::NotFired {
        as_of: "2026-06-25: NO measured per-channel subscriber-count reading crossing the \
                subject-fan-out budget exists. The chat M5 surge family (CHAT-P26 / P-500) measured \
                the gateway SHED budgets (ConnectionTier/AgentMention lanes - the delivery-side \
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
                     home-node escalation - the lost/dup signal = 0 post-escalation (the drill was \
                     written to survive the swap) - SCHED",
    built: false,
};

pub const BEAM_GATEWAY_SIBLING_FLOOR: FloorFollowOn = FloorFollowOn {
    id: "beam-phoenix-gateway",
    what: "a BEAM/Phoenix-Channels connection-tier gateway (the Discord Elixir-gateway split): \
           per-connection preemptive scheduling + Phoenix.PubSub/Presence - the written-but-closed \
           hatch for the connection-tier language (TE-21), a gateway-process swap NOT a platform \
           rewrite",
    built_seam: "the 1.7 cross-language harness shim (te21_pin, currently a NO-OP in the all-Rust \
                 default) + the firehose resume-cursor protocol (3.5) the BEAM gateway would speak on \
                 the wire: the gateway is stateless (sockets + presence + resume cursors only), so the \
                 swap is bounded by the frozen shim, never a rewrite of the correctness-critical Rust \
                 services it calls by RPC",
    preserved_contract: "1.7 - the cross-language harness shim (three-surface, liveness!=readiness, \
                         resilient-client + Retry-After, no fire-and-forget emit, the survival \
                         signals, the protected-human-lane shed order, forward-only migrations); 3.5 \
                         - the firehose resume-cursor protocol the BEAM gateway rides unchanged",
    trigger: "CHAT-D3/D4 proving the Rust connection tier intractable at presence-at-scale / \
              tail-latency (the TE-21 build-gate) - a SEPARATE trigger from the mega-channel \
              subscriber-count budget; not before that intractability is measured",
    status: TriggerStatus::NotFired {
        as_of: "2026-06-25: CHAT-D3/D4 are GREEN in Rust (the surge family run_chat_surge holds the \
                protected human lane under the 30x storm; the deploy-herd reconnect drill holds). The \
                Rust connection tier is NOT intractable, so the BEAM/Phoenix hatch stays \
                written-but-CLOSED - Rust is the connection-tier language (TE-21 default). Floor \
                remains named (the hatch is the honesty hedge, not a planned build).",
    },
    follow_on: "swap the gateway PROCESS to BEAM/Phoenix-Channels behind the 1.7 harness shim + the \
                3.5 firehose protocol (Phoenix.PubSub/Presence for the live tier; the Rust services \
                called by RPC for everything correctness-critical) - a gateway-process swap, not a \
                platform rewrite",
    promotion_gate: "the 1.7 harness shim re-proven against the BEAM gateway + CHAT-D1/D3/D4 GREEN on \
                     the swapped process (0 lost/dup; the protected human lane holds) - only if the \
                     Rust-intractability trigger fires",
    built: false,
};

pub const GATEWAY_MEASURED_TRIGGER_FLOORS: &[FloorFollowOn] =
    &[HOME_NODE_FLOOR, BEAM_GATEWAY_SIBLING_FLOOR];

pub fn home_node_floor_gap_report() -> Result<(), String> {
    gap_report_over(GATEWAY_MEASURED_TRIGGER_FLOORS)
}

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
                "gateway floor follow-on `{}` is a PREMATURE promotion - built with an unfired \
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

    #[test]
    fn the_subject_fan_out_budget_predicate_is_load_bearing() {
        let budget = SubjectFanOutBudget::NAMED;
        let cap = budget.max_subscribers_per_subject;
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
            "the channel-sharded home-node is a NAMED FLOOR - not built speculatively (ADR-10)"
        );
        assert!(
            floor.honours_no_premature_promotion(),
            "the honest-floor invariant holds: ¬fired ⇒ ¬built"
        );
    }

    #[test]
    fn the_home_node_trigger_names_the_measured_signal_and_is_dated() {
        let floor = std::hint::black_box(HOME_NODE_FLOOR);
        assert!(floor.trigger.contains("subscriber count"));
        assert!(floor.trigger.contains("subject-fan-out budget"));
        assert!(floor.trigger.contains("ADR-10"));
        assert!(!floor.status.dated().is_empty());
        assert!(floor.preserved_contract.contains("3.5"));
        assert!(floor.preserved_contract.contains("1.7"));
        assert!(floor.promotion_gate.contains("CHAT-D1"));
    }

    #[test]
    fn the_beam_gateway_sibling_floor_is_named_and_unfired() {
        let floor = std::hint::black_box(BEAM_GATEWAY_SIBLING_FLOOR);
        assert!(
            floor.is_fully_recorded(),
            "the BEAM sibling floor is fully recorded"
        );
        assert!(
            !floor.status.has_fired(),
            "CHAT-D3/D4 are GREEN in Rust - the Rust connection tier is NOT intractable, the hatch \
             stays closed"
        );
        assert!(
            !floor.built,
            "the BEAM/Phoenix gateway is a NAMED FLOOR - not built"
        );
        assert!(floor.trigger.contains("CHAT-D3/D4"));
        assert!(floor.trigger.contains("SEPARATE trigger"));
        assert!(floor.preserved_contract.contains("1.7"));
    }

    #[test]
    fn the_gap_report_is_honest_and_its_verdict_is_load_bearing() {
        home_node_floor_gap_report().expect("the gateway gap-report is honest - 0 invisible gaps");
        let ids: Vec<&str> = GATEWAY_MEASURED_TRIGGER_FLOORS
            .iter()
            .map(|f| f.id)
            .collect();
        assert_eq!(
            ids,
            vec!["channel-sharded-home-node", "beam-phoenix-gateway"]
        );

        let invisible_gap = FloorFollowOn {
            id: "gap-row",
            what: "x",
            built_seam: "x",
            preserved_contract: "x",
            trigger: "",
            status: TriggerStatus::NotFired { as_of: "x" },
            follow_on: "x",
            promotion_gate: "x",
            built: false,
        };
        let err = gap_report_over(&[invisible_gap]).expect_err("an invisible gap is an Err");
        assert!(err.contains("gap-row") && err.contains("invisible gap"));

        let premature = FloorFollowOn {
            id: "premature-row",
            what: "x",
            built_seam: "x",
            preserved_contract: "x",
            trigger: "x",
            status: TriggerStatus::NotFired { as_of: "x" },
            follow_on: "x",
            promotion_gate: "x",
            built: true,
        };
        let err = gap_report_over(&[premature]).expect_err("a premature promotion is an Err");
        assert!(err.contains("premature-row") && err.contains("PREMATURE"));
    }
}
