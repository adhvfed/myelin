#[cfg(feature = "integration")]
pub mod pg;

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use myelin_events::firehose::FirehoseScope;
use myelin_storage::cache::Cache;
use myelin_tenancy::TenantId;

use crate::events::CHAT_READ_STATE_UPDATED;
use crate::store::{ConversationId, MessageId, MessageStore};

pub const DEFAULT_FLUSH_CADENCE: Duration = Duration::from_secs(2);

pub const HOT_MARKER_TTL: Duration = Duration::from_secs(300);

pub const CHAT_READ_STATE_STORE: &str = "chat_read_state";

pub trait ReadStatePush {
    fn push_read_state(&self, scope: &FirehoseScope, marker: &ReadMarker) -> u64;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadMarker {
    pub conv: ConversationId,
    pub principal: String,
    pub last_read: MessageId,
}

impl ReadMarker {
    pub fn new(
        conv: ConversationId,
        principal: impl Into<String>,
        last_read: MessageId,
    ) -> ReadMarker {
        ReadMarker {
            conv,
            principal: principal.into(),
            last_read,
        }
    }

    pub fn cache_key(conv: &ConversationId, principal: &str) -> String {
        format!(
            "read:{}:{}:{}",
            conv.region, principal, conv.conversation_id
        )
    }
}

#[derive(Default)]
pub struct ReadStateRecord {
    records: Mutex<BTreeMap<(ConversationId, String), MessageId>>,
}

impl ReadStateRecord {
    pub fn new() -> ReadStateRecord {
        ReadStateRecord::default()
    }

    pub fn upsert(&self, marker: &ReadMarker) -> bool {
        let mut g = self.records.lock().unwrap_or_else(|e| e.into_inner());
        let key = (marker.conv.clone(), marker.principal.clone());
        match g.get(&key) {
            Some(existing) if *existing >= marker.last_read => false,
            _ => {
                g.insert(key, marker.last_read.clone());
                true
            }
        }
    }

    pub fn load(&self, conv: &ConversationId, principal: &str) -> Option<MessageId> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(conv.clone(), principal.to_string()))
            .cloned()
    }

    pub fn purge_principal(&self, principal: &str) -> usize {
        let mut g = self.records.lock().unwrap_or_else(|e| e.into_inner());
        let before = g.len();
        g.retain(|(_, p), _| p != principal);
        before - g.len()
    }

    pub fn len(&self) -> usize {
        self.records.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct ReadStateService<'a, S: MessageStore, C: Cache> {
    cache: C,
    record: ReadStateRecord,
    store: &'a S,
    pending: Mutex<BTreeMap<(ConversationId, String), MessageId>>,
}

impl<'a, S: MessageStore, C: Cache> ReadStateService<'a, S, C> {
    pub fn new(cache: C, store: &'a S) -> ReadStateService<'a, S, C> {
        ReadStateService {
            cache,
            record: ReadStateRecord::new(),
            store,
            pending: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn record(&self) -> &ReadStateRecord {
        &self.record
    }

    fn tenant_of(conv: &ConversationId) -> TenantId {
        TenantId(conv.tenant.clone())
    }

    pub fn mark_read(&self, marker: ReadMarker) -> ReadMarker {
        let key = ReadMarker::cache_key(&marker.conv, &marker.principal);
        let tenant = Self::tenant_of(&marker.conv);
        let _ = self.cache.set(
            &tenant,
            &key,
            marker.last_read.as_str().as_bytes(),
            HOT_MARKER_TTL,
        );
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                (marker.conv.clone(), marker.principal.clone()),
                marker.last_read.clone(),
            );
        marker
    }

    pub fn mark_read_and_push<P: ReadStatePush>(
        &self,
        marker: ReadMarker,
        push: &P,
    ) -> Result<u64, myelin_events::firehose::FirehoseError> {
        let scope = crate::glue::chat_channel_scope(&marker.conv.conversation_id)?;
        let marker = self.mark_read(marker);
        Ok(push.push_read_state(&scope, &marker))
    }

    pub fn read_pos(&self, conv: &ConversationId, principal: &str) -> Option<MessageId> {
        let key = ReadMarker::cache_key(conv, principal);
        let tenant = Self::tenant_of(conv);
        if let Ok(Some(bytes)) = self.cache.get(&tenant, &key) {
            if let Ok(s) = String::from_utf8(bytes) {
                return Some(MessageId(s));
            }
        }
        self.record.load(conv, principal)
    }

    pub fn flush(&self) -> usize {
        let drained: Vec<((ConversationId, String), MessageId)> = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *pending).into_iter().collect()
        };
        let mut advanced = 0;
        for ((conv, principal), last_read) in drained {
            let marker = ReadMarker {
                conv,
                principal,
                last_read,
            };
            if self.record.upsert(&marker) {
                advanced += 1;
            }
        }
        advanced
    }

    pub fn pending_len(&self) -> usize {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn drop_cache(&self, conv: &ConversationId, principal: &str) {
        let key = ReadMarker::cache_key(conv, principal);
        let tenant = Self::tenant_of(conv);
        let _ = self.cache.delete(&tenant, &key);
    }

    pub fn unread_count(
        &self,
        conv: &ConversationId,
        principal: &str,
    ) -> Result<usize, crate::store::StoreError> {
        match self.read_pos(conv, principal) {
            Some(last_read) => Ok(self.store.resync_from(conv, &last_read)?.len()),
            None => Ok(self
                .store
                .range(conv, crate::store::RangeCursor::Recent, u32::MAX)?
                .len()),
        }
    }

    pub fn ambient_post_unread_writes(&self, _member_count: usize) -> usize {
        0
    }
}

pub const READ_STATE_UPDATED: &str = CHAT_READ_STATE_UPDATED;

#[cfg(test)]
mod tests;
