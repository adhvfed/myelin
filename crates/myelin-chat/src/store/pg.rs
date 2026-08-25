use std::collections::BTreeSet;

use sqlx::postgres::PgPool;
use sqlx::{Acquire, Row};

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

#[cfg(any(test, feature = "test-support"))]
use super::TombstoneReason;
use super::{
    is_canonical_ulid, AuthorKind, ConversationId, Message, MessageId, MessageLocation, NewMessage,
    RangeCursor, StoreError,
};

mod thread;

const EXACT_MESSAGE_BATCH_MAX: usize = 100;

pub const MESSAGE_TABLE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS {table} (
    tenant_id       text        NOT NULL,
    region          text        NOT NULL,
    conversation_id text        NOT NULL,
    message_id      text        NOT NULL,
    thread_root_id  text,
    author          text        NOT NULL,
    author_kind     smallint    NOT NULL,
    body_inline     bytea       NOT NULL,
    body_nodes      bytea       NOT NULL DEFAULT '\\x',
    client_nonce    text        NOT NULL,
    edited_seq      int         NOT NULL DEFAULT 0,
    state           smallint    NOT NULL DEFAULT 0,
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region, conversation_id, message_id),
    CONSTRAINT {table}_nonce_unique UNIQUE (tenant_id, region, conversation_id, client_nonce)
);
CREATE INDEX IF NOT EXISTS {table}_range
    ON {table} (tenant_id, region, conversation_id, message_id DESC);";

#[derive(Clone)]
pub struct PgMessageStore {
    pool: PgPool,
    region: String,
    table: String,
}

