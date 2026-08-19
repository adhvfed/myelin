use crate::catalogue::{page_envelope, Handler, HandlerCtx};
use crate::chat_authz::ChatAuthorization;
use crate::error::EdgeError;
use crate::gateway::{sse_scope_for_resource, GatewayBuilder};
use crate::request::EdgeResponse;
use crate::sse::SseHub;
use crate::Method;
use myelin_chat::conversation::{Conversation, ConversationError, ConversationKind};
use myelin_chat::events::{event_actor_pseudonym, pseudonymized_event_principal};
use myelin_chat::store::pg::PgMessageStore;
use myelin_chat::store::pg_conversation::{PgConversationStore, MESSAGE_TABLE};
use myelin_chat::store::{
    AuthorKind, ConversationId, Message, MessageId, MessageState, NewMessage, RangeCursor,
    StoreError, SystemUlidSource, UlidSource,
};
use myelin_chat::{
    channel_ref, decode_encrypted_body, decrypt_body, encode_encrypted_body, encrypt_body,
    ChatFreeText,
};
use myelin_content::{InlineNode, OBJ};
use myelin_events::{Actor, EventId, IdMinter, Timestamp};
use myelin_identity::{Principal, PrincipalKind, PrincipalStatus};
use myelin_identity_service::StoreBackedCheck;
use myelin_storage::{KeyClass, KmsEngine, SubjectId};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::runtime::Handle;

use crate::runtime::drive_result_on_runtime;

const MAX_CHAT_JSON_BYTES: usize = 36 * 1024;
const MAX_MESSAGE_BYTES: usize = 32 * 1024;
const MAX_MESSAGE_REFERENCES: usize = 32;
const MAX_ARTIFACT_REF_BYTES: usize = 1024;
const DEFAULT_PAGE_LIMIT: u32 = 50;
const MAX_PAGE_LIMIT: u32 = 100;

#[derive(Clone)]
pub struct DurableChatReadApi {
    conversations: PgConversationStore,
    messages: PgMessageStore,
    runtime: Handle,
    kms: Arc<KmsEngine>,
    authorization: ChatAuthorization,
}

impl DurableChatReadApi {
    pub fn new(
        pool: PgPool,
        region: impl Into<String>,
        runtime: Handle,
        kms: Arc<KmsEngine>,
        identity: StoreBackedCheck,
    ) -> Self {
        Self {
            conversations: PgConversationStore::new(pool.clone()),
            messages: PgMessageStore::new(pool, region, MESSAGE_TABLE),
            runtime,
            kms,
            authorization: ChatAuthorization::new(identity),
        }
    }

    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: std::future::Future<Output = Result<T, EdgeError>>,
    {
        drive_result_on_runtime(
            &self.runtime,
            future,
            EdgeError::Unavailable("Chat requires the Edge multi-thread runtime".into()),
        )
    }

    fn conversation_id(&self, principal: &Principal, opaque_id: &str) -> ConversationId {
        ConversationId::new(
            principal.tenant.0.clone(),
            principal.region.0.clone(),
            opaque_id,
        )
    }

    async fn public_conversation(
        &self,
        principal: &Principal,
        opaque_id: &str,
    ) -> Result<Conversation, EdgeError> {
        let conversation = self
            .conversations
            .get(&self.conversation_id(principal, opaque_id))
            .await
            .map_err(map_conversation_error)?;
        if conversation.kind != ConversationKind::ChannelPublic || conversation.archived {
            return Err(EdgeError::NotFound("conversation not found".into()));
        }
        if !self
            .authorization
            .may_read_channel(principal, &conversation)
        {
            return Err(EdgeError::NotFound("conversation not found".into()));
        }
        Ok(conversation)
    }

    async fn postable_public_conversation(
        &self,
        principal: &Principal,
        opaque_id: &str,
    ) -> Result<Conversation, EdgeError> {
        let conversation = self
            .conversations
            .get(&self.conversation_id(principal, opaque_id))
            .await
            .map_err(map_conversation_error)?;
        if conversation.kind != ConversationKind::ChannelPublic
            || conversation.archived
            || !self
                .authorization
                .may_post_to_channel(principal, &conversation)
        {
            return Err(EdgeError::NotFound("conversation not found".into()));
        }
        Ok(conversation)
    }

