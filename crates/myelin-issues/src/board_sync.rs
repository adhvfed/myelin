use std::collections::HashMap;

use myelin_events::{
    Firehose, FirehoseError, FirehoseScope, FirehoseSubscription, Frame, FrameDraft,
};
use myelin_substrate::firehose_selector::ScopeWindow;

pub const BOARD_FIREHOSE_STREAM_PREFIX: &str = "fan";

pub fn board_stream(tenant: &str, project: &str) -> String {
    format!("{BOARD_FIREHOSE_STREAM_PREFIX}.{tenant}.{project}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardCard {
    pub issue_id: String,
    pub state_category: String,
    pub order_key: String,
}

impl BoardCard {
    pub fn new(
        issue_id: impl Into<String>,
        state_category: impl Into<String>,
        order_key: impl Into<String>,
    ) -> BoardCard {
        BoardCard {
            issue_id: issue_id.into(),
            state_category: state_category.into(),
            order_key: order_key.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoardOp {
    Upsert(BoardCard),
    Move {
        issue_id: String,
        state_category: String,
    },
    Reorder {
        issue_id: String,
        order_key: String,
    },
    Remove {
        issue_id: String,
    },
}

impl BoardOp {
    pub fn issue_id(&self) -> &str {
        match self {
            BoardOp::Upsert(c) => &c.issue_id,
            BoardOp::Move { issue_id, .. }
            | BoardOp::Reorder { issue_id, .. }
            | BoardOp::Remove { issue_id } => issue_id,
        }
    }

    pub fn encode(&self) -> String {
        match self {
            BoardOp::Upsert(c) => {
                format!("upsert|{}|{}|{}", c.issue_id, c.state_category, c.order_key)
            }
            BoardOp::Move {
                issue_id,
                state_category,
            } => format!("move|{issue_id}|{state_category}"),
            BoardOp::Reorder {
                issue_id,
                order_key,
            } => {
                format!("reorder|{issue_id}|{order_key}")
            }
            BoardOp::Remove { issue_id } => format!("remove|{issue_id}"),
        }
    }

    pub fn decode(payload: &str) -> Option<BoardOp> {
        let mut parts = payload.split('|');
        match parts.next()? {
            "upsert" => {
                let issue_id = parts.next()?.to_string();
                let state_category = parts.next()?.to_string();
                let order_key = parts.next()?.to_string();
                Some(BoardOp::Upsert(BoardCard {
                    issue_id,
                    state_category,
                    order_key,
                }))
            }
            "move" => Some(BoardOp::Move {
                issue_id: parts.next()?.to_string(),
                state_category: parts.next()?.to_string(),
            }),
            "reorder" => Some(BoardOp::Reorder {
                issue_id: parts.next()?.to_string(),
                order_key: parts.next()?.to_string(),
            }),
            "remove" => Some(BoardOp::Remove {
                issue_id: parts.next()?.to_string(),
            }),
            _ => None,
        }
    }

    pub fn to_draft(&self) -> FrameDraft {
        FrameDraft::new(self.encode())
    }
}

#[derive(Clone, Debug, Default)]
pub struct BoardCache {
    cards: HashMap<String, BoardCard>,
}

impl BoardCache {
    pub fn new() -> BoardCache {
        BoardCache::default()
    }

    pub fn apply(&mut self, op: &BoardOp) {
        match op {
            BoardOp::Upsert(card) => {
                self.cards.insert(card.issue_id.clone(), card.clone());
            }
            BoardOp::Move {
                issue_id,
                state_category,
            } => {
                if let Some(card) = self.cards.get_mut(issue_id) {
                    card.state_category = state_category.clone();
                }
            }
            BoardOp::Reorder {
                issue_id,
                order_key,
            } => {
                if let Some(card) = self.cards.get_mut(issue_id) {
                    card.order_key = order_key.clone();
                }
            }
            BoardOp::Remove { issue_id } => {
                self.cards.remove(issue_id);
            }
        }
    }

    pub fn card(&self, issue_id: &str) -> Option<&BoardCard> {
        self.cards.get(issue_id)
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    pub fn lane(&self, state_category: &str) -> Vec<BoardCard> {
        let mut lane: Vec<BoardCard> = self
            .cards
            .values()
            .filter(|c| c.state_category == state_category)
            .cloned()
            .collect();
        lane.sort_by(|a, b| a.order_key.cmp(&b.order_key));
        lane
    }

    fn replace_from_snapshot(&mut self, cards: Vec<BoardCard>) {
        self.cards = cards.into_iter().map(|c| (c.issue_id.clone(), c)).collect();
    }
}

#[derive(Clone, Debug)]
struct PendingMutation {
    op: BoardOp,
    prior: Option<BoardCard>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalMutationError {
    AlreadyPending { mutation_id: String },
}

impl core::fmt::Display for LocalMutationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LocalMutationError::AlreadyPending { mutation_id } => {
                write!(f, "optimistic mutation `{mutation_id}` is already pending")
            }
        }
    }
}

impl std::error::Error for LocalMutationError {}

pub struct BoardSync {
    stream: String,
    scope: FirehoseScope,
    window: ScopeWindow,
    sub: Option<FirehoseSubscription>,
    last_seq: u64,
    cache: BoardCache,
    pending: HashMap<String, PendingMutation>,
    resync_required_count: u64,
}

impl BoardSync {
    pub fn open(
        stream: impl Into<String>,
        board_scope: &str,
        window: ScopeWindow,
    ) -> Result<BoardSync, FirehoseError> {
        let scope = FirehoseScope::parse(board_scope)?;
        Ok(BoardSync {
            stream: stream.into(),
            scope,
            window,
            sub: None,
            last_seq: 0,
            cache: BoardCache::new(),
            pending: HashMap::new(),
            resync_required_count: 0,
        })
    }

    pub fn subscribe(
        &mut self,
        fh: &mut Firehose,
        cursor: Option<u64>,
    ) -> Result<(), FirehoseError> {
        let sub = fh.subscribe(&self.stream, &self.scope, cursor)?;
        self.drain_into_cache(&sub);
        self.sub = Some(sub);
        Ok(())
    }

    pub fn pump(&mut self) -> usize {
        let Some(sub) = self.sub.clone() else {
            return 0;
        };
        self.drain_into_cache(&sub)
    }

    fn drain_into_cache(&mut self, sub: &FirehoseSubscription) -> usize {
        let mut applied = 0;
        for frame in sub.drain_ready() {
            self.apply_frame(&frame);
            applied += 1;
        }
        applied
    }

    fn apply_frame(&mut self, frame: &Frame) {
        if let Some(op) = BoardOp::decode(&frame.payload.0) {
            self.cache.apply(&op);
        }
        self.last_seq = self.last_seq.max(frame.seq);
    }

    pub fn reconnect(&mut self, fh: &mut Firehose) -> Result<usize, FirehoseError> {
        self.sub = None;
        let before = self.last_seq;
        self.subscribe(fh, Some(self.last_seq))?;
        Ok((self.last_seq - before) as usize)
    }

    pub fn resync_from_snapshot(
        &mut self,
        fh: &mut Firehose,
        snapshot: Vec<BoardCard>,
        as_of_seq: u64,
    ) -> Result<(), FirehoseError> {
        self.cache.replace_from_snapshot(snapshot);
        self.last_seq = as_of_seq;
        self.pending.clear();
        self.resync_required_count += 1;
        self.sub = None;
        self.subscribe(fh, Some(as_of_seq))
    }

    pub fn apply_local(
        &mut self,
        mutation_id: impl Into<String>,
        op: BoardOp,
    ) -> Result<(), LocalMutationError> {
        let mutation_id = mutation_id.into();
        if self.pending.contains_key(&mutation_id) {
            return Err(LocalMutationError::AlreadyPending { mutation_id });
        }
        let prior = self.cache.card(op.issue_id()).cloned();
        self.cache.apply(&op);
        self.pending
            .insert(mutation_id, PendingMutation { op, prior });
        Ok(())
    }

    pub fn confirm_local(&mut self, mutation_id: &str) -> bool {
        self.pending.remove(mutation_id).is_some()
    }

    pub fn reject_local(&mut self, mutation_id: &str) -> bool {
        let Some(pending) = self.pending.remove(mutation_id) else {
            return false;
        };
        let issue_id = pending.op.issue_id().to_string();
        match pending.prior {
            Some(prior) => self.cache.apply(&BoardOp::Upsert(prior)),
            None => self.cache.apply(&BoardOp::Remove { issue_id }),
        }
        true
    }

    pub fn cache(&self) -> &BoardCache {
        &self.cache
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    pub fn scope(&self) -> &FirehoseScope {
        &self.scope
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub fn window(&self) -> &ScopeWindow {
        &self.window
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn resync_required_count(&self) -> u64 {
        self.resync_required_count
    }

    pub fn is_connected(&self) -> bool {
        self.sub.is_some()
    }
}

#[cfg(test)]
mod tests;
