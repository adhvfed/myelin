//! # `knowledge_tools` — the per-producer **Knowledge** ToolDefs registered into the ONE ToolSurface
//! (AG-P19 → P-268, M3): `publish` / `edit_confidential` (gated) + `draft` / `comment` (reversible)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §6.1 (ONE catalogue, two
//! front-ends — every subsystem registers typed [`ToolDef`]s into the ONE shared [`ToolSurface`]; the
//! same registry is consumed internally by the loop and externally as the MCP projection — NO second
//! governance model), §6.3 (**the FROZEN `requires_approval` defaults table** — Knowledge `publish` /
//! `edit(confidential_page)` = **yes** (publishing/confidential edits are consequential, an approver
//! set), `draft` / `comment` = **no** (reversible)), §5.0/§5.2 (a `mutate` effect routes through
//! [`EffectApi::apply`] — plan-then-apply; a `requires_approval` tool WITHHOLDS at step 6 → `Gated`,
//! applies only after the HITL resume).
//!
//! **VISION §3** (suggest-by-default; consequential/irreversible actions human-confirmed — `publish`
//! and a confidential `edit` are consequential, `draft`/`comment` are reversible). **EI-03 §4** (each
//! new tool is a PROJECTION of the existing plan-then-apply path — NO new engine: registering a
//! `ToolDef` is data, not code). **EI-01 §7** (the compounding payoff — this KN producer surface is
//! the SAME registration shape as the Git one [`crate::git_tools`]; no new machinery, just four more
//! `ToolDef` rows seeded from the frozen table).
//!
//! **Contract-index:** OWNS the Knowledge slice of **8.1** (`register_tool` — the four KN producer
//! ToolDefs). CONSUMES **4.9** (the KN ReBAC fragment supplies the `required_caps` — the producer-tool
//! write permissions on the `page` object: `page.publish` / `page.edit` / `page.draft` /
//! `page.comment`; `myelin-content`'s [`rebac_fragment`](myelin_content::rebac_fragment) is the SINGLE
//! source of truth for those names, the KN parallel to `myelin-git::rebac_fragment` Git's tools use).
//! The `requires_approval` column is SEEDED from the frozen §6.3 table via
//! [`crate::defaults::seed_requires_approval`] (AG-P8), so the gating is the frozen default, never
//! hand-set here.
//!
//! ## What this prompt (AG-P19) ships — the Knowledge producer ToolDefs (NO new engine)
//! - [`publish_tool_def`] — `knowledge.publish`: `effect_kind = mutate`, `side_effecting`,
//!   `requires_approval = yes` (seeded from §6.3 — consequential, an approver set), `required_caps =
//!   [page.publish]` (4.9). Routes through [`EffectApi::apply`] → step-6 WITHHOLD → `Gated` until the
//!   HITL resume (AG-P9). A publish NEVER applies before approval.
//! - [`edit_confidential_tool_def`] — `knowledge.edit_confidential`: gated identically (`yes`),
//!   `required_caps = [page.edit]` (4.9). The §6.3 `edit(confidential_page)` row.
//! - [`draft_tool_def`] — `knowledge.draft`: `requires_approval = no` (reversible), `required_caps =
//!   [page.draft]` (4.9). Applies DIRECTLY through the pipeline (no HITL gate).
//! - [`comment_tool_def`] — `knowledge.comment`: `requires_approval = no` (reversible),
//!   `required_caps = [page.comment]` (4.9). Applies DIRECTLY.
//! - [`register_knowledge_tools`] — registers ALL FOUR into a caller-supplied [`ToolSurface`] through
//!   the frozen seed + the no-silent-loosening guard ([`crate::defaults::assert_no_silent_loosening`]),
//!   so a registration that tried to silently un-gate `publish`/`edit_confidential` is REJECTED LOUD
//!   (VISION §3).
//!
//! ## Why this is NOT a new engine (the EI-03 §4 / EI-01 §7 compounding-payoff check)
//! The KN endpoints (publish/edit/draft/comment) + the KN ReBAC fragment + the trace HOLDER itself
//! are KNOWLEDGE's deliverables (KN-P04+). The Fabric half is PURELY the catalogue registration: a
//! `ToolDef` is a row in the ONE registry; the routing (`mutate` → [`EffectApi`]), the gating
//! (`requires_approval` → step-6 withhold), and the HITL machinery already exist (AG-P6/P9). This
//! module adds NO `apply` path, NO gate machinery, NO second governance model — it is data that lights
//! up the existing pipeline, identical in shape to the Git producer tools (AG-P18). If a KN tool had
//! needed new machinery, the substrate would have been wrong (EI-01 §7).
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **NONE for the KN tools** — they are projections of the existing plan-then-apply path (the apply
//!   pipeline AG-P6, the HITL withhold AG-P9, the frozen defaults AG-P8 all already exist; the §6.3 KN
//!   defaults were already in the seed table at AG-P8).
//! - **The agent-trace HOLDER seam is wired in [`crate::trace_seam`]** (the M3 leg of AG-7 / 8.8): the
//!   content-addressed write reuses the frozen `myelin-content` 13.1 block model and `run.trace_ref`
//!   resolves to it; the holder registers as the erasable H17 PersonalDataHolder ([`crate::holder`]).
//! - **Agent long-term memory / RAG over prior runs is a NAMED HOLDER SEAM, NOT BUILT** — v1 agents
//!   are stateless across runs EXCEPT for the content-addressed trace document (see
//!   [`crate::trace_seam`]). The embedding store + its erasure are a Search/Knowledge follow-on
//!   (post-M5, AG-P25); the FULL DSR fan-out over the trace is AG-P23 (→ P-479).
//! - **The external MCP ENDPOINT** (auth + the agent-lane rate-limit) is the post-M5 follow-on
//!   (AG-P25); these producer tools are NOT MCP-exposed at v1 (`exposed_over_mcp = false`).

use myelin_agent::{ToolDef, ToolSurface};
use myelin_content::rebac_fragment::object_types as kn_objects;
use myelin_content::rebac_fragment::{COMMENT, DRAFT, EDIT, PUBLISH};

use crate::defaults::{cap, mutate_tool_def, register_tool_defs, LooseningViolation};

// ───────────────────────── the frozen KN producer-tool identity (the §6.3 keys) ──────────────────

/// **The Knowledge subsystem token** — the `subsystem` half of the catalogue key `(subsystem, name,
/// version)` and the key the FROZEN §6.3 defaults table is looked up under (`("knowledge", "publish")`
/// → gated, `("knowledge", "draft")` → not). The SINGLE source of truth so a typo can't drift the
/// seed.
pub const KNOWLEDGE_SUBSYSTEM: &str = "knowledge";

/// **The `knowledge.publish` tool name** (§6.3 — consequential, gated). The `requires_approval` seed
/// keys on `("knowledge", "publish")`.
pub const PUBLISH_TOOL: &str = "publish";

/// **The `knowledge.edit_confidential` tool name** (§6.3 `edit(confidential_page)` — consequential,
/// gated). The seed keys on `("knowledge", "edit_confidential")` (the seed name in
/// [`crate::defaults`]).
pub const EDIT_CONFIDENTIAL_TOOL: &str = "edit_confidential";

/// **The `knowledge.draft` tool name** (§6.3 — reversible, NOT gated). The seed keys on
/// `("knowledge", "draft")`.
pub const DRAFT_TOOL: &str = "draft";

/// **The `knowledge.comment` tool name** (§6.3 — reversible, NOT gated). The seed keys on
/// `("knowledge", "comment")`.
pub const COMMENT_TOOL: &str = "comment";

/// **The ToolDef version** the KN producer tools register at (forward-only; the catalogue key is
/// `(subsystem, name, version)`, §4.2). v1 is the first frozen shape.
pub const KNOWLEDGE_TOOL_VERSION: u32 = 1;

// ───────────────────────── the required_caps from the KN ReBAC fragment (4.9) ────────────────────

/// **The `required_caps` for `knowledge.publish` (CONSUMED from 4.9).** Publishing a page is governed
/// by the `page.publish` write permission the KN names-only ReBAC carrier declares
/// ([`page_write_fragment`](myelin_content::rebac_fragment::page_write_fragment)). The cap STRING is
/// `"<object_type>.<permission>"` — the same shape the EffectApi `check` step (4.2) resolves. Built
/// from the canonical `myelin-content` constants so a rename in the carrier is a compile-or-test break
/// here, never a silent drift (the KN parallel to git_tools sourcing from `myelin-git`).
pub fn publish_required_caps() -> Vec<String> {
    cap(kn_objects::PAGE, PUBLISH)
}

/// **The `required_caps` for `knowledge.edit_confidential` (CONSUMED from 4.9).** Editing a
/// confidential page is governed by the `page.edit` write permission (4.9). Consequential → gated.
pub fn edit_confidential_required_caps() -> Vec<String> {
    cap(kn_objects::PAGE, EDIT)
}

/// **The `required_caps` for `knowledge.draft` (CONSUMED from 4.9).** Creating/updating a private
/// draft is governed by the `page.draft` write permission (4.9). Reversible → NOT gated.
pub fn draft_required_caps() -> Vec<String> {
    cap(kn_objects::PAGE, DRAFT)
}

/// **The `required_caps` for `knowledge.comment` (CONSUMED from 4.9).** Commenting on a page is
/// governed by the `page.comment` write permission (4.9). Reversible → NOT gated.
pub fn comment_required_caps() -> Vec<String> {
    cap(kn_objects::PAGE, COMMENT)
}

// ───────────────────────── the four KN producer ToolDefs (8.1 — the OWNED registration) ───────────

/// **The `knowledge.publish` ToolDef (8.1) — the consequential, HITL-GATED KN producer tool
/// (§6.3).**
///
/// - `effect_kind = Mutate` ⇒ it routes through [`EffectApi::apply`](myelin_agent::EffectApi) —
///   plan-then-apply, NEVER a direct mutation (§5.0).
/// - `requires_approval` is SEEDED from the frozen §6.3 default (`("knowledge", "publish")` →
///   `true`), so step 6 of the pipeline WITHHOLDS (returns `Gated`, does NOT mutate) until the HITL
///   resume adds it to the run's `approved` set (AG-P9). A publish NEVER applies before approval.
/// - `required_caps = [page.publish]` (4.9) — the cap the EffectApi `check` step enforces.
/// - `exposed_over_mcp = false` — internal-loop only at v1 (the external MCP endpoint is AG-P25).
pub fn publish_tool_def() -> ToolDef {
    // The publish input the KN public endpoint validates (the page ref + the approver-set basis). An
    // opaque-string JSON-Schema carrier at this seam (the rich schema is KN's endpoint's).
    mutate_tool_def(
        KNOWLEDGE_SUBSYSTEM,
        PUBLISH_TOOL,
        KNOWLEDGE_TOOL_VERSION,
        r#"{"type":"object","required":["page"],"properties":{"page":{"type":"string"},"space":{"type":"string"}}}"#,
        publish_required_caps(),
    )
}

/// **The `knowledge.edit_confidential` ToolDef (8.1) — the consequential, HITL-GATED confidential-edit
/// producer tool (§6.3 `edit(confidential_page)`).** Gated identically to `publish` (`yes`),
/// `required_caps = [page.edit]` (4.9). A confidential edit NEVER applies before approval.
pub fn edit_confidential_tool_def() -> ToolDef {
    // The edit input: the page ref + the content delta (the rich block delta is KN's endpoint's).
    mutate_tool_def(
        KNOWLEDGE_SUBSYSTEM,
        EDIT_CONFIDENTIAL_TOOL,
        KNOWLEDGE_TOOL_VERSION,
        r#"{"type":"object","required":["page","blocks"],"properties":{"page":{"type":"string"},"blocks":{"type":"array"}}}"#,
        edit_confidential_required_caps(),
    )
}

/// **The `knowledge.draft` ToolDef (8.1) — the reversible, NON-gated KN producer tool (§6.3).**
///
/// - `effect_kind = Mutate` ⇒ it routes through [`EffectApi::apply`] (plan-then-apply) — still
///   governed (cap-checked, metered), just NOT HITL-gated.
/// - `requires_approval` is SEEDED from the frozen §6.3 default (`("knowledge", "draft")` → `false`),
///   so the pipeline applies it DIRECTLY (step 6 is a no-op; no gate).
/// - `required_caps = [page.draft]` (4.9). `exposed_over_mcp = false`.
pub fn draft_tool_def() -> ToolDef {
    mutate_tool_def(
        KNOWLEDGE_SUBSYSTEM,
        DRAFT_TOOL,
        KNOWLEDGE_TOOL_VERSION,
        r#"{"type":"object","required":["space"],"properties":{"space":{"type":"string"},"title":{"type":"string"},"blocks":{"type":"array"}}}"#,
        draft_required_caps(),
    )
}

/// **The `knowledge.comment` ToolDef (8.1) — the reversible, NON-gated KN producer tool (§6.3).**
/// `required_caps = [page.comment]` (4.9). Applies DIRECTLY (no HITL gate).
pub fn comment_tool_def() -> ToolDef {
    mutate_tool_def(
        KNOWLEDGE_SUBSYSTEM,
        COMMENT_TOOL,
        KNOWLEDGE_TOOL_VERSION,
        r#"{"type":"object","required":["page","body"],"properties":{"page":{"type":"string"},"body":{"type":"string"}}}"#,
        comment_required_caps(),
    )
}

/// **The four KN producer ToolDefs, in catalogue order (publish → edit_confidential → draft →
/// comment).** The single list every registration + CDC consumes (one source of truth — a drift in
/// any def is caught once). All four are SEEDED from the frozen §6.3 defaults.
pub fn knowledge_tool_defs() -> Vec<ToolDef> {
    vec![
        publish_tool_def(),
        edit_confidential_tool_def(),
        draft_tool_def(),
        comment_tool_def(),
    ]
}

// ───────────────────────── the registration seam (8.1 — into the ONE ToolSurface) ────────────────

/// **Register the Knowledge producer ToolDefs into the ONE [`ToolSurface`] (8.1 / §6.1) — the OWNED
/// deliverable.** Every def is passed through the VISION §3 no-silent-loosening guard FIRST
/// ([`assert_no_silent_loosening`]): a registration that tried to flip the frozen `publish` /
/// `edit_confidential` `yes → no` WITHOUT a written deviation is REJECTED LOUD (`Err`), never silently
/// un-gated. The defs themselves are already seeded from the frozen table, so the strict (no-deviation)
/// call always admits them — the guard is the structural proof that a future hand-edit can't loosen
/// the consequential gate unnoticed.
///
/// Returns the registered defs on success (so the caller can assert the catalogue), or the first
/// [`LooseningViolation`] if any def silently loosened a frozen gate (it never does for the seeded
/// defs — the guard is the ratchet, not a runtime branch). Identical in shape to
/// [`crate::git_tools::register_git_tools`] — the compounding-payoff reuse.
pub fn register_knowledge_tools<S: ToolSurface>(
    surface: &mut S,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    register_tool_defs(surface, knowledge_tool_defs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::{assert_no_silent_loosening, requires_approval_default};
    use myelin_agent::{EffectKind, ToolName};

    /// A `ToolSurface` over a fixed catalogue (the §4.2 in-memory registry — the same shape the
    /// EffectApi CDC uses). The ONE catalogue all four producer tools register into.
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

    /// **`publish` + `edit_confidential` carry the FROZEN §6.3 `requires_approval = yes` default
    /// (consequential) — and they are SEEDED from the frozen table, not hand-set.** The def's
    /// `requires_approval` equals [`requires_approval_default`] verbatim (a hand-set value can't drift
    /// the gate).
    #[test]
    fn publish_and_edit_confidential_are_gated_by_the_frozen_default() {
        for (def, tool) in [
            (publish_tool_def(), PUBLISH_TOOL),
            (edit_confidential_tool_def(), EDIT_CONFIDENTIAL_TOOL),
        ] {
            assert!(
                def.requires_approval,
                "knowledge.{tool} is HITL-gated (§6.3 — consequential)"
            );
            assert_eq!(
                def.requires_approval,
                requires_approval_default(KNOWLEDGE_SUBSYSTEM, tool),
                "knowledge.{tool}'s gating IS the frozen §6.3 default (seeded, not hand-set)"
            );
            // a mutate effect routes through EffectApi (plan-then-apply), never a direct mutation.
            assert_eq!(def.effect_kind, EffectKind::Mutate);
            assert!(def.side_effecting);
        }
    }

    /// **`draft` + `comment` carry the FROZEN §6.3 `requires_approval = no` default (reversible) —
    /// seeded, not hand-set.** They apply DIRECTLY through the pipeline (no HITL gate).
    #[test]
    fn draft_and_comment_are_not_gated_by_the_frozen_default() {
        for (def, tool) in [
            (draft_tool_def(), DRAFT_TOOL),
            (comment_tool_def(), COMMENT_TOOL),
        ] {
            assert!(
                !def.requires_approval,
                "knowledge.{tool} is reversible → NOT gated (§6.3)"
            );
            assert_eq!(
                def.requires_approval,
                requires_approval_default(KNOWLEDGE_SUBSYSTEM, tool),
                "knowledge.{tool}'s (non-)gating IS the frozen §6.3 default (seeded, not hand-set)"
            );
            assert_eq!(def.effect_kind, EffectKind::Mutate);
            assert!(def.side_effecting);
        }
    }

    /// **The `required_caps` come from the FROZEN KN ReBAC fragment (4.9), not invented here.**
    /// `publish` → `page.publish`; `edit_confidential` → `page.edit`; `draft` → `page.draft`;
    /// `comment` → `page.comment`. Built from the canonical `myelin-content` carrier constants, so a
    /// rename breaks this test (no silent drift — the KN parallel to the Git CDC).
    #[test]
    fn required_caps_are_the_kn_rebac_fragment_permissions() {
        assert_eq!(
            publish_tool_def().required_caps,
            vec!["page.publish".to_string()]
        );
        assert_eq!(
            edit_confidential_tool_def().required_caps,
            vec!["page.edit".to_string()]
        );
        assert_eq!(
            draft_tool_def().required_caps,
            vec!["page.draft".to_string()]
        );
        assert_eq!(
            comment_tool_def().required_caps,
            vec!["page.comment".to_string()]
        );
        // the object-type half IS the canonical KN ReBAC name (4.9), not a local string.
        assert_eq!(kn_objects::PAGE, "page");
    }

    /// **`register_knowledge_tools` registers ALL FOUR producer ToolDefs into the ONE catalogue
    /// (8.1 / §6.1) and they resolve by name with their frozen shapes.** The registration is the whole
    /// deliverable — a `ToolDef` is a row in the ONE registry, no second governance model.
    #[test]
    fn register_knowledge_tools_registers_all_four_into_the_one_surface() {
        let mut cat = Catalogue { defs: vec![] };
        let registered = register_knowledge_tools(&mut cat).expect("seeded defs always admit");
        assert_eq!(
            registered.len(),
            4,
            "publish + edit_confidential + draft + comment"
        );

        let publish = cat
            .resolve(&ToolName(PUBLISH_TOOL.into()))
            .expect("publish registered");
        assert_eq!(publish.subsystem, KNOWLEDGE_SUBSYSTEM);
        assert!(publish.requires_approval, "the registered publish is gated");
        assert_eq!(publish.required_caps, vec!["page.publish".to_string()]);

        let edit = cat
            .resolve(&ToolName(EDIT_CONFIDENTIAL_TOOL.into()))
            .expect("edit_confidential registered");
        assert!(
            edit.requires_approval,
            "the registered edit_confidential is gated"
        );

        let draft = cat
            .resolve(&ToolName(DRAFT_TOOL.into()))
            .expect("draft registered");
        assert!(
            !draft.requires_approval,
            "the registered draft is NOT gated"
        );
        assert_eq!(draft.required_caps, vec!["page.draft".to_string()]);

        let comment = cat
            .resolve(&ToolName(COMMENT_TOOL.into()))
            .expect("comment registered");
        assert!(
            !comment.requires_approval,
            "the registered comment is NOT gated"
        );

        // a tool NOT registered resolves to None (the catalogue is exactly these four).
        assert!(cat
            .resolve(&ToolName("knowledge.delete_space".into()))
            .is_none());
    }

    /// **The no-silent-loosening guard (VISION §3) protects the registration path.** A `publish` def
    /// hand-loosened to `requires_approval = false` WITHOUT a written deviation is REJECTED LOUD —
    /// proving the registration seam can't silently un-gate the consequential publish.
    #[test]
    fn a_hand_loosened_publish_registration_is_rejected_loud() {
        let mut loosened = publish_tool_def();
        loosened.requires_approval = false;
        let err = assert_no_silent_loosening(&loosened, &[]).unwrap_err();
        assert_eq!(err.subsystem, "knowledge");
        assert_eq!(err.tool, "publish");
        assert!(
            err.to_string().contains("WITHOUT a written deviation"),
            "the loosening is surfaced LOUD: {err}"
        );
    }

    /// **The compounding-payoff / no-new-engine check (EI-03 §4 / EI-01 §7).** The KN producer tools
    /// are PURE data: every def is a `mutate` `ToolDef` whose gating is the frozen §6.3 seed and whose
    /// caps are the frozen 4.9 carrier — there is NO bespoke apply/gate machinery in this module (it
    /// constructs `ToolDef`s and registers them; the routing + gating + HITL are the existing
    /// pipeline). All four route the SAME way (`Mutate` → EffectApi) and differ ONLY in their frozen
    /// `requires_approval` seed.
    #[test]
    fn the_kn_tools_are_a_projection_not_a_new_engine() {
        let defs = knowledge_tool_defs();
        assert_eq!(defs.len(), 4);
        for d in &defs {
            assert_eq!(
                d.effect_kind,
                EffectKind::Mutate,
                "every KN producer tool routes through EffectApi (plan-then-apply) — no new path"
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
        // the gated set (publish, edit_confidential) and the un-gated set (draft, comment) split on
        // the frozen consequential line.
        assert!(
            defs[0].requires_approval && defs[1].requires_approval,
            "publish + edit gated"
        );
        assert!(
            !defs[2].requires_approval && !defs[3].requires_approval,
            "draft + comment not gated"
        );
    }
}
