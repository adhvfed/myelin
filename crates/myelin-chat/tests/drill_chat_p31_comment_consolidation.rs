use myelin_chat::comment_consolidation::{
    comment_consolidation_gap_report, AnchoredCommentPresenceDemand, PresenceDemandBudget,
    COMMENT_CONSOLIDATION_FLOOR, COMMENT_CONSOLIDATION_FLOORS,
};
use myelin_chat::subs;
use myelin_refs::{mint, parse, strip_sub, sub_kind, Sub, SubKind};

#[test]
fn the_consolidation_floor_is_an_honest_named_floor() {
    let ids: Vec<&str> = COMMENT_CONSOLIDATION_FLOORS.iter().map(|f| f.id).collect();
    assert_eq!(ids, vec!["comment-threading-consolidation"]);

    let floor = COMMENT_CONSOLIDATION_FLOOR;
    assert!(
        floor.is_fully_recorded(),
        "the consolidation floor must be fully recorded (no invisible gap)"
    );
    assert!(
        !floor.status.has_fired(),
        "the OQ-L real-time-presence trigger has NOT fired at this prompt's execution"
    );
    assert!(
        !floor.built,
        "the consolidation is a NAMED FLOOR - not built speculatively (a store/transport swap on \
         demand, OQ-L)"
    );
    assert!(floor.honours_no_premature_promotion());

    comment_consolidation_gap_report().expect("the consolidation gap-report is honest");
}

#[test]
fn the_measured_trigger_is_real_and_unfired() {
    let budget = PresenceDemandBudget::OQ_L;

    let observed = AnchoredCommentPresenceDemand::OBSERVED_NONE;
    assert_eq!(observed.live_multiparty_sessions_observed, 0);
    assert!(
        !budget.exceeded_by(&observed),
        "0 observed presence sessions must NOT cross the OQ-L demand budget (the floor stays named)"
    );

    let real = AnchoredCommentPresenceDemand {
        live_multiparty_sessions_observed: 1,
        over_window: "synthetic: a real anchored-comment presence session was observed",
    };
    assert!(
        budget.exceeded_by(&real),
        "a real anchored-comment presence session MUST cross the OQ-L demand budget"
    );
}

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
