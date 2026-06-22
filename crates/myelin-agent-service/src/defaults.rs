//! # `defaults` — the FROZEN per-subsystem `requires_approval` defaults seed (the §6.3 table)
//! + the no-silent-loosening guard (AG-P8 → P-220, M2-B)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §6.3 (**the FROZEN
//! `requires_approval` defaults table** — CI deploy/secret = yes; Git merge = yes, open_pr = no;
//! Issues forecast/triage/sla_draft = no, SLA transition = caveat-gated; KN publish/confidential =
//! yes, draft/comment = no; Chat post/react = no; **a cross-subsystem effect inherits the TARGET
//! subsystem's default** — "governed where it lands"), §5.2 step 6 (the HITL gate reads the §6.3
//! defaults), §6 (the one registry the seed populates). Reconciliation §X-6 (the defaults frozen).
//!
//! **Contract-index:** OWNS the *seed values* of row 8.1's `requires_approval` column (the COLUMN
//! itself is AG-P1 → P-130; the per-subsystem DEFAULTS are seeded HERE). The VISION §3 rule binds it:
//! suggest-by-default, human-confirm consequential/irreversible actions; **a subsystem may TIGHTEN a
//! default (mark more tools gated) but may NOT loosen a `yes → no` for a consequential action without
//! a WRITTEN DEVIATION** (the no-silent-loosening guard, [`assert_no_silent_loosening`]).
//!
//! ## What this prompt (AG-P8) ships — the seed + the guard
//! - [`requires_approval_default`] — the FROZEN §6.3 lookup: `(subsystem, tool) → bool`. The single
//!   source of truth a [`ToolDef`](myelin_agent::ToolDef) registration seeds its `requires_approval`
//!   column from. The cross-subsystem rule (a Chat-invoked effect that mutates ANOTHER subsystem
//!   inherits THAT subsystem's default) is [`requires_approval_for_landing`] — "governed where it
//!   lands, not where it's invoked" (§6.3 last row).
//! - [`seed_requires_approval`] — stamps the frozen default onto a [`ToolDef`] at registration (the
//!   seam every subsystem's `register_tool` calls; the column is no longer hand-set).
//! - [`assert_no_silent_loosening`] — the VISION §3 guard: a registration that LOOSENS a frozen
//!   `yes → no` for a consequential action WITHOUT a [`WrittenDeviation`] is REJECTED (loud, never
//!   silent). Tightening (`no → yes`) is always allowed.
//!
//! ## The frozen consequential set (the `yes` half of §6.3 — the loosen-guarded actions)
//! These are the consequential/irreversible actions the table marks `requires_approval = yes`. A
//! registration may not flip ANY of them to `no` without a written deviation (GDPR Art. 22;
//! ADR-08.6 suggest-by-default):
//! - CI `deploy` to a protected env, `approve_deploy`, `write_secret`.
//! - Git `git.merge` (the consequential gate, AG-8).
//! - Knowledge `publish`, `edit_confidential` (the confidential-page edit).
//! - Issues `transition` on an SLA-bound issue with an approver edge (the ABAC-gated transition).
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **The external MCP `exposed_over_mcp` column exists from AG-P1; the external MCP ENDPOINT (its
//!   auth, the agent-lane rate-limit) is the post-M5 follow-on named in AG-P25.** This module seeds
//!   `requires_approval`, not the MCP exposure path.
//! - **No other floor.** The seed is the frozen table; the guard is the live VISION §3 rule.

use myelin_agent::{ToolDef, ToolName};

// ───────────────────────── the frozen §6.3 default (the seed source of truth) ────────────────────

/// **The FROZEN §6.3 `requires_approval` default for a `(subsystem, tool)` pair.** The single source
/// of truth the [`ToolDef`] registration seeds from (the COLUMN is AG-P1; the VALUE is HERE). The
/// match arms ARE the §6.3 table, verbatim — gated-by-default for any consequential/irreversible
/// action (CI deploy/secret, Git merge, KN publish/confidential, an SLA-bound Issues transition);
/// suggest-by-default (no gate) for advisory/reversible actions (Issues forecast/triage/sla_draft,
/// Git open_pr, KN draft/comment, Chat post/react).
///
/// **The cross-subsystem rule is NOT here** — a Chat-invoked effect that mutates another subsystem
/// inherits THAT subsystem's default ([`requires_approval_for_landing`]). This function answers "what
/// is the default for a tool that LANDS in `subsystem`?" (the where-it-lands subsystem).
///
/// A `(subsystem, tool)` not named in the frozen table defaults to **gated (`true`)** — fail-closed:
/// an unrecognised consequential action is gated until the table is extended (a new gated action is
/// added HERE, never silently un-gated at runtime).
pub fn requires_approval_default(subsystem: &str, tool: &str) -> bool {
    match (subsystem, tool) {
        // ── CI (§6.3) ──
        ("ci", "deploy") => true,         // protected-env deploy is consequential
        ("ci", "approve_deploy") => true, // approval is privileged
        ("ci", "write_secret") => true,   // secret write is privileged
        ("ci", "run_pipeline") => false,  // non-prod: cheap, reversible, metered

        // ── Git (§6.3) ──
        ("git", "merge") => true,    // merge is the consequential gate (AG-8)
        ("git", "open_pr") => false, // reversible
        // The AGENT AUTHOR/REVIEWER surface (GIT-P28 → P-289, §7 frozen ToolDef table). Agents are
        // FIRST-CLASS authors/reviewers (legible, bounded): every one of these is REVERSIBLE/advisory
        // — a comment/review/suggestion/thread-resolution can be revised or dismissed — so the frozen
        // §6.3 default is NOT gated (suggest-by-default, VISION §3). The ONLY consequential git gate
        // stays `git.merge` (yes, above). Legibility (ADR-08 AI-Act: an agent author is never disguised
        // as human) is carried by `myelin_git::agent_author`, not by the approval gate.
        ("git", "comment") => false,        // inline/thread comment (agent legibly labelled)
        ("git", "submit_review") => false,  // approve / request-changes / comment review
        ("git", "suggest_change") => false, // a committable suggestion (reversible)
        ("git", "resolve_thread") => false, // resolve a review thread (reversible)
        // The CODE-EXECUTING git tools (GIT-P27 → P-283, §7). history_rewrite is the audited
        // erasure-admin op — it changes every downstream hash (consequential, irreversible), so it is
        // GATED (VISION §3). scip_index is a read-only code-intelligence index build (a `compute`
        // artifact over readable bytes) — reversible/advisory, so NOT gated (it is governed by the
        // routing split + the AG-D4 escape gate on the git tool image, not by HITL).
        ("git", "history_rewrite") => true,
        ("git", "scip_index") => false,

        // ── Issues (§6.3) ──
        ("issues", "forecast") => false,  // advisory (suggest)
        ("issues", "triage") => false,    // advisory (suggest)
        ("issues", "sla_draft") => false, // advisory (suggest)
        // an SLA-bound transition with an approver edge is caveat-gated (ABAC, §5.2 step 2). The
        // STATIC default is `true` (gated) — the ABAC caveat is the refinement, never a loosening:
        // a transition WITHOUT an approver edge is admitted by the caveat at check-time, but the
        // tool_def default is gated so the gate is the conservative floor.
        ("issues", "transition") => true,

        // ── Knowledge (§6.3) ──
        ("knowledge", "publish") => true,          // publishing is consequential (approver set)
        ("knowledge", "edit_confidential") => true, // confidential edit is consequential
        ("knowledge", "draft") => false,           // reversible
        ("knowledge", "comment") => false,         // reversible

        // ── Chat (§6.3) ──
        ("chat", "post_message") => false, // reversible, cheap
        ("chat", "react") => false,        // reversible, cheap

        // Fail-closed: an unrecognised action LANDING in a subsystem is gated by default until the
        // frozen table is extended HERE (a new gated/un-gated action is a frozen-table edit, never a
        // runtime invention). This is the conservative floor (gated > un-gated for the unknown).
        _ => true,
    }
}

/// **The cross-subsystem rule (§6.3 last row) — "governed where it LANDS, not where it's invoked".**
/// A Chat-invoked `EffectApi` tool that mutates ANOTHER subsystem inherits the TARGET subsystem's
/// default. `invoking_subsystem` is where the tool was called from (e.g. `chat`); `landing_subsystem`
/// is where the MUTATION lands (e.g. `git`). The default is the LANDING subsystem's — a `chat`-invoked
/// `git.merge` is gated (Git's default), NOT un-gated (Chat's `post_message` default).
///
/// When the invoking and landing subsystems are the SAME, this is exactly
/// [`requires_approval_default`] for that subsystem.
pub fn requires_approval_for_landing(
    _invoking_subsystem: &str,
    landing_subsystem: &str,
    tool: &str,
) -> bool {
    // The effect is governed where it LANDS — the invoking subsystem is irrelevant to the default.
    requires_approval_default(landing_subsystem, tool)
}

// ───────────────────────── the seed seam (registration stamps the frozen default) ────────────────

/// **Stamp the FROZEN §6.3 default onto a [`ToolDef`] at registration (the seed seam).** The
/// registration path every subsystem calls: the `requires_approval` column is no longer hand-set —
/// it is SEEDED from the frozen table by `(subsystem, name)`. Returns the def with its
/// `requires_approval` set to the frozen default (idempotent — seeding an already-correctly-seeded
/// def is a no-op).
pub fn seed_requires_approval(mut def: ToolDef) -> ToolDef {
    def.requires_approval = requires_approval_default(&def.subsystem, &def.name.0);
    def
}

// ───────────────────────── the no-silent-loosening guard (VISION §3) ─────────────────────────────

/// **A WRITTEN DEVIATION authorising a `yes → no` loosening of a frozen consequential default
/// (VISION §3 / EI-01 §1).** A subsystem may TIGHTEN a default freely, but loosening a frozen `yes`
/// to `no` for a consequential action requires an EXPLICIT, recorded deviation — never a silent edit.
/// Carries the `(subsystem, tool)` it authorises + the human-readable rationale (the audit fact).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrittenDeviation {
    /// The subsystem the loosening lands in (the where-it-lands subsystem).
    pub subsystem: String,
    /// The tool whose frozen `yes` this deviation loosens to `no`.
    pub tool: String,
    /// The recorded rationale (the written justification VISION §3 requires).
    pub rationale: String,
}

