use super::*;

#[test]
fn catalogue_advertises_only_the_executable_post_effect() {
    let definitions = chat_tool_defs();
    assert_eq!(definitions.len(), 1);

    let post = &definitions[0];
    post.validate().unwrap();
    assert_eq!(post.canonical_name(), "chat.post");
    assert_eq!(post.required_caps, ["chat.post"]);
    assert_eq!(post.effect_kind, EffectKind::Mutate);
    assert!(post.side_effecting);
    assert!(!post.requires_approval);
    assert!(post.exposed_over_mcp);
}

#[test]
fn unknown_chat_mutations_remain_fail_closed() {
    for unavailable in [
        "reply_in_thread",
        "react",
        "start_dm",
        "create_channel",
        "invite",
        "archive_channel",
    ] {
        assert!(requires_approval_default(unavailable));
        assert!(chat_tool_defs()
            .iter()
            .all(|definition| definition.name.0 != unavailable));
    }
}
