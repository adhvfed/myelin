use myelin_identity::{Consistency, ObjectId, Permission, PrincipalId};
use myelin_notif::{InboxFilter, Reason};

use crate::glue::{
    fanout_class, FanoutClass, RULE_KEY_APPROVAL_REQUESTED, RULE_KEY_MENTIONED, RULE_KEY_REPLIED,
};

pub const WATCHER_RELATION: &str = "watcher";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signal {
    pub recipient: PrincipalId,
    pub rule_key: &'static str,
    pub subject: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteFanoutReason {
    Mentioned,
    DirectMessage,
    ThreadReplyToYou,
    HitlApprovalForYou,
    KeywordMatch,
}

impl WriteFanoutReason {
    pub fn rule_key(self) -> &'static str {
        match self {
            WriteFanoutReason::Mentioned
            | WriteFanoutReason::DirectMessage
            | WriteFanoutReason::KeywordMatch => RULE_KEY_MENTIONED,
            WriteFanoutReason::ThreadReplyToYou => RULE_KEY_REPLIED,
            WriteFanoutReason::HitlApprovalForYou => RULE_KEY_APPROVAL_REQUESTED,
        }
    }

    pub fn notif_reason(self) -> Reason {
        match self {
            WriteFanoutReason::Mentioned
            | WriteFanoutReason::DirectMessage
            | WriteFanoutReason::KeywordMatch => Reason::Mentioned,
            WriteFanoutReason::ThreadReplyToYou => Reason::Replied,
            WriteFanoutReason::HitlApprovalForYou => Reason::ApprovalRequested,
        }
    }
}

pub trait SignalSink {
    fn emit_signal(&self, signal: &Signal);
}

pub trait WatcherDirectory {
    fn list_watchers(&self, channel: &ObjectId, at: &Consistency) -> Vec<PrincipalId>;
}

pub fn resolve_watchers<D: WatcherDirectory>(
    dir: &D,
    channel: &ObjectId,
    at: &Consistency,
) -> Vec<PrincipalId> {
    let _permission = Permission(WATCHER_RELATION.to_string());
    dir.list_watchers(channel, at)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FanoutBehaviour {
    WriteFanout(Vec<Signal>),
    ReadFanout,
}

impl FanoutBehaviour {
    pub fn inbox_writes(&self) -> usize {
        match self {
            FanoutBehaviour::WriteFanout(signals) => signals.len(),
            FanoutBehaviour::ReadFanout => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressedRecipient {
    pub principal: PrincipalId,
    pub reason: WriteFanoutReason,
}

pub fn fanout_behaviour(
    token: &str,
    subject: &str,
    addressed: &[AddressedRecipient],
) -> FanoutBehaviour {
    match fanout_class(token) {
        Some(FanoutClass::WriteFanout) => {
            let signals = addressed
                .iter()
                .map(|a| Signal {
                    recipient: a.principal.clone(),
                    rule_key: a.reason.rule_key(),
                    subject: subject.to_string(),
                })
                .collect();
            FanoutBehaviour::WriteFanout(signals)
        }
        Some(FanoutClass::ReadFanout) | None => FanoutBehaviour::ReadFanout,
    }
}

pub fn write_fanout<S: SignalSink>(
    sink: &S,
    token: &str,
    subject: &str,
    addressed: &[AddressedRecipient],
) -> usize {
    let behaviour = fanout_behaviour(token, subject, addressed);
    match &behaviour {
        FanoutBehaviour::WriteFanout(signals) => {
            for s in signals {
                sink.emit_signal(s);
            }
        }
        FanoutBehaviour::ReadFanout => {}
    }
    behaviour.inbox_writes()
}

pub fn ambient_post_inbox_writes(member_count: usize) -> usize {
    let _ = member_count;
    let behaviour = fanout_behaviour(
        crate::events::CHAT_MESSAGE_CREATED,
        "myelin://t/chat/channel/c",
        &[],
    );
    behaviour.inbox_writes()
}

pub fn activity_filter() -> InboxFilter {
    InboxFilter::chat_activity()
}

#[allow(clippy::too_many_arguments)]
pub fn activity(
    inbox: &myelin_notif::InboxProjection,
    me: &myelin_identity::Principal,
    page: &myelin_notif::Page,
    authorize: &dyn myelin_notif::ReadAuthorizePort,
    at: &Consistency,
) -> myelin_notif::InboxPage {
    myelin_notif::list_inbox(inbox, me, &activity_filter(), page, authorize, at)
}

pub fn no_second_activity_store() -> bool {
    let filter = activity_filter();
    if filter != InboxFilter::chat_activity() {
        return false;
    }
    let activity_reasons = [
        Reason::Mentioned,
        Reason::Replied,
        Reason::ThreadWatched,
        Reason::ApprovalRequested,
    ];
    match &filter.reasons {
        Some(reasons) => activity_reasons.iter().all(|r| reasons.contains(r)),
        None => false,
    }
}

#[cfg(test)]
mod tests;