    pub fn list_conversations(
        &self,
        principal: &Principal,
        limit: u32,
        cursor: Option<String>,
    ) -> Result<Value, EdgeError> {
        validate_query(limit, cursor.as_deref())?;
        if principal.status != PrincipalStatus::Active {
            return Ok(page_envelope(json!([]), None, limit as usize));
        }
        let rows = self.drive(async {
            self.conversations
                .list_visible_public(
                    &principal.tenant.0,
                    &principal.region.0,
                    &principal.principal_id.0,
                    cursor.as_deref(),
                    limit + 1,
                )
                .await
                .map_err(map_conversation_error)
        })?;
        let has_more = rows.len() > limit as usize;
        let visible = &rows[..rows.len().min(limit as usize)];
        let next = has_more
            .then(|| visible.last().map(|row| row.id.conversation_id.clone()))
            .flatten();
        let items = visible.iter().map(conversation_json).collect::<Vec<_>>();
        Ok(page_envelope(json!(items), next, limit as usize))
    }

    pub fn read_messages(
        &self,
        principal: &Principal,
        conversation_id: &str,
        limit: u32,
        before: Option<String>,
    ) -> Result<Value, EdgeError> {
        validate_ulid(conversation_id)?;
        validate_query(limit, before.as_deref())?;
        let conversation = self.drive(self.public_conversation(principal, conversation_id))?;
        let range = before
            .map(|cursor| RangeCursor::Before(MessageId(cursor)))
            .unwrap_or(RangeCursor::Recent);
        let messages = self.drive(async {
            self.messages
                .range(&conversation.id, range, limit + 1)
                .await
                .map_err(map_store_error)
        })?;
        let has_more = messages.len() > limit as usize;
        let start = messages.len().saturating_sub(limit as usize);
        let visible = &messages[start..];
        let next = has_more
            .then(|| visible.first().map(|message| message.message_id.0.clone()))
            .flatten();
        let viewer = event_actor_pseudonym(&principal.tenant.0, &principal.principal_id.0);
        let items = visible
            .iter()
            .map(|message| message_json(message, &viewer, self.kms.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({
            "conversation": conversation_json(&conversation),
            "items": items,
            "page": { "next_cursor": next, "limit": limit },
        }))
    }
}

#[derive(Clone)]
pub struct DurableChatMutationApi {
    reads: DurableChatReadApi,
    event_ids: Arc<dyn IdMinter>,
    object_ids: Arc<dyn UlidSource>,
}

impl DurableChatMutationApi {
    pub fn new(reads: DurableChatReadApi) -> Self {
        Self {
            reads,
            event_ids: Arc::new(myelin_events::UlidMinter::new()),
            object_ids: Arc::new(SystemUlidSource::new()),
        }
    }

    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: std::future::Future<Output = Result<T, EdgeError>>,
    {
        self.reads.drive(future)
    }

    fn conversation_id(&self, principal: &Principal, opaque_id: &str) -> ConversationId {
        self.reads.conversation_id(principal, opaque_id)
    }

