//! # `git_tools` — the per-producer **Git** ToolDefs registered into the ONE ToolSurface
//! (AG-P18 → P-267, M3): `git.merge` (gated) + `open_pr` (reversible)
//!
//! **GIT-P27 → P-283 (M3-G6) extends this registration site** with the two **code-executing** Git
//! tools that ride the ONE unified sandbox (§7): `git.history_rewrite` (a `Mutate` →
//! `EffectApi::apply`, **gated** by the fail-closed §6.3 default — the audited erasure-admin op,
//! 10.6) and `git.scip_index` (a `Compute` tool → `ToolHands::exec` = the CI `kind=agent` job the
//! AG-D4 escape drill gates). Both inherit the FOUR uniform sandbox guarantees BY CONSTRUCTION; their
//! OP bodies + identity constants live in `myelin_git::code_tools` (git's domain, riding git's
//! no-host-exec `WireExecutor` port), this is the THIN catalogue registration. See
//! [`git_history_rewrite_tool_def`] / [`git_scip_index_tool_def`].
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §6.1 (ONE catalogue, two
//! front-ends — every subsystem registers typed [`ToolDef`]s into the ONE shared [`ToolSurface`];
//! the same registry is consumed internally by the loop and externally as the MCP projection — NO
//! second governance model), §6.3 (**the FROZEN `requires_approval` defaults table** — Git
//! `git.merge` = **yes** (the consequential gate, AG-8), `open_pr` = **no** (reversible)), §5.0/§5.2
//! (a `mutate` effect routes through [`EffectApi::apply`] — plan-then-apply; a `requires_approval`
//! tool WITHHOLDS at step 6 → `Gated`, applies only after the HITL resume).
//!
//! **VISION §3** (suggest-by-default; consequential/irreversible actions human-confirmed — `merge`
//! is consequential, `open_pr` is reversible). **EI-03 §4** (each new tool is a PROJECTION of the
//! existing plan-then-apply path — NO new engine: registering a `ToolDef` is data, not code).
//! **EI-01 §7** (the compounding payoff — each new producer surface is SMALLER than the last; this
//! whole module is a pair of `ToolDef` constructors + the seed/guard reuse, no new machinery).
//!
//! **Contract-index:** OWNS the Git slice of **8.1** (`register_tool` — the two Git producer
//! ToolDefs). CONSUMES **4.9** (the Git ReBAC fragment supplies the `required_caps` — the
//! `pull_request.merge` permission for `git.merge`, the `repo.push` permission for `open_pr`;
//! `myelin-git`'s [`rebac_fragment`](myelin_git::rebac_fragment) is the SINGLE source of truth for
//! those names). The `requires_approval` column is SEEDED from the frozen §6.3 table via
//! [`crate::defaults::seed_requires_approval`] (AG-P8), so the gating is the frozen default, never
//! hand-set here.
//!
//! ## What this prompt (AG-P18) ships — the Git producer ToolDefs (NO new engine)
//! - [`git_merge_tool_def`] — the `git.merge` ToolDef: `effect_kind = mutate`, `side_effecting`,
//!   `requires_approval = yes` (seeded from §6.3 — the consequential gate AG-8), `required_caps =
//!   [pull_request.merge]` (4.9). It routes through [`EffectApi::apply`] → step-6 WITHHOLD → `Gated`
//!   until the HITL resume (AG-P9). A merge NEVER applies before approval.
//! - [`open_pr_tool_def`] — the `open_pr` ToolDef: `effect_kind = mutate`, `side_effecting`,
//!   `requires_approval = no` (seeded from §6.3 — reversible), `required_caps = [repo.push]` (4.9).
//!   It applies DIRECTLY through the pipeline (no HITL gate).
//! - [`register_git_tools`] — registers BOTH into a caller-supplied [`ToolSurface`] through the
//!   frozen seed + the no-silent-loosening guard ([`crate::defaults::assert_no_silent_loosening`]),
//!   so a registration that tried to silently un-gate `git.merge` is REJECTED LOUD (VISION §3).
//!
//! ## Why this is NOT a new engine (the EI-03 §4 / EI-01 §7 compounding-payoff check)
//! The Git endpoints + the ReBAC fragment + the merge gate are GIT's deliverables (GIT-*). The
//! Fabric half is PURELY the catalogue registration: a `ToolDef` is a row in the ONE registry; the
//! routing (`mutate` → [`EffectApi`]), the gating (`requires_approval` → step-6 withhold), and the
//! HITL machinery already exist (AG-P6/P9). This module adds NO `apply` path, NO gate machinery, NO
//! second governance model — it is data that lights up the existing pipeline. If a Git tool had
//! needed new machinery, the substrate would have been wrong (EI-01 §7).
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **NONE for the Git tools** — they are projections of the existing plan-then-apply path (the
//!   apply pipeline AG-P6, the HITL withhold AG-P9, the frozen defaults AG-P8 all already exist).
//! - **The KNOWLEDGE producer ToolDefs + the content-addressed agent-trace HOLDER seam are
//!   AG-P19 (→ P-268)** (KN `publish`/`edit_confidential` gated, `draft`/`comment` not; the trace
//!   holder KN-D11/KN-D12). This module is the Git slice only; the registration PATTERN here is what
//!   AG-P19 reuses (it depends on this prompt).
//! - **The external MCP ENDPOINT** (auth + the agent-lane rate-limit) is the post-M5 follow-on
//!   (AG-P25); the `exposed_over_mcp` column exists from AG-P1 — these producer tools are NOT
//!   MCP-exposed at v1 (internal-loop only).

use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};
use myelin_git::code_tools as git_code;
use myelin_git::rebac_fragment::object_types as git_objects;

use crate::defaults::{assert_no_silent_loosening, seed_requires_approval, LooseningViolation};

// ───────────────────────── the frozen Git producer-tool identity (the §6.3 keys) ─────────────────

/// **The Git subsystem token** — the `subsystem` half of the catalogue key `(subsystem, name,
/// version)` and the key the FROZEN §6.3 defaults table is looked up under (`("git", "merge")` →
/// gated, `("git", "open_pr")` → not). The SINGLE source of truth so a typo can't drift the seed.
pub const GIT_SUBSYSTEM: &str = "git";

/// **The `git.merge` tool name** (§6.3 — the consequential gate, AG-8). The `requires_approval` seed
/// keys on `("git", "merge")`; the catalogue key is `("git", "merge", version)`.
pub const GIT_MERGE_TOOL: &str = "merge";

/// **The `open_pr` tool name** (§6.3 — reversible). The seed keys on `("git", "open_pr")`.
pub const OPEN_PR_TOOL: &str = "open_pr";

/// **The ToolDef version** the Git producer tools register at (forward-only; the catalogue key is
/// `(subsystem, name, version)`, §4.2). v1 is the first frozen shape.
pub const GIT_TOOL_VERSION: u32 = 1;

// ───────────────────────── the required_caps from the Git ReBAC fragment (4.9) ───────────────────

/// **The `required_caps` for `git.merge` (CONSUMED from 4.9).** Merge is governed by the
/// `pull_request.merge` permission Git's frozen ReBAC fragment declares
/// ([`pull_request_fragment`](myelin_git::rebac_fragment::pull_request_fragment): `merge =
/// parent_repo->protected_push`). The cap STRING is `"<object_type>.<permission>"` — the same shape
/// the EffectApi `check` step (4.2) resolves. Built from the canonical `myelin-git` constants so a
/// rename in the fragment is a compile-or-test break here, never a silent drift.
pub fn git_merge_required_caps() -> Vec<String> {
    vec![format!("{}.merge", git_objects::PULL_REQUEST)]
}

/// **The `required_caps` for `open_pr` (CONSUMED from 4.9).** Opening a PR pushes a branch + creates
/// the PR object — governed by the `repo.push` permission Git's frozen ReBAC fragment declares
/// ([`repo_fragment`](myelin_git::rebac_fragment::repo_fragment): `push = writer + admin +
/// parent_project->write`). Reversible (the PR can be closed), hence NOT gated (§6.3).
pub fn open_pr_required_caps() -> Vec<String> {
    vec![format!("{}.push", git_objects::REPO)]
}

// ───────────────────────── the two Git producer ToolDefs (8.1 — the OWNED registration) ───────────

/// **The `git.merge` ToolDef (8.1) — the consequential, HITL-GATED Git producer tool (§6.3 / AG-8).**
///
/// - `effect_kind = Mutate` ⇒ it routes through [`EffectApi::apply`](myelin_agent::EffectApi) —
///   plan-then-apply, NEVER a direct mutation (§5.0).
/// - `requires_approval` is SEEDED from the frozen §6.3 default (`("git", "merge")` → `true`), so
///   step 6 of the pipeline WITHHOLDS (returns `Gated`, does NOT mutate) until the HITL resume adds
///   it to the run's `approved` set (AG-P9). A merge NEVER applies before approval (AG-8).
/// - `required_caps = [pull_request.merge]` (4.9) — the cap the EffectApi `check` step enforces.
/// - `exposed_over_mcp = false` — internal-loop only at v1 (the external MCP endpoint is AG-P25).
pub fn git_merge_tool_def() -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(GIT_MERGE_TOOL.to_string()),
        subsystem: GIT_SUBSYSTEM.to_string(),
        version: GIT_TOOL_VERSION,
        // The merge input the Git public endpoint validates (the PR ref + the merge strategy). An
        // opaque-string JSON-Schema carrier at this seam (the rich schema is Git's endpoint's).
        input_schema: r#"{"type":"object","required":["pull_request"],"properties":{"pull_request":{"type":"string"},"strategy":{"type":"string","enum":["merge","squash","rebase"]}}}"#.to_string(),
        required_caps: git_merge_required_caps(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED below from §6.3 (the value here is overwritten by seed_requires_approval → true).
        requires_approval: true,
        exposed_over_mcp: false,
    })
}

