//! # `reflexes` — the cross-subsystem reflexes (git/chat/identity/ci consumers) (ISS-P28 / P-395, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//! §1.1 (the cross-subsystem reflexes — the flows C1–C4):
//! | Consumed event | Reflex |
//! |---|---|
//! | `git.branch.created` (refs an issue) | create the issue↔branch ref edge; **workflow-permitting** auto-transition → *In Progress* |
//! | `git.pr.opened` / `git.pr.merged` | link PR↔issue (`closes` typed edge); on merge, transition → *Done* IFF the guard is satisfied |
//! | `ci.check.updated` (the frozen `CheckStatus`, contract 5.9) | feed the "can't mark Done while CI red" guard — read the tier OFF the fact, never recompute (Δ10) |
//! | `chat.message.created` ("create issue") | `issue.create` with the chat message as a `relates` ref edge |
//! | `identity.member.added` / deactivated / **erased** | reassign/anonymise: the actor becomes the frozen pseudonym across history (the erasure lever, §7) |
//!
//! **VISION §2** — *work flows between tools*: this module IS the differentiator. A branch opened in
//! Git advances the issue; a chat message becomes an issue; a merged PR closes the work — all WITHOUT
//! bespoke point-to-point integrations. **EI-01 §7** — each reflex is a CONSUMER off the bus
//! ([`myelin_events::EventHandler`], contract 2.4), never a synchronous cross-subsystem call; the
//! acyclic-producer invariant (EI-02 §3) holds (Git/CI/Chat/Identity emit; Issues consumes — Issues
//! imports NO producer crate, it whitelists the foreign subjects as validated tokens).
//!
//! ## Contracts implemented (to the frozen shapes)
//! - **2.4** (CONSUMED) — every reflex is an [`EventHandler`]: a `*`-free subject whitelist
//!   ([`reflex_subjects`]) + idempotent-on-`event_id` `handle`. The runtime's `consumer_dedup` ledger
//!   (contract 2.5) is the durable anchor; this module ALSO dedups within the handler (the same
//!   pattern [`crate::rollup::RollupConsumer`] uses) so a replay produces **0 duplicate** links/issues.
//! - **5.4** (CONSUMED) — the link/relates edges are `refs.edge.created` PRODUCED by the content nodes
//!   (the `closes`/`relates` typed edges go through the ONE [`crate::refs_glue::emit_relation_edge`]
//!   TE-7 mirror, contract 5.5; there is NO standalone edge-write API — the edge rides the issue's
//!   typed-row write). The reflex STAGES the edge; the write path emits it.
//! - **5.9** (CONSUMED) — `ci.check.updated` feeds the guard via the ISS-P27 [`crate::ci_guard`]: the
//!   posture is read OFF the fact ([`crate::ci_guard::LinkedPrCheck`]); Issues NEVER recomputes trust.
//!
//! ## The no-governance-bypass invariant (FLOOR named: auto-transitions are workflow-permitting only)
//! An auto-transition driven by a reflex (a branch advancing an issue to *In Progress*, a merge
//! closing it to *Done*) **runs through the FSM interpreter** ([`crate::workflow::Workflow::plan_transition`])
//! — it is NEVER a bypass of the workflow guards. If the workflow does not declare the edge, or a guard
//! BLOCKS it (e.g. the CI-red Done guard on a merge with a red linked PR), the reflex
//! produces a [`ReflexEffect::TransitionBlocked`] (the link still lands; the transition does not). A
//! reflex can never green a governed transition the human path could not — there is no second
//! transition authority (EI-01 §7). This is the **0-governance-bypass** green artifact.
//!
//! ## Plan, don't mutate (the ci_guard pattern — emit is the ONE `OutboxTx::emit` verb)
//! Each reflex is a PURE planner: it reduces an incoming foreign event to a typed [`ReflexEffect`] (a
//! staged edge / a planned transition / an anonymise op / a guard feed). It does NOT mutate the typed
//! core nor emit — the ISS-P06 write path ([`crate::write_path`]) drives the staged effect inside the
//! one validate → check → mutate → `OutboxTx::emit` transaction (emit-iff-committed). This keeps the
//! reflex deterministic + unit-testable + idempotent, exactly like [`crate::ci_guard`].
//!
//! ## Mutation-score floor (mandatory-core? — NO, but the no-bypass + 0-dup are CI-gated)
//! The reflexes are NOT the mandatory-core poisoned-Done defence (that is [`crate::ci_guard`], 100%).
//! The reflex module's invariants — **0 duplicate on replay** + **0 governance bypass** — are CI gates
//! asserted by the unit + e2e tests below (the GATE artifacts named in the prompt), not a mutation
//! floor. No threshold is weakened: the no-bypass assertion routes every auto-transition through the
//! real FSM interpreter, and the 0-dup assertion replays a `git.pr.merged` and asserts the effect count
//! is unchanged.
//!
//! ## FLOOR named: none new.
//! The reflexes REST on the proven producers (the M3 Git `git.*`, the M4 CI `ci.check.*` X-1 seam
//! GIT-D10/CI-D8 green, the Chat `chat.message.created`, Identity `identity.member.*`) + the existing
//! Issues machinery (the FSM interpreter ISS-P12, the TE-7 mirror ISS-P17, the CI guard ISS-P27, the
//! pseudonym anonymise ISS-P07). The only NAMED posture is the no-bypass note above (auto-transitions
//! are workflow-permitting only). The live OLTP write of a staged effect rides the ISS-P06/P20 write
//! path; here the planner is the SEMANTICS, proven by the unit + e2e + CDC tests.

use crate::ci_guard::{plan_ci_gated_transition, LinkedPrCheck};
use crate::refs_glue::IssueLifecycleRel;
use crate::workflow::{TransitionPlan, Workflow};
use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use std::collections::BTreeSet;
use std::sync::Mutex;

// =================================================================================================
// 0. The consumed foreign-subject whitelist (contract 2.4 rule 3 — NEVER `*`).
//
// Issues imports NO producer crate (the acyclic-producer invariant, EI-02 §3): the foreign subjects
// are whitelisted as the frozen grammatical tokens (each PROVEN grammatical by the ONE Bus validator
// in the tests below). A rename on the producer side is a contract change reconciled at the seam.
// =================================================================================================