    fn create_public_conversation(
        &self,
        principal: &Principal,
        conversation: Conversation,
        client_nonce: &str,
        event_id: EventId,
        actor: Actor,
        now: Timestamp,
    ) -> Result<(Conversation, bool), EdgeError> {
        let project_id = conversation
            .parent_project
            .as_deref()
            .expect("validated public conversation project");
        if !self
            .reads
            .authorization
            .may_view_project(principal, project_id)
        {
            return Err(EdgeError::NotFound("project not found".into()));
        }
        let (mut persisted, created) = self.drive(async {
            match self
                .reads
                .conversations
                .create_co_commit(
                    &conversation,
                    client_nonce,
                    event_id,
                    actor,
                    now.clone(),
                    now.clone(),
                )
                .await
            {
                Ok(()) => Ok((conversation, true)),
                Err(ConversationError::AlreadyExists(_)) => {
                    if let Some(existing) = self
                        .reads
                        .conversations
                        .find_public_by_client_nonce(
                            &conversation.id.tenant,
                            &conversation.id.region,
                            client_nonce,
                        )
                        .await
                        .map_err(map_conversation_error)?
                    {
                        if same_public_topic(&existing, &conversation) {
                            return Ok((existing, false));
                        }
                        return Err(EdgeError::Conflict(
                            "that idempotency key was already used for a different Chat conversation"
                                .into(),
                        ));
                    }
                    self.reads
                        .conversations
                        .find_public_by_name_topic(
                            &conversation.id.tenant,
                            &conversation.id.region,
                            conversation
                                .parent_project
                                .as_deref()
                                .expect("validated public project"),
                            conversation
                                .name
                                .as_deref()
                                .expect("validated public channel"),
                            conversation
                                .topic
                                .as_deref()
                                .expect("validated public topic"),
                        )
                        .await
                        .map_err(map_conversation_error)?
                        .filter(|existing| same_public_topic(existing, &conversation))
                        .map(|existing| (existing, false))
                        .ok_or_else(|| {
                            EdgeError::Conflict(
                                "a topic with that channel and name already exists".into(),
                            )
                        })
                }
                Err(error) => Err(map_conversation_error(error)),
            }
        })?;
        let zookie = self
            .reads
            .authorization
            .bind_public_project(principal, &persisted, now)?;
        self.drive(async {
            self.reads
                .conversations
                .stamp_acl_zookie(&persisted.id, &zookie.0)
                .await
                .map_err(map_conversation_error)
        })?;
        persisted.acl_zookie = Some(zookie.0);
        Ok((persisted, created))
    }

    pub fn post_message(
        &self,
        actor: &Principal,
        authorized_viewer: &Principal,
        conversation_id: &str,
        content: &str,
        references: &[String],
        client_nonce: String,
    ) -> Result<MessageId, EdgeError> {
        if actor.tenant != authorized_viewer.tenant || actor.region != authorized_viewer.region {
            return Err(EdgeError::Forbidden(
                "Chat actor and delegated viewer must share one tenant and region".into(),
            ));
        }
        validate_ulid(conversation_id)?;
        validate_message(content)?;
        validate_nonce(&client_nonce)?;
        let structured_nodes = reference_nodes(actor, content, references)?;
        let conversation = self.drive(
            self.reads
                .postable_public_conversation(authorized_viewer, conversation_id),
        )?;
        let event_principal = pseudonymized_event_principal(&actor.tenant.0, actor);
        let author_kind = match actor.kind {
            PrincipalKind::Human => AuthorKind::Human,
            PrincipalKind::Agent { .. } => AuthorKind::Agent,
            PrincipalKind::Service => AuthorKind::Service,
        };
        let new_message = NewMessage {
            conv: conversation.id,
            thread_root_id: None,
            author: event_principal.principal_id.0.clone(),
            author_kind,
            body_inline: encrypt_message_column(
                self.reads.kms.as_ref(),
                actor,
                &event_principal.principal_id.0,
                ChatFreeText::BodyInline,
                content.as_bytes(),
            )?,
            body_nodes: encrypt_message_column(
                self.reads.kms.as_ref(),
                actor,
                &event_principal.principal_id.0,
                ChatFreeText::BodyNodes,
                &serde_json::to_vec(&structured_nodes).map_err(|error| {
                    EdgeError::Internal(format!(
                        "Chat structured nodes could not be encoded: {error}"
                    ))
                })?,
            )?,
            client_nonce,
        };
        let now = now_timestamp();
        self.drive(async {
            self.reads
                .messages
                .append_structured_co_commit(
                    self.object_ids.as_ref(),
                    new_message,
                    EventId(self.event_ids.mint().0),
                    self.event_ids.as_ref(),
                    &structured_nodes,
                    Actor(event_principal),
                    now.clone(),
                    now,
                )
                .await
                .map_err(map_store_error)
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateConversationBody {
    project_id: String,
    channel: String,
    topic: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostMessageBody {
    content: String,
    #[serde(default)]
    references: Vec<String>,
}

struct ConversationListHandler {
    api: DurableChatReadApi,
}

impl Handler for ConversationListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let (limit, cursor) = parse_page_query(&ctx.request.query)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self.api.list_conversations(ctx.principal, limit, cursor)?,
        )))
    }
}

struct ConversationCreateHandler {
    api: DurableChatMutationApi,
}

impl Handler for ConversationCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body: CreateConversationBody = parse_body(&ctx.request.body)?;
        validate_project_id(&body.project_id)?;
        validate_label("channel", &body.channel)?;
        validate_label("topic", &body.topic)?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let opaque_id = self.api.object_ids.mint();
        let event_id = EventId(self.api.event_ids.mint().0);
        let event_principal = pseudonymized_event_principal(&ctx.principal.tenant.0, ctx.principal);
        let conversation = Conversation {
            id: self.api.conversation_id(ctx.principal, opaque_id.as_str()),
            kind: ConversationKind::ChannelPublic,
            home_cell: Conversation::home_cell_for(
                &self.api.conversation_id(ctx.principal, opaque_id.as_str()),
            ),
            parent_project: Some(body.project_id),
            name: Some(body.channel),
            topic: Some(body.topic),
            linked_ref: None,
            pinned_canvas: None,
            retention_days: None,
            archived: false,
            created_by: event_principal.principal_id.0.clone(),
            acl_zookie: None,
        };
        let now = now_timestamp();
        let (conversation, created) = self.api.create_public_conversation(
            ctx.principal,
            conversation,
            &client_nonce,
            event_id,
            Actor(event_principal),
            now,
        )?;
        Ok(no_store(EdgeResponse::json(
            if created { 201 } else { 200 },
            &json!({ "conversation": conversation_json(&conversation), "durable": true }),
        )))
    }
}

