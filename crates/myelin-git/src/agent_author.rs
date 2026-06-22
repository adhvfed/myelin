//! # `agent_author` — agents as **first-class, legible, bounded** authors/reviewers
//! (GIT-P28 / P-289, M3-G6)
//!
//! An agent can **author** into git's PR surface — open a PR, comment, submit a review, suggest a
//! committable change, resolve a thread — exactly the way a human principal can, **through the ONE
//! plan-then-apply `EffectApi`** (contract 8.2), governed by the SAME branch protection + caps as any
//! principal (8.4). The agent is FIRST-CLASS (not a second-class bot lane) but BOUNDED (every effect
//! rides the eight-step pipeline; nothing reaches a subsystem store except the public endpoint) and
//! LEGIBLE (every agent-authored artifact is rendered visually distinct with provenance — which
//! agent, why, which run — and is **never disguised as a human**, ADR-08 / EU AI-Act).
//!
//! **The ONLY consequential git gate is `git.merge`** (`requires_approval = yes`, §6.3 / AG-8):
//! authoring is reversible/advisory (a comment/review/suggestion can be revised or dismissed), so the
//! author/reviewer tools are NOT HITL-gated; `git.merge` WITHHOLDS until a human approves. This module
//! owns the GIT-domain half (the legibility value + the tool identity constants + the `required_caps`
//! built from the frozen Git ReBAC fragment); the thin Fabric `ToolDef` registration lives in
//! `myelin_agent_service::git_tools` (the §2.9 DAG — `myelin-git` is a LEAF, it does not depend on
//! `myelin-agent`). The OP bodies (open/comment/review) are the GIT-P16 lifecycle
//! ([`crate::lifecycle`]); this module is the agent-authoring identity + legibility seam over them.
//!
//! **Owning architecture docs (read in full before editing):**
//! - `planning/04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md`
//!   §7 (the frozen `ToolDef` table — `git.open_pr`/`comment`/`submit_review`/`suggest_change`/
//!   `resolve_thread` = `mutate`, `requires_approval = no`; `git.merge` = gated; **agents as
//!   authors/reviewers via `EffectApi`**; "agent authors/reviewers render visually distinct with
//!   provenance and are never disguised as humans — `review.is_agent` + `agent_run` carry this").
//! - `00-reconciliation-decisions.md` X-6 (the `requires_approval` defaults + the four uniform
//!   guarantees; the effect-intersection denial `agent.policy ∩ delegation ∩ tenant.policy`).
//! - VISION §3 (agent-native; suggest-by-default; consequential/irreversible actions human-confirmed
//!   — `git.merge` is the consequential gate, authoring is reversible).
//!
//! **Contract-index rows:**
//! - **8.1** (the Git slice OWNED here as the tool IDENTITY constants + `required_caps`; the
//!   registration row lives in the Fabric) — the agent author/reviewer ToolDefs.
//! - **8.2** (CONSUMED) — every authored effect routes through `EffectApi::apply` (plan-then-apply);
//!   agents NEVER write directly (the apply pipeline is the Fabric's, AG-P6).
//! - **8.4** (CONSUMED) — agents are subject to the same governance as any principal (branch
//!   protection, caps); no carve-out.
//! - **4.7** (CONSUMED) — the run executes under a per-run attenuated token (`mint_run_token`).
//! - **11.7** (CONSUMED) — reserve/settle fronts the run (an authoring effect is metered like any).
//!
//! ## Why the legibility lives here (not in the Fabric)
//! Legibility is a GIT-domain rendering fact: an agent-authored PR/review/comment carries the
//! `is_agent` flag the git lifecycle ([`crate::lifecycle::Review::is_agent`]) already declares, plus
//! the provenance triple (which agent, the run, the rationale) that git's Web UI renders distinctly.
//! This module is the typed [`AgentAuthorship`] value that the public endpoint stamps onto an
//! agent-authored artifact — the structural guarantee that an agent author is NEVER rendered as a
//! human (the `is_agent` bit is REQUIRED, not optional, on an agent-authored row).
//!
//! ## DB-free
//! In-memory identity constants + the typed legibility value + cap construction from the frozen
//! fragment. No DB. `cargo build --workspace` stays DB-free.

