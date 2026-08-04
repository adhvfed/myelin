use crate::scylla_followon::{FloorFollowOn, TriggerStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchoredCommentPresenceDemand {
    pub live_multiparty_sessions_observed: u64,
    pub over_window: &'static str,
}

impl AnchoredCommentPresenceDemand {
    pub const OBSERVED_NONE: AnchoredCommentPresenceDemand = AnchoredCommentPresenceDemand {
        live_multiparty_sessions_observed: 0,
        over_window:
            "2026-06-25: the whole M5 band to date - 0 anchored-comment presence sessions \
                      observed (no KB/Issues comment surface emits or consumes a chat.presence.* \
                      frame; the comment stores are CAS-guarded OLTP threads, not a firehose live \
                      tier)",
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresenceDemandBudget {
    pub min_observed_sessions: u64,
}

impl PresenceDemandBudget {
    pub const OQ_L: PresenceDemandBudget = PresenceDemandBudget {
        min_observed_sessions: 1,
    };

    pub fn exceeded_by(&self, demand: &AnchoredCommentPresenceDemand) -> bool {
        demand.live_multiparty_sessions_observed >= self.min_observed_sessions
    }
}

pub const COMMENT_CONSOLIDATION_FLOOR: FloorFollowOn = FloorFollowOn {
    id: "comment-threading-consolidation",
    what: "document-anchored comment threads (Knowledge/Issues) promoted onto the Chat threading \
           primitive + the firehose resume-cursor transport (OQ-L / M5-C-X2 / R-C8) - a \
           store/transport SWAP over the shared #thread-/#comment- #sub + content + refs scheme, \
           NOT a CRDT and NOT a rewrite",
    built_seam: "the shared scheme the two stores ALREADY agree on: the #thread-/#comment- #sub \
                 grammar (subs + myelin_knowledge::comments through the ONE myelin_refs grammar, \
                 5.7), the myelin_content AST (the body, 13.1), the refs.edge.created events (5.4), \
                 and the firehose resume-cursor transport (myelin_events::firehose, 3.5) the \
                 anchored comments promote ONTO - so the consolidation is a backing swap, not a \
                 redesign",
    preserved_contract: "5.7 - the shared #sub scheme (the #thread-/#comment- subjects unchanged \
                         across the swap); 13.1 - the shared content AST (the comment body \
                         unchanged); 5.4 - the shared refs edges (unchanged); 3.5 - the firehose \
                         resume-cursor protocol the comments ride; the per-message CAS (CHAT-P12) is \
                         unchanged. Only the STORE/TRANSPORT behind the shared scheme changes, never \
                         the data model",
    trigger: "document-anchored comments are MEASURED needing real-time multi-party presence \
              (PresenceDemandBudget::OQ_L crossed by an AnchoredCommentPresenceDemand reading - a \
              live, concurrent, presence-bearing session on an anchored comment gutter); the \
              promote-on-demand mandate (OQ-L / VISION §3) - not before the demand is real",
    status: TriggerStatus::NotFired {
        as_of: "2026-06-25: 0 anchored-comment real-time-presence sessions observed \
                (AnchoredCommentPresenceDemand::OBSERVED_NONE). The KB/Issues comment surfaces are \
                CAS-guarded OLTP comment threads (myelin_knowledge::comments - create/resolve/reopen \
                over a stable block anchor) with NO presence surface; no KB/Issues comment emits or \
                consumes a chat.presence.* frame, and no demand for real-time multi-party presence \
                on an anchored comment has been measured. The v1 two-stores-one-scheme floor is \
                correct; the consolidation remains named (promote-on-demand: the demand is owed).",
    },
    follow_on: "promote the anchored-comment threads onto the Chat threading primitive (the \
                conversation/thread model + the per-message CAS) and the firehose resume-cursor live \
                tier (presence + partials), keyed by the SAME #thread-/#comment- subjects; the KB/ \
                Issues comment OPS become a write to the Chat threading store, the gutter subscribes \
                to the firehose scope - the #sub + content + refs data model is untouched",
    promotion_gate: "the relevant content + refs + #sub drills re-run GREEN across the \
                     store/transport swap (0 round-trip / edge / #sub regressions post-swap - each \
                     drill written to survive the swap) - CI; the per-message CAS (CHAT-P12) holds \
                     across the swap",
    built: false,
};

pub const COMMENT_CONSOLIDATION_FLOORS: &[FloorFollowOn] = &[COMMENT_CONSOLIDATION_FLOOR];

pub fn comment_consolidation_gap_report() -> Result<(), String> {
    consolidation_gap_report_over(
        COMMENT_CONSOLIDATION_FLOORS,
        &AnchoredCommentPresenceDemand::OBSERVED_NONE,
        &PresenceDemandBudget::OQ_L,
    )
}

fn consolidation_gap_report_over(
    floors: &[FloorFollowOn],
    demand: &AnchoredCommentPresenceDemand,
    budget: &PresenceDemandBudget,
) -> Result<(), String> {
    let predicate_fired = budget.exceeded_by(demand);
    for f in floors {
        if !f.is_fully_recorded() {
            return Err(format!(
                "comment-consolidation floor `{}` is an invisible gap (a must-be-non-empty field \
                 is empty)",
                f.id
            ));
        }
        if !f.honours_no_premature_promotion() {
            return Err(format!(
                "comment-consolidation floor `{}` is a PREMATURE promotion - built with an unfired \
                 trigger (OQ-L / EI-04 §5 forbid promoting before the demand is real)",
                f.id
            ));
        }
        if f.status.has_fired() != predicate_fired {
            return Err(format!(
                "comment-consolidation floor `{}` status/predicate INCONSISTENCY: recorded \
                 has_fired={} but the measured trigger predicate (PresenceDemandBudget::OQ_L over \
                 the observed demand) = {}",
                f.id,
                f.status.has_fired(),
                predicate_fired
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_consolidation_floor_is_recorded_with_no_invisible_gap() {
        let ids: Vec<&str> = COMMENT_CONSOLIDATION_FLOORS.iter().map(|f| f.id).collect();
        assert_eq!(ids, vec!["comment-threading-consolidation"]);
        let floor = std::hint::black_box(COMMENT_CONSOLIDATION_FLOOR);
        assert!(
            floor.is_fully_recorded(),
            "the comment-consolidation floor must be fully recorded (no must-be-non-empty field \
             empty)"
        );
    }

    #[test]
    fn no_premature_promotion_trigger_unfired_stays_a_floor() {
        let floor = std::hint::black_box(COMMENT_CONSOLIDATION_FLOOR);
        assert!(
            !floor.status.has_fired(),
            "the real-time-presence-on-anchored-comments trigger has NOT fired at this prompt's \
             execution"
        );
        assert!(
            !floor.built,
            "the comment-threading consolidation is a NAMED FLOOR - not built speculatively"
        );
        assert!(
            floor.honours_no_premature_promotion(),
            "the honest-floor invariant holds: ¬fired ⇒ ¬built"
        );
        comment_consolidation_gap_report().expect("the gap-report is honest - 0 invisible gaps");
    }

    #[test]
    fn the_floor_names_the_swap_not_a_rewrite_and_the_preserved_scheme() {
        let floor = std::hint::black_box(COMMENT_CONSOLIDATION_FLOOR);
        assert!(floor.what.contains("SWAP"));
        assert!(floor.what.contains("NOT a CRDT"));
        assert!(floor.what.contains("NOT a rewrite"));
        assert!(floor.preserved_contract.contains("5.7"));
        assert!(floor.preserved_contract.contains("13.1"));
        assert!(floor.preserved_contract.contains("5.4"));
        assert!(floor.preserved_contract.contains("3.5"));
        assert!(floor.preserved_contract.contains("CHAT-P12"));
        assert!(floor.trigger.contains("real-time multi-party presence"));
        assert!(floor.trigger.contains("PresenceDemandBudget::OQ_L"));
        assert!(!floor.status.dated().is_empty());
    }

    #[test]
    fn the_trigger_predicate_is_real_and_evaluable() {
        let budget = PresenceDemandBudget::OQ_L;
        let observed = AnchoredCommentPresenceDemand::OBSERVED_NONE;
        assert_eq!(observed.live_multiparty_sessions_observed, 0);
        assert!(
            !budget.exceeded_by(&observed),
            "0 observed presence sessions must NOT cross the OQ-L demand budget"
        );
        let demand_real = AnchoredCommentPresenceDemand {
            live_multiparty_sessions_observed: 1,
            over_window: "synthetic: a real anchored-comment presence session was observed",
        };
        assert!(
            budget.exceeded_by(&demand_real),
            "a real anchored-comment presence session MUST cross the OQ-L demand budget"
        );
        assert!(!observed.over_window.is_empty());
    }

    #[test]
    fn gap_report_couples_status_to_the_measured_predicate() {
        assert!(comment_consolidation_gap_report().is_ok());
        assert!(consolidation_gap_report_over(
            COMMENT_CONSOLIDATION_FLOORS,
            &AnchoredCommentPresenceDemand::OBSERVED_NONE,
            &PresenceDemandBudget::OQ_L,
        )
        .is_ok());
        let crossing = AnchoredCommentPresenceDemand {
            live_multiparty_sessions_observed: 3,
            over_window: "synthetic: demand crossed",
        };
        let err = consolidation_gap_report_over(
            COMMENT_CONSOLIDATION_FLOORS,
            &crossing,
            &PresenceDemandBudget::OQ_L,
        )
        .expect_err("a NotFired status over a crossed predicate is an inconsistency");
        assert!(err.contains("comment-threading-consolidation"));
        assert!(err.contains("INCONSISTENCY"));
    }

    #[test]
    fn gap_report_catches_invisible_gap_and_premature_promotion() {
        let invisible_gap = FloorFollowOn {
            id: "gap-row",
            trigger: "",
            ..COMMENT_CONSOLIDATION_FLOOR
        };
        let err = consolidation_gap_report_over(
            &[invisible_gap],
            &AnchoredCommentPresenceDemand::OBSERVED_NONE,
            &PresenceDemandBudget::OQ_L,
        )
        .expect_err("an invisible gap is an Err");
        assert!(err.contains("gap-row") && err.contains("invisible gap"));

        let premature = FloorFollowOn {
            id: "premature-row",
            built: true,
            status: TriggerStatus::NotFired { as_of: "d" },
            ..COMMENT_CONSOLIDATION_FLOOR
        };
        let err = consolidation_gap_report_over(
            &[premature],
            &AnchoredCommentPresenceDemand::OBSERVED_NONE,
            &PresenceDemandBudget::OQ_L,
        )
        .expect_err("a premature promotion is an Err");
        assert!(err.contains("premature-row") && err.contains("PREMATURE"));
    }
}
