use myelin_agent::{ToolDef, ToolSurface};
use myelin_chat::rebac_fragment::object_types as chat_objects;

use crate::defaults::{
    cap, mutate_tool_def, register_tool_defs, requires_approval_for_landing, LooseningViolation,
};

pub const CHAT_SUBSYSTEM: &str = "chat";

pub const POST_MESSAGE_TOOL: &str = "post_message";

pub const REACT_TOOL: &str = "react";

pub const CHAT_TOOL_VERSION: u32 = 1;

pub fn post_required_caps() -> Vec<String> {
    cap(chat_objects::CHANNEL, "post")
}

fn chat_tool_def(name: &str, input_schema: &str) -> ToolDef {
    mutate_tool_def(
        CHAT_SUBSYSTEM,
        name,
        CHAT_TOOL_VERSION,
        input_schema,
        post_required_caps(),
    )
}

pub fn post_message_tool_def() -> ToolDef {
    chat_tool_def(
        POST_MESSAGE_TOOL,
        r#"{"type":"object","required":["channel","body"],"properties":{"channel":{"type":"string"},"body":{"type":"string"},"thread":{"type":"string"}}}"#,
    )
}

pub fn react_tool_def() -> ToolDef {
    chat_tool_def(
        REACT_TOOL,
        r#"{"type":"object","required":["channel","message","emoji"],"properties":{"channel":{"type":"string"},"message":{"type":"string"},"emoji":{"type":"string"}}}"#,
    )
}

pub fn landing_requires_approval(landing_subsystem: &str, tool: &str) -> bool {
    requires_approval_for_landing(CHAT_SUBSYSTEM, landing_subsystem, tool)
}

pub fn chat_tool_defs() -> Vec<ToolDef> {
    vec![post_message_tool_def(), react_tool_def()]
}

pub fn register_chat_tools<S: ToolSurface>(
    surface: &mut S,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    register_tool_defs(surface, chat_tool_defs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::requires_approval_default;
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

    #[test]
    fn cross_subsystem_effect_is_governed_where_it_lands() {
        assert!(
            landing_requires_approval("git", "merge"),
            "a chat-invoked git.merge is governed where it LANDS (git → gated)"
        );
        assert!(
            landing_requires_approval("knowledge", "publish"),
            "a chat-invoked knowledge.publish lands in knowledge → gated"
        );
        assert!(
            landing_requires_approval("issues", "close"),
            "a chat-invoked issues.close lands in issues → gated"
        );
        assert!(
            !landing_requires_approval(CHAT_SUBSYSTEM, POST_MESSAGE_TOOL),
            "a chat post lands in chat → its own un-gated default"
        );
    }

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

        assert!(cat
            .resolve(&ToolName("chat.delete_channel".into()))
            .is_none());
    }

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
