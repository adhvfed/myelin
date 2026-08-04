use crate::list_inbox::{list_inbox, InboxFilter, InboxPage, Page, ReadAuthorizePort};
use crate::router::{InboxProjection, RoutedInboxItem};
use myelin_events::firehose::{
    Firehose, FirehoseError, FirehoseScope, FrameDraft, Subscription as FirehoseSubscription,
};
use myelin_identity::{Consistency, Principal};

pub fn inbox_stream(principal: &Principal) -> String {
    format!("fan.{}.inbox", principal.tenant.0)
}

pub fn inbox_scope(principal: &Principal) -> Result<FirehoseScope, FirehoseError> {
    FirehoseScope::parse(&format!("inbox:{}", principal.principal_id.0))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxFrame {
    pub seq: u64,
    pub item_id: String,
}

#[derive(Debug)]
pub enum WatchOutcome {
    Live(InboxWatch),
    ResyncRequired {
        last_seq: u64,
        window_floor: u64,
    },
}

impl WatchOutcome {
    pub fn is_resync_required(&self) -> bool {
        matches!(self, WatchOutcome::ResyncRequired { .. })
    }

    pub fn into_live(self) -> Option<InboxWatch> {
        match self {
            WatchOutcome::Live(w) => Some(w),
            WatchOutcome::ResyncRequired { .. } => None,
        }
    }
}

#[derive(Debug)]
pub struct InboxWatch {
    sub: FirehoseSubscription,
    scope: FirehoseScope,
}

impl InboxWatch {
    pub fn next(&self) -> Option<InboxFrame> {
        self.sub.pull().map(decode_frame)
    }

    pub fn drain(&self) -> Vec<InboxFrame> {
        self.sub
            .drain_ready()
            .into_iter()
            .map(decode_frame)
            .collect()
    }

    pub fn last_seq(&self) -> u64 {
        self.sub.last_seq()
    }

    pub fn resync_required(&self) -> bool {
        self.sub.resync_required()
    }

    pub fn ready_len(&self) -> usize {
        self.sub.ready_len()
    }

    pub fn scope(&self) -> &FirehoseScope {
        &self.scope
    }
}

fn decode_frame(frame: myelin_events::firehose::Frame) -> InboxFrame {
    InboxFrame {
        seq: frame.seq,
        item_id: frame.payload.0,
    }
}

pub fn publish_inbox_frame(
    firehose: &mut Firehose,
    recipient: &Principal,
    item_id: &str,
) -> Result<InboxFrame, FirehoseError> {
    let stream = inbox_stream(recipient);
    let scope = inbox_scope(recipient)?;
    let frame = Firehose::publish(firehose, &stream, &scope, FrameDraft::new(item_id));
    Ok(decode_frame(frame))
}

pub fn watch_open(
    firehose: &mut Firehose,
    principal: &Principal,
) -> Result<WatchOutcome, FirehoseError> {
    let stream = inbox_stream(principal);
    let scope = inbox_scope(principal)?;
    let sub = firehose.subscribe(&stream, &scope, None)?;
    Ok(WatchOutcome::Live(InboxWatch { sub, scope }))
}

pub fn watch_resume(
    firehose: &mut Firehose,
    principal: &Principal,
    last_seq: u64,
) -> Result<WatchOutcome, FirehoseError> {
    let stream = inbox_stream(principal);
    let scope = inbox_scope(principal)?;
    match firehose.resume(&stream, &scope, last_seq) {
        Ok(sub) => Ok(WatchOutcome::Live(InboxWatch { sub, scope })),
        Err(FirehoseError::ResyncRequired {
            last_seq,
            window_floor,
        }) => Ok(WatchOutcome::ResyncRequired {
            last_seq,
            window_floor,
        }),
        Err(other) => Err(other),
    }
}

impl InboxWatch {
    pub fn resume(
        firehose: &mut Firehose,
        principal: &Principal,
        last_seq: u64,
    ) -> Result<WatchOutcome, FirehoseError> {
        watch_resume(firehose, principal, last_seq)
    }
}

pub fn cold_rebuild(
    inbox: &InboxProjection,
    principal: &Principal,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
) -> InboxPage {
    list_inbox(
        inbox,
        principal,
        &InboxFilter::all(),
        &Page::default(),
        authorize,
        at,
    )
}

pub fn cold_rebuild_item_ids(
    inbox: &InboxProjection,
    principal: &Principal,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
) -> Vec<String> {
    cold_rebuild(inbox, principal, authorize, at)
        .items
        .iter()
        .map(|i: &RoutedInboxItem| i.item_id.clone())
        .collect()
}

#[cfg(test)]
#[path = "watch/tests.rs"]
mod tests;
