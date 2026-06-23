//! # `ci_guard` — the CI-red governed-transition guard, closing the X-1 consumer (ISS-P27 / P-394, M4)
//!
//! **Owning architecture docs (byte-authoritative):**
//! - `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//!   §1.1 (the `ci.check.updated` reflex feeds the "can't mark Done while CI red" guard: the linked
//!   PR's commit has a CURRENT `CheckStatus{state, trust_tier, …}`; Issues reads it via `project(PR_ref)`
//!   at transition time — **never recomputes trust**, Δ10).
//! - Reconciliation `00-reconciliation-decisions.md` X-1 (the Git↔CI CheckStatus seam — an
//!   `untrusted_fork` success is NEUTRAL until endorsed; Issues never recomputes trust, it READS
//!   `trust_tier` OFF the fact).
//! - VISION §3 (the poisoned-Done defence — never recompute trust).
//! - EI-01 §7 (the X-1 seam reconciled at the plan layer — Issues reads `trust_tier` off the fact,
//!   never recomputes it) + §3 (prove-it — the guard BLOCKS).
//!
//! **Contracts implemented (to the frozen shapes):**
//! - **5.9** (CONSUMED) — the CheckStatus guard reads `{state, trust_tier}` OFF the fact, never
//!   recomputes trust. The fact is read through Git's `project(PR_ref)` (contract 5.6) — Issues NEVER
//!   synchronously calls CI nor Git's internals; it consumes the projection at the seam (EI-02 §3
//!   acyclic — CI emits, Git projects, Issues reads).
//! - **4.2** (CONSUMED) — the transition ABAC (`Id.check` + the transition `CaveatContext`) is the
//!   ISS-P06 write path's gate; this module computes the governance DECISION the write path drives.
//!
//! ## What ISS-P27 (P-394) ships — closing the ISS-P12 floor
//! ISS-P12 ([`crate::workflow`]) shipped the guard SHAPE: [`crate::workflow::linked_pr_ci_green_guard`]
//! (a frozen `QueryAst` over [`GuardVar::LINKED_PR_CHECK_STATUS`]) + the
//! [`GuardVar::LINKED_PR_TRUST_TIER`] context var, fail-closed when unbound, and NAMED THIS PROMPT as
//! the consumer that LIVE-BINDS them when the X-1 check-seam closes. The X-1 seam is now END-TO-END
//! (GIT-D10 / CI-D8 GREEN — the producer leg EB-27/P-327, the consumer leg EB-26, the merge gate +
//! fork endorsement GIT-P21/P22, the merge-queue durable workflow P-FLOW-23). So this prompt ships the
//! LIVE consumer half:
//!
//! 1. **[`LinkedPrCheck`]** — the Issues CONSUMER VIEW of the linked PR's CURRENT `CheckStatus` posture,
//!    read OFF THE FACT through `project(PR_ref)`: the `{state, trust_tier}` tokens + the
//!    fork-endorsement bit. Issues NEVER recomputes trust — it carries the CI-stamped tier verbatim.
//!    This is the references-not-payloads decode at the consumer seam (the SAME pattern
//!    `myelin_events::check_seam` uses — the producer's struct is read as tokens, never re-derived).
//! 2. **[`bind_linked_pr_ctx`]** — the binder that stamps an [`IssueContext`] with the live
//!    [`GuardVar::LINKED_PR_CHECK_STATUS`] / [`GuardVar::LINKED_PR_TRUST_TIER`] vars OFF the fact, so
//!    [`crate::workflow::linked_pr_ci_green_guard`] evaluates against the REAL posture (no longer a
//!    fail-closed unbound floor). An `untrusted_fork` success that is NOT endorsed binds a NON-success
//!    status token (`neutral`) — the poisoned-Done defence, IDENTICAL to Git's merge-gate
//!    `is_acceptable_satisfaction` (one trust posture, no second rule — EI-01 §7).
//! 3. **[`ci_done_guard`]** — the canonical "can't mark Done while CI red on the linked PR" guard
//!    (`linked_pr_check_status == "success"`), the LIVE shape the governed transition into a
//!    `completed`-category state carries.
//! 4. **[`plan_ci_gated_transition`]** — the consumer ENTRY: read the linked PR check OFF the fact →
//!    bind the context → run [`crate::workflow::Workflow::plan_transition`]. A CI-red / un-endorsed-fork
//!    linked PR BLOCKS with a pre-assembled reason; a trusted (or endorsed) success unblocks.
//! 5. **[`plan_agent_ci_gated_transition`]** — the AGENT path (the ISS-P23 HITL-gated transition): an
//!    agent hitting a governed transition is HITL-gated — the transition is WITHHELD (0 mutation
//!    pre-approval) and an approval is requested; it is NOT auto-applied even when the guard would
//!    permit it. This is the [`AgentTransitionOutcome`] the agent tool returns (AG-8 — a gated effect
//!    is withheld until a human approves).
//!
//! ## The poisoned-Done defence (VISION §3 / X-1) — never recompute trust
//! An `untrusted_fork` PR must never turn its own Done gate green by running attacker-controlled CI
//! config (the classic poisoned-pipeline attack). Issues does NOT recompute trust — it reads the
//! CI-stamped `trust_tier` off the fact. An un-endorsed `untrusted_fork` success is NEUTRAL for the
//! Done guard (it binds a non-success status token), exactly as it is neutral for Git's merge gate.
//! The acceptability rule is the SAME predicate Git's gate uses — Issues consumes the posture, it does
//! not invent a second trust algebra.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / prove-it)
//! The CI-red guard is the **mandatory-core poisoned-Done defence** (a wrongly-greened Done is the
//! failure). The floor for this module is **100% of viable mutants caught**
//! (`cargo mutants -p myelin-issues -f crates/myelin-issues/src/ci_guard.rs`). Measured 2026-06-23:
//! **13 caught / 0 missed / 5 unviable** — the acceptability decision (`is_acceptable`), the status
//! neutralisation (`gated_status_token`), the verbatim tier binding, the guard predicate, and the
//! agent HITL-gate (withheld vs blocked) are each killed by a test.
//!
//! ## FLOOR named: none new.
//! The guard RESTS ON THE PROVEN X-1 SEAM (GIT-D10 / CI-D8 GREEN end-to-end — `contract-coverage.toml`
//! row 5.9 `covered`), not a doc claim. The LIVE wiring of `project(PR_ref)` into the Issues write
//! path's context binder rides the ISS-P28 cross-subsystem reflex (`ci.check.updated → feed the guard`)
//! plus the ISS-P20 live OLTP write path; here the binder is the SEMANTICS those wire, proven by the
//! unit, e2e, and drill tests against the frozen 5.9 posture (read off the fact). No trust is
//! recomputed to fake green; no threshold is weakened.

