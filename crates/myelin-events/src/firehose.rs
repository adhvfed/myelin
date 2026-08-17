use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

pub const FIREHOSE_MAX_SCOPE_ID_BYTES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FirehoseScope {
    kind: ScopeKind,
    id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScopeKind {
    Board,
    Doc,
    Channel,
    Inbox,
    Run,
}

impl ScopeKind {
    pub fn prefix(self) -> &'static str {
        match self {
            ScopeKind::Board => "board",
            ScopeKind::Doc => "doc",
            ScopeKind::Channel => "channel",
            ScopeKind::Inbox => "inbox",
            ScopeKind::Run => "run",
        }
    }
}

impl FirehoseScope {
    pub fn parse(raw: &str) -> Result<FirehoseScope, FirehoseError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(FirehoseError::OverBroadScope {
                scope: String::new(),
                why: "an empty scope is unbounded (it names no resource)",
            });
        }
        if raw.len() > FIREHOSE_MAX_SCOPE_ID_BYTES + 16 {
            return Err(FirehoseError::ScopeLimitExceeded {
                maximum: FIREHOSE_MAX_SCOPE_ID_BYTES,
            });
        }
        if raw.contains('*') {
            return Err(FirehoseError::OverBroadScope {
                scope: raw.to_string(),
                why: "scope must be a bounded selector (board:/doc:/channel:), never `*`",
            });
        }
        let Some((prefix, id)) = raw.split_once(':') else {
            return Err(FirehoseError::OverBroadScope {
                scope: raw.to_string(),
                why: "scope must name its kind: board:/doc:/channel:",
            });
        };
        if id.is_empty() {
            return Err(FirehoseError::OverBroadScope {
                scope: raw.to_string(),
                why: "scope resource id must not be empty",
            });
        }
        if id.len() > FIREHOSE_MAX_SCOPE_ID_BYTES {
            return Err(FirehoseError::ScopeLimitExceeded {
                maximum: FIREHOSE_MAX_SCOPE_ID_BYTES,
            });
        }
        let kind = match prefix {
            "board" => ScopeKind::Board,
            "doc" => ScopeKind::Doc,
            "channel" => ScopeKind::Channel,
            "inbox" => ScopeKind::Inbox,
            "run" => ScopeKind::Run,
            _ => {
                return Err(FirehoseError::OverBroadScope {
                    scope: raw.to_string(),
                    why: "unknown scope kind (only board:/doc:/channel:/inbox:/run:)",
                })
            }
        };
        Ok(FirehoseScope {
            kind,
            id: id.to_string(),
        })
    }

    pub fn kind(&self) -> ScopeKind {
        self.kind
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn selector(&self) -> String {
        format!("{}:{}", self.kind.prefix(), self.id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub seq: u64,
    pub payload: FramePayload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FramePayload(pub String);

impl Frame {
    pub fn new(seq: u64, payload: impl Into<String>) -> Frame {
        Frame {
            seq,
            payload: FramePayload(payload.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameDraft {
    pub payload: FramePayload,
}

impl FrameDraft {
    pub fn new(payload: impl Into<String>) -> FrameDraft {
        FrameDraft {
            payload: FramePayload(payload.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirehoseError {
    OverBroadScope { scope: String, why: &'static str },
    ResyncRequired { last_seq: u64, window_floor: u64 },
    TailLimitExceeded,
    ScopeLimitExceeded { maximum: usize },
    SequenceExhausted { last_seq: u64 },
    InvalidHeadSeed { current: u64, attempted: u64 },
}

impl FirehoseError {
    pub fn is_resync_required(&self) -> bool {
        matches!(self, FirehoseError::ResyncRequired { .. })
    }

    pub fn is_over_broad_scope(&self) -> bool {
        matches!(
            self,
            FirehoseError::OverBroadScope { .. } | FirehoseError::ScopeLimitExceeded { .. }
        )
    }
}

impl core::fmt::Display for FirehoseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FirehoseError::OverBroadScope { scope, why } => {
                write!(f, "firehose rejects over-broad scope `{scope}`: {why}")
            }
            FirehoseError::ResyncRequired { last_seq, window_floor } => write!(
                f,
                "resync_required: last_seq={last_seq} is older than the retention window (floor={window_floor}) \
                 → fall back to a *.snapshot replay (EB-22)"
            ),
            FirehoseError::TailLimitExceeded => {
                f.write_str("firehose tail read limit exceeded")
            }
            FirehoseError::ScopeLimitExceeded { maximum } => write!(
                f,
                "firehose rejects scope resource id longer than {maximum} bytes"
            ),
            FirehoseError::SequenceExhausted { last_seq } => write!(
                f,
                "firehose sequence is exhausted at {last_seq}; a new stream generation is required"
            ),
            FirehoseError::InvalidHeadSeed { current, attempted } => write!(
                f,
                "firehose cannot seed head {attempted} over current head {current}"
            ),
        }
    }
}

impl std::error::Error for FirehoseError {}

#[derive(Clone, Debug)]
pub struct RetentionWindow {
    frames: std::collections::VecDeque<Frame>,
    capacity: usize,
    last_seq: u64,
}

impl RetentionWindow {
    pub const DEFAULT_FRAMES: usize = 4096;

    pub fn new(capacity: usize) -> RetentionWindow {
        RetentionWindow {
            frames: std::collections::VecDeque::new(),
            capacity: capacity.max(1),
            last_seq: 0,
        }
    }

    fn publish(&mut self, draft: FrameDraft) -> Result<Frame, FirehoseError> {
        let next_seq = self
            .last_seq
            .checked_add(1)
            .ok_or(FirehoseError::SequenceExhausted {
                last_seq: self.last_seq,
            })?;
        let frame = Frame {
            seq: next_seq,
            payload: draft.payload,
        };
        if self.frames.len() == self.capacity {
            self.frames.pop_front();
        }
        self.frames.push_back(frame.clone());
        self.last_seq = next_seq;
        Ok(frame)
    }

    fn seed_head(&mut self, last_seq: u64) -> Result<(), FirehoseError> {
        if last_seq == u64::MAX {
            return Err(FirehoseError::SequenceExhausted { last_seq });
        }
        if self.last_seq == last_seq {
            return Ok(());
        }
        if self.frames.is_empty() && last_seq > self.last_seq {
            self.last_seq = last_seq;
            return Ok(());
        }
        Err(FirehoseError::InvalidHeadSeed {
            current: self.last_seq,
            attempted: last_seq,
        })
    }

    fn window_floor(&self) -> u64 {
        self.frames.front().map(|f| f.seq).unwrap_or(0)
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    fn backfill(&self, last_seq: u64) -> Result<Vec<Frame>, FirehoseError> {
        if last_seq >= self.last_seq {
            return Ok(Vec::new());
        }
        let floor = self.window_floor();
        let first_missing = last_seq + 1;
        if floor == 0 || first_missing < floor {
            return Err(FirehoseError::ResyncRequired {
                last_seq,
                window_floor: floor,
            });
        }
        Ok(self
            .frames
            .iter()
            .filter(|f| f.seq > last_seq)
            .cloned()
            .collect())
    }

    fn tail(&self, lo: u64, hi: u64) -> Vec<Frame> {
        self.frames
            .iter()
            .filter(|f| f.seq >= lo && f.seq <= hi)
            .cloned()
            .collect()
    }

    fn tail_bounded(
        &self,
        lo: u64,
        hi: u64,
        maximum_frames: usize,
        maximum_payload_bytes: usize,
    ) -> Result<Vec<Frame>, FirehoseError> {
        let mut frames = Vec::new();
        let mut payload_bytes = 0usize;
        for frame in self
            .frames
            .iter()
            .filter(|frame| frame.seq >= lo && frame.seq <= hi)
        {
            if frames.len() >= maximum_frames {
                return Err(FirehoseError::TailLimitExceeded);
            }
            payload_bytes = payload_bytes
                .checked_add(frame.payload.0.len())
                .ok_or(FirehoseError::TailLimitExceeded)?;
            if payload_bytes > maximum_payload_bytes {
                return Err(FirehoseError::TailLimitExceeded);
            }
            frames.push(frame.clone());
        }
        Ok(frames)
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

pub const DEFAULT_INFLIGHT_CAP: usize = 1024;

#[derive(Clone, Debug)]
pub struct SubStream {
    stream: String,
    scope: FirehoseScope,
    ready: std::collections::VecDeque<Frame>,
    inflight_cap: usize,
    delivered_seq: u64,
    resync_required: bool,
    enqueued_seq: u64,
}

impl SubStream {
    fn new(
        stream: String,
        scope: FirehoseScope,
        backfill: Vec<Frame>,
        inflight_cap: usize,
        start_seq: u64,
    ) -> SubStream {
        let enqueued_seq = backfill.last().map(|f| f.seq).unwrap_or(start_seq);
        SubStream {
            stream,
            scope,
            ready: backfill.into_iter().collect(),
            inflight_cap: inflight_cap.max(1),
            delivered_seq: start_seq,
            resync_required: false,
            enqueued_seq,
        }
    }

    fn enqueue_live(&mut self, frame: Frame) {
        if self.resync_required {
            return;
        }
        if frame.seq <= self.enqueued_seq {
            return;
        }
        if self.ready.len() >= self.inflight_cap {
            self.drop_to_resync();
            return;
        }
        self.enqueued_seq = frame.seq;
        self.ready.push_back(frame);
    }

    pub fn pull(&mut self) -> Option<Frame> {
        if self.resync_required {
            return None;
        }
        let frame = self.ready.pop_front()?;
        self.delivered_seq = frame.seq;
        Some(frame)
    }

    pub fn drain_ready(&mut self) -> Vec<Frame> {
        let mut out = Vec::new();
        while let Some(f) = self.pull() {
            out.push(f);
        }
        out
    }

    pub fn last_seq(&self) -> u64 {
        self.delivered_seq
    }

    pub fn resync_required(&self) -> bool {
        self.resync_required
    }

    pub fn ready_len(&self) -> usize {
        self.ready.len()
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub fn scope(&self) -> &FirehoseScope {
        &self.scope
    }

    fn drop_to_resync(&mut self) {
        if self.resync_required {
            return;
        }
        self.ready.clear();
        self.resync_required = true;
    }
}

#[derive(Default)]
pub struct Firehose {
    windows: HashMap<(String, FirehoseScope), RetentionWindow>,
    subscribers: HashMap<(String, FirehoseScope), Vec<SubHandle>>,
    window_capacity: usize,
    inflight_cap: usize,
}

#[derive(Clone)]
struct SubHandle(Weak<Mutex<SubStream>>);

#[derive(Clone, Debug)]
pub struct Subscription(Arc<Mutex<SubStream>>);

impl Subscription {
    pub fn pull(&self) -> Option<Frame> {
        self.lock_stream().pull()
    }

    pub fn drain_ready(&self) -> Vec<Frame> {
        self.lock_stream().drain_ready()
    }

    pub fn last_seq(&self) -> u64 {
        self.lock_stream().last_seq()
    }

    pub fn resync_required(&self) -> bool {
        self.lock_stream().resync_required()
    }

    pub fn ready_len(&self) -> usize {
        self.lock_stream().ready_len()
    }

    pub fn stream(&self) -> String {
        self.lock_stream().stream().to_string()
    }

    pub fn scope(&self) -> FirehoseScope {
        self.lock_stream().scope().clone()
    }

    fn lock_stream(&self) -> MutexGuard<'_, SubStream> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Firehose {
    pub fn new() -> Firehose {
        Firehose::with_limits(RetentionWindow::DEFAULT_FRAMES, DEFAULT_INFLIGHT_CAP)
    }

    pub fn for_stream_class(class: crate::retention::StreamClass) -> Firehose {
        Firehose::with_limits(class.window_frames(), DEFAULT_INFLIGHT_CAP)
    }

    pub fn with_limits(window_capacity: usize, inflight_cap: usize) -> Firehose {
        Firehose {
            windows: HashMap::new(),
            subscribers: HashMap::new(),
            window_capacity: window_capacity.max(1),
            inflight_cap: inflight_cap.max(1),
        }
    }

    pub fn publish(
        &mut self,
        stream: &str,
        scope: &FirehoseScope,
        draft: FrameDraft,
    ) -> Result<Frame, FirehoseError> {
        let key = (stream.to_string(), scope.clone());
        let window = self
            .windows
            .entry(key.clone())
            .or_insert_with(|| RetentionWindow::new(self.window_capacity));
        let frame = window.publish(draft)?;
        if let Some(subs) = self.subscribers.get_mut(&key) {
            subs.retain(|handle| {
                let Some(subscriber) = handle.0.upgrade() else {
                    return false;
                };
                let mut subscriber = subscriber
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                subscriber.enqueue_live(frame.clone());
                !subscriber.resync_required()
            });
        }
        Ok(frame)
    }

    pub fn tail(&self, stream: &str, scope: &FirehoseScope, lo: u64, hi: u64) -> Vec<Frame> {
        let key = (stream.to_string(), scope.clone());
        self.windows
            .get(&key)
            .map(|w| w.tail(lo, hi))
            .unwrap_or_default()
    }

    pub fn tail_bounded(
        &self,
        stream: &str,
        scope: &FirehoseScope,
        lo: u64,
        hi: u64,
        maximum_frames: usize,
        maximum_payload_bytes: usize,
    ) -> Result<Vec<Frame>, FirehoseError> {
        let key = (stream.to_string(), scope.clone());
        self.windows
            .get(&key)
            .map(|window| window.tail_bounded(lo, hi, maximum_frames, maximum_payload_bytes))
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    pub fn subscribe(
        &mut self,
        stream: &str,
        scope: &FirehoseScope,
        cursor: Option<u64>,
    ) -> Result<Subscription, FirehoseError> {
        match cursor {
            None => Ok(self.open_live(stream, scope, Vec::new())),
            Some(last_seq) => self.resume(stream, scope, last_seq),
        }
    }

    pub fn subscribe_raw(
        &mut self,
        stream: &str,
        raw_scope: &str,
        cursor: Option<u64>,
    ) -> Result<Subscription, FirehoseError> {
        let scope = FirehoseScope::parse(raw_scope)?;
        self.subscribe(stream, &scope, cursor)
    }

    pub fn resume(
        &mut self,
        stream: &str,
        scope: &FirehoseScope,
        last_seq: u64,
    ) -> Result<Subscription, FirehoseError> {
        let backfill = self.backfill(stream, scope, last_seq)?;
        Ok(self.open_live(stream, scope, backfill))
    }

    pub fn backfill(
        &self,
        stream: &str,
        scope: &FirehoseScope,
        last_seq: u64,
    ) -> Result<Vec<Frame>, FirehoseError> {
        let key = (stream.to_string(), scope.clone());
        Ok(match self.windows.get(&key) {
            None => Vec::new(),
            Some(window) => window.backfill(last_seq)?,
        })
    }

    pub fn seed_head(
        &mut self,
        stream: &str,
        scope: &FirehoseScope,
        last_seq: u64,
    ) -> Result<(), FirehoseError> {
        if last_seq == u64::MAX {
            return Err(FirehoseError::SequenceExhausted { last_seq });
        }
        self.windows
            .entry((stream.to_string(), scope.clone()))
            .or_insert_with(|| RetentionWindow::new(self.window_capacity))
            .seed_head(last_seq)
    }

    fn open_live(
        &mut self,
        stream: &str,
        scope: &FirehoseScope,
        backfill: Vec<Frame>,
    ) -> Subscription {
        let key = (stream.to_string(), scope.clone());
        let head = self.windows.get(&key).map(|w| w.last_seq()).unwrap_or(0);
        let start_seq = backfill.last().map(|f| f.seq).unwrap_or(head);
        let sub = SubStream::new(
            stream.to_string(),
            scope.clone(),
            backfill,
            self.inflight_cap,
            start_seq,
        );
        let shared = Arc::new(Mutex::new(sub));
        self.subscribers
            .entry(key)
            .or_default()
            .push(SubHandle(Arc::downgrade(&shared)));
        Subscription(shared)
    }

    pub fn head_seq(&self, stream: &str, scope: &FirehoseScope) -> u64 {
        self.windows
            .get(&(stream.to_string(), scope.clone()))
            .map(|w| w.last_seq())
            .unwrap_or(0)
    }

    pub fn window_len(&self, stream: &str, scope: &FirehoseScope) -> usize {
        self.windows
            .get(&(stream.to_string(), scope.clone()))
            .map(|w| w.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(s: &str) -> FirehoseScope {
        FirehoseScope::parse(s).expect("a bounded scope")
    }

    fn draft(p: &str) -> FrameDraft {
        FrameDraft::new(p)
    }

    #[test]
    fn publish_assigns_per_stream_scope_monotonic_seq() {
        let mut fh = Firehose::new();
        let board_a = scope("board:a");
        let board_b = scope("board:b");

        let f1 = fh
            .publish("chat-live", &board_a, draft("op-1"))
            .expect("valid frame");
        let f2 = fh
            .publish("chat-live", &board_a, draft("op-2"))
            .expect("valid frame");
        let f3 = fh
            .publish("chat-live", &board_a, draft("op-3"))
            .expect("valid frame");
        assert_eq!(
            (f1.seq, f2.seq, f3.seq),
            (1, 2, 3),
            "monotone per (stream,scope)"
        );

        let g1 = fh
            .publish("chat-live", &board_b, draft("op-x"))
            .expect("valid frame");
        assert_eq!(
            g1.seq, 1,
            "a different scope has an independent monotonic seq"
        );
        let f4 = fh
            .publish("chat-live", &board_a, draft("op-4"))
            .expect("valid frame");
        assert_eq!(f4.seq, 4, "the original scope's sequence is independent");
    }

    #[test]
    fn resume_backfills_the_gap_then_goes_live_losing_zero_ops() {
        let mut fh = Firehose::new();
        let s = scope("doc:design");

        fh.publish("kn-ops", &s, draft("op-1"))
            .expect("the fixture publishes a valid frame");
        fh.publish("kn-ops", &s, draft("op-2"))
            .expect("the fixture publishes a valid frame");
        fh.publish("kn-ops", &s, draft("op-3"))
            .expect("the fixture publishes a valid frame");
        fh.publish("kn-ops", &s, draft("op-4"))
            .expect("the fixture publishes a valid frame");
        fh.publish("kn-ops", &s, draft("op-5"))
            .expect("the fixture publishes a valid frame");

        let sub = fh
            .resume("kn-ops", &s, 2)
            .expect("in-window resume backfills");
        let backfilled = sub.drain_ready();
        let seqs: Vec<u64> = backfilled.iter().map(|f| f.seq).collect();
        assert_eq!(
            seqs,
            vec![3, 4, 5],
            "the gap (last_seq, now] is replayed - ZERO ops lost"
        );
        assert_eq!(sub.last_seq(), 5, "the resume cursor advanced to the head");

        fh.publish("kn-ops", &s, draft("op-6"))
            .expect("the fixture publishes a valid frame");
        let live = sub.drain_ready();
        assert_eq!(
            live.iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![6],
            "live continues gap-free"
        );

        let mut all = seqs;
        all.extend(live.iter().map(|f| f.seq));
        assert_eq!(
            all,
            vec![3, 4, 5, 6],
            "across the reconnect: 0 lost, 0 duplicate"
        );
    }

    #[test]
    fn reading_a_backfill_does_not_open_a_live_subscription() {
        let mut fh = Firehose::new();
        let s = scope("doc:design");
        fh.publish("kn-ops", &s, draft("op-1"))
            .expect("the fixture publishes a valid frame");
        fh.publish("kn-ops", &s, draft("op-2"))
            .expect("the fixture publishes a valid frame");

        let frames = fh
            .backfill("kn-ops", &s, 0)
            .expect("the cursor is in the retention window");

        assert_eq!(frames.len(), 2);
        assert!(
            fh.subscribers.is_empty(),
            "a recovery read owns no live subscriber state"
        );
    }

    #[test]
    fn a_snapshot_can_seed_an_empty_stream_cursor_once() {
        let mut fh = Firehose::new();
        let s = scope("doc:design");

        fh.seed_head("kn-ops", &s, 7)
            .expect("an empty stream accepts a recovered head");
        assert_eq!(fh.head_seq("kn-ops", &s), 7);
        assert_eq!(
            fh.publish("kn-ops", &s, draft("op-8"))
                .expect("the next live frame fits")
                .seq,
            8
        );
        assert!(
            fh.seed_head("kn-ops", &s, 8).is_ok(),
            "the current head is idempotent"
        );
        assert!(
            matches!(
                fh.seed_head("kn-ops", &s, 9),
                Err(FirehoseError::InvalidHeadSeed {
                    current: 8,
                    attempted: 9
                })
            ),
            "a seed cannot jump over retained live frames"
        );
        let fresh = scope("doc:fresh");
        assert!(
            matches!(
                fh.seed_head("kn-ops", &fresh, u64::MAX),
                Err(FirehoseError::SequenceExhausted { last_seq: u64::MAX })
            ),
            "a seed must leave room for the next live frame"
        );
    }

    #[test]
    fn an_exhausted_sequence_refuses_the_frame_without_mutating_live_state() {
        let mut fh = Firehose::new();
        let s = scope("doc:long-lived");
        fh.seed_head("kn-ops", &s, u64::MAX - 1)
            .expect("the recovered head leaves room for one frame");
        let sub = fh
            .subscribe("kn-ops", &s, None)
            .expect("the bounded stream subscribes");

        let final_frame = fh
            .publish("kn-ops", &s, draft("final"))
            .expect("the final representable sequence publishes");
        assert_eq!(final_frame.seq, u64::MAX);
        assert_eq!(sub.pull(), Some(final_frame));

        let error = fh
            .publish("kn-ops", &s, draft("must-not-appear"))
            .expect_err("a monotonic cursor can never wrap");
        assert_eq!(
            error,
            FirehoseError::SequenceExhausted { last_seq: u64::MAX }
        );
        assert_eq!(fh.head_seq("kn-ops", &s), u64::MAX);
        assert_eq!(fh.window_len("kn-ops", &s), 1);
        assert_eq!(sub.pull(), None, "no rejected frame reaches a subscriber");
    }

    #[test]
    fn firehose_handles_can_cross_worker_threads() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<Firehose>();
        assert_send_sync::<Subscription>();
    }

    #[test]
    fn the_caller_owns_a_live_subscription_lifetime() {
        let mut fh = Firehose::new();
        let s = scope("doc:design");
        let key = ("kn-ops".to_string(), s.clone());
        let subscription = fh
            .subscribe("kn-ops", &s, None)
            .expect("the live subscription opens");

        assert_eq!(
            Arc::strong_count(&subscription.0),
            1,
            "the firehose does not retain a dropped caller's stream"
        );
        drop(subscription);
        fh.publish("kn-ops", &s, draft("op-1"))
            .expect("the fixture publishes a valid frame");

        assert!(
            fh.subscribers.get(&key).is_none_or(Vec::is_empty),
            "publishing reaps the expired subscription handle"
        );
    }

    #[test]
    fn subscribe_with_no_cursor_starts_live_from_now() {
        let mut fh = Firehose::new();
        let s = scope("channel:eng");
        fh.publish("chat-live", &s, draft("old-1"))
            .expect("the fixture publishes a valid frame");
        fh.publish("chat-live", &s, draft("old-2"))
            .expect("the fixture publishes a valid frame");

        let sub = fh
            .subscribe("chat-live", &s, None)
            .expect("bounded scope subscribes");
        assert!(
            sub.drain_ready().is_empty(),
            "no backfill on a None cursor (live from now)"
        );

        fh.publish("chat-live", &s, draft("new-3"))
            .expect("the fixture publishes a valid frame");
        fh.publish("chat-live", &s, draft("new-4"))
            .expect("the fixture publishes a valid frame");
        let live: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
        assert_eq!(
            live,
            vec![3, 4],
            "a None-cursor subscribe receives only post-open live frames"
        );
    }

    #[test]
    fn resume_at_head_is_a_no_op_backfill() {
        let mut fh = Firehose::new();
        let s = scope("board:7");
        for _ in 0..5 {
            fh.publish("issues", &s, draft("row"))
                .expect("the fixture publishes a valid frame");
        }
        let sub = fh
            .resume("issues", &s, 5)
            .expect("caught-up resume is fine");
        assert!(
            sub.drain_ready().is_empty(),
            "a caught-up resume backfills nothing"
        );
        assert!(!sub.resync_required(), "a caught-up resume is NOT a resync");
    }

    #[test]
    fn out_of_window_last_seq_yields_resync_required() {
        let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
        let s = scope("doc:hot");
        for _ in 0..6 {
            fh.publish("kn-ops", &s, draft("op"))
                .expect("the fixture publishes a valid frame");
        }
        assert_eq!(
            fh.window_len("kn-ops", &s),
            3,
            "the window is bounded at 3 (1,2,3 evicted)"
        );

        let err = fh
            .resume("kn-ops", &s, 2)
            .expect_err("an out-of-window cursor cannot backfill");
        assert!(
            err.is_resync_required(),
            "the over-window cursor RAISES resync_required (NAMED)"
        );
        if let FirehoseError::ResyncRequired {
            last_seq,
            window_floor,
        } = err
        {
            assert_eq!(last_seq, 2);
            assert_eq!(
                window_floor, 4,
                "the window floor is the oldest held seq (4)"
            );
        } else {
            panic!("expected ResyncRequired");
        }

        let sub = fh
            .resume("kn-ops", &s, 4)
            .expect("an in-window cursor backfills");
        assert_eq!(
            sub.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![5, 6]
        );
    }

    #[test]
    fn window_floor_boundary_is_exact() {
        let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
        let s = scope("board:big");
        for _ in 0..6 {
            fh.publish("issues", &s, draft("row"))
                .expect("the fixture publishes a valid frame");
        }
        let sub = fh
            .resume("issues", &s, 3)
            .expect("first-missing == floor is in-window");
        assert_eq!(
            sub.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![4, 5, 6]
        );
        assert!(fh
            .resume("issues", &s, 2)
            .expect_err("first-missing < floor")
            .is_resync_required());
    }

    #[test]
    fn the_transport_rejects_an_over_broad_scope() {
        let mut fh = Firehose::new();

        let err = fh
            .subscribe_raw("chat-live", "*", None)
            .expect_err("`*` is rejected");
        assert!(
            err.is_over_broad_scope(),
            "scope = * is an over-broad scope (rejected)"
        );

        for raw in [
            "*", "board:*", "doc:a*", "123", "", "   ", "team:eng", "all",
        ] {
            let r = fh.subscribe_raw("chat-live", raw, None);
            assert!(
                r.is_err(),
                "over-broad scope `{raw}` must be rejected at subscribe, got {r:?}"
            );
            assert!(
                r.unwrap_err().is_over_broad_scope(),
                "`{raw}` is an over-broad-scope rejection"
            );
        }

        assert!(
            fh.subscribe_raw("chat-live", "channel:eng", None).is_ok(),
            "a bounded scope subscribes"
        );
    }

    #[test]
    fn over_broad_scopes_are_all_rejected_bounded_scopes_all_parse() {
        for raw in [
            "*",
            "board:*",
            "doc:a*",
            "channel:*",
            "123",
            "",
            "   ",
            "team:eng",
            "all",
            "board:",
        ] {
            let r = FirehoseScope::parse(raw);
            assert!(
                r.is_err(),
                "over-broad/invalid scope `{raw}` must be rejected, got {r:?}"
            );
            assert!(r.unwrap_err().is_over_broad_scope());
        }
        for raw in ["board:42", "doc:design", "channel:eng", "board:proj-1_x"] {
            let s = FirehoseScope::parse(raw).unwrap_or_else(|_| panic!("`{raw}` must parse"));
            assert_eq!(
                s.selector(),
                raw,
                "a bounded scope round-trips its selector string"
            );
        }
        let exact = format!("board:{}", "x".repeat(FIREHOSE_MAX_SCOPE_ID_BYTES));
        assert_eq!(
            FirehoseScope::parse(&exact)
                .expect("exact scope limit accepted")
                .id()
                .len(),
            FIREHOSE_MAX_SCOPE_ID_BYTES
        );
        assert_eq!(
            FirehoseScope::parse(&format!(
                "board:{}",
                "x".repeat(FIREHOSE_MAX_SCOPE_ID_BYTES + 1)
            )),
            Err(FirehoseError::ScopeLimitExceeded {
                maximum: FIREHOSE_MAX_SCOPE_ID_BYTES,
            })
        );
        assert_eq!(
            FirehoseScope::parse(&format!(
                "board:*{}",
                "x".repeat(FIREHOSE_MAX_SCOPE_ID_BYTES + 32)
            )),
            Err(FirehoseError::ScopeLimitExceeded {
                maximum: FIREHOSE_MAX_SCOPE_ID_BYTES,
            }),
            "overlong invalid input is rejected without copying it into an error"
        );
    }

    #[test]
    fn run_scope_is_a_bounded_kind_and_unbounded_run_is_rejected() {
        let s = FirehoseScope::parse("run:01J0RUN").expect("a bounded run scope parses");
        assert_eq!(s.kind(), ScopeKind::Run, "run: parses to the Run kind");
        assert_eq!(s.id(), "01J0RUN", "the run id is the bounded resource id");
        assert_eq!(
            s.selector(),
            "run:01J0RUN",
            "the run scope round-trips its selector"
        );
        for raw in ["run:*", "run:", "run"] {
            let r = FirehoseScope::parse(raw);
            assert!(
                r.is_err(),
                "unbounded/empty run scope `{raw}` must be rejected, got {r:?}"
            );
            assert!(
                r.unwrap_err().is_over_broad_scope(),
                "`{raw}` is an over-broad-scope rejection"
            );
        }
    }

    #[test]
    fn inbox_scope_is_a_bounded_kind_and_unbounded_inbox_is_rejected() {
        let s = FirehoseScope::parse("inbox:p-opaque-1").expect("a bounded inbox scope parses");
        assert_eq!(
            s.kind(),
            ScopeKind::Inbox,
            "inbox: parses to the Inbox kind"
        );
        assert_eq!(
            s.id(),
            "p-opaque-1",
            "the principal id is the bounded resource id"
        );
        assert_eq!(
            s.selector(),
            "inbox:p-opaque-1",
            "the inbox scope round-trips its selector"
        );
        for raw in ["inbox:*", "inbox:", "inbox"] {
            let r = FirehoseScope::parse(raw);
            assert!(
                r.is_err(),
                "unbounded/empty inbox scope `{raw}` must be rejected, got {r:?}"
            );
            assert!(
                r.unwrap_err().is_over_broad_scope(),
                "`{raw}` is an over-broad-scope rejection"
            );
        }
    }

    #[test]
    fn a_slow_consumer_is_dropped_to_resync_required_with_bounded_memory() {
        let mut fh = Firehose::with_limits(1024, 3);
        let s = scope("channel:firehose");

        let sub = fh.subscribe("chat-live", &s, None).expect("subscribe");
        for _ in 0..3 {
            fh.publish("chat-live", &s, draft("frame"))
                .expect("the fixture publishes a valid frame");
        }
        assert_eq!(sub.ready_len(), 3, "the in-flight queue filled to the cap");
        assert!(
            !sub.resync_required(),
            "not dropped yet (at the cap, not over it)"
        );

        fh.publish("chat-live", &s, draft("over-cap"))
            .expect("the fixture publishes a valid frame");
        assert!(
            sub.resync_required(),
            "a slow consumer is dropped to resync_required (NAMED)"
        );
        assert_eq!(
            sub.ready_len(),
            0,
            "the buffer is RELEASED - memory bounded, the gap NOT buffered"
        );
        assert!(
            sub.pull().is_none(),
            "a dropped subscription delivers nothing until it resumes"
        );
    }

    #[test]
    fn a_keeping_up_consumer_is_never_dropped() {
        let mut fh = Firehose::with_limits(1024, 4);
        let s = scope("channel:eng");
        let sub = fh.subscribe("chat-live", &s, None).expect("subscribe");
        for i in 1..=100u64 {
            fh.publish("chat-live", &s, draft("f"))
                .expect("the fixture publishes a valid frame");
            let pulled = sub
                .pull()
                .expect("a keeping-up consumer always has its frame");
            assert_eq!(pulled.seq, i, "delivered in order");
            assert!(
                sub.ready_len() <= 1,
                "the in-flight stays bounded for a keeping-up consumer"
            );
        }
        assert!(
            !sub.resync_required(),
            "a keeping-up consumer is never dropped"
        );
    }

    #[test]
    fn tail_reads_the_range_the_window_holds() {
        let mut fh = Firehose::new();
        let s = scope("board:logs");
        for _ in 0..10 {
            fh.publish("ci-logs", &s, draft("line"))
                .expect("the fixture publishes a valid frame");
        }
        let mid: Vec<u64> = fh.tail("ci-logs", &s, 3, 6).iter().map(|f| f.seq).collect();
        assert_eq!(
            mid,
            vec![3, 4, 5, 6],
            "tail reads the inclusive [lo, hi] range"
        );
        let tail: Vec<u64> = fh
            .tail("ci-logs", &s, 8, 100)
            .iter()
            .map(|f| f.seq)
            .collect();
        assert_eq!(tail, vec![8, 9, 10], "tail clamps to the held frames");
    }

    #[test]
    fn bounded_tail_checks_count_and_payload_bytes_before_cloning() {
        let mut fh = Firehose::new();
        let s = scope("board:bounded-tail");
        for _ in 0..3 {
            fh.publish("ci-logs", &s, draft("line"))
                .expect("the fixture publishes a valid frame");
        }

        assert_eq!(
            fh.tail_bounded("ci-logs", &s, 1, 3, 3, 12)
                .expect("exact limits accepted")
                .len(),
            3
        );
        assert_eq!(
            fh.tail_bounded("ci-logs", &s, 1, 3, 2, 12),
            Err(FirehoseError::TailLimitExceeded)
        );
        assert_eq!(
            fh.tail_bounded("ci-logs", &s, 1, 3, 3, 11),
            Err(FirehoseError::TailLimitExceeded)
        );
    }

    #[test]
    fn publish_fans_out_to_every_open_subscription() {
        let mut fh = Firehose::new();
        let s = scope("channel:town-hall");
        let a = fh.subscribe("chat-live", &s, None).expect("a subscribes");
        let b = fh.subscribe("chat-live", &s, None).expect("b subscribes");
        fh.publish("chat-live", &s, draft("hello"))
            .expect("the fixture publishes a valid frame");
        assert_eq!(
            a.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            b.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![1],
            "both viewers receive it"
        );
    }

    #[test]
    fn the_resume_cursor_is_the_last_delivered_seq() {
        let mut fh = Firehose::new();
        let s = scope("doc:x");
        for _ in 0..5 {
            fh.publish("kn-ops", &s, draft("op"))
                .expect("the fixture publishes a valid frame");
        }
        let sub = fh
            .resume("kn-ops", &s, 0)
            .expect("fresh client replays the window");
        assert_eq!(sub.pull().map(|f| f.seq), Some(1));
        assert_eq!(sub.pull().map(|f| f.seq), Some(2));
        assert_eq!(
            sub.last_seq(),
            2,
            "the cursor is the last DELIVERED seq (not the head 5)"
        );
        let sub2 = fh
            .resume("kn-ops", &s, sub.last_seq())
            .expect("resume from the partial cursor");
        assert_eq!(
            sub2.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
    }
}