/// **The `open_pr` ToolDef (8.1) — the reversible, NON-gated Git producer tool (§6.3).**
///
/// - `effect_kind = Mutate` ⇒ it routes through [`EffectApi::apply`] (plan-then-apply) — still
///   governed (cap-checked, metered), just NOT HITL-gated.
/// - `requires_approval` is SEEDED from the frozen §6.3 default (`("git", "open_pr")` → `false`), so
///   the pipeline applies it DIRECTLY (step 6 is a no-op; no gate).
/// - `required_caps = [repo.push]` (4.9).
/// - `exposed_over_mcp = false` — internal-loop only at v1.
pub fn open_pr_tool_def() -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(OPEN_PR_TOOL.to_string()),
        subsystem: GIT_SUBSYSTEM.to_string(),
        version: GIT_TOOL_VERSION,
        input_schema: r#"{"type":"object","required":["repo","source_ref","target_ref"],"properties":{"repo":{"type":"string"},"source_ref":{"type":"string"},"target_ref":{"type":"string"},"title":{"type":"string"}}}"#.to_string(),
        required_caps: open_pr_required_caps(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED below from §6.3 (the value here is overwritten by seed_requires_approval → false).
        requires_approval: false,
        exposed_over_mcp: false,
    })
}

// ───────────────── the CODE-EXECUTING Git producer ToolDefs (GIT-P27 → P-283, M3-G6) ─────────────
//
// The two tools that EXECUTE CODE on the ONE unified sandbox (§7 — "any Git tool that executes code:
// the history-rewrite activity, SCIP indexing if run as a job"). They inherit the FOUR uniform
// sandbox guarantees BY CONSTRUCTION (reserve/settle 11.7, per-run token 4.7, HITL withhold, the
// isolation floor + the AG-D4 real-kernel escape drill) because they ride the SAME seams the agent
// loop already routes through — `EffectApi::apply` (the gated history-rewrite) and the sandbox exec
// (the SCIP compute job). The OP bodies + the identity constants live in `myelin_git::code_tools`
// (git's domain, riding git's no-host-exec `WireExecutor` port); this is the THIN catalogue
// registration (a `ToolDef` is a row in the ONE registry — EI-03 §4, no new engine).

