use sqlx::Row;

use super::{row_to_message, PgMessageStore};
use crate::store::{
    ConversationId, MessageId, MessageState, RangeCursor, StoreError, TimelineMessage,
};

pub(super) struct ThreadRoot {
    pub(super) notification_recipient: Option<String>,
}

impl PgMessageStore {
    pub(super) async fn require_thread_root_in_tx(
        &self,
        connection: &mut sqlx::PgConnection,
        conversation: &ConversationId,
        root: &MessageId,
    ) -> Result<ThreadRoot, StoreError> {
        let participant_table = self.thread_participant_table();
        let row = sqlx::query(&format!(
            "SELECT root.thread_root_id, root.state, participant.principal_id \
               FROM {table} root \
               LEFT JOIN {participant_table} participant \
                 ON participant.tenant_id = root.tenant_id \
                AND participant.region = root.region \
                AND participant.conversation_id = root.conversation_id \
                AND participant.thread_root_id = root.message_id \
                AND participant.role = 0 \
              WHERE root.tenant_id = $1 AND root.region = $2 \
                AND root.conversation_id = $3 AND root.message_id = $4 \
              FOR SHARE OF root",
            table = self.table,
        ))
        .bind(&conversation.tenant)
        .bind(&conversation.region)
        .bind(&conversation.conversation_id)
        .bind(root.as_str())
        .fetch_optional(connection)
        .await
        .map_err(|error| StoreError::Cold(format!("thread root select: {error}")))?
        .ok_or_else(|| StoreError::NotFound(root.clone()))?;
        if row.get::<Option<String>, _>("thread_root_id").is_some() {
            return Err(StoreError::NotFound(root.clone()));
        }
        let stored_state: i16 = row.get("state");
        let state = super::decode_message_state(stored_state)?;
        if matches!(state, MessageState::Deleted | MessageState::Tombstoned) {
            return Err(StoreError::CasConflict {
                message_id: root.clone(),
                expected: 0,
                actual: i32::from(stored_state),
            });
        }
        Ok(ThreadRoot {
            notification_recipient: row.get("principal_id"),
        })
    }

    /// Pages only roots for the calm, top-level conversation timeline.
    pub async fn range_roots(
        &self,
        conversation: &ConversationId,
        cursor: RangeCursor,
        limit: u32,
    ) -> Result<Vec<TimelineMessage>, StoreError> {
        self.range_timeline(conversation, None, cursor, limit).await
    }

    /// Pages replies belonging to one root. The root is read separately so it
    /// remains visible even when the reply history spans many pages.
    pub async fn range_replies(
        &self,
        conversation: &ConversationId,
        root: &MessageId,
        cursor: RangeCursor,
        limit: u32,
    ) -> Result<Vec<TimelineMessage>, StoreError> {
        self.range_timeline(conversation, Some(root), cursor, limit)
            .await
    }

    pub async fn reply_count(
        &self,
        conversation: &ConversationId,
        root: &MessageId,
    ) -> Result<u64, StoreError> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| StoreError::Cold(format!("acquire: {error}")))?;
        self.set_session_scope(&mut connection, &conversation.tenant, &conversation.region)
            .await?;
        let count = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT COUNT(*) FROM {} \
             WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 \
               AND thread_root_id = $4",
            self.table,
        ))
        .bind(&conversation.tenant)
        .bind(&conversation.region)
        .bind(&conversation.conversation_id)
        .bind(root.as_str())
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| StoreError::Cold(format!("reply count: {error}")))?;
        u64::try_from(count).map_err(|_| StoreError::Cold("stored reply count is negative".into()))
    }

    async fn range_timeline(
        &self,
        conversation: &ConversationId,
        root: Option<&MessageId>,
        cursor: RangeCursor,
        limit: u32,
    ) -> Result<Vec<TimelineMessage>, StoreError> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| StoreError::Cold(format!("acquire: {error}")))?;
        self.set_session_scope(&mut connection, &conversation.tenant, &conversation.region)
            .await?;
        let (cursor_predicate, cursor_id, order) = match &cursor {
            RangeCursor::Recent => ("", None, "DESC"),
            RangeCursor::Before(id) => ("AND message.message_id < $6", Some(id.as_str()), "DESC"),
            RangeCursor::After(id) => ("AND message.message_id > $6", Some(id.as_str()), "ASC"),
        };
        let sql = format!(
            "SELECT message.tenant_id, message.region, message.conversation_id, \
                    message.message_id, message.thread_root_id, message.author, \
                    message.author_kind, message.body_inline, message.body_nodes, \
                    message.client_nonce, message.edited_seq, message.state, \
                    CASE WHEN message.thread_root_id IS NULL THEN ( \
                        SELECT COUNT(*) FROM {table} reply \
                         WHERE reply.tenant_id = message.tenant_id \
                           AND reply.region = message.region \
                           AND reply.conversation_id = message.conversation_id \
                           AND reply.thread_root_id = message.message_id \
                    ) ELSE 0 END AS reply_count \
               FROM {table} message \
              WHERE message.tenant_id = $1 AND message.region = $2 \
                AND message.conversation_id = $3 \
                AND (($4::text IS NULL AND message.thread_root_id IS NULL) \
                     OR message.thread_root_id = $4) \
                {cursor_predicate} \
              ORDER BY message.message_id {order} LIMIT $5",
            table = self.table,
        );
        let mut query = sqlx::query(&sql)
            .bind(&conversation.tenant)
            .bind(&conversation.region)
            .bind(&conversation.conversation_id)
            .bind(root.map(MessageId::as_str))
            .bind(i64::from(limit));
        if let Some(cursor_id) = cursor_id {
            query = query.bind(cursor_id);
        }
        let rows = query
            .fetch_all(&mut *connection)
            .await
            .map_err(|error| StoreError::Cold(format!("timeline select: {error}")))?;
        let mut messages = rows
            .iter()
            .map(|row| {
                let count = row.get::<i64, _>("reply_count");
                Ok(TimelineMessage {
                    message: row_to_message(row)?,
                    reply_count: u64::try_from(count)
                        .map_err(|_| StoreError::Cold("stored reply count is negative".into()))?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        messages.sort_by(|left, right| left.message.message_id.cmp(&right.message.message_id));
        Ok(messages)
    }
}
