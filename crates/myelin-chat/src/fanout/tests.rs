//! Unit + structural tests for the fanout-class boundary WIRED to behaviour + Activity-as-view
//! (CHAT-P17 / P-412). The CDC pair (the REAL Notif `list_inbox` Activity view + the REAL Identity
//! `list_subjects` watcher resolution) lives in `tests/cdc_7_1_7_6_4_4_chat_fanout.rs`.

use super::*;
use crate::events::{
    CHAT_CHANNEL_CREATED, CHAT_MESSAGE_CREATED, CHAT_MESSAGE_MENTIONED, CHAT_READ_STATE_UPDATED,
    CHAT_THREAD_REPLIED,
};
use crate::glue::FanoutClass;

use std::cell::RefCell;

use myelin_identity::{Consistency, ConsistencyMode, ObjectId, PrincipalId, Zookie};

fn pid(s: &str) -> PrincipalId {
    PrincipalId(s.into())
}

fn strong() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

/// A counting write-fanout sink — records every per-recipient Signal it is handed. The
/// celebrity-fanout property is "this sink saw 0 emits for an ambient post".
#[derive(Default)]
struct CountingSignalSink {
    emitted: RefCell<Vec<Signal>>,
}
impl SignalSink for CountingSignalSink {
    fn emit_signal(&self, signal: &Signal) {
        self.emitted.borrow_mut().push(signal.clone());
    }
}
impl CountingSignalSink {
    fn count(&self) -> usize {
        self.emitted.borrow().len()
    }
}

/// An in-memory `list_subjects(channel, watcher)` model (the read-fanout port) — DB-free. Returns
/// the configured watcher set for the channel; resolving it materialises NO inbox item.
struct InMemoryWatchers {
    watchers: Vec<PrincipalId>,
}
impl WatcherDirectory for InMemoryWatchers {
    fn list_watchers(&self, _channel: &ObjectId, _at: &Consistency) -> Vec<PrincipalId> {
        self.watchers.clone()
    }
}

// ───────────────────────────── the write-fanout-vs-read-fanout class decision ─────────────────────

/// **The class decision is correct: a mention/reply → WRITE-FANOUT; channel/thread activity →
/// READ-FANOUT; 0 misclassified events (the prompt gate).** This wires the static
/// [`crate::glue::fanout_class`] decision to the [`fanout_behaviour`] behaviour and asserts each
/// token lands in the right arm.
#[test]
fn the_write_fanout_vs_read_fanout_class_decision_is_correct() {
    // a mention addresses a bounded recipient → write-fanout (one Signal).
    let mention = AddressedRecipient {
        principal: pid("p:alice"),
        reason: WriteFanoutReason::Mentioned,
    };
    let b = fanout_behaviour(
        CHAT_MESSAGE_MENTIONED,
        "myelin://t/chat/message/m1",
        &[mention],
    );
    assert!(
        matches!(b, FanoutBehaviour::WriteFanout(ref s) if s.len() == 1),
        "a mention is write-fanout (one per-recipient Signal)"
    );
    assert_eq!(
        b.inbox_writes(),
        1,
        "the mention write-fanned to its 1 recipient"
    );

    // a thread reply to you is write-fanout (a direct participating address).
    let reply = AddressedRecipient {
        principal: pid("p:bob"),
        reason: WriteFanoutReason::ThreadReplyToYou,
    };
    let b = fanout_behaviour(CHAT_THREAD_REPLIED, "myelin://t/chat/thread/t1", &[reply]);
    assert!(
        matches!(b, FanoutBehaviour::WriteFanout(_)),
        "a thread reply is write-fanout"
    );

    // a channel post is read-fanout (ambient) — NO per-member Signal even with members present.
    let b = fanout_behaviour(CHAT_MESSAGE_CREATED, "myelin://t/chat/channel/c1", &[]);
    assert_eq!(
        b,
        FanoutBehaviour::ReadFanout,
        "a channel post is ambient read-fanout"
    );
    assert_eq!(
        b.inbox_writes(),
        0,
        "an ambient post does 0 per-member writes"
    );

    // channel lifecycle + coarse read-state are ambient read-fanout too.
    for ambient in [CHAT_CHANNEL_CREATED, CHAT_READ_STATE_UPDATED] {
        assert_eq!(
            fanout_behaviour(ambient, "myelin://t/chat/channel/c1", &[]),
            FanoutBehaviour::ReadFanout,
            "{ambient} is ambient read-fanout"
        );
    }
}