/// **The `git.history_rewrite` ToolDef (8.1 / 10.6) — the audited, HITL-GATED erasure-admin tool.**
///
/// - `effect_kind = Mutate` ⇒ it routes through [`EffectApi::apply`](myelin_agent::EffectApi)
///   (plan-then-apply) — a history-rewrite changes every downstream hash (consequential), so it is
///   NEVER a direct mutation.
/// - `requires_approval` is SEEDED from the frozen §6.3 default — `("git", "history_rewrite")` is a
///   NAMED row (`true`, gated): a history-rewrite changes every downstream hash (consequential,
///   irreversible — VISION §3), so step 6 WITHHOLDS (`Gated`) until the HITL resume.
/// - `required_caps = [repo.administer]` (4.9) — the admin-scoped tenant op (recon §9), built from
///   the canonical `myelin_git::code_tools::history_rewrite_required_caps` (a fragment rename breaks
///   here, never a silent drift).
/// - `exposed_over_mcp = false` — GF-9 floor (the external MCP server + threat model is GIT-P33/P6).
pub fn git_history_rewrite_tool_def() -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(git_code::HISTORY_REWRITE_TOOL.to_string()),
        subsystem: git_code::GIT_SUBSYSTEM.to_string(),
        version: git_code::GIT_CODE_TOOL_VERSION,
        // The rewrite input the Git audited op validates (the repo + the target refs + the opaque
        // reason code — never the leaked content). An opaque-string JSON-Schema carrier at this seam.
        input_schema: r#"{"type":"object","required":["repo","target_refs","reason_code"],"properties":{"repo":{"type":"string"},"target_refs":{"type":"array","items":{"type":"string"}},"reason_code":{"type":"string"}}}"#.to_string(),
        required_caps: git_code::history_rewrite_required_caps(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED below from §6.3 (fail-closed default `true` for this unnamed consequential op).
        requires_approval: true,
        exposed_over_mcp: false,
    })
}

