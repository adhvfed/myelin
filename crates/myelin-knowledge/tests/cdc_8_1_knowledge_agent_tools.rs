//! # CDC 8.1 (Knowledge slice) — the KN tool identity + `required_caps` + frozen §6.3 gate
//! classification AGREE with the Fabric registration (KN-P27 → P-317, M3)
//!
//! Contract 8.1 (`ToolSurface::register_tool(ToolDef)`): every subsystem contributes typed `ToolDef`s
//! into the ONE catalogue. The Knowledge-domain SOURCE OF TRUTH lives in `myelin_knowledge::agent`
//! (the tool identity constants, the `required_caps` built from the FROZEN `myelin-content` ReBAC
//! carrier, the FROZEN §6.3 consequential-gate classification); the THIN `ToolDef` REGISTRATION lives
//! in `myelin_agent_service::knowledge_tools` (the §2.9 DAG — `myelin-knowledge` cannot depend on the
//! Fabric). This CDC pins the two halves: the registered ToolDefs MUST carry the KN-domain caps + the
//! KN-domain gate classification byte-for-byte. A rename or a gate-drift on either side is a
//! compile/test break here, never a silent divergence (the `cdc_8_1` / git_tools precedent).
//!
//! **CDC pair (8.1, Knowledge slice).** PROVIDER side: `myelin_agent_service::knowledge_tools` — the
//! Fabric builds the registered `ToolDef`s (the catalogue rows). CONSUMER side:
//! `myelin_knowledge::agent` — the KN-domain source of truth (tool identity + caps + gate) the
//! registration is built FROM and that the live KN public endpoint reads. This test asserts the
//! provider's registered defs carry the consumer's caps + gate classification byte-for-byte.

use myelin_agent::EffectKind;
use myelin_agent_service::knowledge_tools as fabric;
use myelin_knowledge::agent as kn;

/// **The tool-name constants AGREE** — the KN-domain identity and the Fabric registration key on the
/// SAME `(subsystem, name)` tokens.
#[test]
fn cdc_8_1_kn_tool_names_agree_with_the_fabric_registration() {
    assert_eq!(kn::KNOWLEDGE_SUBSYSTEM, fabric::KNOWLEDGE_SUBSYSTEM);
    assert_eq!(kn::PUBLISH_TOOL, fabric::PUBLISH_TOOL);
    assert_eq!(kn::EDIT_CONFIDENTIAL_TOOL, fabric::EDIT_CONFIDENTIAL_TOOL);
    assert_eq!(kn::DRAFT_TOOL, fabric::DRAFT_TOOL);
    assert_eq!(kn::COMMENT_TOOL, fabric::COMMENT_TOOL);
}

/// **The `required_caps` AGREE** — the Fabric ToolDefs carry exactly the KN-domain caps (both sourced
/// from the frozen `myelin-content` ReBAC carrier; a rename in the carrier breaks BOTH).
#[test]
fn cdc_8_1_kn_required_caps_agree_with_the_registered_tool_defs() {
    assert_eq!(
        fabric::publish_tool_def().required_caps,
        kn::publish_required_caps()
    );
    assert_eq!(
        fabric::edit_confidential_tool_def().required_caps,
        kn::edit_confidential_required_caps()
    );
    assert_eq!(
        fabric::draft_tool_def().required_caps,
        kn::draft_required_caps()
    );
    assert_eq!(
        fabric::comment_tool_def().required_caps,
        kn::comment_required_caps()
    );
}

/// **The FROZEN §6.3 consequential-gate classification AGREES** — the registered ToolDef's
/// `requires_approval` equals the KN-domain frozen default for every shared tool. This is the X-6
/// joint freeze: KN publish/confidential = yes; draft/comment = no, agreed on BOTH sides.
#[test]
fn cdc_8_1_kn_gate_classification_agrees_with_the_registered_tool_defs() {
    for (def, tool) in [
        (fabric::publish_tool_def(), kn::PUBLISH_TOOL),
        (
            fabric::edit_confidential_tool_def(),
            kn::EDIT_CONFIDENTIAL_TOOL,
        ),
        (fabric::draft_tool_def(), kn::DRAFT_TOOL),
        (fabric::comment_tool_def(), kn::COMMENT_TOOL),
    ] {
        assert_eq!(
            def.requires_approval,
            kn::requires_approval_default(tool),
            "knowledge.{tool}: the registered ToolDef gate MUST equal the KN-domain frozen §6.3 default"
        );
        // every KN producer tool routes through EffectApi (plan-then-apply) — a mutate effect.
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
    }
    // the consequential ones are gated; the reversible ones are not (the frozen split).
    assert!(kn::requires_approval_default(kn::PUBLISH_TOOL));
    assert!(kn::requires_approval_default(kn::EDIT_CONFIDENTIAL_TOOL));
    assert!(!kn::requires_approval_default(kn::DRAFT_TOOL));
    assert!(!kn::requires_approval_default(kn::COMMENT_TOOL));
}
