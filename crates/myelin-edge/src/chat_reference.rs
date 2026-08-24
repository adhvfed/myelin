use myelin_chat::conversation::{Conversation, ConversationError};
use myelin_chat::store::pg_conversation::PgConversationStore;
use myelin_identity::{Principal, PrincipalStatus};
use sqlx::PgPool;
use tokio::runtime::Handle;

use crate::runtime::drive_result_on_runtime;

#[derive(Clone)]
pub struct DurableChatReferenceApi {
    conversations: PgConversationStore,
    runtime: Handle,
}

impl DurableChatReferenceApi {
    pub fn new(pool: PgPool, runtime: Handle) -> Self {
        Self {
            conversations: PgConversationStore::new(pool),
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
}
