use std::sync::Arc;

use myelin_chat::store::{MessageId, MessageState, RangeCursor, TimelineMessage};
use myelin_identity::Principal;
use serde_json::{json, Value};

use super::{
    conversation_json, map_store_error, message_param, no_store, parse_body, parse_messages_query,
    require_empty_query, validate_query, validate_ulid, DurableChatMutationApi, DurableChatReadApi,
    MessageInput,
};
use crate::catalogue::{Handler, HandlerCtx};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::Method;

impl DurableChatReadApi {
    pub fn read_thread(
        &self,
        principal: &Principal,
        root_message_id: &str,
        limit: u32,
        before: Option<String>,
    ) -> Result<Value, EdgeError> {
        validate_ulid(root_message_id)?;
        validate_query(limit, before.as_deref())?;
        let root_message_id = MessageId(root_message_id.to_owned());
        let root = self
            .drive(async {
                self.messages
                    .get_exact(principal.tenant.as_str(), &root_message_id)
                    .await
                    .map_err(map_store_error)
            })?
            .ok_or_else(|| EdgeError::NotFound("thread not found".into()))?;
        let conversation =
            self.drive(self.visible_conversation(principal, &root.conv.conversation_id))?;
        if root.thread_root_id.is_some() {
            return Err(EdgeError::NotFound("thread not found".into()));
        }
        let range = before
            .map(|cursor| RangeCursor::Before(MessageId(cursor)))
            .unwrap_or(RangeCursor::Recent);
        let replies = self.drive(async {
            self.messages
                .range_replies(&conversation.id, &root_message_id, range, limit + 1)
                .await
                .map_err(map_store_error)
        })?;
        let has_more = replies.len() > limit as usize;
        let start = replies.len().saturating_sub(limit as usize);
        let visible_replies = &replies[start..];
        let next = has_more
            .then(|| {
                visible_replies
                    .first()
                    .map(|reply| reply.message.message_id.0.clone())
            })
            .flatten();
        let reply_count = self.drive(async {
            self.messages
                .reply_count(&conversation.id, &root_message_id)
                .await
                .map_err(map_store_error)
        })?;
        let following = self.drive(async {
            self.messages
                .thread_following(&conversation.id, &root_message_id, &principal.principal_id)
                .await
                .map_err(map_store_error)
        })?;
        let mut timeline = Vec::with_capacity(visible_replies.len() + 1);
        timeline.push(TimelineMessage {
            message: root,
            reply_count,
        });
        timeline.extend_from_slice(visible_replies);
        let mut rendered = self.render_timeline(principal, &timeline)?.into_iter();
        let root = rendered
            .next()
            .expect("an authorized Chat thread always renders its root");
        let items = rendered.collect::<Vec<_>>();
        let reference =
            myelin_chat::subs::mint_thread(principal.tenant.as_str(), root_message_id.as_str())
                .map_err(|error| {
                    EdgeError::Internal(format!("mint Chat thread reference: {error}"))
                })?;
        Ok(json!({
            "conversation": conversation_json(&conversation),
            "ref": myelin_refs::format(&reference),
            "following": following,
            "root": root,
            "items": items,
            "page": { "next_cursor": next, "limit": limit },
        }))
    }
}

impl DurableChatMutationApi {
    fn set_thread_following(
        &self,
        principal: &Principal,
        root_message_id: &str,
        following: bool,
    ) -> Result<(), EdgeError> {
        validate_ulid(root_message_id)?;
        let root_message_id = MessageId(root_message_id.to_owned());
        let root = self
            .drive(async {
                self.reads
                    .messages
                    .get_exact(principal.tenant.as_str(), &root_message_id)
                    .await
                    .map_err(map_store_error)
            })?
            .ok_or_else(|| EdgeError::NotFound("thread not found".into()))?;
        let conversation = self.drive(
            self.reads
                .visible_conversation(principal, &root.conv.conversation_id),
        )?;
        if root.thread_root_id.is_some() {
            return Err(EdgeError::NotFound("thread not found".into()));
        }
        self.drive(async {
            self.reads
                .messages
                .set_thread_following(
                    &conversation.id,
                    &root_message_id,
                    &principal.principal_id,
                    following,
                )
                .await
                .map_err(map_store_error)
        })
    }