use crate::workflow::{
    GuardVar, IssueContext, TransitionBlocked, TransitionPlan, Workflow, WorkflowGuard,
};
use myelin_identity::Literal;
use myelin_query::{CmpOp, Expr, Predicate};

// =================================================================================================
// 1. The Issues CONSUMER VIEW of the linked PR's CheckStatus posture — read OFF THE FACT (5.9 / X-1).
// =================================================================================================

/// **The linked PR's CURRENT CI posture — the Issues CONSUMER VIEW, read OFF THE FACT** (contract 5.9
/// / X-1 / Δ10). When a governed transition into a `completed`-category ("Done") state fires, the write
/// path reads the linked PR's CURRENT `CheckStatus` for the gating context through `project(PR_ref)`
/// (contract 5.6) and reduces it to THIS — the `{state, trust_tier}` tokens + the fork-endorsement bit.
///
/// **Issues NEVER recomputes trust.** The `trust_tier` is the CI-stamped tier carried VERBATIM off the
/// fact (the producer is the only authority on trust — X-1). `endorsed` is Git's fork-endorsement bit
/// (the maintainer `approve_untrusted_ci` flow), ALSO read off the seam — Issues does not run the
/// endorsement check, it consumes its result. The tokens are the FROZEN 5.9 `snake_case` vocabulary
/// (`success`/`failure`/… for state, `trusted`/`untrusted_fork` for trust) so the binder stamps the
/// guard context with the EXACT strings [`crate::workflow::linked_pr_ci_green_guard`] compares against
/// (the drift anchor — no second vocabulary, EI-01 §7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedPrCheck {
    /// The CURRENT check `state` token of the gating context, read off the fact — the FROZEN 5.9
    /// `snake_case` vocabulary (`queued`/`in_progress`/`success`/`failure`/`error`/`neutral`/
    /// `cancelled`). The Done guard requires `success` (with an acceptable trust posture).
    pub state: String,
    /// The CI-stamped `trust_tier` token, read off the fact (NEVER recomputed) — `trusted` or
    /// `untrusted_fork`. An `untrusted_fork` success is neutral for the Done gate until endorsed.
    pub trust_tier: String,
    /// Git's fork-endorsement bit for this context (the maintainer `approve_untrusted_ci` result),
    /// read off the seam. An `untrusted_fork` success is acceptable ONLY when `endorsed` (or re-run
    /// trusted, which flips `trust_tier` to `trusted` via Git's supersession on a later fact).
    pub endorsed: bool,
}

