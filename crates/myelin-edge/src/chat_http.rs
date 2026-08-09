use crate::catalogue::{page_envelope, Handler, HandlerCtx};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
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
    decode_encrypted_body, decrypt_body, encode_encrypted_body, encrypt_body, ChatFreeText,
};
use myelin_events::{Actor, EventId, IdMinter, Timestamp};
use myelin_identity::{Principal, PrincipalKind};
use myelin_storage::{KeyClass, KmsEngine, SubjectId};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::future::Future;
use std::sync::Arc;
use tokio::runtime::{Handle, RuntimeFlavor};

const MAX_CHAT_JSON_BYTES: usize = 36 * 1024;
const MAX_MESSAGE_BYTES: usize = 32 * 1024;
const DEFAULT_PAGE_LIMIT: u32 = 50;
const MAX_PAGE_LIMIT: u32 = 100;

#[derive(Clone)]
struct DurableChatApi {
    conversations: PgConversationStore,
    messages: PgMessageStore,
    runtime: Handle,
    event_ids: Arc<dyn IdMinter>,
    object_ids: Arc<dyn UlidSource>,
    kms: Arc<KmsEngine>,
}

impl DurableChatApi {
    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: Future<Output = Result<T, EdgeError>>,
    {
        match Handle::try_current() {
            Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| self.runtime.block_on(future))
            }
            Ok(_) => Err(EdgeError::Unavailable(
                "Chat requires the Edge multi-thread runtime".into(),
            )),
            Err(_) => self.runtime.block_on(future),
        }
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
        Ok(conversation)
    }

    async fn create_public_conversation(
        &self,
        conversation: Conversation,
        client_nonce: &str,
        event_id: EventId,
        actor: Actor,
        now: Timestamp,
    ) -> Result<(Conversation, bool), EdgeError> {
        match self
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
                self.conversations
                    .find_public_by_name_topic(
                        &conversation.id.tenant,
                        &conversation.id.region,
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
                    .map(|existing| (existing, false))
                    .ok_or_else(|| {
                        EdgeError::Conflict(
                            "a topic with that channel and name already exists".into(),
                        )
                    })
            }
            Err(error) => Err(map_conversation_error(error)),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateConversationBody {
    channel: String,
    topic: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostMessageBody {
    content: String,
    client_nonce: Option<String>,
}

struct ConversationListHandler {
    api: DurableChatApi,
}

impl Handler for ConversationListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let (limit, cursor) = parse_page_query(&ctx.request.query)?;
        let rows = self.api.drive(async {
            self.api
                .conversations
                .list_public(
                    &ctx.principal.tenant.0,
                    &ctx.principal.region.0,
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
        Ok(no_store(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next, limit as usize),
        )))
    }
}

struct ConversationCreateHandler {
    api: DurableChatApi,
}

impl Handler for ConversationCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body: CreateConversationBody = parse_body(&ctx.request.body)?;
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
            parent_project: None,
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
        let (conversation, created) = self.api.drive(self.api.create_public_conversation(
            conversation,
            &client_nonce,
            event_id,
            Actor(event_principal),
            now,
        ))?;
        Ok(no_store(EdgeResponse::json(
            if created { 201 } else { 200 },
            &json!({ "conversation": conversation_json(&conversation), "durable": true }),
        )))
    }
}

struct MessageListHandler {
    api: DurableChatApi,
}

impl Handler for MessageListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let conversation_id = conversation_param(ctx)?.to_string();
        let (limit, before) = parse_messages_query(&ctx.request.query)?;
        let conversation = self.api.drive(
            self.api
                .public_conversation(ctx.principal, &conversation_id),
        )?;
        let range = before
            .as_deref()
            .map(|cursor| RangeCursor::Before(MessageId(cursor.to_string())))
            .unwrap_or(RangeCursor::Recent);
        let messages = self.api.drive(async {
            self.api
                .messages
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
        let viewer = event_actor_pseudonym(&ctx.principal.tenant.0, &ctx.principal.principal_id.0);
        let items = visible
            .iter()
            .map(|message| message_json(message, &viewer, self.api.kms.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "conversation": conversation_json(&conversation),
                "items": items,
                "page": { "next_cursor": next, "limit": limit },
            }),
        )))
    }
}

struct MessagePostHandler {
    api: DurableChatApi,
}

