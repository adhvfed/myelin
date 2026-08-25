use sqlx::{Acquire, Row};

use myelin_events::{derive_envelope, EmitContext, IdMinter};
use myelin_tenancy::{Region, TenantId};

use super::{
    AuthoredMessageEraseReceipt, AuthoredMessageErasureState, MessageErasureAttempt, PgMessageStore,
};
use crate::events::CHAT_MESSAGE_ERASED;
use crate::store::{message_event_draft, ConversationId, MessageId, StoreError};

#[derive(Debug)]
struct TombstonedMessage {
    conversation: ConversationId,
    message_id: MessageId,
    author: String,
    thread_root_id: Option<MessageId>,
}

impl PgMessageStore {
    /// Persists the operation before the caller performs irreversible key
    /// destruction. While this marker is incomplete, production appends for
    /// the author are refused rather than racing the erase.
    pub async fn prepare_author_erasure(
        &self,
        tenant: &str,
        author: &str,
        operation_id: &str,
    ) -> Result<AuthoredMessageErasureState, StoreError> {
        validate_erasure_identity(tenant, author, operation_id)?;
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| StoreError::Cold(format!("acquire: {error}")))?;
        self.set_session_scope(&mut connection, tenant, &self.region)
            .await?;
        let mut transaction = connection
            .begin()
            .await
            .map_err(|error| StoreError::Cold(format!("begin Chat erasure marker: {error}")))?;
        lock_author(&mut transaction, tenant, author, AuthorLock::Exclusive).await?;