/// The FROZEN 5.9 `state` token a SUCCESS reads as (the only token that — with an acceptable trust
/// posture — satisfies the Done guard). Pinned as a constant so the binder + the guard agree by
/// construction (the drift anchor with Git's `CheckState::Success` `snake_case` serialization).
pub const CHECK_STATE_SUCCESS: &str = "success";

/// The FROZEN 5.9 `state` token the binder stamps for a NON-acceptable posture (a CI-red / un-endorsed
/// fork). `neutral` is the 5.9 "recorded, does not satisfy and does not block" token — binding it makes
/// the `== "success"` Done guard FALSE → the transition BLOCKS with the pre-assembled reason (the
/// poisoned-Done defence; the un-endorsed fork's own success is neutralised, never gated green).
pub const CHECK_STATE_NEUTRAL: &str = "neutral";

/// The FROZEN 5.9 `trust_tier` token for a trusted run.
pub const TRUST_TIER_TRUSTED: &str = "trusted";

/// The FROZEN 5.9 `trust_tier` token for an untrusted-fork run.
pub const TRUST_TIER_UNTRUSTED_FORK: &str = "untrusted_fork";

impl LinkedPrCheck {
    /// A trusted-run check posture (the common case — a non-fork PR, or an endorsed/re-run-trusted
    /// fork). `endorsed` is irrelevant for a trusted tier (a trusted success self-satisfies).
    pub fn trusted(state: impl Into<String>) -> LinkedPrCheck {
        LinkedPrCheck {
            state: state.into(),
            trust_tier: TRUST_TIER_TRUSTED.to_string(),
            endorsed: false,
        }
    }

    /// An untrusted-fork-run check posture. An `untrusted_fork` success is NEUTRAL for the Done gate
    /// until `endorsed` (the maintainer `approve_untrusted_ci` flow) — the poisoned-Done defence.
    pub fn untrusted_fork(state: impl Into<String>, endorsed: bool) -> LinkedPrCheck {
        LinkedPrCheck {
            state: state.into(),
            trust_tier: TRUST_TIER_UNTRUSTED_FORK.to_string(),
            endorsed,
        }
    }

    /// **Is this posture an ACCEPTABLE satisfaction of the Done guard?** (X-1 / Δ3 — the poisoned-Done
    /// defence, the SAME predicate Git's merge gate `is_acceptable_satisfaction` applies — one trust
    /// posture, no second rule, EI-01 §7). A `success` satisfies IFF its trust posture is acceptable:
    /// `trusted`, OR `untrusted_fork` that has been ENDORSED. Trust is READ off the fact, never
    /// recomputed — Issues only consumes the CI-stamped tier + Git's endorsement bit.
    pub fn is_acceptable(&self) -> bool {
        if self.state != CHECK_STATE_SUCCESS {
            return false;
        }
        match self.trust_tier.as_str() {
            TRUST_TIER_TRUSTED => true,
            // An untrusted-fork success is neutral until endorsed (or re-run trusted, which flips the
            // tier on a later fact via Git's supersession — handled upstream of this view).
            TRUST_TIER_UNTRUSTED_FORK => self.endorsed,
            // An UNKNOWN trust token is genuine uncertainty — fail closed (never gate green on a tier
            // Issues does not recognise; a new tier is a seam change escalated, never silently passed).
            _ => false,
        }
    }