impl WrittenDeviation {
    /// Build a written deviation authorising a `(subsystem, tool)` loosening with a `rationale`.
    pub fn new(
        subsystem: impl Into<String>,
        tool: impl Into<String>,
        rationale: impl Into<String>,
    ) -> WrittenDeviation {
        WrittenDeviation {
            subsystem: subsystem.into(),
            tool: tool.into(),
            rationale: rationale.into(),
        }
    }

    /// Whether this deviation authorises loosening `(subsystem, tool)`.
    fn authorises(&self, subsystem: &str, tool: &str) -> bool {
        self.subsystem == subsystem && self.tool == tool && !self.rationale.trim().is_empty()
    }
}

/// **A loosening that VIOLATES the VISION §3 rule (a frozen `yes → no` with NO written deviation).**
/// Surfaced LOUD (never swallowed) — the registration is REJECTED. Carries the `(subsystem, tool)`
/// the registration tried to silently un-gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LooseningViolation {
    /// The subsystem the silent loosening landed in.
    pub subsystem: String,
    /// The tool whose frozen `yes` the registration silently flipped to `no`.
    pub tool: String,
}

impl core::fmt::Display for LooseningViolation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "registration loosens the frozen requires_approval=yes default for {}.{} to no WITHOUT a \
             written deviation (VISION §3: a consequential action may not be silently un-gated)",
            self.subsystem, self.tool
        )
    }
}

