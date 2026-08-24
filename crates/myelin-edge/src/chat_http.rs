use crate::catalogue::{page_envelope, Handler, HandlerCtx};
use crate::chat_authz::ChatAuthorization;
use crate::chat_message_input::MessageInput;
use crate::error::EdgeError;
use crate::gateway::{sse_scope_for_resource, GatewayBuilder};
use crate::request::EdgeResponse;
use crate::sse::SseHub;
use crate::Method;
use crate::{ReferenceCard, ReferenceCardResolver};
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
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{PrincipalStore, StoreBackedCheck};
use myelin_storage::{KeyClass, KmsEngine, SubjectId, TenantScope};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use tokio::runtime::Handle;

use crate::runtime::drive_result_on_runtime;

const MAX_CHAT_JSON_BYTES: usize = 36 * 1024;
const DEFAULT_PAGE_LIMIT: u32 = 50;
const MAX_PAGE_LIMIT: u32 = 100;

#[derive(Clone)]
pub struct DurableChatReadApi {
    conversations: PgConversationStore,
    messages: PgMessageStore,
    runtime: Handle,
    kms: Arc<KmsEngine>,
    authorization: ChatAuthorization,
    reference_cards: Arc<dyn ReferenceCardResolver>,
}

impl DurableChatReadApi {
    pub fn new(
        pool: PgPool,
        region: impl Into<String>,
        runtime: Handle,
        kms: Arc<KmsEngine>,
        identity: StoreBackedCheck,
        reference_cards: Arc<dyn ReferenceCardResolver>,
    ) -> Self {
        Self {
            conversations: PgConversationStore::new(pool.clone()),
            messages: PgMessageStore::new(pool, region, MESSAGE_TABLE),
            runtime,
            kms,
            authorization: ChatAuthorization::new(identity),
            reference_cards,
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

    pub fn may_view_project(&self, principal: &Principal, project_id: &str) -> bool {
        self.authorization.may_view_project(principal, project_id)
    }

    async fn visible_conversation(
        &self,
        principal: &Principal,
        opaque_id: &str,
    ) -> Result<Conversation, EdgeError> {
        let conversation = self
            .conversations
            .get(&self.conversation_id(principal, opaque_id))
            .await
            .map_err(map_conversation_error)?;
        if conversation.archived {
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

    async fn postable_conversation(
        &self,
        principal: &Principal,
        opaque_id: &str,
    ) -> Result<Conversation, EdgeError> {
        let conversation = self
            .conversations
            .get(&self.conversation_id(principal, opaque_id))
            .await
            .map_err(map_conversation_error)?;
        if conversation.archived
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
                .list_visible(
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
        let conversation = self.drive(self.visible_conversation(principal, conversation_id))?;
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
        let readable = visible
            .iter()
            .map(|message| decode_readable_message(message, self.kms.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        let references = readable
            .iter()
            .flat_map(|message| message.reference_nodes())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let cards = self.reference_cards.resolve(principal, &references);
        let items = readable
            .iter()
            .map(|message| readable_message_json(message, &viewer, &cards))
            .collect::<Vec<_>>();
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
    principals: PrincipalStore,
    event_ids: Arc<dyn IdMinter>,
    object_ids: Arc<dyn UlidSource>,
}

pub struct PrivateConversationCreation {
    pub conversation: Conversation,
    pub members: Vec<PrincipalId>,
    pub expires_at: Option<Timestamp>,
    pub client_nonce: String,
    pub event_id: EventId,
    pub actor: Actor,
    pub now: Timestamp,
}

impl DurableChatMutationApi {
    pub fn new(reads: DurableChatReadApi, principals: PrincipalStore) -> Self {
        Self {
            reads,
            principals,
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
        let (mut persisted, created) =
            self.persist_conversation(conversation, client_nonce, event_id, actor, now.clone())?;
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

    pub fn create_private_conversation(
        &self,
        principal: &Principal,
        request: PrivateConversationCreation,
    ) -> Result<(Conversation, bool), EdgeError> {
        let PrivateConversationCreation {
            conversation,
            members,
            expires_at,
            client_nonce,
            event_id,
            actor,
            now,
        } = request;
        if !conversation.kind.membership_is_acl() {
            return Err(EdgeError::BadRequest(
                "private Chat creation requires a membership-governed conversation".into(),
            ));
        }
        if let Some(project_id) = conversation.parent_project.as_deref() {
            if !self
                .reads
                .authorization
                .may_view_project(principal, project_id)
            {
                return Err(EdgeError::NotFound("project not found".into()));
            }
        }
        let (mut persisted, created) =
            self.persist_conversation(conversation, &client_nonce, event_id, actor, now.clone())?;
        let zookie = self
            .reads
            .authorization
            .bind_direct_members(principal, &persisted, &members, expires_at, now)?;
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

    fn persist_conversation(
        &self,
        conversation: Conversation,
        client_nonce: &str,
        event_id: EventId,
        actor: Actor,
        now: Timestamp,
    ) -> Result<(Conversation, bool), EdgeError> {
        self.drive(async {
            match self
                .reads
                .conversations
                .create_co_commit(
                    &conversation,
                    client_nonce,
                    event_id,
                    actor,
                    now.clone(),
                    now,
                )
                .await
            {
                Ok(()) => Ok((conversation, true)),
                Err(ConversationError::AlreadyExists(_)) => {
                    if let Some(existing) = self
                        .reads
                        .conversations
                        .find_by_client_nonce(
                            &conversation.id.tenant,
                            &conversation.id.region,
                            client_nonce,
                        )
                        .await
                        .map_err(map_conversation_error)?
                    {
                        return if same_conversation_intent(&existing, &conversation) {
                            Ok((existing, false))
                        } else {
                            Err(EdgeError::Conflict(
                                "that idempotency key was already used for a different Chat conversation"
                                    .into(),
                            ))
                        };
                    }
                    if conversation.kind == ConversationKind::ChannelPublic {
                        return self
                            .reads
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
                            .filter(|existing| same_conversation_intent(existing, &conversation))
                            .map(|existing| (existing, false))
                            .ok_or_else(|| {
                                EdgeError::Conflict(
                                    "a topic with that channel and name already exists".into(),
                                )
                            });
                    }
                    Err(EdgeError::Conflict(
                        "the private conversation could not be replayed".into(),
                    ))
                }
                Err(error) => Err(map_conversation_error(error)),
            }
        })
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
        self.post_message_input(
            actor,
            authorized_viewer,
            conversation_id,
            &MessageInput::references(content, references),
            client_nonce,
        )
    }

    fn post_message_input(
        &self,
        actor: &Principal,
        authorized_viewer: &Principal,
        conversation_id: &str,
        input: &MessageInput,
        client_nonce: String,
    ) -> Result<MessageId, EdgeError> {
        if actor.tenant != authorized_viewer.tenant || actor.region != authorized_viewer.region {
            return Err(EdgeError::Forbidden(
                "Chat actor and delegated viewer must share one tenant and region".into(),
            ));
        }
        validate_ulid(conversation_id)?;
        input.validate_content()?;
        validate_nonce(&client_nonce)?;
        let conversation = self.drive(
            self.reads
                .postable_conversation(authorized_viewer, conversation_id),
        )?;
        let structured_nodes = input.resolve_nodes(actor, |principal_id| {
            self.resolve_mention(actor, &conversation, principal_id)
        })?;
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
                input.content.as_bytes(),
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

    fn resolve_mention(
        &self,
        actor: &Principal,
        conversation: &Conversation,
        principal_id: &PrincipalId,
    ) -> Result<Principal, EdgeError> {
        let unavailable = || {
            EdgeError::BadRequest(
                "Chat mention recipient must be an active member of this conversation".into(),
            )
        };
        if principal_id == &actor.principal_id {
            return Err(unavailable());
        }
        let scope = TenantScope::from_verified_token(actor, actor.region.clone());
        let row = self
            .principals
            .try_get_principal(&scope, principal_id)
            .map_err(|error| {
                EdgeError::Internal(format!("Chat mention directory lookup failed: {error}"))
            })?
            .ok_or_else(unavailable)?;
        let mentioned = Principal::new(
            row.tenant,
            row.region,
            row.principal_id,
            row.kind,
            row.data_role,
            row.status,
        );
        if mentioned.status != PrincipalStatus::Active {
            return Err(unavailable());
        }
        if !self
            .reads
            .authorization
            .may_read_channel(&mentioned, conversation)
        {
            return Err(unavailable());
        }
        Ok(mentioned)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateConversationBody {
    project_id: String,
    channel: String,
    topic: String,
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
        let body: MessageInput = parse_body(&ctx.request.body)?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let message_id = self.api.post_message_input(
            ctx.principal,
            ctx.principal,
            &conversation_id,
            &body,
            client_nonce,
        )?;
        Ok(no_store(EdgeResponse::json(
            201,
            &json!({ "message_id": message_id.as_str(), "durable": true }),
        )))
    }
}

/// Live delivery for one conversation. The subscription is authorized with
/// the SAME visibility gate as message reads (`visible_conversation`), so a
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
        self.api.drive(
            self.api
                .visible_conversation(ctx.principal, conversation_id),
        )?;
        let scope =
            sse_scope_for_resource(&ctx.principal.tenant.0, "conversation", conversation_id);
        Ok(EdgeResponse::sse(
            self.sse.subscribe("chat", &scope),
            ctx.identity.capability().expires_at_unix,
        ))
    }
}

pub fn register_chat(
    builder: GatewayBuilder,
    reads: DurableChatReadApi,
    principals: PrincipalStore,
) -> GatewayBuilder {
    let api = DurableChatMutationApi::new(reads.clone(), principals);
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

pub(crate) fn parse_messages_query(query: &str) -> Result<(u32, Option<String>), EdgeError> {
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
    if myelin_chat::is_canonical_ulid(value) {
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
        "kind": conversation.kind.as_token(),
        "project_id": conversation.parent_project,
        "channel": conversation.name,
        "topic": conversation.topic,
        "linked_ref": conversation.linked_ref,
        "pinned_canvas": conversation.pinned_canvas,
        "retention_days": conversation.retention_days,
    })
}

fn same_conversation_intent(left: &Conversation, right: &Conversation) -> bool {
    left.kind == right.kind
        && left.parent_project == right.parent_project
        && left.name == right.name
        && left.topic == right.topic
        && left.linked_ref == right.linked_ref
        && left.pinned_canvas == right.pinned_canvas
        && left.retention_days == right.retention_days
        && left.created_by == right.created_by
}

struct ReadableMessage<'a> {
    stored: &'a Message,
    content: String,
    nodes: Vec<InlineNode>,
}

impl ReadableMessage<'_> {
    fn reference_nodes(&self) -> impl Iterator<Item = String> + '_ {
        self.nodes.iter().filter_map(|node| match node {
            InlineNode::ArtifactRefNode(reference) | InlineNode::Embed(reference) => {
                Some(reference.0.clone())
            }
            InlineNode::Mention(_) => None,
        })
    }
}

fn decode_readable_message<'a>(
    message: &'a Message,
    kms: &KmsEngine,
) -> Result<ReadableMessage<'a>, EdgeError> {
    let content = decrypt_message_column(message, kms, &message.body_inline, "body_inline")?;
    let content = std::str::from_utf8(&content)
        .map_err(|_| EdgeError::Internal("stored Chat message is not valid UTF-8".into()))?
        .to_string();
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
    Ok(ReadableMessage {
        stored: message,
        content,
        nodes,
    })
}

fn readable_message_json(
    message: &ReadableMessage<'_>,
    viewer: &str,
    cards: &HashMap<String, ReferenceCard>,
) -> Value {
    let stored = message.stored;
    json!({
        "id": stored.message_id.as_str(),
        "author": stored.author,
        "author_kind": match stored.author_kind {
            AuthorKind::Human => "human",
            AuthorKind::Agent => "agent",
            AuthorKind::Service => "service",
        },
        "is_you": stored.author == viewer,
        "content": message.content,
        "nodes": message.nodes.iter().map(|node| message_node_json(node, cards)).collect::<Vec<_>>(),
        "edited": stored.edited_seq > 0,
        "state": match stored.state {
            MessageState::Active => "active",
            MessageState::Edited => "edited",
            MessageState::Deleted => "deleted",
            MessageState::Tombstoned => "tombstoned",
        },
        "created_at": stored.message_id.timestamp_ms().map(|value| value / 1000),
    })
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

fn message_node_json(node: &InlineNode, cards: &HashMap<String, ReferenceCard>) -> Value {
    match node {
        InlineNode::Mention(principal) => json!({
            "kind": "mention",
            "principal_id": principal.principal_id.0,
        }),
        InlineNode::ArtifactRefNode(reference) => {
            reference_node_json("artifact_ref", reference, cards)
        }
        InlineNode::Embed(reference) => reference_node_json("embed", reference, cards),
    }
}

fn reference_node_json(
    kind: &str,
    reference: &myelin_refs::ArtifactRef,
    cards: &HashMap<String, ReferenceCard>,
) -> Value {
    json!({
        "kind": kind,
        "ref": reference.0,
        "card": cards.get(&reference.0).unwrap_or(&ReferenceCard::Tombstone),
    })
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
        assert!(MessageInput::references("Ship it\nwith care", &[])
            .validate_content()
            .is_ok());
        assert!(MessageInput::references("  ", &[])
            .validate_content()
            .is_err());
        assert!(MessageInput::references("bad\0message", &[])
            .validate_content()
            .is_err());
        assert!(validate_nonce("01J-client_nonce").is_ok());
        assert!(validate_nonce("spaces are not stable").is_err());
    }

    #[test]
    fn reference_nodes_carry_the_viewers_card_or_a_safe_tombstone() {
        let reference = myelin_refs::ArtifactRef("myelin://acme/issue/issue/ENG-41".into());
        let node = InlineNode::ArtifactRefNode(reference.clone());
        let card = ReferenceCard::Projection {
            title: "Coordinate the rollout".into(),
            state: "open".into(),
            icon: "issue".into(),
            render_hint: "issue".into(),
            sub_anchor: None,
            flag: None,
        };

        assert_eq!(
            message_node_json(&node, &HashMap::from([(reference.0.clone(), card)])),
            json!({
                "kind": "artifact_ref",
                "ref": reference.0.clone(),
                "card": {
                    "kind": "projection",
                    "title": "Coordinate the rollout",
                    "state": "open",
                    "icon": "issue",
                    "render_hint": "issue",
                    "sub_anchor": null,
                    "flag": null,
                }
            })
        );
        assert_eq!(
            message_node_json(&node, &HashMap::new()),
            json!({
                "kind": "artifact_ref",
                "ref": reference.0.clone(),
                "card": { "kind": "tombstone" }
            })
        );
    }

    #[test]
    fn message_body_contains_domain_input_not_retry_transport_metadata() {
        let body: MessageInput =
            serde_json::from_slice(br#"{"content":"Ship it"}"#).expect("content-only message body");
        assert_eq!(body.content, "Ship it");
        assert!(body
            .resolve_nodes(&principal("acme"), |_| unreachable!())
            .unwrap()
            .is_empty());
        assert!(serde_json::from_slice::<MessageInput>(
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
}
