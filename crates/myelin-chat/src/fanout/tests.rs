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

struct InMemoryWatchers {
    watchers: Vec<PrincipalId>,
}
impl WatcherDirectory for InMemoryWatchers {
    fn list_watchers(&self, _channel: &ObjectId, _at: &Consistency) -> Vec<PrincipalId> {
        self.watchers.clone()
    }
}

#[test]
fn the_write_fanout_vs_read_fanout_class_decision_is_correct() {
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

    let reply = AddressedRecipient {
        principal: pid("p:bob"),
        reason: WriteFanoutReason::ThreadReplyToYou,
    };
    let b = fanout_behaviour(CHAT_THREAD_REPLIED, "myelin://t/chat/thread/t1", &[reply]);
    assert!(
        matches!(b, FanoutBehaviour::WriteFanout(_)),
        "a thread reply is write-fanout"
    );

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

    for ambient in [CHAT_CHANNEL_CREATED, CHAT_READ_STATE_UPDATED] {
        assert_eq!(
            fanout_behaviour(ambient, "myelin://t/chat/channel/c1", &[]),
            FanoutBehaviour::ReadFanout,
            "{ambient} is ambient read-fanout"
        );
    }
}

#[test]
fn fanout_behaviour_is_total_and_disjoint_over_durable_tokens() {
    assert!(
        crate::glue::fanout_class_is_total_over_durable_tokens(),
        "the static classifier is total (the behaviour layer inherits it)"
    );
    for token in crate::events::CHAT_DURABLE_TOKENS {
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

#[test]
fn a_100k_member_ambient_post_does_zero_per_member_inbox_writes() {
    let sink = CountingSignalSink::default();
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

    for member_count in [0usize, 1, 50_000, 100_000, 1_000_000] {
        assert_eq!(
            ambient_post_inbox_writes(member_count),
            0,
            "an ambient post does 0 per-member writes at {member_count} members (celebrity-fanout)"
        );
    }
}

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
        WriteFanoutReason::ThreadWatched.rule_key(),
        crate::glue::RULE_KEY_THREAD_WATCHED,
    );
    assert_eq!(
        WriteFanoutReason::HitlApprovalForYou.rule_key(),
        RULE_KEY_APPROVAL_REQUESTED
    );

    let activity = activity_filter();
    let reasons = activity
        .reasons
        .expect("the Activity filter narrows by reason");
    for r in [
        WriteFanoutReason::Mentioned,
        WriteFanoutReason::DirectMessage,
        WriteFanoutReason::ThreadReplyToYou,
        WriteFanoutReason::ThreadWatched,
        WriteFanoutReason::HitlApprovalForYou,
        WriteFanoutReason::KeywordMatch,
    ] {
        assert!(
            reasons.contains(&r.notif_reason()),
            "{r:?}'s notif reason is in the Activity view (a write-fanned Signal is an Activity row)"
        );
    }
}

#[test]
fn activity_is_a_list_inbox_filter_not_a_second_store() {
    assert!(
        no_second_activity_store(),
        "Activity holds NO chat-private state - it is a list_inbox filter (C-9)"
    );
    assert_eq!(
        activity_filter(),
        myelin_notif::InboxFilter::chat_activity(),
        "Activity = the frozen Notif chat-activity filter (one inbox, one read-state truth)"
    );
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