/// `git.branch.created` — a branch referencing an issue was created (the C1 reflex).
pub const GIT_BRANCH_CREATED: &str = "git.branch.created";
/// `git.pr.opened` — a PR referencing an issue was opened (the C2 reflex; `closes` edge).
pub const GIT_PR_OPENED: &str = "git.pr.opened";
/// `git.pr.merged` — a PR was merged (the C2 reflex; on merge → auto-transition to Done IFF permitted).
pub const GIT_PR_MERGED: &str = "git.pr.merged";
/// `ci.check.updated` — the frozen `CheckStatus` (contract 5.9 / X-1); feeds the CI-red Done guard.
pub const CI_CHECK_UPDATED: &str = "ci.check.updated";
/// `chat.message.created` — a chat "create issue" message (the C3 reflex; `relates` edge).
pub const CHAT_MESSAGE_CREATED: &str = "chat.message.created";
/// `identity.member.added` — a member joined (the C4 reflex; reassign on the actor↔pseudonym map).
pub const IDENTITY_MEMBER_ADDED: &str = "identity.member.added";
/// `identity.member.deactivated` — a member was deactivated (the C4 reflex; reassign their open work).
pub const IDENTITY_MEMBER_DEACTIVATED: &str = "identity.member.deactivated";
/// `identity.member.erased` — a member was erased (the C4 reflex; anonymise across history — §7).
pub const IDENTITY_MEMBER_ERASED: &str = "identity.member.erased";

/// The FROZEN cross-subsystem reflex subject whitelist (contract 2.4 rule 3 — `*`-free). Each token is
/// grammatical against the ONE Bus validator (`myelin_events::validate_event_type`) and is CONSUMED,
/// never originated by Issues (the acyclic-producer invariant, EI-02 §3).
pub const REFLEX_SUBJECTS: &[&str] = &[
    GIT_BRANCH_CREATED,
    GIT_PR_OPENED,
    GIT_PR_MERGED,
    CI_CHECK_UPDATED,
    CHAT_MESSAGE_CREATED,
    IDENTITY_MEMBER_ADDED,
    IDENTITY_MEMBER_DEACTIVATED,
    IDENTITY_MEMBER_ERASED,
];

/// The `&'static [SubjectPattern]` whitelist the [`EventHandler`] binds (contract 2.4 rule 3). Built
/// once from [`REFLEX_SUBJECTS`]; the over-broad `*` is unconstructable (the subscription rejects it
/// at registration — BUS-3 / D7-i).
pub fn reflex_subjects() -> &'static [SubjectPattern] {
    use std::sync::OnceLock;
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| {
            REFLEX_SUBJECTS
                .iter()
                .map(|s| SubjectPattern((*s).to_string()))
                .collect()
        })
        .as_slice()
}

// =================================================================================================
// 1. The typed reflex EFFECT — what a reflex STAGES for the write path to drive (plan, don't mutate).
// =================================================================================================

/// **The typed, staged effect a reflex produces** (the ci_guard "plan, don't mutate" pattern). A
/// reflex reduces an incoming foreign event to ONE of these; the ISS-P06 write path
/// ([`crate::write_path`]) drives it inside the one validate → check → mutate → `OutboxTx::emit`
/// transaction. The reflex itself NEVER mutates the typed core nor emits (emit is the ONE
/// `OutboxTx::emit` verb) — this keeps the reflex deterministic, unit-testable, and idempotent.
#[derive(Clone, Debug, PartialEq)]
pub enum ReflexEffect {
    /// **Link an issue to a Git/Chat artifact via a typed edge + (optionally) a workflow-permitting
    /// auto-transition** (the C1/C2/C3 flows). The edge is the TE-7 `issue_relation` typed-row the ONE
    /// [`crate::refs_glue::emit_relation_edge`] mirror emits (contract 5.5; the `refs.edge.created`
    /// rides the typed-row write — there is NO standalone edge-write API, 5.4). The optional
    /// `transition` is a PERMITTED [`TransitionPlan`] the FSM interpreter approved (workflow-permitting
    /// only — `None` if the workflow does not permit it / there is no edge to advance).
    Link {
        /// The issue the foreign artifact references (the `src_issue` of the `issue_relation` row).
        issue: String,
        /// The foreign artifact URN (the `dst_ref` — a Git branch/PR, or a Chat message).
        artifact: String,
        /// The typed lifecycle relation (the `closes` for a PR, `relates` for a branch/chat message).
        rel: IssueLifecycleRel,
        /// The workflow-permitting auto-transition the FSM interpreter approved, if any (NEVER a
        /// bypass — it ran through [`crate::workflow::Workflow::plan_transition`]). `None` if the
        /// workflow does not declare/permit the edge (the link still lands; the transition does not).
        transition: Option<TransitionPlan>,
    },
    /// **Create a new issue from a chat message + a `relates` ref edge back to the message** (the C3
    /// "create issue from chat" flow). The write path mints the canonical key (ISS-P08), writes the
    /// typed core, and emits `issue.created` + the `relates` `issue.relation.created` edge in the one
    /// transaction. The `source_message` is the `dst_ref` of the `relates` edge.
    CreateIssueFromChat {
        /// The chat message URN the new issue relates back to (the `relates` edge `dst_ref`).
        source_message: String,
        /// The issue title lifted from the chat message (the write path seals it under the per-subject
        /// DEK — ISS-P07; the reflex carries the cleartext the planner saw).
        title: String,
    },
    /// **An auto-transition the workflow BLOCKED** (the no-governance-bypass artifact). The reflex
    /// tried a workflow-permitting auto-transition (a merge → Done) but a guard BLOCKED it (e.g. the
    /// CI-red Done guard on a merge with a red linked PR — contract 5.9). The link still lands (a
    /// separate [`ReflexEffect::Link`] effect carries it); THIS effect records the block reason so the
    /// reflex is loud, never a silent allow AND never a silent drop. The transition does NOT fire.
    TransitionBlocked {
        /// The issue whose auto-transition was blocked.
        issue: String,
        /// The pre-assembled, admin-authored block reason (deterministic — same inputs → same text).
        reason: String,
    },
    /// **Reassign/anonymise an actor across the issue history** (the C4 `identity.member.*` flow, the
    /// §7 erasure lever). The actor's pseudonymous principal is rewritten to the frozen anonymised
    /// pseudonym `<pseudonym>@<tenant>.noreply` across the issues they touched (assignee / reporter /
    /// comment-author / change-log-actor) WITHOUT rewriting issues others own. Issues drives Identity's
    /// `erase` for the pseudonym-map shred (contract 4.8); this effect is the Issues-side reassign the
    /// write path applies. The reassign target is the frozen anonymised handle (NEVER a raw id).
    AnonymiseActor {
        /// The pseudonymous principal being anonymised/reassigned (the `<pseudonym>@<tenant>.noreply`
        /// handle stored in the Issues identity columns — NEVER a raw id, contract 4.8).
        actor_pseudonym: String,
        /// `true` iff this is the ERASURE lever (`identity.member.erased`, §7) — the pseudonym-map is
        /// shredded so the handle resolves to "Former user <opaque>" across history. `false` for a
        /// deactivate/add reassign (the handle persists; only open work is reassigned).
        is_erasure: bool,
    },
    /// **Feed the CI-red Done guard with the linked PR's CURRENT posture** (the C / 5.9 flow). A
    /// `ci.check.updated` for a commit on an issue's linked PR refreshes the [`LinkedPrCheck`] the
    /// guard reads — the posture is read OFF the fact (Issues NEVER recomputes trust). The effect
    /// carries the issue + the refreshed posture; the write path updates the cached `CheckStatus`
    /// projection the next Done transition reads (it does NOT itself transition — the guard is
    /// CONSULTED at transition time, not fired by the check update).
    GuardFeed {
        /// The issue whose linked-PR check posture is refreshed.
        issue: String,
        /// The CURRENT linked-PR CheckStatus posture, read OFF the fact (5.9 / X-1 — never recomputed).
        check: LinkedPrCheck,
    },
    /// **A no-op** — the incoming event does NOT reference an Issues artifact (e.g. a `git.branch.created`
    /// on a branch with no issue key, a `chat.message.created` that is not a "create issue" message).
    /// The reflex is loud about a malformed-but-relevant event (poison) but SILENT-OK about an
    /// irrelevant one: not every foreign event is a reflex trigger, and an irrelevant one is acked, not
    /// dead-lettered.
    NoOp,
}