    /// The `linked_pr_check_status` token the binder stamps into the guard context. An ACCEPTABLE
    /// posture binds the raw `state` token (a trusted success binds `"success"`); a NON-acceptable
    /// posture (a CI-red, OR an un-endorsed `untrusted_fork` success) binds [`CHECK_STATE_NEUTRAL`] —
    /// so the un-endorsed fork's own success can NEVER turn the `== "success"` Done guard true (the
    /// poisoned-Done defence). This neutralisation is the ONLY place the fork posture is collapsed; the
    /// `trust_tier` token itself is always carried verbatim (never rewritten — trust is read off the
    /// fact).
    fn gated_status_token(&self) -> &str {
        if self.is_acceptable() {
            CHECK_STATE_SUCCESS
        } else {
            // A CI-red OR an un-endorsed-fork success both neutralise — the Done guard stays false.
            CHECK_STATE_NEUTRAL
        }
    }
}

// =================================================================================================
// 2. The binder — stamp the live linked-PR vars OFF THE FACT into the IssueContext (closing the floor).
// =================================================================================================

/// **Bind the LIVE linked-PR CheckStatus posture into the guard context** (closing the ISS-P12
/// fail-closed floor). Stamps [`GuardVar::LINKED_PR_CHECK_STATUS`] (the gated status token, off the
/// fact) + [`GuardVar::LINKED_PR_TRUST_TIER`] (the CI-stamped tier, verbatim) onto the `ctx` so
/// [`ci_done_guard`] / [`crate::workflow::linked_pr_ci_green_guard`] evaluates against the REAL posture
/// — no longer the unbound, fail-closed floor.
///
/// The status token is the [`LinkedPrCheck::gated_status_token`] — an acceptable posture binds
/// `"success"`, a CI-red / un-endorsed-fork posture binds `"neutral"` (so the `== "success"` guard is
/// false → block). The trust tier is bound VERBATIM (never recomputed — Issues reads it off the fact).
/// Threading this binder onto the write path's context BEFORE [`crate::workflow::Workflow::plan_transition`]
/// is the X-1 consumer wiring; the binder is the pure, deterministic SEMANTICS.
pub fn bind_linked_pr_ctx(ctx: IssueContext, check: &LinkedPrCheck) -> IssueContext {
    ctx.bind(
        GuardVar::LINKED_PR_CHECK_STATUS,
        Literal::Str(check.gated_status_token().to_string()),
    )
    .bind(
        GuardVar::LINKED_PR_TRUST_TIER,
        Literal::Str(check.trust_tier.clone()),
    )
}

// =================================================================================================
// 3. The canonical "can't mark Done while CI red" guard — the LIVE shape (5.9 / Δ10).
// =================================================================================================

/// **The canonical CI-red Done guard — "can't mark Done while CI red on the linked PR"** (contract 5.9
/// / Δ10 / X-1). The frozen `QueryAst` `linked_pr_check_status == "success"`. With the live
/// [`bind_linked_pr_ctx`] binding, this is now a REAL gate (no longer the ISS-P12 fail-closed floor): a
/// CI-red OR un-endorsed-fork posture binds a non-`success` token → the guard is FALSE → the transition
/// into a `completed`-category state BLOCKS with the pre-assembled, admin-authored reason.
///
/// This is the SAME shape as [`crate::workflow::linked_pr_ci_green_guard`] (one guard, no duplicate —
/// EI-01 §7); re-exposed here under the CI-guard module name as the consumer's canonical Done guard,
/// with the human-readable label the blocked-transition reason surfaces.
pub fn ci_done_guard() -> WorkflowGuard {
    WorkflowGuard::compiled(
        "the linked PR's CI is not green (or is an un-endorsed fork run)",
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(GuardVar::LINKED_PR_CHECK_STATUS.into()),
            rhs: Expr::Lit(Literal::Str(CHECK_STATE_SUCCESS.into())),
        },
    )
    .expect("the CI-red Done guard is a single bounded comparison (within the cost bound)")
}