/// **The `git.scip_index` ToolDef (8.1 / §7) — the SCIP-indexing COMPUTE tool on the unified sandbox.**
///
/// - `effect_kind = Compute` ⇒ the platform loop routes it to the **sandbox** (`ToolHands::exec` =
///   the CI `kind=agent` job, §5.0) — untrusted code execution that produces a read-only
///   code-intelligence index. `side_effecting = false` (it reads repo bytes, writes an artifact).
/// - `requires_approval = false` — a read-only index build is reversible/advisory, NOT a gated
///   mutation. (It is the ONLY effect kind that reaches the bare kernel sandbox, so the AG-D4 escape
///   drill on the git tool image gates it BY CONSTRUCTION — `exec.rs`'s routing split + escape gate.)
/// - `required_caps = [repo.pull]` (4.9) — reading the repo objects to index them.
/// - `exposed_over_mcp = false` — GF-9 floor.
pub fn git_scip_index_tool_def() -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(git_code::SCIP_INDEX_TOOL.to_string()),
        subsystem: git_code::GIT_SUBSYSTEM.to_string(),
        version: git_code::GIT_CODE_TOOL_VERSION,
        input_schema: r#"{"type":"object","required":["repo","commit_oid"],"properties":{"repo":{"type":"string"},"commit_oid":{"type":"string"}}}"#.to_string(),
        required_caps: git_code::scip_index_required_caps(),
        // A `compute` tool → the unified sandbox (the ONLY route that touches the kernel; the AG-D4
        // escape drill on the git tool image gates it). It is NOT a mutation (no EffectApi gate).
        effect_kind: EffectKind::Compute,
        side_effecting: false,
        // SEEDED below from §6.3 (a read-only index build is NOT gated).
        requires_approval: false,
        exposed_over_mcp: false,
    })
}

/// **The four Git producer ToolDefs, in catalogue order** — the two producer mutations (`git.merge`
/// gated, `open_pr` reversible, P-267) + the two CODE-EXECUTING tools on the unified sandbox
/// (`git.history_rewrite` gated/`Mutate`, `git.scip_index` `Compute`, P-283). The single list every
/// registration + CDC consumes (one source of truth — a drift in any def is caught once). Each is
/// SEEDED from the frozen §6.3 defaults.
pub fn git_tool_defs() -> Vec<ToolDef> {
    vec![
        git_merge_tool_def(),
        open_pr_tool_def(),
        git_history_rewrite_tool_def(),
        git_scip_index_tool_def(),
    ]
}

// ───────────────────────── the registration seam (8.1 — into the ONE ToolSurface) ────────────────