impl Handler for MessagePostHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let conversation_id = conversation_param(ctx)?.to_string();
        let body: PostMessageBody = parse_body(&ctx.request.body)?;
        validate_message(&body.content)?;
        let client_nonce = client_nonce(
            ctx.request,
            &ctx.principal.principal_id.0,
            body.client_nonce.as_deref(),
        )?;
        let conversation = self.api.drive(
            self.api
                .public_conversation(ctx.principal, &conversation_id),
        )?;
        let event_principal = pseudonymized_event_principal(&ctx.principal.tenant.0, ctx.principal);
        let author_kind = match ctx.principal.kind {
            PrincipalKind::Human => AuthorKind::Human,
            PrincipalKind::Agent { .. } => AuthorKind::Agent,
            PrincipalKind::Service => AuthorKind::Service,
        };
        let new_message = NewMessage {
            conv: conversation.id,
            thread_root_id: None,
            author: event_principal.principal_id.0.clone(),
            author_kind,
            body_inline: encrypt_message_body(
                self.api.kms.as_ref(),
                ctx.principal,
                &event_principal.principal_id.0,
                body.content.as_bytes(),
            )?,
            body_nodes: b"[]".to_vec(),
            client_nonce,
        };
        let now = now_timestamp();
        let message_id = self.api.drive(async {
            self.api
                .messages
                .append_co_commit(
                    self.api.object_ids.as_ref(),
                    new_message,
                    EventId(self.api.event_ids.mint().0),
                    Actor(event_principal),
                    now.clone(),
                    now,
                )
                .await
                .map_err(map_store_error)
        })?;
        Ok(no_store(EdgeResponse::json(
            201,
            &json!({ "message_id": message_id.as_str(), "durable": true }),
        )))
    }
}

pub fn register_chat(
    builder: GatewayBuilder,
    pool: PgPool,
    runtime: Handle,
    kms: Arc<KmsEngine>,
) -> GatewayBuilder {
    let api = DurableChatApi {
        conversations: PgConversationStore::new(pool.clone()),
        messages: PgMessageStore::new(pool, "edge", MESSAGE_TABLE),
        runtime,
        event_ids: Arc::new(myelin_events::UlidMinter::new()),
        object_ids: Arc::new(SystemUlidSource::new()),
        kms,
    };
    builder
        .route(
            Method::Get,
            "/v1/chat/conversations",
            "chat.conversations.list",
            Arc::new(ConversationListHandler { api: api.clone() }),
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
            Arc::new(MessageListHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/chat/conversations/{conversation}/messages",
            "chat.message.post",
            Arc::new(MessagePostHandler { api }),
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
                    validate_ulid(value)?;
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
    if !(1..=MAX_PAGE_LIMIT).contains(&limit) {
        return Err(EdgeError::BadRequest(
            "Chat limit must be between 1 and 100".into(),
        ));
    }
    Ok((limit, cursor))
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

fn client_nonce(
    request: &crate::request::EdgeRequest,
    principal_id: &str,
    explicit: Option<&str>,
) -> Result<String, EdgeError> {
    match explicit {
        Some(value) => {
            validate_nonce(value)?;
            Ok(value.to_string())
        }
        None => request.stable_idempotency_nonce(principal_id),
    }
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
        "channel": conversation.name,
        "topic": conversation.topic,
        "linked_ref": conversation.linked_ref,
        "pinned_canvas": conversation.pinned_canvas,
    })
}

fn same_public_topic(left: &Conversation, right: &Conversation) -> bool {
    left.name == right.name && left.topic == right.topic
}

fn message_json(message: &Message, viewer: &str, kms: &KmsEngine) -> Result<Value, EdgeError> {
    let encrypted = decode_encrypted_body(&message.body_inline).map_err(|_| {
        EdgeError::Internal("stored Chat message has an invalid encrypted body".into())
    })?;
    if encrypted.key_ref.tenant.as_str() != message.conv.tenant
        || encrypted.key_ref.class != KeyClass::Subject(message.author.clone())
    {
        return Err(EdgeError::Internal(
            "stored Chat message encryption scope does not match its author".into(),
        ));
    }
    let plaintext = decrypt_body(
        kms,
        &myelin_tenancy::Region(message.conv.region.clone()),
        &encrypted,
    )
    .map_err(|_| EdgeError::Internal("stored Chat message cannot be decrypted".into()))?;
    let content = std::str::from_utf8(&plaintext)
        .map_err(|_| EdgeError::Internal("stored Chat message is not valid UTF-8".into()))?;
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

fn encrypt_message_body(
    kms: &KmsEngine,
    principal: &Principal,
    author: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, EdgeError> {
    let column = encrypt_body(
        kms,
        &principal.region,
        &principal.tenant,
        &SubjectId::new(author),
        ChatFreeText::BodyInline,
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
        assert!(validate_label("channel", "engineering").is_ok());
        assert!(validate_label("channel", " engineering").is_err());
        assert!(validate_message("Ship it\nwith care").is_ok());
        assert!(validate_message("  ").is_err());
        assert!(validate_message("bad\0message").is_err());
        assert!(validate_nonce("01J-client_nonce").is_ok());
        assert!(validate_nonce("spaces are not stable").is_err());
    }

    #[test]
    fn message_nonce_accepts_the_legacy_body_field_or_one_public_retry_key() {
        let request = crate::request::EdgeRequest::new(
            "POST",
            "/v1/chat/conversations/01J00000000000000000000000/messages",
            "",
            vec![("idempotency-key".into(), "send-42".into())],
            Vec::new(),
        );
        assert_eq!(
            client_nonce(&request, "svc:agent", Some("legacy-42")).unwrap(),
            "legacy-42"
        );
        assert_eq!(
            client_nonce(&request, "svc:agent", None).unwrap(),
            request.stable_idempotency_nonce("svc:agent").unwrap()
        );
    }
}
