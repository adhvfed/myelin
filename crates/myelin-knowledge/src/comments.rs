use myelin_content::block::Block;
use myelin_events::{EventId, OutboxTx};
use myelin_refs::{mint, ArtifactRef, ParseError, Sub, SubKind, SubKindRegistration};
use myelin_tenancy::TenantId;
use std::collections::BTreeMap;

use crate::block_tree::BlockId;
use crate::emit::{emit_change, KnowledgeChange};
use crate::subs::KNOWLEDGE_SUBSYSTEM;

pub const KNOWLEDGE_COMMENT_SUB_KINDS: &[SubKind] = &[SubKind::Comment, SubKind::Thread];

pub fn register_knowledge_comment_kinds(
) -> Result<SubKindRegistration, myelin_refs::RegistrationError> {
    SubKindRegistration {
        subsystem: KNOWLEDGE_SUBSYSTEM.to_string(),
        kinds: KNOWLEDGE_COMMENT_SUB_KINDS.to_vec(),
    }
    .validate()
}

fn page_root(tenant: &TenantId, page_id: &str) -> Result<ArtifactRef, ParseError> {
    myelin_refs::parse(&format!("myelin://{}/knowledge/page/{}", tenant.0, page_id))
}

pub fn mint_thread(
    tenant: &TenantId,
    page_id: &str,
    thread_id: &str,
) -> Result<ArtifactRef, ParseError> {
    mint(
        &page_root(tenant, page_id)?,
        Sub::Thread(thread_id.to_string()),
    )
}

pub fn mint_comment(
    tenant: &TenantId,
    page_id: &str,
    comment_id: &str,
) -> Result<ArtifactRef, ParseError> {
    mint(
        &page_root(tenant, page_id)?,
        Sub::Comment(comment_id.to_string()),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommentAnchor {
    Block {
        block_id: BlockId,
    },
    Range {
        block_id: BlockId,
        start: usize,
        end: usize,
    },
}

impl CommentAnchor {
    pub fn block_id(&self) -> &BlockId {
        match self {
            CommentAnchor::Block { block_id } | CommentAnchor::Range { block_id, .. } => block_id,
        }
    }

    pub fn block(block_id: BlockId) -> Self {
        CommentAnchor::Block { block_id }
    }

    pub fn range(block_id: BlockId, start: usize, end: usize) -> Option<Self> {
        (start < end).then_some(CommentAnchor::Range {
            block_id,
            start,
            end,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub comment_id: String,
    pub body: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentThread {
    pub thread_id: String,
    pub anchor: CommentAnchor,
    pub comments: Vec<Comment>,
    pub resolved: bool,
}

impl CommentThread {
    pub fn anchored_block(&self) -> &BlockId {
        self.anchor.block_id()
    }
}

#[derive(Debug, Default)]
pub struct CommentStore {
    threads: BTreeMap<String, CommentThread>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CommentError {
    Ungrammatical(String),
    DuplicateThread(String),
    NoSuchThread(String),
    DegenerateRange { start: usize, end: usize },
}

impl std::fmt::Display for CommentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommentError::Ungrammatical(s) => write!(f, "ungrammatical comment/thread mint: {s}"),
            CommentError::DuplicateThread(t) => write!(f, "thread_id already live: {t}"),
            CommentError::NoSuchThread(t) => write!(f, "no such thread: {t}"),
            CommentError::DegenerateRange { start, end } => {
                write!(
                    f,
                    "degenerate text-range anchor: start {start} >= end {end}"
                )
            }
        }
    }
}

impl std::error::Error for CommentError {}

impl CommentStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn thread(&self, thread_id: &str) -> Option<&CommentThread> {
        self.threads.get(thread_id)
    }

    pub fn threads_on_block(&self, block_id: &BlockId) -> Vec<&CommentThread> {
        self.threads
            .values()
            .filter(|t| t.anchored_block() == block_id)
            .collect()
    }

    pub fn create_thread(
        &mut self,
        tenant: &TenantId,
        page_id: &str,
        thread_id: String,
        comment_id: String,
        anchor: CommentAnchor,
        body: Vec<Block>,
    ) -> Result<&CommentThread, CommentError> {
        mint_thread(tenant, page_id, &thread_id)
            .map_err(|e| CommentError::Ungrammatical(e.to_string()))?;
        mint_comment(tenant, page_id, &comment_id)
            .map_err(|e| CommentError::Ungrammatical(e.to_string()))?;
        if let CommentAnchor::Range { start, end, .. } = &anchor {
            if start >= end {
                return Err(CommentError::DegenerateRange {
                    start: *start,
                    end: *end,
                });
            }
        }
        if self.threads.contains_key(&thread_id) {
            return Err(CommentError::DuplicateThread(thread_id));
        }
        let thread = CommentThread {
            thread_id: thread_id.clone(),
            anchor,
            comments: vec![Comment { comment_id, body }],
            resolved: false,
        };
        Ok(self.threads.entry(thread_id).or_insert(thread))
    }

    pub fn resolve_thread(&mut self, thread_id: &str) -> Result<&CommentThread, CommentError> {
        let thread = self
            .threads
            .get_mut(thread_id)
            .ok_or_else(|| CommentError::NoSuchThread(thread_id.to_string()))?;
        thread.resolved = true;
        Ok(thread)
    }

    pub fn reopen_thread(&mut self, thread_id: &str) -> Result<&CommentThread, CommentError> {
        let thread = self
            .threads
            .get_mut(thread_id)
            .ok_or_else(|| CommentError::NoSuchThread(thread_id.to_string()))?;
        thread.resolved = false;
        Ok(thread)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn create_comment(
    store: &mut CommentStore,
    tx: &mut dyn OutboxTx,
    tenant: &TenantId,
    page_id: &str,
    thread_id: String,
    comment_id: String,
    anchor: CommentAnchor,
    body: Vec<Block>,
) -> Result<EventId, CommentOpError> {
    store
        .create_thread(tenant, page_id, thread_id, comment_id.clone(), anchor, body)
        .map_err(CommentOpError::Store)?;
    let change = KnowledgeChange::CommentCreated {
        page_id: page_id.to_string(),
        comment_id,
    };
    emit_change(tx, tenant, &change, None).map_err(CommentOpError::Bus)
}

pub fn resolve_comment(
    store: &mut CommentStore,
    tx: &mut dyn OutboxTx,
    tenant: &TenantId,
    page_id: &str,
    thread_id: &str,
    root_comment_id: String,
) -> Result<EventId, CommentOpError> {
    store
        .resolve_thread(thread_id)
        .map_err(CommentOpError::Store)?;
    let change = KnowledgeChange::CommentResolved {
        page_id: page_id.to_string(),
        comment_id: root_comment_id,
    };
    emit_change(tx, tenant, &change, None).map_err(CommentOpError::Bus)
}

#[derive(Debug)]
pub enum CommentOpError {
    Store(CommentError),
    Bus(myelin_events::OutboxError),
}

impl std::fmt::Display for CommentOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommentOpError::Store(e) => write!(f, "comment store rejected: {e}"),
            CommentOpError::Bus(e) => write!(f, "comment event emit failed: {e:?}"),
        }
    }
}

impl std::error::Error for CommentOpError {}

#[cfg(test)]
mod tests;
