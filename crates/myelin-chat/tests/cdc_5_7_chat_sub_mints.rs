use myelin_chat::subs::{mint_message, mint_thread, register_chat_sub_kinds, CHAT_OWNED_SUB_KINDS};
use myelin_refs::{format, strip_sub, sub_kind, ArtifactRef, SubKind};

fn provider_mints() -> Vec<(SubKind, ArtifactRef)> {
    vec![
        (
            SubKind::Message,
            mint_message("acme-eu", "01J0MSGULID").expect("message mint is grammatical"),
        ),
        (
            SubKind::Thread,
            mint_thread("acme-eu", "01J0THRROOT").expect("thread mint is grammatical"),
        ),
    ]
}

fn consumer_classifies(r: &ArtifactRef) -> Option<SubKind> {
    let reparsed = myelin_refs::parse(&format(r)).ok()?;
    assert_eq!(format(&reparsed), format(r), "minted ref must be canonical");
    sub_kind(&reparsed).map(|s| s.kind())
}

#[test]
fn cdc_5_7_chat_provider_mints_consumer_accepts_and_classifies_every_kind() {
    let reg = register_chat_sub_kinds().expect("Refs must ACCEPT chat's #sub kind registration");
    assert_eq!(reg.subsystem, "chat");
    assert_eq!(reg.kinds, CHAT_OWNED_SUB_KINDS.to_vec());

    for (declared, minted) in provider_mints() {
        assert_eq!(
            consumer_classifies(&minted),
            Some(declared),
            "Refs wrongly classified chat's mint `{}` (declared {declared:?})",
            format(&minted)
        );
        let root = strip_sub(&minted);
        assert!(
            !format(&root).contains('#'),
            "stripped root still carries a `#sub`: `{}`",
            format(&root)
        );
        assert!(
            myelin_refs::parse(&format(&root)).is_ok(),
            "stripped root `{}` must itself be a parseable canonical root",
            format(&root)
        );
    }
}

#[test]
fn cdc_5_7_consumer_rejects_a_malformed_chat_mint_loudly() {
    assert!(mint_message("acme", "").is_err());
    assert!(mint_thread("acme", "").is_err());
}

#[test]
fn cdc_5_7_chat_registers_only_its_own_kinds() {
    let reg = register_chat_sub_kinds().expect("registration accepted");
    for k in &reg.kinds {
        assert!(
            matches!(k, SubKind::Message | SubKind::Thread),
            "chat registered a non-chat-owned #sub kind `{k:?}`"
        );
    }
    for foreign in [
        SubKind::Comment,
        SubKind::LineRange,
        SubKind::Block,
        SubKind::Heading,
        SubKind::Row,
        SubKind::Field,
        SubKind::Check,
        SubKind::Step,
    ] {
        assert!(
            !reg.kinds.contains(&foreign),
            "chat must not register the foreign kind {foreign:?}"
        );
    }
}

#[test]
fn cdc_5_7_chat_sub_is_stable_across_edits() {
    let before = mint_message("acme", "01J0STABLE").expect("mint");
    let after = mint_message("acme", "01J0STABLE").expect("re-mint after an edit");
    assert_eq!(
        before, after,
        "the #sub must be stable across edits (the message_id is immutable, §2)"
    );
}
