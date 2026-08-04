use myelin_agent::{ToolDef, ToolSurface};
use myelin_content::rebac_fragment::object_types as kn_objects;
use myelin_content::rebac_fragment::{COMMENT, DRAFT, EDIT, PUBLISH};

use crate::defaults::{cap, mutate_tool_def, register_tool_defs, LooseningViolation};

pub const KNOWLEDGE_SUBSYSTEM: &str = "knowledge";

pub const PUBLISH_TOOL: &str = "publish";

pub const EDIT_CONFIDENTIAL_TOOL: &str = "edit_confidential";

pub const DRAFT_TOOL: &str = "draft";

pub const COMMENT_TOOL: &str = "comment";

pub const KNOWLEDGE_TOOL_VERSION: u32 = 1;

pub fn publish_required_caps() -> Vec<String> {
    cap(kn_objects::PAGE, PUBLISH)
}

pub fn edit_confidential_required_caps() -> Vec<String> {
    cap(kn_objects::PAGE, EDIT)
}

pub fn draft_required_caps() -> Vec<String> {
    cap(kn_objects::PAGE, DRAFT)
}

pub fn comment_required_caps() -> Vec<String> {
    cap(kn_objects::PAGE, COMMENT)
}

pub fn publish_tool_def() -> ToolDef {
    mutate_tool_def(
        KNOWLEDGE_SUBSYSTEM,
        PUBLISH_TOOL,
        KNOWLEDGE_TOOL_VERSION,
        r#"{"type":"object","required":["page"],"properties":{"page":{"type":"string"},"space":{"type":"string"}}}"#,
        publish_required_caps(),
    )
}

pub fn edit_confidential_tool_def() -> ToolDef {
    mutate_tool_def(
        KNOWLEDGE_SUBSYSTEM,
        EDIT_CONFIDENTIAL_TOOL,
        KNOWLEDGE_TOOL_VERSION,
        r#"{"type":"object","required":["page","blocks"],"properties":{"page":{"type":"string"},"blocks":{"type":"array"}}}"#,
        edit_confidential_required_caps(),
    )
}

pub fn draft_tool_def() -> ToolDef {
    mutate_tool_def(
        KNOWLEDGE_SUBSYSTEM,
        DRAFT_TOOL,
        KNOWLEDGE_TOOL_VERSION,
        r#"{"type":"object","required":["space"],"properties":{"space":{"type":"string"},"title":{"type":"string"},"blocks":{"type":"array"}}}"#,
        draft_required_caps(),
    )
}

pub fn comment_tool_def() -> ToolDef {
    mutate_tool_def(
        KNOWLEDGE_SUBSYSTEM,
        COMMENT_TOOL,
        KNOWLEDGE_TOOL_VERSION,
        r#"{"type":"object","required":["page","body"],"properties":{"page":{"type":"string"},"body":{"type":"string"}}}"#,
        comment_required_caps(),
    )
}

pub fn knowledge_tool_defs() -> Vec<ToolDef> {
    vec![
        publish_tool_def(),
        edit_confidential_tool_def(),
        draft_tool_def(),
        comment_tool_def(),
    ]
}

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

    #[test]
    fn publish_and_edit_confidential_are_gated_by_the_frozen_default() {
        for (def, tool) in [
            (publish_tool_def(), PUBLISH_TOOL),
            (edit_confidential_tool_def(), EDIT_CONFIDENTIAL_TOOL),
        ] {
            assert!(
                def.requires_approval,
                "knowledge.{tool} is HITL-gated (§6.3 - consequential)"
            );
            assert_eq!(
                def.requires_approval,
                requires_approval_default(KNOWLEDGE_SUBSYSTEM, tool),
                "knowledge.{tool}'s gating IS the frozen §6.3 default (seeded, not hand-set)"
            );
            assert_eq!(def.effect_kind, EffectKind::Mutate);
            assert!(def.side_effecting);
        }
    }

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
        assert_eq!(kn_objects::PAGE, "page");
    }

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

        assert!(cat
            .resolve(&ToolName("knowledge.delete_space".into()))
            .is_none());
    }

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

    #[test]
    fn the_kn_tools_are_a_projection_not_a_new_engine() {
        let defs = knowledge_tool_defs();
        assert_eq!(defs.len(), 4);
        for d in &defs {
            assert_eq!(
                d.effect_kind,
                EffectKind::Mutate,
                "every KN producer tool routes through EffectApi (plan-then-apply) - no new path"
            );
            assert!(d.side_effecting);
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating is the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
        }
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