// ───────────────────────── the agent author/reviewer tool identity (the §7 catalogue keys) ────────

/// **The `git.comment` tool name** (§7 — an inline/thread comment, agent legibly labelled). A
/// `mutate` tool → `EffectApi::apply`; reversible (a comment can be edited/deleted) → NOT gated.
pub const COMMENT_TOOL: &str = "comment";

/// **The `git.submit_review` tool name** (§7 — approve / request-changes / comment review). A
/// `mutate` tool → `EffectApi::apply`; reversible (a review can be dismissed/revised) → NOT gated.
/// The agent reviewer's verdict rides `git.review.submitted` with `is_agent = true` (legibility).
pub const SUBMIT_REVIEW_TOOL: &str = "submit_review";

/// **The `git.suggest_change` tool name** (§7 — a committable suggestion). A `mutate` tool →
/// `EffectApi::apply`; reversible (a suggestion is applied or dismissed by a human) → NOT gated.
pub const SUGGEST_CHANGE_TOOL: &str = "suggest_change";

/// **The `git.resolve_thread` tool name** (§7 — resolve a review thread). A `mutate` tool →
/// `EffectApi::apply`; reversible (a thread can be re-opened) → NOT gated.
pub const RESOLVE_THREAD_TOOL: &str = "resolve_thread";

/// **The agent author/reviewer `ToolDef` version** (forward-only; the catalogue key is
/// `(subsystem, name, version)`). v1, aligned with the `merge`/`open_pr` producer tools (P-267).
pub const GIT_AUTHOR_TOOL_VERSION: u32 = 1;

/// **The full agent author/reviewer tool-name set, in catalogue order** (the legible, bounded
/// authoring surface — `open_pr` lives in the producer-mutation set, P-267; this is the comment /
/// review / suggest / resolve quartet GIT-P28 adds). A closed set so a new authoring tool can NOT be
/// added without a `required_caps` + §6.3-default decision (the routing is total).
pub const AUTHOR_TOOLS: [&str; 4] = [
    COMMENT_TOOL,
    SUBMIT_REVIEW_TOOL,
    SUGGEST_CHANGE_TOOL,
    RESOLVE_THREAD_TOOL,
];

// ───────────────────────── the required_caps from the frozen Git ReBAC fragment (4.9) ─────────────

/// **The `required_caps` for the agent **review/comment** authoring tools (CONSUMED from 4.9).** An
/// agent that comments, submits a review, suggests a change, or resolves a thread is WRITING to the
/// PR review surface — governed by the `pull_request.review` permission Git's frozen ReBAC fragment
/// declares ([`pull_request_fragment`](crate::rebac_fragment::pull_request_fragment): `review =
/// reviewer + parent_repo->push`). The cap STRING is `"<object_type>.<permission>"` (the EffectApi
/// `check` step resolves it). Built from the canonical `myelin-git` constants so a fragment rename is
/// a compile/test break here, never a silent drift. (We do NOT invent a new permission — the agent
/// reviewer is governed by the SAME `review` permission a human reviewer is, EI-02 §2: an agent can
/// do nothing no human role can.)
pub fn review_authoring_required_caps() -> Vec<String> {
    vec![format!(
        "{}.review",
        crate::rebac_fragment::object_types::PULL_REQUEST
    )]
}

// ───────────────────────── agent legibility (ADR-08 / AI-Act — never disguised as human) ──────────