impl PgMessageStore {
    pub fn new(
        pool: PgPool,
        region: impl Into<String>,
        table: impl Into<String>,
    ) -> PgMessageStore {
        PgMessageStore {
            pool,
            region: region.into(),
            table: table.into(),
        }
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub async fn migrate(&self) -> Result<(), StoreError> {
        let ddl = MESSAGE_TABLE_DDL.replace("{table}", &self.table);
        sqlx::raw_sql(&ddl)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("message DDL: {e}")))?;
        sqlx::raw_sql(&format!(
            "CREATE UNIQUE INDEX IF NOT EXISTS {table}_identity \
             ON {table} (tenant_id, region, message_id)",
            table = self.table,
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("message identity index: {e}")))?;
        sqlx::raw_sql(&format!(
            "CREATE INDEX IF NOT EXISTS {table}_thread_range \
             ON {table} (tenant_id, region, conversation_id, thread_root_id, message_id)",
            table = self.table,
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("message thread range index: {e}")))?;
        sqlx::raw_sql(&format!(
            "DO $myelin$ \
             BEGIN \
               IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint \
                 WHERE conrelid = '{table}'::regclass \
                   AND conname = '{table}_author_kind_known') THEN \
                 ALTER TABLE {table} ADD CONSTRAINT {table}_author_kind_known \
                   CHECK (author_kind BETWEEN 0 AND 2); \
               END IF; \
               IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint \
                 WHERE conrelid = '{table}'::regclass \
                   AND conname = '{table}_state_known') THEN \
                 ALTER TABLE {table} ADD CONSTRAINT {table}_state_known \
                   CHECK (state BETWEEN 0 AND 3); \
               END IF; \
             END \
             $myelin$;",
            table = self.table,
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("message enum constraints: {e}")))?;
        sqlx::raw_sql(&format!(
            "SELECT myelin_make_tenant_scoped('{}')",
            self.table
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("make tenant scoped: {e}")))?;
        sqlx::raw_sql(&format!("GRANT ALL ON {} TO myelin_app", self.table))
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("grant: {e}")))?;
        Ok(())
    }

    async fn set_session_scope(
        &self,
        conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
        tenant: &str,
        region: &str,
    ) -> Result<(), StoreError> {
        if region != self.region {
            return Err(StoreError::Cold(format!(
                "message store is pinned to region {}, not {region}",
                self.region
            )));
        }
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false), set_config('myelin.region', $2, false)")
            .bind(tenant)
            .bind(region)
            .execute(&mut **conn)
            .await
            .map_err(|e| StoreError::Cold(format!("set session scope: {e}")))?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Mutates only the message table for storage-parity tests; emits no event.
    pub async fn append_storage_only(
        &self,
        minter: &dyn super::UlidSource,
        msg: NewMessage,
    ) -> Result<MessageId, StoreError> {
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
        let stored_message_id = sqlx::query_scalar::<_, String>(&format!(
            "INSERT INTO {} (tenant_id, region, conversation_id, message_id, thread_root_id, \
             author, author_kind, body_inline, body_nodes, client_nonce, edited_seq, state) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 0, 0) \
             ON CONFLICT (tenant_id, region, conversation_id, client_nonce) DO UPDATE \
             SET client_nonce = EXCLUDED.client_nonce \
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
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("insert: {e}")))?;
        Ok(MessageId(stored_message_id))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn append_co_commit(
        &self,
        minter: &dyn super::UlidSource,
        msg: NewMessage,
        event_id: EventId,
        actor: Actor,
        occurred: Timestamp,
        recorded: Timestamp,
    ) -> Result<MessageId, StoreError> {
        self.append_co_commit_inner(minter, msg, event_id, None, &[], actor, occurred, recorded)
            .await
    }

    /// Appends a message and its structured reference edges atomically. The
    /// caller supplies the plaintext nodes only for event derivation; the
    /// persisted `body_nodes` bytes remain the caller's encrypted envelope.
    #[allow(clippy::too_many_arguments)]
    pub async fn append_structured_co_commit(
        &self,
        minter: &dyn super::UlidSource,
        msg: NewMessage,
        event_id: EventId,
        related_event_ids: &dyn IdMinter,
        structured_nodes: &[InlineNode],
        actor: Actor,
        occurred: Timestamp,
        recorded: Timestamp,
    ) -> Result<MessageId, StoreError> {
        self.append_co_commit_inner(
            minter,
            msg,
            event_id,
            Some(related_event_ids),
            structured_nodes,
            actor,
            occurred,
            recorded,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn append_co_commit_inner(
        &self,
        minter: &dyn super::UlidSource,
        msg: NewMessage,
        event_id: EventId,
        related_event_ids: Option<&dyn IdMinter>,
        structured_nodes: &[InlineNode],
        actor: Actor,
        occurred: Timestamp,
        recorded: Timestamp,
    ) -> Result<MessageId, StoreError> {
        if actor.0.tenant.0 != msg.conv.tenant {
            return Err(StoreError::Cold(
                "message actor is outside the conversation tenant".into(),
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

    pub async fn range(
        &self,
        conv: &ConversationId,
        cursor: RangeCursor,
        limit: u32,
    ) -> Result<Vec<Message>, StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &conv.tenant, &conv.region)
            .await?;

        let (where_clause, bound, order): (&str, Option<String>, &str) = match &cursor {
            RangeCursor::Recent => ("", None, "DESC"),
            RangeCursor::Before(id) => ("AND message_id < $5", Some(id.0.clone()), "DESC"),
            RangeCursor::After(id) => ("AND message_id > $5", Some(id.0.clone()), "ASC"),
        };

        let range_sql_tenant_id = format!(
            "SELECT tenant_id, region, conversation_id, message_id, thread_root_id, author, \
             author_kind, body_inline, body_nodes, client_nonce, edited_seq, state \
             FROM {} WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 {} \
             ORDER BY message_id {} LIMIT $4",
            self.table, where_clause, order,
        );
        let mut q = sqlx::query(&range_sql_tenant_id)
            .bind(&conv.tenant)
            .bind(&conv.region)
            .bind(&conv.conversation_id)
            .bind(limit as i64);
        if let Some(b) = &bound {
            q = q.bind(b);
        }
        let rows = q
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| StoreError::Cold(format!("range select: {e}")))?;

        let mut out: Vec<Message> = rows.iter().map(row_to_message).collect::<Result<_, _>>()?;
        out.sort_by(|a, b| a.message_id.cmp(&b.message_id));
        Ok(out)
    }

    pub async fn resync_from(
        &self,
        conv: &ConversationId,
        cursor: &MessageId,
    ) -> Result<Vec<Message>, StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &conv.tenant, &conv.region)
            .await?;
        let rows = sqlx::query(&format!(
            "SELECT tenant_id, region, conversation_id, message_id, thread_root_id, author, \
             author_kind, body_inline, body_nodes, client_nonce, edited_seq, state \
             FROM {} WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 \
             AND message_id > $4 ORDER BY message_id ASC",
            self.table
        ))
        .bind(&conv.tenant)
        .bind(&conv.region)
        .bind(&conv.conversation_id)
        .bind(cursor.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("resync select: {e}")))?;
        rows.iter().map(row_to_message).collect()
    }

    /// Finds one message by its artifact identity without materializing any other conversation.
    /// Callers must still authorize the returned parent conversation before exposing the row.
    pub async fn get_exact(
        &self,
        tenant: &str,
        message_id: &MessageId,
    ) -> Result<Option<Message>, StoreError> {
        validate_exact_message_ids(std::slice::from_ref(message_id))?;
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, tenant, &self.region)
            .await?;
        let row = sqlx::query(&format!(
            "SELECT tenant_id, region, conversation_id, message_id, thread_root_id, author, \
             author_kind, body_inline, body_nodes, client_nonce, edited_seq, state \
             FROM {} WHERE tenant_id = $1 AND region = $2 AND message_id = $3",
            self.table
        ))
        .bind(tenant)
        .bind(&self.region)
        .bind(message_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("exact message select: {e}")))?;
        row.as_ref().map(row_to_message).transpose()
    }

    /// Resolves a bounded card viewport to content-free message coordinates in one query.
    /// Conversation visibility is deliberately not inferred here; the Edge composes that owner.
    pub async fn locate_exact(
        &self,
        tenant: &str,
        message_ids: &[MessageId],
    ) -> Result<Vec<MessageLocation>, StoreError> {
        let message_ids = validate_exact_message_ids(message_ids)?;
        if message_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, tenant, &self.region)
            .await?;
        let rows = sqlx::query(&format!(
            "SELECT tenant_id, region, conversation_id, message_id, thread_root_id, state \
             FROM {} WHERE tenant_id = $1 AND region = $2 AND message_id = ANY($3::text[]) \
             ORDER BY message_id ASC",
            self.table
        ))
        .bind(tenant)
        .bind(&self.region)
        .bind(message_ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("exact message location select: {e}")))?;
        rows.iter().map(row_to_message_location).collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Mutates only the message table for storage-parity tests; emits no event.
    pub async fn revise_storage_only(
        &self,
        conv: &ConversationId,
        msg_id: &MessageId,
        body_inline: Vec<u8>,
        body_nodes: Vec<u8>,
        expect_seq: i32,
    ) -> Result<(), StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &conv.tenant, &conv.region)
            .await?;
        let revise_sql_tenant_id = format!(
            "UPDATE {} SET body_inline = $5, body_nodes = $6, edited_seq = edited_seq + 1, \
             state = 1 WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 \
             AND message_id = $4 AND edited_seq = $7",
            self.table
        );
        let updated = sqlx::query(&revise_sql_tenant_id)
            .bind(&conv.tenant)
            .bind(&conv.region)
            .bind(&conv.conversation_id)
            .bind(msg_id.as_str())
            .bind(&body_inline)
            .bind(&body_nodes)
            .bind(expect_seq)
            .execute(&mut *conn)
            .await
            .map_err(|e| StoreError::Cold(format!("revise: {e}")))?;
        if updated.rows_affected() == 0 {
            let actual: Option<i32> = sqlx::query_scalar(&format!(
                "SELECT edited_seq FROM {} WHERE tenant_id = $1 AND region = $2 \
                 AND conversation_id = $3 AND message_id = $4",
                self.table
            ))
            .bind(&conv.tenant)
            .bind(&conv.region)
            .bind(&conv.conversation_id)
            .bind(msg_id.as_str())
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| StoreError::Cold(format!("revise seq read: {e}")))?;
            return match actual {
                Some(actual) => Err(StoreError::CasConflict {
                    message_id: msg_id.clone(),
                    expected: expect_seq,
                    actual,
                }),
                None => Err(StoreError::NotFound(msg_id.clone())),
            };
        }
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Mutates only the message table for storage-parity tests; emits no event.
    pub async fn tombstone_storage_only(
        &self,
        conv: &ConversationId,
        msg_id: &MessageId,
        _reason: TombstoneReason,
    ) -> Result<(), StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &conv.tenant, &conv.region)
            .await?;
        let tombstone_sql_tenant_id = format!(
            "UPDATE {} SET state = 3, body_inline = '\\x', body_nodes = '\\x' \
             WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 AND message_id = $4",
            self.table
        );
        let updated = sqlx::query(&tombstone_sql_tenant_id)
            .bind(&conv.tenant)
            .bind(&conv.region)
            .bind(&conv.conversation_id)
            .bind(msg_id.as_str())
            .execute(&mut *conn)
            .await
            .map_err(|e| StoreError::Cold(format!("tombstone: {e}")))?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::NotFound(msg_id.clone()));
        }
        Ok(())
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