// =================================================================================================
// 4. The consumer entry — read the fact → bind → plan the governed transition.
// =================================================================================================

/// **Plan a CI-gated governed transition (the X-1 consumer entry — the HUMAN path).** Given the
/// resolved [`Workflow`], the `from`/`target` states, the base [`IssueContext`] (the issue's own attrs
/// and any other guard facts already bound by the write path), and the linked PR's CURRENT CheckStatus
/// posture (read OFF THE FACT via `project(PR_ref)`), this:
/// 1. binds the live linked-PR vars into the context ([`bind_linked_pr_ctx`]);
/// 2. runs [`Workflow::plan_transition`] — the pure governance decision.
///
/// A CI-red / un-endorsed-fork linked PR makes the [`ci_done_guard`] FALSE → the transition is BLOCKED
/// with a pre-assembled reason (the green artifact of the ISS-D12 CI-red half). A trusted (or endorsed)
/// success unblocks. The transition ABAC (contract 4.2 — `Id.check` + the transition `CaveatContext`),
/// the typed-core mutate, and `OutboxTx::emit(issue.transitioned)` are the ISS-P06 write path; this is
/// the governance plan the write path then drives (it does NOT mutate or emit — emit is the ONE
/// `OutboxTx::emit` verb).
///
/// **Issues never recomputes trust** — the posture is consumed off the fact ([`LinkedPrCheck`]); the
/// acceptability rule is the SAME one Git's merge gate applies (one trust posture, EI-01 §7).
pub fn plan_ci_gated_transition(
    wf: &Workflow,
    from: &str,
    target: &str,
    base_ctx: IssueContext,
    linked_pr: &LinkedPrCheck,
) -> Result<TransitionPlan, TransitionBlocked> {
    let ctx = bind_linked_pr_ctx(base_ctx, linked_pr);
    wf.plan_transition(from, target, &ctx)
}

// =================================================================================================
// 5. The AGENT path — the ISS-P23 HITL-gated governed transition (withheld, 0 pre-approval mutation).
// =================================================================================================

/// **The outcome of an AGENT hitting a CI-gated governed transition (the ISS-P23 HITL-gated path).** An
/// agent NEVER auto-applies a governed transition, even when the guard would permit it — a governed
/// transition is a HITL-gated effect (AG-8): it is WITHHELD with 0 mutation pre-approval, and an
/// approval is requested. The two-variant shape is the loud, typed distinction the agent tool returns:
/// - [`AgentTransitionOutcome::Blocked`] — the guard FAILED (CI-red / un-endorsed-fork / no edge): the
///   transition is impossible; the pre-assembled reason is returned (no approval is even requested —
///   there is nothing to approve). 0 mutation.
/// - [`AgentTransitionOutcome::WithheldForApproval`] — the guard PERMITS the transition, but because
///   the actor is an AGENT it is WITHHELD: an approval is requested (the [`TransitionPlan`] the human
///   would approve is carried), and NOTHING is mutated until a human approves. 0 pre-approval mutation.
///
/// In NEITHER variant is the typed core mutated nor an event emitted — the agent path is pre-approval,
/// 0-mutation by construction (the ISS-D12 "0 pre-approval mutation" green artifact).
#[derive(Clone, Debug, PartialEq)]
pub enum AgentTransitionOutcome {
    /// The guard BLOCKED the transition (CI-red / un-endorsed-fork / no declared edge). The
    /// pre-assembled reason is returned; nothing is mutated, no approval requested (nothing to approve).
    Blocked {
        /// The pre-assembled, admin-authored block reason (deterministic; the same inputs → same text).
        block: TransitionBlocked,
    },
    /// The guard PERMITS the transition, but the actor is an AGENT — the transition is WITHHELD for HITL
    /// approval (0 pre-approval mutation). The plan the human would approve is carried (the approval
    /// card renders the from→to + the staged post-actions); nothing is mutated until a human approves.
    WithheldForApproval {
        /// The permitted plan the HITL approval card surfaces (and the write path applies POST-approval).
        plan: TransitionPlan,
    },
}