struct MessageListHandler {
    api: DurableChatReadApi,
}

impl Handler for MessageListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let conversation_id = conversation_param(ctx)?.to_string();
        let (limit, before) = parse_messages_query(&ctx.request.query)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self
                .api
                .read_messages(ctx.principal, &conversation_id, limit, before)?,
        )))
    }
}

struct MessagePostHandler {
    api: DurableChatMutationApi,
}

impl Handler for MessagePostHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let conversation_id = conversation_param(ctx)?.to_string();
        let body: PostMessageBody = parse_body(&ctx.request.body)?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let message_id = self.api.post_message(
            ctx.principal,
            ctx.principal,
            &conversation_id,
            &body.content,
            &body.references,
            client_nonce,
        )?;
        Ok(no_store(EdgeResponse::json(
            201,
            &json!({ "message_id": message_id.as_str(), "durable": true }),
        )))
    }
}

/// Live delivery for one conversation. The subscription is authorized with
/// the SAME visibility gate as message reads (`public_conversation`), so a
/// viewer who cannot read a conversation cannot observe its activity either.
/// Frames carry references only (conversation id, message id); subscribers
/// fetch content through the authorized read path.
struct ConversationEventsHandler {
    api: DurableChatReadApi,
    sse: SseHub,
}

impl Handler for ConversationEventsHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let conversation_id = conversation_param(ctx)?;
        self.api
            .drive(self.api.public_conversation(ctx.principal, conversation_id))?;
        let scope = sse_scope_for_resource(
            &ctx.principal.tenant.0,
            "conversation",
            conversation_id,
        );
        Ok(EdgeResponse::sse(
            self.sse.subscribe("chat", &scope),
            ctx.identity.capability().expires_at_unix,
        ))
    }
}

