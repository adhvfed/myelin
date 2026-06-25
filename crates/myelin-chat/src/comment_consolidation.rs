//! # `comment_consolidation` — the OQ-L comment-threading consolidation floor (CHAT-P31 → global
//! P-505; M5-C-X2, R-C8).
//!
//! **Status note (DATED 2026-06-25; re-date on any change — a claim that outlives its verification
//! misleads the next agent, VISION §3 / EI-01 §1).** This module is a *gap-report*: it NAMES the
//! comment-threading consolidation (the OQ-L named follow-on) and records whether its trigger has
//! FIRED. Per VISION §3 (name-your-floors — promote on demand, NOT a rewrite, NOT premature),
//! external-insights/01 §1 (name-your-floors), and the reconciliation's own OQ-L decision
//! (05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-L: "v1 ships two
//! separate threading implementations over one shared `#sub` + content + ref scheme, so
//! consolidation later is a **merge, not a rewrite**"), a consolidation whose trigger has NOT fired
//! stays a **named floor — it is NOT built speculatively**, and its trigger status is recorded here,
//! dated.
//!
//! ## What the consolidation IS (a store/transport swap, NOT a CRDT, NOT a rewrite)
//! The reconciliation (OQ-L) froze TWO threading stores over ONE shared scheme:
//! - **Chat** owns conversation-threads — real-time, presence, the connection tier — over the
//!   `#thread-`/`#message-` `#sub` grammar ([`crate::subs`]), the [`myelin_content`] AST, and the
//!   `refs.edge.created` events (5.4).
//! - **Knowledge/Issues** own document-anchored comment threads — anchored to a stable
//!   block/line/field via `#sub` — over the SAME `#thread-`/`#comment-` grammar
//!   ([`myelin_knowledge::comments`]), the SAME content AST, and the SAME refs events. They are a
//!   SEPARATE store because their concurrency/transport profile differs (Chat: firehose live tier;
//!   KN/Issues: a comment on a CAS-guarded block).
//!
//! Because the two stores ALREADY share the `#sub` grammar + the content AST + the refs edges, the
//! consolidation — when document-anchored comments need real-time multi-party presence — **promotes
//! the anchored comments onto the Chat threading primitive + the firehose resume-cursor transport
//! ([`myelin_events::firehose`], contract 3.5).** The promotion swaps the STORE/TRANSPORT, not the
//! DATA MODEL: the `#thread-`/`#comment-` subjects, the content bodies, and the refs edges are
//! unchanged across the swap, and the per-message CAS (CHAT-P12) is unchanged. It is **NOT a CRDT**
//! (the OQ-L decision is explicit: the related consolidation is a store/transport swap, NOT the
//! collab CRDT — that is a separate, later question) and **NOT a rewrite** (the data model is
//! shared, so the merge is a backing swap).
//!
//! ## Why a gap-report, not a build (the trigger has not fired)
//! The consolidation is taken ONLY when **document-anchored comments need real-time multi-party
//! presence** (the OQ-L trigger). That demand has NOT been observed: the platform's anchored-comment
//! owners (Knowledge [`myelin_knowledge::comments`], and the Issues comment surface) hold their
//! threads in a CAS-guarded OLTP comment store with a resolve/reopen lifecycle — there is **no
//! real-time presence on an anchored comment thread** (no "X is typing on this block", no live
//! multi-party cursor on a comment gutter), and no measured demand for it exists. The chat presence
//! primitive ([`crate::presence`]) + the firehose live tier are CHANNEL/agent-presence surfaces, not
//! anchored-comment surfaces. With the trigger unfired, the v1 floor (two stores, one scheme) is
//! RETAINED and the consolidation **remains a named floor**. Building it now — wiring a CAS-guarded
//! comment store onto a firehose live tier nobody has asked it to live on — would be exactly the
//! "add it before the demand is real" anti-pattern external-insights/04 §5 forbids and the "floor
//! that masquerades as done" VISION §3 forbids.
//!
//! ## The trigger is a REAL, evaluable predicate (not a hand-typed boolean)
//! So the floor promotes on a measured signal — never a hand-flipped flag — the trigger is an
//! evaluable predicate over a measured presence-demand reading: [`AnchoredCommentPresenceDemand`]
//! carries whether anchored comments have been OBSERVED needing real-time multi-party presence (the
//! `live_multiparty_sessions_observed` count over an `over_window`). [`PresenceDemandBudget`] is the
//! demand threshold; `exceeded_by` is the predicate the gap-report evaluates. At this prompt's
//! execution the observed count is **0** (no anchored-comment surface emits or consumes a presence
//! frame), so the predicate is false and the floor stays named.
//!
//! ## What is already BUILT — the seam the consolidation swaps behind (no rewrite)
//! - The shared `#thread-`/`#comment-` `#sub` grammar — [`crate::subs`] (Chat's `message-`/`thread-`
//!   mints) + [`myelin_knowledge::comments`] (Knowledge's `comment-`/`thread-` mints), BOTH through
//!   the ONE [`myelin_refs`] grammar (5.7).
//! - The shared content AST — [`myelin_content`] (13.1): a Chat message body and a KB comment body
//!   are the SAME `Vec<Block>` model.
//! - The shared refs scheme — the `refs.edge.created` events (5.4) both stores emit.
//! - The firehose resume-cursor transport — [`myelin_events::firehose`] (3.5): the live tier the
//!   anchored comments would promote ONTO (the SAME transport chat threads already ride).
//!
//! So the consolidation is a **store/transport swap behind the unchanged shared scheme**, not a
//! redesign.
//!
//! ## The gap-report invariant (this prompt's gate — the "If NOT triggered" branch)
//! The follow-on is recorded as a [`FloorFollowOn`] (the FROZEN chat floor vocabulary — imported
//! from [`crate::scylla_followon`], NOT re-copied, EI-01 §7: one shape, no second copy in the same
//! crate) with a NON-EMPTY trigger, follow-on, preserved-contract, and a dated [`TriggerStatus`].
//! [`comment_consolidation_gap_report`] asserts **0 invisible gaps** AND the **honest-floor
//! invariant** (built ⇒ trigger fired). The trigger predicate is additionally driven against a
//! deliberately-over-budget demand reading so the predicate itself is load-bearing (a mutated
//! threshold is caught).

