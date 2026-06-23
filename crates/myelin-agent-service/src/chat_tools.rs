//! # `chat_tools` — the per-consumer **Chat** ToolDefs registered into the ONE ToolSurface
//! (AG-P20 → P-347, M4): `post_message` / `react` (reversible, NOT gated) + the cross-subsystem
//! "governed where it LANDS" rule for any `EffectApi` tool a Chat run invokes against another subsystem
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §6.1 (ONE catalogue — every
//! subsystem registers typed [`ToolDef`]s into the ONE shared [`ToolSurface`]), §6.3 (**the FROZEN
//! `requires_approval` defaults table** — Chat `post_message` / `react` = **no** (reversible, cheap);
//! **any `EffectApi` tool that mutates ANOTHER subsystem inherits THAT subsystem's default** — "the
//! effect is governed where it lands, not where it's invoked"), §3.4 (**explicit-first dispatch** — an
//! `@agent` mention NOTIFIES via Notif's one inbox, it does NOT auto-spawn a costed run; the dispatch
//! tier owns that, [`crate::dispatch`]).
//!
//! **VISION §3** (suggest-by-default; reversible/cheap actions like a chat post/react are NOT gated —
//! they are recovered by editing/deleting). **EI-03 §7** (explicit-first — a mention is a notification,
//! not a costed run). **EI-01 §7** (the compounding payoff — this consumer surface is the SAME
//! registration shape as the Git/KN/Issues surfaces; the cross-subsystem rule reuses the AG-P8
//! [`requires_approval_for_landing`] — no new governance model).
//!
//! **Contract-index:** OWNS the Chat slice of **8.1** (`register_tool` — the two Chat consumer
//! ToolDefs). CONSUMES **4.9** (the Chat ReBAC fragment supplies the `required_caps` — the
//! `channel.post` permission name from [`myelin_chat::rebac_fragment`]) + the AG-P8 cross-subsystem
//! rule ([`crate::defaults::requires_approval_for_landing`], §6.3 last row). The `requires_approval`
//! column is SEEDED from the frozen §6.3 table via [`crate::defaults::seed_requires_approval`].
//!
//! ## What this prompt (AG-P20) ships — the Chat consumer ToolDefs (NO new engine)
//! - [`post_message_tool_def`] — `chat.post_message`: `effect_kind = mutate`, `side_effecting`,
//!   `requires_approval = no` (seeded from §6.3 — reversible), `required_caps = [channel.post]` (4.9).
//!   Applies DIRECTLY through the pipeline (no HITL gate). An agent-authored message is legible (the
//!   agent principal is `kind=agent`; never disguised as human — ADR-08 / AI-Act).
//! - [`react_tool_def`] — `chat.react`: reversible, NOT gated; `required_caps = [channel.post]` (4.9 —
//!   reacting is governed by the same `post` membership permission posting is).
//! - [`landing_requires_approval`] — the cross-subsystem "governed where it LANDS" rule (§6.3 last
//!   row): a Chat run that invokes an `EffectApi` tool mutating ANOTHER subsystem inherits THAT
//!   subsystem's frozen default (a chat-invoked `git.merge` is GATED — Git's default — NOT un-gated by
//!   Chat's `post_message` default). Thin re-export of [`requires_approval_for_landing`] pinned to the
//!   `chat` invoking subsystem.
//! - [`register_chat_tools`] — registers BOTH into a caller-supplied [`ToolSurface`] through the frozen
//!   seed + the no-silent-loosening guard (a tightened registration is admitted; a loosening of an
//!   inherited gated default is the LANDING subsystem's guard, never bypassed here).
//!
//! ## Explicit-first dispatch (§3.4) — the registration is data; the dispatch tier is the gate
//! Registering `post_message`/`react` does NOT make an `@agent` mention auto-spawn a costed run. The
//! mention→notify vs explicit-trigger→dispatch distinction is [`crate::dispatch`]'s (the typed
//! classifier): a casual mention NOTIFIES (0 spawn), only an explicit trigger dispatches (through the
//! reserve gate). These ToolDefs are the catalogue the dispatched run draws from once it IS running —
//! they are not themselves a dispatch trigger.
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **NONE for the Chat tools** — they are projections of the existing plan-then-apply path; the
//!   cross-subsystem rule is the AG-P8 frozen-defaults helper.
//! - **Implicit auto-dispatch on a casual mention remains [OPEN → LEGAL] (L-3, counsel-gated)** — GDPR
//!   Art. 22 / EU AI-Act human-oversight. Explicit-first is v1; NO auto-spawn path is wired (see
//!   [`crate::dispatch`]). Stated as the defensible posture.
//! - **The external MCP ENDPOINT** is the post-M5 follow-on (AG-P25); not MCP-exposed at v1.

use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};
use myelin_chat::rebac_fragment::object_types as chat_objects;

use crate::defaults::{
    assert_no_silent_loosening, requires_approval_for_landing, seed_requires_approval,
    LooseningViolation,
};

// ───────────────────────── the frozen Chat consumer-tool identity (the §6.3 keys) ────────────────