/// **Every durable chat token classifies into a behaviour arm — 0 misclassified / 0 unclassified.**
/// The behaviour layer is total over the durable tokens (it inherits [`fanout_class`]'s totality):
/// each token is exactly write-fanout XOR read-fanout, never both, never neither.
#[test]
fn fanout_behaviour_is_total_and_disjoint_over_durable_tokens() {
    assert!(
        crate::glue::fanout_class_is_total_over_durable_tokens(),
        "the static classifier is total (the behaviour layer inherits it)"
    );
    for token in crate::events::CHAT_DURABLE_TOKENS {
        // an addressed recipient is only consumed by the write-fanout arm; supply one so a
        // mis-classified ambient token would (wrongly) write-fan and be caught.
        let addressed = [AddressedRecipient {
            principal: pid("p:x"),
            reason: WriteFanoutReason::Mentioned,
        }];
        let b = fanout_behaviour(token, "myelin://t/chat/message/m", &addressed);
        let class = fanout_class(token).expect("durable token classifies");
        match class {
            FanoutClass::WriteFanout => assert!(
                matches!(b, FanoutBehaviour::WriteFanout(_)),
                "{token} classed write-fanout → write-fanout behaviour"
            ),
            FanoutClass::ReadFanout => assert_eq!(
                b,
                FanoutBehaviour::ReadFanout,
                "{token} classed read-fanout → read-fanout behaviour (0 per-member writes even with an addressed recipient)"
            ),
        }
    }
}

/// **An unknown / non-chat token defaults to read-fanout (the SAFE non-amplifying default).** A new
/// token never silently write-amplifies — it must be DELIBERATELY added to the write-fanout set.
#[test]
fn an_unknown_token_defaults_to_the_safe_read_fanout_arm() {
    let b = fanout_behaviour("issue.issue.updated", "myelin://t/issue/issue/1", &[]);
    assert_eq!(
        b,
        FanoutBehaviour::ReadFanout,
        "a non-chat token is the safe read-fanout default"
    );
    assert_eq!(
        b.inbox_writes(),
        0,
        "an unknown token never write-amplifies"
    );
}

// ───────────────────────────── the celebrity-fanout 0-per-member-write property ───────────────────

/// **THE CELEBRITY-FANOUT PROPERTY (the gate): a 100k-member channel post does ZERO per-member inbox
/// writes.** An ambient post (read-fanout) through the [`write_fanout`] driver emits 0 Signals to the
/// [`SignalSink`] REGARDLESS of channel size — the structural mitigation (the read-fanout class
/// produces no Signal). Every member's unread is derived lazily (the read-state service).
#[test]
fn a_100k_member_ambient_post_does_zero_per_member_inbox_writes() {
    let sink = CountingSignalSink::default();
    // a channel post to a 100k-member channel: read-fanout, NO addressed recipients.
    let writes = write_fanout(
        &sink,
        CHAT_MESSAGE_CREATED,
        "myelin://t/chat/channel/big",
        &[],
    );
    assert_eq!(writes, 0, "an ambient post write-fans to 0 recipients");
    assert_eq!(
        sink.count(),
        0,
        "the SINK saw 0 emits (no per-member inbox write)"
    );

    // the NAMED property holds for any member count (the class, not the size, gates the writes).
    for member_count in [0usize, 1, 50_000, 100_000, 1_000_000] {
        assert_eq!(
            ambient_post_inbox_writes(member_count),
            0,
            "an ambient post does 0 per-member writes at {member_count} members (celebrity-fanout)"
        );
    }
}

/// **The read-fanout half resolves watchers via `list_subjects(channel, watcher)` WITHOUT writing.**
/// Resolving a 100k watcher set is a pure read of the authz reverse index — it returns the audience
/// and materialises 0 inbox items (the unread each watcher sees is derived lazily). This proves the
/// resolve and the 0-write are independent: you can KNOW the 100k watchers and still write 0 items.
#[test]
fn watcher_resolution_reads_the_audience_but_writes_nothing() {
    let watchers: Vec<PrincipalId> = (0..100_000).map(|i| pid(&format!("p:{i}"))).collect();
    let dir = InMemoryWatchers {
        watchers: watchers.clone(),
    };
    let channel = ObjectId("channel:big".into());
    let resolved = resolve_watchers(&dir, &channel, &strong());
    assert_eq!(
        resolved.len(),
        100_000,
        "the read-fanout audience resolves all 100k watchers"
    );
    // resolving the audience writes NO inbox item — the ambient post still does 0 per-member writes.
    let sink = CountingSignalSink::default();
    let writes = write_fanout(
        &sink,
        CHAT_MESSAGE_CREATED,
        "myelin://t/chat/channel/big",
        &[],
    );
    assert_eq!(
        writes, 0,
        "knowing 100k watchers, the ambient post still writes 0 items"
    );
    assert_eq!(sink.count(), 0);
}

// ───────────────────────────── the write-fanout bound is the addressed set, not the channel ────────