use crate::scylla_followon::{FloorFollowOn, TriggerStatus};

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE MEASURED TRIGGER SIGNAL — anchored-comment real-time-presence DEMAND (the evaluable predicate)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A **measured reading of the OQ-L trigger signal**: how many times a document-anchored comment
/// thread (Knowledge/Issues) was OBSERVED needing real-time multi-party presence (a live, concurrent,
/// presence-bearing session on the comment gutter — "X is editing this comment", a live cursor on the
/// anchored block's thread) over a measurement window. This is the signal that, when it crosses the
/// [`PresenceDemandBudget`], FIRES the consolidation — never a hand-typed boolean.
///
/// At this prompt's execution the count is `0`: no anchored-comment surface emits or consumes a
/// `chat.presence.*` frame; the comment stores ([`myelin_knowledge::comments`] + the Issues comment
/// surface) are CAS-guarded OLTP threads with a resolve/reopen lifecycle, NOT a firehose live tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchoredCommentPresenceDemand {
    /// The count of OBSERVED live multi-party presence sessions on document-anchored comment threads
    /// over [`Self::over_window`]. `0` = the trigger demand has not been seen.
    pub live_multiparty_sessions_observed: u64,
    /// The measurement window the count was observed over (a dated, human-readable span — an
    /// un-windowed count is itself a gap, the same discipline as a dated `TriggerStatus`).
    pub over_window: &'static str,
}

impl AnchoredCommentPresenceDemand {
    /// The dated reading at this prompt's execution: **0 observed** live multi-party presence
    /// sessions on any document-anchored comment thread. The anchored-comment owners are CAS-guarded
    /// OLTP comment stores with no presence surface; no demand has been measured.
    pub const OBSERVED_NONE: AnchoredCommentPresenceDemand = AnchoredCommentPresenceDemand {
        live_multiparty_sessions_observed: 0,
        over_window:
            "2026-06-25: the whole M5 band to date — 0 anchored-comment presence sessions \
                      observed (no KB/Issues comment surface emits or consumes a chat.presence.* \
                      frame; the comment stores are CAS-guarded OLTP threads, not a firehose live \
                      tier)",
    };
}