/// **The Chat subsystem token** — the `subsystem` half of the catalogue key + the key the FROZEN §6.3
/// defaults table is looked up under (`("chat", "post_message")` → not gated). Also the INVOKING
/// subsystem the cross-subsystem rule ([`landing_requires_approval`]) pins. The SINGLE source of truth.
pub const CHAT_SUBSYSTEM: &str = "chat";

/// **The `chat.post_message` tool name** (§6.3 — reversible, NOT gated). The seed keys on
/// `("chat", "post_message")`.
pub const POST_MESSAGE_TOOL: &str = "post_message";

/// **The `chat.react` tool name** (§6.3 — reversible, NOT gated). The seed keys on `("chat", "react")`.
pub const REACT_TOOL: &str = "react";

/// **The ToolDef version** the Chat consumer tools register at (forward-only; the catalogue key is
/// `(subsystem, name, version)`, §4.2). v1 is the first frozen shape.
pub const CHAT_TOOL_VERSION: u32 = 1;

// ───────────────────────── the required_caps from the Chat ReBAC fragment (4.9) ──────────────────

/// **The `required_caps` for `chat.post_message` / `chat.react` (CONSUMED from 4.9).** Posting (and
/// reacting) is governed by the `channel.post` permission the Chat frozen ReBAC fragment declares
/// ([`channel_fragment`](myelin_chat::rebac_fragment::channel_fragment): `post = member`) — an agent
/// may only post where it is a member, exactly as a human is. Built from the canonical `myelin-chat`
/// object-type constant so a rename in the fragment is a compile-or-test break here, never a silent
/// drift.
pub fn post_required_caps() -> Vec<String> {
    vec![format!("{}.post", chat_objects::CHANNEL)]
}

// ───────────────────────── the two Chat consumer ToolDefs (8.1 — the OWNED registration) ──────────

/// Build a reversible, NON-gated Chat ToolDef (post_message / react) — a `Mutate` tool that routes
/// through the plan-then-apply path (cap-checked, metered) but is NOT HITL-gated (reversible/cheap,
/// §6.3 → `requires_approval = no`, seeded). `required_caps = [channel.post]` (4.9).
fn chat_tool_def(name: &str, input_schema: &str) -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(name.to_string()),
        subsystem: CHAT_SUBSYSTEM.to_string(),
        version: CHAT_TOOL_VERSION,
        input_schema: input_schema.to_string(),
        required_caps: post_required_caps(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED below from §6.3 (a reversible chat action is NOT gated).
        requires_approval: false,
        exposed_over_mcp: false,
    })
}

/// **The `chat.post_message` ToolDef (8.1) — the reversible, NON-gated Chat producer tool (§6.3).** An
/// agent posts a message (legibly labelled `kind=agent`). Reversible (edit/delete) → NOT gated; routes
/// through [`EffectApi::apply`](myelin_agent::EffectApi).
pub fn post_message_tool_def() -> ToolDef {
    chat_tool_def(
        POST_MESSAGE_TOOL,
        r#"{"type":"object","required":["channel","body"],"properties":{"channel":{"type":"string"},"body":{"type":"string"},"thread":{"type":"string"}}}"#,
    )
}

/// **The `chat.react` ToolDef (8.1) — the reversible, NON-gated reaction tool (§6.3).** An agent reacts
/// to a message. Reversible (un-react) → NOT gated; routes through `EffectApi::apply`.
pub fn react_tool_def() -> ToolDef {
    chat_tool_def(
        REACT_TOOL,
        r#"{"type":"object","required":["channel","message","emoji"],"properties":{"channel":{"type":"string"},"message":{"type":"string"},"emoji":{"type":"string"}}}"#,
    )
}

// ───────────────────────── the cross-subsystem "governed where it lands" rule (§6.3 last row) ─────

/// **The cross-subsystem rule — a Chat-invoked `EffectApi` tool that mutates ANOTHER subsystem
/// inherits THAT subsystem's frozen default (§6.3 last row — "governed where it LANDS").** A Chat run
/// can invoke any `EffectApi` tool; the gate for that effect is the LANDING subsystem's default, NOT
/// Chat's. A chat-invoked `git.merge` is GATED (Git's `yes`), NOT un-gated by Chat's `post_message`
/// `no`. Thin pin of [`requires_approval_for_landing`] to the `chat` invoking subsystem (the AG-P8
/// helper is the single source of truth — no second governance model).
pub fn landing_requires_approval(landing_subsystem: &str, tool: &str) -> bool {
    requires_approval_for_landing(CHAT_SUBSYSTEM, landing_subsystem, tool)
}

/// **The two Chat consumer ToolDefs, in catalogue order (post_message → react).** The single list
/// every registration + CDC consumes (one source of truth). Both SEEDED from the frozen §6.3 defaults
/// (NOT gated).
pub fn chat_tool_defs() -> Vec<ToolDef> {
    vec![post_message_tool_def(), react_tool_def()]
}

// ───────────────────────── the registration seam (8.1 — into the ONE ToolSurface) ────────────────