// =================================================================================================
// 2. The reflex PLANNERS — pure functions: foreign event → typed effect (plan, don't mutate).
// =================================================================================================

/// The well-known payload keys a foreign producer stamps onto its event (the reflex reads these OFF
/// the payload — it never re-derives them). Named as constants so the seam is the drift anchor with
/// the producer (a producer rename is reconciled here, never silently mis-read).
pub mod payload_key {
    /// The issue URN a Git branch/PR references (the branch name / PR body carries `<PROJECTKEY>-<n>`;
    /// the Git producer resolves it to the canonical issue URN and stamps it here).
    pub const ISSUE_REF: &str = "issue_ref";
    /// The Git artifact URN (the branch URN for `git.branch.created`, the PR URN for `git.pr.*`).
    pub const ARTIFACT_REF: &str = "artifact_ref";
    /// The chat message URN (`chat.message.created`) the new issue relates back to.
    pub const MESSAGE_REF: &str = "message_ref";
    /// The chat "create issue" title lifted from the message body.
    pub const TITLE: &str = "title";
    /// `true` iff the chat message is a "create issue" command (the C3 trigger — not every message).
    pub const CREATE_ISSUE: &str = "create_issue";
    /// The anonymised/pseudonymous actor handle (`<pseudonym>@<tenant>.noreply`, contract 4.8).
    pub const ACTOR_PSEUDONYM: &str = "actor_pseudonym";
    /// The linked-PR check `state` token (the frozen 5.9 `snake_case` vocabulary).
    pub const CHECK_STATE: &str = "state";
    /// The linked-PR check `trust_tier` token (`trusted` / `untrusted_fork`, read OFF the fact).
    pub const TRUST_TIER: &str = "trust_tier";
    /// Git's fork-endorsement bit (the maintainer `approve_untrusted_ci` result, read OFF the seam).
    pub const ENDORSED: &str = "endorsed";
}

/// Read a string field off an event payload (the reflex reads OFF the fact — references-not-payloads
/// decode at the consumer seam). Returns `None` if absent or not a string.
fn payload_str<'a>(ev: &'a EventEnvelope, key: &str) -> Option<&'a str> {
    ev.payload.get(key).and_then(|v| v.as_str())
}