/// **The legibility provenance an agent-authored git artifact MUST carry (ADR-08 / EU AI-Act).** An
/// agent author/reviewer is rendered visually distinct with provenance — *which agent, why, which
/// run* — and is **never disguised as a human**. This is the typed value the public endpoint stamps
/// onto an agent-authored PR/review/comment row; its presence is the structural guarantee that the
/// `is_agent` bit is set (a human-authored artifact carries [`Authorship::Human`] instead — the two
/// are a CLOSED enum, so an authored artifact is unambiguously one or the other).
///
/// PII-free: the agent is named by its OPAQUE pseudonym (GIT-1, contract 4.8 — never a raw identity),
/// the run by its opaque run id, the rationale by an agent-authored summary (the "why" the UI shows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentAuthorship {
    /// The authoring agent's OPAQUE pseudonym (GIT-1 / 4.8) — never a raw name/email. Rendered as the
    /// distinct agent author label (with the agent glyph), never as a human name.
    pub agent_pseudonym: String,
    /// The opaque run id this authoring effect belongs to (`agent_run` — the provenance link the UI
    /// surfaces so a human can see WHICH run authored this, and trace/replay it).
    pub run_id: String,
    /// The agent-authored rationale (the "why" — a short human-readable summary the UI shows next to
    /// the agent-authored artifact). Agent-authored free text; PII handled by the ONE erasure posture.
    pub rationale: String,
}

impl AgentAuthorship {
    /// Build an agent authorship provenance from the opaque agent pseudonym + run id + rationale.
    pub fn new(
        agent_pseudonym: impl Into<String>,
        run_id: impl Into<String>,
        rationale: impl Into<String>,
    ) -> AgentAuthorship {
        AgentAuthorship {
            agent_pseudonym: agent_pseudonym.into(),
            run_id: run_id.into(),
            rationale: rationale.into(),
        }
    }
}

/// **Who authored a git artifact — a CLOSED two-arm enum (human XOR agent).** Every authored PR /
/// review / comment is exactly one. The agent arm CARRIES the [`AgentAuthorship`] provenance, so an
/// agent-authored artifact STRUCTURALLY cannot exist without its legibility metadata — there is no
/// way to author as an agent and omit the provenance (the AI-Act "never disguised as human"
/// guarantee is the type, not a runtime check that could be skipped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Authorship {
    /// A human principal authored this (rendered as the human's pseudonym, no agent glyph).
    Human {
        /// The human author's OPAQUE pseudonym (GIT-1 / 4.8).
        author_pseudonym: String,
    },
    /// An AGENT authored this (rendered visually distinct with provenance — never as a human).
    Agent(AgentAuthorship),
}

impl Authorship {
    /// **Whether this artifact is agent-authored** — the `is_agent` bit the git lifecycle
    /// ([`crate::lifecycle::Review::is_agent`]) and the Web UI read. `true` iff [`Authorship::Agent`].
    pub fn is_agent(&self) -> bool {
        matches!(self, Authorship::Agent(_))
    }

