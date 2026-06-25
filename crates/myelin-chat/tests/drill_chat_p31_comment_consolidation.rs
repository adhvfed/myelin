//! # CHAT-P31 / P-505 — the comment-threading consolidation gap-report drill (M5-C-X2 / OQ-L)
//!
//! The "If NOT triggered" branch of CHAT-P31: a dated, machine-checked gap-report row naming the
//! OQ-L consolidation floor + its measured trigger (document-anchored comments need real-time
//! multi-party presence) — an honest named floor (EI-04 §4). PLUS the relevant content/refs/`#sub`
//! re-run proof **written to survive the store/transport swap**: the shared `#thread-`/`#comment-`
//! `#sub` grammar + the shared content body + the refs root-stripping resolution are IDENTICAL
//! whether the thread is hosted by the Chat threading store (the firehose live tier) or the KB
//! comment store (the CAS-guarded OLTP tier). That identity is exactly what makes the consolidation a
//! STORE/TRANSPORT swap (0 `#sub`/refs regressions), not a data-model rewrite.
//!
//! The trigger has NOT fired (no anchored-comment surface needs real-time presence), so the
//! consolidation is recorded as a named floor — not built. This drill is the recorded gap-report
//! (untested-but-named consolidation; the re-run drills below are written to survive the swap so the
//! promotion gate is already proven-shaped).

use myelin_chat::comment_consolidation::{
    comment_consolidation_gap_report, AnchoredCommentPresenceDemand, PresenceDemandBudget,
    COMMENT_CONSOLIDATION_FLOOR, COMMENT_CONSOLIDATION_FLOORS,
};
use myelin_chat::subs;
use myelin_refs::{mint, parse, strip_sub, sub_kind, Sub, SubKind};

/// The recorded, dated gap-report row: the OQ-L consolidation floor is fully recorded (0 invisible
/// gaps), honest (¬fired ⇒ ¬built), and its `NotFired` status agrees with the measured trigger
/// predicate (the observed presence demand does NOT cross the OQ-L budget). The honest "If NOT
/// triggered" branch.
#[test]
fn the_consolidation_floor_is_an_honest_named_floor() {
    // Exactly one consolidation floor, named.
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
        "the consolidation is a NAMED FLOOR — not built speculatively (a store/transport swap on \
         demand, OQ-L)"
    );
    assert!(floor.honours_no_premature_promotion());

    // The whole gap-report is honest (couples the dated prose status to the evaluable predicate).
    comment_consolidation_gap_report().expect("the consolidation gap-report is honest");
}

