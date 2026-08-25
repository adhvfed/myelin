use sqlx::{Acquire, Row};

use myelin_events::{derive_envelope, Actor, EmitContext, IdMinter, Timestamp};
use myelin_tenancy::{Region, TenantId};

use super::{AuthoredMessageEraseReceipt, PgMessageStore};
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
    /// Tombstones every live message written by one pseudonymous author and
    /// co-commits one durable erasure event per message in the same transaction.
    /// A retry observes no live rows and therefore emits no duplicate events.
    pub async fn tombstone_author_co_commit(
        &self,
        tenant: &str,
        author: &str,
        event_ids: &dyn IdMinter,
        actor: Actor,
        occurred: Timestamp,
        recorded: Timestamp,
    ) -> Result<AuthoredMessageEraseReceipt, StoreError> {
        if tenant.is_empty() || author.is_empty() {
            return Err(StoreError::Cold(
                "Chat author erasure requires a tenant and author".into(),
            ));
        }
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
            .map(|row| TombstonedMessage {
                conversation: ConversationId::new(
                    tenant,
                    &self.region,
                    row.get::<String, _>("conversation_id"),
                ),
                message_id: MessageId(row.get("message_id")),
                author: row.get("author"),
                thread_root_id: row
                    .get::<Option<String>, _>("thread_root_id")
                    .map(MessageId),
            })
            .collect::<Vec<_>>();
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

        transaction
            .commit()
            .await
            .map_err(|error| StoreError::Cold(format!("commit Chat erasure: {error}")))?;
        let count = tombstones.len() as u64;
        Ok(AuthoredMessageEraseReceipt {
            messages_tombstoned: count,
            erasure_events_co_committed: count,
        })
    }
}