/// **Register the Git producer ToolDefs into the ONE [`ToolSurface`] (8.1 / §6.1) — the OWNED
/// deliverable.** Every def is passed through the VISION §3 no-silent-loosening guard FIRST
/// ([`assert_no_silent_loosening`]): a registration that tried to flip the frozen `git.merge`
/// `yes → no` WITHOUT a written deviation is REJECTED LOUD (`Err`), never silently un-gated. The
/// defs themselves are already seeded from the frozen table, so the strict (no-deviation) call
/// always admits them — the guard is the structural proof that a future hand-edit can't loosen the
/// consequential gate unnoticed.
///
/// Returns the registered defs on success (so the caller can assert the catalogue), or the first
/// [`LooseningViolation`] if any def silently loosened a frozen gate (it never does for the seeded
/// defs — the guard is the ratchet, not a runtime branch).
pub fn register_git_tools<S: ToolSurface>(surface: &mut S) -> Result<Vec<ToolDef>, LooseningViolation> {
    let defs = git_tool_defs();
    // The guard runs with NO deviations (the strict default): the seeded defs always pass; a
    // hand-loosened def would be rejected LOUD here.
    for def in &defs {
        assert_no_silent_loosening(def, &[])?;
    }
    for def in &defs {
        surface.register_tool(def.clone());
    }
    Ok(defs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::requires_approval_default;

    /// A `ToolSurface` over a fixed catalogue (the §4.2 in-memory registry — the same shape the
    /// EffectApi CDC uses). The ONE catalogue both producer tools register into.
    struct Catalogue {
        defs: Vec<ToolDef>,
    }
    impl ToolSurface for Catalogue {
        fn register_tool(&mut self, def: ToolDef) {
            self.defs.push(def);
        }
        fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
            self.defs.iter().find(|d| &d.name == name)
        }
    }

    /// **`git.merge` carries the FROZEN §6.3 `requires_approval = yes` default (the consequential
    /// gate, AG-8) — and it is SEEDED from the frozen table, not hand-set.** A hand-set value can't
    /// drift the gate: the def's `requires_approval` equals [`requires_approval_default`] verbatim.
    #[test]
    fn git_merge_is_gated_by_the_frozen_default() {
        let def = git_merge_tool_def();
        assert!(def.requires_approval, "git.merge is HITL-gated (§6.3 / AG-8)");
        assert_eq!(
            def.requires_approval,
            requires_approval_default(GIT_SUBSYSTEM, GIT_MERGE_TOOL),
            "git.merge's gating IS the frozen §6.3 default (seeded, not hand-set)"
        );
        // a mutate effect routes through EffectApi (plan-then-apply), never a direct mutation.
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
    }

    /// **`open_pr` carries the FROZEN §6.3 `requires_approval = no` default (reversible) — seeded,
    /// not hand-set.** It applies DIRECTLY through the pipeline (no HITL gate).
    #[test]
    fn open_pr_is_not_gated_by_the_frozen_default() {
        let def = open_pr_tool_def();
        assert!(!def.requires_approval, "open_pr is reversible → NOT gated (§6.3)");
        assert_eq!(
            def.requires_approval,
            requires_approval_default(GIT_SUBSYSTEM, OPEN_PR_TOOL),
            "open_pr's (non-)gating IS the frozen §6.3 default (seeded, not hand-set)"
        );
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
    }

    /// **The `required_caps` come from the FROZEN Git ReBAC fragment (4.9), not invented here.**
    /// `git.merge` → `pull_request.merge`; `open_pr` → `repo.push`. Built from the canonical
    /// `myelin-git` object-type constants, so a fragment rename breaks this test (no silent drift).
    #[test]
    fn required_caps_are_the_git_rebac_fragment_permissions() {
        assert_eq!(git_merge_tool_def().required_caps, vec!["pull_request.merge".to_string()]);
        assert_eq!(open_pr_tool_def().required_caps, vec!["repo.push".to_string()]);
        // the object-type halves ARE the canonical Git ReBAC names (4.9), not local strings.
        assert_eq!(git_objects::PULL_REQUEST, "pull_request");
        assert_eq!(git_objects::REPO, "repo");
    }

    /// **`register_git_tools` registers BOTH producer ToolDefs into the ONE catalogue (8.1 / §6.1)
    /// and they resolve by name with their frozen shapes.** The registration is the whole deliverable
    /// — a `ToolDef` is a row in the ONE registry, no second governance model.
    #[test]
    fn register_git_tools_registers_both_into_the_one_surface() {
        let mut cat = Catalogue { defs: vec![] };
        let registered = register_git_tools(&mut cat).expect("seeded defs always admit");
        assert_eq!(registered.len(), 4, "git.merge + open_pr + history_rewrite + scip_index");

        let merge = cat.resolve(&ToolName(GIT_MERGE_TOOL.into())).expect("git.merge registered");
        assert_eq!(merge.subsystem, GIT_SUBSYSTEM);
        assert!(merge.requires_approval, "the registered git.merge is gated");
        assert_eq!(merge.required_caps, vec!["pull_request.merge".to_string()]);

        let pr = cat.resolve(&ToolName(OPEN_PR_TOOL.into())).expect("open_pr registered");
        assert_eq!(pr.subsystem, GIT_SUBSYSTEM);
        assert!(!pr.requires_approval, "the registered open_pr is NOT gated");
        assert_eq!(pr.required_caps, vec!["repo.push".to_string()]);

        // a tool NOT registered resolves to None (the catalogue is exactly these four).
        assert!(cat.resolve(&ToolName("git.delete_repo".into())).is_none());
    }

    // ───────────── the CODE-EXECUTING tools on the unified sandbox (GIT-P27 → P-283) ─────────────

    /// **`git.history_rewrite` is `Mutate` → `EffectApi::apply`, GATED by the fail-closed §6.3
    /// default (the audited erasure-admin op, 10.6 / recon §9).** A history-rewrite changes every
    /// downstream hash (consequential), so it routes through plan-then-apply and WITHHOLDS until the
    /// HITL resume — never a direct, un-approved mutation.
    #[test]
    fn git_history_rewrite_is_a_gated_mutate_tool() {
        let def = git_history_rewrite_tool_def();
        assert_eq!(def.subsystem, GIT_SUBSYSTEM);
        assert_eq!(def.name.0, myelin_git::code_tools::HISTORY_REWRITE_TOOL);
        // Mutate ⇒ EffectApi (plan-then-apply), never the bare sandbox / a direct write.
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
        // GATED — and it IS the frozen §6.3 seed (the named consequential erasure-admin row).
        assert!(def.requires_approval, "history-rewrite is HITL-gated (the §6.3 consequential row)");
        assert_eq!(
            def.requires_approval,
            requires_approval_default(GIT_SUBSYSTEM, myelin_git::code_tools::HISTORY_REWRITE_TOOL),
            "the gating IS the frozen §6.3 seed (the consequential history-rewrite row)"
        );
        // admin-scoped tenant op cap (recon §9), from the canonical git constants.
        assert_eq!(def.required_caps, vec!["repo.administer".to_string()]);
        // GF-9: not MCP-exposed at v1.
        assert!(!def.exposed_over_mcp, "GF-9: no external MCP endpoint at v1");
    }

    /// **`git.scip_index` is a `Compute` tool → the unified sandbox (`ToolHands::exec`), NOT gated
    /// (§7).** SCIP indexing reads repo bytes + writes a read-only code-intelligence artifact — the
    /// ONLY effect kind that reaches the bare kernel sandbox, so the AG-D4 escape drill on the git
    /// tool image gates it BY CONSTRUCTION (the routing split + escape gate in `exec.rs`).
    #[test]
    fn git_scip_index_is_a_compute_tool_on_the_unified_sandbox() {
        let def = git_scip_index_tool_def();
        assert_eq!(def.subsystem, GIT_SUBSYSTEM);
        assert_eq!(def.name.0, myelin_git::code_tools::SCIP_INDEX_TOOL);
        // Compute ⇒ the sandbox (the only kind that touches the kernel — gated by AG-D4).
        assert_eq!(def.effect_kind, EffectKind::Compute);
        assert!(!def.side_effecting, "a read-only index build is not a mutation");
        // NOT gated (a reversible/advisory read-only artifact) — and it IS the frozen seed.
        assert!(!def.requires_approval, "a read-only index build is NOT gated");
        assert_eq!(def.required_caps, vec!["repo.pull".to_string()]);
        assert!(!def.exposed_over_mcp, "GF-9: no external MCP endpoint at v1");
    }

    /// **The catalogue carries ALL FOUR Git producer tools, seeded from the frozen §6.3 defaults
    /// (the two mutations + the two code-executing tools).** A drift in any def's gating is caught
    /// against the frozen seed once.
    #[test]
    fn all_four_git_tools_are_seeded_from_the_frozen_defaults() {
        let defs = git_tool_defs();
        assert_eq!(defs.len(), 4, "merge + open_pr + history_rewrite + scip_index");
        for d in &defs {
            assert_eq!(d.subsystem, GIT_SUBSYSTEM);
            // every def's gating IS the frozen §6.3 seed (never a value local to this module).
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating is the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
        }
        // the consequential split: merge + history_rewrite gated; open_pr + scip_index not.
        let gated: Vec<&str> = defs
            .iter()
            .filter(|d| d.requires_approval)
            .map(|d| d.name.0.as_str())
            .collect();
        assert!(gated.contains(&"merge"));
        assert!(gated.contains(&"history_rewrite"));
        assert!(!gated.contains(&"open_pr"));
        assert!(!gated.contains(&"scip_index"));
        // exactly ONE compute (sandbox-bound) tool — the SCIP indexer.
        let compute: Vec<&str> = defs
            .iter()
            .filter(|d| d.effect_kind == EffectKind::Compute)
            .map(|d| d.name.0.as_str())
            .collect();
        assert_eq!(compute, vec!["scip_index"], "only SCIP indexing reaches the bare sandbox");
    }

    /// **The no-silent-loosening guard (VISION §3) protects the registration path.** A `git.merge`
    /// def hand-loosened to `requires_approval = false` WITHOUT a written deviation is REJECTED LOUD
    /// — proving the registration seam can't silently un-gate the consequential merge.
    #[test]
    fn a_hand_loosened_git_merge_registration_is_rejected_loud() {
        // a def that bypassed the seed and set the gate OFF (the failure the guard catches).
        let mut loosened = git_merge_tool_def();
        loosened.requires_approval = false;
        let err = assert_no_silent_loosening(&loosened, &[]).unwrap_err();
        assert_eq!(err.subsystem, "git");
        assert_eq!(err.tool, "merge");
        assert!(
            err.to_string().contains("WITHOUT a written deviation"),
            "the loosening is surfaced LOUD: {err}"
        );
    }

    /// **The compounding-payoff / no-new-engine check (EI-03 §4 / EI-01 §7).** The two PRODUCER
    /// MUTATION tools (`git.merge`, `open_pr`) are PURE data: each is a `mutate` `ToolDef` whose
    /// gating is the frozen §6.3 seed and whose caps are the frozen 4.9 fragment — there is NO
    /// bespoke apply/gate machinery in this module (it constructs `ToolDef`s and registers them; the
    /// routing + gating + HITL are the existing pipeline). This test pins that invariant for the
    /// producer mutations: both route the SAME way (`Mutate` → EffectApi) and differ ONLY in their
    /// frozen `requires_approval` seed. (The CODE-EXECUTING tools, P-283, are covered by
    /// [`all_four_git_tools_are_seeded_from_the_frozen_defaults`] — `history_rewrite` is a gated
    /// `Mutate`, `scip_index` is an un-gated `Compute` that rides the sandbox.)
    #[test]
    fn the_git_producer_mutations_are_a_projection_not_a_new_engine() {
        let mutations = [git_merge_tool_def(), open_pr_tool_def()];
        for d in &mutations {
            assert_eq!(
                d.effect_kind,
                EffectKind::Mutate,
                "every Git producer MUTATION routes through EffectApi (plan-then-apply) — no new path"
            );
            assert!(d.side_effecting);
            // the gating is the frozen seed, never a value local to this module.
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating is the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
        }
        // the ONLY difference between the two producer mutations is the frozen gate (merge yes,
        // open_pr no).
        assert_ne!(
            mutations[0].requires_approval, mutations[1].requires_approval,
            "git.merge is gated, open_pr is not — the consequential split"
        );
    }
}
