use sqlx::{Acquire, Row};

use myelin_events::{derive_envelope, EmitContext, IdMinter};
use myelin_tenancy::{Region, TenantId};

use super::{
    AuthoredMessageEraseReceipt, AuthoredMessageErasureState, MessageErasureAttempt,
    PgMessageStore, VerifiedMessageErasureAttempt, AUTHOR_ERASURE_BATCH_MAX,
};
use crate::dek::{chat_subject_key_class, decode_encrypted_body};
use crate::events::CHAT_MESSAGE_ERASED;
use crate::store::{message_event_draft, ConversationId, MessageId, StoreError};

#[derive(Debug)]
struct TombstonedMessage {
    conversation: ConversationId,
    message_id: MessageId,
    author: String,
    thread_root_id: Option<MessageId>,
}

#[derive(Clone, Copy)]
struct AuthorErasure<'a> {
    tenant: &'a str,
    region: &'a str,
    author: &'a str,
    operation_id: &'a str,
}

struct ErasureEvents<'a> {
    ids: &'a dyn IdMinter,
    actor: &'a myelin_events::Actor,
    occurred: &'a myelin_events::Timestamp,
    recorded: &'a myelin_events::Timestamp,
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

    /// Proves that every live body is on the independent Chat key before a
    /// caller destroys that key. Each bounded transaction advances a durable
    /// cursor. The pending marker fences new writes, so an earlier verified
    /// batch cannot change while a later batch is inspected.
    pub(crate) async fn verify_author_erasure_ready(
        &self,
        tenant: &str,
        author: &str,
        operation_id: &str,
    ) -> Result<(), StoreError> {
        validate_erasure_identity(tenant, author, operation_id)?;
        loop {
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
                .map_err(|error| StoreError::Cold(format!("begin Chat erasure check: {error}")))?;
            lock_author(&mut transaction, tenant, author, AuthorLock::Exclusive).await?;
            let marker = load_erasure_marker(
                &mut transaction,
                &self.erasure_operation_table(),
                tenant,
                &self.region,
                operation_id,
            )
            .await?;
            require_same_author(&marker, author)?;
            let ready = match erasure_state_from_row(&marker)? {
                AuthoredMessageErasureState::Completed(_) => true,
                AuthoredMessageErasureState::Pending => {
                    let erasure = AuthorErasure {
                        tenant,
                        region: &self.region,
                        author,
                        operation_id,
                    };
                    validate_next_authored_body_batch(
                        &mut transaction,
                        &self.table,
                        &self.erasure_operation_table(),
                        erasure,
                        &marker,
                    )
                    .await?
                }
            };
            transaction
                .commit()
                .await
                .map_err(|error| StoreError::Cold(format!("commit Chat erasure check: {error}")))?;
            if ready {
                return Ok(());
            }
        }
    }

    /// Tombstones every live message written by one pseudonymous author in
    /// bounded transactions. Every batch co-commits one durable erasure event
    /// per message and advances the operation's durable counts. A retry resumes
    /// at the first live message and returns the original cumulative receipt.
    /// `prepare_author_erasure` and bounded envelope verification must have
    /// committed before the caller can construct `verified`.
    pub(crate) async fn tombstone_author_co_commit(
        &self,
        tenant: &str,
        event_ids: &dyn IdMinter,
        verified: VerifiedMessageErasureAttempt,
    ) -> Result<AuthoredMessageEraseReceipt, StoreError> {
        let VerifiedMessageErasureAttempt { author, attempt } = verified;
        let author = author.as_str();
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

        let erasure = AuthorErasure {
            tenant,
            region: &self.region,
            author,
            operation_id,
        };
        let events = ErasureEvents {
            ids: event_ids,
            actor: &actor,
            occurred: &occurred,
            recorded: &recorded,
        };
        loop {
            if let Some(receipt) = self.tombstone_next_author_batch(erasure, &events).await? {
                return Ok(receipt);
            }
        }
    }

    async fn tombstone_next_author_batch(
        &self,
        erasure: AuthorErasure<'_>,
        events: &ErasureEvents<'_>,
    ) -> Result<Option<AuthoredMessageEraseReceipt>, StoreError> {
        let AuthorErasure {
            tenant,
            region,
            author,
            operation_id,
        } = erasure;
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| StoreError::Cold(format!("acquire: {error}")))?;
        self.set_session_scope(&mut connection, tenant, region)
            .await?;
        let mut transaction = connection
            .begin()
            .await
            .map_err(|error| StoreError::Cold(format!("begin Chat erasure batch: {error}")))?;
        lock_author(&mut transaction, tenant, author, AuthorLock::Exclusive).await?;

        let operation_table = self.erasure_operation_table();
        let marker = load_erasure_marker(
            &mut transaction,
            &operation_table,
            tenant,
            region,
            operation_id,
        )
        .await?;
        require_same_author(&marker, author)?;
        if let AuthoredMessageErasureState::Completed(receipt) = erasure_state_from_row(&marker)? {
            transaction
                .commit()
                .await
                .map_err(|error| StoreError::Cold(format!("commit Chat erasure retry: {error}")))?;
            return Ok(Some(receipt));
        }
        if !marker
            .try_get::<bool, _>("validation_completed")
            .map_err(erasure_row_decode)?
        {
            return Err(StoreError::Cold(
                "Chat erasure cannot mutate messages before envelope verification".into(),
            ));
        }

        let rows = sqlx::query(&format!(
            "WITH batch AS ( \
                 SELECT tenant_id, region, conversation_id, message_id \
                   FROM {} \
                  WHERE tenant_id = $1 AND region = $2 AND author = $3 AND state <> 3 \
                  ORDER BY message_id \
                  LIMIT $4 FOR UPDATE \
             ) \
             UPDATE {} AS message \
                SET state = 3, body_inline = '\\x', body_nodes = '\\x' \
               FROM batch \
              WHERE message.tenant_id = batch.tenant_id \
                AND message.region = batch.region \
                AND message.conversation_id = batch.conversation_id \
                AND message.message_id = batch.message_id \
              RETURNING message.conversation_id, message.message_id, \
                        message.thread_root_id, message.author",
            self.table, self.table,
        ))
        .bind(tenant)
        .bind(region)
        .bind(author)
        .bind(i64::try_from(AUTHOR_ERASURE_BATCH_MAX).expect("batch limit fits in i64"))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| {
            StoreError::Cold(format!("tombstone authored Chat message batch: {error}"))
        })?;

        let mut tombstones = rows
            .into_iter()
            .map(|row| {
                Ok(TombstonedMessage {
                    conversation: ConversationId::new(
                        tenant,
                        region,
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
                    event_id: events.ids.mint().into(),
                    tenant: TenantId(tenant.to_string()),
                    region: Region(region.to_string()),
                    actor: events.actor.clone(),
                    schema_ver: 1,
                    occurred_at: events.occurred.clone(),
                    recorded_at: events.recorded.clone(),
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

        let batch_count = i64::try_from(tombstones.len())
            .map_err(|_| StoreError::Cold("Chat erasure batch count overflowed".into()))?;
        let progress = sqlx::query(&format!(
            "UPDATE {operation_table} \
                SET messages_erased_so_far = messages_erased_so_far + $4, \
                    events_emitted_so_far = events_emitted_so_far + $4 \
              WHERE tenant_id = $1 AND region = $2 AND operation_id = $3 \
                AND completed_at IS NULL \
              RETURNING messages_erased_so_far",
        ))
        .bind(tenant)
        .bind(region)
        .bind(operation_id)
        .bind(batch_count)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| StoreError::Cold(format!("advance Chat erasure progress: {error}")))?;
        let messages_erased: i64 = progress
            .try_get("messages_erased_so_far")
            .map_err(erasure_row_decode)?;
        let more_messages: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS( \
                 SELECT 1 FROM {} \
                  WHERE tenant_id = $1 AND region = $2 AND author = $3 AND state <> 3 \
             )",
            self.table,
        ))
        .bind(tenant)
        .bind(region)
        .bind(author)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| StoreError::Cold(format!("check Chat erasure remainder: {error}")))?;

        let receipt = if more_messages {
            None
        } else {
            let completed = sqlx::query(&format!(
                "UPDATE {operation_table} \
                    SET completed_at = CURRENT_TIMESTAMP, \
                        messages_erased = messages_erased_so_far, \
                        events_emitted = events_emitted_so_far \
                  WHERE tenant_id = $1 AND region = $2 AND operation_id = $3 \
                    AND completed_at IS NULL",
            ))
            .bind(tenant)
            .bind(region)
            .bind(operation_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| StoreError::Cold(format!("complete Chat erasure marker: {error}")))?;
            if completed.rows_affected() != 1 {
                return Err(StoreError::Cold(
                    "Chat erasure marker did not complete exactly once".into(),
                ));
            }
            let messages_tombstoned = u64::try_from(messages_erased)
                .map_err(|_| StoreError::Cold("Chat erasure count is negative".into()))?;
            Some(AuthoredMessageEraseReceipt {
                messages_tombstoned,
                erasure_events_co_committed: messages_tombstoned,
            })
        };

        transaction
            .commit()
            .await
            .map_err(|error| StoreError::Cold(format!("commit Chat erasure batch: {error}")))?;
        Ok(receipt)
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

async fn load_erasure_marker(
    connection: &mut sqlx::PgConnection,
    table: &str,
    tenant: &str,
    region: &str,
    operation_id: &str,
) -> Result<sqlx::postgres::PgRow, StoreError> {
    sqlx::query(&format!(
        "SELECT author, completed_at IS NOT NULL AS completed, \
                validation_cursor, validation_completed_at IS NOT NULL AS validation_completed, \
                messages_erased, events_emitted \
           FROM {table} \
          WHERE tenant_id = $1 AND region = $2 AND operation_id = $3 \
          FOR UPDATE",
    ))
    .bind(tenant)
    .bind(region)
    .bind(operation_id)
    .fetch_optional(connection)
    .await
    .map_err(|error| StoreError::Cold(format!("lock Chat erasure marker: {error}")))?
    .ok_or_else(|| {
        StoreError::Cold("Chat erasure was not durably prepared before message mutation".into())
    })
}

async fn validate_next_authored_body_batch(
    connection: &mut sqlx::PgConnection,
    table: &str,
    operation_table: &str,
    erasure: AuthorErasure<'_>,
    marker: &sqlx::postgres::PgRow,
) -> Result<bool, StoreError> {
    let AuthorErasure {
        tenant,
        region,
        author,
        operation_id,
    } = erasure;
    if marker
        .try_get::<bool, _>("validation_completed")
        .map_err(erasure_row_decode)?
    {
        return Ok(true);
    }
    let cursor = marker
        .try_get::<Option<String>, _>("validation_cursor")
        .map_err(erasure_row_decode)?;
    let bodies = sqlx::query(&format!(
        "SELECT message_id, body_inline, body_nodes \
           FROM {table} \
          WHERE tenant_id = $1 AND region = $2 AND author = $3 AND state <> 3 \
            AND ($4::text IS NULL OR message_id > $4) \
          ORDER BY message_id LIMIT $5 FOR UPDATE",
    ))
    .bind(tenant)
    .bind(region)
    .bind(author)
    .bind(cursor.as_deref())
    .bind(i64::try_from(AUTHOR_ERASURE_BATCH_MAX).expect("batch limit fits in i64"))
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| {
        StoreError::Cold(format!("inspect authored Chat encryption batch: {error}"))
    })?;
    for row in &bodies {
        let message_id: String = row.try_get("message_id").map_err(erasure_row_decode)?;
        for (column, encoded) in [
            (
                "body_inline",
                row.try_get::<Vec<u8>, _>("body_inline")
                    .map_err(erasure_row_decode)?,
            ),
            (
                "body_nodes",
                row.try_get::<Vec<u8>, _>("body_nodes")
                    .map_err(erasure_row_decode)?,
            ),
        ] {
            let envelope = decode_encrypted_body(&encoded).map_err(|_| {
                StoreError::Cold(format!(
                    "Chat erasure cannot prove the encryption boundary of {message_id} {column}"
                ))
            })?;
            if envelope.key_ref.tenant.as_str() != tenant
                || envelope.key_ref.class != chat_subject_key_class(author)
            {
                return Err(StoreError::Cold(format!(
                    "Chat erasure refuses legacy or foreign key scope on {message_id} {column}"
                )));
            }
        }
    }
    let next_cursor = bodies
        .last()
        .map(|row| row.try_get::<String, _>("message_id"))
        .transpose()
        .map_err(erasure_row_decode)?
        .or(cursor);
    let validation_completed = bodies.len() < AUTHOR_ERASURE_BATCH_MAX;
    let advanced = sqlx::query(&format!(
        "UPDATE {operation_table} \
            SET validation_cursor = $4, \
                validation_completed_at = CASE WHEN $5 THEN CURRENT_TIMESTAMP ELSE NULL END \
          WHERE tenant_id = $1 AND region = $2 AND operation_id = $3 \
            AND completed_at IS NULL",
    ))
    .bind(tenant)
    .bind(region)
    .bind(operation_id)
    .bind(next_cursor)
    .bind(validation_completed)
    .execute(&mut *connection)
    .await
    .map_err(|error| StoreError::Cold(format!("advance Chat erasure validation: {error}")))?;
    if advanced.rows_affected() != 1 {
        return Err(StoreError::Cold(
            "Chat erasure validation marker did not advance exactly once".into(),
        ));
    }
    Ok(validation_completed)
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