pub fn register_chat(
    builder: GatewayBuilder,
    pool: PgPool,
    region: impl Into<String>,
    runtime: Handle,
    kms: Arc<KmsEngine>,
    identity: StoreBackedCheck,
) -> GatewayBuilder {
    let reads = DurableChatReadApi::new(pool, region, runtime, kms, identity);
    let api = DurableChatMutationApi::new(reads.clone());
    let sse = builder.sse_hub();
    builder
        .route(
            Method::Get,
            "/v1/chat/conversations",
            "chat.conversations.list",
            Arc::new(ConversationListHandler { api: reads.clone() }),
        )
        .route(
            Method::Post,
            "/v1/chat/conversations",
            "chat.conversation.create",
            Arc::new(ConversationCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/chat/conversations/{conversation}/messages",
            "chat.messages.list",
            Arc::new(MessageListHandler { api: reads.clone() }),
        )
        .route(
            Method::Post,
            "/v1/chat/conversations/{conversation}/messages",
            "chat.message.post",
            Arc::new(MessagePostHandler { api }),
        )
        .route(
            Method::Get,
            "/v1/chat/conversations/{conversation}/events",
            "chat.conversation.events.subscribe",
            Arc::new(ConversationEventsHandler { api: reads, sse }),
        )
}

fn parse_body<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, EdgeError> {
    if body.is_empty() {
        return Err(EdgeError::BadRequest("Chat request body is empty".into()));
    }
    if body.len() > MAX_CHAT_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(
            "Chat request exceeds the interactive body limit".into(),
        ));
    }
    serde_json::from_slice(body)
        .map_err(|error| EdgeError::BadRequest(format!("invalid Chat request: {error}")))
}

fn parse_page_query(query: &str) -> Result<(u32, Option<String>), EdgeError> {
    parse_query(query, "cursor")
}

fn parse_messages_query(query: &str) -> Result<(u32, Option<String>), EdgeError> {
    parse_query(query, "before")
}

fn parse_query(query: &str, cursor_name: &str) -> Result<(u32, Option<String>), EdgeError> {
    let mut limit = None;
    let mut cursor = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair
                .split_once('=')
                .ok_or_else(|| EdgeError::BadRequest("malformed Chat query parameter".into()))?;
            match name {
                "limit" if limit.is_none() => {
                    limit = Some(value.parse::<u32>().map_err(|_| {
                        EdgeError::BadRequest("Chat limit must be an integer".into())
                    })?);
                }
                name if name == cursor_name && cursor.is_none() => {
                    cursor = Some(value.to_string());
                }
                "limit" => return Err(EdgeError::BadRequest("duplicate Chat limit".into())),
                name if name == cursor_name => {
                    return Err(EdgeError::BadRequest(format!(
                        "duplicate Chat {cursor_name}"
                    )))
                }
                other => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown Chat query parameter `{other}`"
                    )))
                }
            }
        }
    }
    let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    validate_query(limit, cursor.as_deref())?;
    Ok((limit, cursor))
}

fn validate_query(limit: u32, cursor: Option<&str>) -> Result<(), EdgeError> {
    if !(1..=MAX_PAGE_LIMIT).contains(&limit) {
        return Err(EdgeError::BadRequest(
            "Chat limit must be between 1 and 100".into(),
        ));
    }
    if let Some(cursor) = cursor {
        validate_ulid(cursor)?;
    }
    Ok(())
}

fn conversation_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let value = ctx
        .params
        .get("conversation")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a conversation id".into()))?;
    validate_ulid(value)?;
    Ok(value)
}

fn validate_ulid(value: &str) -> Result<(), EdgeError> {
    if value.len() == 26
        && value
            .bytes()
            .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte))
    {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "Chat cursor and conversation ids must be canonical ULIDs".into(),
        ))
    }
}

fn validate_label(field: &str, value: &str) -> Result<(), EdgeError> {
    if value.trim() == value
        && !value.is_empty()
        && value.len() <= 255
        && !value.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(format!(
            "Chat {field} must be 1-255 clean UTF-8 bytes without surrounding whitespace"
        )))
    }
}

fn validate_project_id(value: &str) -> Result<(), EdgeError> {
    let parsed = sqlx::types::Uuid::parse_str(value)
        .map_err(|_| EdgeError::BadRequest("Chat project_id must be a canonical UUID".into()))?;
    if parsed.to_string() != value {
        return Err(EdgeError::BadRequest(
            "Chat project_id must be a canonical UUID".into(),
        ));
    }
    Ok(())
}