    /// **The provenance to render (legibility, ADR-08 / AI-Act)** — `Some` for an agent author (the UI
    /// MUST render it distinctly with the run + rationale), `None` for a human author. The presence of
    /// `Some` IS the "never disguised as human" guarantee: an agent author always surfaces its
    /// provenance.
    pub fn agent_provenance(&self) -> Option<&AgentAuthorship> {
        match self {
            Authorship::Agent(a) => Some(a),
            Authorship::Human { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ───────────────────────── the author tool identity (the registration keys) ─────────────────

    #[test]
    fn the_author_tool_identity_constants_are_the_frozen_keys() {
        assert_eq!(COMMENT_TOOL, "comment");
        assert_eq!(SUBMIT_REVIEW_TOOL, "submit_review");
        assert_eq!(SUGGEST_CHANGE_TOOL, "suggest_change");
        assert_eq!(RESOLVE_THREAD_TOOL, "resolve_thread");
        // the closed author-tool set is exactly the comment/review/suggest/resolve quartet.
        assert_eq!(AUTHOR_TOOLS.len(), 4);
        assert!(AUTHOR_TOOLS.contains(&COMMENT_TOOL));
        assert!(AUTHOR_TOOLS.contains(&SUBMIT_REVIEW_TOOL));
        assert!(AUTHOR_TOOLS.contains(&SUGGEST_CHANGE_TOOL));
        assert!(AUTHOR_TOOLS.contains(&RESOLVE_THREAD_TOOL));
    }

    /// **The author/reviewer `required_caps` come from the FROZEN Git ReBAC fragment (4.9), not
    /// invented here** — an agent reviewer is governed by the SAME `pull_request.review` permission a
    /// human reviewer is (EI-02 §2: an agent can do nothing no human role can). A fragment rename is a
    /// compile/test break here.
    #[test]
    fn review_authoring_caps_are_the_frozen_pull_request_review_permission() {
        assert_eq!(
            review_authoring_required_caps(),
            vec!["pull_request.review".to_string()]
        );
        // the object-type half IS the canonical Git ReBAC name (4.9), not a local string.
        assert_eq!(
            crate::rebac_fragment::object_types::PULL_REQUEST,
            "pull_request"
        );
    }

    /// **The frozen `pull_request` ReBAC fragment actually DECLARES the `review` permission the agent
    /// authoring caps consume (the provider half of the CDC, in-crate).** A drop/rename of `review`
    /// breaks both this and the cap construction above.
    #[test]
    fn the_pull_request_fragment_declares_the_review_permission() {
        let frag = crate::rebac_fragment::pull_request_fragment();
        assert!(
            frag.permissions.iter().any(|p| p.0 == "review"),
            "the Git `pull_request` fragment declares the `review` permission (4.9) the agent \
             reviewer/commenter is governed by"
        );
    }

    // ───────────────────────── agent legibility (ADR-08 / AI-Act) ────────────────────────────────

    /// **An agent-authored artifact STRUCTURALLY carries its provenance (never disguised as human).**
    /// The `Agent` arm carries the [`AgentAuthorship`] — there is no way to be agent-authored and omit
    /// the run/rationale; `is_agent()` is true and `agent_provenance()` is `Some`.
    #[test]
    fn an_agent_author_is_legible_with_provenance_never_disguised() {
        let authored = Authorship::Agent(AgentAuthorship::new(
            "psn:agent-7",
            "run:R1",
            "addresses the failing test in src/foo.rs",
        ));
        assert!(
            authored.is_agent(),
            "an agent author is legibly flagged (is_agent)"
        );
        let prov = authored
            .agent_provenance()
            .expect("agent provenance is REQUIRED (AI-Act)");
        assert_eq!(
            prov.agent_pseudonym, "psn:agent-7",
            "which agent (opaque pseudonym)"
        );
        assert_eq!(
            prov.run_id, "run:R1",
            "which run (the traceable provenance link)"
        );
        assert!(
            prov.rationale.contains("failing test"),
            "the why (the rendered rationale)"
        );
    }

    /// **A human-authored artifact is NOT flagged agent + carries NO agent provenance** — the closed
    /// enum makes human XOR agent unambiguous (no third "maybe agent" state to disguise an agent as).
    #[test]
    fn a_human_author_is_not_agent_and_has_no_agent_provenance() {
        let human = Authorship::Human {
            author_pseudonym: "psn:human-x".into(),
        };
        assert!(!human.is_agent(), "a human author is NOT flagged agent");
        assert!(
            human.agent_provenance().is_none(),
            "a human author carries no agent provenance"
        );
    }

    /// **Legibility ties to the git lifecycle `is_agent` bit** — an agent-submitted review rides
    /// `git.review.submitted` with `is_agent = true` (§7); a human review with `false`. The
    /// [`Authorship`] value and the lifecycle flag agree (one source of truth for "is this an agent").
    #[test]
    fn authorship_is_agent_drives_the_lifecycle_review_is_agent_flag() {
        let agent = Authorship::Agent(AgentAuthorship::new("psn:agent-7", "run:R1", "lgtm"));
        let human = Authorship::Human {
            author_pseudonym: "psn:human-x".into(),
        };
        // a review request stamped from the authorship carries the matching is_agent bit.
        let agent_review = crate::lifecycle::Review::request("psn:agent-7", agent.is_agent());
        let human_review = crate::lifecycle::Review::request("psn:human-x", human.is_agent());
        assert!(
            agent_review.is_agent,
            "an agent reviewer rides is_agent = true (legibility)"
        );
        assert!(
            !human_review.is_agent,
            "a human reviewer rides is_agent = false"
        );
    }
}