        sqlx::query(&format!(
            "INSERT INTO {} (tenant_id, region, operation_id, author) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
            self.erasure_operation_table(),
        ))
        .bind(tenant)
        .bind(&self.region)
        .bind(operation_id)
        .bind(author)
        .execute(&mut *transaction)
        .await
        .map_err(|error| StoreError::Cold(format!("prepare Chat erasure marker: {error}")))?;
        let row = sqlx::query(&format!(
            "SELECT author, completed_at IS NOT NULL AS completed, \
                    messages_erased, events_emitted \
               FROM {} \
              WHERE tenant_id = $1 AND region = $2 AND operation_id = $3",
            self.erasure_operation_table(),
        ))
        .bind(tenant)
        .bind(&self.region)
        .bind(operation_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| StoreError::Cold(format!("read Chat erasure marker: {error}")))?;
        require_same_author(&row, author)?;
        let state = erasure_state_from_row(&row)?;
        transaction
            .commit()
            .await
            .map_err(|error| StoreError::Cold(format!("commit Chat erasure marker: {error}")))?;
        Ok(state)
    }

    /// Tombstones every live message written by one pseudonymous author and
    /// co-commits one durable erasure event per message in the same transaction.
    /// A retry returns the operation's original receipt and emits no duplicate
    /// events. `prepare_author_erasure` must have committed first.
    pub async fn tombstone_author_co_commit(
        &self,
        tenant: &str,
        author: &str,
        event_ids: &dyn IdMinter,
        attempt: MessageErasureAttempt,
    ) -> Result<AuthoredMessageEraseReceipt, StoreError> {
        let MessageErasureAttempt {
            operation_id,
            actor,
            occurred,
            recorded,
        } = attempt;
        let operation_id = operation_id.as_str();
        validate_erasure_identity(tenant, author, operation_id)?;
        if actor.0.tenant.as_str() != tenant {
            return Err(StoreError::Cold(
                "Chat erasure actor is outside the message tenant".into(),
            ));
        }

        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| StoreError::Cold(format!("acquire: {error}")))?;
        self.set_session_scope(&mut connection, tenant, &self.region)
            .await?;
        let mut transaction = connection
            .begin()
            .await
            .map_err(|error| StoreError::Cold(format!("begin Chat erasure: {error}")))?;
        lock_author(&mut transaction, tenant, author, AuthorLock::Exclusive).await?;

        let marker = sqlx::query(&format!(
            "SELECT author, completed_at IS NOT NULL AS completed, \
                    messages_erased, events_emitted \
               FROM {} \
              WHERE tenant_id = $1 AND region = $2 AND operation_id = $3 \
              FOR UPDATE",
            self.erasure_operation_table(),
        ))
        .bind(tenant)
        .bind(&self.region)
        .bind(operation_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| StoreError::Cold(format!("lock Chat erasure marker: {error}")))?
        .ok_or_else(|| {
            StoreError::Cold("Chat erasure was not durably prepared before message mutation".into())
        })?;
        require_same_author(&marker, author)?;
        if let AuthoredMessageErasureState::Completed(receipt) = erasure_state_from_row(&marker)? {
            transaction
                .commit()
                .await
                .map_err(|error| StoreError::Cold(format!("commit Chat erasure retry: {error}")))?;
            return Ok(receipt);
        }

        let rows = sqlx::query(&format!(
            "UPDATE {} \
                SET state = 3, body_inline = '\\x', body_nodes = '\\x' \
              WHERE tenant_id = $1 AND region = $2 AND author = $3 AND state <> 3 \
              RETURNING conversation_id, message_id, thread_root_id, author",
            self.table,
        ))
        .bind(tenant)
        .bind(&self.region)
        .bind(author)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| StoreError::Cold(format!("tombstone authored Chat messages: {error}")))?;

        let mut tombstones = rows
            .into_iter()
            .map(|row| {
                Ok(TombstonedMessage {
                    conversation: ConversationId::new(
                        tenant,
                        &self.region,
                        row.try_get::<String, _>("conversation_id")
                            .map_err(erasure_row_decode)?,
                    ),
                    message_id: MessageId(row.try_get("message_id").map_err(erasure_row_decode)?),
                    author: row.try_get("author").map_err(erasure_row_decode)?,
                    thread_root_id: row
                        .try_get::<Option<String>, _>("thread_root_id")
                        .map_err(erasure_row_decode)?
                        .map(MessageId),
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        tombstones.sort_by(|left, right| left.message_id.cmp(&right.message_id));

        for message in &tombstones {
            let draft = message_event_draft(
                CHAT_MESSAGE_ERASED,
                &message.conversation,
                &message.message_id,
                &message.author,
                message.thread_root_id.as_ref(),
            )?;
            let envelope = derive_envelope(
                draft,
                EmitContext {
                    event_id: event_ids.mint().into(),
                    tenant: TenantId(tenant.to_string()),
                    region: Region(self.region.clone()),
                    actor: actor.clone(),
                    schema_ver: 1,
                    occurred_at: occurred.clone(),
                    recorded_at: recorded.clone(),
                    caused_by: None,
                },
                None,
            );
            myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
                &mut transaction,
                &envelope.aggregate.0,
                &envelope,
            )
            .await
            .map_err(|error| {
                StoreError::Cold(format!("co-commit Chat message erasure: {error}"))
            })?;
        }

        let count = i64::try_from(tombstones.len())
            .map_err(|_| StoreError::Cold("Chat erasure count exceeds PostgreSQL bigint".into()))?;
        let completed = sqlx::query(&format!(
            "UPDATE {} \
                SET completed_at = CURRENT_TIMESTAMP, messages_erased = $4, events_emitted = $4 \
              WHERE tenant_id = $1 AND region = $2 AND operation_id = $3 \
                AND completed_at IS NULL",
            self.erasure_operation_table(),
        ))
        .bind(tenant)
        .bind(&self.region)
        .bind(operation_id)
        .bind(count)
        .execute(&mut *transaction)
        .await
        .map_err(|error| StoreError::Cold(format!("complete Chat erasure marker: {error}")))?;
        if completed.rows_affected() != 1 {
            return Err(StoreError::Cold(
                "Chat erasure marker did not complete exactly once".into(),
            ));
        }

        transaction
            .commit()
            .await
            .map_err(|error| StoreError::Cold(format!("commit Chat erasure: {error}")))?;
        let count = u64::try_from(count)
            .map_err(|_| StoreError::Cold("Chat erasure count is negative".into()))?;
        Ok(AuthoredMessageEraseReceipt {
            messages_tombstoned: count,
            erasure_events_co_committed: count,
        })
    }

    pub(super) async fn refuse_append_during_author_erasure(
        &self,
        transaction: &mut sqlx::PgConnection,
        tenant: &str,
        author: &str,
    ) -> Result<(), StoreError> {
        lock_author(transaction, tenant, author, AuthorLock::Shared).await?;
        let erasing: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS( \
                SELECT 1 FROM {} \
                 WHERE tenant_id = $1 AND region = $2 AND author = $3 \
                   AND completed_at IS NULL \
            )",
            self.erasure_operation_table(),
        ))
        .bind(tenant)
        .bind(&self.region)
        .bind(author)
        .fetch_one(transaction)
        .await
        .map_err(|error| StoreError::Cold(format!("check Chat author erasure: {error}")))?;
        if erasing {
            return Err(StoreError::Cold(
                "Chat message refused while this author's erasure is in progress".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum AuthorLock {
    Shared,
    Exclusive,
}

async fn lock_author(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    author: &str,
    mode: AuthorLock,
) -> Result<(), StoreError> {
    let function = match mode {
        AuthorLock::Shared => "pg_advisory_xact_lock_shared",
        AuthorLock::Exclusive => "pg_advisory_xact_lock",
    };
    sqlx::query(&format!(
        "SELECT {function}(hashtextextended( \
            'myelin.chat.author-erasure.v1:' || length($1)::text || ':' || $1 || ':' || $2, 0 \
        ))",
    ))
    .bind(tenant)
    .bind(author)
    .execute(connection)
    .await
    .map_err(|error| StoreError::Cold(format!("lock Chat author lifecycle: {error}")))?;
    Ok(())
}

fn validate_erasure_identity(
    tenant: &str,
    author: &str,
    operation_id: &str,
) -> Result<(), StoreError> {
    if tenant.is_empty() || author.is_empty() {
        return Err(StoreError::Cold(
            "Chat author erasure requires a tenant and author".into(),
        ));
    }
    if operation_id.is_empty()
        || operation_id.len() > 255
        || operation_id.trim() != operation_id
        || operation_id.chars().any(char::is_control)
    {
        return Err(StoreError::Cold(
            "Chat author erasure requires a clean 1-255 byte operation identity".into(),
        ));
    }
    Ok(())
}

fn require_same_author(row: &sqlx::postgres::PgRow, author: &str) -> Result<(), StoreError> {
    let stored: String = row.try_get("author").map_err(erasure_row_decode)?;
    if stored != author {
        return Err(StoreError::Cold(
            "Chat erasure operation is already bound to another author".into(),
        ));
    }
    Ok(())
}

fn erasure_state_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<AuthoredMessageErasureState, StoreError> {
    if !row
        .try_get::<bool, _>("completed")
        .map_err(erasure_row_decode)?
    {
        return Ok(AuthoredMessageErasureState::Pending);
    }
    let messages = row
        .try_get::<Option<i64>, _>("messages_erased")
        .map_err(erasure_row_decode)?
        .ok_or_else(|| StoreError::Cold("completed Chat erasure has no message count".into()))?;
    let events = row
        .try_get::<Option<i64>, _>("events_emitted")
        .map_err(erasure_row_decode)?
        .ok_or_else(|| StoreError::Cold("completed Chat erasure has no event count".into()))?;
    let messages_tombstoned = u64::try_from(messages)
        .map_err(|_| StoreError::Cold("Chat erasure message count is negative".into()))?;
    let erasure_events_co_committed = u64::try_from(events)
        .map_err(|_| StoreError::Cold("Chat erasure event count is negative".into()))?;
    Ok(AuthoredMessageErasureState::Completed(
        AuthoredMessageEraseReceipt {
            messages_tombstoned,
            erasure_events_co_committed,
        },
    ))
}

fn erasure_row_decode(error: sqlx::Error) -> StoreError {
    StoreError::Cold(format!("decode Chat erasure row: {error}"))
}