/// The **demand threshold** that fires the consolidation: the consolidation is promoted once the
/// observed live-multi-party-presence demand on anchored comments crosses this budget. A non-zero
/// floor (`min_observed_sessions`) so a single accidental/synthetic session does not trip the
/// promotion — the demand must be REAL and sustained (the same measure-before-promote discipline as
/// the Scylla hot-tier budget, ADR-10 / EI-04 §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresenceDemandBudget {
    /// The minimum count of observed live multi-party presence sessions that constitutes REAL demand
    /// (below this, the v1 two-stores-one-scheme floor is retained).
    pub min_observed_sessions: u64,
}

impl PresenceDemandBudget {
    /// The OQ-L trigger budget: the consolidation fires once anchored comments are observed needing
    /// real-time multi-party presence at all (a sustained, non-accidental demand). The threshold is
    /// `1` sustained session — the OQ-L decision phrases the trigger as "when document-anchored
    /// comments NEED real-time multi-party presence", so the budget is the first REAL demand, not a
    /// volume.
    pub const OQ_L: PresenceDemandBudget = PresenceDemandBudget {
        min_observed_sessions: 1,
    };

    /// **The trigger predicate (evaluable, not hand-typed): does this demand reading cross the
    /// budget?** True iff the observed live-multi-party-presence count is at least the budget's
    /// minimum — i.e. anchored comments have been measured needing real-time presence. While false,
    /// the consolidation MUST remain a named floor.
    pub fn exceeded_by(&self, demand: &AnchoredCommentPresenceDemand) -> bool {
        demand.live_multiparty_sessions_observed >= self.min_observed_sessions
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE NAMED FLOOR (the OQ-L consolidation, recorded in the FROZEN chat floor vocabulary)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The comment-threading consolidation — the OQ-L named follow-on this prompt (CHAT-P31 / P-505)
/// names.** Stays a named floor until document-anchored comments are measured needing real-time
/// multi-party presence ([`PresenceDemandBudget::OQ_L`] crossed by an [`AnchoredCommentPresenceDemand`]
/// reading); the gap-report enforces no premature promotion. A store/transport swap over the shared
/// scheme — NOT a CRDT, NOT a rewrite.
pub const COMMENT_CONSOLIDATION_FLOOR: FloorFollowOn = FloorFollowOn {
    id: "comment-threading-consolidation",
    what: "document-anchored comment threads (Knowledge/Issues) promoted onto the Chat threading \
           primitive + the firehose resume-cursor transport (OQ-L / M5-C-X2 / R-C8) — a \
           store/transport SWAP over the shared #thread-/#comment- #sub + content + refs scheme, \
           NOT a CRDT and NOT a rewrite",
    built_seam: "the shared scheme the two stores ALREADY agree on: the #thread-/#comment- #sub \
                 grammar (subs + myelin_knowledge::comments through the ONE myelin_refs grammar, \
                 5.7), the myelin_content AST (the body, 13.1), the refs.edge.created events (5.4), \
                 and the firehose resume-cursor transport (myelin_events::firehose, 3.5) the \
                 anchored comments promote ONTO — so the consolidation is a backing swap, not a \
                 redesign",
    preserved_contract: "5.7 — the shared #sub scheme (the #thread-/#comment- subjects unchanged \
                         across the swap); 13.1 — the shared content AST (the comment body \
                         unchanged); 5.4 — the shared refs edges (unchanged); 3.5 — the firehose \
                         resume-cursor protocol the comments ride; the per-message CAS (CHAT-P12) is \
                         unchanged. Only the STORE/TRANSPORT behind the shared scheme changes, never \
                         the data model",
    trigger: "document-anchored comments are MEASURED needing real-time multi-party presence \
              (PresenceDemandBudget::OQ_L crossed by an AnchoredCommentPresenceDemand reading — a \
              live, concurrent, presence-bearing session on an anchored comment gutter); the \
              promote-on-demand mandate (OQ-L / VISION §3) — not before the demand is real",
    status: TriggerStatus::NotFired {
        as_of: "2026-06-25: 0 anchored-comment real-time-presence sessions observed \
                (AnchoredCommentPresenceDemand::OBSERVED_NONE). The KB/Issues comment surfaces are \
                CAS-guarded OLTP comment threads (myelin_knowledge::comments — create/resolve/reopen \
                over a stable block anchor) with NO presence surface; no KB/Issues comment emits or \
                consumes a chat.presence.* frame, and no demand for real-time multi-party presence \
                on an anchored comment has been measured. The v1 two-stores-one-scheme floor is \
                correct; the consolidation remains named (promote-on-demand: the demand is owed).",
    },
    follow_on: "promote the anchored-comment threads onto the Chat threading primitive (the \
                conversation/thread model + the per-message CAS) and the firehose resume-cursor live \
                tier (presence + partials), keyed by the SAME #thread-/#comment- subjects; the KB/ \
                Issues comment OPS become a write to the Chat threading store, the gutter subscribes \
                to the firehose scope — the #sub + content + refs data model is untouched",
    promotion_gate: "the relevant content + refs + #sub drills re-run GREEN across the \
                     store/transport swap (0 round-trip / edge / #sub regressions post-swap — each \
                     drill written to survive the swap) — CI; the per-message CAS (CHAT-P12) holds \
                     across the swap",
    built: false,
};

/// Every comment-consolidation floor this gap-report accounts for (the one OQ-L consolidation). A
/// `slice` (not a single value) so the manifest extends uniformly if a related consolidation is
/// later named.
pub const COMMENT_CONSOLIDATION_FLOORS: &[FloorFollowOn] = &[COMMENT_CONSOLIDATION_FLOOR];

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE GAP-REPORT GATE (the "If NOT triggered" branch's dated, machine-checked row)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The gap-report gate (CHAT-P31 / P-505 — the "If NOT triggered" branch's dated row).** Asserts,
/// over the OQ-L consolidation floor: (1) it is FULLY recorded (0 invisible gaps), (2) the
/// honest-floor invariant holds (built ⇒ trigger fired), AND (3) the recorded `NotFired` status is
/// CONSISTENT with the evaluable trigger predicate over the measured demand reading — i.e. the floor
/// is `NotFired` exactly because `PresenceDemandBudget::OQ_L` is NOT exceeded by the observed demand.
/// This couples the dated prose status to the real predicate, so the floor cannot silently claim
/// "not fired" while the measured signal says otherwise. Returns `Ok(())` when the gap-report is
/// honest; an `Err` names the inconsistency.
pub fn comment_consolidation_gap_report() -> Result<(), String> {
    consolidation_gap_report_over(
        COMMENT_CONSOLIDATION_FLOORS,
        &AnchoredCommentPresenceDemand::OBSERVED_NONE,
        &PresenceDemandBudget::OQ_L,
    )
}

/// The gap-report verdict over an ARBITRARY floor slice + demand reading + budget (so the predicate
/// coupling is load-bearing — a test can drive it against an over-budget reading or a broken row).
/// Three failure modes: an invisible gap, a premature promotion (built ⇒ ¬fired), or a
/// status/predicate INCONSISTENCY (the recorded `NotFired`/`Fired` disagrees with the measured
/// predicate). Each is an `Err` naming the floor.
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
                "comment-consolidation floor `{}` is a PREMATURE promotion — built with an unfired \
                 trigger (OQ-L / EI-04 §5 forbid promoting before the demand is real)",
                f.id
            ));
        }
        // The dated prose status MUST agree with the evaluable predicate over the measured demand —
        // a floor cannot claim `NotFired` while the predicate says the demand crossed (or vice
        // versa). This couples the human-readable status to the machine-checked signal.
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

    /// The OQ-L consolidation is the one comment-consolidation floor this prompt names, and it is
    /// fully recorded (0 invisible gaps).
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

    /// The honest-floor invariant: at 2026-06-25 the trigger has NOT fired (0 anchored-comment
    /// presence sessions observed), so the consolidation MUST remain a named floor (`built ==
    /// false`) — no premature promotion. The whole gap-report passes.
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
            "the comment-threading consolidation is a NAMED FLOOR — not built speculatively"
        );
        assert!(
            floor.honours_no_premature_promotion(),
            "the honest-floor invariant holds: ¬fired ⇒ ¬built"
        );
        comment_consolidation_gap_report().expect("the gap-report is honest — 0 invisible gaps");
    }

    /// The floor names it as a store/transport SWAP over the shared scheme — NOT a CRDT, NOT a
    /// rewrite — and names the shared #sub/content/refs + firehose contracts it preserves.
    #[test]
    fn the_floor_names_the_swap_not_a_rewrite_and_the_preserved_scheme() {
        let floor = std::hint::black_box(COMMENT_CONSOLIDATION_FLOOR);
        // It is a swap, not a CRDT, not a rewrite (the OQ-L decision is explicit).
        assert!(floor.what.contains("SWAP"));
        assert!(floor.what.contains("NOT a CRDT"));
        assert!(floor.what.contains("NOT a rewrite"));
        // The preserved contracts: 5.7 (#sub) + 13.1 (content) + 5.4 (refs) + 3.5 (firehose).
        assert!(floor.preserved_contract.contains("5.7"));
        assert!(floor.preserved_contract.contains("13.1"));
        assert!(floor.preserved_contract.contains("5.4"));
        assert!(floor.preserved_contract.contains("3.5"));
        // The per-message CAS (CHAT-P12) is unchanged across the swap.
        assert!(floor.preserved_contract.contains("CHAT-P12"));
        // The trigger names the measured presence-demand signal + the promote-on-demand mandate.
        assert!(floor.trigger.contains("real-time multi-party presence"));
        assert!(floor.trigger.contains("PresenceDemandBudget::OQ_L"));
        assert!(!floor.status.dated().is_empty());
    }

    /// The trigger PREDICATE is real + evaluable (not a hand-typed boolean): the observed-none demand
    /// does NOT cross the OQ-L budget (so the floor is correctly unfired), and a synthetic
    /// over-budget reading DOES cross it (so the predicate would actually promote the floor — it is
    /// load-bearing, not vacuous).
    #[test]
    fn the_trigger_predicate_is_real_and_evaluable() {
        let budget = PresenceDemandBudget::OQ_L;
        // The measured reading at this prompt: 0 sessions → predicate false → floor stays named.
        let observed = AnchoredCommentPresenceDemand::OBSERVED_NONE;
        assert_eq!(observed.live_multiparty_sessions_observed, 0);
        assert!(
            !budget.exceeded_by(&observed),
            "0 observed presence sessions must NOT cross the OQ-L demand budget"
        );
        // A synthetic demand reading that crosses the budget → predicate true (the promotion would
        // fire). The predicate is therefore load-bearing — a mutated threshold flips this.
        let demand_real = AnchoredCommentPresenceDemand {
            live_multiparty_sessions_observed: 1,
            over_window: "synthetic: a real anchored-comment presence session was observed",
        };
        assert!(
            budget.exceeded_by(&demand_real),
            "a real anchored-comment presence session MUST cross the OQ-L demand budget"
        );
        // The window string is non-empty in the dated reading (an un-windowed count is a gap).
        assert!(!observed.over_window.is_empty());
    }

    /// The gap-report COUPLES the dated prose status to the evaluable predicate: it is `Ok` when the
    /// recorded `NotFired` agrees with the unfired predicate, and `Err` (an inconsistency) when the
    /// floor claims `NotFired` while the measured demand actually crosses the budget — so a stale
    /// "not fired" prose claim over a real signal is caught.
    #[test]
    fn gap_report_couples_status_to_the_measured_predicate() {
        // Ok over the real manifest (NotFired status + unfired predicate agree).
        assert!(comment_consolidation_gap_report().is_ok());
        // Ok over an honest custom slice with a consistent (unfired) status + (unfired) predicate.
        assert!(consolidation_gap_report_over(
            COMMENT_CONSOLIDATION_FLOORS,
            &AnchoredCommentPresenceDemand::OBSERVED_NONE,
            &PresenceDemandBudget::OQ_L,
        )
        .is_ok());
        // Err (status/predicate inconsistency): the recorded floor is `NotFired`, but feed a demand
        // reading that CROSSES the budget — the prose status now disagrees with the measured signal.
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

    /// The gap-report's other two failure modes stay load-bearing on this manifest path: an invisible
    /// gap (an empty field) and a premature promotion (built ⇒ unfired) each Err, naming the floor.
    #[test]
    fn gap_report_catches_invisible_gap_and_premature_promotion() {
        // An invisible gap: empty `trigger` field.
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

        // A premature promotion: built with an unfired trigger. (Use a row whose recorded status is
        // `Fired` so the status/predicate check is reached only AFTER the premature-promotion check —
        // here the premature check fires first because `built && !fired`.)
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