fn validate_message(content: &str) -> Result<(), EdgeError> {
    if content.len() > MAX_MESSAGE_BYTES || content.trim().is_empty() {
        return Err(EdgeError::BadRequest(
            "Chat message must contain 1-32768 UTF-8 bytes".into(),
        ));
    }
    if content.chars().any(|character| {
        character == '\0' || (character.is_control() && character != '\n' && character != '\t')
    }) {
        return Err(EdgeError::BadRequest(
            "Chat message contains an unsupported control character".into(),
        ));
    }
    Ok(())
}

fn reference_nodes(
    principal: &Principal,
    content: &str,
    references: &[String],
) -> Result<Vec<InlineNode>, EdgeError> {
    if references.len() > MAX_MESSAGE_REFERENCES {
        return Err(EdgeError::BadRequest(format!(
            "Chat message may contain at most {MAX_MESSAGE_REFERENCES} structured references"
        )));
    }
    let placeholders = content
        .chars()
        .filter(|character| *character == OBJ)
        .count();
    if placeholders != references.len() {
        return Err(EdgeError::BadRequest(
            "Chat content must contain one U+FFFC placeholder for each structured reference".into(),
        ));
    }
    references
        .iter()
        .map(|reference| {
            if reference.len() > MAX_ARTIFACT_REF_BYTES {
                return Err(EdgeError::BadRequest(format!(
                    "Chat ArtifactRef exceeds {MAX_ARTIFACT_REF_BYTES} bytes"
                )));
            }
            let parsed = myelin_refs::parse_scoped(reference).map_err(|error| {
                EdgeError::BadRequest(format!("invalid Chat ArtifactRef: {error}"))
            })?;
            if parsed.tenant != principal.tenant {
                return Err(EdgeError::BadRequest(
                    "Chat cannot store a cross-tenant ArtifactRef".into(),
                ));
            }
            Ok(InlineNode::ArtifactRefNode(parsed.artifact_ref))
        })
        .collect()
}

fn validate_nonce(value: &str) -> Result<(), EdgeError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(EdgeError::BadRequest(
            "Chat client_nonce must be 1-128 URL-safe characters".into(),
        ));
    }
    Ok(())
}

fn require_empty_query(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.query.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "Chat mutation accepts no query parameters".into(),
        ))
    }
}

fn conversation_json(conversation: &Conversation) -> Value {
    json!({
        "id": conversation.id.conversation_id,
        "ref": channel_ref(&conversation.id).0,
        "project_id": conversation.parent_project,
        "channel": conversation.name,
        "topic": conversation.topic,
        "linked_ref": conversation.linked_ref,
        "pinned_canvas": conversation.pinned_canvas,
    })
}

fn same_public_topic(left: &Conversation, right: &Conversation) -> bool {
    left.parent_project == right.parent_project
        && left.name == right.name
        && left.topic == right.topic
}

fn message_json(message: &Message, viewer: &str, kms: &KmsEngine) -> Result<Value, EdgeError> {
    let content = decrypt_message_column(message, kms, &message.body_inline, "body_inline")?;
    let content = std::str::from_utf8(&content)
        .map_err(|_| EdgeError::Internal("stored Chat message is not valid UTF-8".into()))?;
    let nodes = decode_message_nodes(message, kms)?;
    if content
        .chars()
        .filter(|character| *character == OBJ)
        .count()
        != nodes.len()
    {
        return Err(EdgeError::Internal(
            "stored Chat content and structured nodes disagree".into(),
        ));
    }
    Ok(json!({
        "id": message.message_id.as_str(),
        "author": message.author,
        "author_kind": match message.author_kind {
            AuthorKind::Human => "human",
            AuthorKind::Agent => "agent",
            AuthorKind::Service => "service",
        },
        "is_you": message.author == viewer,
        "content": content,
        "nodes": nodes.iter().map(message_node_json).collect::<Vec<_>>(),
        "edited": message.edited_seq > 0,
        "state": match message.state {
            MessageState::Active => "active",
            MessageState::Edited => "edited",
            MessageState::Deleted => "deleted",
            MessageState::Tombstoned => "tombstoned",
        },
        "created_at": message.message_id.timestamp_ms().map(|value| value / 1000),
    }))
}