/// Read a bool field off an event payload (defaulting to `false` if absent/non-bool).
fn payload_bool(ev: &EventEnvelope, key: &str) -> bool {
    ev.payload
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// The admin-named workflow state a branch advances an issue INTO (the C1 reflex target). A
/// `git.branch.created` referencing an issue advances it to *In Progress* IFF the workflow permits
/// the `from → In Progress` edge (workflow-permitting only — never a bypass).
pub const AUTO_STATE_IN_PROGRESS: &str = "In Progress";
/// The admin-named workflow state a merge closes an issue INTO (the C2 reflex target). A
/// `git.pr.merged` closes the linked issue to *Done* IFF the workflow permits it AND the guards
/// (incl. the CI-red Done guard, 5.9) are satisfied.
pub const AUTO_STATE_DONE: &str = "Done";

/// **Plan the `git.branch.created` reflex (C1): link the issue↔branch + workflow-permitting
/// auto-transition → In Progress.** Reads the referenced issue + the branch URN OFF the payload; if
/// the branch references no issue, [`ReflexEffect::NoOp`]. The auto-transition runs through the FSM
/// interpreter ([`Workflow::plan_transition`]) from `current_state` → [`AUTO_STATE_IN_PROGRESS`]: a
/// PERMITTED transition is carried on the [`ReflexEffect::Link`]; a transition the workflow does not
/// permit (no declared edge, or a guard blocks) leaves `transition: None` — the link STILL lands, the
/// transition does NOT (workflow-permitting only, the no-bypass invariant). `wf` + `current_state` are
/// the resolved workflow + the issue's current state the write path supplies.
pub fn plan_branch_created(ev: &EventEnvelope, wf: &Workflow, current_state: &str) -> ReflexEffect {
    let (Some(issue), Some(branch)) = (
        payload_str(ev, payload_key::ISSUE_REF),
        payload_str(ev, payload_key::ARTIFACT_REF),
    ) else {
        return ReflexEffect::NoOp;
    };
    // A branch RELATES to the issue (the Git branch is not a `closes` — only a merged PR closes).
    // The workflow-permitting auto-transition advances the issue to In Progress (NEVER a bypass: it
    // goes through the FSM interpreter; an undeclared/guarded edge leaves the transition unset).
    let transition = wf
        .plan_transition(current_state, AUTO_STATE_IN_PROGRESS, &Default::default())
        .ok();
    ReflexEffect::Link {
        issue: issue.to_string(),
        artifact: branch.to_string(),
        rel: IssueLifecycleRel::Relates,
        transition,
    }
}

/// **Plan the `git.pr.opened` reflex (C2, opened): link the PR↔issue via a `closes` edge.** A PR that
/// references an issue mints the `closes` typed edge (merging the PR will close the issue). Opening a
/// PR does NOT auto-transition (the issue advances on MERGE, not on open) — so `transition: None`. If
/// the PR references no issue, [`ReflexEffect::NoOp`].
pub fn plan_pr_opened(ev: &EventEnvelope) -> ReflexEffect {
    let (Some(issue), Some(pr)) = (
        payload_str(ev, payload_key::ISSUE_REF),
        payload_str(ev, payload_key::ARTIFACT_REF),
    ) else {
        return ReflexEffect::NoOp;
    };
    ReflexEffect::Link {
        issue: issue.to_string(),
        artifact: pr.to_string(),
        rel: IssueLifecycleRel::Closes,
        transition: None,
    }
}

/// **Plan the `git.pr.merged` reflex (C2, merged): link the `closes` edge + workflow-permitting
/// auto-transition → Done IFF the guard is satisfied.** The merge advances the linked issue to *Done*
/// — but ONLY through the FSM interpreter, AND only if the CI-red Done guard (contract 5.9) is
/// satisfied by the linked PR's CURRENT CheckStatus posture (read OFF the merge event's payload, the
/// X-1 seam — Issues NEVER recomputes trust). This returns a PAIR of effects:
/// - the [`ReflexEffect::Link`] (the `closes` edge always lands), carrying the PERMITTED transition if
///   the guard + workflow allow the close;
/// - if the guard BLOCKED the auto-close, a [`ReflexEffect::TransitionBlocked`] recording the reason
///   (the no-governance-bypass artifact — the merge cannot green a Done the human path could not).
///
/// `wf` + `current_state` are the resolved workflow + the issue's current state. The linked-PR posture
/// is read off the payload via [`linked_pr_from_payload`]; an absent posture is treated as a NON-green
/// check (fail-closed — a merge whose check posture Issues cannot read does NOT auto-close).
pub fn plan_pr_merged(ev: &EventEnvelope, wf: &Workflow, current_state: &str) -> Vec<ReflexEffect> {
    let (Some(issue), Some(pr)) = (
        payload_str(ev, payload_key::ISSUE_REF),
        payload_str(ev, payload_key::ARTIFACT_REF),
    ) else {
        return vec![ReflexEffect::NoOp];
    };
    // The linked PR's CURRENT posture, read OFF the fact (5.9 / X-1 — never recomputed). An absent
    // posture fails closed (a non-success check → the Done guard blocks).
    let check = linked_pr_from_payload(ev)
        .unwrap_or_else(|| LinkedPrCheck::trusted(crate::ci_guard::CHECK_STATE_NEUTRAL));
    // The workflow-permitting auto-close runs through the FSM interpreter (NEVER a bypass) WITH the
    // CI-red Done guard binding (the SAME consumer entry the human path uses — ISS-P27).
    let plan = plan_ci_gated_transition(
        wf,
        current_state,
        AUTO_STATE_DONE,
        Default::default(),
        &check,
    );
    let mut effects = Vec::with_capacity(2);
    match plan {
        Ok(transition) => {
            // The guard PERMITS: the `closes` edge lands AND the issue auto-closes to Done.
            effects.push(ReflexEffect::Link {
                issue: issue.to_string(),
                artifact: pr.to_string(),
                rel: IssueLifecycleRel::Closes,
                transition: Some(transition),
            });
        }
        Err(block) => {
            // The guard BLOCKED (CI-red / un-endorsed fork / no declared edge): the `closes` edge
            // STILL lands (the PR↔issue link is a fact), but the auto-close does NOT fire — and the
            // block is LOUD (the no-governance-bypass artifact), never a silent allow nor a silent drop.
            effects.push(ReflexEffect::Link {
                issue: issue.to_string(),
                artifact: pr.to_string(),
                rel: IssueLifecycleRel::Closes,
                transition: None,
            });
            effects.push(ReflexEffect::TransitionBlocked {
                issue: issue.to_string(),
                reason: block.reason(),
            });
        }
    }
    effects
}

/// **Plan the `ci.check.updated` reflex (5.9): feed the CI-red Done guard.** Reads the linked PR's
/// CURRENT CheckStatus posture OFF the fact (the `{state, trust_tier, endorsed}` tokens — the frozen
/// 5.9 vocabulary; Issues NEVER recomputes trust). The effect refreshes the cached posture the next
/// Done transition's guard reads — it does NOT itself transition (the guard is CONSULTED at transition
/// time, not fired by a check update). If the check references no issue, [`ReflexEffect::NoOp`].
pub fn plan_check_updated(ev: &EventEnvelope) -> ReflexEffect {
    let Some(issue) = payload_str(ev, payload_key::ISSUE_REF) else {
        return ReflexEffect::NoOp;
    };
    let Some(check) = linked_pr_from_payload(ev) else {
        return ReflexEffect::NoOp;
    };
    ReflexEffect::GuardFeed {
        issue: issue.to_string(),
        check,
    }
}

/// Read the linked-PR [`LinkedPrCheck`] posture OFF an event payload (the references-not-payloads
/// decode at the 5.9 seam). Reads the `{state, trust_tier, endorsed}` tokens VERBATIM (never recomputed
/// — Issues consumes the CI-stamped posture). Returns `None` if the `state`/`trust_tier` tokens are
/// absent (a non-check event). The `trust_tier` is carried verbatim into the [`LinkedPrCheck`] so the
/// guard's acceptability rule (`is_acceptable`) — the SAME predicate Git's merge gate applies — decides.
pub fn linked_pr_from_payload(ev: &EventEnvelope) -> Option<LinkedPrCheck> {
    let state = payload_str(ev, payload_key::CHECK_STATE)?;
    let trust_tier = payload_str(ev, payload_key::TRUST_TIER)?;
    let endorsed = payload_bool(ev, payload_key::ENDORSED);
    Some(LinkedPrCheck {
        state: state.to_string(),
        trust_tier: trust_tier.to_string(),
        endorsed,
    })
}

/// **Plan the `chat.message.created` reflex (C3): create an issue from a chat message + a `relates`
/// edge.** A chat message that is a "create issue" command (the `create_issue` payload bit set) mints
/// a new issue with the message body as the title and a `relates` ref edge back to the message. A
/// message that is NOT a create-issue command is [`ReflexEffect::NoOp`] (not every message creates an
/// issue). The new issue's canonical key is minted by the write path (ISS-P08); the reflex carries the
/// title + the source message URN.
pub fn plan_chat_message_created(ev: &EventEnvelope) -> ReflexEffect {
    if !payload_bool(ev, payload_key::CREATE_ISSUE) {
        return ReflexEffect::NoOp;
    }
    let (Some(message), Some(title)) = (
        payload_str(ev, payload_key::MESSAGE_REF),
        payload_str(ev, payload_key::TITLE),
    ) else {
        return ReflexEffect::NoOp;
    };
    ReflexEffect::CreateIssueFromChat {
        source_message: message.to_string(),
        title: title.to_string(),
    }
}

/// **Plan an `identity.member.*` reflex (C4): reassign/anonymise the actor across issue history (§7).**
/// `is_erasure` selects the lever: an `identity.member.erased` (the §7 erasure lever) shreds the
/// pseudonym map so the actor's handle resolves to "Former user <opaque>" across history WITHOUT
/// rewriting issues others own; an `added`/`deactivated` is a reassign (the handle persists; open work
/// is reassigned). Reads the pseudonymous actor handle OFF the payload (`<pseudonym>@<tenant>.noreply`,
/// contract 4.8 — NEVER a raw id). An event carrying no actor handle is [`ReflexEffect::NoOp`].
pub fn plan_member_event(ev: &EventEnvelope, is_erasure: bool) -> ReflexEffect {
    let Some(actor) = payload_str(ev, payload_key::ACTOR_PSEUDONYM) else {
        return ReflexEffect::NoOp;
    };
    ReflexEffect::AnonymiseActor {
        actor_pseudonym: actor.to_string(),
        is_erasure,
    }
}

/// **Plan a reflex for ANY whitelisted foreign event (the consumer dispatch).** Routes the incoming
/// event by its [`EventEnvelope::type_`] to the matching planner. A `git.pr.merged` yields a VEC (the
/// `closes` link + the possible `TransitionBlocked`); every other reflex yields exactly one effect.
/// An event whose type is NOT on the whitelist is a programming error (the consumer's subject whitelist
/// should have filtered it) — it returns a single [`ReflexEffect::NoOp`] rather than panicking. `wf` +
/// `current_state` are only consulted by the transition-bearing reflexes (branch/merge).
pub fn plan_reflex(ev: &EventEnvelope, wf: &Workflow, current_state: &str) -> Vec<ReflexEffect> {
    match ev.type_.0.as_str() {
        GIT_BRANCH_CREATED => vec![plan_branch_created(ev, wf, current_state)],
        GIT_PR_OPENED => vec![plan_pr_opened(ev)],
        GIT_PR_MERGED => plan_pr_merged(ev, wf, current_state),
        CI_CHECK_UPDATED => vec![plan_check_updated(ev)],
        CHAT_MESSAGE_CREATED => vec![plan_chat_message_created(ev)],
        IDENTITY_MEMBER_ADDED | IDENTITY_MEMBER_DEACTIVATED => {
            vec![plan_member_event(ev, false)]
        }
        IDENTITY_MEMBER_ERASED => vec![plan_member_event(ev, true)],
        // Off-whitelist: the subject whitelist should have filtered this; treat as a no-op (the
        // consumer's rule-3 whitelist is the real guard — this is belt-and-braces).
        _ => vec![ReflexEffect::NoOp],
    }
}

// =================================================================================================
// 3. The idempotent reflex CONSUMER (contract 2.4) — 0 duplicate on replay.
// =================================================================================================

/// **The cross-subsystem reflex consumer (contract 2.4 — the EventHandler).** Whitelists the foreign
/// reflex subjects ([`reflex_subjects`], `*`-free), is idempotent on `event_id` (the within-handler
/// dedup on TOP of the runtime's `consumer_dedup` ledger — the SAME pattern
/// [`crate::rollup::RollupConsumer`] uses), and STAGES the planned [`ReflexEffect`]s (the write path
/// drives them). A replayed event produces **0 duplicate** staged effects (the ISS-P28 0-dup green
/// artifact) — the second delivery is a no-op.
///
/// The consumer holds the resolved [`Workflow`] (the transition-bearing reflexes consult it) + the
/// per-issue current state (a real consumer reads this off the projection; the drill seeds it). It
/// COLLECTS the staged effects so a drill can assert the 0-dup + no-bypass invariants; in production
/// the write path drains them per delivery inside the one outbox transaction.
pub struct ReflexConsumer {
    state: Mutex<ReflexState>,
}

struct ReflexState {
    /// The resolved workflow the transition-bearing reflexes (branch/merge) run through (the FSM
    /// interpreter — NEVER a bypass). A real consumer loads the issue's resolved workflow scheme; the
    /// drill seeds a fixed one.
    workflow: Workflow,
    /// The per-issue current state the auto-transition's `from` reads (a real consumer reads the
    /// projection; the drill seeds it). Keyed by issue URN.
    current_state: std::collections::BTreeMap<String, String>,
    /// The `event_id`s already handled (idempotent on `event_id`, contract 2.4) — the within-handler
    /// dedup on top of the runtime's `consumer_dedup` ledger. A replay is a no-op (0 dup).
    seen_events: BTreeSet<String>,
    /// The staged effects the write path drives (collected so the drill asserts 0-dup + no-bypass).
    staged: Vec<ReflexEffect>,
}

impl ReflexConsumer {
    /// A fresh reflex consumer over the resolved `workflow` (the FSM the transition-bearing reflexes
    /// run through — never a bypass). The per-issue current state is seeded via [`Self::set_state`].
    pub fn new(workflow: Workflow) -> ReflexConsumer {
        ReflexConsumer {
            state: Mutex::new(ReflexState {
                workflow,
                current_state: std::collections::BTreeMap::new(),
                seen_events: BTreeSet::new(),
                staged: Vec::new(),
            }),
        }
    }

    /// Seed/refresh an issue's current workflow state (a real consumer reads this off the projection;
    /// the drill seeds it so the auto-transition's `from` is the issue's real state).
    pub fn set_state(&self, issue: &str, state: &str) {
        let mut s = self.state.lock().expect("reflex state lock");
        s.current_state.insert(issue.to_string(), state.to_string());
    }

    /// The issue's current state, defaulting to the FROZEN initial state if unseeded (a brand-new
    /// issue's branch reflex advances from the workflow's first declared state).
    fn state_of(state: &ReflexState, issue: &str) -> String {
        state
            .current_state
            .get(issue)
            .cloned()
            .or_else(|| state.workflow.states.first().map(|s| s.name.clone()))
            .unwrap_or_default()
    }

    /// The staged effects so far (the write path drains these; the drill reads them to assert the
    /// 0-dup + no-bypass invariants). A snapshot copy.
    pub fn staged(&self) -> Vec<ReflexEffect> {
        self.state.lock().expect("reflex state lock").staged.clone()
    }

    /// The number of staged effects (the 0-dup drill asserts this is unchanged across a replay).
    pub fn staged_count(&self) -> usize {
        self.state.lock().expect("reflex state lock").staged.len()
    }
}

impl EventHandler for ReflexConsumer {
    /// The whitelist — the foreign reflex subjects ONLY, NEVER `*` (contract 2.4 rule 3 / BUS-3).
    fn subjects(&self) -> &'static [SubjectPattern] {
        reflex_subjects()
    }

    /// **Handle one foreign reflex event (contract 2.4 — idempotent on `event_id`; off the bus).**
    /// Idempotent: a redelivered `event_id` is a no-op (the within-handler dedup on top of the
    /// runtime's `consumer_dedup` ledger) → **0 duplicate** staged effects on replay. Routes the
    /// event through [`plan_reflex`] (the auto-transitions run through the FSM interpreter — never a
    /// bypass) and STAGES the resulting effects; the write path drives them. The reflex never mutates
    /// the typed core nor emits (emit is the ONE `OutboxTx::emit` verb).
    ///
    /// A `NoOp` effect (an irrelevant event — a branch with no issue key, a non-create-issue chat
    /// message) is acked, NOT dead-lettered: not every foreign event is a reflex trigger.
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
        let mut state = self.state.lock().expect("reflex state lock");
        // Idempotent on event_id (contract 2.4) — a redelivery is a no-op (0 duplicate staged).
        if !state.seen_events.insert(ev.event_id.0.clone()) {
            return HandleOutcome::Done;
        }
        // The subject must carry a non-empty type (a malformed envelope is poison — it can never
        // become well-formed by retry).
        if ev.type_.0.is_empty() {
            return HandleOutcome::NonRetryable(Reason(
                "reflex: event carries no type — cannot route the reflex".into(),
            ));
        }
        // The current state the transition-bearing reflexes read (off the seeded projection).
        let issue = payload_str(ev, payload_key::ISSUE_REF)
            .unwrap_or_default()
            .to_string();
        let current = Self::state_of(&state, &issue);
        let effects = plan_reflex(ev, &state.workflow, &current);
        for effect in effects {
            // A no-op stages nothing (an irrelevant event — acked, not dead-lettered).
            if effect != ReflexEffect::NoOp {
                state.staged.push(effect);
            }
        }
        HandleOutcome::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{StateCategory, WorkflowState, WorkflowTransition};
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventId, EventType, Timestamp,
        Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn ev(type_: &str, payload: serde_json::Value) -> EventEnvelope {
        ev_with_id("e-1", type_, payload)
    }

    fn ev_with_id(id: &str, type_: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ArtifactRef("myelin://acme/issue/issue/ENG-1".into()),
            aggregate: AggregateKey("issue:ENG-1".into()),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
            payload,
        }
    }

    /// A 3-state workflow: Todo →(branch)→ In Progress →(merge, CI-gated)→ Done. The merge edge is
    /// gated by the CI-red Done guard (the SAME guard the human path uses — ISS-P27).
    fn dev_workflow() -> Workflow {
        Workflow {
            states: vec![
                WorkflowState {
                    name: "Todo".into(),
                    category: StateCategory::Unstarted,
                },
                WorkflowState {
                    name: AUTO_STATE_IN_PROGRESS.into(),
                    category: StateCategory::Started,
                },
                WorkflowState {
                    name: AUTO_STATE_DONE.into(),
                    category: StateCategory::Completed,
                },
            ],
            transitions: vec![
                WorkflowTransition {
                    from: "Todo".into(),
                    to: AUTO_STATE_IN_PROGRESS.into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: AUTO_STATE_IN_PROGRESS.into(),
                    to: AUTO_STATE_DONE.into(),
                    guards: vec![crate::ci_guard::ci_done_guard()],
                    required_fields: vec![],
                    post_actions: vec![],
                },
            ],
        }
    }

    // ---- the subject whitelist is *-free + grammatical (contract 2.4 rule 3 / 2.9) ----

    /// **Every reflex subject is grammatical against the ONE Bus validator + `*`-free (contract 2.4
    /// rule 3 / 2.9).** Issues consumes these foreign subjects; each parses the §6.1 grammar (Issues
    /// imports no producer crate — the acyclic-producer invariant, EI-02 §3).
    #[test]
    fn every_reflex_subject_is_grammatical_and_wildcard_free() {
        for &subj in REFLEX_SUBJECTS {
            assert!(
                myelin_events::validate_event_type(subj).is_ok(),
                "reflex subject `{subj}` is UNGRAMMATICAL: {:?}",
                myelin_events::validate_event_type(subj)
            );
            assert!(
                !subj.contains('*') && !subj.contains('>'),
                "no `*`/`>`: {subj}"
            );
        }
        // a consumer subscription over these is constructable (rule 3 admits them).
        let subjects: Vec<&str> = REFLEX_SUBJECTS.to_vec();
        let sub = myelin_events::Subscription::bind(
            myelin_events::ConsumerName("issue-reflexes".into()),
            &subjects,
            myelin_events::PrefetchBound::DEFAULT,
        );
        assert!(sub.is_ok(), "the reflex whitelist binds: {sub:?}");
    }

    /// Issues registers/consumes NO Issues-originated reflex subject — each is a FOREIGN subsystem
    /// (the acyclic-producer invariant). None carries the `issue` prefix.
    #[test]
    fn no_reflex_subject_is_issue_originated() {
        for &subj in REFLEX_SUBJECTS {
            assert!(
                !subj.starts_with("issue."),
                "reflex subject `{subj}` must be FOREIGN (consumed, not originated)"
            );
        }
    }

    // ---- C1: git.branch.created → relates link + workflow-permitting auto-transition ----

    /// **A `git.branch.created` referencing an issue links it + auto-advances to In Progress (through
    /// the FSM interpreter — never a bypass).** The reflex stages a `relates` Link carrying the
    /// PERMITTED transition the FSM approved (Todo → In Progress is a declared, unguarded edge).
    #[test]
    fn branch_created_links_and_auto_advances_through_the_fsm() {
        let wf = dev_workflow();
        let e = ev(
            GIT_BRANCH_CREATED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/branch/eng-1-fix",
            }),
        );
        let effect = plan_branch_created(&e, &wf, "Todo");
        match effect {
            ReflexEffect::Link {
                issue,
                artifact,
                rel,
                transition,
            } => {
                assert_eq!(issue, "myelin://acme/issue/issue/ENG-1");
                assert_eq!(artifact, "myelin://acme/git/branch/eng-1-fix");
                assert_eq!(rel, IssueLifecycleRel::Relates);
                let plan = transition.expect("Todo → In Progress is permitted");
                // The auto-transition went through the FSM interpreter (the FIXED category proves it).
                assert_eq!(plan.to, AUTO_STATE_IN_PROGRESS);
                assert_eq!(plan.to_category, StateCategory::Started);
            }
            other => panic!("expected a Link effect, got {other:?}"),
        }
    }

    /// **A branch that the workflow does NOT permit to advance still LINKS (the no-bypass invariant).**
    /// If the issue is already in a state with no declared edge to In Progress, the link lands but the
    /// transition is `None` — the reflex never bypasses the FSM.
    #[test]
    fn branch_created_links_but_does_not_transition_when_not_permitted() {
        let wf = dev_workflow();
        let e = ev(
            GIT_BRANCH_CREATED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/branch/eng-1-fix",
            }),
        );
        // From "Done" there is no declared edge to In Progress → the FSM does not permit it.
        let effect = plan_branch_created(&e, &wf, "Done");
        match effect {
            ReflexEffect::Link { transition, .. } => {
                assert!(
                    transition.is_none(),
                    "no FSM edge → no auto-transition (no bypass)"
                );
            }
            other => panic!("expected a Link effect, got {other:?}"),
        }
    }

    /// A branch referencing NO issue is a no-op (not every branch advances an issue).
    #[test]
    fn branch_created_with_no_issue_ref_is_a_noop() {
        let wf = dev_workflow();
        let e = ev(
            GIT_BRANCH_CREATED,
            serde_json::json!({ "artifact_ref": "x" }),
        );
        assert_eq!(plan_branch_created(&e, &wf, "Todo"), ReflexEffect::NoOp);
    }

    // ---- C2: git.pr.opened/merged → closes link + CI-gated auto-close ----

    /// A `git.pr.opened` mints the `closes` edge but does NOT auto-transition (the issue closes on
    /// MERGE, not on open).
    #[test]
    fn pr_opened_links_closes_without_transition() {
        let e = ev(
            GIT_PR_OPENED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
            }),
        );
        match plan_pr_opened(&e) {
            ReflexEffect::Link {
                rel, transition, ..
            } => {
                assert_eq!(rel, IssueLifecycleRel::Closes);
                assert!(transition.is_none(), "opening a PR does not auto-close");
            }
            other => panic!("expected a Link, got {other:?}"),
        }
    }

    /// **A `git.pr.merged` with a TRUSTED green linked PR auto-closes the issue to Done (through the
    /// CI-gated FSM — never a bypass).** The `closes` edge lands AND the transition is the PERMITTED
    /// Done plan the CI-red guard approved (trusted success → the guard holds).
    #[test]
    fn pr_merged_trusted_green_auto_closes_through_the_ci_gated_fsm() {
        let wf = dev_workflow();
        let e = ev(
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "success",
                "trust_tier": "trusted",
            }),
        );
        let effects = plan_pr_merged(&e, &wf, AUTO_STATE_IN_PROGRESS);
        assert_eq!(
            effects.len(),
            1,
            "a permitted close is one Link effect (no block)"
        );
        match &effects[0] {
            ReflexEffect::Link {
                rel, transition, ..
            } => {
                assert_eq!(*rel, IssueLifecycleRel::Closes);
                let plan = transition
                    .as_ref()
                    .expect("a trusted green merge auto-closes");
                assert_eq!(plan.to, AUTO_STATE_DONE);
                assert_eq!(plan.to_category, StateCategory::Completed);
            }
            other => panic!("expected a Link, got {other:?}"),
        }
    }

    /// **A `git.pr.merged` with a CI-RED linked PR LINKS but the auto-close is BLOCKED (the
    /// no-governance-bypass artifact).** The `closes` edge still lands; the auto-transition is blocked
    /// by the CI-red Done guard (a failure check) → a loud `TransitionBlocked` effect, never a silent
    /// allow. The merge cannot green a Done the human path could not.
    #[test]
    fn pr_merged_ci_red_links_but_blocks_the_auto_close() {
        let wf = dev_workflow();
        let e = ev(
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "failure",
                "trust_tier": "trusted",
            }),
        );
        let effects = plan_pr_merged(&e, &wf, AUTO_STATE_IN_PROGRESS);
        assert_eq!(
            effects.len(),
            2,
            "a blocked close = the link + the loud block"
        );
        // the link lands with NO transition
        match &effects[0] {
            ReflexEffect::Link { transition, .. } => {
                assert!(transition.is_none(), "a CI-red merge does NOT auto-close");
            }
            other => panic!("expected a Link, got {other:?}"),
        }
        // the block is loud
        match &effects[1] {
            ReflexEffect::TransitionBlocked { reason, .. } => {
                assert!(
                    reason.contains("CI is not green"),
                    "the block names the guard: {reason}"
                );
            }
            other => panic!("expected a TransitionBlocked, got {other:?}"),
        }
    }

    /// **An un-endorsed untrusted-fork merge is NEUTRAL → the auto-close is BLOCKED (the poisoned-Done
    /// defence reaches the reflex).** A fork success Issues did not endorse cannot turn its own Done
    /// green — the reflex blocks the auto-close exactly as the human path does (one trust rule).
    #[test]
    fn pr_merged_unendorsed_fork_success_blocks_the_auto_close() {
        let wf = dev_workflow();
        let e = ev(
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "success",
                "trust_tier": "untrusted_fork",
                "endorsed": false,
            }),
        );
        let effects = plan_pr_merged(&e, &wf, AUTO_STATE_IN_PROGRESS);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, ReflexEffect::TransitionBlocked { .. })),
            "an un-endorsed fork success blocks the auto-close (poisoned-Done defence)"
        );
    }

    /// **A `git.pr.merged` with NO readable check posture FAILS CLOSED (does not auto-close).** A merge
    /// whose linked-PR posture Issues cannot read off the fact is treated as a non-green check → the
    /// Done guard blocks. Fail-closed, never a silent auto-close on missing data.
    #[test]
    fn pr_merged_with_no_check_posture_fails_closed() {
        let wf = dev_workflow();
        let e = ev(
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
            }),
        );
        let effects = plan_pr_merged(&e, &wf, AUTO_STATE_IN_PROGRESS);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, ReflexEffect::TransitionBlocked { .. })),
            "a merge with no readable check posture fails closed (no auto-close)"
        );
    }

    // ---- 5.9: ci.check.updated → guard feed (read off the fact, never recompute) ----

    /// **A `ci.check.updated` feeds the guard with the posture read OFF the fact (5.9 — never
    /// recomputed).** The `{state, trust_tier, endorsed}` tokens are carried verbatim into the
    /// `LinkedPrCheck` the next Done transition's guard reads.
    #[test]
    fn check_updated_feeds_the_guard_off_the_fact() {
        let e = ev(
            CI_CHECK_UPDATED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "state": "success",
                "trust_tier": "untrusted_fork",
                "endorsed": true,
            }),
        );
        match plan_check_updated(&e) {
            ReflexEffect::GuardFeed { issue, check } => {
                assert_eq!(issue, "myelin://acme/issue/issue/ENG-1");
                // the tier is carried VERBATIM (never recomputed) and the endorsement is read off the fact
                assert_eq!(check.trust_tier, "untrusted_fork");
                assert!(check.endorsed);
                assert!(
                    check.is_acceptable(),
                    "an endorsed fork success is acceptable"
                );
            }
            other => panic!("expected a GuardFeed, got {other:?}"),
        }
    }

    // ---- C3: chat.message.created → create issue + relates edge ----

    /// **A chat "create issue" message creates an issue with a `relates` edge back to the message.**
    #[test]
    fn chat_create_issue_message_creates_an_issue_with_a_relates_edge() {
        let e = ev(
            CHAT_MESSAGE_CREATED,
            serde_json::json!({
                "create_issue": true,
                "message_ref": "myelin://acme/chat/message/m-7",
                "title": "Investigate the flaky test",
            }),
        );
        match plan_chat_message_created(&e) {
            ReflexEffect::CreateIssueFromChat {
                source_message,
                title,
            } => {
                assert_eq!(source_message, "myelin://acme/chat/message/m-7");
                assert_eq!(title, "Investigate the flaky test");
            }
            other => panic!("expected CreateIssueFromChat, got {other:?}"),
        }
    }

    /// A chat message that is NOT a "create issue" command is a no-op (not every message creates an
    /// issue).
    #[test]
    fn chat_non_create_issue_message_is_a_noop() {
        let e = ev(
            CHAT_MESSAGE_CREATED,
            serde_json::json!({ "message_ref": "myelin://acme/chat/message/m-7" }),
        );
        assert_eq!(plan_chat_message_created(&e), ReflexEffect::NoOp);
    }

    // ---- C4: identity.member.* → reassign/anonymise ----

    /// **An `identity.member.erased` anonymises the actor across history (the §7 erasure lever).** The
    /// reflex stages an AnonymiseActor with `is_erasure = true` carrying the frozen pseudonym handle
    /// (never a raw id).
    #[test]
    fn member_erased_anonymises_the_actor() {
        let e = ev(
            IDENTITY_MEMBER_ERASED,
            serde_json::json!({ "actor_pseudonym": "8a2f@acme.noreply" }),
        );
        match plan_member_event(&e, true) {
            ReflexEffect::AnonymiseActor {
                actor_pseudonym,
                is_erasure,
            } => {
                assert_eq!(actor_pseudonym, "8a2f@acme.noreply");
                assert!(is_erasure, "an erased member is the §7 erasure lever");
            }
            other => panic!("expected AnonymiseActor, got {other:?}"),
        }
    }

    /// An `identity.member.deactivated` is a reassign (not an erasure): the handle persists, only open
    /// work is reassigned.
    #[test]
    fn member_deactivated_reassigns_without_erasure() {
        let e = ev(
            IDENTITY_MEMBER_DEACTIVATED,
            serde_json::json!({ "actor_pseudonym": "8a2f@acme.noreply" }),
        );
        match plan_member_event(&e, false) {
            ReflexEffect::AnonymiseActor { is_erasure, .. } => {
                assert!(!is_erasure, "a deactivate is a reassign, not an erasure");
            }
            other => panic!("expected AnonymiseActor, got {other:?}"),
        }
    }

    // ---- the consumer: idempotent on event_id (0 duplicate on replay) ----

    /// **GATE: a replayed `git.pr.merged` produces 0 duplicate staged effects (contract 2.4
    /// idempotent on `event_id`).** The consumer handles the merge once (staging the `closes` link +
    /// the auto-close); a redelivery of the SAME `event_id` is a no-op — the staged effect count is
    /// UNCHANGED. This is the ISS-P28 0-duplicate-on-replay green artifact.
    #[test]
    fn replayed_merge_produces_zero_duplicate_effects() {
        let consumer = ReflexConsumer::new(dev_workflow());
        consumer.set_state("myelin://acme/issue/issue/ENG-1", AUTO_STATE_IN_PROGRESS);
        let e = ev_with_id(
            "merge-1",
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "success",
                "trust_tier": "trusted",
            }),
        );
        assert_eq!(consumer.handle(&e), HandleOutcome::Done);
        let after_first = consumer.staged_count();
        assert_eq!(after_first, 1, "the merge staged one Link (the auto-close)");
        // REPLAY the same event_id: 0 duplicate (the within-handler dedup absorbs it).
        assert_eq!(consumer.handle(&e), HandleOutcome::Done);
        assert_eq!(
            consumer.staged_count(),
            after_first,
            "a replayed merge produces 0 duplicate staged effects (idempotent on event_id)"
        );
    }

    /// **GATE: a chained git.pr.merged → link + auto-transition → replay → 0 duplicate (the e2e
    /// chained-mutation property, ISS-P28).** A branch advances the issue, a merge closes it; replaying
    /// BOTH produces 0 duplicate links/transitions. The auto-transitions ran through the FSM
    /// interpreter (no bypass) and the dedup absorbed the replay.
    #[test]
    fn chained_branch_then_merge_is_idempotent_on_replay() {
        let consumer = ReflexConsumer::new(dev_workflow());
        let issue = "myelin://acme/issue/issue/ENG-1";
        // a branch advances Todo → In Progress
        consumer.set_state(issue, "Todo");
        let branch = ev_with_id(
            "branch-1",
            GIT_BRANCH_CREATED,
            serde_json::json!({ "issue_ref": issue, "artifact_ref": "myelin://acme/git/branch/b" }),
        );
        consumer.handle(&branch);
        // (a real consumer would advance the projection; the drill seeds it to In Progress)
        consumer.set_state(issue, AUTO_STATE_IN_PROGRESS);
        let merge = ev_with_id(
            "merge-1",
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": issue,
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "success",
                "trust_tier": "trusted",
            }),
        );
        consumer.handle(&merge);
        let after = consumer.staged_count();
        assert_eq!(
            after, 2,
            "the branch link + the merge close are two staged effects"
        );
        // REPLAY both: 0 duplicate.
        consumer.handle(&branch);
        consumer.handle(&merge);
        assert_eq!(
            consumer.staged_count(),
            after,
            "replaying the chain produces 0 duplicate staged effects"
        );
        // and the auto-transitions went through the FSM (the staged Links carry the FIXED categories)
        let staged = consumer.staged();
        let advanced = staged.iter().any(|e| {
            matches!(
                e,
                ReflexEffect::Link { transition: Some(p), .. } if p.to == AUTO_STATE_IN_PROGRESS
            )
        });
        let closed = staged.iter().any(|e| {
            matches!(
                e,
                ReflexEffect::Link { transition: Some(p), .. } if p.to == AUTO_STATE_DONE
            )
        });
        assert!(advanced, "the branch auto-advanced through the FSM");
        assert!(closed, "the merge auto-closed through the CI-gated FSM");
    }

    /// **The consumer NEVER bypasses the workflow guard (the 0-governance-bypass artifact).** A merge
    /// with a CI-red linked PR stages the `closes` link but the auto-close is BLOCKED (the guard ran
    /// through the FSM interpreter) — the consumer surfaces a `TransitionBlocked`, never a Done.
    #[test]
    fn consumer_never_bypasses_the_workflow_guard() {
        let consumer = ReflexConsumer::new(dev_workflow());
        let issue = "myelin://acme/issue/issue/ENG-1";
        consumer.set_state(issue, AUTO_STATE_IN_PROGRESS);
        let merge = ev_with_id(
            "merge-red",
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": issue,
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "failure",
                "trust_tier": "trusted",
            }),
        );
        consumer.handle(&merge);
        let staged = consumer.staged();
        // the link landed
        assert!(staged.iter().any(|e| matches!(
            e,
            ReflexEffect::Link {
                rel: IssueLifecycleRel::Closes,
                transition: None,
                ..
            }
        )));
        // but the auto-close was BLOCKED (no Done transition was staged — the guard ran through the FSM)
        assert!(staged
            .iter()
            .any(|e| matches!(e, ReflexEffect::TransitionBlocked { .. })));
        assert!(
            !staged.iter().any(|e| matches!(
                e,
                ReflexEffect::Link { transition: Some(p), .. } if p.to == AUTO_STATE_DONE
            )),
            "0 governance bypass: a CI-red merge never auto-closes to Done"
        );
    }

    /// A redelivered event to the consumer is `Done` (acked) and the handler runs the body once — the
    /// `plan_reflex` dispatch covers every whitelisted type (the full routing table).
    #[test]
    fn plan_reflex_routes_every_whitelisted_type() {
        let wf = dev_workflow();
        for &subj in REFLEX_SUBJECTS {
            let payload = serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "x",
                "actor_pseudonym": "8a2f@acme.noreply",
                "create_issue": true,
                "message_ref": "m",
                "title": "t",
                "state": "success",
                "trust_tier": "trusted",
            });
            let e = ev(subj, payload);
            let effects = plan_reflex(&e, &wf, "Todo");
            assert!(
                !effects.is_empty(),
                "every type routes to ≥1 effect: {subj}"
            );
            assert!(
                !effects.iter().all(|e| *e == ReflexEffect::NoOp),
                "a well-formed `{subj}` event is not a no-op"
            );
        }
    }
}