/// **A write-fanout event's writes are bounded by the ADDRESSED recipient count, never the channel
/// size.** A message that mentions 3 people in a 100k-member channel writes EXACTLY 3 inbox items
/// (one per mentioned principal) — the bounded high-signal set, not 100k.
#[test]
fn write_fanout_is_bounded_by_the_addressed_set_not_the_channel_size() {
    let addressed: Vec<AddressedRecipient> = ["p:a", "p:b", "p:c"]
        .iter()
        .map(|p| AddressedRecipient {
            principal: pid(p),
            reason: WriteFanoutReason::Mentioned,
        })
        .collect();
    let sink = CountingSignalSink::default();
    let writes = write_fanout(
        &sink,
        CHAT_MESSAGE_MENTIONED,
        "myelin://t/chat/message/m",
        &addressed,
    );
    assert_eq!(
        writes, 3,
        "3 mentions in a 100k-member channel write EXACTLY 3 items"
    );
    assert_eq!(sink.count(), 3);
    // every emitted Signal names its recipient + the registered `mentioned` rule key + the subject.
    let emitted = sink.emitted.borrow();
    for (sig, exp) in emitted.iter().zip(["p:a", "p:b", "p:c"]) {
        assert_eq!(sig.recipient, pid(exp));
        assert_eq!(
            sig.rule_key, RULE_KEY_MENTIONED,
            "the Signal carries the registered rule key (7.6)"
        );
        assert_eq!(
            sig.subject, "myelin://t/chat/message/m",
            "references-not-payloads (the ref, not the body)"
        );
    }
}

/// **Each write-fanout reason maps to its registered Notif rule key + reason (contract 7.6 / §1.3).**
/// The five write-fanout classes carry the M2-C0-declared rule keys, and all map into the
/// chat-activity reason set (so a write-fanned Signal is always an Activity-view row — the §5.3 link).
#[test]
fn write_fanout_reasons_map_to_the_registered_rule_keys_and_activity_reasons() {
    use crate::glue::{RULE_KEY_APPROVAL_REQUESTED, RULE_KEY_REPLIED};
    assert_eq!(WriteFanoutReason::Mentioned.rule_key(), RULE_KEY_MENTIONED);
    assert_eq!(
        WriteFanoutReason::DirectMessage.rule_key(),
        RULE_KEY_MENTIONED
    );
    assert_eq!(
        WriteFanoutReason::KeywordMatch.rule_key(),
        RULE_KEY_MENTIONED
    );
    assert_eq!(
        WriteFanoutReason::ThreadReplyToYou.rule_key(),
        RULE_KEY_REPLIED
    );
    assert_eq!(
        WriteFanoutReason::HitlApprovalForYou.rule_key(),
        RULE_KEY_APPROVAL_REQUESTED
    );

    // every write-fanout reason maps into the chat-activity reason set (the §5.3 round-trip).
    let activity = activity_filter();
    let reasons = activity
        .reasons
        .expect("the Activity filter narrows by reason");
    for r in [
        WriteFanoutReason::Mentioned,
        WriteFanoutReason::DirectMessage,
        WriteFanoutReason::ThreadReplyToYou,
        WriteFanoutReason::HitlApprovalForYou,
        WriteFanoutReason::KeywordMatch,
    ] {
        assert!(
            reasons.contains(&r.notif_reason()),
            "{r:?}'s notif reason is in the Activity view (a write-fanned Signal is an Activity row)"
        );
    }
}

// ───────────────────────────── Activity is a VIEW, never a second store (the CI gate) ─────────────

/// **THE STRUCTURAL GATE: Activity is a `list_inbox` filter, not a second store (0 chat-private
/// activity store; §5.3, C-9).** The [`no_second_activity_store`] check holds, the Activity filter is
/// EXACTLY the frozen platform [`myelin_notif::InboxFilter::chat_activity`], and chat exposes no
/// `ActivityStore` type (it forwards to Notif's one inbox).
#[test]
fn activity_is_a_list_inbox_filter_not_a_second_store() {
    assert!(
        no_second_activity_store(),
        "Activity holds NO chat-private state — it is a list_inbox filter (C-9)"
    );
    // the filter IS the frozen platform chat-activity view (chat does not define its own shape).
    assert_eq!(
        activity_filter(),
        myelin_notif::InboxFilter::chat_activity(),
        "Activity = the frozen Notif chat-activity filter (one inbox, one read-state truth)"
    );
    // it narrows by the four chat reasons over the chat subsystem (a view, not a store vocabulary).
    let f = activity_filter();
    assert_eq!(
        f.subsystems,
        Some([myelin_notif::Subsystem::Chat].into_iter().collect()),
        "Activity is scoped to subsystem ∈ {{chat}}"
    );
    let reasons = f.reasons.expect("the Activity view narrows by reason");
    assert_eq!(
        reasons,
        [
            Reason::Mentioned,
            Reason::Replied,
            Reason::ThreadWatched,
            Reason::ApprovalRequested,
        ]
        .into_iter()
        .collect(),
        "Activity ∈ {{mentioned, replied, thread_watched, approval_requested}} (§5.3)"
    );
}