fn decode_message_nodes(message: &Message, kms: &KmsEngine) -> Result<Vec<InlineNode>, EdgeError> {
    // The first public Edge floor wrote only an empty plaintext node array.
    // It contains no personal data and remains readable during the rolling
    // transition; every newly written node array is encrypted.
    if message.body_nodes.is_empty() || message.body_nodes == b"[]" {
        return Ok(Vec::new());
    }
    let node_bytes = decrypt_message_column(message, kms, &message.body_nodes, "body_nodes")?;
    serde_json::from_slice(&node_bytes)
        .map_err(|_| EdgeError::Internal("stored Chat structured nodes are not valid".into()))
}

fn message_node_json(node: &InlineNode) -> Value {
    match node {
        InlineNode::Mention(principal) => json!({
            "kind": "mention",
            "principal_id": principal.principal_id.0,
        }),
        InlineNode::ArtifactRefNode(reference) => json!({
            "kind": "artifact_ref",
            "ref": reference.0,
        }),
        InlineNode::Embed(reference) => json!({
            "kind": "embed",
            "ref": reference.0,
        }),
    }
}

fn decrypt_message_column(
    message: &Message,
    kms: &KmsEngine,
    encoded: &[u8],
    column: &str,
) -> Result<Vec<u8>, EdgeError> {
    let encrypted = decode_encrypted_body(encoded).map_err(|_| {
        EdgeError::Internal(format!(
            "stored Chat message has an invalid encrypted {column}"
        ))
    })?;
    if encrypted.key_ref.tenant.as_str() != message.conv.tenant
        || encrypted.key_ref.class != KeyClass::Subject(message.author.clone())
    {
        return Err(EdgeError::Internal(
            "stored Chat message encryption scope does not match its author".into(),
        ));
    }
    decrypt_body(
        kms,
        &myelin_tenancy::Region(message.conv.region.clone()),
        &encrypted,
    )
    .map_err(|_| EdgeError::Internal(format!("stored Chat {column} cannot be decrypted")))
}

fn encrypt_message_column(
    kms: &KmsEngine,
    principal: &Principal,
    author: &str,
    kind: ChatFreeText,
    plaintext: &[u8],
) -> Result<Vec<u8>, EdgeError> {
    let column = encrypt_body(
        kms,
        &principal.region,
        &principal.tenant,
        &SubjectId::new(author),
        kind,
        plaintext,
    )
    .map_err(|error| EdgeError::Internal(format!("Chat message encryption failed: {error}")))?;
    encode_encrypted_body(&column)
        .map_err(|error| EdgeError::Internal(format!("Chat message encoding failed: {error}")))
}

fn map_conversation_error(error: ConversationError) -> EdgeError {
    match error {
        ConversationError::NotFound(_) => EdgeError::NotFound("conversation not found".into()),
        ConversationError::AlreadyExists(_) => {
            EdgeError::Conflict("a topic with that channel and name already exists".into())
        }
        ConversationError::SchemaViolation(reason) => EdgeError::BadRequest(reason),
        ConversationError::Storage(reason) => EdgeError::Internal(reason),
    }
}

fn map_store_error(error: StoreError) -> EdgeError {
    match error {
        StoreError::NotFound(_) => EdgeError::NotFound("message not found".into()),
        StoreError::CasConflict { .. } | StoreError::DuplicateNonce { .. } => {
            EdgeError::Conflict(error.to_string())
        }
        StoreError::Cold(reason) => EdgeError::Internal(reason),
    }
}