fn row_to_message(r: &sqlx::postgres::PgRow) -> Result<Message, StoreError> {
    let author_kind = decode_author_kind(r.get("author_kind"))?;
    let state = decode_message_state(r.get("state"))?;
    Ok(Message {
        message_id: MessageId(r.get::<String, _>("message_id")),
        conv: ConversationId {
            tenant: r.get::<String, _>("tenant_id"),
            region: r.get::<String, _>("region"),
            conversation_id: r.get::<String, _>("conversation_id"),
        },
        thread_root_id: r.get::<Option<String>, _>("thread_root_id").map(MessageId),
        author: r.get::<String, _>("author"),
        author_kind,
        body_inline: r.get::<Vec<u8>, _>("body_inline"),
        body_nodes: r.get::<Vec<u8>, _>("body_nodes"),
        client_nonce: r.get::<String, _>("client_nonce"),
        edited_seq: r.get::<i32, _>("edited_seq"),
        state,
    })
}

fn row_to_message_location(row: &sqlx::postgres::PgRow) -> Result<MessageLocation, StoreError> {
    Ok(MessageLocation {
        message_id: MessageId(row.get("message_id")),
        conv: ConversationId {
            tenant: row.get("tenant_id"),
            region: row.get("region"),
            conversation_id: row.get("conversation_id"),
        },
        thread_root_id: row
            .get::<Option<String>, _>("thread_root_id")
            .map(MessageId),
        state: decode_message_state(row.get("state"))?,
    })
}