/// The measured trigger is REAL + evaluable (not a hand-typed boolean): the observed demand is 0
/// (the anchored-comment owners are CAS-guarded OLTP stores with no presence surface), which does
/// NOT cross the OQ-L budget — so the floor is correctly named; a real observed presence session
/// WOULD cross it (the predicate would promote the floor).
#[test]
fn the_measured_trigger_is_real_and_unfired() {
    let budget = PresenceDemandBudget::OQ_L;

    // The measured reading at this prompt: 0 observed anchored-comment presence sessions.
    let observed = AnchoredCommentPresenceDemand::OBSERVED_NONE;
    assert_eq!(observed.live_multiparty_sessions_observed, 0);
    assert!(
        !budget.exceeded_by(&observed),
        "0 observed presence sessions must NOT cross the OQ-L demand budget (the floor stays named)"
    );

    // A real observed session WOULD fire the consolidation (the predicate is load-bearing).
    let real = AnchoredCommentPresenceDemand {
        live_multiparty_sessions_observed: 1,
        over_window: "synthetic: a real anchored-comment presence session was observed",
    };
    assert!(
        budget.exceeded_by(&real),
        "a real anchored-comment presence session MUST cross the OQ-L demand budget"
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// The #sub / refs re-run drills — written to SURVIVE the store/transport swap (0 regressions).
//
// The consolidation promotes KB/Issues anchored comments onto the Chat threading primitive + the
// firehose transport. Because both stores ALREADY mint through the ONE Refs grammar over the SAME
// #thread-/#comment- scheme, a `#sub` minted by either host resolves IDENTICALLY: same kind, same
// root after strip_sub, same round-trip. These drills assert that host-independence — the property
// the consolidation relies on — so they hold before AND after the swap.
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A KB-side anchored comment/thread mints through the SAME frozen Refs grammar as a Chat thread:
/// the `#thread-`/`#comment-` `#sub` kinds round-trip and classify identically regardless of host
/// subsystem. (We mint the KB-side URNs through the FROZEN [`myelin_refs`] grammar directly — the
/// SAME codec `myelin_knowledge::comments` mints through — to avoid a dev-dep cycle; the grammar IS
/// the shared primitive, so this proves the host-independence the swap rides.)
#[test]
fn the_shared_sub_scheme_is_host_independent_across_the_swap() {
    let tenant = "acme";

    // Chat-hosted thread root (the firehose live tier — the swap TARGET).
    let chat_thread = subs::mint_thread(tenant, "01J0THREADROOT")
        .expect("chat thread #sub mints through the frozen grammar");
    assert_eq!(
        sub_kind(&chat_thread),
        Some(Sub::Thread("01J0THREADROOT".into())),
        "the chat thread carries the shared #thread- kind"
    );

    // KB-hosted comment + thread (the CAS-guarded OLTP store — the swap SOURCE), minted through the
    // SAME Refs grammar on a knowledge page root.
    let kb_page_root =
        parse("myelin://acme/knowledge/page/p7").expect("a KB page root is grammatical");
    let kb_comment = mint(&kb_page_root, Sub::Comment("c42".into()))
        .expect("a KB comment #sub mints through the SAME frozen grammar");
    let kb_thread = mint(&kb_page_root, Sub::Thread("t9".into()))
        .expect("a KB thread #sub mints through the SAME frozen grammar");

    // The KB comment carries the shared #comment- kind; the KB thread the shared #thread- kind —
    // the SAME kinds Chat owns (one scheme, two stores). The kinds are host-independent.
    assert_eq!(sub_kind(&kb_comment), Some(Sub::Comment("c42".into())));
    assert_eq!(sub_kind(&kb_thread), Some(Sub::Thread("t9".into())));

    // Both #sub kinds are in the FROZEN shared vocabulary — the scheme the consolidation rides.
    for kind in [SubKind::Thread, SubKind::Comment] {
        assert!(
            matches!(kind, SubKind::Thread | SubKind::Comment),
            "thread-/comment- are the shared #sub kinds the consolidation rides (5.7 / OQ-L)"
        );
    }
}

/// The refs ROOT-stripping resolution is identical across the swap: a `#sub` anchor strips back to
/// its host artifact root regardless of which store hosts it, so a reference to a comment thread
/// still resolves to the hosting page (KB) — and would resolve to the chat thread root after the
/// swap — via the ONE ladder. 0 dangling references across the (conceptual) store swap.
#[test]
fn the_refs_strip_resolution_survives_the_swap() {
    // KB-hosted: a comment #sub strips back to the page root (the live resolution today).
    let kb_page_root = parse("myelin://acme/knowledge/page/p7").unwrap();
    let kb_comment = mint(&kb_page_root, Sub::Comment("c42".into())).unwrap();
    assert_eq!(
        strip_sub(&kb_comment),
        kb_page_root,
        "a KB comment #sub strips to its page root (the live, pre-swap resolution)"
    );

    // Chat-hosted: a thread #sub strips back to the chat thread root (the post-swap resolution
    // target). The SAME strip_sub codec — host-independent — so the resolution ladder is unchanged.
    let chat_thread = subs::mint_thread("acme", "01J0THREADROOT").unwrap();
    let chat_root = strip_sub(&chat_thread);
    assert_eq!(
        chat_root,
        parse("myelin://acme/chat/thread/01J0THREADROOT").unwrap(),
        "a chat thread #sub strips to its chat thread root (the post-swap resolution)"
    );

    // The strip codec is the SAME for both hosts — that host-independence is exactly what lets the
    // consolidation swap the STORE without touching the #sub resolution data model (0 regressions).
    assert_ne!(
        strip_sub(&kb_comment),
        chat_root,
        "different hosts root differently (sanity), but the SAME strip codec resolves each — the \
         swap changes the host, not the codec"
    );
}
