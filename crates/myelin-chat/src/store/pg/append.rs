use sqlx::Acquire;

use myelin_content::InlineNode;
use myelin_events::{
    derive_envelope, Actor, DataRole, EmitContext, EventDraft, EventEnvelope, EventId, EventType,
    IdMinter, Timestamp, Visibility,
};
use myelin_identity::{ObjectId, PrincipalId, RelName, RelationTuple, TupleDelta};
use myelin_identity_service::tuple_written_event;
use myelin_storage::{
    DurableTupleBacking, DurableTupleDelta, DurableTupleWriteOutcome, TenantScope, TupleEdgeOp,
};
use myelin_tenancy::Region;

use super::{author_kind_code, thread_notification, MessageAttribution, PgMessageStore};
use crate::store::{MessageId, NewMessage, StoreError, UlidSource};

impl PgMessageStore {
    #[allow(clippy::too_many_arguments)]
    pub async fn append_co_commit(
        &self,
        minter: &dyn UlidSource,
        msg: NewMessage,
        event_id: EventId,
        attribution: MessageAttribution,
        occurred: Timestamp,
        recorded: Timestamp,
    ) -> Result<MessageId, StoreError> {
        self.append_co_commit_inner(
            minter,
            msg,
            event_id,
            None,
            &[],
            attribution,
            occurred,
            recorded,
        )
        .await
    }

    /// Appends a message and its structured reference edges atomically. The
    /// caller supplies the plaintext nodes only for event derivation; the
    /// persisted `body_nodes` bytes remain the caller's encrypted envelope.
    #[allow(clippy::too_many_arguments)]
    pub async fn append_structured_co_commit(
        &self,
        minter: &dyn UlidSource,
        msg: NewMessage,
        event_id: EventId,
        related_event_ids: &dyn IdMinter,
        structured_nodes: &[InlineNode],
        attribution: MessageAttribution,
        occurred: Timestamp,
        recorded: Timestamp,
    ) -> Result<MessageId, StoreError> {
        self.append_co_commit_inner(
            minter,
            msg,
            event_id,
            Some(related_event_ids),
            structured_nodes,
            attribution,
            occurred,
            recorded,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn append_co_commit_inner(
        &self,
        minter: &dyn UlidSource,
        msg: NewMessage,
        event_id: EventId,
        related_event_ids: Option<&dyn IdMinter>,
        structured_nodes: &[InlineNode],
        attribution: MessageAttribution,
        occurred: Timestamp,
        recorded: Timestamp,
    ) -> Result<MessageId, StoreError> {
        let MessageAttribution {
            event_actor: actor,
            notification_recipient,
        } = attribution;
        if actor.0.tenant.0 != msg.conv.tenant {
            return Err(StoreError::Cold(
                "message actor is outside the conversation tenant".into(),
            ));
        }
        if notification_recipient.0.is_empty() {
            return Err(StoreError::Cold(
                "message notification recipient is empty".into(),
            ));
        }
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &msg.conv.tenant, &msg.conv.region)
            .await?;

        if let Some(existing) = sqlx::query_scalar::<_, String>(&format!(
            "SELECT message_id FROM {} WHERE tenant_id = $1 AND region = $2 \
             AND conversation_id = $3 AND client_nonce = $4",
            self.table
        ))
        .bind(&msg.conv.tenant)
        .bind(&msg.conv.region)
        .bind(&msg.conv.conversation_id)
        .bind(&msg.client_nonce)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("nonce check: {e}")))?
        {
            return Ok(MessageId(existing));
        }

        let message_id = minter.mint();
        let visibility_event_id = message_visibility_event_id(&event_id);
        let envelope = self.message_created_envelope(
            &msg,
            &message_id,
            event_id,
            actor.clone(),
            occurred.clone(),
            recorded.clone(),
        )?;
        let edge_envelopes = if structured_nodes.is_empty() {
            Vec::new()
        } else {
            let related_event_ids = related_event_ids.ok_or_else(|| {
                StoreError::Cold(
                    "structured Chat append requires an event-id source for reference edges".into(),
                )
            })?;
            crate::content::extract_body_edges(&envelope.subject, structured_nodes)
                .into_iter()
                .map(|edge| {
                    derive_envelope(
                        crate::content::edge_event_draft(&edge),
                        EmitContext {
                            event_id: related_event_ids.mint().into(),
                            tenant: envelope.tenant.clone(),
                            region: envelope.region.clone(),
                            actor: actor.clone(),
                            schema_ver: envelope.schema_ver,
                            occurred_at: occurred.clone(),
                            recorded_at: recorded.clone(),
                            caused_by: envelope.caused_by.clone(),
                        },
                        Some(&envelope),
                    )
                })
                .collect()
        };
        let mention_envelope = if structured_nodes
            .iter()
            .any(|node| matches!(node, InlineNode::Mention(_)))
        {
            let related_event_ids = related_event_ids.ok_or_else(|| {
                StoreError::Cold(
                    "structured Chat append requires an event-id source for mention delivery"
                        .into(),
                )
            })?;
            crate::mention_signal::message_mention_signal(
                &envelope,
                related_event_ids.mint().into(),
                message_id.as_str(),
                structured_nodes,
            )
            .map_err(|error| {
                StoreError::Cold(format!("derive Chat mention signal payload: {error}"))
            })?
        } else {
            None
        };

