pub mod ulid;

pub mod pg;
pub mod pg_conversation;

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_events::{
    AggregateKey, DataRole, EventDraft, EventType, OutboxTransaction, OutboxTx as OutboxTxTrait,
    Visibility,
};
use myelin_storage::{BlobStore, ContentHash};
#[cfg(any(test, feature = "test-support"))]
use myelin_storage::FsBlobStore;
use myelin_tenancy::TenantId;

use crate::events::CHAT_MESSAGE_ERASED;
#[cfg(any(test, feature = "test-support"))]
use crate::events::{CHAT_MESSAGE_CREATED, CHAT_MESSAGE_EDITED};

pub use ulid::{MessageId, MonotonicUlidSource, SystemUlidSource, UlidSource};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConversationId {
    pub tenant: String,
    pub region: String,
    pub conversation_id: String,
}

impl ConversationId {
    pub fn new(
        tenant: impl Into<String>,
        region: impl Into<String>,
        conversation_id: impl Into<String>,
    ) -> ConversationId {
        ConversationId {
            tenant: tenant.into(),
            region: region.into(),
            conversation_id: conversation_id.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorKind {
    Human,
    Agent,
    Service,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageState {
    Active,
    Edited,
    Deleted,
    Tombstoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    SubjectErased,
    RetentionPurge,
    Moderation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewMessage {
    pub conv: ConversationId,
    pub thread_root_id: Option<MessageId>,
    pub author: String,
    pub author_kind: AuthorKind,
    pub body_inline: Vec<u8>,
    pub body_nodes: Vec<u8>,
    pub client_nonce: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub message_id: MessageId,
    pub conv: ConversationId,
    pub thread_root_id: Option<MessageId>,
    pub author: String,
    pub author_kind: AuthorKind,
    pub body_inline: Vec<u8>,
    pub body_nodes: Vec<u8>,
    pub client_nonce: String,
    pub edited_seq: i32,
    pub state: MessageState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RangeCursor {
    Recent,
    Before(MessageId),
    After(MessageId),
}

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    CasConflict {
        message_id: MessageId,
        expected: i32,
        actual: i32,
    },
    NotFound(MessageId),
    DuplicateNonce {
        conversation_id: String,
        client_nonce: String,
    },
    Cold(String),
}

impl core::fmt::Display for StoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StoreError::CasConflict {
                message_id,
                expected,
                actual,
            } => write!(
                f,
                "CAS conflict on {message_id:?}: expected edited_seq {expected}, found {actual}"
            ),
            StoreError::NotFound(id) => write!(f, "message {id:?} not found"),
            StoreError::DuplicateNonce {
                conversation_id,
                client_nonce,
            } => write!(
                f,
                "duplicate client_nonce {client_nonce} in conversation {conversation_id}"
            ),
            StoreError::Cold(e) => write!(f, "cold-segment tier error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

pub type Result<T> = core::result::Result<T, StoreError>;

pub type OutboxTx = OutboxTransaction;

fn emit_message_event(
    tx: &mut OutboxTx,
    event_type: &str,
    conv: &ConversationId,
    message_id: &MessageId,
    author: &str,
    thread_root_id: Option<&MessageId>,
) -> Result<()> {
    let subject = crate::subs::mint_message(&conv.tenant, message_id.as_str())
        .map_err(|e| StoreError::Cold(format!("mint message #sub anchor: {e}")))?;
    let draft = EventDraft {
        type_: EventType(event_type.to_string()),
        subject,
        aggregate: AggregateKey(conv.conversation_id.clone()),
        payload: serde_json::json!({
            "conversation_id": conv.conversation_id,
            "message_id": message_id.as_str(),
            "author": author,
            "thread_root_id": thread_root_id.map(|t| t.as_str().to_string()),
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    };
    tx.emit(draft, None)
        .map_err(|e| StoreError::Cold(format!("outbox emit {event_type}: {e:?}")))?;
    Ok(())
}

pub fn emit_erased_tombstone(
    tx: &mut OutboxTx,
    conv: &ConversationId,
    message_id: &MessageId,
    author: &str,
) -> Result<()> {
    tx.stage_state_change(format!("chat.message.erased:{}", message_id.as_str()));
    emit_message_event(tx, CHAT_MESSAGE_ERASED, conv, message_id, author, None)
}

pub trait MessageStore {
    fn append(&self, tx: &mut OutboxTx, msg: NewMessage) -> Result<MessageId>;

    fn range(&self, conv: &ConversationId, cursor: RangeCursor, limit: u32)
        -> Result<Vec<Message>>;

    fn revise(
        &self,
        tx: &mut OutboxTx,
        msg_id: &MessageId,
        body_inline: Vec<u8>,
        body_nodes: Vec<u8>,
        expect_seq: i32,
    ) -> Result<()>;

    fn tombstone(
        &self,
        tx: &mut OutboxTx,
        msg_id: &MessageId,
        reason: TombstoneReason,
    ) -> Result<()>;

    fn resync_from(&self, conv: &ConversationId, cursor: &MessageId) -> Result<Vec<Message>>;
}

#[cfg(any(test, feature = "test-support"))]
pub struct MemHotTier {
    minter: Box<dyn UlidSource>,
    partitions: Mutex<BTreeMap<ConversationId, BTreeMap<MessageId, Message>>>,
    cold: ColdSegments<FsBlobStore>,
}

#[cfg(any(test, feature = "test-support"))]
impl MemHotTier {
    pub fn new() -> MemHotTier {
        MemHotTier::with_source(Box::new(MonotonicUlidSource::new()))
    }

    pub fn with_source(minter: Box<dyn UlidSource>) -> MemHotTier {
        MemHotTier {
            minter,
            partitions: Mutex::new(BTreeMap::new()),
            cold: ColdSegments::new(),
        }
    }

    pub fn cold(&self) -> &ColdSegments<FsBlobStore> {
        &self.cold
    }

    pub fn seal_before(&self, conv: &ConversationId, up_to: &MessageId) -> Result<usize> {
        let mut parts = self.lock();
        let log = match parts.get_mut(conv) {
            Some(log) => log,
            None => return Ok(0),
        };
        let to_seal: Vec<MessageId> = log
            .range(..up_to.clone())
            .map(|(id, _)| id.clone())
            .collect();
        let mut sealed = Vec::with_capacity(to_seal.len());
        for id in &to_seal {
            if let Some(msg) = log.remove(id) {
                sealed.push(msg);
            }
        }
        if sealed.is_empty() {
            return Ok(0);
        }
        let n = sealed.len();
        self.cold.seal(conv, sealed)?;
        Ok(n)
    }

    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<ConversationId, BTreeMap<MessageId, Message>>> {
        self.partitions.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn merged_log(&self, conv: &ConversationId) -> Result<Vec<Message>> {
        let mut out = self.cold.read(conv)?;
        let parts = self.lock();
        if let Some(log) = parts.get(conv) {
            out.extend(log.values().cloned());
        }
        out.sort_by(|a, b| a.message_id.cmp(&b.message_id));
        Ok(out)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for MemHotTier {
    fn default() -> Self {
        MemHotTier::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl MessageStore for MemHotTier {
    fn append(&self, tx: &mut OutboxTx, msg: NewMessage) -> Result<MessageId> {
        let mut parts = self.lock();
        let log = parts.entry(msg.conv.clone()).or_default();
        if let Some(existing) = log
            .values()
            .find(|m| m.client_nonce == msg.client_nonce)
            .map(|m| m.message_id.clone())
        {
            return Ok(existing);
        }
        let message_id = self.minter.mint();
        let conv_key = msg.conv.clone();
        let author = msg.author.clone();
        let thread_root_id = msg.thread_root_id.clone();
        let stored = Message {
            message_id: message_id.clone(),
            conv: msg.conv,
            thread_root_id: msg.thread_root_id,
            author: msg.author,
            author_kind: msg.author_kind,
            body_inline: msg.body_inline,
            body_nodes: msg.body_nodes,
            client_nonce: msg.client_nonce,
            edited_seq: 0,
            state: MessageState::Active,
        };
        log.insert(message_id.clone(), stored);
        drop(parts);
        tx.stage_state_change(format!("chat.message.created:{}", message_id.as_str()));
        emit_message_event(
            tx,
            CHAT_MESSAGE_CREATED,
            &conv_key,
            &message_id,
            &author,
            thread_root_id.as_ref(),
        )?;
        Ok(message_id)
    }

    fn range(
        &self,
        conv: &ConversationId,
        cursor: RangeCursor,
        limit: u32,
    ) -> Result<Vec<Message>> {
        let all = self.merged_log(conv)?;
        let limit = limit as usize;
        let out = match cursor {
            RangeCursor::Recent => {
                let start = all.len().saturating_sub(limit);
                all[start..].to_vec()
            }
            RangeCursor::Before(id) => {
                let before: Vec<Message> = all.into_iter().filter(|m| m.message_id < id).collect();
                let start = before.len().saturating_sub(limit);
                before[start..].to_vec()
            }
            RangeCursor::After(id) => all
                .into_iter()
                .filter(|m| m.message_id > id)
                .take(limit)
                .collect(),
        };
        Ok(out)
    }

    fn revise(
        &self,
        tx: &mut OutboxTx,
        msg_id: &MessageId,
        body_inline: Vec<u8>,
        body_nodes: Vec<u8>,
        expect_seq: i32,
    ) -> Result<()> {
        let mut parts = self.lock();
        let mut found: Option<(ConversationId, String, Option<MessageId>)> = None;
        for log in parts.values_mut() {
            if let Some(msg) = log.get_mut(msg_id) {
                if msg.edited_seq != expect_seq {
                    return Err(StoreError::CasConflict {
                        message_id: msg_id.clone(),
                        expected: expect_seq,
                        actual: msg.edited_seq,
                    });
                }
                msg.body_inline = body_inline;
                msg.body_nodes = body_nodes;
                msg.edited_seq += 1;
                msg.state = MessageState::Edited;
                found = Some((
                    msg.conv.clone(),
                    msg.author.clone(),
                    msg.thread_root_id.clone(),
                ));
                break;
            }
        }
        let (conv, author, thread_root_id) = match found {
            Some(f) => f,
            None => return Err(StoreError::NotFound(msg_id.clone())),
        };
        drop(parts);
        tx.stage_state_change(format!("chat.message.edited:{}", msg_id.as_str()));
        emit_message_event(
            tx,
            CHAT_MESSAGE_EDITED,
            &conv,
            msg_id,
            &author,
            thread_root_id.as_ref(),
        )?;
        Ok(())
    }

    fn tombstone(
        &self,
        tx: &mut OutboxTx,
        msg_id: &MessageId,
        _reason: TombstoneReason,
    ) -> Result<()> {
        let mut parts = self.lock();
        let mut found: Option<(ConversationId, String, Option<MessageId>)> = None;
        for log in parts.values_mut() {
            if let Some(msg) = log.get_mut(msg_id) {
                msg.state = MessageState::Tombstoned;
                msg.body_inline.clear();
                msg.body_nodes.clear();
                found = Some((
                    msg.conv.clone(),
                    msg.author.clone(),
                    msg.thread_root_id.clone(),
                ));
                break;
            }
        }
        let (conv, author, thread_root_id) = match found {
            Some(f) => f,
            None => return Err(StoreError::NotFound(msg_id.clone())),
        };
        drop(parts);
        tx.stage_state_change(format!("chat.message.erased:{}", msg_id.as_str()));
        emit_message_event(
            tx,
            CHAT_MESSAGE_ERASED,
            &conv,
            msg_id,
            &author,
            thread_root_id.as_ref(),
        )?;
        Ok(())
    }

    fn resync_from(&self, conv: &ConversationId, cursor: &MessageId) -> Result<Vec<Message>> {
        let all = self.merged_log(conv)?;
        Ok(all.into_iter().filter(|m| &m.message_id > cursor).collect())
    }
}

#[cfg_attr(not(any(test, feature = "test-support")), allow(dead_code))]
pub struct ColdSegments<B: BlobStore> {
    blob: B,
    index: Mutex<BTreeMap<ConversationId, Vec<ContentHash>>>,
}

#[cfg(any(test, feature = "test-support"))]
impl ColdSegments<FsBlobStore> {
    pub fn new() -> ColdSegments<FsBlobStore> {
        ColdSegments::with_blob_store(FsBlobStore::new())
    }
}

#[cfg_attr(not(any(test, feature = "test-support")), allow(dead_code))]
impl<B: BlobStore> ColdSegments<B> {
    pub fn with_blob_store(blob: B) -> ColdSegments<B> {
        ColdSegments {
            blob,
            index: Mutex::new(BTreeMap::new()),
        }
    }

    fn seal(&self, conv: &ConversationId, messages: Vec<Message>) -> Result<()> {
        let bytes = encode_segment(&messages);
        let tenant = TenantId(conv.tenant.clone());
        let hash = self
            .blob
            .put(&tenant, &bytes)
            .map_err(|e| StoreError::Cold(e.to_string()))?;
        self.index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(conv.clone())
            .or_default()
            .push(hash);
        Ok(())
    }

    fn read(&self, conv: &ConversationId) -> Result<Vec<Message>> {
        let hashes = {
            let index = self.index.lock().unwrap_or_else(|e| e.into_inner());
            match index.get(conv) {
                Some(h) => h.clone(),
                None => return Ok(Vec::new()),
            }
        };
        let tenant = TenantId(conv.tenant.clone());
        let mut out = Vec::new();
        for hash in &hashes {
            let bytes = self
                .blob
                .get(&tenant, hash)
                .map_err(|e| StoreError::Cold(e.to_string()))?;
            out.extend(decode_segment(&bytes)?);
        }
        out.sort_by(|a, b| a.message_id.cmp(&b.message_id));
        Ok(out)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for ColdSegments<FsBlobStore> {
    fn default() -> Self {
        ColdSegments::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColdBlobParityVerdict {
    pub fs_address: ContentHash,
    pub object_address: ContentHash,
    pub byte_identical: bool,
}

pub fn chat_cold_blob_store_parity<F, O>(
    fs: &F,
    object: &O,
    tenant: &TenantId,
    messages: &[Message],
) -> Result<ColdBlobParityVerdict>
where
    F: BlobStore,
    O: BlobStore,
{
    let bytes = encode_segment(messages);
    let fs_address = fs
        .put(tenant, &bytes)
        .map_err(|e| StoreError::Cold(e.to_string()))?;
    let object_address = object
        .put(tenant, &bytes)
        .map_err(|e| StoreError::Cold(e.to_string()))?;
    let fs_back = fs
        .get(tenant, &fs_address)
        .map_err(|e| StoreError::Cold(e.to_string()))?;
    let object_back = object
        .get(tenant, &object_address)
        .map_err(|e| StoreError::Cold(e.to_string()))?;
    let fs_rows = decode_segment(&fs_back)?;
    let object_rows = decode_segment(&object_back)?;
    let address_identical = fs_address == object_address;
    let fs_roundtrip_ok = fs_rows == messages;
    let object_roundtrip_ok = object_rows == messages;
    let byte_identical = address_identical && fs_roundtrip_ok && object_roundtrip_ok;
    Ok(ColdBlobParityVerdict {
        fs_address,
        object_address,
        byte_identical,
    })
}

pub const SCYLLA_HOT_TIER_PROMOTED: bool = false;

pub const SCYLLA_PROMOTION_TRIGGER: &str =
    "measured per-cell message-store write/partition volume crossing the hot-tier budget (R-C6/R-5)";

pub const SCYLLA_PROMOTION_LANDING: &str = "CHAT-P28 / P-502";

fn encode_segment(messages: &[Message]) -> Vec<u8> {
    let mut buf = Vec::new();
    for m in messages {
        let line = serde_json::to_string(&SegmentRow::from(m)).expect("segment row serialises");
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
    }
    buf
}

fn decode_segment(bytes: &[u8]) -> Result<Vec<Message>> {
    let mut out = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let row: SegmentRow = serde_json::from_slice(line)
            .map_err(|e| StoreError::Cold(format!("decode segment row: {e}")))?;
        out.push(row.into());
    }
    Ok(out)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SegmentRow {
    message_id: String,
    tenant: String,
    region: String,
    conversation_id: String,
    thread_root_id: Option<String>,
    author: String,
    author_kind: u8,
    body_inline: Vec<u8>,
    body_nodes: Vec<u8>,
    client_nonce: String,
    edited_seq: i32,
    state: u8,
}

impl SegmentRow {
    fn from(m: &Message) -> SegmentRow {
        SegmentRow {
            message_id: m.message_id.0.clone(),
            tenant: m.conv.tenant.clone(),
            region: m.conv.region.clone(),
            conversation_id: m.conv.conversation_id.clone(),
            thread_root_id: m.thread_root_id.as_ref().map(|t| t.0.clone()),
            author: m.author.clone(),
            author_kind: author_kind_code(m.author_kind),
            body_inline: m.body_inline.clone(),
            body_nodes: m.body_nodes.clone(),
            client_nonce: m.client_nonce.clone(),
            edited_seq: m.edited_seq,
            state: state_code(m.state),
        }
    }
}

impl From<SegmentRow> for Message {
    fn from(r: SegmentRow) -> Message {
        Message {
            message_id: MessageId(r.message_id),
            conv: ConversationId {
                tenant: r.tenant,
                region: r.region,
                conversation_id: r.conversation_id,
            },
            thread_root_id: r.thread_root_id.map(MessageId),
            author: r.author,
            author_kind: author_kind_from_code(r.author_kind),
            body_inline: r.body_inline,
            body_nodes: r.body_nodes,
            client_nonce: r.client_nonce,
            edited_seq: r.edited_seq,
            state: state_from_code(r.state),
        }
    }
}

fn author_kind_code(k: AuthorKind) -> u8 {
    match k {
        AuthorKind::Human => 0,
        AuthorKind::Agent => 1,
        AuthorKind::Service => 2,
    }
}

fn author_kind_from_code(c: u8) -> AuthorKind {
    match c {
        1 => AuthorKind::Agent,
        2 => AuthorKind::Service,
        _ => AuthorKind::Human,
    }
}

fn state_code(s: MessageState) -> u8 {
    match s {
        MessageState::Active => 0,
        MessageState::Edited => 1,
        MessageState::Deleted => 2,
        MessageState::Tombstoned => 3,
    }
}

fn state_from_code(c: u8) -> MessageState {
    match c {
        1 => MessageState::Edited,
        2 => MessageState::Deleted,
        3 => MessageState::Tombstoned,
        _ => MessageState::Active,
    }
}

#[cfg(test)]
mod tests;