    fn post_reply_input(
        &self,
        actor: &Principal,
        authorized_viewer: &Principal,
        root_message_id: &str,
        input: &MessageInput,
        client_nonce: String,
    ) -> Result<MessageId, EdgeError> {
        self.validate_message_intent(actor, authorized_viewer, input, &client_nonce)?;
        validate_ulid(root_message_id)?;
        let root_message_id = MessageId(root_message_id.to_owned());
        let root = self
            .drive(async {
                self.reads
                    .messages
                    .get_exact(authorized_viewer.tenant.as_str(), &root_message_id)
                    .await
                    .map_err(map_store_error)
            })?
            .ok_or_else(|| EdgeError::NotFound("thread not found".into()))?;
        let conversation = self.drive(
            self.reads
                .postable_conversation(authorized_viewer, &root.conv.conversation_id),
        )?;
        if root.thread_root_id.is_some() {
            return Err(EdgeError::NotFound("thread not found".into()));
        }
        if matches!(root.state, MessageState::Deleted | MessageState::Tombstoned) {
            return Err(EdgeError::Conflict(
                "a removed Chat message cannot accept new replies".into(),
            ));
        }
        self.append_message(
            actor,
            conversation,
            Some(root_message_id),
            input,
            client_nonce,
        )
    }
}

struct ThreadMessagesHandler {
    api: DurableChatReadApi,
}

impl Handler for ThreadMessagesHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let root_message_id = thread_param(ctx)?.to_string();
        let (limit, before) = parse_messages_query(&ctx.request.query)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self
                .api
                .read_thread(ctx.principal, &root_message_id, limit, before)?,
        )))
    }
}

struct ReplyPostHandler {
    api: DurableChatMutationApi,
}

struct ThreadFollowHandler {
    api: DurableChatMutationApi,
    following: bool,
}

impl Handler for ThreadFollowHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        require_optional_empty_body(ctx)?;
        let root_message_id = thread_param(ctx)?;
        self.api
            .set_thread_following(ctx.principal, root_message_id, self.following)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({ "following": self.following, "durable": true }),
        )))
    }
}

impl Handler for ReplyPostHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body: MessageInput = parse_body(&ctx.request.body)?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let message_id = self.api.post_reply_input(
            ctx.principal,
            ctx.principal,
            message_param(ctx)?,
            &body,
            client_nonce,
        )?;
        Ok(no_store(EdgeResponse::json(
            201,
            &json!({ "message_id": message_id.as_str(), "durable": true }),
        )))
    }
}

pub(super) fn register_routes(
    builder: GatewayBuilder,
    reads: DurableChatReadApi,
    mutations: DurableChatMutationApi,
) -> GatewayBuilder {
    builder
        .route(
            Method::Get,
            "/v1/chat/threads/{thread}/messages",
            "chat.thread.messages.list",
            Arc::new(ThreadMessagesHandler { api: reads }),
        )
        .route(
            Method::Post,
            "/v1/chat/messages/{message}/replies",
            "chat.reply.post",
            Arc::new(ReplyPostHandler {
                api: mutations.clone(),
            }),
        )
        .route(
            Method::Put,
            "/v1/chat/threads/{thread}/follow",
            "chat.thread.follow",
            Arc::new(ThreadFollowHandler {
                api: mutations.clone(),
                following: true,
            }),
        )
        .route(
            Method::Delete,
            "/v1/chat/threads/{thread}/follow",
            "chat.thread.mute",
            Arc::new(ThreadFollowHandler {
                api: mutations,
                following: false,
            }),
        )
}

fn require_optional_empty_body(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.body.is_empty() {
        return Ok(());
    }
    crate::request::require_empty_json_object(
        &ctx.request.body,
        "Chat thread follow",
        super::MAX_CHAT_JSON_BYTES,
    )
}

fn thread_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let value = ctx
        .params
        .get("thread")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a thread id".into()))?;
    validate_ulid(value)?;
    Ok(value)
}
