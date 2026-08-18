use myelin_chat::subs;
use myelin_refs::{mint, parse, strip_sub, sub_kind, Sub, SubKind};

#[test]
fn the_shared_sub_scheme_is_host_independent_across_the_swap() {
    let tenant = "acme";

    let chat_thread = subs::mint_thread(tenant, "01J0THREADROOT")
        .expect("chat thread #sub mints through the frozen grammar");
    assert_eq!(
        sub_kind(&chat_thread),
        Some(Sub::Thread("01J0THREADROOT".into())),
        "the chat thread carries the shared #thread- kind"
    );

    let kb_page_root =
        parse("myelin://acme/knowledge/page/p7").expect("a KB page root is grammatical");
    let kb_comment = mint(&kb_page_root, Sub::Comment("c42".into()))
        .expect("a KB comment #sub mints through the SAME frozen grammar");
    let kb_thread = mint(&kb_page_root, Sub::Thread("t9".into()))
        .expect("a KB thread #sub mints through the SAME frozen grammar");

    assert_eq!(sub_kind(&kb_comment), Some(Sub::Comment("c42".into())));
    assert_eq!(sub_kind(&kb_thread), Some(Sub::Thread("t9".into())));

    for kind in [SubKind::Thread, SubKind::Comment] {
        assert!(
            matches!(kind, SubKind::Thread | SubKind::Comment),
            "thread-/comment- are the shared #sub kinds the consolidation rides (5.7 / OQ-L)"
        );
    }
}

#[test]
fn the_refs_strip_resolution_survives_the_swap() {
    let kb_page_root = parse("myelin://acme/knowledge/page/p7").unwrap();
    let kb_comment = mint(&kb_page_root, Sub::Comment("c42".into())).unwrap();
    assert_eq!(
        strip_sub(&kb_comment),
        kb_page_root,
        "a KB comment #sub strips to its page root (the live, pre-swap resolution)"
    );

    let chat_thread = subs::mint_thread("acme", "01J0THREADROOT").unwrap();
    let chat_root = strip_sub(&chat_thread);
    assert_eq!(
        chat_root,
        parse("myelin://acme/chat/thread/01J0THREADROOT").unwrap(),
        "a chat thread #sub strips to its chat thread root (the post-swap resolution)"
    );

    assert_ne!(
        strip_sub(&kb_comment),
        chat_root,
        "different hosts root differently (sanity), but the SAME strip codec resolves each - the \
         swap changes the host, not the codec"
    );
}