        let mut dbtx = conn
            .begin()
            .await
            .map_err(|e| StoreError::Cold(format!("begin co-commit tx: {e}")))?;

        let thread_root = match msg.thread_root_id.as_ref() {
            Some(root) => Some(
                self.require_thread_root_in_tx(&mut dbtx, &msg.conv, root)
                    .await?,
            ),
            None => None,
        };

        let inserted = sqlx::query_scalar::<_, String>(&format!(
            "INSERT INTO {} (tenant_id, region, conversation_id, message_id, thread_root_id, \
             author, author_kind, body_inline, body_nodes, client_nonce, edited_seq, state) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 0, 0) \
             ON CONFLICT (tenant_id, region, conversation_id, client_nonce) DO NOTHING \
             RETURNING message_id",
            self.table
        ))
        .bind(&msg.conv.tenant)
        .bind(&msg.conv.region)
        .bind(&msg.conv.conversation_id)
        .bind(message_id.as_str())
        .bind(msg.thread_root_id.as_ref().map(|t| t.0.clone()))
        .bind(&msg.author)
        .bind(author_kind_code(msg.author_kind) as i16)
        .bind(&msg.body_inline)
        .bind(&msg.body_nodes)
        .bind(&msg.client_nonce)
        .fetch_optional(&mut *dbtx)
        .await
        .map_err(|e| StoreError::Cold(format!("co-commit message insert: {e}")))?;

        if inserted.is_none() {
            let existing = sqlx::query_scalar::<_, String>(&format!(
                "SELECT message_id FROM {} WHERE tenant_id = $1 AND region = $2 \
                 AND conversation_id = $3 AND client_nonce = $4",
                self.table
            ))
            .bind(&msg.conv.tenant)
            .bind(&msg.conv.region)
            .bind(&msg.conv.conversation_id)
            .bind(&msg.client_nonce)
            .fetch_optional(&mut *dbtx)
            .await
            .map_err(|e| StoreError::Cold(format!("co-commit nonce resolution: {e}")))?
            .ok_or_else(|| {
                StoreError::Cold(
                    "message nonce conflicted without an authoritative existing row".into(),
                )
            })?;
            dbtx.rollback()
                .await
                .map_err(|e| StoreError::Cold(format!("rollback duplicate append: {e}")))?;
            return Ok(MessageId(existing));
        }

        let (participant_root, participant_role) = match msg.thread_root_id.as_ref() {
            Some(root) => (root, super::thread::FOLLOWER_ROLE),
            None => (&message_id, super::thread::ROOT_AUTHOR_ROLE),
        };
        self.record_thread_participant_in_tx(
            &mut dbtx,
            &msg,
            participant_root,
            &notification_recipient,
            participant_role,
        )
        .await?;

        let reply_events = match (msg.thread_root_id.as_ref(), thread_root.as_ref()) {
            (Some(root), Some(thread_root)) => Some(thread_notification::thread_reply_events(
                &envelope,
                root,
                &message_id,
                thread_root.notification_recipient.as_ref(),
                &thread_root.followers,
                &notification_recipient,
            )?),
            _ => None,
        };

        let visibility_tuple = message_visibility_tuple(&msg, &message_id);
        let identity_delta = TupleDelta::Add(visibility_tuple.clone());
        let durable_delta = DurableTupleDelta {
            op: TupleEdgeOp::Add,
            object: visibility_tuple.object.0,
            relation: visibility_tuple.relation.0,
            subject: visibility_tuple.subject.0,
            expires_at: None,
        };
        let scope = TenantScope::from_verified_token(&actor.0, Region(msg.conv.region.clone()));
        let (tenant, region) = (&msg.conv.tenant, &msg.conv.region);
        let visibility_outcome = DurableTupleBacking::apply_deltas_in_tx(
            &mut dbtx,
            tenant,
            region,
            &[durable_delta],
            None,
            |revision| {
                let (aggregate, envelope) = tuple_written_event(
                    visibility_event_id,
                    &scope,
                    &actor.0,
                    &[identity_delta],
                    revision,
                    None,
                    &occurred,
                    &recorded,
                );
                (aggregate.0, envelope)
            },
        )
        .await
        .map_err(|error| StoreError::Cold(format!("co-commit message visibility: {error}")))?;
        let DurableTupleWriteOutcome::Committed { .. } = visibility_outcome else {
            return Err(StoreError::Cold(
                "message visibility write unexpectedly rejected without a precondition".into(),
            ));
        };

        myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
            &mut dbtx,
            &envelope.aggregate.0,
            &envelope,
        )
        .await
        .map_err(|e| StoreError::Cold(format!("co-commit outbox insert: {e}")))?;
        if let Some(reply_events) = reply_events {
            myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
                &mut dbtx,
                &reply_events.replied.aggregate.0,
                &reply_events.replied,
            )
            .await
            .map_err(|error| StoreError::Cold(format!("co-commit Chat thread reply: {error}")))?;
            for notification in reply_events.notifications {
                myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
                    &mut dbtx,
                    &notification.aggregate.0,
                    &notification,
                )
                .await
                .map_err(|error| {
                    StoreError::Cold(format!("co-commit Chat thread notification: {error}"))
                })?;
            }
        }
        for edge in edge_envelopes {
            myelin_storage::pgrelay::PgRelay::co_commit_in_tx(&mut dbtx, &edge.aggregate.0, &edge)
                .await
                .map_err(|error| {
                    StoreError::Cold(format!("co-commit Chat reference edge: {error}"))
                })?;
        }
        if let Some(mention) = mention_envelope {
            myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
                &mut dbtx,
                &mention.aggregate.0,
                &mention,
            )
            .await
            .map_err(|error| StoreError::Cold(format!("co-commit Chat mention signal: {error}")))?;
        }

        dbtx.commit()
            .await
            .map_err(|e| StoreError::Cold(format!("co-commit: {e}")))?;
        Ok(message_id)
    }

    async fn record_thread_participant_in_tx(
        &self,
        transaction: &mut sqlx::PgConnection,
        message: &NewMessage,
        root: &MessageId,
        principal: &PrincipalId,
        role: i16,
    ) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO {} \
               (tenant_id, region, conversation_id, thread_root_id, principal_id, role) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT DO NOTHING",
            self.thread_participant_table(),
        ))
        .bind(&message.conv.tenant)
        .bind(&message.conv.region)
        .bind(&message.conv.conversation_id)
        .bind(root.as_str())
        .bind(&principal.0)
        .bind(role)
        .execute(transaction)
        .await
        .map_err(|error| StoreError::Cold(format!("co-commit thread participant: {error}")))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn message_created_envelope(
        &self,
        msg: &NewMessage,
        message_id: &MessageId,
        event_id: EventId,
        actor: Actor,
        occurred: Timestamp,
        recorded: Timestamp,
    ) -> Result<EventEnvelope, StoreError> {
        let subject = crate::subs::mint_message(&msg.conv.tenant, message_id.as_str())
            .map_err(|e| StoreError::Cold(format!("mint message #sub anchor: {e}")))?;
        let draft = EventDraft {
            type_: EventType(crate::events::CHAT_MESSAGE_CREATED.to_string()),
            subject,
            aggregate: crate::events::channel_aggregate(&msg.conv.conversation_id),
            payload: serde_json::json!({
                "conversation_id": msg.conv.conversation_id,
                "message_id": message_id.as_str(),
                "author": msg.author,
                "thread_root_id": msg.thread_root_id.as_ref().map(|t| t.as_str().to_string()),
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        let ctx = EmitContext {
            event_id,
            tenant: myelin_tenancy::TenantId(msg.conv.tenant.clone()),
            region: myelin_tenancy::Region(msg.conv.region.clone()),
            actor,
            schema_ver: 1,
            occurred_at: occurred,
            recorded_at: recorded,
            caused_by: None,
        };
        Ok(derive_envelope(draft, ctx, None))
    }
}

fn message_visibility_tuple(msg: &NewMessage, message_id: &MessageId) -> RelationTuple {
    RelationTuple {
        object: ObjectId(format!("message:{}", message_id.as_str())),
        relation: RelName("parent_channel".into()),
        subject: PrincipalId(format!("channel:{}#read", msg.conv.conversation_id)),
        caveat: None,
    }
}

fn message_visibility_event_id(message_event_id: &EventId) -> EventId {
    let mut digest = blake3::Hasher::new();
    digest.update(b"myelin.chat.message-visibility-event.v1\0");
    digest.update(message_event_id.0.as_bytes());
    EventId(format!(
        "chat-visibility-{}",
        &digest.finalize().to_hex()[..32]
    ))
}