/// **Register the Chat consumer ToolDefs into the ONE [`ToolSurface`] (8.1 / §6.1) — the OWNED
/// deliverable.** Every def is passed through the VISION §3 no-silent-loosening guard FIRST. The Chat
/// tools are frozen `no`, so the guard always admits them (a tightening would also be admitted); the
/// guard is the structural ratchet. Identical in shape to the Git/KN/Issues registrations — the
/// compounding-payoff reuse.
pub fn register_chat_tools<S: ToolSurface>(
    surface: &mut S,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    let defs = chat_tool_defs();
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

    /// **`post_message` + `react` carry the FROZEN §6.3 `requires_approval = no` default (reversible)
    /// — seeded, not hand-set.** They apply DIRECTLY through the pipeline (no HITL gate).
    #[test]
    fn post_message_and_react_are_reversible_not_gated() {
        for (def, tool) in [
            (post_message_tool_def(), POST_MESSAGE_TOOL),
            (react_tool_def(), REACT_TOOL),
        ] {
            assert!(
                !def.requires_approval,
                "chat.{tool} is reversible → NOT gated (§6.3)"
            );
            assert_eq!(
                def.requires_approval,
                requires_approval_default(CHAT_SUBSYSTEM, tool),
                "chat.{tool}'s (non-)gating IS the frozen §6.3 default (seeded, not hand-set)"
            );
            assert_eq!(def.effect_kind, EffectKind::Mutate);
            assert!(def.side_effecting);
        }
    }

    /// **The `required_caps` come from the FROZEN Chat ReBAC fragment (4.9), not invented here.** Both
    /// tools → `channel.post`. Built from the canonical `myelin-chat` object-type constant, so a
    /// rename breaks this test (no silent drift).
    #[test]
    fn required_caps_are_the_chat_rebac_fragment_permissions() {
        assert_eq!(
            post_message_tool_def().required_caps,
            vec!["channel.post".to_string()]
        );
        assert_eq!(
            react_tool_def().required_caps,
            vec!["channel.post".to_string()]
        );
        assert_eq!(chat_objects::CHANNEL, "channel");
    }

    /// **The cross-subsystem rule (§6.3 last row) — a Chat-invoked effect is "governed where it
    /// LANDS".** A chat-invoked `git.merge` inherits Git's GATED default; a chat-invoked
    /// `issues.forecast` inherits Issues' advisory (NOT gated) default. The invoking subsystem (chat)
    /// is irrelevant to the gate.
    #[test]
    fn cross_subsystem_effect_is_governed_where_it_lands() {
        // chat → git.merge LANDS in git → Git's GATED default (NOT Chat's un-gated post default).
        assert!(
            landing_requires_approval("git", "merge"),
            "a chat-invoked git.merge is governed where it LANDS (git → gated)"
        );
        // chat → knowledge.publish LANDS in knowledge → KN's GATED default.
        assert!(
            landing_requires_approval("knowledge", "publish"),
            "a chat-invoked knowledge.publish lands in knowledge → gated"
        );
        // chat → issues.forecast LANDS in issues → Issues' advisory (NOT gated) default.
        assert!(
            !landing_requires_approval("issues", "forecast"),
            "a chat-invoked issues.forecast lands in issues → advisory (NOT gated)"
        );
        // a chat-invoked chat.post_message lands in chat → its OWN (un-gated) default.
        assert!(
            !landing_requires_approval(CHAT_SUBSYSTEM, POST_MESSAGE_TOOL),
            "a chat post lands in chat → its own un-gated default"
        );
    }

    /// **`register_chat_tools` registers BOTH consumer ToolDefs into the ONE catalogue (8.1 / §6.1)
    /// and they resolve by name with their frozen shapes.**
    #[test]
    fn register_chat_tools_registers_both_into_the_one_surface() {
        let mut cat = Catalogue { defs: vec![] };
        let registered = register_chat_tools(&mut cat).expect("seeded defs always admit");
        assert_eq!(registered.len(), 2, "post_message + react");

        let post = cat
            .resolve(&ToolName(POST_MESSAGE_TOOL.into()))
            .expect("post_message registered");
        assert_eq!(post.subsystem, CHAT_SUBSYSTEM);
        assert!(!post.requires_approval, "the registered post is NOT gated");
        assert_eq!(post.required_caps, vec!["channel.post".to_string()]);

        // a tool NOT registered resolves to None.
        assert!(cat
            .resolve(&ToolName("chat.delete_channel".into()))
            .is_none());
    }

    /// **The compounding-payoff / no-new-engine check (EI-03 §4 / EI-01 §7).** Both Chat consumer
    /// tools are PURE data: a `mutate` `ToolDef` whose gating is the frozen §6.3 seed and whose caps
    /// are the frozen 4.9 fragment — no bespoke apply/gate machinery; the cross-subsystem rule reuses
    /// the AG-P8 helper.
    #[test]
    fn the_chat_tools_are_a_projection_not_a_new_engine() {
        let defs = chat_tool_defs();
        assert_eq!(defs.len(), 2);
        for d in &defs {
            assert_eq!(d.effect_kind, EffectKind::Mutate);
            assert!(d.side_effecting);
            assert!(
                !d.requires_approval,
                "both Chat tools are reversible → NOT gated"
            );
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating is the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
        }
    }
}