impl std::error::Error for LooseningViolation {}

/// **The no-silent-loosening guard (VISION §3 / X-6 / EI-01 §1).** Asserts a [`ToolDef`]
/// registration's `requires_approval` does not silently LOOSEN the frozen §6.3 default:
/// - The frozen default is `yes` (gated, consequential) and the registration sets `no` (un-gated):
///   **REJECTED** unless a matching [`WrittenDeviation`] authorises it.
/// - The frozen default is `no` and the registration sets `yes` (TIGHTENING): **always allowed**
///   (a subsystem may always mark more tools gated per tenant policy, §6.3).
/// - The registration matches the frozen default: **allowed** (the seeded path).
///
/// `deviations` is the set of recorded written deviations in force (empty for the strict default).
/// Returns `Ok(())` when the registration is admissible, `Err(LooseningViolation)` (loud) otherwise.
pub fn assert_no_silent_loosening(
    def: &ToolDef,
    deviations: &[WrittenDeviation],
) -> Result<(), LooseningViolation> {
    let frozen = requires_approval_default(&def.subsystem, &def.name.0);
    // The only inadmissible move is a frozen `yes` loosened to `no` with no written deviation.
    if frozen && !def.requires_approval {
        let authorised = deviations
            .iter()
            .any(|d| d.authorises(&def.subsystem, &def.name.0));
        if !authorised {
            return Err(LooseningViolation {
                subsystem: def.subsystem.clone(),
                tool: def.name.0.clone(),
            });
        }
    }
    // frozen `no` → registration `yes` (tighten) and frozen == registration are both fine.
    Ok(())
}