fn now_timestamp() -> Timestamp {
    let now = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now());
    Timestamp(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(tenant: &str) -> Principal {
        Principal::stub(
            myelin_identity::PrincipalId("author".into()),
            PrincipalKind::Human,
            myelin_tenancy::TenantId(tenant.into()),
        )
    }

    #[test]
    fn query_parser_is_bounded_and_strict() {
        assert_eq!(parse_page_query(""), Ok((50, None)));
        assert_eq!(parse_page_query("limit=10"), Ok((10, None)));
        assert!(parse_page_query("limit=0").is_err());
        assert!(parse_page_query("limit=1&limit=2").is_err());
        assert!(parse_page_query("offset=1").is_err());
        assert!(parse_messages_query("before=not-an-id").is_err());
    }

    #[test]
    fn mutation_input_rejects_blank_oversized_and_controls() {
        assert!(validate_project_id("11111111-1111-1111-1111-111111111111").is_ok());
        assert!(validate_project_id("11111111-1111-1111-1111-11111111111A").is_err());
        assert!(validate_label("channel", "engineering").is_ok());
        assert!(validate_label("channel", " engineering").is_err());
        assert!(validate_message("Ship it\nwith care").is_ok());
        assert!(validate_message("  ").is_err());
        assert!(validate_message("bad\0message").is_err());
        assert!(validate_nonce("01J-client_nonce").is_ok());
        assert!(validate_nonce("spaces are not stable").is_err());
    }

    #[test]
    fn message_body_contains_domain_input_not_retry_transport_metadata() {
        let body: PostMessageBody =
            serde_json::from_slice(br#"{"content":"Ship it"}"#).expect("content-only message body");
        assert_eq!(body.content, "Ship it");
        assert!(body.references.is_empty());
        assert!(serde_json::from_slice::<PostMessageBody>(
            br#"{"content":"Ship it","client_nonce":"legacy-42"}"#
        )
        .is_err());

        let request = crate::request::EdgeRequest::new(
            "POST",
            "/v1/chat/conversations/01J00000000000000000000000/messages",
            "",
            vec![("idempotency-key".into(), "send-42".into())],
            Vec::new(),
        );
        let nonce = request
            .stable_idempotency_nonce("svc:agent")
            .expect("standard retry header");
        assert!(validate_nonce(&nonce).is_ok());
        assert_ne!(
            nonce,
            request
                .stable_idempotency_nonce("svc:other")
                .expect("principal-scoped retry header")
        );

        let missing = crate::request::EdgeRequest::new(
            "POST",
            "/v1/chat/conversations/01J00000000000000000000000/messages",
            "",
            Vec::new(),
            Vec::new(),
        );
        assert!(missing.stable_idempotency_nonce("svc:agent").is_err());
    }

    #[test]
    fn structured_references_are_positionally_complete_and_tenant_scoped() {
        let issue_ref = "myelin://acme/issue/issue/ENG-41";
        assert_eq!(
            reference_nodes(&principal("acme"), "Tracking \u{FFFC}", &[issue_ref.into()]).unwrap(),
            vec![InlineNode::ArtifactRefNode(myelin_refs::ArtifactRef(
                issue_ref.into()
            ))]
        );

        assert!(reference_nodes(&principal("acme"), "Tracking", &[issue_ref.into()]).is_err());
        assert!(reference_nodes(&principal("acme"), "Tracking \u{FFFC}", &[]).is_err());
        assert!(reference_nodes(
            &principal("acme"),
            "Tracking \u{FFFC}",
            &["not-a-reference".into()]
        )
        .is_err());
        assert!(reference_nodes(
            &principal("acme"),
            "Tracking \u{FFFC}",
            &["myelin://other/issue/issue/ENG-41".into()]
        )
        .is_err());
    }

    #[test]
    fn structured_reference_count_is_bounded_before_parsing() {
        let references = vec!["not-a-reference".into(); MAX_MESSAGE_REFERENCES + 1];
        let content = OBJ.to_string().repeat(references.len());
        let error = reference_nodes(&principal("acme"), &content, &references).unwrap_err();
        assert!(matches!(error, EdgeError::BadRequest(message) if message.contains("at most")));
    }
}