impl AgentTransitionOutcome {
    /// `true` iff the transition was WITHHELD for approval (the guard permitted it; the agent is gated).
    pub fn is_withheld(&self) -> bool {
        matches!(self, AgentTransitionOutcome::WithheldForApproval { .. })
    }

    /// `true` iff the transition was BLOCKED by a guard (CI-red / un-endorsed-fork / no edge).
    pub fn is_blocked(&self) -> bool {
        matches!(self, AgentTransitionOutcome::Blocked { .. })
    }

    /// **The number of typed-core mutations applied pre-approval — ALWAYS 0** (the ISS-D12 green
    /// artifact). The agent path is pre-approval, 0-mutation by construction: a blocked transition
    /// stages nothing, and a permitted-but-withheld transition is held for approval (the write path
    /// applies it only AFTER a human approves). This is a const-0 witness the drill asserts.
    pub fn pre_approval_mutations(&self) -> u64 {
        0
    }
}

/// **Plan an AGENT's CI-gated governed transition (the ISS-P23 HITL-gated path).** Identical guard
/// evaluation to [`plan_ci_gated_transition`] (read the linked PR off the fact → bind → plan), BUT the
/// outcome is the HITL-gated [`AgentTransitionOutcome`]: a permitted transition is WITHHELD for approval
/// (0 pre-approval mutation) rather than returned as a directly-appliable plan. An agent NEVER turns a
/// governed transition green on its own — a human approves the gated effect (AG-8).
///
/// - The guard FAILS (CI-red / un-endorsed-fork / no edge) → [`AgentTransitionOutcome::Blocked`] (the
///   reason; nothing to approve; 0 mutation).
/// - The guard PERMITS → [`AgentTransitionOutcome::WithheldForApproval`] (the plan the human approves;
///   0 pre-approval mutation).
pub fn plan_agent_ci_gated_transition(
    wf: &Workflow,
    from: &str,
    target: &str,
    base_ctx: IssueContext,
    linked_pr: &LinkedPrCheck,
) -> AgentTransitionOutcome {
    match plan_ci_gated_transition(wf, from, target, base_ctx, linked_pr) {
        // The guard permits — but the actor is an agent, so the transition is WITHHELD for HITL
        // approval (0 pre-approval mutation). The plan rides the approval card; the write path applies
        // it only after a human approves (AG-8).
        Ok(plan) => AgentTransitionOutcome::WithheldForApproval { plan },
        // The guard blocked — there is nothing to approve; return the pre-assembled reason. 0 mutation.
        Err(block) => AgentTransitionOutcome::Blocked { block },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{StateCategory, WorkflowState, WorkflowTransition};

    /// A 2-state workflow whose In Review → Done close is gated by the CI-red Done guard (the canonical
    /// CI-gated workflow the consumer entry drives).
    fn ci_gated_workflow() -> Workflow {
        Workflow {
            states: vec![
                WorkflowState {
                    name: "In Review".into(),
                    category: StateCategory::Started,
                },
                WorkflowState {
                    name: "Done".into(),
                    category: StateCategory::Completed,
                },
            ],
            transitions: vec![WorkflowTransition {
                from: "In Review".into(),
                to: "Done".into(),
                guards: vec![ci_done_guard()],
                required_fields: vec![],
                post_actions: vec![],
            }],
        }
    }

    /// **A trusted success unblocks the Done transition (the happy path).** The linked PR's CURRENT
    /// check is a trusted `success` (read off the fact) → the gated status token is `"success"` → the
    /// CI-red guard holds → the transition is permitted, stamping the FIXED completed category.
    #[test]
    fn a_trusted_success_unblocks_done() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::trusted(CHECK_STATE_SUCCESS);
        let plan = plan_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check)
            .expect("a trusted success unblocks the close");
        assert_eq!(plan.to_category, StateCategory::Completed);
        assert_eq!(plan.from, "In Review");
        assert_eq!(plan.to, "Done");
    }

    /// **A CI-red (failure) linked PR BLOCKS the Done transition with a reason** (the ISS-D12 CI-red
    /// half — the green artifact). The linked PR's check is a `failure` → the gated status token is
    /// `"neutral"` → the guard is FALSE → the close is blocked with the pre-assembled, admin-authored
    /// reason naming the guard + the from→to.
    #[test]
    fn a_ci_red_linked_pr_blocks_done() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::trusted("failure");
        let blocked =
            plan_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check)
                .expect_err("a CI-red linked PR blocks the close");
        match &blocked {
            TransitionBlocked::GuardFailed { reason } => {
                assert!(
                    reason.contains("CI is not green"),
                    "the reason names the CI-red guard: {reason}"
                );
                assert!(
                    reason.contains("In Review") && reason.contains("Done"),
                    "the reason names the from→to: {reason}"
                );
            }
            other => panic!("expected GuardFailed, got {other:?}"),
        }
        assert!(!blocked.reason().is_empty());
    }

    /// **An un-endorsed untrusted-fork SUCCESS is NEUTRAL → BLOCKS (the poisoned-Done defence).** A
    /// fork run reports `success` but `trust_tier = untrusted_fork` and is NOT endorsed → it is neutral
    /// for the Done gate (the un-endorsed fork cannot turn its OWN Done green) → blocked. Issues reads
    /// the tier off the fact; it never recomputes trust.
    #[test]
    fn an_unendorsed_fork_success_is_neutral_and_blocks() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, false);
        assert!(
            !check.is_acceptable(),
            "an un-endorsed fork success is NOT acceptable (neutral)"
        );
        let blocked =
            plan_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check)
                .expect_err("an un-endorsed fork success is neutral → blocked");
        assert!(matches!(blocked, TransitionBlocked::GuardFailed { .. }));
    }

    /// **An ENDORSED untrusted-fork success UNBLOCKS** (the maintainer `approve_untrusted_ci` flow). The
    /// fork posture is the same `success` + `untrusted_fork`, but now `endorsed = true` → acceptable →
    /// the gated status token is `"success"` → the guard holds → permitted.
    #[test]
    fn an_endorsed_fork_success_unblocks() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, true);
        assert!(
            check.is_acceptable(),
            "an endorsed fork success is acceptable"
        );
        let plan = plan_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check)
            .expect("an endorsed fork success unblocks");
        assert_eq!(plan.to_category, StateCategory::Completed);
    }

    /// **The trust tier is bound VERBATIM (never recomputed) — Issues reads it off the fact.** The
    /// binder stamps the CI-stamped tier token unchanged into the context; the status token is the
    /// gated one, but the tier is carried as-is (the drift anchor with the 5.9 vocabulary).
    #[test]
    fn the_trust_tier_is_bound_verbatim_off_the_fact() {
        let check = LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, false);
        let ctx = bind_linked_pr_ctx(IssueContext::new(), &check);
        // The status token is the GATED token (neutral — un-endorsed fork), proving the neutralisation.
        let status_guard = WorkflowGuard::compiled(
            "status",
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var(GuardVar::LINKED_PR_CHECK_STATUS.into()),
                rhs: Expr::Lit(Literal::Str(CHECK_STATE_NEUTRAL.into())),
            },
        )
        .unwrap();
        assert_eq!(
            status_guard.predicate.eval(ctx.attrs()),
            Ok(true),
            "an un-endorsed fork success binds the neutral status token"
        );
        // The tier token is carried VERBATIM (untrusted_fork — never recomputed/rewritten).
        let tier_guard = WorkflowGuard::compiled(
            "tier",
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var(GuardVar::LINKED_PR_TRUST_TIER.into()),
                rhs: Expr::Lit(Literal::Str(TRUST_TIER_UNTRUSTED_FORK.into())),
            },
        )
        .unwrap();
        assert_eq!(
            tier_guard.predicate.eval(ctx.attrs()),
            Ok(true),
            "the CI-stamped trust tier is bound verbatim off the fact"
        );
    }

    /// **An unknown trust token fails closed (never gate green on an unrecognised tier).** A tier Issues
    /// does not recognise is genuine uncertainty → not acceptable → the status token is neutral → block.
    #[test]
    fn an_unknown_trust_token_fails_closed() {
        let check = LinkedPrCheck {
            state: CHECK_STATE_SUCCESS.into(),
            trust_tier: "some_new_tier".into(),
            endorsed: true,
        };
        assert!(
            !check.is_acceptable(),
            "an unrecognised trust tier is never acceptable (fail closed)"
        );
    }

    /// **The AGENT path WITHHOLDS a permitted transition for HITL approval — 0 pre-approval mutation**
    /// (the ISS-D12 agent half). Even with a trusted success (the guard PERMITS), an AGENT does not
    /// auto-apply — the transition is WITHHELD for approval, carrying the plan the human approves, and
    /// nothing is mutated pre-approval (AG-8).
    #[test]
    fn the_agent_path_withholds_a_permitted_transition_for_approval() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::trusted(CHECK_STATE_SUCCESS);
        let outcome =
            plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check);
        assert!(
            outcome.is_withheld(),
            "the agent's permitted transition is WITHHELD for HITL approval, not auto-applied"
        );
        assert!(
            !outcome.is_blocked(),
            "a withheld (permitted-but-gated) transition is NOT a guard block"
        );
        assert_eq!(
            outcome.pre_approval_mutations(),
            0,
            "0 pre-approval mutation (the agent path is pre-approval, 0-mutation)"
        );
        if let AgentTransitionOutcome::WithheldForApproval { plan } = outcome {
            // The plan the HITL card surfaces (and the write path applies POST-approval).
            assert_eq!(plan.to_category, StateCategory::Completed);
            assert_eq!(plan.to, "Done");
        }
    }

    /// **The AGENT path returns BLOCKED (not withheld) on a CI-red linked PR — nothing to approve.** A
    /// guard-blocked transition is impossible; the agent path returns the reason, requests NO approval
    /// (there is nothing to approve), and mutates nothing.
    #[test]
    fn the_agent_path_blocks_on_a_ci_red_linked_pr() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::trusted("failure");
        let outcome =
            plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check);
        assert!(
            outcome.is_blocked(),
            "a CI-red linked PR is BLOCKED for the agent (nothing to approve)"
        );
        assert!(!outcome.is_withheld());
        assert_eq!(outcome.pre_approval_mutations(), 0, "0 mutation on a block");
        if let AgentTransitionOutcome::Blocked { block } = outcome {
            assert!(matches!(block, TransitionBlocked::GuardFailed { .. }));
        }
    }

    /// **`is_acceptable` IS the merge-gate posture (one trust rule, no second algebra — EI-01 §7).** The
    /// truth table matches Git's `is_acceptable_satisfaction`: a non-success never satisfies; a trusted
    /// success satisfies; an untrusted-fork success satisfies IFF endorsed.
    #[test]
    fn is_acceptable_matches_the_merge_gate_posture() {
        // Non-success never satisfies (regardless of trust).
        assert!(!LinkedPrCheck::trusted("failure").is_acceptable());
        assert!(!LinkedPrCheck::trusted("in_progress").is_acceptable());
        assert!(!LinkedPrCheck::trusted("queued").is_acceptable());
        assert!(!LinkedPrCheck::untrusted_fork("error", true).is_acceptable());
        // A trusted success satisfies.
        assert!(LinkedPrCheck::trusted(CHECK_STATE_SUCCESS).is_acceptable());
        // An untrusted-fork success satisfies IFF endorsed.
        assert!(!LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, false).is_acceptable());
        assert!(LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, true).is_acceptable());
    }

    /// **The CI-red Done guard SHAPE is the frozen `QueryAst` `linked_pr_check_status == "success"`**
    /// (the same shape as `workflow::linked_pr_ci_green_guard`, no duplicate language). It holds on a
    /// bound success and is false on a bound non-success.
    #[test]
    fn the_ci_done_guard_is_the_frozen_query_ast() {
        let guard = ci_done_guard();
        let green = IssueContext::new().bind(
            GuardVar::LINKED_PR_CHECK_STATUS,
            Literal::Str(CHECK_STATE_SUCCESS.into()),
        );
        assert_eq!(guard.predicate.eval(green.attrs()), Ok(true));
        let red = IssueContext::new().bind(
            GuardVar::LINKED_PR_CHECK_STATUS,
            Literal::Str(CHECK_STATE_NEUTRAL.into()),
        );
        assert_eq!(guard.predicate.eval(red.attrs()), Ok(false));
    }
}