/// **Resolve the tool-name half of the §6.3 key (the catalogue key is `(subsystem, name, version)`).**
/// A convenience the seed path uses when it holds a [`ToolName`] rather than a bare `&str`.
pub fn default_for_tool(subsystem: &str, name: &ToolName) -> bool {
    requires_approval_default(subsystem, &name.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::EffectKind;

    fn tool_def(subsystem: &str, name: &str, requires_approval: bool) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: subsystem.into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec![],
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            requires_approval,
            exposed_over_mcp: false,
        }
    }

    /// **The FROZEN §6.3 table is seeded VERBATIM — every named `(subsystem, tool)` matches the
    /// architecture table exactly (the seed assertion the GATE requires).** A change to any arm flips
    /// an assertion here (the table is frozen — an edit is a deliberate, reviewed change).
    #[test]
    fn the_frozen_6_3_defaults_table_is_seeded_verbatim() {
        // CI — deploy/secret/approval = yes; non-prod pipeline = no.
        assert!(requires_approval_default("ci", "deploy"), "CI deploy is gated (consequential)");
        assert!(requires_approval_default("ci", "approve_deploy"), "CI approve_deploy is gated");
        assert!(requires_approval_default("ci", "write_secret"), "CI write_secret is gated");
        assert!(!requires_approval_default("ci", "run_pipeline"), "CI non-prod pipeline is NOT gated");

        // Git — merge = yes, open_pr = no.
        assert!(requires_approval_default("git", "merge"), "git.merge is gated (AG-8)");
        assert!(!requires_approval_default("git", "open_pr"), "open_pr is reversible → NOT gated");
        // Git code-executing tools (GIT-P27 → P-283) — history_rewrite = yes (consequential erasure
        // -admin op), scip_index = no (read-only code-intelligence index build).
        assert!(
            requires_approval_default("git", "history_rewrite"),
            "history-rewrite is gated (changes every downstream hash — consequential)"
        );
        assert!(
            !requires_approval_default("git", "scip_index"),
            "SCIP indexing is a read-only index build → NOT gated (governed by AG-D4, not HITL)"
        );

        // Git agent author/reviewer surface (GIT-P28 → P-289) — every authoring tool is reversible →
        // NOT gated (suggest-by-default); only git.merge stays the consequential gate.
        assert!(!requires_approval_default("git", "comment"), "git.comment is reversible → NOT gated");
        assert!(
            !requires_approval_default("git", "submit_review"),
            "git.submit_review is reversible → NOT gated"
        );
        assert!(
            !requires_approval_default("git", "suggest_change"),
            "git.suggest_change is reversible → NOT gated"
        );
        assert!(
            !requires_approval_default("git", "resolve_thread"),
            "git.resolve_thread is reversible → NOT gated"
        );

        // Issues — forecast/triage/sla_draft = no (advisory); transition = gated (ABAC floor).
        assert!(!requires_approval_default("issues", "forecast"), "forecast is advisory → NOT gated");
        assert!(!requires_approval_default("issues", "triage"), "triage is advisory → NOT gated");
        assert!(!requires_approval_default("issues", "sla_draft"), "sla_draft is advisory → NOT gated");
        assert!(requires_approval_default("issues", "transition"), "SLA transition is caveat-gated (floor)");

        // Knowledge — publish/confidential = yes; draft/comment = no.
        assert!(requires_approval_default("knowledge", "publish"), "publish is gated (consequential)");
        assert!(requires_approval_default("knowledge", "edit_confidential"), "confidential edit is gated");
        assert!(!requires_approval_default("knowledge", "draft"), "draft is reversible → NOT gated");
        assert!(!requires_approval_default("knowledge", "comment"), "comment is reversible → NOT gated");

        // Chat — post/react = no.
        assert!(!requires_approval_default("chat", "post_message"), "post_message is reversible → NOT gated");
        assert!(!requires_approval_default("chat", "react"), "react is reversible → NOT gated");
    }

    /// **An unrecognised `(subsystem, tool)` is GATED by default (fail-closed — gated > un-gated for
    /// the unknown).** A new consequential action is added to the frozen table HERE, never silently
    /// un-gated at runtime.
    #[test]
    fn an_unknown_action_is_gated_fail_closed() {
        assert!(requires_approval_default("ci", "nuke_prod"), "an unknown action is gated (fail-closed)");
        assert!(requires_approval_default("brand_new_subsystem", "anything"), "unknown subsystem → gated");
    }

    /// **The cross-subsystem rule (§6.3 last row) — a Chat-invoked effect inherits the LANDING
    /// subsystem's default ("governed where it lands").** A `chat`-invoked `git.merge` is gated
    /// (Git's `yes`), NOT un-gated (Chat's `post_message` `no`).
    #[test]
    fn cross_subsystem_effect_inherits_the_landing_subsystems_default() {
        // chat → git.merge LANDS in git → Git's gated default (NOT Chat's un-gated post default).
        assert!(
            requires_approval_for_landing("chat", "git", "merge"),
            "a chat-invoked git.merge is governed where it LANDS (git → gated)"
        );
        // chat → issues.forecast LANDS in issues → Issues' advisory (NOT gated) default.
        assert!(
            !requires_approval_for_landing("chat", "issues", "forecast"),
            "a chat-invoked issues.forecast lands in issues → advisory (NOT gated)"
        );
        // same-subsystem: invoking == landing → exactly requires_approval_default.
        assert_eq!(
            requires_approval_for_landing("git", "git", "merge"),
            requires_approval_default("git", "merge"),
            "invoking == landing collapses to the plain default"
        );
    }

    /// **`seed_requires_approval` stamps the frozen default (the column is no longer hand-set).** A
    /// def registered with the WRONG `requires_approval` is corrected to the frozen value; an
    /// already-correct def is unchanged (idempotent).
    #[test]
    fn seed_stamps_the_frozen_default_onto_the_tool_def() {
        // a git.merge def registered (wrongly) as un-gated is SEEDED back to gated.
        let wrong = tool_def("git", "merge", /* requires_approval */ false);
        let seeded = seed_requires_approval(wrong);
        assert!(seeded.requires_approval, "git.merge is seeded gated regardless of the input value");

        // an open_pr def registered (wrongly) as gated is SEEDED back to NOT gated.
        let wrong_pr = tool_def("git", "open_pr", true);
        let seeded_pr = seed_requires_approval(wrong_pr);
        assert!(!seeded_pr.requires_approval, "open_pr is seeded NOT gated");

        // idempotent: seeding an already-correct def is a no-op.
        let already = tool_def("git", "merge", true);
        assert_eq!(seed_requires_approval(already.clone()), seed_requires_approval(already));
    }

    /// **The GATE fixture — a registration that LOOSENS a frozen `yes → no` WITHOUT a written
    /// deviation is REJECTED (VISION §3).** `git.merge` set to NOT gated, no deviation → loud
    /// violation; the SAME registration WITH a matching written deviation → admitted.
    #[test]
    fn loosening_a_frozen_yes_without_a_deviation_is_rejected() {
        // git.merge (frozen yes) registered as NOT gated, NO deviation → REJECTED (loud).
        let loosened = tool_def("git", "merge", /* requires_approval */ false);
        let err = assert_no_silent_loosening(&loosened, &[]).unwrap_err();
        assert_eq!(err.subsystem, "git");
        assert_eq!(err.tool, "merge");
        assert!(
            err.to_string().contains("WITHOUT a written deviation"),
            "the violation is surfaced LOUD: {err}"
        );

        // the SAME registration WITH a matching written deviation → admitted.
        let dev = WrittenDeviation::new("git", "merge", "tenant policy: auto-merge bot, audited");
        assert!(
            assert_no_silent_loosening(&loosened, std::slice::from_ref(&dev)).is_ok(),
            "a written deviation authorises the loosening"
        );

        // a deviation for a DIFFERENT tool does NOT authorise this loosening.
        let other = WrittenDeviation::new("ci", "deploy", "unrelated");
        assert!(
            assert_no_silent_loosening(&loosened, &[other]).is_err(),
            "a deviation for another tool does not authorise this one"
        );

        // a deviation with an EMPTY rationale is not a real deviation → still rejected.
        let empty = WrittenDeviation::new("git", "merge", "   ");
        assert!(
            assert_no_silent_loosening(&loosened, &[empty]).is_err(),
            "an empty-rationale deviation is not a real written deviation"
        );
    }

    /// **TIGHTENING (a frozen `no → yes`) is ALWAYS allowed (no deviation needed, §6.3).** A
    /// subsystem may mark more tools gated per tenant policy; only LOOSENING is guarded.
    #[test]
    fn tightening_a_frozen_no_is_always_allowed() {
        // open_pr (frozen no) registered as gated (tighten) → admitted with no deviation.
        let tightened = tool_def("git", "open_pr", /* requires_approval */ true);
        assert!(
            assert_no_silent_loosening(&tightened, &[]).is_ok(),
            "tightening (no → yes) needs no deviation"
        );
        // chat post_message (frozen no) tightened to gated → admitted.
        let chat_tight = tool_def("chat", "post_message", true);
        assert!(assert_no_silent_loosening(&chat_tight, &[]).is_ok());
    }

    /// **A registration that MATCHES the frozen default is admitted (the seeded path is always
    /// admissible).** Both the gated and un-gated frozen tools, registered at their frozen value,
    /// pass.
    #[test]
    fn a_registration_matching_the_frozen_default_is_admitted() {
        let gated = tool_def("git", "merge", true); // frozen yes, registered yes.
        assert!(assert_no_silent_loosening(&gated, &[]).is_ok());
        let ungated = tool_def("git", "open_pr", false); // frozen no, registered no.
        assert!(assert_no_silent_loosening(&ungated, &[]).is_ok());
    }

    /// **`default_for_tool` is the `ToolName`-keyed convenience (same answer as the `&str` form).**
    #[test]
    fn default_for_tool_matches_the_str_form() {
        assert_eq!(
            default_for_tool("git", &ToolName("merge".into())),
            requires_approval_default("git", "merge")
        );
    }
}
