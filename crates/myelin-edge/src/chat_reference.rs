use myelin_chat::conversation::{Conversation, ConversationError};
use myelin_chat::store::pg::PgMessageStore;
use myelin_chat::store::pg_conversation::PgConversationStore;
use myelin_chat::store::{MessageId, MessageState};
use myelin_identity::{Principal, PrincipalStatus};
use sqlx::PgPool;
use std::collections::BTreeMap;
use tokio::runtime::Handle;

use crate::runtime::drive_result_on_runtime;

#[derive(Clone)]
pub struct DurableChatReferenceApi {
    conversations: PgConversationStore,
    messages: PgMessageStore,
    runtime: Handle,
}

pub(crate) struct ChatMessageProjection {
    pub(crate) message_id: String,
    pub(crate) conversation_topic: String,
    pub(crate) thread_root_id: Option<String>,
    pub(crate) state: MessageState,
}

impl DurableChatReferenceApi {
    pub fn new(pool: PgPool, region: impl Into<String>, runtime: Handle) -> Self {
        Self {
            conversations: PgConversationStore::new(pool.clone()),
            messages: PgMessageStore::new(
                pool,
                region,
                myelin_chat::store::pg_conversation::MESSAGE_TABLE,
            ),
            runtime,
        }
    }

    pub(crate) fn project_conversations(
        &self,
        principal: &Principal,
        conversation_ids: &[String],
    ) -> Result<Vec<Conversation>, ConversationError> {
        if principal.status != PrincipalStatus::Active {
            return Ok(Vec::new());
        }
        drive_result_on_runtime(
            &self.runtime,
            self.conversations.get_visible_exact(
                principal.tenant.as_str(),
                principal.region.as_str(),
                principal.principal_id.0.as_str(),
                conversation_ids,
            ),
            ConversationError::Storage("Chat projection requires a multi-thread runtime".into()),
        )
    }

    pub(crate) fn project_messages(
        &self,
        principal: &Principal,
        message_ids: &[MessageId],
    ) -> Result<Vec<ChatMessageProjection>, ConversationError> {
        if principal.status != PrincipalStatus::Active || message_ids.is_empty() {
            return Ok(Vec::new());
        }
        drive_result_on_runtime(
            &self.runtime,
            async {
                let locations = self
                    .messages
                    .locate_exact(principal.tenant.as_str(), message_ids)
                    .await
                    .map_err(|error| {
                        ConversationError::Storage(format!(
                            "read exact Chat message coordinates: {error}"
                        ))
                    })?;
                let conversation_ids = locations
                    .iter()
                    .map(|location| location.conv.conversation_id.clone())
                    .collect::<Vec<_>>();
                let conversations = self
                    .conversations
                    .get_visible_exact(
                        principal.tenant.as_str(),
                        principal.region.as_str(),
                        principal.principal_id.0.as_str(),
                        &conversation_ids,
                    )
                    .await?;
                let topics = conversations
                    .into_iter()
                    .filter_map(|conversation| {
                        conversation
                            .topic
                            .map(|topic| (conversation.id.conversation_id, topic))
                    })
                    .collect::<BTreeMap<_, _>>();
                Ok(locations
                    .into_iter()
                    .filter_map(|location| {
                        topics.get(&location.conv.conversation_id).map(|topic| {
                            ChatMessageProjection {
                                message_id: location.message_id.0,
                                conversation_topic: topic.clone(),
                                thread_root_id: location.thread_root_id.map(|id| id.0),
                                state: location.state,
                            }
                        })
                    })
                    .collect())
            },
            ConversationError::Storage("Chat projection requires a multi-thread runtime".into()),
        )
    }
}
