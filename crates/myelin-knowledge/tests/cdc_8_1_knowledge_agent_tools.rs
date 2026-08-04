use myelin_agent::EffectKind;
use myelin_agent_service::knowledge_tools as fabric;
use myelin_knowledge::agent as kn;

#[test]
fn cdc_8_1_kn_tool_names_agree_with_the_fabric_registration() {
    assert_eq!(kn::KNOWLEDGE_SUBSYSTEM, fabric::KNOWLEDGE_SUBSYSTEM);
    assert_eq!(kn::PUBLISH_TOOL, fabric::PUBLISH_TOOL);
    assert_eq!(kn::EDIT_CONFIDENTIAL_TOOL, fabric::EDIT_CONFIDENTIAL_TOOL);
    assert_eq!(kn::DRAFT_TOOL, fabric::DRAFT_TOOL);
    assert_eq!(kn::COMMENT_TOOL, fabric::COMMENT_TOOL);
}

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
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
    }
    assert!(kn::requires_approval_default(kn::PUBLISH_TOOL));
    assert!(kn::requires_approval_default(kn::EDIT_CONFIDENTIAL_TOOL));
    assert!(!kn::requires_approval_default(kn::DRAFT_TOOL));
    assert!(!kn::requires_approval_default(kn::COMMENT_TOOL));
}