fn validate_exact_message_ids(message_ids: &[MessageId]) -> Result<Vec<String>, StoreError> {
    if message_ids.len() > EXACT_MESSAGE_BATCH_MAX {
        return Err(StoreError::Cold(format!(
            "exact message lookup exceeds {EXACT_MESSAGE_BATCH_MAX} coordinates"
        )));
    }
    let mut canonical = BTreeSet::new();
    for message_id in message_ids {
        if !is_canonical_ulid(message_id.as_str()) {
            return Err(StoreError::Cold(
                "exact message lookup requires canonical identifiers".into(),
            ));
        }
        canonical.insert(message_id.0.clone());
    }
    Ok(canonical.into_iter().collect())
}

fn decode_author_kind(code: i16) -> Result<AuthorKind, StoreError> {
    u8::try_from(code)
        .ok()
        .and_then(super::author_kind_from_code)
        .ok_or_else(|| StoreError::Cold("stored message author kind is invalid".into()))
}

fn decode_message_state(code: i16) -> Result<super::MessageState, StoreError> {
    u8::try_from(code)
        .ok()
        .and_then(super::state_from_code)
        .ok_or_else(|| StoreError::Cold("stored message state is invalid".into()))
}

fn author_kind_code(k: AuthorKind) -> u8 {
    match k {
        AuthorKind::Human => 0,
        AuthorKind::Agent => 1,
        AuthorKind::Service => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_enum_codes_never_fall_back_to_human_or_active() {
        assert_eq!(decode_author_kind(0).unwrap(), AuthorKind::Human);
        assert_eq!(decode_author_kind(1).unwrap(), AuthorKind::Agent);
        assert_eq!(decode_author_kind(2).unwrap(), AuthorKind::Service);
        for invalid in [-1, 3, i16::MAX] {
            assert!(
                decode_author_kind(invalid).is_err(),
                "unknown author kind {invalid} must not be attributed to a human"
            );
        }

        assert_eq!(
            decode_message_state(0).unwrap(),
            crate::store::MessageState::Active
        );
        assert_eq!(
            decode_message_state(3).unwrap(),
            crate::store::MessageState::Tombstoned
        );
        for invalid in [-1, 4, i16::MAX] {
            assert!(
                decode_message_state(invalid).is_err(),
                "unknown message state {invalid} must not resurrect as active"
            );
        }
    }

    #[test]
    fn exact_message_lookups_are_canonical_deduplicated_and_bounded() {
        let first = MessageId("01J00000000000000000000000".into());
        let second = MessageId("01J00000000000000000000001".into());
        assert_eq!(
            validate_exact_message_ids(&[second.clone(), first.clone(), second]),
            Ok(vec![first.0, "01J00000000000000000000001".into()])
        );
        assert!(validate_exact_message_ids(&[MessageId("not-an-id".into())]).is_err());
        assert!(validate_exact_message_ids(
            &(0..=EXACT_MESSAGE_BATCH_MAX)
                .map(|_| MessageId("01J00000000000000000000000".into()))
                .collect::<Vec<_>>()
        )
        .is_err());
    }
}
